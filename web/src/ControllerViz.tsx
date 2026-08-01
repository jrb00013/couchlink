import { useEffect, useRef, useState, type ReactNode } from "react";
import { controllerKind, type ControllerKind } from "./controllerKind";

export type LivePad = {
  index: number;
  id: string;
  label: string;
  kind: ControllerKind;
  buttons: boolean[];
  axes: number[];
  /** Analog trigger values 0..1 from buttons[6]/[7]. */
  l2: number;
  r2: number;
};

function readLivePads(): LivePad[] {
  const pads = navigator.getGamepads?.() ?? [];
  const out: LivePad[] = [];
  for (const p of pads) {
    if (!p) continue;
    const cut = p.id.indexOf(" (");
    const label = (cut > 0 ? p.id.slice(0, cut) : p.id).trim() || p.id;
    out.push({
      index: p.index,
      id: p.id,
      label,
      kind: controllerKind(p.id),
      buttons: p.buttons.map((b) => !!b.pressed),
      axes: [...p.axes],
      l2: p.buttons[6]?.value ?? 0,
      r2: p.buttons[7]?.value ?? 0,
    });
  }
  return out;
}

/** Poll Gamepad API and return live pad snapshots (rAF). */
export function useLivePads(enabled: boolean): LivePad[] {
  const [pads, setPads] = useState<LivePad[]>([]);
  const raf = useRef(0);

  useEffect(() => {
    if (!enabled) {
      setPads([]);
      return;
    }

    const refreshList = () => setPads(readLivePads());
    refreshList();
    window.addEventListener("gamepadconnected", refreshList);
    window.addEventListener("gamepaddisconnected", refreshList);

    const tick = () => {
      setPads(readLivePads());
      raf.current = requestAnimationFrame(tick);
    };
    raf.current = requestAnimationFrame(tick);

    return () => {
      cancelAnimationFrame(raf.current);
      window.removeEventListener("gamepadconnected", refreshList);
      window.removeEventListener("gamepaddisconnected", refreshList);
    };
  }, [enabled]);

  return pads;
}

function stickNudge(axis: number, max = 10): number {
  return Math.max(-1, Math.min(1, axis || 0)) * max;
}

function Btn({
  pressed,
  className,
  children,
}: {
  pressed: boolean;
  className?: string;
  children?: ReactNode;
}) {
  return (
    <g className={`cv-btn${pressed ? " is-on" : ""}${className ? ` ${className}` : ""}`}>
      {children}
    </g>
  );
}

function XboxBody({ pad }: { pad: LivePad }) {
  const b = pad.buttons;
  const ax = pad.axes;
  const lx = stickNudge(ax[0]);
  const ly = stickNudge(ax[1]);
  const rx = stickNudge(ax[2]);
  const ry = stickNudge(ax[3]);

  return (
    <svg className="cv-svg cv-xbox" viewBox="0 0 360 220" aria-hidden="true">
      {/* shell */}
      <path
        className="cv-shell"
        d="M70 78c8-42 48-62 110-62h0c62 0 102 20 110 62 14 18 28 48 22 78-4 22-22 36-46 36-20 0-34-10-46-24-8-10-18-18-40-18s-32 8-40 18c-12 14-26 24-46 24-24 0-42-14-46-36-6-30 8-60 22-78z"
      />
      {/* grips shadow */}
      <path
        className="cv-grip"
        d="M78 150c-18 8-28 28-24 46 18-6 36-22 42-40zm204 0c18 8 28 28 24 46-18-6-36-22-42-40z"
      />

      {/* bumpers */}
      <Btn pressed={!!b[4]} className="cv-bumper">
        <rect x="88" y="42" width="52" height="12" rx="4" />
        <text x="114" y="51" textAnchor="middle">
          LB
        </text>
      </Btn>
      <Btn pressed={!!b[5]} className="cv-bumper">
        <rect x="220" y="42" width="52" height="12" rx="4" />
        <text x="246" y="51" textAnchor="middle">
          RB
        </text>
      </Btn>

      {/* triggers (fill height) */}
      <g className="cv-trigger">
        <rect className="cv-trigger-well" x="96" y="18" width="36" height="18" rx="3" />
        <rect
          className={`cv-trigger-fill${pad.l2 > 0.08 ? " is-on" : ""}`}
          x="96"
          y={18 + 18 * (1 - pad.l2)}
          width="36"
          height={18 * pad.l2}
          rx="3"
        />
        <text x="114" y="31" textAnchor="middle">
          LT
        </text>
      </g>
      <g className="cv-trigger">
        <rect className="cv-trigger-well" x="228" y="18" width="36" height="18" rx="3" />
        <rect
          className={`cv-trigger-fill${pad.r2 > 0.08 ? " is-on" : ""}`}
          x="228"
          y={18 + 18 * (1 - pad.r2)}
          width="36"
          height={18 * pad.r2}
          rx="3"
        />
        <text x="246" y="31" textAnchor="middle">
          RT
        </text>
      </g>

      {/* d-pad */}
      <g className="cv-dpad" transform="translate(108,118)">
        <Btn pressed={!!b[12]}>
          <rect x="-8" y="-26" width="16" height="18" rx="2" />
        </Btn>
        <Btn pressed={!!b[13]}>
          <rect x="-8" y="8" width="16" height="18" rx="2" />
        </Btn>
        <Btn pressed={!!b[14]}>
          <rect x="-26" y="-8" width="18" height="16" rx="2" />
        </Btn>
        <Btn pressed={!!b[15]}>
          <rect x="8" y="-8" width="18" height="16" rx="2" />
        </Btn>
        <circle className="cv-dpad-hub" r="6" />
      </g>

      {/* face: A B X Y (Standard: 0=A/Cross, 1=B/Circle, 2=X/Square, 3=Y/Triangle) */}
      <g className="cv-face" transform="translate(252,100)">
        <Btn pressed={!!b[0]} className="cv-a">
          <circle cy="22" r="11" />
          <text y="26" textAnchor="middle">
            A
          </text>
        </Btn>
        <Btn pressed={!!b[1]} className="cv-b">
          <circle cx="22" r="11" />
          <text x="22" y="4" textAnchor="middle">
            B
          </text>
        </Btn>
        <Btn pressed={!!b[2]} className="cv-x">
          <circle cx="-22" r="11" />
          <text x="-22" y="4" textAnchor="middle">
            X
          </text>
        </Btn>
        <Btn pressed={!!b[3]} className="cv-y">
          <circle cy="-22" r="11" />
          <text y="-18" textAnchor="middle">
            Y
          </text>
        </Btn>
      </g>

      {/* sticks */}
      <g className="cv-stick" transform={`translate(${138 + lx} ${148 + ly})`}>
        <circle className="cv-stick-well" r="22" />
        <Btn pressed={!!b[10]}>
          <circle className="cv-stick-knob" r="14" />
        </Btn>
      </g>
      <g className="cv-stick" transform={`translate(${210 + rx} ${148 + ry})`}>
        <circle className="cv-stick-well" r="22" />
        <Btn pressed={!!b[11]}>
          <circle className="cv-stick-knob" r="14" />
        </Btn>
      </g>

      {/* view / menu / guide */}
      <Btn pressed={!!b[8]} className="cv-sys">
        <rect x="148" y="92" width="18" height="10" rx="2" />
      </Btn>
      <Btn pressed={!!b[9]} className="cv-sys">
        <rect x="194" y="92" width="18" height="10" rx="2" />
      </Btn>
      <Btn pressed={!!b[16]} className="cv-guide">
        <circle cx="180" cy="78" r="10" />
      </Btn>
    </svg>
  );
}

function DualSenseBody({ pad }: { pad: LivePad }) {
  const b = pad.buttons;
  const ax = pad.axes;
  const lx = stickNudge(ax[0]);
  const ly = stickNudge(ax[1]);
  const rx = stickNudge(ax[2]);
  const ry = stickNudge(ax[3]);

  return (
    <svg className="cv-svg cv-ds" viewBox="0 0 360 220" aria-hidden="true">
      <path
        className="cv-shell"
        d="M78 70c10-38 46-54 102-54s92 16 102 54c16 20 30 52 24 82-4 20-20 34-44 34-22 0-36-12-48-26-10-12-20-20-34-20s-24 8-34 20c-12 14-26 26-48 26-24 0-40-14-44-34-6-30 8-62 24-82z"
      />
      {/* light bar */}
      <rect className="cv-lightbar" x="140" y="58" width="80" height="8" rx="3" />

      <Btn pressed={!!b[4]} className="cv-bumper">
        <rect x="90" y="40" width="48" height="11" rx="3" />
        <text x="114" y="48.5" textAnchor="middle">
          L1
        </text>
      </Btn>
      <Btn pressed={!!b[5]} className="cv-bumper">
        <rect x="222" y="40" width="48" height="11" rx="3" />
        <text x="246" y="48.5" textAnchor="middle">
          R1
        </text>
      </Btn>

      <g className="cv-trigger">
        <rect className="cv-trigger-well" x="98" y="16" width="32" height="18" rx="3" />
        <rect
          className={`cv-trigger-fill${pad.l2 > 0.08 ? " is-on" : ""}`}
          x="98"
          y={16 + 18 * (1 - pad.l2)}
          width="32"
          height={18 * pad.l2}
          rx="3"
        />
        <text x="114" y="29" textAnchor="middle">
          L2
        </text>
      </g>
      <g className="cv-trigger">
        <rect className="cv-trigger-well" x="230" y="16" width="32" height="18" rx="3" />
        <rect
          className={`cv-trigger-fill${pad.r2 > 0.08 ? " is-on" : ""}`}
          x="230"
          y={16 + 18 * (1 - pad.r2)}
          width="32"
          height={18 * pad.r2}
          rx="3"
        />
        <text x="246" y="29" textAnchor="middle">
          R2
        </text>
      </g>

      {/* d-pad */}
      <g className="cv-dpad" transform="translate(108,108)">
        <Btn pressed={!!b[12]}>
          <rect x="-7" y="-24" width="14" height="16" rx="2" />
        </Btn>
        <Btn pressed={!!b[13]}>
          <rect x="-7" y="8" width="14" height="16" rx="2" />
        </Btn>
        <Btn pressed={!!b[14]}>
          <rect x="-24" y="-7" width="16" height="14" rx="2" />
        </Btn>
        <Btn pressed={!!b[15]}>
          <rect x="8" y="-7" width="16" height="14" rx="2" />
        </Btn>
      </g>

      {/* face: Cross Circle Square Triangle */}
      <g className="cv-face" transform="translate(252,108)">
        <Btn pressed={!!b[0]} className="cv-cross">
          <circle cy="22" r="11" />
          <text y="26" textAnchor="middle">
            ✕
          </text>
        </Btn>
        <Btn pressed={!!b[1]} className="cv-circle">
          <circle cx="22" r="11" />
          <text x="22" y="5" textAnchor="middle">
            ○
          </text>
        </Btn>
        <Btn pressed={!!b[2]} className="cv-square">
          <circle cx="-22" r="11" />
          <text x="-22" y="5" textAnchor="middle">
            □
          </text>
        </Btn>
        <Btn pressed={!!b[3]} className="cv-tri">
          <circle cy="-22" r="11" />
          <text y="-17" textAnchor="middle">
            △
          </text>
        </Btn>
      </g>

      <g className="cv-stick" transform={`translate(${138 + lx} ${150 + ly})`}>
        <circle className="cv-stick-well" r="20" />
        <Btn pressed={!!b[10]}>
          <circle className="cv-stick-knob" r="13" />
        </Btn>
      </g>
      <g className="cv-stick" transform={`translate(${222 + rx} ${150 + ry})`}>
        <circle className="cv-stick-well" r="20" />
        <Btn pressed={!!b[11]}>
          <circle className="cv-stick-knob" r="13" />
        </Btn>
      </g>

      <Btn pressed={!!b[8]} className="cv-sys">
        <rect x="150" y="88" width="14" height="8" rx="2" />
      </Btn>
      <Btn pressed={!!b[9]} className="cv-sys">
        <rect x="196" y="88" width="14" height="8" rx="2" />
      </Btn>
      <Btn pressed={!!b[16]} className="cv-ps">
        <circle cx="180" cy="78" r="8" />
      </Btn>
      <Btn pressed={!!b[17]} className="cv-touch">
        <rect x="155" y="68" width="50" height="14" rx="3" />
      </Btn>
    </svg>
  );
}

function GenericBody({ pad }: { pad: LivePad }) {
  // Same layout as Xbox with neutral labels — Standard Gamepad mapping.
  return <XboxBody pad={pad} />;
}

export function ControllerViz({ pad, active }: { pad: LivePad; active?: boolean }) {
  const Body =
    pad.kind === "dualsense" ? DualSenseBody : pad.kind === "xbox" ? XboxBody : GenericBody;

  return (
    <figure className={`cv${active ? " is-active" : ""}`} title={pad.id}>
      <Body pad={pad} />
      <figcaption className="cv-cap">
        <span className="cv-slot">P{pad.index + 1}</span>
        <span className="cv-name">{pad.label}</span>
        {active && <span className="cv-active">active</span>}
      </figcaption>
    </figure>
  );
}
