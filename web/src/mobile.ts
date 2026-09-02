/**
 * Mobile-device detection for the touch-controller layout.
 *
 * Desktop layout is untouched; this only decides whether to render the
 * touch controller and the mobile fullscreen shell. Coarse pointer is the
 * strongest signal (touch is the primary input). A phone also has a small
 * viewport, so we require it when touch is present but the pointer is not
 * flagged coarse (e.g. a hybrid device).
 *
 * `?mobile=1` forces the mobile layout (handy on a desktop browser), and
 * `?mobile=0` forces the desktop layout (handy on a real phone).
 */

export function detectMobile(): boolean {
  if (typeof window === "undefined" || typeof navigator === "undefined") {
    return false;
  }
  const override = new URLSearchParams(window.location.search).get("mobile");
  if (override === "1") return true;
  if (override === "0") return false;

  const coarse = window.matchMedia?.("(pointer: coarse)").matches ?? false;
  const touch = navigator.maxTouchPoints > 0;
  const small = window.innerWidth <= 820;
  return coarse || (touch && small);
}

/** Phone turned on its side — the play posture for mobile side-mode. */
export function detectLandscape(): boolean {
  if (typeof window === "undefined") return false;
  const mm = window.matchMedia?.("(orientation: landscape)");
  if (mm && typeof mm.matches === "boolean") return mm.matches;
  return window.innerWidth > window.innerHeight;
}

/** Landscape + mobile + in-session → side-screen play (video fill, rails). */
export function isSideMode(opts: {
  mobile: boolean;
  landscape: boolean;
  connected: boolean;
}): boolean {
  return opts.mobile && opts.landscape && opts.connected;
}

type FsEl = HTMLElement & {
  webkitRequestFullscreen?: () => Promise<void> | void;
  webkitRequestFullScreen?: () => Promise<void> | void;
};

type FsDoc = Document & {
  webkitExitFullscreen?: () => Promise<void> | void;
  webkitFullscreenElement?: Element | null;
};

export function isNativeFullscreen(): boolean {
  if (typeof document === "undefined") return false;
  const doc = document as FsDoc;
  return !!(document.fullscreenElement || doc.webkitFullscreenElement);
}

/** Best-effort Fullscreen API. iOS often rejects this without a tap; CSS side-mode still applies. */
export async function enterElementFullscreen(el: HTMLElement): Promise<boolean> {
  const anyEl = el as FsEl;
  try {
    if (el.requestFullscreen) {
      await el.requestFullscreen();
      return true;
    }
    if (anyEl.webkitRequestFullscreen) {
      await anyEl.webkitRequestFullscreen();
      return true;
    }
    if (anyEl.webkitRequestFullScreen) {
      await anyEl.webkitRequestFullScreen();
      return true;
    }
  } catch {
    return false;
  }
  return false;
}

export async function exitElementFullscreen(): Promise<void> {
  if (typeof document === "undefined") return;
  const doc = document as FsDoc;
  try {
    if (document.fullscreenElement) await document.exitFullscreen();
    else if (doc.webkitFullscreenElement && doc.webkitExitFullscreen) {
      await doc.webkitExitFullscreen();
    }
  } catch {
    /* ignore */
  }
}

export async function lockLandscape(): Promise<void> {
  try {
    const o = screen.orientation as ScreenOrientation & {
      lock?: (orientation: string) => Promise<void>;
    };
    await o.lock?.("landscape");
  } catch {
    /* iOS / unsigned web — lock is optional */
  }
}

export function unlockOrientation(): void {
  try {
    screen.orientation?.unlock?.();
  } catch {
    /* ignore */
  }
}
