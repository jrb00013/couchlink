import { useEffect, useRef, useState } from "react";
import { CouchlinkPlayer, type ConnectionState } from "./player";
import {
  canUseLowLatencyCanvas,
  LowLatencyCanvasView,
} from "./lowLatencyCanvas";
import {
  canUseWebCodecs,
  WebCodecsCanvasView,
} from "./webCodecsCanvas";
import {
  getInputPhotonSnapshot,
  inputFreshnessMs,
  notePhotonPaint,
  photonP50Ms,
  resetInputPhoton,
  surplusP50Ms,
} from "./inputPhoton";
import { classifyPresentStuck, type PresentStuckReason } from "./presentPromote";
import { ControllerViz, silhouettePad, useLivePads } from "./ControllerViz";
import type { ControllerKind } from "./controllerKind";
import { seatForRemoteSlot } from "./seat";
import { KeyboardMouseViz } from "./KeyboardMouseViz";
import { clog, cerror, cwarn } from "./log";
import { usePlayerCallbacks } from "./usePlayerCallbacks";
import DebugDrawer, { type PresentSummary } from "./DebugDrawer";
import { KeyboardMouseInput } from "./keyboardMouse";
import { KeybindsModal } from "./KeybindsModal";
import { loadKbmBinds, type KbmBinds } from "./kbmBinds";
import {
  detectLandscape,
  detectMobile,
  enterElementFullscreen,
  exitElementFullscreen,
  isNativeFullscreen,
  isSideMode,
  lockLandscape,
  unlockOrientation,
} from "./mobile";
import { TouchGamepadInput } from "./touchPad";
import { TouchOverlay } from "./TouchOverlay";
import type { PlayerTelemetry } from "./player";
import { parseInviteString } from "./invite";
import "./App.css";

const DEFAULT_WS =
  typeof location !== "undefined" && location.port === "5174"
    ? `${location.protocol === "https:" ? "wss" : "ws"}://${location.hostname}:8443/ws`
    : `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`;

function preferLegacyVideo(): boolean {
  if (typeof location === "undefined") return false;
  return new URLSearchParams(location.search).get("legacyVideo") === "1";
}

function padDisplayName(kind: string, id: string): string {
  if (id === "keyboard+mouse") return "Keyboard + Mouse";
  if (id === "touch") return "Touch";
  if (kind === "dualsense") return "DualSense";
  if (kind === "xbox") return "Xbox";
  return "Gamepad";
}

function secureContextHint(): string | null {
  if (typeof window === "undefined") return null;
  if (window.isSecureContext) return null;
  return "Open via http://127.0.0.1 (or https) for WebCodecs near-zero latency — LAN http falls back to RTP (~7ms JB).";
}

function readInvite() {
  if (typeof location === "undefined")
    return { sessionId: "", pin: "", auto: false, signalingUrl: undefined, turn: null };
  const q = new URLSearchParams(location.search);
  const sessionId = (q.get("s") ?? q.get("session") ?? "").trim();
  const pin = (q.get("p") ?? q.get("pin") ?? "").trim();
  const auto = q.get("auto") === "1" || (!!sessionId && !!pin && q.get("auto") !== "0");
  const ws = q.get("ws") ?? q.get("signaling") ?? undefined;
  const turnUrl = q.get("turn");
  const turnUser = q.get("turnu");
  const turnPass = q.get("turnp");
  const turn =
    turnUrl && turnUser && turnPass ? { url: turnUrl, user: turnUser, pass: turnPass } : null;
  return { sessionId, pin, auto, signalingUrl: ws, turn };
}

export default function App() {
  const invite = readInvite();
  const [signalingUrl, setSignalingUrl] = useState(invite.signalingUrl ?? DEFAULT_WS);
  const [sessionId, setSessionId] = useState(invite.sessionId);
  const [pin, setPin] = useState(invite.pin);
  const [pasteLink, setPasteLink] = useState("");
  const [pasteError, setPasteError] = useState<string | null>(null);
  const [pastedTurn, setPastedTurn] = useState(invite.turn);
  const [state, setState] = useState<ConnectionState>("disconnected");
  const [detail, setDetail] = useState("");
  const [streamMeta, setStreamMeta] = useState("—");
  const [captureHint, setCaptureHint] = useState<string | null>(null);
  const [padMeta, setPadMeta] = useState("press a button on your DualSense / pad");
  const [fullscreen, setFullscreen] = useState(false);
  const [presentMode, setPresentMode] = useState<"webcodecs" | "canvas" | "video" | "—">("—");
  const [ctxHint, setCtxHint] = useState<string | null>(() => secureContextHint());
  const [telemetry, setTelemetry] = useState<PlayerTelemetry | null>(null);
  const rttRef = useRef(0);
  const [hostStats, setHostStats] = useState<{
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
  } | null>(null);
  const [presentStuck, setPresentStuck] = useState<PresentStuckReason | null>(null);
  /** Session occupancy snapshot — "N/4 players connected" (host owns P1). */
  const [playersStatus, setPlayersStatus] = useState<{
    occupied: number;
    max: number;
  } | null>(null);
  /** Per-slot controller status for the debug drawer's Controller tab — kind,
   * raw device id, and when we last heard a pad_info heartbeat from them
   * (player.ts re-announces every 3s while actually sending input), so a
   * stale entry visibly ages instead of silently claiming "connected"
   * forever after someone's controller stops working. */
  const [playerPads, setPlayerPads] = useState<
    Record<number, { kind: string; id: string; lastSeenAt: number }>
  >({});
  /** This browser's own player slot, assigned by the session on registration. */
  const [mySlot, setMySlot] = useState<number | null>(null);
  const [present, setPresent] = useState<PresentSummary | null>(null);

  function presentPhotonFields() {
    const rtt = rttRef.current;
    const photon = photonP50Ms();
    const surplus = surplusP50Ms(rtt);
    return {
      inputFreshnessMs: inputFreshnessMs() ?? undefined,
      photonP50Ms: photon ?? undefined,
      surplusP50Ms: surplus ?? undefined,
    };
  }
  /** Headless Ricardo scrape (`regression-latency-live.mjs`) reads this. */
  useEffect(() => {
    type RicardoHook = {
      presentMode: string;
      rttMs: number;
      hostStats: typeof hostStats;
      present: typeof present;
      inputPhoton: ReturnType<typeof getInputPhotonSnapshot>;
    };
    const w = window as Window & { __couchlinkRicardo?: () => RicardoHook };
    w.__couchlinkRicardo = () => ({
      presentMode,
      rttMs: rttRef.current,
      hostStats,
      present,
      inputPhoton: getInputPhotonSnapshot(rttRef.current),
    });
    return () => {
      delete w.__couchlinkRicardo;
    };
  }, [presentMode, hostStats, present]);
  const [debugOpen, setDebugOpen] = useState(false);
  const [kbmActive, setKbmActive] = useState(false);
  const [keybindsOpen, setKeybindsOpen] = useState(false);
  const [kbmBinds, setKbmBinds] = useState<KbmBinds>(() => loadKbmBinds());
  const [pointerLocked, setPointerLocked] = useState(false);
  const kbmRef = useRef<KeyboardMouseInput | null>(null);
  const [kbmInput, setKbmInput] = useState<KeyboardMouseInput | null>(null);
  const [isMobile, setIsMobile] = useState(() => detectMobile());
  const [landscape, setLandscape] = useState(() => detectLandscape());
  const touchInputRef = useRef<TouchGamepadInput | null>(null);

  const videoRef = useRef<HTMLVideoElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  /** Canvas the WebCodecs path paints to, kept separate from the RTP canvas so
   * RTP can stay on screen as a safety net while WebCodecs warms up. */
  const wcCanvasRef = useRef<HTMLCanvasElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  /** Fullscreen target on mobile — wraps the stage + touch controller so both
   * are visible (controller overlays the video) in fullscreen. Desktop keeps
   * fullscreening the stage element exactly as before. */
  const mobileFsRef = useRef<HTMLDivElement>(null);
  const playerRef = useRef<CouchlinkPlayer | null>(null);
  const viewRef = useRef<LowLatencyCanvasView | null>(null);
  const wcRef = useRef<WebCodecsCanvasView | null>(null);
  const webcodecsActiveRef = useRef(false);
  /** Saw at least one CLVD access unit this session (for stuck taxonomy). */
  const sawAuRef = useRef(false);
  /** WebCodecs stalled this session (warmup rescue); cleared on promote. */
  const stalledRef = useRef(false);
  /** WebCodecs has painted / photon path live (RTP canvas may still be visible). */
  const promotedRef = useRef(false);
  /** WC stamps input_wm in the background; RTP stays the visible high-fps present. */
  const softwarePhotonRef = useRef(false);
  const rtpFallbackTimer = useRef<number | null>(null);
  const autoStarted = useRef(false);
  const pendingStreamRef = useRef<MediaStream | null>(null);
  /**
   * Most recent RTP stream, kept so the WebCodecs fallback can re-attach it.
   *
   * Distinct from pendingStreamRef on purpose: that one means "waiting for the
   * video ref to exist" and must be cleared once used. Overloading it for the
   * fallback left it permanently set while WebCodecs owned the canvas, and the
   * render-time effect below re-entered attachStream on every single render.
   */
  const heldStreamRef = useRef<MediaStream | null>(null);
  /** Last stream we logged as held, so the notice prints once per stream. */
  const heldLoggedRef = useRef<MediaStream | null>(null);
  const [videoDiag, setVideoDiag] = useState("video: —");

  function clearRtpFallbackTimer() {
    if (rtpFallbackTimer.current) {
      clearTimeout(rtpFallbackTimer.current);
      rtpFallbackTimer.current = null;
    }
  }

  /** If WebCodecs never paints, fall back to the RTP media track.
   *
   * RTP has been painting the whole warm-up, so the fallback just hides the
   * empty WebCodecs canvas and keeps the RTP canvas — no re-attach needed. */
  function armWebCodecsFallback() {
    clearRtpFallbackTimer();
    // Once WC has painted, hybrid keeps RTP visible — don't stamp fallback_timer
    // and thrash present_path. Photon sidecar recovers via IDR only.
    if (wcRef.current?.hasPainted() || softwarePhotonRef.current) return;
    rtpFallbackTimer.current = window.setTimeout(() => {
      rtpFallbackTimer.current = null;
      if (!webcodecsActiveRef.current) return;
      if (wcRef.current?.hasPainted() || softwarePhotonRef.current) return;
      cwarn("WebCodecs produced no frames — falling back to RTP canvas (will retry on next AU)");
      // Do not resumeWarmup() — hybrid stays on dual; path flips blacked RTP.
      promotedRef.current = false;
      wcCanvasRef.current?.classList.add("is-hidden");
      canvasRef.current?.classList.remove("is-hidden");
      setVideoDiag("webcodecs: no frames yet — RTP safety net (warmup)");
      const reason = classifyPresentStuck({
        preferLegacy: preferLegacyVideo(),
        hasDecoder: typeof VideoDecoder === "function",
        sawAu: sawAuRef.current,
        painted: !!wcRef.current?.hasPainted(),
        stalled: stalledRef.current,
        fallbackFired: true,
      });
      clog("present stuck", {
        reason,
        hasDecoder: typeof VideoDecoder === "function",
        secure: window.isSecureContext,
      });
      setPresentStuck(reason);
    }, 15000);
  }

  function ensureWebCodecs(): boolean {
    if (preferLegacyVideo() || !canUseWebCodecs() || !wcCanvasRef.current) return false;
    if (!wcRef.current) {
      wcRef.current = new WebCodecsCanvasView(wcCanvasRef.current);
      wcRef.current.setStatsHandler((s) => {
        clearRtpFallbackTimer();
        if (!promotedRef.current) {
          promoteWebcodecsPresent();
        }
        const fresh = inputFreshnessMs();
        const photon = photonP50Ms();
        if (softwarePhotonRef.current) {
          setPresentMode("webcodecs");
          setPresentStuck(null);
          setVideoDiag(
            `LIVE RTP+WC · WC ${s.presentFps}fps · photon path live · drop=${s.dropped}${
              photon != null ? ` · Φ ${photon.toFixed(0)}ms` : fresh != null ? ` · input ${fresh.toFixed(0)}ms` : ""
            }`
          );
          return;
        }
        setVideoDiag(
          `LIVE ${s.presentFps}fps · ${s.ageMs.toFixed(1)}ms age (${s.ageBand}) · ${s.decodeMs.toFixed(1)}ms decode${
            photon != null ? ` · photon ${photon.toFixed(0)}ms (est.)` : fresh != null ? ` · input ${fresh.toFixed(0)}ms` : ""
          } · drop=${s.dropped}`
        );
        setPresentMode("webcodecs");
        setPresent({
          fps: s.presentFps,
          dropped: s.dropped,
          width: s.width,
          height: s.height,
          ageMs: s.ageMs,
          ageBand: s.ageBand,
          decodeMs: s.decodeMs,
          diagnosis: s.diagnosis,
          ...presentPhotonFields(),
        });
      });
      wcRef.current.setKeyframeHandler(() => {
        playerRef.current?.requestVideoKeyframe();
      });
      // Hand the visible canvas over to WebCodecs only once it has actually
      // painted — until then RTP stays on screen as the safety net.
      wcRef.current.setFirstPaintHandler(() => {
        promoteWebcodecsPresent();
      });
      wcRef.current.setPaintedHandler((a) => {
        notePhotonPaint(a.paintMs, a.inputWm);
        playerRef.current?.echoPaintedAge(a);
      });
      wcRef.current.setStallHandler(() => {
        // Exclusive-WC stall only (photon sidecar never calls this).
        // Still: do not path-flip — RTP canvas is already visible in hybrid.
        promotedRef.current = false;
        stalledRef.current = true;
        wcCanvasRef.current?.classList.add("is-hidden");
        canvasRef.current?.classList.remove("is-hidden");
        videoRef.current?.classList.remove("is-hidden");
        setVideoDiag("webcodecs stalled — showing live RTP (decoder kept warm)");
        const reason = classifyPresentStuck({
          preferLegacy: preferLegacyVideo(),
          hasDecoder: typeof VideoDecoder === "function",
          sawAu: sawAuRef.current,
          painted: false,
          stalled: true,
          fallbackFired: false,
        });
        clog("present stuck", {
          reason,
          hasDecoder: typeof VideoDecoder === "function",
          secure: window.isSecureContext,
        });
        setPresentStuck(reason);
      });
    }
    // Don't tear down a live decoder on every callback.
    if (!wcRef.current.isRunning() && !wcRef.current.start()) return false;
    webcodecsActiveRef.current = true;
    // Hybrid: RTP canvas visible; mark sidecar so stall never path-flips.
    wcRef.current.setPhotonSidecar(true);
    clog("webcodecs warming — RTP canvas visible, CLVD photon sidecar", {
      accel: wcRef.current.hardwareAcceleration(),
    });
    armWebCodecsFallback();
    return true;
  }

  /**
   * WebCodecs painted — photon/`input_wm` path is live.
   *
   * Hybrid (v25 feel + S_p50): keep RTP canvas visible for high paint fps;
   * WC runs in background for watermarks. Never hide RTP / go exclusive binary
   * (that killed responsiveness and left fallback at 1fps).
   */
  function promoteWebcodecsPresent() {
    if (promotedRef.current) return;
    promotedRef.current = true;
    stalledRef.current = false;
    softwarePhotonRef.current = true;
    clearRtpFallbackTimer();
    // RTP canvas stays on screen. WC canvas stays hidden (photon sidecar).
    wcCanvasRef.current?.classList.add("is-hidden");
    canvasRef.current?.classList.remove("is-hidden");
    videoRef.current?.classList.remove("is-hidden");
    setPresentMode("webcodecs");
    setPresentStuck(null);
    clog(
      "present mode: RTP canvas + WC photon sidecar (full RTP stays on host)"
    );
    playerRef.current?.promoteWebcodecs();
  }

  function attachStream(stream: MediaStream) {
    heldStreamRef.current = stream;
    // Hybrid: RTP is always the visible present. Never skip attaching just
    // because WC photon has promoted — that left paint fps stuck / black.
    if (promotedRef.current && !softwarePhotonRef.current) {
      if (heldLoggedRef.current !== stream) {
        heldLoggedRef.current = stream;
        clog("RTP stream held for fallback — WebCodecs present active");
      }
      return;
    }
    if (softwarePhotonRef.current && heldLoggedRef.current !== stream) {
      heldLoggedRef.current = stream;
      clog("RTP stream live — WC photon sidecar active");
    }
    clearRtpFallbackTimer();
    // Don't tear down a warming WebCodecs decoder — this is the safety-net
    // RTP delivery, not a switch away from an active WebCodecs present.
    if (!webcodecsActiveRef.current) {
      wcRef.current?.stop();
    }
    const track = stream.getVideoTracks()[0];
    const wantCanvas =
      !preferLegacyVideo() && !!track && canUseLowLatencyCanvas() && !!canvasRef.current;

    if (wantCanvas && track && canvasRef.current) {
      if (!viewRef.current) {
        viewRef.current = new LowLatencyCanvasView(canvasRef.current);
        // Self-heal net: the view already retries internally a few times.
        // If it still can't revive (e.g. the underlying track itself went
        // bad), re-attach the held RTP stream fresh rather than leaving the
        // canvas frozen until someone reloads the page.
        viewRef.current.setPumpDiedHandler(() => {
          const stream = heldStreamRef.current;
          if (!stream) return;
          cwarn("low-latency canvas pump died — re-attaching RTP stream");
          attachStream(stream);
        });
        viewRef.current.setStatsHandler((s) => {
          // Hybrid: RTP canvas owns paint fps; WC photon sidecar owns S_p50.
          if (softwarePhotonRef.current) {
            const fresh = inputFreshnessMs();
            const photon = photonP50Ms();
            setPresentMode("webcodecs");
            setVideoDiag(
              `LIVE RTP+WC · paint ${s.presentFps}fps · photon path live · drop=${s.dropped}${
                photon != null
                  ? ` · Φ ${photon.toFixed(0)}ms`
                  : fresh != null
                    ? ` · input ${fresh.toFixed(0)}ms`
                    : ""
              }`
            );
            setPresent({
              fps: s.presentFps,
              dropped: s.dropped,
              width: s.width,
              height: s.height,
              ageMs: s.ageMs,
              ageBand: s.ageMs <= 25 ? "ok" : s.ageMs <= 40 ? "warn" : "drop",
              ...presentPhotonFields(),
            });
            return;
          }
          if (promotedRef.current) return;
          const fresh = inputFreshnessMs();
          setVideoDiag(
            `canvas: ${s.width}×${s.height} @ ${s.presentFps}fps · ${s.ageMs.toFixed(1)}ms age${
              fresh != null ? ` · input ${fresh.toFixed(0)}ms` : ""
            } drop=${s.dropped}`
          );
          setPresentMode("canvas");
          setPresent({
            fps: s.presentFps,
            dropped: s.dropped,
            width: s.width,
            height: s.height,
            ageMs: s.ageMs,
            ageBand: s.ageMs <= 25 ? "ok" : s.ageMs <= 40 ? "warn" : "drop",
            ...presentPhotonFields(),
          });
        });
        viewRef.current.setPaintedHandler((a) => {
          playerRef.current?.echoPaintedAge({
            seq: a.seq,
            stampUs: 0,
            recvMs: a.recvMs,
            paintMs: a.paintMs,
          });
        });
        viewRef.current.setPumpDiedHandler(() => {
          const stream = heldStreamRef.current;
          if (!stream) return;
          cwarn("RTP canvas pump dead — reattaching stream (no page refresh)");
          attachStream(stream);
        });
      }
      void viewRef.current.start(track).then((ok) => {
        if (ok) {
          if (promotedRef.current) return;
          clog("present mode: low-latency canvas");
          setPresentMode("canvas");
          if (videoRef.current) {
            videoRef.current.srcObject = null;
            videoRef.current.classList.add("is-hidden");
          }
          canvasRef.current?.classList.remove("is-hidden");
          setVideoDiag(
            `canvas: ${track.getSettings().width ?? "?"}×${track.getSettings().height ?? "?"} starting`
          );
          return;
        }
        cwarn("canvas present failed — falling back to <video>");
        attachVideoFallback(stream);
      });
      return;
    }

    attachVideoFallback(stream);
  }

  function attachVideoFallback(stream: MediaStream) {
    viewRef.current?.stop();
    canvasRef.current?.classList.add("is-hidden");
    videoRef.current?.classList.remove("is-hidden");
    setPresentMode("video");

    const bind = (v: HTMLVideoElement) => {
      const logVideo = (tag: string) => {
        clog(tag, {
          videoWidth: v.videoWidth,
          videoHeight: v.videoHeight,
          readyState: v.readyState,
          paused: v.paused,
          currentTime: v.currentTime,
        });
        setVideoDiag(
          `video: ${v.videoWidth || "?"}×${v.videoHeight || "?"} rs=${v.readyState} ${v.paused ? "paused" : "playing"}`
        );
      };
      const tryPlay = (why: string) => {
        void v
          .play()
          .then(() => logVideo(`video.play ok (${why})`))
          .catch((e: unknown) => {
            const name =
              e && typeof e === "object" && "name" in e
                ? String((e as { name: string }).name)
                : "";
            if (name === "AbortError") {
              clog("play aborted (reattach)", why);
              window.setTimeout(() => {
                void v
                  .play()
                  .then(() => logVideo("video.play ok (retry)"))
                  .catch((e2) => {
                    cerror("video.play failed", e2);
                  });
              }, 50);
              return;
            }
            cerror("video.play failed (autoplay policy?)", e);
          });
      };

      v.onloadedmetadata = () => logVideo("video loadedmetadata");
      v.onplaying = () => logVideo("video playing");
      v.onwaiting = () => clog("video waiting (buffering / no keyframe?)");
      v.onstalled = () => cwarn("video stalled");
      v.onerror = () => cerror("video element error", v.error);

      if (v.srcObject === stream) {
        tryPlay("same-stream");
        return;
      }
      v.srcObject = stream;
      tryPlay("attach");
    };

    const v = videoRef.current;
    if (!v) {
      pendingStreamRef.current = stream;
      cwarn("onVideo before ref — will attach on ref");
      return;
    }
    pendingStreamRef.current = null;
    bind(v);
  }

  // No dependency array: this must run after whichever render creates the
  // video element. The identity check is what keeps that cheap — without it
  // every render re-attached the same stream and restarted the canvas.
  useEffect(() => {
    const v = videoRef.current;
    const pending = pendingStreamRef.current;
    if (v && pending && v.srcObject !== pending) {
      clog("attach pending stream after ref ready");
      pendingStreamRef.current = null;
      attachStream(pending);
    }
  });

  const playerCallbacks = usePlayerCallbacks({
    onState: (s, d) => {
      clog("ui state", s, d ?? "");
      setState(s);
      if (d) setDetail(d);
      if (s === "disconnected" || s === "error" || s === "waiting_host") {
        clearRtpFallbackTimer();
        viewRef.current?.stop();
        wcRef.current?.stop();
        webcodecsActiveRef.current = false;
        promotedRef.current = false;
        softwarePhotonRef.current = false;
        sawAuRef.current = false;
        stalledRef.current = false;
        resetInputPhoton();
        setPresent(null);
      }
    },
    onVideo: (stream) => attachStream(stream),
    onPresentPath: (path, detail) => {
      clog("present path", path, detail ?? "");
      if (path === "webcodecs" || path === "clvd") {
        if (!ensureWebCodecs()) {
          cwarn("WebCodecs present failed to start — waiting for RTP fallback");
          webcodecsActiveRef.current = false;
          const stream = heldStreamRef.current;
          if (stream) attachStream(stream);
        }
      }
      setCtxHint(secureContextHint());
    },
    onVideoAccessUnit: (au, recvMs) => {
      sawAuRef.current = true;
      if (!webcodecsActiveRef.current) {
        if (!ensureWebCodecs()) return;
      }
      wcRef.current?.push(au, recvMs);
    },
    onStreamInfo: (info) => {
      setStreamMeta(`${info.width}×${info.height}@${info.fps} ${info.codec}`);
      if (info.capture_ok === false && info.capture_hint) {
        setCaptureHint(info.capture_hint);
      } else if (info.capture_ok === true) {
        setCaptureHint(null);
      }
    },
    onPadStats: (hz, name) => {
      setPadMeta(`${hz} Hz · ${name}`);
    },
    onTelemetry: (t) => {
      rttRef.current = t.path?.rttMs ?? 0;
      setTelemetry(t);
    },
    onHostStats: (s) => setHostStats(s),
    onRegistered: (slot) => setMySlot(slot),
    onPlayersStatus: (occupied, max) => setPlayersStatus({ occupied, max }),
    onPlayerPadInfo: (slot, kind, id) => {
      setPlayerPads((prev) => ({ ...prev, [slot]: { kind, id, lastSeenAt: Date.now() } }));
    },
    onPlayerLeft: (slot) => {
      setPlayerPads((prev) => {
        const next = { ...prev };
        delete next[slot];
        return next;
      });
    },
  });

  useEffect(() => {
    const player = new CouchlinkPlayer(playerCallbacks);
    playerRef.current = player;
    const onPageHide = () => {
      clog("page hide → disconnect");
      clearRtpFallbackTimer();
      viewRef.current?.stop();
      wcRef.current?.stop();
      webcodecsActiveRef.current = false;
      promotedRef.current = false;
      player.disconnect();
    };
    window.addEventListener("pagehide", onPageHide);
    return () => {
      window.removeEventListener("pagehide", onPageHide);
      clearRtpFallbackTimer();
      viewRef.current?.stop();
      wcRef.current?.stop();
      webcodecsActiveRef.current = false;
      promotedRef.current = false;
    };
    // playerCallbacks identity is stable for the lifetime of the tab
    // eslint-disable-next-line react-hooks/exhaustive-deps -- intentional singleton player
  }, []);

  useEffect(() => {
    if (!invite.auto || autoStarted.current) return;
    if (!invite.sessionId || !invite.pin) return;
    autoStarted.current = true;
    playerRef.current?.setTurn(invite.turn);
    playerRef.current?.connect(signalingUrl, invite.sessionId, invite.pin);
  }, [invite.auto, invite.sessionId, invite.pin, signalingUrl]);

  // Create/destroy keyboard+mouse input and wire it into the player
  useEffect(() => {
    const canvas = canvasRef.current ?? stageRef.current ?? undefined;
    if (kbmActive) {
      const kbm = new KeyboardMouseInput({
        lockTarget: canvas ?? null,
        binds: kbmBinds,
      });
      kbmRef.current = kbm;
      setKbmInput(kbm);
      kbm.start();
      playerRef.current?.setKbm(kbm);
      const onLockChange = () => setPointerLocked(!!document.pointerLockElement);
      document.addEventListener("pointerlockchange", onLockChange);
      return () => {
        kbm.stop();
        kbmRef.current = null;
        setKbmInput(null);
        playerRef.current?.setKbm(null);
        document.removeEventListener("pointerlockchange", onLockChange);
        setPointerLocked(false);
      };
    } else {
      kbmRef.current?.stop();
      kbmRef.current = null;
      setKbmInput(null);
      playerRef.current?.setKbm(null);
    }
  }, [kbmActive]);

  useEffect(() => {
    kbmRef.current?.setBinds(kbmBinds);
  }, [kbmBinds]);

  // Re-detect mobile + landscape so side-mode follows a phone tilt.
  useEffect(() => {
    const sync = () => {
      setIsMobile(detectMobile());
      setLandscape(detectLandscape());
      touchInputRef.current?.refresh();
    };
    window.addEventListener("resize", sync);
    window.addEventListener("orientationchange", sync);
    window.visualViewport?.addEventListener("resize", sync);
    const mq = window.matchMedia?.("(orientation: landscape)");
    mq?.addEventListener?.("change", sync);
    return () => {
      window.removeEventListener("resize", sync);
      window.removeEventListener("orientationchange", sync);
      window.visualViewport?.removeEventListener("resize", sync);
      mq?.removeEventListener?.("change", sync);
    };
  }, []);

  // Touch controller: one shared input for the mobile layout, live for the
  // lifetime of the page. Desktop is unaffected — setTouchInput(null) when the
  // device is not mobile, and the overlay is only rendered on mobile.
  useEffect(() => {
    const input = new TouchGamepadInput();
    touchInputRef.current = input;
    if (isMobile) {
      playerRef.current?.setTouchInput(input);
    } else {
      playerRef.current?.setTouchInput(null);
    }
    return () => {
      input.detach();
      touchInputRef.current = null;
      playerRef.current?.setTouchInput(null);
    };
  }, [isMobile]);

  const connected = state === "connected" || state === "negotiating";
  const sideMode = isSideMode({ mobile: isMobile, landscape, connected });

  useEffect(() => {
    const onFs = () => setFullscreen(isNativeFullscreen());
    document.addEventListener("fullscreenchange", onFs);
    document.addEventListener("webkitfullscreenchange", onFs as EventListener);
    return () => {
      document.removeEventListener("fullscreenchange", onFs);
      document.removeEventListener("webkitfullscreenchange", onFs as EventListener);
    };
  }, []);

  useEffect(() => {
    if (!isMobile) return;
    if (sideMode) {
      setFullscreen(true);
      touchInputRef.current?.refresh();
    } else {
      void exitElementFullscreen();
      unlockOrientation();
      setFullscreen(false);
    }
  }, [sideMode, isMobile]);

  const enterSidePlay = async () => {
    const el = mobileFsRef.current;
    if (!el) return;
    await lockLandscape();
    await enterElementFullscreen(el);
    setFullscreen(true);
    touchInputRef.current?.refresh();
  };
  const livePads = useLivePads(connected && !isMobile);
  const hasPhysicalPad = livePads.length > 0;
  const hostReported = playerPads[0];
  const hostPad = {
    kind: (["dualsense", "xbox", "generic"].includes(hostReported?.kind ?? "")
      ? hostReported!.kind
      : "dualsense") as ControllerKind,
    id: hostReported?.id || "host",
    label: hostReported?.id === "keyboard+mouse" ? "Keyboard + Mouse" : "Host",
  };
  /** Fellow seated players (slot 0 is the host, drawn above; our own slot is
   * drawn live below) — announced via player_pad_info heartbeats, so everyone
   * sees who else joined and on what device, not just their own pad. */
  const otherPlayerPads = Object.entries(playerPads)
    .map(([slot, p]) => ({ slot: Number(slot), ...p }))
    .filter((p) => p.slot !== 0 && p.slot !== mySlot)
    .sort((a, b) => a.slot - b.slot);

  useEffect(() => {
    setKbmActive(!hasPhysicalPad && !isMobile);
  }, [hasPhysicalPad, isMobile]);

  const applyPastedLink = () => {
    try {
      const parsed = parseInviteString(pasteLink);
      setSessionId(parsed.sessionId);
      setPin(parsed.pin);
      if (parsed.signalingUrl) setSignalingUrl(parsed.signalingUrl);
      setPastedTurn(parsed.turn);
      setPasteError(null);
    } catch (e) {
      setPasteError(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <div
      className={`shell${fullscreen ? " is-fullscreen" : ""}${isMobile ? " is-mobile" : ""}${sideMode ? " is-side" : ""}`}
    >
      <header className="top">
        <div className="brand">
          <img className="brand-logo" src="/logo.png" alt="" width={56} height={56} />
          <div className="brand-copy">
            <h1>couchlink</h1>
            <p>HD co-play · your DualSense → host Bluetooth pad</p>
          </div>
        </div>
        <ol className="roster" aria-label="player seats">
          {([1, 2, 3, 4] as const).map((seat) => {
            const remoteSlot = seat - 1;
            const isHost = seat === 1;
            const filled =
              isHost ||
              !!playerPads[remoteSlot] ||
              (playersStatus != null && remoteSlot <= playersStatus.occupied);
            const mine = !isHost && mySlot === remoteSlot;
            const role = isHost ? "host" : filled ? (mine ? "you" : "player") : "open";
            return (
              <li
                key={seat}
                className={`roster-slot cv-p${seat}${filled ? " is-filled" : " is-open"}${mine ? " is-you" : ""}`}
              >
                <span className="roster-num">P{seat}</span>
                <span className="roster-role">{role}</span>
              </li>
            );
          })}
        </ol>
        <div className="top-pills">
          <div className={`pill state-${state}`}>{state.replace("_", " ")}</div>
        </div>
      </header>

      {!connected && (
        <section className="join">
          <label>
            Join link
            <input
              value={pasteLink}
              onChange={(e) => {
                setPasteLink(e.target.value);
                if (pasteError) setPasteError(null);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") applyPastedLink();
              }}
              placeholder="paste the link your host sent — or session:pin"
              spellCheck={false}
            />
          </label>
          <div className="actions">
            <button type="button" onClick={applyPastedLink}>
              Fill in from link
            </button>
          </div>
          {pasteError && <p className="error">{pasteError}</p>}
          <label>
            Signaling
            <input
              value={signalingUrl}
              onChange={(e) => setSignalingUrl(e.target.value)}
              spellCheck={false}
            />
          </label>
          <label>
            Session
            <input
              value={sessionId}
              onChange={(e) => setSessionId(e.target.value)}
              placeholder="friends-night"
              spellCheck={false}
            />
          </label>
          <label>
            PIN
            <input
              value={pin}
              onChange={(e) => setPin(e.target.value.replace(/\D/g, "").slice(0, 6))}
              inputMode="numeric"
              placeholder="6 digits"
            />
          </label>
          <div className="actions">
            <button
              type="button"
              className="primary"
              onClick={() => {
                playerRef.current?.setTurn(pastedTurn);
                playerRef.current?.connect(signalingUrl, sessionId, pin);
              }}
            >
              Join session
            </button>
            <button
              type="button"
              onClick={() => {
                viewRef.current?.stop();
                playerRef.current?.disconnect();
              }}
            >
              Disconnect
            </button>
          </div>
          {detail && <p className="detail">{detail}</p>}
          {ctxHint && <p className="detail">{ctxHint}</p>}
          <p className="hint">
            Plug in / pair your DualSense, then press any button so the browser
            unlocks Gamepad API. Prefer <code>http://127.0.0.1</code> / HTTPS for
            WebCodecs (DataChannel path, no jitter buffer). LAN <code>http://</code>{" "}
            falls back to RTP canvas. Add <code>?legacyVideo=1</code> to force the
            old video element.
          </p>
        </section>
      )}

      <div className="broadcast">
        <div
          className={`mobile-game${isMobile ? " is-mobile" : ""}${fullscreen || sideMode ? " is-fullscreen" : ""}${sideMode ? " is-side" : ""}`}
          ref={mobileFsRef}
        >
          <div className="stage-wrap" ref={stageRef}>
            <canvas ref={canvasRef} className="stage is-hidden" aria-label="Game stream (RTP)" />
            <canvas
              ref={wcCanvasRef}
              className="stage is-hidden"
              aria-label="Game stream (WebCodecs)"
            />
            <video ref={videoRef} className="stage" playsInline muted autoPlay />
            {state !== "connected" && (
              <div className="overlay">
                <span>{detail || "Waiting for video…"}</span>
              </div>
            )}
            {state === "connected" && videoDiag.includes("?×?") && (
              <div className="overlay overlay-dim">
                <span>{detail || "Connected — waiting for first video frame…"}</span>
              </div>
            )}
            {state === "connected" && captureHint && (
              <div className="overlay overlay-dim">
                <span>{captureHint}</span>
              </div>
            )}
            {isMobile && connected && !landscape && (
              <button type="button" className="tilt-hint" onClick={() => void enterSidePlay()}>
                <span className="tilt-hint-icon" aria-hidden>
                  ↻
                </span>
                <span>Tilt your phone sideways to play</span>
              </button>
            )}
          </div>

          {isMobile && connected && touchInputRef.current && (
            <div className="touch-dock">
              <TouchOverlay input={touchInputRef.current} />
            </div>
          )}
        </div>

        {connected && !isMobile && (
          <section className="pads" aria-live="polite">
            <div className="pads-head">
              <span className="pads-count">
                {otherPlayerPads.length > 0
                  ? `host + you +${otherPlayerPads.length}`
                  : "host + you"}
              </span>
              <span className="pads-hint">
                {hasPhysicalPad
                  ? "host’s pad · your pad"
                  : pointerLocked
                    ? "host’s pad · look locked — Esc to release"
                    : "host’s pad · click the stream to lock look"}
              </span>
            </div>
            <div className="pads-viz">
              {hostPad.id === "keyboard+mouse" ? (
                <KeyboardMouseViz input={null} seat={1} slotLabel="host" />
              ) : (
                <ControllerViz
                  pad={silhouettePad(hostPad.kind, hostPad.id, hostPad.label)}
                  seat={1}
                  slotLabel="host"
                />
              )}
              {hasPhysicalPad ? (
                <ControllerViz
                  key={`${livePads[0].index}-${livePads[0].id}`}
                  pad={livePads[0]}
                  seat={seatForRemoteSlot(mySlot)}
                  slotLabel="you"
                  active
                />
              ) : (
                <KeyboardMouseViz
                  input={kbmInput}
                  seat={seatForRemoteSlot(mySlot)}
                  slotLabel="you"
                  active
                />
              )}
              {otherPlayerPads.map((p) =>
                p.id === "keyboard+mouse" ? (
                  <KeyboardMouseViz
                    key={`remote-p${p.slot}`}
                    input={null}
                    seat={seatForRemoteSlot(p.slot)}
                    slotLabel="keyboard"
                  />
                ) : (
                  <ControllerViz
                    key={`remote-p${p.slot}`}
                    pad={silhouettePad(
                      (["dualsense", "xbox", "generic"].includes(p.kind)
                        ? p.kind
                        : "generic") as ControllerKind,
                      p.id,
                      padDisplayName(p.kind, p.id),
                    )}
                    seat={seatForRemoteSlot(p.slot)}
                    slotLabel={padDisplayName(p.kind, p.id)}
                  />
                ),
              )}
            </div>
            {!hasPhysicalPad && (
              <div className="kbm-row">
                <button
                  type="button"
                  className="kbm-keybinds-btn"
                  onClick={() => setKeybindsOpen(true)}
                >
                  ⌨ keybinds
                </button>
              </div>
            )}
          </section>
        )}
      </div>

      {keybindsOpen && !hasPhysicalPad && (
        <KeybindsModal
          binds={kbmBinds}
          onChange={setKbmBinds}
          onClose={() => setKeybindsOpen(false)}
        />
      )}

      <DebugDrawer
        telemetry={telemetry}
        hostStats={hostStats}
        present={present}
        streamInfo={streamMeta}
        presentMode={presentMode}
        inputPhoton={getInputPhotonSnapshot(rttRef.current)}
        presentStuck={presentStuck}
        playerPads={playerPads}
        mySlot={mySlot}
        myPadName={telemetry?.padName ?? null}
        myPadHz={telemetry?.padHz ?? 0}
        open={debugOpen}
        onToggle={() => setDebugOpen((o) => !o)}
      />

      <footer className="meta">
        <span>{streamMeta}</span>
        <span>{videoDiag}</span>
        <span>present: {presentMode}</span>
        <span>{padMeta}</span>
        <button
          type="button"
          className="ghost"
          onClick={() => {
            if (isMobile) {
              if (sideMode || isNativeFullscreen()) {
                unlockOrientation();
                void exitElementFullscreen();
                setFullscreen(false);
              } else {
                void enterSidePlay();
              }
              return;
            }
            const el = stageRef.current;
            if (!isNativeFullscreen() && el) {
              void enterElementFullscreen(el);
              setFullscreen(true);
            } else {
              void exitElementFullscreen();
              setFullscreen(false);
            }
          }}
        >
          {isMobile ? (sideMode ? "Exit side" : "Play sideways") : "Fullscreen"}
        </button>
      </footer>
    </div>
  );
}
