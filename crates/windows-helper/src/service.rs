//! Windows service registration and service main.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result};
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
const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

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

    // Pipe loop does not exit cleanly; process stop ends the service.
    let _ = pipe_thread;
    Ok(())
}

fn script_dir_from_exe() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files\Couchlink\Helper"))
}

pub fn install_service() -> Result<()> {
    let manager =
        ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)
            .context("OpenSCManager")?;

    let exe = std::env::current_exe().context("current_exe")?;

    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(SERVICE_DISPLAY),
        service_type: SERVICE_TYPE,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments: vec![OsString::from("service")],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };

    match manager.create_service(&info, ServiceAccess::START | ServiceAccess::CHANGE_CONFIG) {
        Ok(svc) => {
            let _ = svc.start::<OsString>(&[]);
            println!("OK installed and started service {SERVICE_NAME}");
        }
        Err(e) => {
            // Already exists — try start.
            let svc = manager
                .open_service(SERVICE_NAME, ServiceAccess::START)
                .with_context(|| format!("create failed ({e}); open existing"))?;
            let _ = svc.start::<OsString>(&[]);
            println!("OK service {SERVICE_NAME} already present; start requested");
        }
    }
    Ok(())
}

pub fn uninstall_service() -> Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("OpenSCManager")?;
    let service = manager
        .open_service(
            SERVICE_NAME,
            ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS,
        )
        .context("open service")?;

    let _ = service.stop();
    // Wait briefly for stop.
    for _ in 0..20 {
        if let Ok(st) = service.query_status() {
            if st.current_state == ServiceState::Stopped {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    service.delete().context("delete service")?;
    println!("OK uninstalled service {SERVICE_NAME}");
    Ok(())
}
