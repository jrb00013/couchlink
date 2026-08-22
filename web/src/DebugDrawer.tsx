import { useEffect, useState } from "react";
import type {
  InboundVideoStats,
  MediaPathStats,
  PlayerTelemetry,
} from "./player";

export type PresentSummary = {
  fps: number;
  dropped: number;
  width: number;
  height: number;
};

export type BottleneckCheck = {
  label: string;
  ok: boolean;
  detail?: string;
};

/**
 * Thresholds for flagging a segment as the bottleneck. Coarse by design:
 * the drawer is a triage tool, not a profiler — each row either looks fine
 * or names the thing to look at first.
 */
/** Direct path RTT limit — 30ms for host/prflx (LAN), 80ms for srflx (internet punched). */
export const LAN_RTT_MAX_MS = 30;
export const SRFLX_RTT_MAX_MS = 80;
export const TURN_RTT_MAX_MS = 120;
export const MAX_JITTER_BUF_MS = 20;
export const MIN_DECODE_FPS = 50;
export const MAX_LOSS_PCT = 1;
export const MIN_PAD_HZ = 100;

export function bottleneckChecks(t: {
  path: MediaPathStats | null;
  video: InboundVideoStats | null;
  padHz: number;
  present: PresentSummary | null;
}): BottleneckCheck[] {
  const checks: BottleneckCheck[] = [];
  const { path } = t;

  if (path) {
    // host/prflx are LAN candidates (tight 30ms budget); srflx is a STUN-punched
    // internet peer (generous 80ms budget). Relayed paths get the TURN budget.
    const isInternet = path.local === "srflx" || path.remote === "srflx";
    const rttLimit = path.relayed ? TURN_RTT_MAX_MS : isInternet ? SRFLX_RTT_MAX_MS : LAN_RTT_MAX_MS;
    checks.push({
      label: path.relayed
        ? `TURN relay RTT ${path.rttMs}ms (limit ${TURN_RTT_MAX_MS}ms)`
        : `${path.family} ${path.local}→${path.remote} RTT ${path.rttMs}ms${path.rttMs <= rttLimit ? "" : " ⚠ high"}`,
      ok: path.rttMs <= rttLimit,
      detail:
        path.rttMs > rttLimit
          ? path.relayed
            ? "relaying through TURN — prefer a direct path (port-forward or WireGuard)"
            : isInternet
              ? "internet peer (STUN punched) — RTT reflects your internet link, not a config problem"
              : "high latency on the direct path — check Wi-Fi / the link to the host"
          : undefined,
    });
    if (path.relayed) {
      checks.push({
        label: "media is TURN-relayed",
        ok: false,
        detail: "every packet goes through the relay server — double the hop. Fix the direct route for lowest latency.",
      });
    }
  }

  if (t.video) {
    checks.push({
      label: `jitter buffer ${t.video.jitterBufferMs.toFixed(1)}ms (limit ${MAX_JITTER_BUF_MS}ms)`,
      ok: t.video.jitterBufferMs <= MAX_JITTER_BUF_MS,
      detail:
        t.video.jitterBufferMs > MAX_JITTER_BUF_MS
          ? "browser buffering ahead of playback — network jitter or host frame pacing"
          : undefined,
    });
    if (t.video.decodeFps > 0) {
      checks.push({
        label: `decode ${t.video.decodeFps.toFixed(1)}fps (min ${MIN_DECODE_FPS}fps)`,
        ok: t.video.decodeFps >= MIN_DECODE_FPS,
        detail:
          t.video.decodeFps < MIN_DECODE_FPS
            ? "decoder not keeping up or the stream isn't arriving at full rate"
            : undefined,
      });
    }
    checks.push({
      label: `feed loss ${t.video.packetLossPct.toFixed(2)}% window`,
      ok: t.video.packetLossPct <= MAX_LOSS_PCT,
      detail:
        t.video.packetLossPct > MAX_LOSS_PCT
          ? "packets lost in the network — the largest felt-latency cause. Check Wi-Fi, or set COUCHLINK_FEC=1 on the host"
          : undefined,
    });
    if (t.video.totalFreezesDuration > 0 || t.video.freezeCount > 0) {
      checks.push({
        label: `${t.video.freezeCount} freeze(s), ${(t.video.totalFreezesDuration / 1000).toFixed(1)}s total`,
        ok: t.video.freezeCount === 0 && t.video.totalFreezesDuration === 0,
        detail: "playback freezes mean the present pipeline stalled — see host stream_info / frame pacing",
      });
    }
  }

  if (t.padHz > 0) {
    checks.push({
      label: `pad input ${t.padHz}Hz (min ${MIN_PAD_HZ}Hz)`,
      ok: t.padHz >= MIN_PAD_HZ,
      detail:
        t.padHz < MIN_PAD_HZ
          ? "input sampling is below 100Hz — the left column (latency) is dominated by this"
          : undefined,
    });
  }

  if (t.present) {
    checks.push({
      label: `paint ${t.present.fps}fps${t.present.dropped > 0 ? ` (${t.present.dropped} dropped)` : ""}`,
      ok: t.present.fps >= MIN_DECODE_FPS && t.present.dropped === 0,
      detail:
        t.present.fps < MIN_DECODE_FPS
          ? "local paint isn't reaching the stream rate — this machine may be the bottleneck"
          : undefined,
    });
  }

  return checks;
}

export function bottleneckSummary(checks: BottleneckCheck[]): {
  verdict: "good" | "check" | "warn";
  text: string;
} {
  const failing = checks.filter((c) => !c.ok);
  if (failing.length === 0) return { verdict: "good", text: "No bottleneck detected" };
  const first = failing[0];
  if (failing.length === 1) return { verdict: "check", text: `Watch: ${first.label}` };
  return {
    verdict: "warn",
    text: `${failing.length} flags — ${failing.map((c) => c.label).slice(0, 3).join(" · ")}`,
  };
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="dt-row">
      <span className="dt-label">{label}</span>
      <span className="dt-value">{value}</span>
    </div>
  );
}

function Group({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="dt-group">
      <h3>{title}</h3>
      {children}
    </section>
  );
}

function fmtKbps(k: number): string {
  if (k >= 1000) return `${(k / 1000).toFixed(2)} Mbps`;
  return `${Math.round(k)} kbps`;
}

export type HostStats = {
  fps: number;
  frames_out: number;
  dropped_frames: number;
  drop_pct: number;
  capture_ms: number;
  scale_ms: number;
  encode_ms: number;
  push_ms: number;
  dominant_stage: string;
  target_width: number;
  target_height: number;
  target_fps: number;
  target_bitrate_kbps: number;
  age_p50_ms?: number;
  age_p95_ms?: number;
};

/** A pad_info heartbeat older than this reads as "not actually sending
 * input right now" rather than merely "connected a while ago" — player.ts
 * re-announces every 3s while genuinely producing input, so a couple of
 * missed beats is a real signal, not noise. */
const PAD_STALE_MS = 8000;

export type PlayerPadEntry = { kind: string; id: string; lastSeenAt: number };

/**
 * `kind` is what gets sent to the *emulator* — keyboard+mouse and touch both
 * report "dualsense" there because CLPD frames from either source are
 * DualSense-shaped, same as a real pad, and that's what makes the emulator
 * pick the right virtual device. A human reading this debug view doesn't
 * care what the emulator was told; showing "DualSense" for someone playing
 * on a keyboard is just wrong, not merely imprecise — the actual input
 * source lives in `id` instead ("keyboard+mouse" / "touch"), so check that
 * first.
 */
export function padKindLabel(kind: string, id: string): string {
  if (id === "keyboard+mouse") return "⌨ Keyboard + Mouse";
  if (id === "touch") return "📱 Touch controls";
  switch (kind) {
    case "dualsense":
      return "DualSense";
    case "xbox":
      return "Xbox";
    case "generic":
      return "Generic pad";
    default:
      return kind || "—";
  }
}

function ControllerRow({
  label,
  kind,
  id,
  hz,
  lastSeenAt,
  nowMs,
}: {
  label: string;
  kind: string;
  id: string;
  hz?: number;
  lastSeenAt: number | null;
  nowMs: number;
}) {
  const ageMs = lastSeenAt == null ? null : nowMs - lastSeenAt;
  const stale = ageMs == null || ageMs > PAD_STALE_MS;
  const status =
    ageMs == null
      ? "no controller reported"
      : ageMs < 1500
        ? "sending input now"
        : `last input ${(ageMs / 1000).toFixed(0)}s ago`;
  return (
    <div className={`dt-row dt-pad-row ${stale ? "dt-pad-stale" : "dt-pad-live"}`}>
      <span className="dt-label">{label}</span>
      <span className="dt-value">
        <span className={`dt-pad-dot ${stale ? "" : "dt-pad-dot-live"}`} />
        {kind || id ? padKindLabel(kind, id) : "—"}
        {kind && id && id !== "keyboard+mouse" && id !== "touch" ? ` (${id})` : ""}
        {typeof hz === "number" && hz > 0 ? ` · ${hz}Hz` : ""} — {status}
      </span>
    </div>
  );
}

export default function DebugDrawer({
  telemetry,
  hostStats,
  present,
  streamInfo,
  presentMode,
  playerPads,
  mySlot,
  myPadName,
  myPadHz,
  open,
  onToggle,
}: {
  telemetry: PlayerTelemetry | null;
  hostStats: HostStats | null;
  present: PresentSummary | null;
  streamInfo: string;
  presentMode: string;
  playerPads?: Record<number, PlayerPadEntry>;
  mySlot?: number | null;
  myPadName?: string | null;
  myPadHz?: number;
  open: boolean;
  onToggle: () => void;
}) {
  const [tab, setTab] = useState<"network" | "controller">("network");
  const [nowMs, setNowMs] = useState(() => Date.now());
  const t = telemetry;
  const checks = bottleneckChecks({
    path: t?.path ?? null,
    video: t?.video ?? null,
    padHz: t?.padHz ?? 0,
    present,
  });
  const summary = checks.length ? bottleneckSummary(checks) : null;

  // Only tick the clock while the drawer is open and on the Controller tab —
  // this is purely so "Xs ago" keeps counting up live, not a data source.
  useEffect(() => {
    if (!open || tab !== "controller") return;
    const id = window.setInterval(() => setNowMs(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [open, tab]);

  return (
    <div className={`debug ${open ? "is-open" : ""}`}>
      <button type="button" className="dt-toggle" onClick={onToggle} aria-expanded={open}>
        <span className="dt-pill">
          <span className={`dt-dot dt-dot-${summary?.verdict ?? "idle"}`} />
          debug
        </span>
        <span className="dt-caret">{open ? "▾" : "▴"}</span>
      </button>

      <div className="dt-panel">
        {summary && (
          <p className={`dt-summary dt-${summary.verdict}`}>{summary.text}</p>
        )}
        <div className="dt-tabs" role="tablist">
          <button
            type="button"
            role="tab"
            aria-selected={tab === "network"}
            className={`dt-tab ${tab === "network" ? "is-active" : ""}`}
            onClick={() => setTab("network")}
          >
            Network
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={tab === "controller"}
            className={`dt-tab ${tab === "controller" ? "is-active" : ""}`}
            onClick={() => setTab("controller")}
          >
            Controller
          </button>
        </div>
        {tab === "controller" ? (
          <div className="dt-grid">
            <Group title="Controllers">
              <ControllerRow
                label={mySlot ? `You (P${mySlot + 1})` : "You"}
                kind=""
                id={myPadName ?? t?.padName ?? "—"}
                hz={myPadHz ?? t?.padHz}
                lastSeenAt={(myPadHz ?? t?.padHz ?? 0) > 0 ? nowMs : null}
                nowMs={nowMs}
              />
              {Object.entries(playerPads ?? {})
                .filter(([slot]) => Number(slot) !== mySlot)
                .sort(([a], [b]) => Number(a) - Number(b))
                .map(([slot, p]) => (
                  <ControllerRow
                    key={slot}
                    label={`P${Number(slot) + 1}`}
                    kind={p.kind}
                    id={p.id}
                    lastSeenAt={p.lastSeenAt}
                    nowMs={nowMs}
                  />
                ))}
              {Object.keys(playerPads ?? {}).length === 0 && (
                <p className="dt-foot">No other players seated yet.</p>
              )}
            </Group>
          </div>
        ) : (
        <div className="dt-grid">
          {t?.path && (
            <Group title="Media path / transit latency">
              <Row label="Path" value={`${t.path.family} ${t.path.local} → ${t.path.remote}`} />
              <Row label="Transport" value={t.path.protocol} />
              <Row label="Relayed" value={t.path.relayed ? "yes (TURN)" : "no"} />
              <Row label="Round-trip" value={`${t.path.rttMs}ms`} />
            </Group>
          )}
          {t?.video && (
            <Group title="Streaming / latency">
              <Row label="Resolution" value={`${t.video.frameWidth}×${t.video.frameHeight}`} />
              <Row label="Decoder rate" value={t.video.decodeFps > 0 ? `${t.video.decodeFps.toFixed(1)}fps` : "…"} />
              <Row label="Frames" value={`${t.video.framesDecoded} decoded · ${t.video.framesDropped} dropped`} />
              <Row label="Jitter buffer" value={`${t.video.jitterBufferMs.toFixed(1)}ms`} />
              <Row label="Freeze/pause" value={`${t.video.freezeCount}f / ${t.video.pauseCount}p`} />
            </Group>
          )}
          {t?.video && (
            <Group title="Network feed">
              <Row label="Bitrate" value={fmtKbps(t.video.bitrateKbps)} />
              <Row label="Received" value={`${(t.video.bytesReceived / 1024).toFixed(0)} KB`} />
              <Row label="Packets" value={`${t.video.packetsReceived} recv · ${t.video.packetsLost} lost`} />
              <Row label="Feed loss" value={`${t.video.packetLossPct.toFixed(2)}%`} />
              <Row label="Jitter" value={`${t.video.jitterMs.toFixed(1)}ms`} />
            </Group>
          )}
          {hostStats && (
            <Group title="Host pipeline (per frame)">
              <Row label="Push rate" value={`${hostStats.fps.toFixed(1)}fps`} />
              <Row
                label="Dropped / shed"
                value={`${hostStats.dropped_frames}/${hostStats.frames_out + hostStats.dropped_frames} (${hostStats.drop_pct}%)`}
              />
              <Row label="Capture" value={`${hostStats.capture_ms.toFixed(1)}ms`} />
              <Row label="Scale" value={`${hostStats.scale_ms.toFixed(1)}ms`} />
              <Row label="Encode" value={`${hostStats.encode_ms.toFixed(1)}ms`} />
              <Row label="Push" value={`${hostStats.push_ms.toFixed(1)}ms`} />
              <Row label="Bottleneck" value={hostStats.dominant_stage} />
              <Row
                label="Encoder target"
                value={`${hostStats.target_width}×${hostStats.target_height}@${hostStats.target_fps} ${fmtKbps(hostStats.target_bitrate_kbps)}`}
              />
              <Row
                label="Age p50/p95"
                value={
                  hostStats.age_p50_ms || hostStats.age_p95_ms
                    ? `${(hostStats.age_p50_ms ?? 0).toFixed(0)} / ${(hostStats.age_p95_ms ?? 0).toFixed(0)} ms`
                    : "—"
                }
              />
            </Group>
          )}
          <Group title="Input">
            <Row label="Pad" value={t?.padName && t.padName !== "none" ? t.padName : "—"} />
            <Row label="Send rate" value={t && t.padHz > 0 ? `${t.padHz}Hz` : "—"} />
            <Row label="Present mode" value={presentMode} />
            {present && (
              <Row label="Paint" value={`${present.fps}fps · ${present.width}×${present.height} ${present.dropped > 0 ? `· ${present.dropped} dropped` : ""}`} />
            )}
            <Row label="Stream info" value={streamInfo} />
          </Group>
        </div>
        )}
        {tab === "network" && streamInfo && (
          <p className="dt-foot">
            {streamInfo} · stats tick every 2s · browser & host numbers fuse
            here to find the slow hop.
          </p>
        )}
      </div>
    </div>
  );
}