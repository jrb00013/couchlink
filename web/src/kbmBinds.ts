/**
 * Remappable keyboard+mouse → DualShock2 actions.
 *
 * The browser translates these keys into the same CLPD/XInput buttons
 * PCSX2 already has bound for this seat. Changing a bind here is what
 * the game sees — PCSX2 is not reading the friend's keyboard.
 */

export const KBM_STORAGE_KEY = "couchlink.kbm.binds.v1";

/** KeyboardEvent.code or Mouse0 / Mouse1 / Mouse2. */
export type KbmCode = string;

export type KbmAction =
  | "moveUp"
  | "moveDown"
  | "moveLeft"
  | "moveRight"
  | "cross"
  | "circle"
  | "square"
  | "triangle"
  | "l1"
  | "r1"
  | "l2"
  | "r2"
  | "l3"
  | "r3"
  | "options"
  | "create"
  | "dpadUp"
  | "dpadDown"
  | "dpadLeft"
  | "dpadRight";

export type KbmBinds = Record<KbmAction, KbmCode[]>;

export const KBM_ACTIONS: ReadonlyArray<{ action: KbmAction; label: string }> = [
  { action: "moveUp", label: "Move up (left stick)" },
  { action: "moveDown", label: "Move down (left stick)" },
  { action: "moveLeft", label: "Move left (left stick)" },
  { action: "moveRight", label: "Move right (left stick)" },
  { action: "cross", label: "✕ Cross — jump / confirm" },
  { action: "circle", label: "○ Circle — cancel" },
  { action: "square", label: "□ Square" },
  { action: "triangle", label: "△ Triangle" },
  { action: "l1", label: "L1" },
  { action: "r1", label: "R1" },
  { action: "l2", label: "L2 — aim" },
  { action: "r2", label: "R2 — shoot" },
  { action: "l3", label: "L3 — stick click" },
  { action: "r3", label: "R3 — stick click" },
  { action: "options", label: "Options / Start" },
  { action: "create", label: "Create / Select" },
  { action: "dpadUp", label: "D-Pad up" },
  { action: "dpadDown", label: "D-Pad down" },
  { action: "dpadLeft", label: "D-Pad left" },
  { action: "dpadRight", label: "D-Pad right" },
];

export const DEFAULT_KBM_BINDS: KbmBinds = {
  moveUp: ["KeyW", "ArrowUp"],
  moveDown: ["KeyS", "ArrowDown"],
  moveLeft: ["KeyA", "ArrowLeft"],
  moveRight: ["KeyD", "ArrowRight"],
  cross: ["Space"],
  circle: ["KeyF"],
  square: ["KeyQ"],
  triangle: ["KeyE"],
  l1: ["ShiftLeft", "ShiftRight"],
  r1: ["KeyR"],
  l2: ["Mouse2"],
  r2: ["Mouse0"],
  l3: ["KeyC", "Mouse1"],
  r3: ["KeyV"],
  options: ["Tab"],
  create: ["KeyG"],
  dpadUp: ["KeyI", "Numpad8"],
  dpadDown: ["KeyK", "Numpad2"],
  dpadLeft: ["KeyJ", "Numpad4"],
  dpadRight: ["KeyL", "Numpad6"],
};

const ACTIONS = new Set(KBM_ACTIONS.map((a) => a.action));

export function isKbmAction(s: string): s is KbmAction {
  return ACTIONS.has(s as KbmAction);
}

export function cloneBinds(b: KbmBinds): KbmBinds {
  const out = {} as KbmBinds;
  for (const { action } of KBM_ACTIONS) {
    out[action] = [...(b[action] ?? DEFAULT_KBM_BINDS[action])];
  }
  return out;
}

export function loadKbmBinds(): KbmBinds {
  const base = cloneBinds(DEFAULT_KBM_BINDS);
  try {
    const raw = localStorage.getItem(KBM_STORAGE_KEY);
    if (!raw) return base;
    const parsed = JSON.parse(raw) as Partial<Record<string, unknown>>;
    for (const { action } of KBM_ACTIONS) {
      const v = parsed[action];
      if (Array.isArray(v) && v.every((x) => typeof x === "string" && x.length > 0)) {
        base[action] = v as KbmCode[];
      }
    }
    return base;
  } catch {
    return cloneBinds(DEFAULT_KBM_BINDS);
  }
}

export function saveKbmBinds(binds: KbmBinds): void {
  try {
    localStorage.setItem(KBM_STORAGE_KEY, JSON.stringify(binds));
  } catch {
    /* quota / private mode — input still works this session */
  }
}

/** Assign `code` to `action`, removing it from every other action. */
export function setBind(binds: KbmBinds, action: KbmAction, code: KbmCode): KbmBinds {
  const next = cloneBinds(binds);
  for (const { action: a } of KBM_ACTIONS) {
    next[a] = next[a].filter((c) => c !== code);
  }
  next[action] = [code];
  return next;
}

export function formatKbmCode(code: KbmCode): string {
  if (code === "Mouse0") return "Left click";
  if (code === "Mouse1") return "Middle click";
  if (code === "Mouse2") return "Right click";
  if (code.startsWith("Key") && code.length === 4) return code.slice(3);
  if (code.startsWith("Digit")) return code.slice(5);
  if (code.startsWith("Arrow")) return code.slice(5);
  if (code.startsWith("Numpad")) return `Numpad ${code.slice(6)}`;
  if (code === "ShiftLeft" || code === "ShiftRight") return "Shift";
  if (code === "ControlLeft" || code === "ControlRight") return "Ctrl";
  if (code === "AltLeft" || code === "AltRight") return "Alt";
  if (code === "Space") return "Space";
  if (code === "Tab") return "Tab";
  return code;
}

export function formatKbmCodes(codes: KbmCode[]): string {
  return codes.map(formatKbmCode).join(" / ") || "—";
}

export function codeFromKeyboardEvent(e: KeyboardEvent): KbmCode | null {
  if (e.code === "Escape") return null;
  return e.code;
}

export function codeFromMouseEvent(e: MouseEvent): KbmCode | null {
  if (e.button < 0 || e.button > 2) return null;
  return `Mouse${e.button}`;
}
