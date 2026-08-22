import { useEffect, useState } from "react";
import {
  DEFAULT_KBM_BINDS,
  KBM_ACTIONS,
  type KbmAction,
  type KbmBinds,
  cloneBinds,
  codeFromKeyboardEvent,
  codeFromMouseEvent,
  formatKbmCodes,
  saveKbmBinds,
  setBind,
} from "./kbmBinds";

export function KeybindsModal({
  binds,
  onChange,
  onClose,
}: {
  binds: KbmBinds;
  onChange: (next: KbmBinds) => void;
  onClose: () => void;
}) {
  const [capturing, setCapturing] = useState<KbmAction | null>(null);

  useEffect(() => {
    if (!capturing) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const code = codeFromKeyboardEvent(e);
      if (!code) {
        if (e.code === "Escape") setCapturing(null);
        return;
      }
      const next = setBind(binds, capturing, code);
      saveKbmBinds(next);
      onChange(next);
      setCapturing(null);
    };
    const onMouse = (e: MouseEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const code = codeFromMouseEvent(e);
      if (!code) return;
      const next = setBind(binds, capturing, code);
      saveKbmBinds(next);
      onChange(next);
      setCapturing(null);
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("mousedown", onMouse, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("mousedown", onMouse, true);
    };
  }, [capturing, binds, onChange]);

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal keybinds-modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h2>Keyboard + Mouse keybinds</h2>
          <button type="button" className="modal-close" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </div>
        <p className="modal-hint">
          Click a row, then press a key or mouse button. These fire the same
          Xbox / DualShock2 buttons PCSX2 already has for your seat — the game
          sees the remap immediately. Mouse look stays on the right stick.
          Saved in this browser.
        </p>
        <div className="keybinds-list">
          {KBM_ACTIONS.map(({ action, label }) => (
            <button
              key={action}
              type="button"
              className={`keybinds-row is-button${capturing === action ? " is-capturing" : ""}`}
              onClick={() => setCapturing(action)}
            >
              <span className="keybinds-key">
                {capturing === action ? "press a key…" : formatKbmCodes(binds[action])}
              </span>
              <span className="keybinds-action">{label}</span>
            </button>
          ))}
        </div>
        <button
          type="button"
          className="kbm-keybinds-btn"
          onClick={() => {
            const next = cloneBinds(DEFAULT_KBM_BINDS);
            saveKbmBinds(next);
            onChange(next);
            setCapturing(null);
          }}
        >
          Reset defaults
        </button>
      </div>
    </div>
  );
}
