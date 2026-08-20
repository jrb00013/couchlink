//! Windows-side half of the Hyper-V socket capture transport.
//!
//! Binds `AF_HYPERV` with `VmId = HV_GUID_ZERO` (the documented wildcard —
//! accept from whichever partition connects, so this side never needs to look
//! up the WSL VM's specific GUID) and a `ServiceId` built from
//! `HV_GUID_VSOCK_TEMPLATE` with the port folded into the low bits — the same
//! port-to-GUID mapping the Linux kernel's `AF_VSOCK`-over-Hyper-V transport
//! uses, so `VsockStream::connect_with_cid_port(VMADDR_CID_HOST, port)` on the
//! WSL2 side lands on exactly this listener with no coordination beyond
//! agreeing on the port number.
//!
//! This talks raw Winsock (no `std::net`, which has no `AF_HYPERV` support)
//! but exposes `Read + Write` so it drops straight into the existing
//! `write_frame_with_format`/`read_frame_body_sync` wire protocol unchanged.

use anyhow::{bail, Result};
use std::io::{Read, Write};
use windows::Win32::Networking::WinSock::{
    accept, bind, closesocket, listen, recv, send, socket, WSAGetLastError, WSAStartup, ADDRESS_FAMILY,
    SEND_RECV_FLAGS, SOCKADDR, SOCKET, SOCK_STREAM, WSADATA,
};
use windows::Win32::System::Hypervisor::{HV_GUID_VSOCK_TEMPLATE, HV_GUID_ZERO, SOCKADDR_HV};

const AF_HYPERV: i32 = 34;
const HV_PROTOCOL_RAW: i32 = 1;

fn service_id_for_port(port: u32) -> windows::core::GUID {
    let template = u128::from(HV_GUID_VSOCK_TEMPLATE);
    // Data1 (the leftmost 8 hex digits of the string form) is the top 32 bits.
    windows::core::GUID::from_u128((template & !(0xFFFF_FFFFu128 << 96)) | ((port as u128) << 96))
}

fn last_error(what: &str) -> anyhow::Error {
    let code = unsafe { WSAGetLastError() };
    anyhow::anyhow!("{what} failed: WSA error {}", code.0)
}

pub struct HvListener {
    sock: SOCKET,
}

// Safety: a raw Winsock SOCKET handle behaves like any OS socket handle —
// operations on it are thread-safe at the OS level, same guarantee std::net
// relies on for TcpListener/TcpStream.
unsafe impl Send for HvListener {}
unsafe impl Send for HvStream {}

impl HvListener {
    pub fn bind(port: u32) -> Result<Self> {
        unsafe {
            let mut wsadata = WSADATA::default();
            let started = WSAStartup(0x0202, &mut wsadata);
            if started != 0 {
                bail!("WSAStartup failed: {started}");
            }

            let sock = socket(AF_HYPERV, SOCK_STREAM, HV_PROTOCOL_RAW)
                .map_err(|e| anyhow::anyhow!("socket(AF_HYPERV): {e}"))?;

            let addr = SOCKADDR_HV {
                Family: ADDRESS_FAMILY(AF_HYPERV as u16),
                Reserved: 0,
                VmId: HV_GUID_ZERO,
                ServiceId: service_id_for_port(port),
            };
            let addr_ptr = &addr as *const SOCKADDR_HV as *const SOCKADDR;
            if bind(sock, addr_ptr, std::mem::size_of::<SOCKADDR_HV>() as i32) != 0 {
                let e = last_error("bind(AF_HYPERV)");
                closesocket(sock);
                return Err(e);
            }
            if listen(sock, 1) != 0 {
                let e = last_error("listen(AF_HYPERV)");
                closesocket(sock);
                return Err(e);
            }
            tracing::info!("Hyper-V socket listening on port {port} (VmId=wildcard)");
            Ok(Self { sock })
        }
    }

    /// Blocks until the WSL2 host connects.
    pub fn accept(&self) -> Result<HvStream> {
        unsafe {
            let sock = accept(self.sock, None, None)
                .map_err(|e| anyhow::anyhow!("accept(AF_HYPERV): {e}"))?;
            Ok(HvStream::new(sock))
        }
    }
}

impl Drop for HvListener {
    fn drop(&mut self) {
        unsafe {
            closesocket(self.sock);
        }
    }
}

/// Shared, refcounted handle so a reader thread (watching for `REQUEST_IDR`/
/// `SET_TARGET` commands, same as the TCP path's `watch_commands`) and the
/// writer thread can use the same underlying socket without a double-close —
/// a raw `SOCKET` is just a copyable handle, but only the last owner may call
/// `closesocket`.
pub struct HvStream {
    sock: std::sync::Arc<SocketHandle>,
}

struct SocketHandle(SOCKET);

impl Drop for SocketHandle {
    fn drop(&mut self) {
        unsafe {
            closesocket(self.0);
        }
    }
}

impl HvStream {
    fn new(sock: SOCKET) -> Self {
        Self {
            sock: std::sync::Arc::new(SocketHandle(sock)),
        }
    }

    /// A second handle onto the same socket, for a dedicated reader/writer
    /// thread — mirrors `TcpStream::try_clone`.
    pub fn try_clone(&self) -> Self {
        Self {
            sock: std::sync::Arc::clone(&self.sock),
        }
    }
}

impl Read for HvStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = unsafe { recv(self.sock.0, buf, SEND_RECV_FLAGS(0)) };
        if n < 0 {
            let code = unsafe { WSAGetLastError() };
            return Err(std::io::Error::other(format!("recv: WSA error {}", code.0)));
        }
        Ok(n as usize)
    }
}

impl Write for HvStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = unsafe { send(self.sock.0, buf, SEND_RECV_FLAGS(0)) };
        if n < 0 {
            let code = unsafe { WSAGetLastError() };
            return Err(std::io::Error::other(format!("send: WSA error {}", code.0)));
        }
        Ok(n as usize)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
