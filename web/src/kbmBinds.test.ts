import { describe, expect, it, beforeEach } from "vitest";
import {
  DEFAULT_KBM_BINDS,
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
});
