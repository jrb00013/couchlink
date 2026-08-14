/**
 * Touch-screen → DualSense PadState emulation for the mobile layout.
 *
 * The screen is split into two halves, like a mobile shooter:
 *   left half  → left stick  (movement)
 *   right half → right stick (camera / aim)
 *
 * Sticks use a *dynamic origin*: wherever the thumb lands in a half becomes
 * the stick's center, and deflection away from that origin drives the axis.
 * That lets the player re-grip without losing control, which is how touch
 * controller games behave.
 *
 * Face buttons (✕△□○), the D-pad, L1/L2 and R1/R2, and Options/Create are
 * hit-testable touch targets overlaid on the halves; a touch that starts on
 * one of them is a button press, not a stick move.
 *
 * Output is the same DualSense-shaped `PadState` the keyboard/mouse source
 * produces, so the host sees an identical controller either way.
 */

import { BTN, type PadState } from "./clpd";

export const TOUCH_RADIUS = 64;
/** Fraction of the surface a stick origin can deflect before clamping. */
export const TOUCH_DEFLECTION = 1;
export const TOUCH_MAX = 255;

export type TouchButton =
  | "cross"
  | "circle"
  | "square"
  | "triangle"
  | "l1"
  | "r1"
  | "l2"
  | "r2"
  | "dpad_up"
  | "dpad_down"
  | "dpad_left"
  | "dpad_right"
  | "options"
  | "create";

const BUTTON_BITS: Record<TouchButton, number> = {
  cross: BTN.CROSS,
  circle: BTN.CIRCLE,
  square: BTN.SQUARE,
  triangle: BTN.TRIANGLE,
  l1: BTN.L1,
  r1: BTN.R1,
  l2: BTN.L2,
  r2: BTN.R2,
  dpad_up: BTN.DPAD_UP,
  dpad_down: BTN.DPAD_DOWN,
  dpad_left: BTN.DPAD_LEFT,
  dpad_right: BTN.DPAD_RIGHT,
  options: BTN.OPTIONS,
  create: BTN.CREATE,
};

const TRIGGER_BUTTONS: Record<string, "l2" | "r2"> = {
  l2: "l2",
  r2: "r2",
};

type StickSide = "left" | "right";

export type StickVisual = {
  active: boolean;
  originX: number;
  originY: number;
  dx: number;
  dy: number;
  radius: number;
};

/** Snapshot the overlay renders on each frame. */
export type TouchVisual = {
  left: StickVisual;
  right: StickVisual;
  buttons: Record<TouchButton, boolean>;
};

type Pointer = {
  side: "stick" | "button";
  stick?: StickSide;
  button?: TouchButton;
  originX: number;
  originY: number;
};

/**
 * Creates a DualSense-shaped PadState from touch-screen state.
 * Pure — unit-testable without any DOM.
 */
export function sampleTouch(
  seq: number,
  left: { active: boolean; dx: number; dy: number },
  right: { active: boolean; dx: number; dy: number },
  pressed: ReadonlySet<TouchButton>
): PadState {
  const axis = (s: { active: boolean; dx: number; dy: number }) => {
    if (!s.active) return { x: 128, y: 128 };
    const mag = Math.hypot(s.dx, s.dy);
    if (mag === 0) return { x: 128, y: 128 };
    const clamp = Math.min(mag, TOUCH_RADIUS) / TOUCH_RADIUS;
    const nx = (s.dx / mag) * clamp;
    const ny = (s.dy / mag) * clamp;
    // Same -1..1 → 0..255 (center 128) mapping as a real browser Gamepad.
    return {
      x: Math.round((nx + 1) * 127.5),
      y: Math.round((ny + 1) * 127.5),
    };
  };

  const l = axis(left);
  const r = axis(right);

  let buttons = 0;
  let l2 = 0;
  let r2 = 0;
  for (const b of pressed) {
    buttons |= BUTTON_BITS[b];
    const trig = TRIGGER_BUTTONS[b];
    if (trig === "l2") l2 = TOUCH_MAX;
    if (trig === "r2") r2 = TOUCH_MAX;
  }

  return { seq, buttons, lx: l.x, ly: l.y, rx: r.x, ry: r.y, l2, r2 };
}

/**
 * Touch → PadState source. Attach to an element (the overlay), feed pointer
 * events to it, and call `sample()` on the pad loop like a real controller.
 */
export class TouchGamepadInput {
  private el: HTMLElement | null = null;
  private pointers = new Map<number, Pointer>();
  private leftStick: StickVisual = {
    active: false,
    originX: 0,
    originY: 0,
    dx: 0,
    dy: 0,
    radius: TOUCH_RADIUS,
  };
  private rightStick: StickVisual = {
    active: false,
    originX: 0,
    originY: 0,
    dx: 0,
    dy: 0,
    radius: TOUCH_RADIUS,
  };
  private pressed = new Set<TouchButton>();
  /** Surface rect, cached each pointerdown so moves resolve to local px. */
  private rect: DOMRect | null = null;

  attach(el: HTMLElement) {
    if (this.el === el) return;
    this.detach();
    this.el = el;
    el.addEventListener("pointerdown", this.onPointerDown);
    el.addEventListener("pointermove", this.onPointerMove);
    el.addEventListener("pointerup", this.onPointerUp);
    el.addEventListener("pointercancel", this.onPointerCancel);
    el.addEventListener("contextmenu", this.onContextMenu);
  }

  detach() {
    const el = this.el;
    if (!el) return;
    el.removeEventListener("pointerdown", this.onPointerDown);
    el.removeEventListener("pointermove", this.onPointerMove);
    el.removeEventListener("pointerup", this.onPointerUp);
    el.removeEventListener("pointercancel", this.onPointerCancel);
    el.removeEventListener("contextmenu", this.onContextMenu);
    this.el = null;
    this.clear();
  }

  /** Re-read the surface geometry (e.g. after a fullscreen resize). */
  refresh() {
    if (this.el) this.rect = this.el.getBoundingClientRect();
  }

  /** Sample current state into a PadState (call at the pad poll rate). */
  sample(seq: number): PadState {
    return sampleTouch(
      seq,
      { active: this.leftStick.active, dx: this.leftStick.dx, dy: this.leftStick.dy },
      { active: this.rightStick.active, dx: this.rightStick.dx, dy: this.rightStick.dy },
      this.pressed
    );
  }

  /** True while any stick is deflected or any button is held. */
  hasInput(): boolean {
    if (this.pressed.size > 0) return true;
    const l = Math.hypot(this.leftStick.dx, this.leftStick.dy);
    const r = Math.hypot(this.rightStick.dx, this.rightStick.dy);
    return l > 1 || r > 1;
  }

  /** Snapshot for the overlay render loop. */
  visual(): TouchVisual {
    return {
      left: { ...this.leftStick },
      right: { ...this.rightStick },
      buttons: {
        cross: this.pressed.has("cross"),
        circle: this.pressed.has("circle"),
        square: this.pressed.has("square"),
        triangle: this.pressed.has("triangle"),
        l1: this.pressed.has("l1"),
        r1: this.pressed.has("r1"),
        l2: this.pressed.has("l2"),
        r2: this.pressed.has("r2"),
        dpad_up: this.pressed.has("dpad_up"),
        dpad_down: this.pressed.has("dpad_down"),
        dpad_left: this.pressed.has("dpad_left"),
        dpad_right: this.pressed.has("dpad_right"),
        options: this.pressed.has("options"),
        create: this.pressed.has("create"),
      },
    };
  }

  private onContextMenu = (e: Event) => e.preventDefault();

  private onPointerDown = (e: PointerEvent) => {
    if (this.pointers.has(e.pointerId)) return;
    e.preventDefault();
    this.refresh();
    if (!this.rect) return;
    const x = e.clientX - this.rect.left;
    const y = e.clientY - this.rect.top;

    const btn = this.buttonAt(x, y);
    if (btn) {
      this.pressed.add(btn);
      this.pointers.set(e.pointerId, {
        side: "button",
        button: btn,
        originX: x,
        originY: y,
      });
      return;
    }

    const half = this.el!.clientWidth / 2;
    const side: StickSide = x < half ? "left" : "right";
    const stick = side === "left" ? this.leftStick : this.rightStick;
    stick.active = true;
    stick.originX = x;
    stick.originY = y;
    stick.dx = 0;
    stick.dy = 0;
    this.pointers.set(e.pointerId, { side: "stick", stick: side, originX: x, originY: y });
    this.capture(e);
  };

  private onPointerMove = (e: PointerEvent) => {
    const p = this.pointers.get(e.pointerId);
    if (!p || p.side !== "stick" || !p.stick || !this.rect) return;
    const x = e.clientX - this.rect.left;
    const y = e.clientY - this.rect.top;
    const stick = p.stick === "left" ? this.leftStick : this.rightStick;
    let dx = x - p.originX;
    let dy = y - p.originY;
    const mag = Math.hypot(dx, dy);
    if (mag > TOUCH_RADIUS) {
      const scale = TOUCH_RADIUS / mag;
      dx *= scale;
      dy *= scale;
    }
    stick.dx = dx;
    stick.dy = dy;
    this.capture(e);
  };

  private onPointerUp = (e: PointerEvent) => {
    this.release(e.pointerId);
  };

  private onPointerCancel = (e: PointerEvent) => {
    this.release(e.pointerId);
  };

  private release(pointerId: number) {
    const p = this.pointers.get(pointerId);
    if (!p) return;
    this.pointers.delete(pointerId);
    if (p.side === "button" && p.button) {
      this.pressed.delete(p.button);
    } else if (p.side === "stick" && p.stick) {
      const stick = p.stick === "left" ? this.leftStick : this.rightStick;
      stick.active = false;
      stick.dx = 0;
      stick.dy = 0;
    }
  }

  private clear() {
    this.pointers.clear();
    this.pressed.clear();
    this.leftStick.active = false;
    this.leftStick.dx = 0;
    this.leftStick.dy = 0;
    this.rightStick.active = false;
    this.rightStick.dx = 0;
    this.rightStick.dy = 0;
  }

  private capture(e: PointerEvent) {
    try {
      this.el?.setPointerCapture(e.pointerId);
    } catch {
      /* pointer already released */
    }
  }

  /** True when `el` (or a child) is a labelled touch button. */
  private buttonAt(x: number, y: number): TouchButton | null {
    if (!this.el) return null;
    const hit = document.elementFromPoint?.(x + (this.rect?.left ?? 0), y + (this.rect?.top ?? 0));
    const node = hit?.closest?.("[data-touch-btn]");
    const name = node?.getAttribute("data-touch-btn");
    if (name && name in BUTTON_BITS) return name as TouchButton;
    return null;
  }
}