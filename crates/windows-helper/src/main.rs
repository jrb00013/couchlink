//! couchlink-helper — Windows privileged helper (service + pipe).

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use couchlink_windows_helper::ops::handle_request;
use couchlink_windows_helper::protocol::Request;

#[derive(Parser, Debug)]
#[command(name = "couchlink-helper", about = "Couchlink Windows privileged helper")]
struct Cli {
    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Respond to a local ping without the pipe (sanity check).
    Ping,
    /// Foreground named-pipe server (dev / debugging).
    Run {
        #[arg(long)]
        script_dir: Option<PathBuf>,
    },
    /// Windows service entry (used by SCM).
    Service,
    /// Register + start the Windows service (requires elevation).
    Install,
    /// Stop + delete the Windows service (requires elevation).
    Uninstall,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Commands::Ping => {
            let resp = handle_request(&Request::Ping, std::path::Path::new("."));
            println!("{}", serde_json::to_string(&resp)?);
            Ok(())
        }
        Commands::Run { script_dir } => {
            #[cfg(windows)]
            {
                let dir = script_dir.unwrap_or_else(default_script_dir);
                eprintln!(
                    "couchlink-helper: serving {} (scripts={})",
                    couchlink_windows_helper::pipe_server::PIPE_NAME,
                    dir.display()
                );
                couchlink_windows_helper::pipe_server::serve_pipe(&dir)
            }
            #[cfg(not(windows))]
            {
                let _ = script_dir;
                anyhow::bail!("couchlink-helper run is Windows-only");
            }
        }
        Commands::Service => {
            #[cfg(windows)]
            {
                couchlink_windows_helper::service::run_service_dispatcher()
            }
            #[cfg(not(windows))]
            {
                anyhow::bail!("couchlink-helper service is Windows-only");
            }
        }
        Commands::Install => {
            #[cfg(windows)]
            {
                couchlink_windows_helper::service::install_service()
            }
            #[cfg(not(windows))]
            {
                anyhow::bail!("couchlink-helper install is Windows-only");
            }
        }
        Commands::Uninstall => {
            #[cfg(windows)]
            {
                couchlink_windows_helper::service::uninstall_service()
            }
            #[cfg(not(windows))]
            {
                anyhow::bail!("couchlink-helper uninstall is Windows-only");
            }
        }
    }
}

#[cfg(windows)]
fn default_script_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files\Couchlink\Helper"))
}
