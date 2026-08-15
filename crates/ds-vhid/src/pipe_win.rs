//! Named pipe server for native Windows hosts (`\\.\pipe\couchlink-ds-vhid`).

use std::os::windows::io::{FromRawHandle, IntoRawHandle, OwnedHandle};
use std::time::Duration;

use anyhow::{Context, Result};
use couchlink_pad::vhid_proto::VHID_PIPE_NAME;
use tracing::{info, warn};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL, INVALID_HANDLE_VALUE,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};

use crate::backend;
use crate::session::{self, OutputHub};

const PIPE_SDDL: &str = "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;AU)";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

struct PipeSd {
    sd: PSECURITY_DESCRIPTOR,
}

impl PipeSd {
    fn new() -> Result<Self> {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        let sddl = wide(PIPE_SDDL);
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
            .context("ConvertStringSecurityDescriptorToSecurityDescriptorW")?;
        }
        Ok(Self { sd })
    }

    fn attrs(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.sd.0 as *mut _,
            bInheritHandle: false.into(),
        }
    }
}

impl Drop for PipeSd {
    fn drop(&mut self) {
        if !self.sd.is_invalid() {
            unsafe {
                let _ = LocalFree(HLOCAL(self.sd.0 as *mut _));
            }
        }
    }
}

pub fn serve_pipe(backend_kind: crate::BackendKind) -> Result<()> {
    let name = wide(VHID_PIPE_NAME);
    let pipe_sd = PipeSd::new()?;
    info!("listening named pipe {VHID_PIPE_NAME}");
    let mut next_player: u32 = 1;
    loop {
        let mut sa = pipe_sd.attrs();
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                4096,
                4096,
                0,
                Some(&mut sa),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            anyhow::bail!(
                "CreateNamedPipeW failed: {:?}",
                std::io::Error::last_os_error()
            );
        }

        let connected = unsafe { ConnectNamedPipe(handle, None) };
        match connected {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_PIPE_CONNECTED.to_hresult() => {}
            Err(e) => {
                let _ = unsafe { CloseHandle(handle) };
                warn!("ConnectNamedPipe: {e}");
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
        }

        info!("pipe client connected");
        let player = next_player;
        next_player += 1;
        // Each pipe client is one player's pad — a fresh backend/hub per
        // connection, same reasoning as the TCP listener below.
        let hub = OutputHub::new();
        let backend = match backend::create(backend_kind, hub.clone()) {
            Ok(b) => b,
            Err(e) => {
                warn!("player {player}: virtual pad create failed: {e:#}");
                let _ = unsafe { CloseHandle(handle) };
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        // Move HANDLE into a std File for Read/Write.
        let file = unsafe {
            let owned = OwnedHandle::from_raw_handle(handle.0 as *mut _);
            std::fs::File::from(owned)
        };
        std::thread::spawn(move || {
            let reader = match file.try_clone() {
                Ok(r) => r,
                Err(e) => {
                    warn!("pipe clone: {e}");
                    return;
                }
            };
            let writer = file;
            if let Err(e) = session::serve_duplex(reader, writer, backend, hub) {
                warn!("pipe session (player {player}): {e:#}");
            }
            // File drop closes the pipe instance; parent loop creates the next one.
        });
        std::thread::sleep(Duration::from_millis(10));
    }
}
