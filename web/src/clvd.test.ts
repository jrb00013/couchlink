import { describe, expect, it } from "vitest";
import {
  ClvdAssembler,
  decodeClvdFragment,
  VIDEO_HEADER_LEN,
  VIDEO_HEADER_LEN_V2,
  VIDEO_MAGIC,
  VIDEO_MAX_FRAGMENT_PAYLOAD,
  VIDEO_VERSION,
  VIDEO_VERSION_V2,
  FLAG_KEYFRAME,
} from "./clvd";

/**
 * Test-only encoder mirroring the host's Rust `encode_fragments_with_fec` —
 * the client never encodes CLVD in production, only decodes it, so this
 * exists purely to build fixtures matching the real wire format.
 */
function encodeFragment(
  seq: number,
  fragIdx: number,
  fragCount: number,
  keyframe: boolean,
  payload: Uint8Array
): ArrayBuffer {
  const buf = new ArrayBuffer(VIDEO_HEADER_LEN + payload.byteLength);
  const u8 = new Uint8Array(buf);
  const view = new DataView(buf);
  for (let i = 0; i < 4; i++) u8[i] = VIDEO_MAGIC.charCodeAt(i);
  u8[4] = VIDEO_VERSION;
  u8[5] = keyframe ? FLAG_KEYFRAME : 0;
  view.setUint16(6, 1920, true);
  view.setUint16(8, 1080, true);
  view.setUint32(10, seq, true);
  view.setUint16(14, fragIdx, true);
  view.setUint16(16, fragCount, true);
  view.setBigUint64(18, 0n, true);
  u8.set(payload, VIDEO_HEADER_LEN);
  return buf;
}

function makeAccessUnit(len: number): Uint8Array {
  const b = new Uint8Array(len);
  for (let i = 0; i < len; i++) b[i] = i % 251;
  return b;
}

/** Data fragments + one XOR parity fragment, matching the Rust encoder. */
function encodeWithFec(seq: number, annexB: Uint8Array): ArrayBuffer[] {
  const chunk = VIDEO_MAX_FRAGMENT_PAYLOAD;
  const fragCount = Math.max(1, Math.ceil(annexB.byteLength / chunk));
  const out: ArrayBuffer[] = [];
  const xor = new Uint8Array(chunk);
  for (let i = 0; i < fragCount; i++) {
    const piece = annexB.subarray(i * chunk, Math.min((i + 1) * chunk, annexB.byteLength));
    for (let j = 0; j < piece.length; j++) xor[j] ^= piece[j];
    out.push(encodeFragment(seq, i, fragCount, true, piece));
  }
  if (fragCount > 1) {
    const lastLen = annexB.byteLength - (fragCount - 1) * chunk;
    const parityPayload = new Uint8Array(2 + chunk);
    parityPayload[0] = lastLen & 0xff;
    parityPayload[1] = (lastLen >> 8) & 0xff;
    parityPayload.set(xor, 2);
    out.push(encodeFragment(seq, fragCount, fragCount, true, parityPayload));
  }
  return out;
}

describe("ClvdAssembler", () => {
  it("reassembles a single-fragment access unit", () => {
    const payload = new Uint8Array([1, 2, 3]);
    const asm = new ClvdAssembler();
    const frag = decodeClvdFragment(encodeFragment(1, 0, 1, true, payload))!;
    const au = asm.push(frag);
    expect(au).not.toBeNull();
    expect(Array.from(au!.annexB)).toEqual([1, 2, 3]);
  });

  it("reassembles out of order with no loss", () => {
    const annexB = makeAccessUnit(30_000);
    const frags = encodeWithFec(1, annexB).map((b) => decodeClvdFragment(b)!);
    // shuffle: reverse plus drop parity to the middle
    const reordered = [...frags].reverse();
    const asm = new ClvdAssembler();
    let out = null;
    for (const f of reordered) out = asm.push(f) ?? out;
    expect(out).not.toBeNull();
    expect(Array.from(out!.annexB)).toEqual(Array.from(annexB));
  });

  it("recovers any single dropped data fragment via FEC parity", () => {
    const annexB = makeAccessUnit(30_000);
    const frags = encodeWithFec(1, annexB).map((b) => decodeClvdFragment(b)!);
    const nData = frags.filter((f) => f.fragIdx < f.fragCount).length;
    expect(nData).toBeGreaterThan(1);

    for (let dropIdx = 0; dropIdx < nData; dropIdx++) {
      const asm = new ClvdAssembler();
      let out = null;
      for (const f of frags) {
        if (f.fragIdx < nData && f.fragIdx === dropIdx) continue; // simulate loss
        out = asm.push(f) ?? out;
      }
      expect(out, `did not recover dropped frag ${dropIdx}`).not.toBeNull();
      expect(Array.from(out!.annexB)).toEqual(Array.from(annexB));
    }
  });

  it("never fabricates output when two fragments are lost", () => {
    const annexB = makeAccessUnit(30_000);
    const frags = encodeWithFec(1, annexB).map((b) => decodeClvdFragment(b)!);
    const asm = new ClvdAssembler();
    let out = null;
    for (const f of frags) {
      if (f.fragIdx === 0 || f.fragIdx === 1) continue; // drop two data frags
      out = asm.push(f) ?? out;
    }
    expect(out).toBeNull();
  });

  it("tolerates a late/redundant parity fragment after the AU already completed", () => {
    const payload = new Uint8Array([9, 9, 9]);
    const asm = new ClvdAssembler();
    asm.push(decodeClvdFragment(encodeFragment(1, 0, 1, true, payload))!);
    // A stray parity for a completed single-fragment AU must not throw.
    const stray = decodeClvdFragment(
      encodeFragment(1, 1, 1, true, new Uint8Array(2 + VIDEO_MAX_FRAGMENT_PAYLOAD))
    );
    expect(() => asm.push(stray!)).not.toThrow();
  });
});

describe("decodeClvdFragment", () => {
  it("accepts fragIdx === fragCount (parity marker) and rejects beyond it", () => {
    const parityPayload = new Uint8Array(2 + VIDEO_MAX_FRAGMENT_PAYLOAD);
    const ok = decodeClvdFragment(encodeFragment(1, 3, 3, true, parityPayload));
    expect(ok).not.toBeNull();

    const tooFar = decodeClvdFragment(encodeFragment(1, 4, 3, true, new Uint8Array(1)));
    expect(tooFar).toBeNull();
  });

  it("still decodes a v2 18-byte header with stampUs 0", () => {
    const payload = new Uint8Array([1, 2, 3]);
    const buf = new ArrayBuffer(VIDEO_HEADER_LEN_V2 + payload.byteLength);
    const u8 = new Uint8Array(buf);
    const view = new DataView(buf);
    for (let i = 0; i < 4; i++) u8[i] = VIDEO_MAGIC.charCodeAt(i);
    u8[4] = VIDEO_VERSION_V2;
    u8[5] = FLAG_KEYFRAME;
    view.setUint16(6, 1280, true);
    view.setUint16(8, 720, true);
    view.setUint32(10, 3, true);
    view.setUint16(14, 0, true);
    view.setUint16(16, 1, true);
    u8.set(payload, VIDEO_HEADER_LEN_V2);
    const frag = decodeClvdFragment(buf)!;
    expect(frag.stampUs).toBe(0);
    expect(Array.from(frag.payload)).toEqual([1, 2, 3]);
    const au = new ClvdAssembler().push(frag);
    expect(au?.stampUs).toBe(0);
  });
});
