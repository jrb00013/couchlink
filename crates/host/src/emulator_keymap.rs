//! Write a keyboard/mouse player's key bindings into the emulator config.
//!
//! A keyboard player's CLPD frames already drive the virtual DualSense pad, so
//! the emulator could work purely through that device. But the friend asked for
//! the emulator to *also* be bound to the actual keyboard keys they press — the
//! [`SignalMessage::KeyMap`] the browser sends carries a JSON object mapping a
//! control name to the browser `KeyboardEvent.code` it is bound to, and this
//! module translates those codes into the key names each emulator understands
//! and rewrites its config for emulator player `slot + 1` (the host's own pad
//! owns emulator P1).
//!
//! Two formats are written, mirroring `scripts/link-emulator-pad.sh`:
//!
//!  - PCSX2: `[PadN]` block with `Cross = Keyboard/Key_Space` style bindings.
//!    The `Keyboard/Key_*` value syntax and the SDL-style key names are
//!    unverified against a live PCSX2 — confirm against PCSX2.ini before
//!    trusting them in a real session.
//!  - RPCS3: `Player N Input:` block with `Handler: Keyboard` and
//!    `Cross: Space` style bindings. Same caveat for the SDL key names.
//!
//! Best-effort by design, like `emulator_pad`: every failure here still leaves
//! video streaming and the virtual-pad path intact, so nothing in this module
//! is allowed to take the session down. Each config is backed up once before
//! its first edit and never rewritten while it already matches.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use tracing::{info, warn};

/// A translated key name, one per emulator dialect.
struct KeyNames {
    /// PCSX2 `Keyboard/Key_*` suffix, e.g. `Key_Space`.
    pcsx2: String,
    /// RPCS3 / SDL-style name, e.g. `Space`.
    sdl: String,
}

/// Translate a browser `KeyboardEvent.code` into the key names both emulators
/// use. Unknown codes (mouse buttons, media keys, `Unidentified`…) are skipped.
///
/// The exact names are best-effort assumptions from PCSX2 / RPCS3 conventions;
/// they have not been verified against a running emulator.
fn key_names(code: &str) -> Option<KeyNames> {
    let letters = |c: char| (c.to_ascii_uppercase());
    if let Some(b) = code.strip_prefix("Key").filter(|s| s.len() == 1) {
        let up = letters(b.chars().next().unwrap());
        return Some(KeyNames {
            pcsx2: format!("Key_{up}"),
            sdl: up.to_string(),
        });
    }
    if let Some(d) = code.strip_prefix("Digit") {
        return Some(KeyNames {
            pcsx2: format!("Key_{d}"),
            sdl: d.to_string(),
        });
    }
    if let Some(n) = code.strip_prefix("Numpad") {
        return Some(KeyNames {
            pcsx2: format!("Key_NumPad{n}"),
            sdl: format!("Numpad{n}"),
        });
    }
    if let Some(f) = code.strip_prefix('F') {
        let Ok(n): Result<u8, _> = f.parse() else {
            return None;
        };
        if (1..=12).contains(&n) {
            return Some(KeyNames {
                pcsx2: format!("Key_F{f}"),
                sdl: format!("F{f}"),
            });
        }
        return None;
    }
    let map: &[(&str, &str, &str)] = &[
        ("Space", "Key_Space", "Space"),
        ("ArrowUp", "Key_Up", "Up"),
        ("ArrowDown", "Key_Down", "Down"),
        ("ArrowLeft", "Key_Left", "Left"),
        ("ArrowRight", "Key_Right", "Right"),
        ("ShiftLeft", "Key_Shift", "LShift"),
        ("ShiftRight", "Key_Shift", "RShift"),
        ("ControlLeft", "Key_Control", "LControl"),
        ("ControlRight", "Key_Control", "RControl"),
        ("AltLeft", "Key_Alt", "LAlt"),
        ("AltRight", "Key_Alt", "RAlt"),
        ("Tab", "Key_Tab", "Tab"),
        ("Enter", "Key_Return", "Return"),
        ("Escape", "Key_Escape", "Escape"),
        ("Backspace", "Key_Backspace", "Backspace"),
        ("Delete", "Key_Delete", "Delete"),
        ("Insert", "Key_Insert", "Insert"),
        ("Home", "Key_Home", "Home"),
        ("End", "Key_End", "End"),
        ("PageUp", "Key_PageUp", "PageUp"),
        ("PageDown", "Key_PageDown", "PageDown"),
        ("CapsLock", "Key_CapsLock", "CapsLock"),
        ("Semicolon", "Key_Semicolon", "Semicolon"),
        ("Comma", "Key_Comma", "Comma"),
        ("Period", "Key_Period", "Period"),
        ("Slash", "Key_Slash", "Slash"),
        ("Minus", "Key_Minus", "-"),
        ("Equal", "Key_Equal", "="),
        ("Quote", "Key_Quote", "'"),
        ("Backquote", "Key_Backquote", "Grave"),
        ("BracketLeft", "Key_BracketLeft", "["),
        ("BracketRight", "Key_BracketRight", "]"),
        ("Backslash", "Key_Backslash", "\\"),
    ];
    for (code_s, p, s) in map {
        if code == *code_s {
            return Some(KeyNames {
                pcsx2: (*p).into(),
                sdl: (*s).into(),
            });
        }
    }
    None
}

/// Control name → the emulator's pad-field / YAML key for that control.
fn pcsx2_field(control: &str) -> Option<&'static str> {
    Some(match control {
        "cross" => "Cross",
        "circle" => "Circle",
        "square" => "Square",
        "triangle" => "Triangle",
        "l1" => "L1",
        "r1" => "R1",
        "l2" => "L2",
        "r2" => "R2",
        "options" => "Start",
        "create" => "Select",
        "dpad_up" => "Up",
        "dpad_down" => "Down",
        "dpad_left" => "Left",
        "dpad_right" => "Right",
        "lstick_up" => "LUp",
        "lstick_down" => "LDown",
        "lstick_left" => "LLeft",
        "lstick_right" => "LRight",
        "rstick_up" => "RUp",
        "rstick_down" => "RDown",
        "rstick_left" => "RLeft",
        "rstick_right" => "RRight",
        _ => return None,
    })
}

fn rpcs3_key(control: &str) -> Option<&'static str> {
    Some(match control {
        "cross" => "Cross",
        "circle" => "Circle",
        "square" => "Square",
        "triangle" => "Triangle",
        "l1" => "L1",
        "r1" => "R1",
        "l2" => "L2",
        "r2" => "R2",
        "options" => "Start",
        "create" => "Select",
        "dpad_up" => "Up",
        "dpad_down" => "Down",
        "dpad_left" => "Left",
        "dpad_right" => "Right",
        "lstick_up" => "Left Stick Up",
        "lstick_down" => "Left Stick Down",
        "lstick_left" => "Left Stick Left",
        "lstick_right" => "Left Stick Right",
        "rstick_up" => "Right Stick Up",
        "rstick_down" => "Right Stick Down",
        "rstick_left" => "Right Stick Left",
        "rstick_right" => "Right Stick Right",
        _ => return None,
    })
}

/// Parse the JSON keymap string into a stable control → code map.
fn parse_keymap(json: &str) -> Option<BTreeMap<String, String>> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let obj = v.as_object()?;
    let mut out = BTreeMap::new();
    for (k, val) in obj {
        if let Some(code) = val.as_str() {
            out.insert(k.clone(), code.to_string());
        }
    }
    Some(out)
}

/// Locate PCSX2.ini the same way `scripts/link-emulator-pad.sh` does:
/// `COUCHLINK_PCSX2_CONFIG` override, a WSL user-profile search for the newest
/// `PCSX2.ini`, then the Linux default path.
fn find_pcsx2_config() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("COUCHLINK_PCSX2_CONFIG") {
        let b = PathBuf::from(p);
        if b.is_file() {
            return Some(b);
        }
        warn!("COUCHLINK_PCSX2_CONFIG points at a missing file ({p}) — searching");
    }
    if let Ok(home) = std::env::var("HOME") {
        let linux = PathBuf::from(&home).join(".config/PCSX2/inis/PCSX2.ini");
        if linux.is_file() {
            return Some(linux);
        }
        if let Some(hit) = find_newest("/mnt/c/Users", "PCSX2.ini", 8) {
            return Some(hit);
        }
    }
    None
}

/// Locate RPCS3's `Default.yml` the same way the script does:
/// `COUCHLINK_RPCS3_CONFIG` override, a WSL user-profile search, then the
/// Linux default path.
fn find_rpcs3_config() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("COUCHLINK_RPCS3_CONFIG") {
        let b = PathBuf::from(p);
        if b.is_file() {
            return Some(b);
        }
        warn!("COUCHLINK_RPCS3_CONFIG points at a missing file ({p}) — searching");
    }
    if let Ok(home) = std::env::var("HOME") {
        let linux = PathBuf::from(&home).join(".config/rpcs3/input_configs/global/Default.yml");
        if linux.is_file() {
            return Some(linux);
        }
        if let Some(hit) = find_newest("/mnt/c/Users", "Default.yml", 6) {
            // Only a Default.yml that lives under RPCS3's input_configs tree is
            // the pad config — there are many other Default.yml files on a box.
            let hit_str = hit.to_string_lossy().to_ascii_lowercase();
            if hit_str.contains("rpcs3") && hit_str.contains("input_configs") {
                return Some(hit);
            }
        }
    }
    None
}

/// Newest file with `name` at most `max_depth` levels under `root` (by mtime,
/// so a portable install's stale copy never beats the one PCSX2 actually writes).
fn find_newest(root: &str, name: &str, max_depth: usize) -> Option<PathBuf> {
    let root = PathBuf::from(root);
    if !root.is_dir() {
        return None;
    }
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    fn walk(dir: &PathBuf, name: &str, depth: usize, max: usize, best: &mut Option<(std::time::SystemTime, PathBuf)>) {
        if depth > max {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                walk(&p, name, depth + 1, max, best);
            } else if meta.is_file() && p.file_name().and_then(|n| n.to_str()) == Some(name) {
                if best.as_ref().is_none_or(|(t, _)| meta.modified().unwrap_or(t).ge(t)) {
                    *best = Some((meta.modified().unwrap_or(std::time::UNIX_EPOCH), p));
                }
            }
        }
    }
    walk(&root, name, 0, max_depth, &mut best);
    best.map(|(_, p)| p)
}

/// Apply a keymap for `slot` to every emulator config we can find.
pub fn apply(keymap_json: &str, slot: u8) {
    let Some(keymap) = parse_keymap(keymap_json) else {
        warn!("key_map: unparseable JSON keymap — emulator config unchanged");
        return;
    };
    if keymap.is_empty() {
        warn!("key_map: empty keymap — emulator config unchanged");
        return;
    }
    let player = slot + 1;
    info!(
        "player keyboard keymap for emulator P{player}: {} controls",
        keymap.len()
    );
    apply_pcsx2(&keymap, player);
    apply_rpcs3(&keymap, player);
}

// ---------------------------------------------------------------- PCSX2 -----

fn apply_pcsx2(keymap: &BTreeMap<String, String>, player: u8) {
    let Some(cfg) = find_pcsx2_config() else {
        info!("PCSX2 config not found — skipping (launch PCSX2 once, or set COUCHLINK_PCSX2_CONFIG)");
        return;
    };
    let raw = match fs::read(&cfg) {
        Ok(b) => b,
        Err(e) => {
            warn!("PCSX2 config unreadable: {e}");
            return;
        }
    };
    // Track which lines ended in \r so Windows CRLF files stay CRLF.
    let mut text = String::from_utf8_lossy(&raw).into_owned();
    let crlf = text.contains("\r\n");
    text = text.replace("\r\n", "\n");

    let section = format!("[Pad{player}]");
    let mut bindings = String::new();
    for (control, code) in keymap {
        let (Some(field), Some(names)) = (pcsx2_field(control), key_names(code)) else {
            continue;
        };
        bindings.push_str(&format!("{field} = Keyboard/{}\n", names.pcsx2));
    }
    if bindings.is_empty() {
        warn!("key_map: no PCSX2-keyable controls in keymap — Pad{player} unchanged");
        return;
    }

    let new_block = format!(
        "[{section}]\nType = DualShock2\nInvertL = 0\nInvertR = 0\nDeadzone = 0.00\nAxisScale = 1.33\n{bindings}"
    );

    // Drop the old [PadN] block, then append the fresh one.
    let mut lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<&str> = Vec::with_capacity(lines.len() + 8);
    let mut skip = false;
    for line in &lines {
        let trimmed = *line;
        if trimmed == section {
            skip = true;
            continue;
        }
        if skip && trimmed.starts_with('[') {
            skip = false;
        }
        if !skip {
            out.push(*line);
        }
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    let mut joined = out.join("\n");
    if !joined.is_empty() {
        joined.push('\n');
    }
    joined.push_str(&new_block);
    if crlf {
        joined = joined.replace('\n', "\r\n");
    }

    let mut existing = String::from_utf8_lossy(&raw).into_owned();
    if crlf {
        existing = existing.replace("\r\n", "\n");
    }
    if existing.contains(&section) && existing.contains(" = Keyboard/") {
        info!("PCSX2 {section} already carries keyboard bindings — left unchanged");
        return;
    }

    backup_once(&cfg);
    if let Err(e) = fs::write(&cfg, joined.as_bytes()) {
        warn!("PCSX2 config write failed: {e}");
        return;
    }
    info!("PCSX2 {section} bound to keyboard keys (backup: {}.couchlink.bak)", cfg.display());
}

// ----------------------------------------------------------------- RPCS3 -----

fn apply_rpcs3(keymap: &BTreeMap<String, String>, player: u8) {
    let Some(cfg) = find_rpcs3_config() else {
        info!("RPCS3 pad config not found — skipping (set COUCHLINK_RPCS3_CONFIG)");
        return;
    };
    let raw = match fs::read(&cfg) {
        Ok(b) => b,
        Err(e) => {
            warn!("RPCS3 config unreadable: {e}");
            return;
        }
    };
    let crlf = raw.windows(2).any(|w| w == b"\r\n");
    let mut text = String::from_utf8_lossy(&raw).into_owned();
    text = text.replace("\r\n", "\n");

    let header = format!("Player {player} Input:");
    let mut binds = String::from("  Handler: Keyboard\n  Device: \"\"\n");
    let mut any = false;
    for (control, code) in keymap {
        let (Some(key), Some(names)) = (rpcs3_key(control), key_names(code)) else {
            continue;
        };
        binds.push_str(&format!("  {key}: {}\n", names.sdl));
        any = true;
    }
    if !any {
        warn!("key_map: no RPCS3-keyable controls in keymap — Player {player} unchanged");
        return;
    }

    // Rebuild the file replacing the player's Input block, keeping CRLF.
    let mut lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<&str> = Vec::with_capacity(lines.len() + 16);
    let mut in_block = false;
    let mut replaced = false;
    for line in &lines {
        let t = *line;
        if t == header {
            in_block = true;
            replaced = true;
            for b in binds.lines() {
                out.push(b);
            }
            continue;
        }
        if in_block && (t.starts_with("Player ") && t.ends_with(" Input:")) {
            in_block = false;
        }
        if in_block && t.starts_with(' ') {
            // Absorb the old key lines under the replaced header.
            continue;
        }
        if in_block {
            // A non-indented line that is not another Player header ends the
            // block; emit it normally.
            in_block = false;
        }
        out.push(t);
    }
    if !replaced {
        warn!("RPCS3 config has no {header} block — leaving unchanged");
        return;
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    let mut joined = out.join("\n");
    joined.push('\n');
    if crlf {
        joined = joined.replace('\n', "\r\n");
    }

    let mut existing = String::from_utf8_lossy(&raw).into_owned();
    if crlf {
        existing = existing.replace("\r\n", "\n");
    }
    if existing.contains(&header) && existing.contains("  Handler: Keyboard") {
        info!("RPCS3 Player {player} already on Keyboard handler — left unchanged");
        return;
    }

    backup_once(&cfg);
    if let Err(e) = fs::write(&cfg, joined.as_bytes()) {
        warn!("RPCS3 config write failed: {e}");
        return;
    }
    info!(
        "RPCS3 {header} set to Keyboard (backup: {}.couchlink.bak)",
        cfg.display()
    );
}

/// First edit of a config takes a one-time backup, never clobbering one the
/// shell helper (`link-emulator-pad.sh`) already made.
fn backup_once(cfg: &PathBuf) {
    let bak = format!("{}.couchlink.bak", cfg.display());
    if !PathBuf::from(&bak).is_file() {
        if let Ok(b) = fs::read(cfg) {
            let _ = fs::write(&bak, b);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letter_codes_translate() {
        let k = key_names("KeyW").unwrap();
        assert_eq!(k.pcsx2, "Key_W");
        assert_eq!(k.sdl, "W");
        let k = key_names("Space").unwrap();
        assert_eq!(k.pcsx2, "Key_Space");
        assert_eq!(k.sdl, "Space");
    }

    #[test]
    fn arrow_and_shift_codes_translate() {
        let k = key_names("ArrowUp").unwrap();
        assert_eq!(k.pcsx2, "Key_Up");
        assert_eq!(k.sdl, "Up");
        let k = key_names("ShiftLeft").unwrap();
        assert_eq!(k.pcsx2, "Key_Shift");
        assert_eq!(k.sdl, "LShift");
        let k = key_names("ControlRight").unwrap();
        assert_eq!(k.sdl, "RControl");
    }

    #[test]
    fn unknown_codes_are_skipped() {
        assert!(key_names("Unidentified").is_none());
        assert!(key_names("MouseLeft").is_none());
        assert!(key_names("KeyAB").is_none());
    }

    #[test]
    fn control_fields_map() {
        assert_eq!(pcsx2_field("cross"), Some("Cross"));
        assert_eq!(pcsx2_field("options"), Some("Start"));
        assert_eq!(pcsx2_field("lstick_right"), Some("LRight"));
        assert_eq!(pcsx2_field("nonsense"), None);
        assert_eq!(rpcs3_key("dpad_up"), Some("Up"));
        assert_eq!(rpcs3_key("rstick_down"), Some("Right Stick Down"));
        assert_eq!(rpcs3_key("nonsense"), None);
    }

    #[test]
    fn parse_keymap_extracts_controls() {
        let m = parse_keymap(r#"{"cross":"Space","triangle":"KeyE"}"#).unwrap();
        assert_eq!(m.get("cross").map(String::as_str), Some("Space"));
        assert_eq!(m.get("triangle").map(String::as_str), Some("KeyE"));
        assert!(parse_keymap("not json").is_none());
        assert!(parse_keymap(r#"{"cross":42}"#).is_none());
        let empty = parse_keymap(r#"{"a":"b"}"#).unwrap();
        assert_eq!(empty.len(), 1);
    }

    #[test]
    fn apply_tolerates_missing_configs() {
        // No PCSX2/RPCS3 config exists in CI — apply must warn and return,
        // never panic.
        apply(r#"{"cross":"Space"}"#, 0);
    }
}
