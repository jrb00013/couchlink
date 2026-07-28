//! TCP wire format for Windows → WSL (or any) raw BGRA screen frames.

use anyhow::{bail, Context, Result};
use std::io::{Read, Write};

pub const FRAME_MAGIC: &[u8; 4] = b"CLFR";

#[derive(Debug, Clone, Copy)]
pub struct FrameInfo {
    pub width: u32,
    pub height: u32,
}

pub fn read_frame_sync(r: &mut impl Read, buf: &mut Vec<u8>) -> Result<FrameInfo> {
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic).context("frame magic")?;
    if &magic != FRAME_MAGIC {
        bail!("bad frame magic {:?}", magic);
    }
    let mut wh = [0u8; 8];
    r.read_exact(&mut wh).context("frame wh")?;
    let width = u32::from_le_bytes(wh[0..4].try_into()?);
    let height = u32::from_le_bytes(wh[4..8].try_into()?);
    let mut len_b = [0u8; 4];
    r.read_exact(&mut len_b).context("frame len")?;
    let len = u32::from_le_bytes(len_b) as usize;
    let expected = width as usize * height as usize * 4;
    if len != expected || len == 0 || len > 64 * 1024 * 1024 {
        bail!("invalid frame size {len} for {width}x{height}");
    }
    buf.resize(len, 0);
    r.read_exact(buf).context("frame payload")?;
    Ok(FrameInfo { width, height })
}

pub fn write_frame_sync(
    w: &mut impl Write,
    width: u32,
    height: u32,
    bgra: &[u8],
) -> Result<()> {
    let len = bgra.len() as u32;
    w.write_all(FRAME_MAGIC)?;
    w.write_all(&width.to_le_bytes())?;
    w.write_all(&height.to_le_bytes())?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(bgra)?;
    w.flush()?;
    Ok(())
}
