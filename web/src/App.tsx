import { useEffect, useRef, useState } from "react";
import { CouchlinkPlayer, type ConnectionState } from "./player";
import "./App.css";

const DEFAULT_WS =
  typeof location !== "undefined" && location.port === "5174"
    ? `${location.protocol === "https:" ? "wss" : "ws"}://${location.hostname}:8443/ws`
    : `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`;

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
  const [padMeta, setPadMeta] = useState("press a button on your DualSense / pad");
  const [fullscreen, setFullscreen] = useState(false);

  const videoRef = useRef<HTMLVideoElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const playerRef = useRef<CouchlinkPlayer | null>(null);
  const autoStarted = useRef(false);

  useEffect(() => {
    const player = new CouchlinkPlayer({
      onState: (s, d) => {
        setState(s);
        if (d) setDetail(d);
      },
      onVideo: (stream) => {
        const v = videoRef.current;
        if (v) {
          v.srcObject = stream;
          void v.play().catch(() => undefined);
        }
      },
      onStreamInfo: (info) => {
        setStreamMeta(`${info.width}×${info.height}@${info.fps} ${info.codec}`);
      },
      onPadStats: (hz, name) => {
        setPadMeta(`${hz} Hz · ${name}`);
      },
    });
    playerRef.current = player;
    return () => player.disconnect();
  }, []);

  useEffect(() => {
    if (!invite.auto || autoStarted.current) return;
    if (!invite.sessionId || !invite.pin) return;
    autoStarted.current = true;
    playerRef.current?.setTurn(invite.turn);
    playerRef.current?.connect(signalingUrl, invite.sessionId, invite.pin);
  }, [invite.auto, invite.sessionId, invite.pin, signalingUrl]);

  const connected = state === "connected" || state === "negotiating";

  return (
    <div className={`shell ${fullscreen ? "is-fullscreen" : ""}`}>
      <header className="top">
        <div className="brand">
          <h1>couchlink</h1>
          <p>HD co-play · your DualSense → host Bluetooth pad</p>
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
            unlocks Gamepad API. On the host it appears as a Bluetooth DualSense
            for PCSX2 / RPCS3.
          </p>
        </section>
      )}

      <div className="stage-wrap" ref={stageRef}>
        <video ref={videoRef} className="stage" playsInline muted autoPlay />
        {state !== "connected" && (
          <div className="overlay">
            <span>{detail || "Waiting for video…"}</span>
          </div>
        )}
      </div>

      <footer className="meta">
        <span>{streamMeta}</span>
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
