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

export type KbmSnapshot = {
  keys: string[];
  mouseButtons: number;
  lookX: number;
  lookY: number;
  locked: boolean;
};

export type KbmOptions = {
  /** Sensitivity scalar for mouse → right stick. Default 1.15. */
  mouseSensitivity?: number;
  /**
   * Whether mouse movement drives the right stick at all. Default true —
   * pointer-lock still gates it so accidental cursor motion never aims.
   * Toggle off in Keybinds if a title fights mouse-driven look.
   */
  mouseLookEnabled?: boolean;
  /** Element to request pointer lock on (typically the canvas). */
  lockTarget?: HTMLElement | null;
  binds?: KbmBinds;
};

/** Fired on key/button change — send pad immediately, don't wait for poll. */
export type KbmActivityHandler = () => void;
/** Fired on mouse-look motion — may be coalesced; buttons stay on `onActivity`. */
export type KbmLookActivityHandler = () => void;

/**
 * Hold stick deflection this long after the last mouse sample so a ~60Hz game
 * read still sees the look. Snap to center after — no mushy exponential tail.
 */
export const LOOK_HOLD_MS = 16;

export class KeyboardMouseInput {
  private keys = new Set<string>();
  private mouseButtons = 0;
  /** Motion not yet folded into the look latch. */
  private pendingDx = 0;
  private pendingDy = 0;
  /** Stick deflection; time-decayed, snapped after {@link LOOK_HOLD_MS} silence. */
  private latchDx = 0;
  private latchDy = 0;
  /** performance.now() of last pointer-lock mouse sample that added look. */
  private lastMouseMoveAt = 0;
  private lastLookSampleAt = 0;
  /** Unconsumed look for the mini viz — sample() zeros mouseDx/Dy. */
  private lookX = 0;
  private lookY = 0;
  private sensitivity: number;
  private mouseLookEnabled: boolean;
  private lockTarget: HTMLElement | null;
  private active = false;
  private binds: KbmBinds;
  /** Wired by CouchlinkPlayer for sub-poll input (beats 2ms quantisation). */
  onActivity: KbmActivityHandler | null = null;
  /** Look-only path — player may coalesce these; never delay buttons via this. */
  onLookActivity: KbmLookActivityHandler | null = null;

  constructor(opts: KbmOptions = {}) {
    this.sensitivity = opts.mouseSensitivity ?? 1.15;
    this.mouseLookEnabled = opts.mouseLookEnabled ?? true;
    this.lockTarget = opts.lockTarget ?? null;
    this.binds = cloneBinds(opts.binds ?? DEFAULT_KBM_BINDS);
  }

  setBinds(binds: KbmBinds) {
    this.binds = cloneBinds(binds);
  }

  /** Live-tune mouse-look sensitivity without recreating the instance (that would drop pointer lock). */
  setSensitivity(sensitivity: number) {
    this.sensitivity = sensitivity;
  }

  getSensitivity(): number {
    return this.sensitivity;
  }

  /** Live-toggle mouse-look without recreating the instance (that would drop pointer lock). */
  setMouseLookEnabled(enabled: boolean) {
    this.mouseLookEnabled = enabled;
    if (!enabled) {
      // Snap the stick back to neutral immediately, don't wait for hold —
      // otherwise disabling mid-look leaves the last look direction "stuck"
      // held on the wire until LOOK_HOLD_MS elapses.
      this.pendingDx = 0;
      this.pendingDy = 0;
      this.latchDx = 0;
      this.latchDy = 0;
      this.lastMouseMoveAt = 0;
      this.lastLookSampleAt = 0;
      this.lookX = 0;
      this.lookY = 0;
    }
  }

  getMouseLookEnabled(): boolean {
    return this.mouseLookEnabled;
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
    this.pendingDx = 0;
    this.pendingDy = 0;
    this.latchDx = 0;
    this.latchDy = 0;
    this.lastMouseMoveAt = 0;
    this.lastLookSampleAt = 0;
    this.lookX = 0;
    this.lookY = 0;
  }

  /** Live keys/buttons for the keyboard+mouse drawing. Does not consume look. */
  snapshot(): KbmSnapshot {
    this.lookX *= 0.86;
    this.lookY *= 0.86;
    if (Math.abs(this.lookX) < 0.02) this.lookX = 0;
    if (Math.abs(this.lookY) < 0.02) this.lookY = 0;
    return {
      keys: [...this.keys],
      mouseButtons: this.mouseButtons,
      lookX: this.lookX,
      lookY: this.lookY,
      locked: this.isPointerLocked(),
    };
  }

  /**
   * Map accumulated mouse delta into right-stick axes.
   *
   * Pad polls ~every 2ms; BO2 reads closer to ~60Hz. Time-based decay
   * (τ = LOOK_HOLD_MS) keeps deflection visible across game polls regardless
   * of poll Hz, then hard-snaps after LOOK_HOLD_MS of mouse silence so look
   * stops crisply instead of a long mushy tail.
   */
  sampleLookAxes(nowMs: number = performance.now()): { rx: number; ry: number } {
    const clamp = (v: number) => Math.max(0, Math.min(255, Math.round(v)));
    this.latchDx += this.pendingDx;
    this.latchDy += this.pendingDy;
    this.pendingDx = 0;
    this.pendingDy = 0;

    const dt =
      this.lastLookSampleAt > 0
        ? Math.max(0, Math.min(50, nowMs - this.lastLookSampleAt))
        : 0;
    this.lastLookSampleAt = nowMs;
    if (dt > 0) {
      const keep = Math.exp(-dt / LOOK_HOLD_MS);
      this.latchDx *= keep;
      this.latchDy *= keep;
    }

    if (
      this.lastMouseMoveAt <= 0 ||
      nowMs - this.lastMouseMoveAt > LOOK_HOLD_MS ||
      (Math.abs(this.latchDx) < 0.02 && Math.abs(this.latchDy) < 0.02)
    ) {
      this.latchDx = 0;
      this.latchDy = 0;
    }

    const scale = this.sensitivity * 128;
    return {
      rx: clamp(128 + this.latchDx * scale),
      ry: clamp(128 + this.latchDy * scale),
    };
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

    const { rx, ry } = this.sampleLookAxes();

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

  /**
   * Merge held keys, mouse buttons and mouse-look into a physical pad's frame.
   *
   * A connected-but-idle gamepad (paired DualSense, Steam Input, a headset
   * that exposes HID buttons) used to shadow the keyboard completely, so the
   * player's keys and mouse did nothing. The pad still wins where it is
   * actually being used: keyboard axes only replace a stick when a movement
   * key is down, and mouse-look only replaces the right stick while looking.
   */
  overlay(state: PadState): PadState {
    const k = this.sample(state.seq);
    const moving = k.lx !== 128 || k.ly !== 128;
    const looking = k.rx !== 128 || k.ry !== 128;
    return {
      ...state,
      buttons: state.buttons | k.buttons,
      lx: moving ? k.lx : state.lx,
      ly: moving ? k.ly : state.ly,
      rx: looking ? k.rx : state.rx,
      ry: looking ? k.ry : state.ry,
      l2: Math.max(state.l2, k.l2),
      r2: Math.max(state.r2, k.r2),
    };
  }

  /** True while any key or mouse button is held, or unprocessed mouse motion exists. */
  hasInput(): boolean {
    return (
      this.keys.size > 0 ||
      this.mouseButtons !== 0 ||
      Math.abs(this.pendingDx) > 0.001 ||
      Math.abs(this.pendingDy) > 0.001 ||
      Math.abs(this.latchDx) > 0.001 ||
      Math.abs(this.latchDy) > 0.001
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

  private bumpActivity() {
    this.onActivity?.();
  }

  private onKeyDown = (e: KeyboardEvent) => {
    if ((e.target as HTMLElement)?.tagName === "INPUT") return;
    if (e.code === "Escape") {
      if (document.pointerLockElement) document.exitPointerLock();
      return;
    }
    if (e.code === "Tab" || e.code === "Space") e.preventDefault();
    this.keys.add(e.code);
    this.bumpActivity();
  };

  private onKeyUp = (e: KeyboardEvent) => {
    this.keys.delete(e.code);
    this.bumpActivity();
  };

  private onMouseDown = (e: MouseEvent) => {
    this.mouseButtons |= 1 << e.button;
    this.bumpActivity();
  };

  private onMouseUp = (e: MouseEvent) => {
    this.mouseButtons &= ~(1 << e.button);
    this.bumpActivity();
  };

  private onMouseMove = (e: MouseEvent) => {
    if (!document.pointerLockElement || !this.mouseLookEnabled) return;
    // Higher gain than /100: pixel motion must reach meaningful stick
    // deflection so FPS titles (BO2) feel 1:1 with the mouse.
    this.pendingDx += e.movementX / 35;
    this.pendingDy += e.movementY / 35;
    this.lastMouseMoveAt = performance.now();
    this.lookX = Math.max(-1, Math.min(1, this.lookX + e.movementX / 40));
    this.lookY = Math.max(-1, Math.min(1, this.lookY + e.movementY / 40));
    this.onLookActivity?.();
  };

  private onContextMenu = (e: Event) => {
    if (document.pointerLockElement) e.preventDefault();
  };

  /** Window/tab losing focus means no keyup will ever arrive for held keys — release them all. */
  private onBlur = () => {
    this.keys.clear();
    this.mouseButtons = 0;
    this.bumpActivity();
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
