import { useEffect, useRef, useState } from "react";
import type { KeyboardMouseInput, KbmSnapshot } from "./keyboardMouse";
import { formatKbmCode } from "./kbmBinds";
import { SEAT_LABEL, seatClass, type Seat } from "./seat";

const IDLE: KbmSnapshot = {
  keys: [],
  mouseButtons: 0,
  lookX: 0,
  lookY: 0,
  locked: false,
};

type KeySpec = { code: string; label: string; x: number; y: number; w: number; h?: number };

/** Compact 60%-ish board — enough to read as a keyboard, not a 104-key replica. */
const KEYS: KeySpec[] = [
  { code: "Escape", label: "esc", x: 8, y: 10, w: 28 },
  { code: "Digit1", label: "1", x: 40, y: 10, w: 18 },
  { code: "Digit2", label: "2", x: 60, y: 10, w: 18 },
  { code: "Digit3", label: "3", x: 80, y: 10, w: 18 },
  { code: "Digit4", label: "4", x: 100, y: 10, w: 18 },
  { code: "Digit5", label: "5", x: 120, y: 10, w: 18 },
  { code: "Digit6", label: "6", x: 140, y: 10, w: 18 },
  { code: "Digit7", label: "7", x: 160, y: 10, w: 18 },
  { code: "Digit8", label: "8", x: 180, y: 10, w: 18 },
  { code: "Digit9", label: "9", x: 200, y: 10, w: 18 },
  { code: "Digit0", label: "0", x: 220, y: 10, w: 18 },

  { code: "Tab", label: "tab", x: 8, y: 32, w: 28 },
  { code: "KeyQ", label: "Q", x: 40, y: 32, w: 18 },
  { code: "KeyW", label: "W", x: 60, y: 32, w: 18 },
  { code: "KeyE", label: "E", x: 80, y: 32, w: 18 },
  { code: "KeyR", label: "R", x: 100, y: 32, w: 18 },
  { code: "KeyT", label: "T", x: 120, y: 32, w: 18 },
  { code: "KeyY", label: "Y", x: 140, y: 32, w: 18 },
  { code: "KeyU", label: "U", x: 160, y: 32, w: 18 },
  { code: "KeyI", label: "I", x: 180, y: 32, w: 18 },
  { code: "KeyO", label: "O", x: 200, y: 32, w: 18 },
  { code: "KeyP", label: "P", x: 220, y: 32, w: 18 },

  { code: "KeyA", label: "A", x: 48, y: 54, w: 18 },
  { code: "KeyS", label: "S", x: 68, y: 54, w: 18 },
  { code: "KeyD", label: "D", x: 88, y: 54, w: 18 },
  { code: "KeyF", label: "F", x: 108, y: 54, w: 18 },
  { code: "KeyG", label: "G", x: 128, y: 54, w: 18 },
  { code: "KeyH", label: "H", x: 148, y: 54, w: 18 },
  { code: "KeyJ", label: "J", x: 168, y: 54, w: 18 },
  { code: "KeyK", label: "K", x: 188, y: 54, w: 18 },
  { code: "KeyL", label: "L", x: 208, y: 54, w: 18 },

  { code: "ShiftLeft", label: "shift", x: 8, y: 76, w: 36 },
  { code: "KeyZ", label: "Z", x: 48, y: 76, w: 18 },
  { code: "KeyX", label: "X", x: 68, y: 76, w: 18 },
  { code: "KeyC", label: "C", x: 88, y: 76, w: 18 },
  { code: "KeyV", label: "V", x: 108, y: 76, w: 18 },
  { code: "KeyB", label: "B", x: 128, y: 76, w: 18 },
  { code: "KeyN", label: "N", x: 148, y: 76, w: 18 },
  { code: "KeyM", label: "M", x: 168, y: 76, w: 18 },
  { code: "ShiftRight", label: "shift", x: 190, y: 76, w: 48 },

  { code: "ControlLeft", label: "ctrl", x: 8, y: 98, w: 28 },
  { code: "Space", label: "", x: 48, y: 98, w: 120 },
  { code: "ArrowLeft", label: "←", x: 178, y: 98, w: 18 },
  { code: "ArrowUp", label: "↑", x: 198, y: 98, w: 18 },
  { code: "ArrowDown", label: "↓", x: 218, y: 98, w: 18 },
  { code: "ArrowRight", label: "→", x: 238, y: 98, w: 18 },
];

function KeyCap({ spec, on }: { spec: KeySpec; on: boolean }) {
  const h = spec.h ?? 18;
  return (
    <g className={`kbmv-key${on ? " is-on" : ""}`}>
      <rect x={spec.x} y={spec.y} width={spec.w} height={h} rx="3" />
      {spec.label ? (
        <text x={spec.x + spec.w / 2} y={spec.y + h / 2 + 3.2} textAnchor="middle">
          {spec.label}
        </text>
      ) : null}
    </g>
  );
}

export function KeyboardMouseViz({
  input,
  active,
  seat,
  slotLabel,
}: {
  input: KeyboardMouseInput | null;
  active?: boolean;
  seat: Seat;
  slotLabel?: string;
}) {
  const [live, setLive] = useState<KbmSnapshot>(IDLE);
  const raf = useRef(0);

  useEffect(() => {
    if (!input) {
      setLive(IDLE);
      return;
    }
    const tick = () => {
      setLive(input.snapshot());
      raf.current = requestAnimationFrame(tick);
    };
    raf.current = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf.current);
  }, [input]);

  const held = new Set(live.keys);
  const lmb = !!(live.mouseButtons & 1);
  const mmb = !!(live.mouseButtons & 2);
  const rmb = !!(live.mouseButtons & 4);
  const look = Math.hypot(live.lookX, live.lookY) > 0.04;
  const mx = 332 + live.lookX * 10;
  const my = 78 + live.lookY * 10;

  const shiftOn = held.has("ShiftLeft") || held.has("ShiftRight");

  return (
    <figure className={`cv kbmv ${seatClass(seat)}${active ? " is-active" : ""}`}>
      <svg className="cv-svg kbmv-svg" viewBox="0 0 400 128" aria-hidden="true">
        <rect className="kbmv-board" x="4" y="6" width="260" height="116" rx="8" />
        {KEYS.map((spec) => (
          <KeyCap
            key={spec.code}
            spec={spec}
            on={
              spec.code === "ShiftLeft" || spec.code === "ShiftRight"
                ? shiftOn
                : held.has(spec.code)
            }
          />
        ))}

        {/* mouse */}
        <g className="kbmv-mouse">
          <path
            className={`kbmv-mouse-body${lmb || rmb || mmb || look ? " is-on" : ""}`}
            d="M318 28c0-12 10-22 22-22s22 10 22 22v58c0 18-8 30-22 30s-22-12-22-30z"
          />
          <g className={`kbmv-key${lmb ? " is-on" : ""}`}>
            <path d="M320 30c0-10 8-18 20-20v28h-20z" />
          </g>
          <g className={`kbmv-key${rmb ? " is-on" : ""}`}>
            <path d="M360 30c0-10-8-18-20-20v28h20z" />
          </g>
          <rect
            className={`kbmv-wheel${mmb ? " is-on" : ""}`}
            x="337"
            y="32"
            width="6"
            height="16"
            rx="2"
          />
          <circle className="kbmv-look" cx={mx} cy={my} r="5" />
        </g>
      </svg>
      <figcaption className="cv-cap">
        <span className="cv-slot">{SEAT_LABEL[seat]}</span>
        <span className="cv-name">
          {slotLabel ?? "keyboard + mouse"}
          {active
            ? live.locked
              ? " · look locked"
              : " · click stream to lock"
            : ""}
        </span>
        {held.size > 0 || lmb || rmb || mmb ? (
          <span className="cv-active">
            {[...held].slice(0, 3).map(formatKbmCode).join(" ") ||
              (lmb ? "LMB" : rmb ? "RMB" : "MMB")}
          </span>
        ) : null}
      </figcaption>
    </figure>
  );
}
