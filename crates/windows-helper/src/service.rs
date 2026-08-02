//! Windows service registration and service main.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use windows_service::service::{
    ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

use crate::pipe_server;

pub const SERVICE_NAME: &str = "CouchlinkHelper";
pub const SERVICE_DISPLAY: &str = "Couchlink Helper";
pub const INSTALL_DIR: &str = r"C:\Program Files\Couchlink\Helper";
const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

const SCRIPT_NAMES: &[&str] = &["enable-upnp.ps1", "unblock-firewall.ps1", "call-helper.ps1"];

define_windows_service!(ffi_service_main, service_main);

pub fn run_service_dispatcher() -> Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .context("service_dispatcher::start")?;
    Ok(())
}

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run_service() {
        eprintln!("couchlink-helper service error: {e:#}");
    }
}

fn run_service() -> Result<()> {
    let (shutdown_tx, shutdown_rx) = mpsc::channel();

    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                let _ = shutdown_tx.send(());
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle =
        service_control_handler::register(SERVICE_NAME, event_handler).context("register")?;

    status_handle
        .set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Running,
            controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })
        .context("set Running")?;

    let script_dir = script_dir_from_exe();
    let pipe_thread = std::thread::spawn(move || {
        if let Err(e) = pipe_server::serve_pipe(&script_dir) {
            eprintln!("pipe server stopped: {e:#}");
        }
    });

    let _ = shutdown_rx.recv();

    status_handle
        .set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })
        .ok();

    let _ = pipe_thread;
    Ok(())
}

fn script_dir_from_exe() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from(INSTALL_DIR))
}

/// Copy helper exe + PowerShell scripts into Program Files, then register the service.
pub fn install_service(script_source: Option<&Path>) -> Result<()> {
    let install_dir = PathBuf::from(INSTALL_DIR);
    fs::create_dir_all(&install_dir)
        .with_context(|| format!("create {}", install_dir.display()))?;

    let src_exe = std::env::current_exe().context("current_exe")?;
    let dest_exe = install_dir.join("couchlink-helper.exe");
    fs::copy(&src_exe, &dest_exe)
        .with_context(|| format!("copy {} → {}", src_exe.display(), dest_exe.display()))?;
    println!("OK staged {}", dest_exe.display());

    let script_src = resolve_script_source(script_source, &src_exe)?;
    for name in SCRIPT_NAMES {
        let from = script_src.join(name);
        if !from.is_file() {
            bail!("missing required script {} (looked in {})", name, script_src.display());
        }
        let to = install_dir.join(name);
        fs::copy(&from, &to).with_context(|| format!("copy {}", name))?;
        println!("OK staged {}", to.display());
    }

    // Replace any previous registration (may have pointed at a WSL path).
    let _ = uninstall_service_quiet();

    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .context("OpenSCManager")?;

    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(SERVICE_DISPLAY),
        service_type: SERVICE_TYPE,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: dest_exe.clone(),
        launch_arguments: vec![OsString::from("service")],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };

    let svc = manager
        .create_service(&info, ServiceAccess::START | ServiceAccess::CHANGE_CONFIG)
        .context("create_service")?;
    svc.start::<OsString>(&[])
        .context("start service")?;
    println!("OK installed and started service {SERVICE_NAME}");
    println!("    binaries: {}", install_dir.display());
    write_install_marker(0);
    Ok(())
}

fn write_install_marker(code: i32) {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let dir = PathBuf::from(local).join("couchlink-run");
        let _ = fs::create_dir_all(&dir);
        let _ = fs::write(dir.join("helper-install.exit"), format!("{code}"));
    }
    // Also under Public in case elevated LOCALAPPDATA differs.
    let public = PathBuf::from(r"C:\ProgramData\Couchlink\run");
    let _ = fs::create_dir_all(&public);
    let _ = fs::write(public.join("helper-install.exit"), format!("{code}"));
}

fn resolve_script_source(explicit: Option<&Path>, src_exe: &Path) -> Result<PathBuf> {
    if let Some(p) = explicit {
        if p.is_dir() {
            return Ok(p.to_path_buf());
        }
        bail!("--script-dir is not a directory: {}", p.display());
    }
    // Next to the running exe (dev build / Inno layout).
    if let Some(parent) = src_exe.parent() {
        if parent.join("enable-upnp.ps1").is_file() {
            return Ok(parent.to_path_buf());
        }
        // cargo target\release → repo\scripts\windows
        let candidate = parent
            .join("..")
            .join("..")
            .join("scripts")
            .join("windows");
        if let Ok(c) = fs::canonicalize(&candidate) {
            if c.join("enable-upnp.ps1").is_file() {
                return Ok(c);
            }
        }
    }
    // Already installed.
    let installed = PathBuf::from(INSTALL_DIR);
    if installed.join("enable-upnp.ps1").is_file() {
        return Ok(installed);
    }
    bail!(
        "could not find enable-upnp.ps1; pass --script-dir path\\to\\scripts\\windows"
    );
}

fn uninstall_service_quiet() -> Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("OpenSCManager")?;
    let service = match manager.open_service(
        SERVICE_NAME,
        ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS,
    ) {
        Ok(s) => s,
        Err(_) => return Ok(()),
    };
    let _ = service.stop();
    for _ in 0..25 {
        if let Ok(st) = service.query_status() {
            if st.current_state == ServiceState::Stopped {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = service.delete();
    // SCM needs a beat after delete before recreate.
    std::thread::sleep(Duration::from_millis(500));
    Ok(())
}

pub fn uninstall_service() -> Result<()> {
    uninstall_service_quiet()?;
    println!("OK uninstalled service {SERVICE_NAME}");
    Ok(())
}
