/**
 * Parse a join link the same way the native desktop client does
 * (crates/client/src/invite.rs) — accepts a full URL, or the short
 * `session:pin` / `session/pin` form typed by hand.
 *
 * The address bar's own query string is read separately in App.tsx
 * (readInvite), but a link *pasted into the page itself* — the desktop
 * client's normal join flow — had no equivalent here, so users could not
 * "paste the link" the way they can with the native client.
 */

export type ParsedInvite = {
  signalingUrl?: string;
  sessionId: string;
  pin: string;
  turn: { url: string; user: string; pass: string } | null;
};

export function parseInviteString(raw: string): ParsedInvite {
  const trimmed = raw.trim();
  if (!trimmed) {
    throw new Error("paste a join URL, or session:pin");
  }

  const looksLikeUrl =
    trimmed.includes("://") ||
    trimmed.includes("?") ||
    trimmed.startsWith("http") ||
    trimmed.startsWith("ws");

  if (!looksLikeUrl) {
    const [sessionId, pin] =
      trimmed.split(":").length === 2
        ? trimmed.split(":")
        : trimmed.split("/").length === 2
          ? trimmed.split("/")
          : [];
    if (!sessionId?.trim() || !pin?.trim()) {
      throw new Error("expected join URL or session:pin");
    }
    return { sessionId: sessionId.trim(), pin: pin.trim(), turn: null };
  }

  const withScheme = trimmed.includes("://") ? trimmed : `https://${trimmed}`;
  const url = new URL(withScheme);
  const q = url.searchParams;

  const sessionId = (q.get("s") ?? q.get("session") ?? "").trim();
  const pin = (q.get("p") ?? q.get("pin") ?? "").trim();
  if (!sessionId) throw new Error("join URL missing session (?s= or ?session=)");
  if (!pin) throw new Error("join URL missing PIN (?p= or ?pin=)");

  const signalingUrl = q.get("ws") ?? q.get("signaling") ?? undefined;
  const turnUrl = q.get("turn");
  const turnUser = q.get("turnu");
  const turnPass = q.get("turnp");
  const turn =
    turnUrl && turnUser && turnPass ? { url: turnUrl, user: turnUser, pass: turnPass } : null;

  return { signalingUrl, sessionId, pin, turn };
}
