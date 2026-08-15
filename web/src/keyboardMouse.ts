/**
 * Keyboard + mouse → DualSense PadState emulation.
 *
 * Layout:
 *   WASD / Arrow keys  → Left stick
 *   Mouse move (pointer-locked) → Right stick
 *   Left click  → R2 (shoot / confirm)
 *   Right click → L2 (aim / alternate)
 *   Space  → Cross (jump)
 *   E      → Triangle (interact)
 *   Q      → Square (reload / alt)
 *   F      → Circle (cancel / dodge)
 *   R      → R1
 *   Shift  → L1
 *   C      → L3 (crouch)
 *   V      → R3 (melee)
 *   Tab    → Options
 *   G      → Create
 *   Middle click → L3 (sprint)
 *   Numpad/IJKL → D-Pad
 */

import { BTN, type PadState } from "./clpd";

export type KbmOptions = {
  /** Sensitivity scalar for mouse → right stick. Default 0.5. */
  mouseSensitivity?: number;
  /** Element to request pointer lock on (typically the canvas). */
  lockTarget?: HTMLElement | null;
};

export class KeyboardMouseInput {
  private keys = new Set<string>();
  private mouseButtons = 0;
  private mouseDx = 0;
  private mouseDy = 0;
  private sensitivity: number;
  private lockTarget: HTMLElement | null;
  private active = false;

  constructor(opts: KbmOptions = {}) {
    this.sensitivity = opts.mouseSensitivity ?? 0.5;
    this.lockTarget = opts.lockTarget ?? null;
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

    // Left stick — WASD / Arrow keys (digital, full deflection)
    const moveLeft  = k.has("KeyA") || k.has("ArrowLeft");
    const moveRight = k.has("KeyD") || k.has("ArrowRight");
    const moveUp    = k.has("KeyW") || k.has("ArrowUp");
    const moveDown  = k.has("KeyS") || k.has("ArrowDown");
    const lx = moveLeft ? 0 : moveRight ? 255 : 128;
    const ly = moveUp   ? 0 : moveDown  ? 255 : 128;

    // Right stick — accumulated mouse delta, clamped to 0–255
    const clamp = (v: number) => Math.max(0, Math.min(255, Math.round(v)));
    const scale = this.sensitivity * 128;
    const rx = clamp(128 + this.mouseDx * scale);
    const ry = clamp(128 + this.mouseDy * scale);
    this.mouseDx = 0;
    this.mouseDy = 0;

    // Buttons
    let buttons = 0;
    if (k.has("Space"))                         buttons |= BTN.CROSS;
    if (k.has("KeyF"))                          buttons |= BTN.CIRCLE;
    if (k.has("KeyQ"))                          buttons |= BTN.SQUARE;
    if (k.has("KeyE"))                          buttons |= BTN.TRIANGLE;
    if (k.has("ShiftLeft") || k.has("ShiftRight")) buttons |= BTN.L1;
    if (k.has("KeyR"))                          buttons |= BTN.R1;
    if (k.has("KeyC"))                          buttons |= BTN.L3;
    if (k.has("KeyV"))                          buttons |= BTN.R3;
    if (k.has("Tab"))                           buttons |= BTN.OPTIONS;
    if (k.has("KeyG"))                          buttons |= BTN.CREATE;
    if (k.has("Numpad8") || k.has("KeyI"))      buttons |= BTN.DPAD_UP;
    if (k.has("Numpad2") || k.has("KeyK"))      buttons |= BTN.DPAD_DOWN;
    if (k.has("Numpad4") || k.has("KeyJ"))      buttons |= BTN.DPAD_LEFT;
    if (k.has("Numpad6") || k.has("KeyL"))      buttons |= BTN.DPAD_RIGHT;

    // Mouse buttons → triggers
    const leftBtn   = !!(this.mouseButtons & 1);
    const middleBtn = !!(this.mouseButtons & 2);
    const rightBtn  = !!(this.mouseButtons & 4);
    if (leftBtn)   { buttons |= BTN.R2; }
    if (rightBtn)  { buttons |= BTN.L2; }
    if (middleBtn) { buttons |= BTN.L3; }
    const r2 = leftBtn  ? 255 : 0;
    const l2 = rightBtn ? 255 : 0;

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
