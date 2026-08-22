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
    kbm = new KeyboardMouseInput();
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
