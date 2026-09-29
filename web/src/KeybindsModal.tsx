import { useEffect, useState } from "react";
import {
  DEFAULT_KBM_BINDS,
  DEFAULT_KBM_MOUSE_LOOK_ENABLED,
  DEFAULT_KBM_SENSITIVITY,
  KBM_ACTIONS,
  MAX_KBM_SENSITIVITY,
  MIN_KBM_SENSITIVITY,
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
  sensitivity,
  onSensitivityChange,
  mouseLookEnabled,
  onMouseLookEnabledChange,
}: {
  binds: KbmBinds;
  onChange: (next: KbmBinds) => void;
  onClose: () => void;
  sensitivity: number;
  onSensitivityChange: (next: number) => void;
  mouseLookEnabled: boolean;
  onMouseLookEnabledChange: (next: boolean) => void;
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
          sees the remap immediately. Saved in this browser.
        </p>
        <div className="keybinds-row kbm-mouselook-row">
          <label htmlFor="kbm-mouselook" className="keybinds-action">
            Mouse look → right stick
          </label>
          <input
            id="kbm-mouselook"
            type="checkbox"
            checked={mouseLookEnabled}
            onChange={(e) => onMouseLookEnabledChange(e.target.checked)}
          />
        </div>
        {mouseLookEnabled && (
          <div className="keybinds-row kbm-sensitivity-row">
            <label htmlFor="kbm-sensitivity" className="keybinds-action">
              Mouse look sensitivity ({sensitivity.toFixed(2)})
            </label>
            <input
              id="kbm-sensitivity"
              type="range"
              min={MIN_KBM_SENSITIVITY}
              max={MAX_KBM_SENSITIVITY}
              step={0.05}
              value={sensitivity}
              onChange={(e) => onSensitivityChange(Number(e.target.value))}
            />
          </div>
        )}
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
            onSensitivityChange(DEFAULT_KBM_SENSITIVITY);
            onMouseLookEnabledChange(DEFAULT_KBM_MOUSE_LOOK_ENABLED);
            setCapturing(null);
          }}
        >
          Reset defaults
        </button>
      </div>
    </div>
  );
}
