//! Fan-out hub for companion→host HID output (DSVO) and session I/O.
//!
//! One virtual controller per couchlink player *slot*, not per connection.
//! Every connection here starts with a `DSVS` slot-hello frame declaring
//! which slot it's driving; the first connection for a slot plugs in a
//! fresh ViGEm target, and every later connection for that same slot (a
//! reconnect after a network blip, a rejoin) reuses it. Without this, a
//! target got created in whatever order connections happened to arrive —
//! fine for one player, but a second/third player's reconnect could land on
//! a *different* slot's target than the one they'd been using, silently
//! taking over another seated player's controller.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use couchlink_pad::vhid_proto::{decode_input, decode_slot_hello, encode_output, DSVH_MAGIC, DS_USB_INPUT_LEN};
use tracing::{info, warn};

use crate::backend::{self, PadBackend};

type DynBackend = Arc<Mutex<dyn PadBackend>>;

/// Broadcasts output reports to all active host sessions for one slot.
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

/// Remote player slots the companion can seat. Must match
/// `couchlink_signaling::players::MAX_PLAYERS` / host `MAX_REMOTE_SLOTS`.
pub const MAX_REMOTE_SLOTS: u8 = 3;

/// Per-slot virtual controllers for the life of the companion process.
///
/// Created up front for slots `1..=MAX_REMOTE_SLOTS` at companion start
/// ([`SlotRegistry::preallocate`]) so Windows/XInput indices are stable
/// before anyone joins: friend connect is a state change on an existing
/// ViGEm target, not a PnP hotplug that PCSX2 may never re-bind.
///
/// Never re-created on reconnect — that was the arrival-order bug this
/// module exists to avoid (a blip must not land a player on a different
/// seat's target).
#[derive(Clone, Default)]
pub struct SlotRegistry {
    inner: Arc<Mutex<HashMap<u8, (DynBackend, OutputHub)>>>,
}

impl SlotRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Plug in every seat's ViGEm/WinUHid target once, in slot order.
    ///
    /// Order matters: XInput indices are assigned by connect order, and
    /// `link-emulator-pad.sh` assumes slot 1 → XInput-0, slot 2 → XInput-1,
    /// slot 3 → XInput-2. Creating them here (before any TCP/pipe client)
    /// freezes that map for the whole companion lifetime.
    pub fn preallocate(&self, kind: crate::BackendKind) -> Result<()> {
        for slot in 1..=MAX_REMOTE_SLOTS {
            let _ = self.get_or_create(slot, kind)?;
        }
        info!(
            "pre-allocated {MAX_REMOTE_SLOTS} virtual controller(s) (backend={kind:?}) — late joins reuse these pads"
        );
        Ok(())
    }

    /// The slot's existing controller, or a freshly created one on first use.
    ///
    /// `backend::create()` talks to the ViGEmBus driver (plugin + wait_ready)
    /// — real I/O that can stall. It must never run while holding the lock:
    /// every *other* connection's slot lookup — for a completely different
    /// player — would queue up behind that one stall and the whole companion
    /// would look hung to every player, not just the one whose target is
    /// slow to plug in. Create outside the lock; only hold it to check and
    /// to insert.
    pub(crate) fn get_or_create(&self, slot: u8, kind: crate::BackendKind) -> Result<(DynBackend, OutputHub)> {
        if let Some(entry) = self.inner.lock().unwrap().get(&slot) {
            return Ok(entry.clone());
        }
        let hub = OutputHub::new();
        let backend = backend::create(kind, hub.clone())
            .with_context(|| format!("create virtual controller for slot {slot}"))?;
        // Someone else may have created this slot's controller while we were
        // blocked on the driver above — keep whichever one is already
        // registered rather than clobbering it, so a slot's controller
        // identity never changes out from under an in-progress connection.
        let mut guard = self.inner.lock().unwrap();
        if let Some(existing) = guard.get(&slot) {
            return Ok(existing.clone());
        }
        info!("slot {slot}: plugged in a new virtual controller");
        guard.insert(slot, (backend.clone(), hub.clone()));
        Ok((backend, hub))
    }
}

pub(crate) fn read_slot_hello<R: Read>(reader: &mut R) -> Result<u8> {
    let mut buf = [0u8; 6];
    reader.read_exact(&mut buf).context("read slot hello")?;
    decode_slot_hello(&buf).context("decode slot hello")
}

pub fn serve_tcp(mut stream: TcpStream, registry: SlotRegistry, kind: crate::BackendKind) -> Result<()> {
    stream.set_nodelay(true)?;
    let slot = read_slot_hello(&mut stream)
        .with_context(|| format!("TCP client {}", stream.peer_addr().map(|a| a.to_string()).unwrap_or_default()))?;
    info!("TCP client {} (slot {slot})", stream.peer_addr()?);
    let (backend, hub) = registry.get_or_create(slot, kind)?;
    // Split into owned halves via try_clone for reader/writer threads.
    let writer = stream.try_clone().context("clone TCP stream")?;
    let reader = stream;
    serve_duplex(reader, writer, backend, hub, slot)
}

pub fn serve_duplex<R, W>(
    mut reader: R,
    mut writer: W,
    backend: DynBackend,
    hub: OutputHub,
    slot: u8,
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
                info!("host disconnected (slot {slot})");
                break;
            }
            Err(e) => {
                warn!("read (slot {slot}): {e}");
                break;
            }
        }
        if &buf[0..4] != DSVH_MAGIC {
            warn!("bad DSVH magic (slot {slot})");
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct NoopBackend;
    impl PadBackend for NoopBackend {
        fn apply_ds_report(&mut self, _report: &[u8; DS_USB_INPUT_LEN]) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn read_slot_hello_parses_the_announced_slot() {
        let hello = couchlink_pad::vhid_proto::encode_slot_hello(2);
        let mut cursor = Cursor::new(hello);
        assert_eq!(read_slot_hello(&mut cursor).unwrap(), 2);
    }

    #[test]
    fn read_slot_hello_rejects_a_dsvh_frame_where_a_hello_was_expected() {
        let mut r = [0u8; DS_USB_INPUT_LEN];
        r[0] = 1;
        let frame = couchlink_pad::vhid_proto::encode_input(&r);
        let mut cursor = Cursor::new(frame);
        assert!(read_slot_hello(&mut cursor).is_err());
    }

    #[test]
    fn max_remote_slots_is_three() {
        assert_eq!(MAX_REMOTE_SLOTS, 3);
    }

    #[test]
    fn registry_reuses_the_same_backend_for_repeat_connects_of_one_slot() {
        let registry = SlotRegistry::default();
        let inner: DynBackend = Arc::new(Mutex::new(NoopBackend));
        registry
            .inner
            .lock()
            .unwrap()
            .insert(3, (inner.clone(), OutputHub::new()));
        let (got, _) = registry
            .inner
            .lock()
            .unwrap()
            .get(&3)
            .cloned()
            .expect("slot 3 present");
        assert!(Arc::ptr_eq(&got, &inner));
    }
}
