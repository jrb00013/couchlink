import { describe, expect, it } from "vitest";
import { seatClass, seatForRemoteSlot } from "./seat";

describe("seatForRemoteSlot", () => {
  it("maps remote slots onto P2–P4 (host is P1)", () => {
    expect(seatForRemoteSlot(1)).toBe(2);
    expect(seatForRemoteSlot(2)).toBe(3);
    expect(seatForRemoteSlot(3)).toBe(4);
    expect(seatForRemoteSlot(null)).toBe(2);
  });

  it("names the css class after the seat", () => {
    expect(seatClass(1)).toBe("cv-p1");
    expect(seatClass(4)).toBe("cv-p4");
  });
});
