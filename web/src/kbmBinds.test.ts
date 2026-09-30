import { describe, expect, it, beforeEach } from "vitest";
import {
  DEFAULT_KBM_BINDS,
  FIGHTING_KBM_BINDS,
  KBM_ACTIONS,
  KBM_STORAGE_KEY,
  formatKbmCode,
  loadKbmBinds,
  saveKbmBinds,
  setBind,
} from "./kbmBinds";

describe("kbmBinds", () => {
  beforeEach(() => {
    const store = new Map<string, string>();
    (globalThis as unknown as { localStorage: Storage }).localStorage = {
      getItem: (k) => store.get(k) ?? null,
      setItem: (k, v) => {
        store.set(k, v);
      },
      removeItem: (k) => {
        store.delete(k);
      },
      clear: () => store.clear(),
      key: () => null,
      length: 0,
    };
  });

  it("loads defaults when nothing is stored", () => {
    expect(loadKbmBinds().cross).toEqual(["Space"]);
    expect(loadKbmBinds().r2).toEqual(["Mouse0"]);
    expect(loadKbmBinds().l2).toEqual(["Mouse2"]);
  });

  it("migrates v1 storage and forces LMB→R2 / RMB→L2 for shooters", () => {
    localStorage.setItem(
      "couchlink.kbm.binds.v1",
      JSON.stringify({
        ...DEFAULT_KBM_BINDS,
        r2: ["KeyZ"],
        l2: ["KeyX"],
        l3: ["Mouse0", "Mouse2"],
      })
    );
    const loaded = loadKbmBinds();
    expect(loaded.r2).toContain("Mouse0");
    expect(loaded.l2).toContain("Mouse2");
    expect(loaded.l3).not.toContain("Mouse0");
    expect(loaded.l3).not.toContain("Mouse2");
    expect(localStorage.getItem(KBM_STORAGE_KEY)).toBeTruthy();
    expect(localStorage.getItem("couchlink.kbm.binds.v1")).toBeNull();
  });

  it("round-trips a remap through localStorage", () => {
    const next = setBind(DEFAULT_KBM_BINDS, "cross", "KeyZ");
    saveKbmBinds(next);
    expect(loadKbmBinds().cross).toEqual(["KeyZ"]);
    expect(localStorage.getItem(KBM_STORAGE_KEY)).toContain("KeyZ");
  });

  it("removes a code from the previous action when rebound", () => {
    const next = setBind(DEFAULT_KBM_BINDS, "circle", "Space");
    expect(next.circle).toEqual(["Space"]);
    expect(next.cross).not.toContain("Space");
  });

  it("labels mouse and letter codes for the UI", () => {
    expect(formatKbmCode("Mouse0")).toBe("Left click");
    expect(formatKbmCode("KeyE")).toBe("E");
    expect(formatKbmCode("Space")).toBe("Space");
  });

  it("fighting preset never binds one key to two actions", () => {
    const seen = new Map<string, string>();
    for (const { action } of KBM_ACTIONS) {
      for (const code of FIGHTING_KBM_BINDS[action]) {
        expect(seen.get(code), `${code} on ${action}`).toBeUndefined();
        seen.set(code, action);
      }
    }
  });

  it("fighting preset keeps the left stick unbound and the D-pad on WASD", () => {
    expect(FIGHTING_KBM_BINDS.moveLeft).toEqual([]);
    expect(FIGHTING_KBM_BINDS.dpadLeft).toContain("KeyA");
    expect(FIGHTING_KBM_BINDS.dpadRight).toContain("KeyD");
  });
});
