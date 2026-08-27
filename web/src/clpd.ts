/** Binary CLPD pad frames — must match crates/proto/src/pad_frame.rs */

export const PAD_CHANNEL = "pad";
export const PAD_VERSION = 1;
export const PAD_VERSION_V2 = 2;
export const PAD_FRAME_LEN = 31;
export const PAD_FRAME_LEN_V2 = 35;

export const BTN = {
  SQUARE: 1 << 0,
  CROSS: 1 << 1,
  CIRCLE: 1 << 2,
  TRIANGLE: 1 << 3,
  L1: 1 << 4,
  R1: 1 << 5,
  L2: 1 << 6,
  R2: 1 << 7,
  CREATE: 1 << 8,
  OPTIONS: 1 << 9,
  L3: 1 << 10,
  R3: 1 << 11,
  PS: 1 << 12,
  TOUCH: 1 << 13,
  MUTE: 1 << 14,
  DPAD_UP: 1 << 16,
  DPAD_DOWN: 1 << 17,
  DPAD_LEFT: 1 << 18,
  DPAD_RIGHT: 1 << 19,
} as const;

export type PadState = {
  seq: number;
  buttons: number;
  lx: number;
  ly: number;
  rx: number;
  ry: number;
  l2: number;
  r2: number;
  /** Browser performance.now at send (ms, u32 wrap ok). */
  clientTsMs?: number;
};

export function encodeClpd(p: PadState): ArrayBuffer {
  const buf = new ArrayBuffer(PAD_FRAME_LEN_V2);
  const v = new DataView(buf);
  v.setUint8(0, 0x43); // C
  v.setUint8(1, 0x4c); // L
  v.setUint8(2, 0x50); // P
  v.setUint8(3, 0x44); // D
  v.setUint8(4, PAD_VERSION_V2);
  v.setUint32(5, p.seq >>> 0, true);
  v.setUint32(9, p.buttons >>> 0, true);
  v.setUint8(13, p.lx & 0xff);
  v.setUint8(14, p.ly & 0xff);
  v.setUint8(15, p.rx & 0xff);
  v.setUint8(16, p.ry & 0xff);
  v.setUint8(17, p.l2 & 0xff);
  v.setUint8(18, p.r2 & 0xff);
  // gx,gy,gz,touch… zeroed
  v.setUint32(31, (p.clientTsMs ?? performance.now()) >>> 0, true);
  return buf;
}

function axisToU8(v: number): number {
  // Gamepad API: -1..1 → 0..255 (128 center)
  return Math.max(0, Math.min(255, Math.round((v + 1) * 127.5)));
}

function triggerToU8(v: number): number {
  return Math.max(0, Math.min(255, Math.round(v * 255)));
}

/** Map browser Gamepad → DualSense-shaped PadState (standard mapping). */
export function fromBrowserGamepad(gp: Gamepad, seq: number): PadState {
  const b = gp.buttons;
  const ax = gp.axes;
  let buttons = 0;
  const pressed = (i: number) => !!b[i]?.pressed;
  const value = (i: number) => b[i]?.value ?? 0;

  if (pressed(0)) buttons |= BTN.CROSS;
  if (pressed(1)) buttons |= BTN.CIRCLE;
  if (pressed(2)) buttons |= BTN.SQUARE;
  if (pressed(3)) buttons |= BTN.TRIANGLE;
  if (pressed(4)) buttons |= BTN.L1;
  if (pressed(5)) buttons |= BTN.R1;
  if (pressed(6) || value(6) > 0.1) buttons |= BTN.L2;
  if (pressed(7) || value(7) > 0.1) buttons |= BTN.R2;
  if (pressed(8)) buttons |= BTN.CREATE;
  if (pressed(9)) buttons |= BTN.OPTIONS;
  if (pressed(10)) buttons |= BTN.L3;
  if (pressed(11)) buttons |= BTN.R3;
  if (pressed(12)) buttons |= BTN.DPAD_UP;
  if (pressed(13)) buttons |= BTN.DPAD_DOWN;
  if (pressed(14)) buttons |= BTN.DPAD_LEFT;
  if (pressed(15)) buttons |= BTN.DPAD_RIGHT;
  if (pressed(16)) buttons |= BTN.PS;
  if (pressed(17)) buttons |= BTN.TOUCH;

  return {
    seq,
    buttons,
    lx: axisToU8(ax[0] ?? 0),
    ly: axisToU8(ax[1] ?? 0),
    rx: axisToU8(ax[2] ?? 0),
    ry: axisToU8(ax[3] ?? 0),
    l2: triggerToU8(value(6)),
    r2: triggerToU8(value(7)),
  };
}
