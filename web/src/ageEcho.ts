/** Player → host age echo. JSON on the pad DataChannel, never video_dc. */

export type AgeEcho = {
  seq: number;
  stampUs: number;
  recvMs: number;
  paintMs: number;
};

const echoed = new Set<number>();

export function resetAgeEcho(): void {
  echoed.clear();
}

export function encodeAgeEcho(e: AgeEcho): string {
  return JSON.stringify({
    type: "age_echo",
    seq: e.seq >>> 0,
    stamp_us: e.stampUs,
    recv_ms: e.recvMs,
    paint_ms: e.paintMs,
  });
}

/** Once per access-unit seq. stampUs 0 is v2 / unknown — skip. */
export function echoAgeOnce(e: AgeEcho, send: (json: string) => void): boolean {
  if (!e.stampUs || echoed.has(e.seq)) return false;
  echoed.add(e.seq);
  if (echoed.size > 256) {
    const first = echoed.values().next().value;
    if (first !== undefined) echoed.delete(first);
  }
  send(encodeAgeEcho(e));
  return true;
}
