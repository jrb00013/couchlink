//! DualSense VHID framing between couchlink-host and the Windows companion.
//!
//! Host → companion (input):  `DSVH` + ver + 64-byte USB input report
//! Companion → host (output): `DSVO` + ver + u16le len + HID output bytes
//! Host → companion (hello):  `DSVS` + ver + 1-byte slot — sent once, first,
//! right after connecting, so the companion routes this connection's frames
//! into that slot's own pre-existing virtual controller (see module docs on
//! the companion side for why this can't be inferred from connection order).

use thiserror::Error;

pub const DSVH_MAGIC: &[u8; 4] = b"DSVH";
pub const DSVO_MAGIC: &[u8; 4] = b"DSVO";
pub const DSVS_MAGIC: &[u8; 4] = b"DSVS";
pub const VHID_VERSION: u8 = 1;
pub const DS_USB_INPUT_LEN: usize = 64;
/// Default TCP port so WSL hosts can reach the Windows companion via localhost.
pub const VHID_TCP_PORT: u16 = 39251;
pub const VHID_PIPE_NAME: &str = r"\\.\pipe\couchlink-ds-vhid";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum VhidCodecError {
    #[error("buffer too short")]
    Short,
    #[error("bad magic")]
    BadMagic,
    #[error("unsupported version {0}")]
    BadVersion(u8),
}

pub fn encode_slot_hello(slot: u8) -> Vec<u8> {
    let mut buf = Vec::with_capacity(6);
    buf.extend_from_slice(DSVS_MAGIC);
    buf.push(VHID_VERSION);
    buf.push(slot);
    buf
}

pub fn decode_slot_hello(buf: &[u8]) -> Result<u8, VhidCodecError> {
    if buf.len() < 6 {
        return Err(VhidCodecError::Short);
    }
    if &buf[0..4] != DSVS_MAGIC {
        return Err(VhidCodecError::BadMagic);
    }
    if buf[4] != VHID_VERSION {
        return Err(VhidCodecError::BadVersion(buf[4]));
    }
    Ok(buf[5])
}

pub fn encode_input(report: &[u8; DS_USB_INPUT_LEN]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4 + 1 + DS_USB_INPUT_LEN);
    buf.extend_from_slice(DSVH_MAGIC);
    buf.push(VHID_VERSION);
    buf.extend_from_slice(report);
    buf
}

pub fn decode_input(buf: &[u8]) -> Result<[u8; DS_USB_INPUT_LEN], VhidCodecError> {
    if buf.len() < 4 + 1 + DS_USB_INPUT_LEN {
        return Err(VhidCodecError::Short);
    }
    if &buf[0..4] != DSVH_MAGIC {
        return Err(VhidCodecError::BadMagic);
    }
    if buf[4] != VHID_VERSION {
        return Err(VhidCodecError::BadVersion(buf[4]));
    }
    let mut report = [0u8; DS_USB_INPUT_LEN];
    report.copy_from_slice(&buf[5..5 + DS_USB_INPUT_LEN]);
    Ok(report)
}

pub fn encode_output(report: &[u8]) -> Vec<u8> {
    let len = report.len().min(u16::MAX as usize) as u16;
    let mut buf = Vec::with_capacity(4 + 1 + 2 + len as usize);
    buf.extend_from_slice(DSVO_MAGIC);
    buf.push(VHID_VERSION);
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(&report[..len as usize]);
    buf
}

pub fn decode_output(buf: &[u8]) -> Result<Vec<u8>, VhidCodecError> {
    if buf.len() < 4 + 1 + 2 {
        return Err(VhidCodecError::Short);
    }
    if &buf[0..4] != DSVO_MAGIC {
        return Err(VhidCodecError::BadMagic);
    }
    if buf[4] != VHID_VERSION {
        return Err(VhidCodecError::BadVersion(buf[4]));
    }
    let len = u16::from_le_bytes([buf[5], buf[6]]) as usize;
    if buf.len() < 7 + len {
        return Err(VhidCodecError::Short);
    }
    Ok(buf[7..7 + len].to_vec())
}

/// Try next complete DSVO frame from a growable buffer; leaves remainder.
pub fn take_output_frame(buf: &mut Vec<u8>) -> Option<Vec<u8>> {
    if buf.len() < 7 {
        return None;
    }
    if &buf[0..4] != DSVO_MAGIC {
        // Resync: drop one byte
        buf.remove(0);
        return None;
    }
    if buf[4] != VHID_VERSION {
        buf.remove(0);
        return None;
    }
    let len = u16::from_le_bytes([buf[5], buf[6]]) as usize;
    if buf.len() < 7 + len {
        return None;
    }
    let frame = buf[7..7 + len].to_vec();
    let _ = buf.drain(..7 + len);
    Some(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_roundtrip() {
        let mut r = [0u8; DS_USB_INPUT_LEN];
        r[0] = 0x01;
        r[8] = 0x20;
        let enc = encode_input(&r);
        assert_eq!(decode_input(&enc).unwrap(), r);
    }

    #[test]
    fn output_roundtrip_and_take() {
        let report = vec![0x02, 0xff, 0x01, 40, 120];
        let enc = encode_output(&report);
        assert_eq!(decode_output(&enc).unwrap(), report);
        let mut buf = enc;
        buf.extend_from_slice(&encode_output(&[9, 9]));
        assert_eq!(take_output_frame(&mut buf).unwrap(), report);
        assert_eq!(take_output_frame(&mut buf).unwrap(), vec![9, 9]);
        assert!(take_output_frame(&mut buf).is_none());
    }

    #[test]
    fn slot_hello_roundtrip() {
        let enc = encode_slot_hello(2);
        assert_eq!(decode_slot_hello(&enc).unwrap(), 2);
    }

    #[test]
    fn slot_hello_rejects_short_buffer() {
        assert_eq!(decode_slot_hello(&[1, 2, 3]), Err(VhidCodecError::Short));
    }

    #[test]
    fn slot_hello_rejects_wrong_magic() {
        assert_eq!(
            decode_slot_hello(b"DSVHxx"),
            Err(VhidCodecError::BadMagic)
        );
    }
}
