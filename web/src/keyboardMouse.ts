/**
 * Keyboard + mouse → DualSense PadState emulation.
 *
 * The default layout (fully remappable — see {@link KeyMap}):
 *   WASD            → Left stick
 *   Mouse move (pointer-locked) → Right stick
 *   Arrow keys      → Right stick
 *   Space  → Cross (jump)
 *   E      → Triangle (interact)
 *   Q      → Square (reload / alt)
 *   F      → Circle (cancel / dodge)
 *   R      → R1
 *   Shift  → L1
 *   C      → L3 (crouch)   [also middle click]
 *   V      → R3 (melee)
 *   Tab    → Options
 *   G      → Create
 *   IJKL   → D-Pad
 *   Left click  → R2 (shoot / confirm)
 *   Right click → L2 (aim / alternate)
 *
 * The keymap maps a control name to a browser `KeyboardEvent.code`. Mouse
 * buttons stay glued to R2/L2/L3 on top of whatever key is mapped there, so a
 * remap never breaks the mouse-driven triggers.
 */

import { BTN, type PadState } from "./clpd";

export const KBM_CONTROLS = [
  "lstick_up",
  "lstick_down",
  "lstick_left",
  "lstick_right",
  "rstick_up",
  "rstick_down",
  "rstick_left",
  "rstick_right",
  "cross",
  "circle",
  "square",
  "triangle",
  "l1",
  "r1",
  "l2",
  "r2",
  "l3",
  "r3",
  "dpad_up",
  "dpad_down",
  "dpad_left",
  "dpad_right",
  "options",
  "create",
] as const;

export type KbmControl = (typeof KBM_CONTROLS)[number];

/** Control → `KeyboardEvent.code`. A missing/empty value means "mouse only" for
 * the trigger controls (L2/R2/L3) and unbound for everything else. */
export type KeyMap = Partial<Record<KbmControl, string>>;

export const DEFAULT_KEYMAP: KeyMap = {
  lstick_up: "KeyW",
  lstick_down: "KeyS",
  lstick_left: "KeyA",
  lstick_right: "KeyD",
  rstick_up: "ArrowUp",
  rstick_down: "ArrowDown",
  rstick_left: "ArrowLeft",
  rstick_right: "ArrowRight",
  cross: "Space",
  circle: "KeyF",
  square: "KeyQ",
  triangle: "KeyE",
  l1: "ShiftLeft",
  r1: "KeyR",
  r3: "KeyV",
  options: "Tab",
  create: "KeyG",
  dpad_up: "KeyI",
  dpad_down: "KeyK",
  dpad_left: "KeyJ",
  dpad_right: "KeyL",
  // l2/r2 default to mouse only; l3 defaults to middle click (+ C).
  l3: "KeyC",
};

export type KbmOptions = {
  /** Sensitivity scalar for mouse → right stick. Default 0.5. */
  mouseSensitivity?: number;
  /** Element to request pointer lock on (typically the canvas). */
  lockTarget?: HTMLElement | null;
  /** Control → key bindings. Defaults to {@link DEFAULT_KEYMAP}. */
  keymap?: KeyMap;
};

/** Pretty label for a `KeyboardEvent.code`, for the viz + editor. */
export function keyLabel(code: string | undefined): string {
  if (!code) return "";
  const name: Record<string, string> = {
    Space: "Space",
    ArrowUp: "↑",
    ArrowDown: "↓",
    ArrowLeft: "←",
    ArrowRight: "→",
    ShiftLeft: "Shift",
    ShiftRight: "Shift",
    ControlLeft: "Ctrl",
    ControlRight: "Ctrl",
    AltLeft: "Alt",
    AltRight: "Alt",
    Tab: "Tab",
    Enter: "Enter",
    Escape: "Esc",
    Backspace: "⌫",
  };
  if (name[code]) return name[code];
  if (code.startsWith("Key")) return code.slice(3);
  if (code.startsWith("Digit")) return code.slice(5);
  return code;
}

/** Human label for a control, for the keybind editor + viz. */
export function controlLabel(c: KbmControl): string {
  const names: Record<KbmControl, string> = {
    lstick_up: "Left stick ↑",
    lstick_down: "Left stick ↓",
    lstick_left: "Left stick ←",
    lstick_right: "Left stick →",
    rstick_up: "Right stick ↑",
    rstick_down: "Right stick ↓",
    rstick_left: "Right stick ←",
    rstick_right: "Right stick →",
    cross: "✕ Cross",
    circle: "○ Circle",
    square: "□ Square",
    triangle: "△ Triangle",
    l1: "L1",
    r1: "R1",
    l2: "L2 (aim)",
    r2: "R2 (shoot)",
    l3: "L3",
    r3: "R3",
    dpad_up: "D-pad ↑",
    dpad_down: "D-pad ↓",
    dpad_left: "D-pad ←",
    dpad_right: "D-pad →",
    options: "Options",
    create: "Create",
  };
  return names[c];
}

export class KeyboardMouseInput {
  private keys = new Set<string>();
  private mouseButtons = 0;
  private mouseDx = 0;
  private mouseDy = 0;
  private sensitivity: number;
  private lockTarget: HTMLElement | null;
  private active = false;
  private map: KeyMap;

  constructor(opts: KbmOptions = {}) {
    this.sensitivity = opts.mouseSensitivity ?? 0.5;
    this.lockTarget = opts.lockTarget ?? null;
    this.map = { ...DEFAULT_KEYMAP, ...(opts.keymap ?? {}) };
  }

  /** JSON serialisation for the signaling `key_map` message. */
  keymapJson(): string {
    return JSON.stringify(this.map);
  }

  setLockTarget(el: HTMLElement | null) {
    if (this.active && this.lockTarget) {
      this.lockTarget.removeEventListener("click", this.onLockTargetClick);
    }
    this.lockTarget = el;
    if (this.active && el) {
      el.addEventListener("click", this.onLockTargetClick);
    }
  }

  start() {
    if (this.active) return;
    this.active = true;
    window.addEventListener("keydown", this.onKeyDown);
    window.addEventListener("keyup", this.onKeyUp);
    window.addEventListener("mousedown", this.onMouseDown);
    window.addEventListener("mouseup", this.onMouseUp);
    window.addEventListener("mousemove", this.onMouseMove);
    window.addEventListener("contextmenu", this.onContextMenu);
    window.addEventListener("blur", this.onBlur);
    document.addEventListener("visibilitychange", this.onVisibilityChange);
    if (this.lockTarget) {
      this.lockTarget.addEventListener("click", this.onLockTargetClick);
    }
  }

  stop() {
    if (!this.active) return;
    this.active = false;
    window.removeEventListener("keydown", this.onKeyDown);
    window.removeEventListener("keyup", this.onKeyUp);
    window.removeEventListener("mousedown", this.onMouseDown);
    window.removeEventListener("mouseup", this.onMouseUp);
    window.removeEventListener("mousemove", this.onMouseMove);
    window.removeEventListener("contextmenu", this.onContextMenu);
    window.removeEventListener("blur", this.onBlur);
    document.removeEventListener("visibilitychange", this.onVisibilityChange);
    if (this.lockTarget) {
      this.lockTarget.removeEventListener("click", this.onLockTargetClick);
    }
    if (document.pointerLockElement) document.exitPointerLock();
    this.keys.clear();
    this.mouseButtons = 0;
    this.mouseDx = 0;
    this.mouseDy = 0;
  }

  /** Sample current state into a PadState, consuming accumulated mouse delta. */
  sample(seq: number): PadState {
    const k = this.keys;
    const held = (code?: string) => !!code && k.has(code);
    const m = this.map;

    // Left stick — keymapped WASD (digital, full deflection)
    const moveLeft  = held(m.lstick_left);
    const moveRight = held(m.lstick_right);
    const moveUp    = held(m.lstick_up);
    const moveDown  = held(m.lstick_down);
    const lx = moveLeft ? 0 : moveRight ? 255 : 128;
    const ly = moveUp   ? 0 : moveDown  ? 255 : 128;

    // Right stick — accumulated mouse delta, clamped to 0–255, plus keymapped
    // arrows for full deflection when keyboard is preferred.
    const clamp = (v: number) => Math.max(0, Math.min(255, Math.round(v)));
    const scale = this.sensitivity * 128;
    let rx = clamp(128 + this.mouseDx * scale);
    let ry = clamp(128 + this.mouseDy * scale);
    if (held(m.rstick_left)) rx = 0;
    if (held(m.rstick_right)) rx = 255;
    if (held(m.rstick_up)) ry = 0;
    if (held(m.rstick_down)) ry = 255;
    this.mouseDx = 0;
    this.mouseDy = 0;

    // Buttons
    let buttons = 0;
    if (held(m.cross))                          buttons |= BTN.CROSS;
    if (held(m.circle))                         buttons |= BTN.CIRCLE;
    if (held(m.square))                         buttons |= BTN.SQUARE;
    if (held(m.triangle))                       buttons |= BTN.TRIANGLE;
    if (held(m.l1))                             buttons |= BTN.L1;
    if (held(m.r1))                             buttons |= BTN.R1;
    if (held(m.options))                        buttons |= BTN.OPTIONS;
    if (held(m.create))                         buttons |= BTN.CREATE;
    if (held(m.dpad_up))                        buttons |= BTN.DPAD_UP;
    if (held(m.dpad_down))                      buttons |= BTN.DPAD_DOWN;
    if (held(m.dpad_left))                      buttons |= BTN.DPAD_LEFT;
    if (held(m.dpad_right))                     buttons |= BTN.DPAD_RIGHT;

    // Mouse buttons → triggers, OR'ed with any keymap key on the same control
    const leftBtn   = !!(this.mouseButtons & 1);
    const middleBtn = !!(this.mouseButtons & 2);
    const rightBtn  = !!(this.mouseButtons & 4);
    if (held(m.l2) || rightBtn)  buttons |= BTN.L2;
    if (held(m.r2) || leftBtn)   buttons |= BTN.R2;
    if (held(m.l3) || middleBtn) buttons |= BTN.L3;
    if (held(m.r3))              buttons |= BTN.R3;
    const r2 = leftBtn  ? 255 : held(m.r2) ? 255 : 0;
    const l2 = rightBtn ? 255 : held(m.l2) ? 255 : 0;

    return { seq, buttons, lx, ly, rx, ry, l2, r2 };
  }

  /** True while any key or mouse button is held, or unprocessed mouse motion exists. */
  hasInput(): boolean {
    return this.keys.size > 0 || this.mouseButtons !== 0 ||
           Math.abs(this.mouseDx) > 0.001 || Math.abs(this.mouseDy) > 0.001;
  }

  isPointerLocked(): boolean {
    return !!document.pointerLockElement;
  }

  private onKeyDown = (e: KeyboardEvent) => {
    if ((e.target as HTMLElement)?.tagName === "INPUT") return;
    if (e.code === "Escape") {
      if (document.pointerLockElement) document.exitPointerLock();
      return;
    }
    if (e.code === "Tab" || e.code === "Space") e.preventDefault();
    this.keys.add(e.code);
  };

  private onKeyUp = (e: KeyboardEvent) => {
    this.keys.delete(e.code);
  };

  private onMouseDown = (e: MouseEvent) => {
    this.mouseButtons |= (1 << e.button);
  };

  private onMouseUp = (e: MouseEvent) => {
    this.mouseButtons &= ~(1 << e.button);
  };

  private onMouseMove = (e: MouseEvent) => {
    if (!document.pointerLockElement) return;
    this.mouseDx += e.movementX / 100;
    this.mouseDy += e.movementY / 100;
  };

  private onContextMenu = (e: Event) => {
    if (document.pointerLockElement) e.preventDefault();
  };

  /** Window/tab losing focus means no keyup will ever arrive for held keys — release them all. */
  private onBlur = () => {
    this.keys.clear();
    this.mouseButtons = 0;
  };

  private onVisibilityChange = () => {
    if (document.hidden) this.onBlur();
  };

  private onLockTargetClick = () => {
    if (!document.pointerLockElement && this.lockTarget) {
      void this.lockTarget.requestPointerLock();
    }
  };
}
