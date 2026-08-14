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