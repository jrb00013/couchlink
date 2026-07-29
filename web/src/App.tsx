import { useEffect, useRef, useState } from "react";
import { CouchlinkPlayer, type ConnectionState } from "./player";
import { clog, cerror, cwarn } from "./log";
import { usePlayerCallbacks } from "./usePlayerCallbacks";
import "./App.css";

const DEFAULT_WS =
  typeof location !== "undefined" && location.port === "5174"
    ? `${location.protocol === "https:" ? "wss" : "ws"}://${location.hostname}:8443/ws`
    : `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`;

type ConnectedPad = {
  index: number;
  id: string;
  label: string;
};

/** Strip vendor/product noise from Gamepad.id for a readable label. */
function cleanPadLabel(id: string): string {
  const cut = id.indexOf(" (");
  return (cut > 0 ? id.slice(0, cut) : id).trim() || id;
}

function readConnectedPads(): ConnectedPad[] {
  const pads = navigator.getGamepads?.() ?? [];
  const out: ConnectedPad[] = [];
  for (const p of pads) {
    if (!p) continue;
    out.push({ index: p.index, id: p.id, label: cleanPadLabel(p.id) });
  }
  return out;
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
  const [state, setState] = useState<ConnectionState>("disconnected");
  const [detail, setDetail] = useState("");
  const [streamMeta, setStreamMeta] = useState("—");
  const [captureHint, setCaptureHint] = useState<string | null>(null);
  const [padMeta, setPadMeta] = useState("press a button on your DualSense / pad");
  const [pads, setPads] = useState<ConnectedPad[]>([]);
  const [fullscreen, setFullscreen] = useState(false);

  const videoRef = useRef<HTMLVideoElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const playerRef = useRef<CouchlinkPlayer | null>(null);
  const autoStarted = useRef(false);
  const pendingStreamRef = useRef<MediaStream | null>(null);
  const [videoDiag, setVideoDiag] = useState("video: —");

  function attachStream(stream: MediaStream) {
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
            const name = e && typeof e === "object" && "name" in e ? String((e as { name: string }).name) : "";
            if (name === "AbortError") {
              clog("play aborted (reattach)", why);
              window.setTimeout(() => {
                void v.play().then(() => logVideo("video.play ok (retry)")).catch((e2) => {
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

  useEffect(() => {
    const v = videoRef.current;
    const pending = pendingStreamRef.current;
    if (v && pending) {
      clog("attach pending stream after ref ready");
      attachStream(pending);
    }
  });

  const playerCallbacks = usePlayerCallbacks({
    onState: (s, d) => {
      clog("ui state", s, d ?? "");
      setState(s);
      if (d) setDetail(d);
    },
    onVideo: (stream) => attachStream(stream),
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
  });

  useEffect(() => {
    const player = new CouchlinkPlayer(playerCallbacks);
    playerRef.current = player;
    const onPageHide = () => {
      clog("page hide → disconnect");
      player.disconnect();
    };
    window.addEventListener("pagehide", onPageHide);
    return () => {
      window.removeEventListener("pagehide", onPageHide);
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

  const connected = state === "connected" || state === "negotiating";

  useEffect(() => {
    if (!connected) {
      setPads([]);
      return;
    }
    const refresh = () => setPads(readConnectedPads());
    refresh();
    window.addEventListener("gamepadconnected", refresh);
    window.addEventListener("gamepaddisconnected", refresh);
    // Browsers often only expose pads after a button press; poll lightly.
    const timer = window.setInterval(refresh, 1000);
    return () => {
      window.removeEventListener("gamepadconnected", refresh);
      window.removeEventListener("gamepaddisconnected", refresh);
      window.clearInterval(timer);
    };
  }, [connected]);

  return (
    <div className={`shell ${fullscreen ? "is-fullscreen" : ""}`}>
      <header className="top">
        <div className="brand">
          <img className="brand-logo" src="/logo.png" alt="" width={56} height={56} />
          <div className="brand-copy">
            <h1>couchlink</h1>
            <p>HD co-play · your DualSense → host Bluetooth pad</p>
          </div>
        </div>
        <div className={`pill state-${state}`}>{state.replace("_", " ")}</div>
      </header>

      {!connected && (
        <section className="join">
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
                playerRef.current?.setTurn(invite.turn);
                playerRef.current?.connect(signalingUrl, sessionId, pin);
              }}
            >
              Join session
            </button>
            <button
              type="button"
              onClick={() => playerRef.current?.disconnect()}
            >
              Disconnect
            </button>
          </div>
          {detail && <p className="detail">{detail}</p>}
          <p className="hint">
            Plug in / pair your DualSense, then press any button so the browser
            unlocks Gamepad API. Open DevTools → Console and filter{" "}
            <code>couchlink</code> for connection logs (<code>?debug=0</code>{" "}
            to silence).
          </p>
        </section>
      )}

      <div className="broadcast">
        <div className="stage-wrap" ref={stageRef}>
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

        {connected && (
          <section className="pads" aria-live="polite">
            <div className="pads-head">
              <span className="pads-count">
                {pads.length === 0
                  ? "No controllers"
                  : `${pads.length} controller${pads.length === 1 ? "" : "s"}`}
              </span>
              {pads.length > 0 && (
                <span className="pads-hint">first pad is sent to the host</span>
              )}
            </div>
            {pads.length === 0 ? (
              <p className="pads-empty">
                Pair a pad, then press any button so the browser unlocks it.
              </p>
            ) : (
              <ul className="pads-list">
                {pads.map((pad, i) => (
                  <li
                    key={`${pad.index}-${pad.id}`}
                    className={`pads-item${i === 0 ? " is-active" : ""}`}
                    title={pad.id}
                  >
                    <span className="pads-slot">P{pad.index + 1}</span>
                    <span className="pads-name">{pad.label}</span>
                    {i === 0 && <span className="pads-active">active</span>}
                  </li>
                ))}
              </ul>
            )}
          </section>
        )}
      </div>

      <footer className="meta">
        <span>{streamMeta}</span>
        <span>{videoDiag}</span>
        <span>{padMeta}</span>
        <button
          type="button"
          className="ghost"
          onClick={() => {
            const el = stageRef.current;
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
