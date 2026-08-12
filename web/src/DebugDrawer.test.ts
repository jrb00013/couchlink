import { describe, expect, it } from "vitest";
import {
  bottleneckChecks,
  bottleneckSummary,
  type PresentSummary,
} from "./DebugDrawer";
import type { InboundVideoStats, MediaPathStats } from "./player";

function path(partial: Partial<MediaPathStats>): MediaPathStats {
  return {
    local: "host",
    remote: "host",
    family: "IPv4",
    protocol: "udp",
    relayed: false,
    rttMs: 2,
    ...partial,
  };
}

function video(partial: Partial<InboundVideoStats>): InboundVideoStats {
  return {
    jitterBufferMs: 7,
    decodeFps: 59,
    framesDropped: 0,
    framesDecoded: 2000,
    bitrateKbps: 8000,
    bytesReceived: 10_000_000,
    packetsLost: 0,
    packetsReceived: 12_000,
    packetLossPct: 0,
    jitterMs: 0.4,
    frameWidth: 1280,
    frameHeight: 720,
    framesPerSecond: 60,
    pauseCount: 0,
    freezeCount: 0,
    totalFreezesDuration: 0,
    ...partial,
  };
}

const healthy: PresentSummary = { fps: 60, dropped: 0, width: 1280, height: 720 };

describe("bottleneckChecks", () => {
  it("reports nothing when LAN numbers match the measured baseline", () => {
    const checks = bottleneckChecks({
      path: path({}),
      video: video({}),
      padHz: 250,
      present: healthy,
    });
    expect(checks.every((c) => c.ok)).toBe(true);
  });

  it("flags a TURN-relayed path even when RTT is within the relay budget", () => {
    const checks = bottleneckChecks({
      path: path({ relayed: true, local: "relay", remote: "relay", rttMs: 20 }),
      video: video({}),
      padHz: 250,
      present: healthy,
    });
    expect(checks.some((c) => !c.ok && c.label.includes("TURN-relayed"))).toBe(true);
  });

  it("flags RTT over the LAN budget as the likely bottleneck", () => {
    const checks = bottleneckChecks({
      path: path({ rttMs: 80 }),
      video: video({}),
      padHz: 250,
      present: healthy,
    });
    const rtt = checks.find((c) => c.label.includes("RTT"));
    expect(rtt?.ok).toBe(false);
    expect(rtt?.detail).toContain("high latency on the direct path");
  });

  it("flags network feed loss over 1%", () => {
    const checks = bottleneckChecks({
      path: path({}),
      video: video({ packetLossPct: 4.2 }),
      padHz: 250,
      present: healthy,
    });
    const loss = checks.find((c) => c.label.includes("feed loss"));
    expect(loss?.ok).toBe(false);
    expect(loss?.detail).toContain("COUCHLINK_FEC");
  });

  it("flags low input sampling", () => {
    const checks = bottleneckChecks({
      path: path({}),
      video: video({}),
      padHz: 60,
      present: healthy,
    });
    expect(checks.find((c) => c.label.includes("pad input"))?.ok).toBe(false);
  });

  it("flags slow local paint", () => {
    const checks = bottleneckChecks({
      path: path({}),
      video: video({}),
      padHz: 250,
      present: { fps: 38, dropped: 4, width: 1280, height: 720 },
    });
    expect(checks.find((c) => c.label.includes("paint"))?.ok).toBe(false);
  });
});

describe("bottleneckSummary", () => {
  it("is 'good' with no failing checks", () => {
    const s = bottleneckSummary([{ label: "a", ok: true }]);
    expect(s.verdict).toBe("good");
  });

  it("escalates to 'warn' with multiple failures", () => {
    const s = bottleneckSummary([
      { label: "a", ok: false },
      { label: "b", ok: false },
    ]);
    expect(s.verdict).toBe("warn");
  });
});
