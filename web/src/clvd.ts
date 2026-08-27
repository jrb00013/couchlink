/** Binary CLVD video frames — must match crates/proto/src/video_frame.rs */

export const VIDEO_CHANNEL = "video";
export const VIDEO_MAGIC = "CLVD";
/** Tiny host→client watermark tip (no H.264) — magic + u32 LE input_wm. */
export const WM_TIP_MAGIC = "CLWM";
export const WM_TIP_LEN = 8;
export const VIDEO_VERSION = 3;
export const VIDEO_VERSION_V2 = 2;
export const VIDEO_VERSION_V4 = 4;
export const FLAG_KEYFRAME = 1 << 0;
export const VIDEO_HEADER_LEN_V2 = 18;
export const VIDEO_HEADER_LEN = 26;
export const VIDEO_HEADER_LEN_V4 = 30;

export type VideoAccessUnit = {
  seq: number;
  width: number;
  height: number;
  keyframe: boolean;
  annexB: Uint8Array;
  stampUs: number;
  inputWm: number;
};

export type VideoFragment = {
  seq: number;
  width: number;
  height: number;
  keyframe: boolean;
  fragIdx: number;
  fragCount: number;
  stampUs: number;
  inputWm: number;
  payload: Uint8Array;
};

/** Bytes at the front of the parity fragment's payload: last data fragment's length. */
const FEC_LEN_PREFIX = 2;
/** Must match `VIDEO_MAX_FRAGMENT_PAYLOAD` in crates/proto/src/video_frame.rs. */
export const VIDEO_MAX_FRAGMENT_PAYLOAD = 14_000;

function headerLen(ver: number): number {
  if (ver === VIDEO_VERSION_V4) return VIDEO_HEADER_LEN_V4;
  if (ver === VIDEO_VERSION) return VIDEO_HEADER_LEN;
  return VIDEO_HEADER_LEN_V2;
}

/** Decode an 8-byte `CLWM` tip → input_wm, or null. */
export function decodeWmTip(buf: ArrayBuffer | ArrayBufferView): number | null {
  const u8 =
    buf instanceof ArrayBuffer
      ? new Uint8Array(buf)
      : new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength);
  if (u8.byteLength < WM_TIP_LEN) return null;
  const magic = String.fromCharCode(u8[0], u8[1], u8[2], u8[3]);
  if (magic !== WM_TIP_MAGIC) return null;
  const view = new DataView(u8.buffer, u8.byteOffset, u8.byteLength);
  return view.getUint32(4, true) >>> 0;
}

export function decodeClvdFragment(
  buf: ArrayBuffer | ArrayBufferView
): VideoFragment | null {
  const u8 =
    buf instanceof ArrayBuffer
      ? new Uint8Array(buf)
      : new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength);
  if (u8.byteLength < VIDEO_HEADER_LEN_V2) return null;
  const magic = String.fromCharCode(u8[0], u8[1], u8[2], u8[3]);
  if (magic !== VIDEO_MAGIC) return null;
  const ver = u8[4];
  if (ver !== VIDEO_VERSION && ver !== VIDEO_VERSION_V2 && ver !== VIDEO_VERSION_V4) {
    return null;
  }
  const hlen = headerLen(ver);
  if (u8.byteLength < hlen) return null;
  const flags = u8[5];
  const view = new DataView(u8.buffer, u8.byteOffset, u8.byteLength);
  const width = view.getUint16(6, true);
  const height = view.getUint16(8, true);
  const seq = view.getUint32(10, true);
  const fragIdx = view.getUint16(14, true);
  const fragCount = view.getUint16(16, true);
  let stampUs = 0;
  let inputWm = 0;
  if (ver === VIDEO_VERSION_V4) {
    stampUs = Number(view.getBigUint64(18, true));
    inputWm = view.getUint32(26, true);
  } else if (ver === VIDEO_VERSION) {
    stampUs = Number(view.getBigUint64(18, true));
  }
  if (fragCount === 0 || fragIdx > fragCount) return null;
  return {
    seq,
    width,
    height,
    keyframe: (flags & FLAG_KEYFRAME) !== 0,
    fragIdx,
    fragCount,
    stampUs,
    inputWm,
    payload: u8.subarray(hlen),
  };
}

function recoverFragment(
  parts: (Uint8Array | null)[],
  missing: number,
  parity: Uint8Array | null
): Uint8Array | null {
  if (!parity || parity.byteLength < FEC_LEN_PREFIX + VIDEO_MAX_FRAGMENT_PAYLOAD) {
    return null;
  }
  const lastLen = parity[0] | (parity[1] << 8);
  const acc = new Uint8Array(VIDEO_MAX_FRAGMENT_PAYLOAD);
  for (let i = 0; i < acc.length; i++) acc[i] = parity[FEC_LEN_PREFIX + i];
  for (let i = 0; i < parts.length; i++) {
    if (i === missing) continue;
    const p = parts[i];
    if (!p) return null;
    for (let j = 0; j < p.length; j++) acc[j] ^= p[j];
  }
  const wantLen = missing + 1 === parts.length ? lastLen : VIDEO_MAX_FRAGMENT_PAYLOAD;
  if (wantLen > acc.length) return null;
  return acc.subarray(0, wantLen);
}

export class ClvdAssembler {
  private seq: number | null = null;
  private width = 0;
  private height = 0;
  private keyframe = false;
  private fragCount = 0;
  private stampUs = 0;
  private inputWm = 0;
  private parts: (Uint8Array | null)[] = [];
  private parity: Uint8Array | null = null;

  push(frag: VideoFragment): VideoAccessUnit | null {
    if (this.seq !== frag.seq) {
      this.seq = frag.seq;
      this.width = frag.width;
      this.height = frag.height;
      this.keyframe = frag.keyframe;
      this.fragCount = frag.fragCount;
      this.stampUs = frag.stampUs;
      this.inputWm = frag.inputWm;
      this.parts = Array.from({ length: frag.fragCount }, () => null);
      this.parity = null;
    }
    if (frag.fragCount !== this.fragCount) return null;
    if (frag.fragIdx === frag.fragCount) {
      this.parity = frag.payload;
    } else if (frag.fragIdx < this.parts.length) {
      this.parts[frag.fragIdx] = frag.payload;
    } else {
      return null;
    }

    const missing: number[] = [];
    for (let i = 0; i < this.parts.length; i++) {
      if (this.parts[i] === null) missing.push(i);
    }
    if (missing.length === 1) {
      const recovered = recoverFragment(this.parts, missing[0], this.parity);
      if (!recovered) return null;
      this.parts[missing[0]] = recovered;
    } else if (missing.length > 1) {
      return null;
    }

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
      stampUs: this.stampUs,
      inputWm: this.inputWm,
    };
    this.seq = null;
    this.parts = [];
    this.parity = null;
    return au;
  }
}

/** Ask the host for a fresh IDR (any payload on the video DC). */
export const PLI_BYTES = new Uint8Array([0x50, 0x4c, 0x49]); // "PLI"
