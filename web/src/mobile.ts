/**
 * Mobile-device detection for the touch-controller layout.
 *
 * Desktop layout is untouched; this only decides whether to render the
 * touch controller and the mobile fullscreen shell.
 *
 * Once a session is detected as mobile (coarse pointer or small touch
 * viewport), we sticky that for the tab via sessionStorage so rotating to
 * landscape (innerWidth often > 820) does not drop the mobile shell and
 * unmount the pad / collapse the video. `?mobile=0` still forces desktop.
 */

const STICKY_KEY = "couchlink-mobile-layout";

function stickyStorage(): Storage | null {
  try {
    const fromWindow = typeof window !== "undefined" ? window.sessionStorage : undefined;
    if (fromWindow) return fromWindow;
    if (typeof sessionStorage !== "undefined") return sessionStorage;
  } catch {
    /* private mode / missing */
  }
  return null;
}

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
  const store = stickyStorage();

  if (coarse || (touch && small)) {
    try {
      store?.setItem(STICKY_KEY, "1");
    } catch {
      /* ignore */
    }
    return true;
  }

  // Landscape phones often report width > 820; keep mobile if this tab
  // already chose the mobile layout and the device still has touch.
  if (touch) {
    try {
      if (store?.getItem(STICKY_KEY) === "1") return true;
    } catch {
      /* ignore */
    }
  }

  return false;
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

/** Test helper — clear sticky mobile flag between unit tests. */
export function resetMobileStickyForTests(): void {
  try {
    stickyStorage()?.removeItem(STICKY_KEY);
  } catch {
    /* ignore */
  }
}
