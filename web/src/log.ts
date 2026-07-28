/** Browser console diagnostics — filter DevTools with "couchlink". */
const TAG = "[couchlink]";

function enabled(): boolean {
  if (typeof window === "undefined") return false;
  const q = new URLSearchParams(window.location.search);
  if (q.get("debug") === "0") return false;
  return true;
}

export function clog(...args: unknown[]) {
  if (!enabled()) return;
  console.log(TAG, ...args);
}

export function cwarn(...args: unknown[]) {
  if (!enabled()) return;
  console.warn(TAG, ...args);
}

export function cerror(...args: unknown[]) {
  console.error(TAG, ...args);
}
