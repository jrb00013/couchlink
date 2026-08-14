/**
 * On-screen touch controller for the mobile layout.
 *
 * Renders two dynamic-origin stick zones (left half / right half) plus
 * hit-testable button clusters (D-pad, face buttons, shoulders, triggers,
 * Options/Create). The same element is used in two placements:
 *
 *  - docked: in normal flow below the video (not fullscreen), full opacity
 *  - overlay: absolutely over the stage during fullscreen, low opacity
 *
 * The underlying {@link TouchGamepadInput} converts touches into the same
 * DualSense-shaped PadState the keyboard/mouse source produces, so the host
 * sees an identical controller. A rAF loop reads `visual()` and drives DOM
 * directly (no React re-render churn on every touch move).
 */

import { useEffect, useRef } from "react";
import { TouchGamepadInput, type StickVisual, type TouchButton } from "./touchPad";

export const TOUCH_BUTTONS: TouchButton[] = [
  "cross",
  "circle",
  "square",
  "triangle",
  "l1",
  "r1",
  "l2",
  "r2",
  "dpad_up",
  "dpad_down",
  "dpad_left",
  "dpad_right",
  "options",
  "create",
];

export function TouchOverlay({ input }: { input: TouchGamepadInput }) {
  const rootRef = useRef<HTMLDivElement>(null);
  const leftZoneRef = useRef<HTMLDivElement>(null);
  const rightZoneRef = useRef<HTMLDivElement>(null);
  const leftOriginRef = useRef<HTMLDivElement>(null);
  const rightOriginRef = useRef<HTMLDivElement>(null);
  const leftKnobRef = useRef<HTMLDivElement>(null);
  const rightKnobRef = useRef<HTMLDivElement>(null);
  const btnRefs = useRef<Partial<Record<TouchButton, HTMLButtonElement>>>({});

  useEffect(() => {
    const root = rootRef.current;
    if (root) input.attach(root);
    return () => input.detach();
  }, [input]);

  useEffect(() => {
    let raf = 0;
    const tick = () => {
      const v = input.visual();
      paintStick(leftOriginRef.current, leftKnobRef.current, v.left);
      paintStick(rightOriginRef.current, rightKnobRef.current, v.right);
      for (const b of TOUCH_BUTTONS) {
        btnRefs.current[b]?.classList.toggle("is-pressed", v.buttons[b]);
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [input]);

  const setBtnRef = (b: TouchButton) => (el: HTMLButtonElement | null) => {
    if (el) btnRefs.current[b] = el;
  };

  return (
    <div className="touch-overlay" ref={rootRef}>
      <div className="touch-zone touch-zone-left" ref={leftZoneRef}>
        <div className="touch-stick touch-stick-left">
          <div className="touch-stick-origin" ref={leftOriginRef} />
          <div className="touch-stick-knob" ref={leftKnobRef} />
        </div>
      </div>

      <div className="touch-zone touch-zone-right" ref={rightZoneRef}>
        <div className="touch-stick touch-stick-right">
          <div className="touch-stick-origin" ref={rightOriginRef} />
          <div className="touch-stick-knob" ref={rightKnobRef} />
        </div>
      </div>

      <div className="touch-btns touch-btns-dpad">
        <button type="button" data-touch-btn="dpad_up" ref={setBtnRef("dpad_up")} aria-label="D-pad up">▲</button>
        <button type="button" data-touch-btn="dpad_left" ref={setBtnRef("dpad_left")} aria-label="D-pad left">◀</button>
        <button type="button" data-touch-btn="dpad_down" ref={setBtnRef("dpad_down")} aria-label="D-pad down">▼</button>
        <button type="button" data-touch-btn="dpad_right" ref={setBtnRef("dpad_right")} aria-label="D-pad right">▶</button>
      </div>

      <div className="touch-btns touch-btns-face">
        <button type="button" data-touch-btn="triangle" ref={setBtnRef("triangle")} aria-label="Triangle">△</button>
        <button type="button" data-touch-btn="square" ref={setBtnRef("square")} aria-label="Square">□</button>
        <button type="button" data-touch-btn="circle" ref={setBtnRef("circle")} aria-label="Circle">○</button>
        <button type="button" data-touch-btn="cross" ref={setBtnRef("cross")} aria-label="Cross">✕</button>
      </div>

      <div className="touch-btns touch-btns-shoulder touch-btns-shoulder-l">
        <button type="button" data-touch-btn="l2" ref={setBtnRef("l2")} aria-label="L2">L2</button>
        <button type="button" data-touch-btn="l1" ref={setBtnRef("l1")} aria-label="L1">L1</button>
      </div>

      <div className="touch-btns touch-btns-shoulder touch-btns-shoulder-r">
        <button type="button" data-touch-btn="r2" ref={setBtnRef("r2")} aria-label="R2">R2</button>
        <button type="button" data-touch-btn="r1" ref={setBtnRef("r1")} aria-label="R1">R1</button>
      </div>

      <div className="touch-btns touch-btns-menu">
        <button type="button" data-touch-btn="create" ref={setBtnRef("create")} aria-label="Create">▤</button>
        <button type="button" data-touch-btn="options" ref={setBtnRef("options")} aria-label="Options">≡</button>
      </div>
    </div>
  );
}

function paintStick(origin: HTMLDivElement | null, knob: HTMLDivElement | null, s: StickVisual) {
  if (!origin || !knob) return;
  const cls = origin.classList;
  cls.toggle("is-active", s.active);
  if (s.active) {
    origin.style.transform = `translate(${s.originX}px, ${s.originY}px) translate(-50%, -50%)`;
    knob.style.transform = `translate(${s.dx}px, ${s.dy}px) translate(-50%, -50%)`;
  }
}