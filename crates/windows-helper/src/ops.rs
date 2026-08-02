//! Dispatch allowlisted helper ops (runs installed PowerShell scripts on Windows).

use std::path::Path;

#[cfg(windows)]
use std::path::PathBuf;
#[cfg(windows)]
use std::process::Command;

use crate::protocol::{Request, Response};

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn handle_request(req: &Request, script_dir: &Path) -> Response {
    match req {
        Request::Ping => Response::ping_ok(VERSION),
        Request::OnlinePrep {
            skip_map,
            wsl_ip,
            signaling_port,
            turn_port,
        } => run_online_prep(
            script_dir,
            *skip_map,
            wsl_ip.as_deref(),
            *signaling_port,
            *turn_port,
        ),
        Request::FirewallUnblock => run_firewall_unblock(script_dir),
    }
}

fn run_online_prep(
    script_dir: &Path,
    skip_map: bool,
    wsl_ip: Option<&str>,
    signaling_port: u16,
    turn_port: u16,
) -> Response {
    #[cfg(not(windows))]
    {
        let _ = (script_dir, skip_map, wsl_ip, signaling_port, turn_port);
        return Response::err(Some("online_prep"), "windows only");
    }
    #[cfg(windows)]
    {
        let script = script_dir.join("enable-upnp.ps1");
        if !script.is_file() {
            return Response::err(
                Some("online_prep"),
                format!("missing script: {}", script.display()),
            );
        }
        let run_dir = helper_run_dir();
        let _ = std::fs::create_dir_all(&run_dir);
        let marker = run_dir.join("enable-upnp.exit");
        let _ = std::fs::remove_file(&marker);

        let mut args: Vec<String> = vec![
            "-NoProfile".into(),
            "-ExecutionPolicy".into(),
            "Bypass".into(),
            "-File".into(),
            script.to_string_lossy().into_owned(),
            "-RunDir".into(),
            run_dir.to_string_lossy().into_owned(),
            "-SignalingPort".into(),
            signaling_port.to_string(),
            "-TurnPort".into(),
            turn_port.to_string(),
        ];
        if skip_map {
            args.push("-SkipMap".into());
        }
        if let Some(ip) = wsl_ip {
            if !ip.is_empty() {
                args.push("-WslIp".into());
                args.push(ip.to_string());
            }
        }

        let status = match Command::new("powershell.exe").args(&args).status() {
            Ok(s) => s,
            Err(e) => {
                return Response::err(Some("online_prep"), format!("spawn powershell: {e}"));
            }
        };

        let exit = read_marker_exit(&marker).unwrap_or_else(|| status.code().unwrap_or(1));
        Response::ok_exit("online_prep", exit)
    }
}

fn run_firewall_unblock(script_dir: &Path) -> Response {
    #[cfg(not(windows))]
    {
        let _ = script_dir;
        return Response::err(Some("firewall_unblock"), "windows only");
    }
    #[cfg(windows)]
    {
        let script = script_dir.join("unblock-firewall.ps1");
        if !script.is_file() {
            return Response::err(
                Some("firewall_unblock"),
                format!("missing script: {}", script.display()),
            );
        }
        let run_dir = helper_run_dir();
        let _ = std::fs::create_dir_all(&run_dir);
        // unblock-firewall.ps1 writes to LOCALAPPDATA; SYSTEM uses ProgramData run dir
        // via env override if we set LOCALAPPDATA — keep simple: read process exit.
        let status = match Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                &script.to_string_lossy(),
            ])
            .status()
        {
            Ok(s) => s,
            Err(e) => {
                return Response::err(
                    Some("firewall_unblock"),
                    format!("spawn powershell: {e}"),
                );
            }
        };
        let exit = status.code().unwrap_or(1);
        Response::ok_exit("firewall_unblock", exit)
    }
}

#[cfg(windows)]
fn helper_run_dir() -> PathBuf {
    std::env::var_os("PROGRAMDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join("Couchlink")
        .join("run")
}

#[cfg(windows)]
fn read_marker_exit(path: &Path) -> Option<i32> {
    let s = std::fs::read_to_string(path).ok()?;
    s.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn ping_ok() {
        let resp = handle_request(&Request::Ping, Path::new("."));
        assert!(resp.ok);
        assert_eq!(resp.version.as_deref(), Some(VERSION));
        assert_eq!(resp.op.as_deref(), Some("ping"));
    }

    #[test]
    #[cfg(not(windows))]
    fn online_prep_windows_only() {
        let resp = handle_request(
            &Request::OnlinePrep {
                skip_map: true,
                wsl_ip: None,
                signaling_port: 8443,
                turn_port: 3478,
            },
            Path::new("."),
        );
        assert!(!resp.ok);
        assert_eq!(resp.error.as_deref(), Some("windows only"));
    }
}
