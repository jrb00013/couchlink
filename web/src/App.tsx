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
import { ControllerViz, useLivePads } from "./ControllerViz";
import { clog, cerror, cwarn } from "./log";
import { usePlayerCallbacks } from "./usePlayerCallbacks";
import DebugDrawer, { type PresentSummary } from "./DebugDrawer";
import { KeyboardMouseInput } from "./keyboardMouse";
import { detectMobile } from "./mobile";
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
  } | null>(null);
  /** Session occupancy snapshot — "N/3 players connected". */
  const [playersStatus, setPlayersStatus] = useState<{
    occupied: number;
    max: number;
  } | null>(null);
  const [present, setPresent] = useState<PresentSummary | null>(null);
  const [debugOpen, setDebugOpen] = useState(false);
  const [kbmActive, setKbmActive] = useState(false);
  const [pointerLocked, setPointerLocked] = useState(false);
  const kbmRef = useRef<KeyboardMouseInput | null>(null);
  const [isMobile, setIsMobile] = useState(() => detectMobile());
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
  /** WebCodecs has painted and owns the visible canvas (RTP no longer on screen). */
  const promotedRef = useRef(false);
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
    rtpFallbackTimer.current = window.setTimeout(() => {
      rtpFallbackTimer.current = null;
      if (!webcodecsActiveRef.current) return;
      if (wcRef.current?.hasPainted()) return;
      cwarn("WebCodecs produced no frames — falling back to RTP canvas");
      webcodecsActiveRef.current = false;
      promotedRef.current = false;
      playerRef.current?.preferRtpPresent();
      wcRef.current?.stop();
      wcCanvasRef.current?.classList.add("is-hidden");
      canvasRef.current?.classList.remove("is-hidden");
      setVideoDiag("webcodecs: no frames — RTP fallback");
    }, 2500);
  }

  function ensureWebCodecs(): boolean {
    if (preferLegacyVideo() || !canUseWebCodecs() || !wcCanvasRef.current) return false;
    if (!wcRef.current) {
      wcRef.current = new WebCodecsCanvasView(wcCanvasRef.current);
      wcRef.current.setStatsHandler((s) => {
        clearRtpFallbackTimer();
        setVideoDiag(
          `webcodecs: ${s.width}×${s.height} @ ${s.presentFps}fps drop=${s.dropped} dec=${s.decodeMs.toFixed(1)}ms`
        );
        setPresentMode("webcodecs");
        setPresent({ fps: s.presentFps, dropped: s.dropped, width: s.width, height: s.height });
      });
      wcRef.current.setKeyframeHandler(() => {
        playerRef.current?.requestVideoKeyframe();
      });
      // Hand the visible canvas over to WebCodecs only once it has actually
      // painted — until then RTP stays on screen as the safety net.
      wcRef.current.setFirstPaintHandler(() => {
        promoteWebcodecsPresent();
      });
    }
    // Don't tear down a live decoder on every callback.
    if (!wcRef.current.isRunning() && !wcRef.current.start()) return false;
    webcodecsActiveRef.current = true;
    clog("webcodecs decoder warming — RTP stays live until first paint");
    armWebCodecsFallback();
    return true;
  }

  /** WebCodecs painted its first frame — take over the canvas and cut RTP. */
  function promoteWebcodecsPresent() {
    if (promotedRef.current) return;
    promotedRef.current = true;
    clearRtpFallbackTimer();
    viewRef.current?.stop();
    if (videoRef.current) {
      videoRef.current.srcObject = null;
      videoRef.current.classList.add("is-hidden");
    }
    canvasRef.current?.classList.add("is-hidden");
    wcCanvasRef.current?.classList.remove("is-hidden");
    setPresentMode("webcodecs");
    clog("present mode: WebCodecs + CLVD (promoted after first paint)");
    playerRef.current?.promoteWebcodecs();
  }

  function attachStream(stream: MediaStream) {
    heldStreamRef.current = stream;
    // WebCodecs owns the canvas once promoted — keep the RTP stream for
    // fallback only. During warm-up it is NOT promoted, so RTP keeps painting
    // as the visible safety net while the WebCodecs decoder warms up.
    if (promotedRef.current) {
      if (heldLoggedRef.current !== stream) {
        heldLoggedRef.current = stream;
        clog("RTP stream held for fallback — WebCodecs present active");
      }
      return;
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
        viewRef.current.setStatsHandler((s) => {
          setVideoDiag(
            `canvas: ${s.width}×${s.height} @ ${s.presentFps}fps drop=${s.dropped}`
          );
          setPresentMode("canvas");
          setPresent({ fps: s.presentFps, dropped: s.dropped, width: s.width, height: s.height });
        });
      }
      void viewRef.current.start(track).then((ok) => {
        if (ok) {
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
        setPresent(null);
      }
    },
    onVideo: (stream) => attachStream(stream),
    onPresentPath: (path, detail) => {
      clog("present path", path, detail ?? "");
      if (path === "webcodecs") {
        if (!ensureWebCodecs()) {
          cwarn("WebCodecs present failed to start — waiting for RTP fallback");
          webcodecsActiveRef.current = false;
          const stream = heldStreamRef.current;
          if (stream) attachStream(stream);
        }
      }
      setCtxHint(secureContextHint());
    },
    onVideoAccessUnit: (au) => {
      if (!webcodecsActiveRef.current) {
        if (!ensureWebCodecs()) return;
      }
      wcRef.current?.push(au);
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
    onTelemetry: (t) => setTelemetry(t),
    onHostStats: (s) => setHostStats(s),
    onPlayersStatus: (occupied, max) => setPlayersStatus({ occupied, max }),
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
      const kbm = new KeyboardMouseInput({ lockTarget: canvas ?? null });
      kbmRef.current = kbm;
      kbm.start();
      playerRef.current?.setKbm(kbm);
      const onLockChange = () => setPointerLocked(!!document.pointerLockElement);
      document.addEventListener("pointerlockchange", onLockChange);
      return () => {
        kbm.stop();
        kbmRef.current = null;
        playerRef.current?.setKbm(null);
        document.removeEventListener("pointerlockchange", onLockChange);
        setPointerLocked(false);
      };
    } else {
      kbmRef.current?.stop();
      kbmRef.current = null;
      playerRef.current?.setKbm(null);
    }
  }, [kbmActive]);

  // Re-detect mobile on resize/orientation so the layout follows the device.
  useEffect(() => {
    const onResize = () => setIsMobile(detectMobile());
    window.addEventListener("resize", onResize);
    window.addEventListener("orientationchange", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      window.removeEventListener("orientationchange", onResize);
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
  const livePads = useLivePads(true);

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
    <div className={`shell ${fullscreen ? "is-fullscreen" : ""} ${isMobile ? "is-mobile" : ""}`}>
      <header className="top">
        <div className="brand">
          <img className="brand-logo" src="/logo.png" alt="" width={56} height={56} />
          <div className="brand-copy">
            <h1>couchlink</h1>
            <p>HD co-play · your DualSense → host Bluetooth pad</p>
          </div>
        </div>
        <div className="top-pills">
          <div className={`pill state-${state}`}>{state.replace("_", " ")}</div>
          {playersStatus && (
            <div className="pill" title="players connected">
              {playersStatus.occupied}/{playersStatus.max} players
            </div>
          )}
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
          className={`mobile-game${isMobile ? " is-mobile" : ""}${fullscreen ? " is-fullscreen" : ""}`}
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
          </div>

          {isMobile && connected && touchInputRef.current && (
            <div className="touch-dock">
              <TouchOverlay input={touchInputRef.current} />
            </div>
          )}
        </div>

        {!isMobile && livePads.length > 0 && (
          <section className="pads" aria-live="polite">
            <div className="pads-head">
              <span className="pads-count">
                {livePads.length} controller{livePads.length === 1 ? "" : "s"}
              </span>
              <span className="pads-hint">first pad is sent to the host</span>
            </div>
            <div className="pads-viz">
              {livePads.map((pad, i) => (
                <ControllerViz
                  key={`${pad.index}-${pad.id}`}
                  pad={pad}
                  active={i === 0}
                />
              ))}
            </div>
          </section>
        )}
        {connected && !isMobile && livePads.length === 0 && (
          <section className="pads" aria-live="polite">
            <p className="pads-empty">
              Pair a pad, then press any button so the browser unlocks it.
            </p>
            <div className="kbm-row">
              <button
                type="button"
                className={`kbm-toggle ${kbmActive ? "is-active" : ""}`}
                onClick={() => setKbmActive((v) => !v)}
              >
                {kbmActive ? "⌨ keyboard+mouse ON" : "⌨ use keyboard+mouse"}
              </button>
              {kbmActive && (
                <span className="kbm-hint">
                  {pointerLocked
                    ? "🔒 mouse locked — Esc to release"
                    : "click stream to lock mouse · WASD=move · LMB=R2 · RMB=L2 · Space=✕ · E=△ · Q=□ · F=○"}
                </span>
              )}
            </div>
          </section>
        )}
      </div>

      <DebugDrawer
        telemetry={telemetry}
        hostStats={hostStats}
        present={present}
        streamInfo={streamMeta}
        presentMode={presentMode}
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
            // On mobile the fullscreen target wraps stage + touch controller so
            // the controller overlays the video at low opacity; desktop keeps
            // fullscreening the stage element as before.
            const el = isMobile ? mobileFsRef.current : stageRef.current;
            if (!document.fullscreenElement && el) {
              void el.requestFullscreen();
              setFullscreen(true);
            } else {
              void document.exitFullscreen();
              setFullscreen(false);
            }
          }}
        >
          Fullscreen
        </button>
      </footer>
    </div>
  );
}
