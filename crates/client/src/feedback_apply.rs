//! Apply PadFeedback to the local DualSense (rumble / lightbar / adaptive triggers).

use anyhow::Result;
use couchlink_pad::feedback::build_usb_output_report;
use couchlink_proto::PadFeedback;
use tracing::debug;

use crate::dualsense_reader::DualSenseReader;

pub fn apply_feedback(reader: Option<&mut DualSenseReader>, fb: &PadFeedback) -> Result<()> {
    debug!("pad feedback: {fb:?}");
    let Some(reader) = reader else {
        // No DualSense open (Xbox/keyboard) — still validate we can pack the report.
        let _ = build_usb_output_report(fb);
        return Ok(());
    };
    reader.apply_feedback(fb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_without_reader_is_ok() {
        let fb = PadFeedback::Rumble {
            large: 10,
            small: 20,
        };
        apply_feedback(None, &fb).unwrap();
    }

    #[test]
    fn closed_feedback_sender_recv_is_disconnected() {
        // Contract for the client select! loop: after senders drop, try_recv is
        // Disconnected — the loop must set feedback_rx = None to avoid busy-spin.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PadFeedback>();
        drop(tx);
        assert!(matches!(
            rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
        ));
    }
}
