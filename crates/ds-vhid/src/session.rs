//! Fan-out hub for companion→host HID output (DSVO) and session I/O.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use couchlink_pad::vhid_proto::{decode_input, encode_output, DSVH_MAGIC, DS_USB_INPUT_LEN};
use tracing::{info, warn};

use crate::backend::PadBackend;

type DynBackend = Arc<Mutex<dyn PadBackend>>;

/// Broadcasts output reports to all active host sessions.
#[derive(Clone, Default)]
pub struct OutputHub {
    inner: Arc<Mutex<Vec<std::sync::mpsc::Sender<Vec<u8>>>>>,
}

impl OutputHub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(&self) -> std::sync::mpsc::Receiver<Vec<u8>> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.inner.lock().unwrap().push(tx);
        rx
    }

    pub fn broadcast(&self, report: Vec<u8>) {
        let mut guard = self.inner.lock().unwrap();
        guard.retain(|tx| tx.send(report.clone()).is_ok());
    }
}

pub fn serve_tcp(stream: TcpStream, backend: DynBackend, hub: OutputHub) -> Result<()> {
    stream.set_nodelay(true)?;
    info!("TCP client {}", stream.peer_addr()?);
    // Split into owned halves via try_clone for reader/writer threads.
    let writer = stream.try_clone().context("clone TCP stream")?;
    let reader = stream;
    serve_duplex(reader, writer, backend, hub)
}

pub fn serve_duplex<R, W>(
    mut reader: R,
    mut writer: W,
    backend: DynBackend,
    hub: OutputHub,
) -> Result<()>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    let out_rx = hub.subscribe();
    let write_thread = std::thread::spawn(move || -> Result<()> {
        while let Ok(report) = out_rx.recv() {
            let frame = encode_output(&report);
            writer.write_all(&frame)?;
            writer.flush()?;
        }
        Ok(())
    });

    let mut buf = vec![0u8; 4 + 1 + DS_USB_INPUT_LEN];
    loop {
        match reader.read_exact(&mut buf) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                info!("host disconnected");
                break;
            }
            Err(e) => {
                warn!("read: {e}");
                break;
            }
        }
        if &buf[0..4] != DSVH_MAGIC {
            warn!("bad DSVH magic");
            break;
        }
        let report = decode_input(&buf)?;
        backend
            .lock()
            .unwrap()
            .apply_ds_report(&report)
            .context("apply report")?;
    }

    // Dropping hub subscription senders happens when this function returns;
    // wake writer by disconnecting — channel closes when subscribe sender dropped
    // only if we remove it; broadcast retain handles dead senders.
    let _ = write_thread.join();
    // Give writer a moment if blocked
    let _ = Duration::from_millis(1);
    Ok(())
}
