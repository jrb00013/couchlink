import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { detectLandscape, detectMobile, isSideMode } from "./mobile";

const globalAny = globalThis as unknown as {
  window?: {
    location: { search: string };
    innerWidth: number;
    innerHeight: number;
    matchMedia?: (q: string) => unknown;
  };
  navigator?: { maxTouchPoints: number };
};

let navigatorState: { maxTouchPoints: number } = { maxTouchPoints: 0 };

beforeEach(() => {
  const location = { search: "" };
  globalAny.window = {
    location,
    innerWidth: 1200,
    innerHeight: 800,
    matchMedia: () => ({ matches: false, media: "", onchange: null }),
  };
  navigatorState = { maxTouchPoints: 0 };
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    get: () => navigatorState,
  });
});

afterEach(() => {
  delete globalAny.window;
});

function setSearch(q: string) {
  globalAny.window!.location.search = q;
}

function setInnerWidth(v: number) {
  globalAny.window!.innerWidth = v;
}

function setTouchPoints(v: number) {
  navigatorState.maxTouchPoints = v;
}

function fakeMatchMedia(opts: { coarse?: boolean; landscape?: boolean }) {
  const coarse = opts.coarse ?? false;
  const landscape = opts.landscape ?? false;
  globalAny.window!.matchMedia = (query: string) => ({
    matches: query.includes("coarse")
      ? coarse
      : query.includes("orientation: landscape")
        ? landscape
        : false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  });
}

describe("mobile — device detection", () => {
  it("is false when no touch and no coarse pointer", () => {
    setInnerWidth(1200);
    setTouchPoints(0);
    fakeMatchMedia({ coarse: false });
    expect(detectMobile()).toBe(false);
  });

  it("is true for a coarse touch phone", () => {
    setInnerWidth(390);
    setTouchPoints(5);
    fakeMatchMedia({ coarse: true });
    expect(detectMobile()).toBe(true);
  });

  it("is true for a touch laptop with small viewport", () => {
    setInnerWidth(780);
    setTouchPoints(10);
    fakeMatchMedia({ coarse: false });
    expect(detectMobile()).toBe(true);
  });

  it("respects the ?mobile=1 override", () => {
    setSearch("?mobile=1");
    setInnerWidth(1600);
    setTouchPoints(0);
    fakeMatchMedia({ coarse: false });
    expect(detectMobile()).toBe(true);
  });

  it("respects the ?mobile=0 override", () => {
    setSearch("?mobile=0");
    setInnerWidth(390);
    setTouchPoints(5);
    fakeMatchMedia({ coarse: true });
    expect(detectMobile()).toBe(false);
  });
});

describe("mobile — landscape / side mode", () => {
  it("detects landscape from the orientation media query", () => {
    setInnerWidth(390);
    fakeMatchMedia({ landscape: true });
    expect(detectLandscape()).toBe(true);
  });

  it("falls back to width > height when matchMedia has no orientation", () => {
    globalAny.window!.matchMedia = undefined;
    setInnerWidth(844);
    globalAny.window!.innerHeight = 390;
    expect(detectLandscape()).toBe(true);
  });

  it("side mode is only mobile + landscape + connected", () => {
    expect(isSideMode({ mobile: true, landscape: true, connected: true })).toBe(true);
    expect(isSideMode({ mobile: true, landscape: false, connected: true })).toBe(false);
    expect(isSideMode({ mobile: false, landscape: true, connected: true })).toBe(false);
    expect(isSideMode({ mobile: true, landscape: true, connected: false })).toBe(false);
  });
});