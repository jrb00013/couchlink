/** Session seats: host P1, remotes P2–P4. */

export type Seat = 1 | 2 | 3 | 4;

export const SEAT_LABEL: Record<Seat, string> = {
  1: "P1",
  2: "P2",
  3: "P3",
  4: "P4",
};

/** Remote couchlink slot (1–3) → emulator/UI seat (P2–P4). Host is always 1. */
export function seatForRemoteSlot(slot: number | null): Seat {
  if (slot === 2) return 3;
  if (slot === 3) return 4;
  return 2;
}

export function seatClass(seat: Seat): string {
  return `cv-p${seat}`;
}
