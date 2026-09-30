/**
 * Remappable keyboard+mouse → DualShock2 actions.
 *
 * The browser translates these keys into the same CLPD/XInput buttons
 * PCSX2 already has bound for this seat. Changing a bind here is what
 * the game sees — PCSX2 is not reading the friend's keyboard.
 */

export const KBM_STORAGE_KEY = "couchlink.kbm.binds.v2";
/** Prior key — migrated once so stale v1 maps (wrong mouse→trigger) don't stick. */
const KBM_STORAGE_KEY_V1 = "couchlink.kbm.binds.v1";

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
  // BO2 / FPS: DOM Mouse0 = left click, Mouse2 = right click (not Mouse1).
  l2: ["Mouse2"],
  r2: ["Mouse0"],
  l3: ["KeyC"],
  r3: ["KeyV"],
  options: ["Tab"],
  create: ["KeyG"],
  dpadUp: ["KeyI", "Numpad8"],
  dpadDown: ["KeyK", "Numpad2"],
  dpadLeft: ["KeyJ", "Numpad4"],
  dpadRight: ["KeyL", "Numpad6"],
};

/** Alias — Keybinds "Shooter defaults" and BO2-style FPS. */
export const SHOOTER_KBM_BINDS: KbmBinds = DEFAULT_KBM_BINDS;

/**
 * Fighting-game layout (Mortal Kombat): WASD + arrows drive the D-pad, the
 * left stick is unbound so one key never sends two directions, and the four
 * attack buttons sit on a UIOJ-style cluster under the right hand.
 */
export const FIGHTING_KBM_BINDS: KbmBinds = {
  moveUp: [],
  moveDown: [],
  moveLeft: [],
  moveRight: [],
  cross: ["KeyJ"],
  circle: ["KeyK"],
  square: ["KeyU"],
  triangle: ["KeyI"],
  l1: ["KeyH"],
  r1: ["KeyO"],
  l2: ["KeyY"],
  r2: ["Space", "Mouse0"],
  l3: [],
  r3: [],
  options: ["Enter"],
  create: ["Tab"],
  dpadUp: ["KeyW", "ArrowUp"],
  dpadDown: ["KeyS", "ArrowDown"],
  dpadLeft: ["KeyA", "ArrowLeft"],
  dpadRight: ["KeyD", "ArrowRight"],
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
    let raw = localStorage.getItem(KBM_STORAGE_KEY);
    if (!raw) {
      // One-shot migrate from v1, then force LMB→R2 / RMB→L2 for BO2 shooters.
      const v1 = localStorage.getItem(KBM_STORAGE_KEY_V1);
      if (v1) {
        raw = v1;
        localStorage.removeItem(KBM_STORAGE_KEY_V1);
      }
    }
    if (!raw) return base;
    const parsed = JSON.parse(raw) as Partial<Record<string, unknown>>;
    for (const { action } of KBM_ACTIONS) {
      const v = parsed[action];
      if (Array.isArray(v) && v.every((x) => typeof x === "string" && x.length > 0)) {
        base[action] = v as KbmCode[];
      }
    }
    // Always keep FPS mouse→trigger mapping correct even if an old preset
    // stole Left/Right click onto L3 or left them unbound.
    base.r2 = ensureCodes(base.r2, "Mouse0");
    base.l2 = ensureCodes(base.l2, "Mouse2");
    for (const { action } of KBM_ACTIONS) {
      if (action === "r2" || action === "l2") continue;
      base[action] = base[action].filter((c) => c !== "Mouse0" && c !== "Mouse2");
    }
    saveKbmBinds(base);
    return base;
  } catch {
    return cloneBinds(DEFAULT_KBM_BINDS);
  }
}

function ensureCodes(codes: KbmCode[], required: KbmCode): KbmCode[] {
  if (codes.includes(required)) return codes;
  return [...codes, required];
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

export const KBM_SENSITIVITY_STORAGE_KEY = "couchlink.kbm.sensitivity.v1";
/** Desktop default — strong enough that BO2 camera tracks mouse, not a limp stick nudge. */
export const DEFAULT_KBM_SENSITIVITY = 1.15;
/** Applied by "Shooter defaults" — full mouse → look mapping for FPS. */
export const SHOOTER_KBM_SENSITIVITY = 1.35;
export const MIN_KBM_SENSITIVITY = 0.05;
export const MAX_KBM_SENSITIVITY = 2;

export function loadKbmSensitivity(): number {
  try {
    const raw = localStorage.getItem(KBM_SENSITIVITY_STORAGE_KEY);
    if (!raw) return DEFAULT_KBM_SENSITIVITY;
    const n = Number(raw);
    if (!Number.isFinite(n)) return DEFAULT_KBM_SENSITIVITY;
    return Math.max(MIN_KBM_SENSITIVITY, Math.min(MAX_KBM_SENSITIVITY, n));
  } catch {
    return DEFAULT_KBM_SENSITIVITY;
  }
}

export function saveKbmSensitivity(sensitivity: number): void {
  try {
    localStorage.setItem(KBM_SENSITIVITY_STORAGE_KEY, String(sensitivity));
  } catch {
    /* quota / private mode — input still works this session */
  }
}

// Mouse-look defaults on (pointer-lock gated). Toggle lives in Keybinds —
// disable if a title fights mouse-driven right-stick camera.
export const KBM_MOUSE_LOOK_STORAGE_KEY = "couchlink.kbm.mouseLook.v1";
/** Default on — friends expect mouse → camera; pointer-lock still gates it. */
export const DEFAULT_KBM_MOUSE_LOOK_ENABLED = true;

export function loadKbmMouseLookEnabled(): boolean {
  try {
    const raw = localStorage.getItem(KBM_MOUSE_LOOK_STORAGE_KEY);
    if (raw === null) return DEFAULT_KBM_MOUSE_LOOK_ENABLED;
    return raw === "true";
  } catch {
    return DEFAULT_KBM_MOUSE_LOOK_ENABLED;
  }
}

export function saveKbmMouseLookEnabled(enabled: boolean): void {
  try {
    localStorage.setItem(KBM_MOUSE_LOOK_STORAGE_KEY, String(enabled));
  } catch {
    /* quota / private mode — input still works this session */
  }
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
