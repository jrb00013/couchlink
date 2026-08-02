//! Named pipe server for helper requests (Windows only).

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL, INVALID_HANDLE_VALUE,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{
    FlushFileBuffers, ReadFile, WriteFile, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};

use crate::ops::handle_request;
use crate::protocol::{parse_request_line, response_line};

pub const PIPE_NAME: &str = r"\\.\pipe\couchlink-helper";

/// LocalSystem + Admins + Authenticated Users (GENERIC_READ|WRITE via GA).
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

/// Accept clients forever; one connection at a time.
pub fn serve_pipe(script_dir: &Path) -> Result<()> {
    let name = wide(PIPE_NAME);
    let pipe_sd = PipeSd::new()?;
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
                anyhow::bail!("ConnectNamedPipe: {e}");
            }
        }

        if let Err(e) = handle_client(handle, script_dir) {
            eprintln!("couchlink-helper: client error: {e:#}");
        }

        let _ = unsafe { DisconnectNamedPipe(handle) };
        let _ = unsafe { CloseHandle(handle) };
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn handle_client(handle: HANDLE, script_dir: &Path) -> Result<()> {
    let mut buf = Vec::with_capacity(512);
    let mut tmp = [0u8; 256];
    loop {
        let mut read = 0u32;
        unsafe {
            ReadFile(handle, Some(&mut tmp), Some(&mut read), None).context("ReadFile")?;
        }
        if read == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..read as usize]);
        if buf.contains(&b'\n') {
            break;
        }
        if buf.len() > 64 * 1024 {
            anyhow::bail!("request too large");
        }
    }

    let line = String::from_utf8_lossy(&buf);
    let line = line.lines().next().unwrap_or("").trim();
    let resp = match parse_request_line(line) {
        Ok(req) => handle_request(&req, script_dir),
        Err(e) => crate::protocol::Response::err(None, format!("bad request: {e}")),
    };
    let out = response_line(&resp);
    let mut written = 0u32;
    unsafe {
        WriteFile(handle, Some(out.as_bytes()), Some(&mut written), None).context("WriteFile")?;
        let _ = FlushFileBuffers(handle);
    }
    Ok(())
}
