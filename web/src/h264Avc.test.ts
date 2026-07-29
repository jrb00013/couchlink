import { describe, expect, it } from "vitest";
import {
  annexBToLengthPrefixed,
  buildAvcC,
  extractParamSets,
  splitAnnexB,
} from "./h264Avc";
import { ClvdAssembler, decodeClvdFragment, VIDEO_HEADER_LEN } from "./clvd";

describe("h264Avc", () => {
  // Minimal fake SPS/PPS NALs (not bit-valid, just framing)
  const sps = new Uint8Array([0x67, 0x42, 0xe0, 0x1f, 1, 2, 3]);
  const pps = new Uint8Array([0x68, 0xce, 0x3c, 0x80]);
  const idr = new Uint8Array([0x65, 0x88, 0x84, 0x00]);

  function annexB(...nals: Uint8Array[]): Uint8Array {
    const parts: number[] = [];
    for (const n of nals) {
      parts.push(0, 0, 0, 1, ...n);
    }
    return new Uint8Array(parts);
  }

  it("splits annex-B NALs", () => {
    const data = annexB(sps, pps, idr);
    const nals = splitAnnexB(data);
    expect(nals).toHaveLength(3);
    expect(nals[0][0] & 0x1f).toBe(7);
    expect(nals[2][0] & 0x1f).toBe(5);
  });

  it("extracts param sets and builds avcC", () => {
    const data = annexB(sps, pps, idr);
    const params = extractParamSets(data)!;
    expect(params.sps).toEqual(sps);
    expect(params.pps).toEqual(pps);
    const avcc = buildAvcC(params.sps, params.pps);
    expect(avcc[0]).toBe(1);
    expect(avcc[1]).toBe(0x42);
  });

  it("converts to length-prefixed", () => {
    const data = annexB(idr);
    const lp = annexBToLengthPrefixed(data);
    expect(lp[0]).toBe(0);
    expect(lp[1]).toBe(0);
    expect(lp[2]).toBe(0);
    expect(lp[3]).toBe(idr.length);
    expect(Array.from(lp.subarray(4))).toEqual(Array.from(idr));
  });
});

describe("clvd fragments", () => {
  function encodeFrag(opts: {
    seq: number;
    keyframe: boolean;
    fragIdx: number;
    fragCount: number;
    payload: Uint8Array;
  }): ArrayBuffer {
    const buf = new ArrayBuffer(VIDEO_HEADER_LEN + opts.payload.length);
    const u8 = new Uint8Array(buf);
    u8.set([0x43, 0x4c, 0x56, 0x44, 2, opts.keyframe ? 1 : 0]);
    const view = new DataView(buf);
    view.setUint16(6, 1280, true);
    view.setUint16(8, 720, true);
    view.setUint32(10, opts.seq, true);
    view.setUint16(14, opts.fragIdx, true);
    view.setUint16(16, opts.fragCount, true);
    u8.set(opts.payload, VIDEO_HEADER_LEN);
    return buf;
  }

  it("reassembles out-of-order fragments", () => {
    const a = new Uint8Array([1, 2, 3]);
    const b = new Uint8Array([4, 5, 6, 7]);
    const f1 = decodeClvdFragment(
      encodeFrag({ seq: 3, keyframe: true, fragIdx: 1, fragCount: 2, payload: b })
    )!;
    const f0 = decodeClvdFragment(
      encodeFrag({ seq: 3, keyframe: true, fragIdx: 0, fragCount: 2, payload: a })
    )!;
    const asm = new ClvdAssembler();
    expect(asm.push(f1)).toBeNull();
    const au = asm.push(f0)!;
    expect(au.keyframe).toBe(true);
    expect(Array.from(au.annexB)).toEqual([1, 2, 3, 4, 5, 6, 7]);
  });
});
