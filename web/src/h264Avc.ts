/** Annex-B ↔ AVCC helpers for WebCodecs VideoDecoder. */

export type AvcParamSets = {
  sps: Uint8Array;
  pps: Uint8Array;
};

/** Find 3- or 4-byte start codes; return NAL payloads (no start code). */
export function splitAnnexB(data: Uint8Array): Uint8Array[] {
  const nals: Uint8Array[] = [];
  let i = 0;
  const findStart = (from: number): { at: number; len: number } | null => {
    for (let j = from; j + 3 < data.length; j++) {
      if (data[j] === 0 && data[j + 1] === 0) {
        if (data[j + 2] === 1) return { at: j, len: 3 };
        if (data[j + 2] === 0 && data[j + 3] === 1) return { at: j, len: 4 };
      }
    }
    return null;
  };
  let start = findStart(0);
  while (start) {
    const nalStart = start.at + start.len;
    const next = findStart(nalStart);
    const nalEnd = next ? next.at : data.length;
    if (nalEnd > nalStart) nals.push(data.subarray(nalStart, nalEnd));
    start = next;
    i = nalEnd;
  }
  void i;
  return nals;
}

export function extractParamSets(annexB: Uint8Array): AvcParamSets | null {
  let sps: Uint8Array | null = null;
  let pps: Uint8Array | null = null;
  for (const nal of splitAnnexB(annexB)) {
    if (!nal.length) continue;
    const t = nal[0] & 0x1f;
    if (t === 7) sps = nal;
    else if (t === 8) pps = nal;
  }
  if (sps && pps) return { sps, pps };
  return null;
}

/** ISO/IEC 14496-15 avcC box body (no box header). */
export function buildAvcC(sps: Uint8Array, pps: Uint8Array): Uint8Array {
  const out = new Uint8Array(11 + sps.length + 1 + 2 + pps.length);
  let o = 0;
  out[o++] = 1; // configurationVersion
  out[o++] = sps[1]; // AVCProfileIndication
  out[o++] = sps[2]; // profile_compatibility
  out[o++] = sps[3]; // AVCLevelIndication
  out[o++] = 0xff; // lengthSizeMinusOne = 3 → 4-byte lengths
  out[o++] = 0xe1; // numOfSequenceParameterSets = 1
  out[o++] = (sps.length >> 8) & 0xff;
  out[o++] = sps.length & 0xff;
  out.set(sps, o);
  o += sps.length;
  out[o++] = 1; // numOfPictureParameterSets
  out[o++] = (pps.length >> 8) & 0xff;
  out[o++] = pps.length & 0xff;
  out.set(pps, o);
  return out;
}

/** Convert Annex-B AU to length-prefixed AVCC (4-byte big-endian lengths). */
export function annexBToLengthPrefixed(annexB: Uint8Array): Uint8Array {
  const nals = splitAnnexB(annexB).filter((n) => {
    if (!n.length) return false;
    const t = n[0] & 0x1f;
    // Skip AUD / filler; keep SPS/PPS/IDR/non-IDR/SEI
    return t !== 9 && t !== 12;
  });
  let size = 0;
  for (const n of nals) size += 4 + n.length;
  const out = new Uint8Array(size);
  let o = 0;
  for (const n of nals) {
    out[o++] = (n.length >>> 24) & 0xff;
    out[o++] = (n.length >>> 16) & 0xff;
    out[o++] = (n.length >>> 8) & 0xff;
    out[o++] = n.length & 0xff;
    out.set(n, o);
    o += n.length;
  }
  return out;
}

export function codecStringFromSps(sps: Uint8Array): string {
  // avc1.PPCCLL — profile, constraint, level
  const pp = sps[1].toString(16).padStart(2, "0");
  const cc = sps[2].toString(16).padStart(2, "0");
  const ll = sps[3].toString(16).padStart(2, "0");
  return `avc1.${pp}${cc}${ll}`.toUpperCase().replace("AVC1", "avc1");
}
