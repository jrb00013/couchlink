import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { KeyboardMouseInput } from "./keyboardMouse";
import { BTN } from "./clpd";
import { DEFAULT_KBM_BINDS, setBind } from "./kbmBinds";

/**
 * No jsdom dependency in this project — fake just enough DOM surface
 * (EventTarget + the couple of properties keyboardMouse.ts touches) with
 * plain Node globals instead of pulling in a new package for one test file.
 */
class FakeElement extends EventTarget {
  requestPointerLock() {}
}

function installFakeDom() {
  const fakeDocument = Object.assign(new FakeElement(), {
    pointerLockElement: null as unknown,
    hidden: false,
    exitPointerLock() {},
  });
  (globalThis as any).window = new FakeElement();
  (globalThis as any).document = fakeDocument;
  (globalThis as any).KeyboardEvent = class extends Event {
    code: string;
    constructor(type: string, init: { code: string }) {
      super(type);
      this.code = init.code;
    }
  };
  (globalThis as any).MouseEvent = class extends Event {
    button: number;
    movementX: number;
    movementY: number;
    constructor(type: string, init: { button?: number; movementX?: number; movementY?: number } = {}) {
      super(type);
      this.button = init.button ?? 0;
      this.movementX = init.movementX ?? 0;
      this.movementY = init.movementY ?? 0;
    }
  };
  return fakeDocument;
}

function keyEvent(type: string, code: string) {
  return new (globalThis as any).KeyboardEvent(type, { code });
}

describe("KeyboardMouseInput", () => {
  let kbm: KeyboardMouseInput;

  beforeEach(() => {
    installFakeDom();
    // Mouse look is opt-in (default off) — most of this file's mouse-look
    // assertions predate that and assume it's always active, so enable it
    // here; the "disabled by default" behavior gets its own explicit test.
    kbm = new KeyboardMouseInput({ mouseLookEnabled: true });
    kbm.start();
  });

  afterEach(() => {
    kbm.stop();
  });

  it("moves the left stick while WASD is held", () => {
    (globalThis as any).window.dispatchEvent(keyEvent("keydown", "KeyD"));
    const state = kbm.sample(1);
    expect(state.lx).toBe(255);
    expect(state.ly).toBe(128);
  });

  it("snapshot reports held keys without consuming them", () => {
    (globalThis as any).window.dispatchEvent(keyEvent("keydown", "KeyW"));
    const snap = kbm.snapshot();
    expect(snap.keys).toContain("KeyW");
    const state = kbm.sample(1);
    expect(state.ly).toBe(0);
  });

  it("fires onActivity on keydown for immediate pad send", () => {
    let hits = 0;
    kbm.onActivity = () => {
      hits += 1;
    };
    (globalThis as any).window.dispatchEvent(keyEvent("keydown", "KeyE"));
    expect(hits).toBe(1);
  });

  it("fires onActivity on keyup", () => {
    let hits = 0;
    kbm.onActivity = () => {
      hits += 1;
    };
    (globalThis as any).window.dispatchEvent(keyEvent("keydown", "KeyE"));
    (globalThis as any).window.dispatchEvent(keyEvent("keyup", "KeyE"));
    expect(hits).toBe(2);
  });

  it("releases keys on keyup", () => {
    (globalThis as any).window.dispatchEvent(keyEvent("keydown", "KeyD"));
    (globalThis as any).window.dispatchEvent(keyEvent("keyup", "KeyD"));
    const state = kbm.sample(1);
    expect(state.lx).toBe(128);
  });

  it("clears all held keys when the window loses focus, so alt-tab doesn't leave input stuck", () => {
    (globalThis as any).window.dispatchEvent(keyEvent("keydown", "KeyW"));
    (globalThis as any).window.dispatchEvent(keyEvent("keydown", "Space"));
    expect(kbm.hasInput()).toBe(true);

    (globalThis as any).window.dispatchEvent(new Event("blur"));

    expect(kbm.hasInput()).toBe(false);
    const state = kbm.sample(1);
    expect(state.ly).toBe(128);
    expect(state.buttons).toBe(0);
  });

  it("uses remapped binds so a custom jump key fires Cross", () => {
    kbm.setBinds(setBind(DEFAULT_KBM_BINDS, "cross", "KeyZ"));
    (globalThis as any).window.dispatchEvent(keyEvent("keydown", "KeyZ"));
    const state = kbm.sample(1);
    expect(state.buttons & BTN.CROSS).toBe(BTN.CROSS);
    (globalThis as any).window.dispatchEvent(keyEvent("keyup", "KeyZ"));
    (globalThis as any).window.dispatchEvent(keyEvent("keydown", "Space"));
    const after = kbm.sample(2);
    expect(after.buttons & BTN.CROSS).toBe(0);
  });

  it("mouse movement drives the right stick while pointer-locked, centered at 128", () => {
    (globalThis as any).document.pointerLockElement = {};
    const move = (dx: number, dy: number) =>
      (globalThis as any).window.dispatchEvent(
        new (globalThis as any).MouseEvent("mousemove", { movementX: dx, movementY: dy })
      );
    move(50, -50);
    const state = kbm.sample(1);
    expect(state.rx).toBeGreaterThan(128);
    expect(state.ry).toBeLessThan(128);
  });

  it("mouse look is opt-in — disabled by default, ignores mouse movement even while pointer-locked", () => {
    const disabledKbm = new KeyboardMouseInput();
    disabledKbm.start();
    (globalThis as any).document.pointerLockElement = {};
    (globalThis as any).window.dispatchEvent(
      new (globalThis as any).MouseEvent("mousemove", { movementX: 100, movementY: 100 })
    );
    const state = disabledKbm.sample(1);
    expect(state.rx).toBe(128);
    expect(state.ry).toBe(128);
    disabledKbm.stop();
  });

  it("setMouseLookEnabled toggles live and snaps the stick back to neutral when disabled mid-look", () => {
    const toggling = new KeyboardMouseInput({ mouseLookEnabled: true });
    toggling.start();
    (globalThis as any).document.pointerLockElement = {};
    (globalThis as any).window.dispatchEvent(
      new (globalThis as any).MouseEvent("mousemove", { movementX: 80, movementY: 0 })
    );
    expect(toggling.sample(1).rx).toBeGreaterThan(128);

    (globalThis as any).window.dispatchEvent(
      new (globalThis as any).MouseEvent("mousemove", { movementX: 80, movementY: 0 })
    );
    toggling.setMouseLookEnabled(false);
    expect(toggling.sample(2).rx).toBe(128);

    (globalThis as any).window.dispatchEvent(
      new (globalThis as any).MouseEvent("mousemove", { movementX: 80, movementY: 0 })
    );
    expect(toggling.sample(3).rx).toBe(128);
    toggling.stop();
  });

  it("ignores mouse movement while not pointer-locked", () => {
    (globalThis as any).document.pointerLockElement = null;
    (globalThis as any).window.dispatchEvent(
      new (globalThis as any).MouseEvent("mousemove", { movementX: 100, movementY: 100 })
    );
    const state = kbm.sample(1);
    expect(state.rx).toBe(128);
    expect(state.ry).toBe(128);
  });

  it("right stick returns to center the tick after motion stops (delta consumed each sample, not held)", () => {
    (globalThis as any).document.pointerLockElement = {};
    (globalThis as any).window.dispatchEvent(
      new (globalThis as any).MouseEvent("mousemove", { movementX: 80, movementY: 0 })
    );
    const moving = kbm.sample(1);
    expect(moving.rx).toBeGreaterThan(128);
    const settled = kbm.sample(2);
    expect(settled.rx).toBe(128);
  });

  it("setSensitivity scales right-stick deflection for the same mouse delta", () => {
    (globalThis as any).document.pointerLockElement = {};
    kbm.setSensitivity(0.1);
    expect(kbm.getSensitivity()).toBe(0.1);
    (globalThis as any).window.dispatchEvent(
      new (globalThis as any).MouseEvent("mousemove", { movementX: 50, movementY: 0 })
    );
    const low = kbm.sample(1);

    kbm.setSensitivity(2);
    (globalThis as any).window.dispatchEvent(
      new (globalThis as any).MouseEvent("mousemove", { movementX: 50, movementY: 0 })
    );
    const high = kbm.sample(2);

    expect(high.rx - 128).toBeGreaterThan(low.rx - 128);
  });

  it("clears held keys when the tab is hidden", () => {
    const doc = (globalThis as any).document;
    doc.dispatchEvent(keyEvent("keydown", "KeyA")); // no-op target, just to prove doc listeners don't interfere
    (globalThis as any).window.dispatchEvent(keyEvent("keydown", "KeyA"));
    expect(kbm.hasInput()).toBe(true);

    doc.hidden = true;
    doc.dispatchEvent(new Event("visibilitychange"));
    doc.hidden = false;

    expect(kbm.hasInput()).toBe(false);
  });
});
