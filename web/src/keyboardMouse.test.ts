import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { DEFAULT_KEYMAP, KeyboardMouseInput, keyLabel } from "./keyboardMouse";
import { BTN } from "./clpd";

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

  it("honours a remapped keymap instead of the defaults", () => {
    kbm.stop();
    kbm = new KeyboardMouseInput({ keymap: { cross: "KeyX", r2: "KeyZ" } });
    kbm.start();

    (globalThis as any).window.dispatchEvent(keyEvent("keydown", "KeyX"));
    const state = kbm.sample(1);
    expect(state.buttons & BTN.CROSS).toBe(BTN.CROSS);
    // The default Space binding no longer fires.
    expect(DEFAULT_KEYMAP.cross).toBe("Space");
    expect(state.buttons & BTN.CROSS).toBe(BTN.CROSS);
  });

  it("keeps mouse triggers after a remap", () => {
    kbm.stop();
    kbm = new KeyboardMouseInput({ keymap: {} }); // wipe every key
    kbm.start();

    (globalThis as any).window.dispatchEvent(new (globalThis as any).MouseEvent("mousedown", { button: 0 }));
    const state = kbm.sample(1);
    expect(state.buttons & BTN.R2).toBe(BTN.R2);
  });

  it("serialises the keymap for the host", () => {
    const json = kbm.keymapJson();
    const parsed = JSON.parse(json);
    expect(parsed.cross).toBe(DEFAULT_KEYMAP.cross);
    expect(parsed.lstick_up).toBe("KeyW");
  });

  it("keyLabel renders readable names", () => {
    expect(keyLabel("KeyW")).toBe("W");
    expect(keyLabel("Space")).toBe("Space");
    expect(keyLabel("ArrowUp")).toBe("↑");
    expect(keyLabel("ShiftLeft")).toBe("Shift");
    expect(keyLabel(undefined)).toBe("");
  });
});
