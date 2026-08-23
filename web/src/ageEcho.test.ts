import { describe, expect, it } from "vitest";
import { echoAgeOnce, encodeAgeEcho, resetAgeEcho } from "./ageEcho";

describe("ageEcho", () => {
  it("encodes the pad JSON the host parser expects", () => {
    expect(JSON.parse(encodeAgeEcho({ seq: 1, stampUs: 9, recvMs: 1, paintMs: 2 }))).toEqual({
      type: "age_echo",
      seq: 1,
      stamp_us: 9,
      recv_ms: 1,
      paint_ms: 2,
    });
  });

  it("sends once per seq including stamp_us 0 (canvas present-path age)", () => {
    resetAgeEcho();
    const sent: string[] = [];
    const send = (j: string) => sent.push(j);
    expect(echoAgeOnce({ seq: 1, stampUs: 0, recvMs: 1, paintMs: 2 }, send)).toBe(true);
    expect(echoAgeOnce({ seq: 2, stampUs: 9, recvMs: 1, paintMs: 2 }, send)).toBe(true);
    expect(echoAgeOnce({ seq: 2, stampUs: 9, recvMs: 3, paintMs: 4 }, send)).toBe(false);
    expect(sent).toHaveLength(2);
  });
});
