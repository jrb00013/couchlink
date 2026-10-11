//! CouchLink Host Control Panel (egui) — stack scripts + split-seat isolation.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;
use serde::{Deserialize, Serialize};

const DEFAULT_CTRL: &str = "127.0.0.1:18765";

fn repo_root() -> anyhow::Result<PathBuf> {
    if let Ok(root) = std::env::var("COUCHLINK_ROOT") {
        let p = PathBuf::from(root);
        if p.join("scripts").is_dir() {
            return Ok(p);
        }
        anyhow::bail!("COUCHLINK_ROOT has no scripts/ directory");
    }
    let mut dir = std::env::current_dir()?;
    for _ in 0..8 {
        if dir.join("scripts").is_dir() {
            return Ok(dir);
        }
        if !dir.pop() {
            break;
        }
    }
    let exe = std::env::current_exe()?;
    let mut dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    for _ in 0..8 {
        if dir.join("scripts").is_dir() {
            return Ok(dir);
        }
        if !dir.pop() {
            break;
        }
    }
    anyhow::bail!("could not locate couchlink repo root — set COUCHLINK_ROOT")
}

fn ctrl_base() -> String {
    std::env::var("COUCHLINK_SPLIT_SEAT_CTRL").unwrap_or_else(|_| DEFAULT_CTRL.into())
}

fn ctrl_url(path: &str) -> String {
    let base = ctrl_base();
    let base = base.trim_end_matches('/');
    if base.starts_with("http://") || base.starts_with("https://") {
        format!("{base}{path}")
    } else {
        format!("http://{base}{path}")
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct SplitSeatState {
    master_enabled: bool,
    host_view_mode: String,
    layout: String,
    regions: Vec<NormRect>,
    seat_region: Vec<u8>,
    rescan: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct NormRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[derive(Clone)]
struct ScriptJob {
    label: String,
    script: PathBuf,
    args: Vec<String>,
}

enum WorkerMsg {
    ScriptDone {
        label: String,
        ok: bool,
        output: String,
    },
    SplitState(SplitSeatState),
    SplitErr(String),
}

struct HostPanelApp {
    root: PathBuf,
    root_err: Option<String>,
    game_preset: GamePreset,
    custom_needle: String,
    hyperv: bool,
    script_log: String,
    script_busy: bool,
    pending_restart: bool,
    split: Option<SplitSeatState>,
    split_err: Option<String>,
    split_poll_at: Instant,
    master_enabled: bool,
    host_view_mode: HostViewMode,
    tx: mpsc::Sender<WorkerMsg>,
    rx: mpsc::Receiver<WorkerMsg>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GamePreset {
    Mk,
    Bo2,
    Custom,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HostViewMode {
    Own,
    Full,
}

impl Default for HostPanelApp {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        let root_result = repo_root();
        Self {
            root: root_result.as_ref().ok().cloned().unwrap_or_default(),
            root_err: root_result.err().map(|e| e.to_string()),
            game_preset: GamePreset::Mk,
            custom_needle: String::new(),
            hyperv: false,
            script_log: String::new(),
            script_busy: false,
            pending_restart: false,
            split: None,
            split_err: None,
            split_poll_at: Instant::now(),
            master_enabled: false,
            host_view_mode: HostViewMode::Full,
            tx,
            rx,
        }
    }
}

impl HostPanelApp {
    fn env_couchlink_path(&self) -> PathBuf {
        self.root.join(".env.couchlink")
    }

    fn game_needle(&self) -> String {
        match self.game_preset {
            GamePreset::Mk => "mk".into(),
            GamePreset::Bo2 => "bo2".into(),
            GamePreset::Custom => {
                let n = self.custom_needle.trim();
                if n.is_empty() {
                    "custom".into()
                } else {
                    n.to_string()
                }
            }
        }
    }

    fn spawn_script(&mut self, job: ScriptJob) {
        if self.script_busy {
            return;
        }
        self.script_busy = true;
        self.script_log = format!("==> running {} …\n", job.label);
        let tx = self.tx.clone();
        let root = self.root.clone();
        thread::spawn(move || {
            let mut cmd = Command::new("bash");
            cmd.arg(&job.script);
            for a in &job.args {
                cmd.arg(a);
            }
            cmd.current_dir(&root);
            cmd.env("COUCHLINK_ROOT", &root);
            let out = cmd.output();
            let (ok, output) = match out {
                Ok(o) => {
                    let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
                    s.push_str(&String::from_utf8_lossy(&o.stderr));
                    (o.status.success(), s)
                }
                Err(e) => (false, format!("spawn failed: {e}")),
            };
            let _ = tx.send(WorkerMsg::ScriptDone {
                label: job.label,
                ok,
                output,
            });
        });
    }

    fn poll_split_seat(&mut self) {
        if self.split_poll_at.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.split_poll_at = Instant::now();
        let url = ctrl_url("/v1/split-seat");
        let tx = self.tx.clone();
        thread::spawn(move || {
            match ureq::get(&url).call() {
                Ok(resp) => match resp.into_string() {
                    Ok(body) => match serde_json::from_str::<SplitSeatState>(&body) {
                        Ok(s) => {
                            let _ = tx.send(WorkerMsg::SplitState(s));
                        }
                        Err(e) => {
                            let _ = tx.send(WorkerMsg::SplitErr(format!("json: {e}")));
                        }
                    },
                    Err(e) => {
                        let _ = tx.send(WorkerMsg::SplitErr(format!("read: {e}")));
                    }
                },
                Err(e) => {
                    let _ = tx.send(WorkerMsg::SplitErr(format!("GET split-seat: {e}")));
                }
            }
        });
    }

    fn post_split_seat(&mut self, patch: serde_json::Value) {
        let url = ctrl_url("/v1/split-seat");
        let tx = self.tx.clone();
        let body = patch.to_string();
        thread::spawn(move || {
            match ureq::post(&url)
                .set("Content-Type", "application/json")
                .send_string(&body)
            {
                Ok(resp) => match resp.into_string() {
                    Ok(text) => match serde_json::from_str::<SplitSeatState>(&text) {
                        Ok(s) => {
                            let _ = tx.send(WorkerMsg::SplitState(s));
                        }
                        Err(e) => {
                            let _ = tx.send(WorkerMsg::SplitErr(format!("post json: {e}")));
                        }
                    },
                    Err(e) => {
                        let _ = tx.send(WorkerMsg::SplitErr(format!("post read: {e}")));
                    }
                },
                Err(e) => {
                    let _ = tx.send(WorkerMsg::SplitErr(format!("POST split-seat: {e}")));
                }
            }
        });
    }

    fn sync_controls_from_split(&mut self, s: &SplitSeatState) {
        self.master_enabled = s.master_enabled;
        self.host_view_mode = if s.host_view_mode == "own" {
            HostViewMode::Own
        } else {
            HostViewMode::Full
        };
    }

    fn drain_worker(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                WorkerMsg::ScriptDone { label, ok, output } => {
                    self.script_busy = false;
                    self.script_log.push_str(&format!(
                        "\n--- {label} {} ---\n{output}\n",
                        if ok { "ok" } else { "FAILED" }
                    ));
                }
                WorkerMsg::SplitState(s) => {
                    self.split_err = None;
                    self.sync_controls_from_split(&s);
                    self.split = Some(s);
                }
                WorkerMsg::SplitErr(e) => self.split_err = Some(e),
            }
        }
    }

    fn copy_join_url(&mut self) {
        let script = self.root.join("scripts/copy-join-url.sh");
        self.spawn_script(ScriptJob {
            label: "copy-join-url".into(),
            script,
            args: vec![],
        });
        if let Ok(mut cb) = arboard::Clipboard::new() {
            if let Ok(text) = std::fs::read_to_string("/tmp/couchlink-join-url.txt") {
                let t = text.trim();
                if !t.is_empty() {
                    let _ = cb.set_text(t.to_string());
                }
            }
        }
    }

    fn ensure_stack(&mut self) {
        let script = self.root.join("scripts/ensure-host-stack.sh");
        self.spawn_script(ScriptJob {
            label: "ensure-host-stack".into(),
            script,
            args: vec![],
        });
    }

    fn run_restart(&mut self) {
        let script = self.root.join("scripts/restart-stack-for-game.sh");
        let mut args = vec![self.game_needle()];
        if self.hyperv {
            args.push("--hyperv".into());
        }
        self.spawn_script(ScriptJob {
            label: "restart-stack-for-game".into(),
            script,
            args,
        });
    }

    fn open_env_hint(&self) {
        let path = self.env_couchlink_path();
        let _ = if path.is_file() {
            open_path(&path)
        } else {
            open_path(&self.root)
        };
    }
}

fn is_wsl() -> bool {
    std::fs::read_to_string("/proc/version")
        .map(|v| v.to_lowercase().contains("microsoft"))
        .unwrap_or(false)
}

fn open_path(path: &Path) -> std::io::Result<()> {
    if is_wsl() {
        let win = path.to_string_lossy().replace('/', "\\");
        Command::new("explorer.exe")
            .arg(format!("{win}"))
            .spawn()
            .map(|_| ())
    } else if cfg!(target_os = "linux") {
        Command::new("xdg-open").arg(path).spawn().map(|_| ())
    } else {
        Ok(())
    }
}

impl eframe::App for HostPanelApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_worker();
        self.poll_split_seat();
        ctx.request_repaint_after(Duration::from_millis(500));

        egui::TopBottomPanel::top("safety_banner").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(
                    egui::Color32::from_rgb(200, 120, 40),
                    "Session safety: never restart the live CouchLink/RPCS3 stack without host OK.",
                );
                if ui.button("I understand — show restart").clicked() {
                    self.pending_restart = true;
                }
            });
        });

        if self.pending_restart {
            egui::Window::new("Confirm stack restart")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("This stops couchlink-host, signaling, capture, and cloudflared.");
                    ui.label("Friends will need the NEW join URL after restart.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.pending_restart = false;
                        }
                        if ui
                            .add_enabled(!self.script_busy, egui::Button::new("Restart stack"))
                            .clicked()
                        {
                            self.pending_restart = false;
                            self.run_restart();
                        }
                    });
                });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("CouchLink Host Control Panel");
            ui.add_space(4.0);

            if let Some(e) = &self.root_err {
                ui.colored_label(egui::Color32::RED, e);
            } else {
                ui.label(format!("COUCHLINK_ROOT: {}", self.root.display()));
                ui.label(format!(
                    ".env.couchlink: {}",
                    self.env_couchlink_path().display()
                ));
                if ui.button("Open .env.couchlink location").clicked() {
                    self.open_env_hint();
                }
            }

            ui.separator();
            ui.heading("Stack");
            ui.horizontal(|ui| {
                ui.label("Game:");
                ui.selectable_value(&mut self.game_preset, GamePreset::Mk, "mk");
                ui.selectable_value(&mut self.game_preset, GamePreset::Bo2, "bo2");
                ui.selectable_value(&mut self.game_preset, GamePreset::Custom, "custom");
            });
            if self.game_preset == GamePreset::Custom {
                ui.text_edit_singleline(&mut self.custom_needle);
            }
            ui.checkbox(&mut self.hyperv, "Use --hyperv transport");

            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.script_busy, egui::Button::new("Ensure stack"))
                    .clicked()
                {
                    self.ensure_stack();
                }
                if ui
                    .add_enabled(!self.script_busy, egui::Button::new("Copy join URL"))
                    .clicked()
                {
                    self.copy_join_url();
                }
                if ui
                    .add_enabled(!self.script_busy, egui::Button::new("Restart stack…"))
                    .clicked()
                {
                    self.pending_restart = true;
                }
            });

            ui.separator();
            ui.heading("Split-seat isolation");
            ui.label(format!("Control: {}", ctrl_url("")));
            if let Some(e) = &self.split_err {
                ui.colored_label(egui::Color32::YELLOW, e);
            }

            if ui
                .checkbox(&mut self.master_enabled, "Master enable (isolation)")
                .changed()
            {
                self.post_split_seat(serde_json::json!({
                    "master_enabled": self.master_enabled,
                }));
            }

            ui.horizontal(|ui| {
                ui.label("Host view:");
                if ui
                    .selectable_value(&mut self.host_view_mode, HostViewMode::Own, "own")
                    .clicked()
                {
                    self.post_split_seat(serde_json::json!({ "host_view_mode": "own" }));
                }
                if ui
                    .selectable_value(&mut self.host_view_mode, HostViewMode::Full, "full")
                    .clicked()
                {
                    self.post_split_seat(serde_json::json!({ "host_view_mode": "full" }));
                }
            });

            if ui.button("Rescan layout").clicked() {
                self.post_split_seat(serde_json::json!({ "rescan": true }));
            }

            ui.separator();
            ui.heading("Roster / last split state");
            if let Some(s) = &self.split {
                ui.label(format!(
                    "layout={} regions={} seat_region={:?} rescan={}",
                    s.layout,
                    s.regions.len(),
                    s.seat_region,
                    s.rescan
                ));
                for (i, r) in s.regions.iter().enumerate() {
                    ui.label(format!(
                        "  region {i}: x={:.3} y={:.3} w={:.3} h={:.3}",
                        r.x, r.y, r.w, r.h
                    ));
                }
                ui.label("(Player roster UI placeholder — seat slots follow seat_region above.)");
            } else {
                ui.label("No split-seat state yet (is couchlink-host running?)");
            }

            ui.separator();
            ui.heading("Script output");
            if self.script_busy {
                ui.spinner();
            }
            egui::ScrollArea::vertical()
                .max_height(240.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.script_log)
                            .desired_width(f32::INFINITY)
                            .font(egui::TextStyle::Monospace),
                    );
                });
        });
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([920.0, 720.0]),
        ..Default::default()
    };
    eframe::run_native(
        "CouchLink Host Panel",
        options,
        Box::new(|_cc| Ok(Box::new(HostPanelApp::default()))),
    )
}
