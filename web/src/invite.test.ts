import { describe, it, expect } from "vitest";
import { parseInviteString } from "./invite";

describe("parseInviteString", () => {
  it("parses a full host-printed join URL", () => {
    const url =
      "https://airplane-magazines-grace-walter.trycloudflare.com/?s=653ab7023d58&p=012895&auto=1&ws=wss%3A%2F%2Fairplane-magazines-grace-walter.trycloudflare.com%2Fws&turn=turn%3A76.35.135.156%3A3478&turnu=cl90802d0a&turnp=ddb736cf93ee7eafb6c1a01cf66e1440";
    const p = parseInviteString(url);
    expect(p.sessionId).toBe("653ab7023d58");
    expect(p.pin).toBe("012895");
    expect(p.signalingUrl).toBe("wss://airplane-magazines-grace-walter.trycloudflare.com/ws");
    expect(p.turn).toEqual({
      url: "turn:76.35.135.156:3478",
      user: "cl90802d0a",
      pass: "ddb736cf93ee7eafb6c1a01cf66e1440",
    });
  });

  it("accepts a link pasted without a scheme", () => {
    const p = parseInviteString("game.example.com/?s=abc&p=1234");
    expect(p.sessionId).toBe("abc");
    expect(p.pin).toBe("1234");
  });

  it("accepts the short session:pin form", () => {
    const p = parseInviteString("friends-night:012895");
    expect(p.sessionId).toBe("friends-night");
    expect(p.pin).toBe("012895");
    expect(p.turn).toBeNull();
  });

  it("accepts the short session/pin form", () => {
    const p = parseInviteString("friends-night/012895");
    expect(p.sessionId).toBe("friends-night");
    expect(p.pin).toBe("012895");
  });

  it("rejects a URL missing the session", () => {
    expect(() => parseInviteString("https://h/?p=1234")).toThrow(/session/i);
  });

  it("rejects a URL missing the PIN", () => {
    expect(() => parseInviteString("https://h/?s=abc")).toThrow(/PIN/i);
  });

  it("rejects empty input", () => {
    expect(() => parseInviteString("   ")).toThrow();
  });

  it("rejects garbage that is neither a URL nor session:pin", () => {
    expect(() => parseInviteString("not a link")).toThrow();
  });
});
