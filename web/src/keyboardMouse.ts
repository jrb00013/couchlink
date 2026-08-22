/**
 * Keyboard + mouse → DualSense PadState emulation.
 *
 * Bindings live in `kbmBinds` (localStorage). Mouse look is always
 * pointer-lock movement → right stick; it is not a remappable key.
 */

import { BTN, type PadState } from "./clpd";
import {
  DEFAULT_KBM_BINDS,
  type KbmAction,
  type KbmBinds,
  type KbmCode,
  cloneBinds,
} from "./kbmBinds";

export type KbmOptions = {
  /** Sensitivity scalar for mouse → right stick. Default 0.5. */
  mouseSensitivity?: number;
  /** Element to request pointer lock on (typically the canvas). */
  lockTarget?: HTMLElement | null;
  binds?: KbmBinds;
};

export class KeyboardMouseInput {
  private keys = new Set<string>();
  private mouseButtons = 0;
  private mouseDx = 0;
  private mouseDy = 0;
  private sensitivity: number;
  private lockTarget: HTMLElement | null;
  private active = false;
  private binds: KbmBinds;

  constructor(opts: KbmOptions = {}) {
    this.sensitivity = opts.mouseSensitivity ?? 0.5;
    this.lockTarget = opts.lockTarget ?? null;
    this.binds = cloneBinds(opts.binds ?? DEFAULT_KBM_BINDS);
  }

  setBinds(binds: KbmBinds) {
    this.binds = cloneBinds(binds);
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
    const held = (action: KbmAction) => this.actionHeld(action);

    const moveLeft = held("moveLeft");
    const moveRight = held("moveRight");
    const moveUp = held("moveUp");
    const moveDown = held("moveDown");
    const lx = moveLeft ? 0 : moveRight ? 255 : 128;
    const ly = moveUp ? 0 : moveDown ? 255 : 128;

    const clamp = (v: number) => Math.max(0, Math.min(255, Math.round(v)));
    const scale = this.sensitivity * 128;
    const rx = clamp(128 + this.mouseDx * scale);
    const ry = clamp(128 + this.mouseDy * scale);
    this.mouseDx = 0;
    this.mouseDy = 0;

    let buttons = 0;
    if (held("cross")) buttons |= BTN.CROSS;
    if (held("circle")) buttons |= BTN.CIRCLE;
    if (held("square")) buttons |= BTN.SQUARE;
    if (held("triangle")) buttons |= BTN.TRIANGLE;
    if (held("l1")) buttons |= BTN.L1;
    if (held("r1")) buttons |= BTN.R1;
    if (held("l3")) buttons |= BTN.L3;
    if (held("r3")) buttons |= BTN.R3;
    if (held("options")) buttons |= BTN.OPTIONS;
    if (held("create")) buttons |= BTN.CREATE;
    if (held("dpadUp")) buttons |= BTN.DPAD_UP;
    if (held("dpadDown")) buttons |= BTN.DPAD_DOWN;
    if (held("dpadLeft")) buttons |= BTN.DPAD_LEFT;
    if (held("dpadRight")) buttons |= BTN.DPAD_RIGHT;

    const r2Held = held("r2");
    const l2Held = held("l2");
    if (r2Held) buttons |= BTN.R2;
    if (l2Held) buttons |= BTN.L2;

    return {
      seq,
      buttons,
      lx,
      ly,
      rx,
      ry,
      l2: l2Held ? 255 : 0,
      r2: r2Held ? 255 : 0,
    };
  }

  /** True while any key or mouse button is held, or unprocessed mouse motion exists. */
  hasInput(): boolean {
    return (
      this.keys.size > 0 ||
      this.mouseButtons !== 0 ||
      Math.abs(this.mouseDx) > 0.001 ||
      Math.abs(this.mouseDy) > 0.001
    );
  }

  isPointerLocked(): boolean {
    return !!document.pointerLockElement;
  }

  private actionHeld(action: KbmAction): boolean {
    return (this.binds[action] ?? []).some((code) => this.codeHeld(code));
  }

  private codeHeld(code: KbmCode): boolean {
    if (code.startsWith("Mouse")) {
      const btn = Number(code.slice(5));
      if (!Number.isFinite(btn) || btn < 0) return false;
      return !!(this.mouseButtons & (1 << btn));
    }
    return this.keys.has(code);
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
    this.mouseButtons |= 1 << e.button;
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
