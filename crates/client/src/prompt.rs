//! Interactive join prompts (terminal stdin + OS dialogs for GUI launches).

use anyhow::{bail, Context, Result};
use std::io::{self, BufRead, IsTerminal, Write};
use std::process::Command;

#[derive(Debug, Clone, Default)]
pub struct PromptedJoin {
    pub join_url: Option<String>,
    pub session_id: Option<String>,
    pub pin: Option<String>,
    pub signaling: Option<String>,
    pub turn_url: Option<String>,
    pub turn_user: Option<String>,
    pub turn_pass: Option<String>,
}

/// Ask for a join link (and optionally session/PIN/TURN if the link is skipped).
///
/// `prefer_gui`: true for the native desktop window path — try an OS dialog first
/// so double-click launches work without a console. Falls back to stdin when a TTY
/// is attached (e.g. `./scripts/run.sh client`).
pub fn prompt_join(prefill_url: Option<&str>, prefer_gui: bool) -> Result<PromptedJoin> {
    let prefill = prefill_url.unwrap_or("").trim();

    if prefer_gui {
        if let Ok(Some(url)) = prompt_gui_join_url(prefill) {
            return Ok(PromptedJoin {
                join_url: Some(url),
                ..Default::default()
            });
        }
    }

    if io::stdin().is_terminal() {
        return prompt_stdin(prefill);
    }

    if !prefer_gui {
        if let Ok(Some(url)) = prompt_gui_join_url(prefill) {
            return Ok(PromptedJoin {
                join_url: Some(url),
                ..Default::default()
            });
        }
    }

    bail!(
        "no join credentials and no interactive terminal — pass --join-url, \
         set COUCHLINK_JOIN_URL, or launch from a terminal / desktop shortcut"
    )
}

fn prompt_stdin(prefill: &str) -> Result<PromptedJoin> {
    let mut stdout = io::stdout();
    writeln!(stdout, "\n=== Couchlink — join a session ===")?;
    writeln!(
        stdout,
        "Paste the full join URL from the host (http://100.x… Tailscale, or the link they sent)."
    )?;
    if !prefill.is_empty() {
        writeln!(stdout, "Current: {prefill}")?;
        write!(stdout, "Join URL [Enter keeps current]: ")?;
    } else {
        write!(stdout, "Join URL: ")?;
    }
    stdout.flush()?;

    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    let entered = line.trim();
    let url = if entered.is_empty() {
        prefill.to_string()
    } else {
        entered.to_string()
    };

    if !url.is_empty() {
        return Ok(PromptedJoin {
            join_url: Some(url),
            ..Default::default()
        });
    }

    writeln!(stdout, "\nEnter session credentials:")?;
    let session_id = read_required("Session ID")?;
    let pin = read_required("PIN")?;
    let signaling = read_optional("Signaling URL", "ws://127.0.0.1:8443/ws")?;
    let turn_url = read_optional("TURN URL (optional, from join link turn=)", "")?;
    let turn_user = if turn_url.is_empty() {
        String::new()
    } else {
        read_optional("TURN user (turnu=)", "")?
    };
    let turn_pass = if turn_url.is_empty() {
        String::new()
    } else {
        read_optional("TURN password (turnp=)", "")?
    };

    Ok(PromptedJoin {
        join_url: None,
        session_id: Some(session_id),
        pin: Some(pin),
        signaling: (!signaling.is_empty()).then_some(signaling),
        turn_url: (!turn_url.is_empty()).then_some(turn_url),
        turn_user: (!turn_user.is_empty()).then_some(turn_user),
        turn_pass: (!turn_pass.is_empty()).then_some(turn_pass),
    })
}

fn read_required(label: &str) -> Result<String> {
    loop {
        write!(io::stdout(), "{label}: ")?;
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;
        let v = line.trim().to_string();
        if !v.is_empty() {
            return Ok(v);
        }
        writeln!(io::stdout(), "(required)")?;
    }
}

fn read_optional(label: &str, default: &str) -> Result<String> {
    if default.is_empty() {
        write!(io::stdout(), "{label}: ")?;
    } else {
        write!(io::stdout(), "{label} [{default}]: ")?;
    }
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    let v = line.trim();
    Ok(if v.is_empty() {
        default.to_string()
    } else {
        v.to_string()
    })
}

fn prompt_gui_join_url(prefill: &str) -> Result<Option<String>> {
    #[cfg(target_os = "windows")]
    {
        return windows_input_box(prefill);
    }
    #[cfg(target_os = "macos")]
    {
        return macos_input_box(prefill);
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        return linux_input_box(prefill);
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = prefill;
        Ok(None)
    }
}

#[cfg(target_os = "windows")]
fn windows_input_box(prefill: &str) -> Result<Option<String>> {
    let prefill_ps = escape_ps(prefill);
    let script = format!(
        "Add-Type -AssemblyName Microsoft.VisualBasic; \
         [Microsoft.VisualBasic.Interaction]::InputBox(\
           'Paste the full join link from the host (includes turn= for online play):',\
           'Couchlink Player',\
           '{prefill_ps}')"
    );
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .context("powershell InputBox")?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout)
        .trim()
        .trim_matches('\r')
        .to_string();
    if text.is_empty() {
        Ok(None)
    } else {
        Ok(Some(text))
    }
}

#[cfg(target_os = "windows")]
fn escape_ps(s: &str) -> String {
    s.replace('\'', "''")
}

#[cfg(target_os = "macos")]
fn macos_input_box(prefill: &str) -> Result<Option<String>> {
    let prefill_esc = prefill.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        r#"try
  text returned of (display dialog "Paste the full join link from the host:" default answer "{prefill_esc}" with title "Couchlink Player" buttons {{"Cancel", "Join"}} default button "Join")
on error
  ""
end try"#
    );
    let output = Command::new("osascript")
        .args(["-e", &script])
        .output()
        .context("osascript dialog")?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        Ok(None)
    } else {
        Ok(Some(text))
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn linux_input_box(prefill: &str) -> Result<Option<String>> {
    // Prefer zenity, then kdialog, then (on WSL) Windows InputBox.
    if let Some(url) = try_zenity(prefill)? {
        return Ok(Some(url));
    }
    if let Some(url) = try_kdialog(prefill)? {
        return Ok(Some(url));
    }
    if crate::reachability::is_wsl() {
        return wsl_windows_input_box(prefill);
    }
    Ok(None)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn try_zenity(prefill: &str) -> Result<Option<String>> {
    if Command::new("zenity").arg("--version").output().is_err() {
        return Ok(None);
    }
    let output = Command::new("zenity")
        .args([
            "--entry",
            "--title=Couchlink Player",
            "--text=Paste the full join link from the host:",
            "--width=520",
            &format!("--entry-text={prefill}"),
        ])
        .output()
        .context("zenity")?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!text.is_empty()).then_some(text))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn try_kdialog(prefill: &str) -> Result<Option<String>> {
    if Command::new("kdialog").arg("--version").output().is_err() {
        return Ok(None);
    }
    let output = Command::new("kdialog")
        .args([
            "--title",
            "Couchlink Player",
            "--inputbox",
            "Paste the full join link from the host:",
            prefill,
        ])
        .output()
        .context("kdialog")?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!text.is_empty()).then_some(text))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn wsl_windows_input_box(prefill: &str) -> Result<Option<String>> {
    let prefill_ps = prefill.replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName Microsoft.VisualBasic; \
         [Microsoft.VisualBasic.Interaction]::InputBox(\
           'Paste the full join link from the host (includes turn= for online play):',\
           'Couchlink Player',\
           '{prefill_ps}')"
    );
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .context("powershell InputBox from WSL")?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout)
        .trim()
        .trim_matches('\r')
        .to_string();
    Ok((!text.is_empty()).then_some(text))
}
