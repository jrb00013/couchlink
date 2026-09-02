import { useEffect, useState } from "react";
import type { InputPhotonSnapshot } from "./inputPhoton";
import {
  handoffWaitPeriods,
  meanPhaseStackMs,
  photonStretchMs,
  photonWowMs,
  SHM_WAIT_P95_GATE_MS,
  surplusRttUnits,
  WOW_SURPLUS_MS,
  wowSurplusOk,
} from "./latencyBudget";
import type { PresentStuckReason } from "./presentPromote";
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
  /** Receive → present age of last painted frame (WebCodecs or canvas). */
  ageMs?: number;
  ageBand?: string;
  /** Ms since last pad send at paint (client-local lower bound). */
  inputFreshnessMs?: number;
  /** Input→photon p50 (est.) — needs CLVD input_wm. */
  photonP50Ms?: number;
  /** Surplus p50 = photon − RTT (est.). */
  surplusP50Ms?: number;
  /** WebCodecs decode time per frame (local). */
  decodeMs?: number;
  /** WebCodecs present diagnosis string. */
  diagnosis?: string;
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
  inputPhoton?: InputPhotonSnapshot | null;
  hostStats?: HostStats | null;
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

  const rtt = t.path?.rttMs ?? 0;
  const surplus = t.inputPhoton?.surplusP50Ms ?? t.present?.surplusP50Ms;
  if (surplus != null && rtt > 0) {
    checks.push({
      label: `surplus S_p50 ${surplus.toFixed(0)}ms (wow ≤${WOW_SURPLUS_MS}ms)`,
      ok: wowSurplusOk(surplus),
      detail: wowSurplusOk(surplus)
        ? undefined
        : `input→photon minus RTT exceeds the first wow bar — trim host/client pipeline or network`,
    });
    const phi = t.inputPhoton?.photonP50Ms ?? t.present?.photonP50Ms;
    if (phi != null) {
      const wow = photonWowMs(rtt);
      checks.push({
        label: `photon Φ_p50 ${phi.toFixed(0)}ms (wow ≤${wow.toFixed(0)}ms @ ${rtt}ms RTT)`,
        ok: phi <= wow,
      });
    }
  }

  if (t.hostStats?.handoff_wait_p95_ms != null && t.hostStats.handoff_wait_p95_ms > 0) {
    const p95 = t.hostStats.handoff_wait_p95_ms;
    const fps = t.hostStats.target_fps || 60;
    const trips = t.hostStats.shm_gate_trips ?? p95 > SHM_WAIT_P95_GATE_MS;
    checks.push({
      label: `handoff wait_p95 ${p95.toFixed(2)}ms (${handoffWaitPeriods(p95, fps).toFixed(2)} T_v)`,
      ok: !trips,
      detail: trips
        ? `Hyper-V handoff wait p95 exceeds ${SHM_WAIT_P95_GATE_MS}ms — SHM gate trips`
        : undefined,
    });
  } else if (t.hostStats?.handoff_wait_ms && t.hostStats.handoff_wait_ms > SHM_WAIT_P95_GATE_MS) {
    const fps = t.hostStats.target_fps || 60;
    checks.push({
      label: `handoff wait ${t.hostStats.handoff_wait_ms.toFixed(2)}ms (${handoffWaitPeriods(t.hostStats.handoff_wait_ms, fps).toFixed(2)} T_v)`,
      ok: false,
      detail: `Hyper-V handoff wait exceeds ${SHM_WAIT_P95_GATE_MS}ms — SHM gate would trip`,
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
  frames_received?: number;
  handoff_wait_ms?: number;
  handoff_copy_ms?: number;
  handoff_wait_p95_ms?: number;
  shm_gate_trips?: boolean;
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

function fmtMs(v: number | null | undefined, digits = 1): string {
  if (v == null || !Number.isFinite(v)) return "—";
  return `${v.toFixed(digits)}ms`;
}

function WowRow({
  label,
  value,
  ok,
}: {
  label: string;
  value: string;
  ok: boolean | null;
}) {
  return (
    <div className={`dt-row ${ok === false ? "dt-wow-fail" : ok ? "dt-wow-pass" : ""}`}>
      <span className="dt-label">{label}</span>
      <span className="dt-value">
        {ok === true ? "✓ " : ok === false ? "✗ " : ""}
        {value}
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
  inputPhoton,
  presentStuck,
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
  /** Local input→photon ring snapshot — per-browser only. */
  inputPhoton?: InputPhotonSnapshot | null;
  presentStuck?: PresentStuckReason | null;
  playerPads?: Record<number, PlayerPadEntry>;
  mySlot?: number | null;
  myPadName?: string | null;
  myPadHz?: number;
  open: boolean;
  onToggle: () => void;
}) {
  const [tab, setTab] = useState<"network" | "latency" | "controller">("network");
  const [nowMs, setNowMs] = useState(() => Date.now());
  const t = telemetry;
  const checks = bottleneckChecks({
    path: t?.path ?? null,
    video: t?.video ?? null,
    padHz: t?.padHz ?? 0,
    present,
    inputPhoton,
    hostStats,
  });
  const summary = checks.length ? bottleneckSummary(checks) : null;
  const rtt = t?.path?.rttMs ?? 0;
  const padHz = myPadHz ?? t?.padHz ?? 0;
  const videoFps = hostStats?.target_fps ?? t?.video?.framesPerSecond ?? 60;
  const youLabel = mySlot != null ? `You (P${mySlot + 1})` : "You";
  const surplus = inputPhoton?.surplusP50Ms ?? present?.surplusP50Ms;
  const phi = inputPhoton?.photonP50Ms ?? present?.photonP50Ms;
  const wowSurplusOkFlag = surplus != null ? wowSurplusOk(surplus) : null;
  const wowPhotonOk =
    phi != null && rtt > 0 ? phi <= photonWowMs(rtt) : null;
  const stretchPhoton =
    rtt > 0 ? photonStretchMs(rtt) : null;
  const eta =
    phi != null && rtt > 0 ? surplusRttUnits(phi, rtt) : null;
  const phaseStack =
    padHz > 0 ? meanPhaseStackMs(padHz, videoFps, videoFps) : null;

  // Tick the clock while open on tabs that show live ages.
  useEffect(() => {
    if (!open || tab === "network") return;
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
            aria-selected={tab === "latency"}
            className={`dt-tab ${tab === "latency" ? "is-active" : ""}`}
            onClick={() => setTab("latency")}
          >
            Latency
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
        {tab === "latency" ? (
          <div className="dt-grid">
            <Group title={`${youLabel} — interactive latency (local)`}>
              <p className="dt-note">
                Input→photon and surplus are measured in this browser only. Other
                players see their own numbers on their device.
              </p>
              <Row
                label="Φ last"
                value={fmtMs(inputPhoton?.lastPhotonMs ?? null, 0)}
              />
              <Row label="Φ p50" value={fmtMs(phi ?? null, 0)} />
              <Row label="S p50 (Φ−R)" value={fmtMs(surplus ?? null, 0)} />
              <Row label="RTT (yours)" value={rtt > 0 ? `${rtt}ms` : "—"} />
              <Row
                label="Φ* wow bar"
                value={rtt > 0 ? fmtMs(photonWowMs(rtt), 0) : "—"}
              />
              <Row
                label="Φ* stretch"
                value={stretchPhoton != null ? fmtMs(stretchPhoton, 0) : "—"}
              />
              <Row
                label="η = S/R"
                value={eta != null ? eta.toFixed(2) : "—"}
              />
              <WowRow
                label="Wow S_p50"
                value={
                  surplus != null
                    ? `${surplus.toFixed(0)}ms ≤ ${WOW_SURPLUS_MS}ms`
                    : "waiting for CLVD input_wm samples"
                }
                ok={wowSurplusOkFlag}
              />
              <WowRow
                label="Wow Φ_p50"
                value={
                  phi != null && rtt > 0
                    ? `${phi.toFixed(0)}ms ≤ ${photonWowMs(rtt).toFixed(0)}ms`
                    : "—"
                }
                ok={wowPhotonOk}
              />
              <Row
                label="Input freshness"
                value={fmtMs(inputPhoton?.inputFreshnessMs ?? present?.inputFreshnessMs ?? null, 0)}
              />
              <Row
                label="Watermark ring"
                value={
                  inputPhoton
                    ? `${inputPhoton.sampleCount} samples · ring ${inputPhoton.ringSize}${
                        inputPhoton.watermarkActive ? " · wm active" : " · wm pending"
                      }`
                    : "—"
                }
              />
              <Row
                label="Mean phase stack"
                value={
                  phaseStack != null
                    ? `${phaseStack.toFixed(1)}ms @ ${padHz}Hz pad / ${videoFps}fps`
                    : "—"
                }
              />
            </Group>
            <Group title={`${youLabel} — present path`}>
              <Row label="Present mode" value={presentMode} />
              {presentStuck && (
                <Row label="Present stuck" value={presentStuck} />
              )}
              {present && (
                <>
                  <Row label="Paint fps" value={`${present.fps}fps`} />
                  <Row
                    label="Frame age"
                    value={
                      present.ageMs != null
                        ? `${present.ageMs.toFixed(1)}ms${present.ageBand ? ` (${present.ageBand})` : ""}`
                        : "—"
                    }
                  />
                  <Row
                    label="Decode (local)"
                    value={present.decodeMs != null ? fmtMs(present.decodeMs) : "—"}
                  />
                  <Row
                    label="Resolution"
                    value={`${present.width}×${present.height}`}
                  />
                  {present.dropped > 0 && (
                    <Row label="Dropped paints" value={String(present.dropped)} />
                  )}
                  {present.diagnosis && (
                    <Row label="Diagnosis" value={present.diagnosis} />
                  )}
                </>
              )}
            </Group>
            {t?.path && (
              <Group title={`${youLabel} — network path`}>
                <Row label="Path" value={`${t.path.family} ${t.path.local} → ${t.path.remote}`} />
                <Row label="Relayed" value={t.path.relayed ? "yes (TURN)" : "no"} />
                <Row label="Round-trip" value={`${t.path.rttMs}ms`} />
                {t.video && (
                  <>
                    <Row label="Jitter buffer" value={fmtMs(t.video.jitterBufferMs)} />
                    <Row label="Feed loss" value={`${t.video.packetLossPct.toFixed(2)}%`} />
                    <Row label="Decoder rate" value={`${t.video.decodeFps.toFixed(1)}fps`} />
                  </>
                )}
              </Group>
            )}
            {hostStats && (
              <Group title="Host pipeline (shared — same for all clients)">
                <Row label="Push rate" value={`${hostStats.fps.toFixed(1)}fps`} />
                <Row
                  label="Dropped / shed"
                  value={`${hostStats.dropped_frames}/${hostStats.frames_out + hostStats.dropped_frames} (${hostStats.drop_pct}%)`}
                />
                <Row label="Capture" value={fmtMs(hostStats.capture_ms)} />
                <Row label="Scale" value={fmtMs(hostStats.scale_ms)} />
                <Row label="Encode" value={fmtMs(hostStats.encode_ms)} />
                <Row label="Push" value={fmtMs(hostStats.push_ms)} />
                <Row label="Bottleneck" value={hostStats.dominant_stage} />
                <Row
                  label="Age p50/p95"
                  value={
                    hostStats.age_p50_ms || hostStats.age_p95_ms
                      ? `${(hostStats.age_p50_ms ?? 0).toFixed(0)} / ${(hostStats.age_p95_ms ?? 0).toFixed(0)} ms`
                      : "—"
                  }
                />
                <Row
                  label="Frames recv (bridge)"
                  value={
                    hostStats.frames_received != null
                      ? String(hostStats.frames_received)
                      : "—"
                  }
                />
                <Row
                  label="Handoff wait/copy"
                  value={
                    hostStats.handoff_wait_ms || hostStats.handoff_copy_ms
                      ? `${fmtMs(hostStats.handoff_wait_ms ?? 0)} / ${fmtMs(hostStats.handoff_copy_ms ?? 0)}`
                      : "— (not Hyper-V path)"
                  }
                />
                <Row
                  label="Handoff wait p95"
                  value={
                    hostStats.handoff_wait_p95_ms
                      ? `${fmtMs(hostStats.handoff_wait_p95_ms)}${
                          hostStats.shm_gate_trips ? " · SHM_GATE_TRIP" : " · SHM_SKIP"
                        }`
                      : "—"
                  }
                />
                <Row
                  label="Encoder target"
                  value={`${hostStats.target_width}×${hostStats.target_height}@${hostStats.target_fps} ${fmtKbps(hostStats.target_bitrate_kbps)}`}
                />
              </Group>
            )}
            <Group title="Other players">
              {Object.entries(playerPads ?? {})
                .filter(([slot]) => Number(slot) !== mySlot)
                .sort(([a], [b]) => Number(a) - Number(b))
                .map(([slot, p]) => (
                  <Row
                    key={slot}
                    label={`P${Number(slot) + 1}`}
                    value={`${padKindLabel(p.kind, p.id)} — latency metrics local to their browser`}
                  />
                ))}
              {Object.keys(playerPads ?? {}).filter((s) => Number(s) !== mySlot).length === 0 && (
                <p className="dt-foot">No other players seated.</p>
              )}
            </Group>
          </div>
        ) : tab === "controller" ? (
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
              {(hostStats.frames_received != null ||
                hostStats.handoff_wait_ms != null) && (
                <Row
                  label="Bridge recv / handoff"
                  value={`${hostStats.frames_received ?? "—"} recv · wait ${fmtMs(hostStats.handoff_wait_ms ?? 0)} copy ${fmtMs(hostStats.handoff_copy_ms ?? 0)}`}
                />
              )}
            </Group>
          )}
          <Group title="Input">
            <Row label="Pad" value={t?.padName && t.padName !== "none" ? t.padName : "—"} />
            <Row label="Send rate" value={t && t.padHz > 0 ? `${t.padHz}Hz` : "—"} />
            <Row label="Present mode" value={presentMode} />
            {present && (
              <>
                <Row
                  label="Paint"
                  value={`${present.fps}fps · ${present.width}×${present.height}${
                    present.ageMs != null
                      ? ` · ${present.ageMs.toFixed(1)}ms age${present.ageBand ? ` (${present.ageBand})` : ""}`
                      : ""
                  }${present.dropped > 0 ? ` · ${present.dropped} dropped` : ""}`}
                />
                {(present.photonP50Ms != null || present.surplusP50Ms != null) && (
                  <Row
                    label="Photon / S"
                    value={`Φ ${present.photonP50Ms?.toFixed(0) ?? "—"}ms · S ${present.surplusP50Ms?.toFixed(0) ?? "—"}ms — see Latency tab`}
                  />
                )}
              </>
            )}
            <Row label="Stream info" value={streamInfo} />
          </Group>
        </div>
        )}
        {(tab === "network" || tab === "latency") && streamInfo && (
          <p className="dt-foot">
            {streamInfo} · stats tick every 2s · Latency tab = per-browser
            interactive metrics · host pipeline is shared.
          </p>
        )}
      </div>
    </div>
  );
}