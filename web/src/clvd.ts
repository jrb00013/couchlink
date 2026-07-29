/** Binary CLVD video frames — must match crates/proto/src/video_frame.rs */

export const VIDEO_CHANNEL = "video";
export const VIDEO_MAGIC = "CLVD";
export const VIDEO_VERSION = 2;
export const FLAG_KEYFRAME = 1 << 0;
export const VIDEO_HEADER_LEN = 18;

export type VideoAccessUnit = {
  seq: number;
  width: number;
  height: number;
  keyframe: boolean;
  annexB: Uint8Array;
};

export type VideoFragment = {
  seq: number;
  width: number;
  height: number;
  keyframe: boolean;
  fragIdx: number;
  fragCount: number;
  payload: Uint8Array;
};

export function decodeClvdFragment(
  buf: ArrayBuffer | ArrayBufferView
): VideoFragment | null {
  const u8 =
    buf instanceof ArrayBuffer
      ? new Uint8Array(buf)
      : new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength);
  if (u8.byteLength < VIDEO_HEADER_LEN) return null;
  const magic = String.fromCharCode(u8[0], u8[1], u8[2], u8[3]);
  if (magic !== VIDEO_MAGIC) return null;
  if (u8[4] !== VIDEO_VERSION) return null;
  const flags = u8[5];
  const view = new DataView(u8.buffer, u8.byteOffset, u8.byteLength);
  const width = view.getUint16(6, true);
  const height = view.getUint16(8, true);
  const seq = view.getUint32(10, true);
  const fragIdx = view.getUint16(14, true);
  const fragCount = view.getUint16(16, true);
  if (fragCount === 0 || fragIdx >= fragCount) return null;
  return {
    seq,
    width,
    height,
    keyframe: (flags & FLAG_KEYFRAME) !== 0,
    fragIdx,
    fragCount,
    payload: u8.subarray(VIDEO_HEADER_LEN),
  };
}

/** Reassemble unordered CLVD fragments into a full access unit. */
export class ClvdAssembler {
  private seq: number | null = null;
  private width = 0;
  private height = 0;
  private keyframe = false;
  private fragCount = 0;
  private parts: (Uint8Array | null)[] = [];

  push(frag: VideoFragment): VideoAccessUnit | null {
    if (this.seq !== frag.seq) {
      this.seq = frag.seq;
      this.width = frag.width;
      this.height = frag.height;
      this.keyframe = frag.keyframe;
      this.fragCount = frag.fragCount;
      this.parts = Array.from({ length: frag.fragCount }, () => null);
    }
    if (frag.fragCount !== this.fragCount || frag.fragIdx >= this.parts.length) {
      return null;
    }
    this.parts[frag.fragIdx] = frag.payload;
    if (this.parts.some((p) => p === null)) return null;

    let total = 0;
    for (const p of this.parts) total += p!.byteLength;
    const annexB = new Uint8Array(total);
    let o = 0;
    for (const p of this.parts) {
      annexB.set(p!, o);
      o += p!.byteLength;
    }
    const au: VideoAccessUnit = {
      seq: frag.seq,
      width: this.width,
      height: this.height,
      keyframe: this.keyframe,
      annexB,
    };
    this.seq = null;
    this.parts = [];
    return au;
  }
}

/** Ask the host for a fresh IDR (any payload on the video DC). */
export const PLI_BYTES = new Uint8Array([0x50, 0x4c, 0x49]); // "PLI"
