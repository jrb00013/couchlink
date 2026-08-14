import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { detectMobile } from "./mobile";

const globalAny = globalThis as unknown as {
  window?: {
    location: { search: string };
    innerWidth: number;
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

function fakeMatchMedia(coarse: boolean) {
  globalAny.window!.matchMedia = (query: string) => ({
    matches: query.includes("coarse") ? coarse : false,
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
    fakeMatchMedia(false);
    expect(detectMobile()).toBe(false);
  });

  it("is true for a coarse touch phone", () => {
    setInnerWidth(390);
    setTouchPoints(5);
    fakeMatchMedia(true);
    expect(detectMobile()).toBe(true);
  });

  it("is true for a touch laptop with small viewport", () => {
    setInnerWidth(780);
    setTouchPoints(10);
    fakeMatchMedia(false);
    expect(detectMobile()).toBe(true);
  });

  it("respects the ?mobile=1 override", () => {
    setSearch("?mobile=1");
    setInnerWidth(1600);
    setTouchPoints(0);
    fakeMatchMedia(false);
    expect(detectMobile()).toBe(true);
  });

  it("respects the ?mobile=0 override", () => {
    setSearch("?mobile=0");
    setInnerWidth(390);
    setTouchPoints(5);
    fakeMatchMedia(true);
    expect(detectMobile()).toBe(false);
  });
});