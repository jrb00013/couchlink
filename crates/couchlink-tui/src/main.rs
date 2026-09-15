//! Terminal control panel for a running couchlink host.
//!
//! Built for headless/SSH use: no browser, no clicking through PowerShell
//! windows — just a terminal. Shows host/signaling/capture status, lists
//! live Windows windows, and lets you switch the capture target or force a
//! capture restart, all by shelling out to the same scripts the host itself
//! uses (`scripts/ensure-win-capture.sh`), so there is exactly one code path
//! for "start/switch capture" whether it's triggered by this TUI, the host's
//! own self-heal, or a human running the script by hand.

use std::io;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, ExecutableCommand};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

fn repo_root() -> Result<std::path::PathBuf> {
    if let Ok(root) = std::env::var("COUCHLINK_ROOT") {
        return Ok(std::path::PathBuf::from(root));
    }
    // Fall back to walking up from the exe location; installed binaries
    // (~/.local/bin) won't find a repo, so COUCHLINK_ROOT is the normal path.
    let exe = std::env::current_exe()?;
    let mut dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    for _ in 0..6 {
        if dir.join("scripts/ensure-win-capture.sh").is_file() {
            return Ok(dir);
        }
        if !dir.pop() {
            break;
        }
    }
    anyhow::bail!("could not locate couchlink repo root — set COUCHLINK_ROOT")
}

fn is_wsl() -> bool {
    std::fs::read_to_string("/proc/version")
        .map(|v| v.to_lowercase().contains("microsoft"))
        .unwrap_or(false)
}

fn run_capture(cmd: &str, args: &[&str]) -> Result<(bool, String)> {
    let out = Command::new(cmd).args(args).output()?;
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), s))
}

#[derive(Clone)]
struct WinInfo {
    title: String,
}

fn list_windows() -> Vec<WinInfo> {
    if !is_wsl() {
        return Vec::new();
    }
    // Same EnumWindows approach used ad hoc from the shell: every visible
    // top-level window with a non-empty title.
    let script = r#"
Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class W {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);
  [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder text, int count);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
  public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
}
'@
$titles = New-Object System.Collections.Generic.List[string]
[W]::EnumWindows({ param($h,$l)
  if ([W]::IsWindowVisible($h)) {
    $sb = New-Object System.Text.StringBuilder 256
    [W]::GetWindowText($h,$sb,256) | Out-Null
    if ($sb.Length -gt 0) { $titles.Add($sb.ToString()) }
  }
  $true
}, [IntPtr]::Zero) | Out-Null
$titles
"#;
    match run_capture("powershell.exe", &["-NoProfile", "-Command", script]) {
        Ok((true, out)) => out
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .map(|l| WinInfo { title: l.to_string() })
            .collect(),
        _ => Vec::new(),
    }
}

fn capture_running() -> Option<String> {
    if !is_wsl() {
        return None;
    }
    let (ok, out) = run_capture(
        "powershell.exe",
        &[
            "-NoProfile",
            "-Command",
            "(Get-CimInstance Win32_Process -Filter \"Name='couchlink-win-capture.exe'\").CommandLine",
        ],
    )
    .ok()?;
    if !ok {
        return None;
    }
    let cmdline = out.trim();
    if cmdline.is_empty() {
        None
    } else {
        Some(cmdline.to_string())
    }
}

fn proc_alive(needle: &str) -> bool {
    Command::new("pgrep")
        .args(["-f", needle])
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false)
}

fn switch_capture_window(root: &std::path::Path, title: &str) -> Result<(bool, String)> {
    let script = root.join("scripts/ensure-win-capture.sh");
    let out = Command::new("bash")
        .arg(&script)
        .env("COUCHLINK_WIN_CAPTURE_FORCE", "1")
        .env("COUCHLINK_CAPTURE_WINDOW", title)
        .env("COUCHLINK_CAPTURE_SOURCE", "window")
        .env("COUCHLINK_SKIP_WIN_CAPTURE_BUILD", "1")
        .output()?;
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), s))
}

fn restart_capture(root: &std::path::Path) -> Result<(bool, String)> {
    let script = root.join("scripts/ensure-win-capture.sh");
    let out = Command::new("bash")
        .arg(&script)
        .env("COUCHLINK_WIN_CAPTURE_FORCE", "1")
        .env("COUCHLINK_SKIP_WIN_CAPTURE_BUILD", "1")
        .output()?;
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), s))
}

struct App {
    root: std::path::PathBuf,
    windows: Vec<WinInfo>,
    filter: String,
    list_state: ListState,
    log: Vec<String>,
    last_refresh: Instant,
    capture_cmdline: Option<String>,
    host_alive: bool,
    signaling_alive: bool,
}

impl App {
    fn new(root: std::path::PathBuf) -> Self {
        let mut s = Self {
            root,
            windows: Vec::new(),
            filter: String::new(),
            list_state: ListState::default(),
            log: vec!["couchlink-tui ready — j/k or arrows to move, / to filter, enter to switch capture, r to restart capture, q to quit".into()],
            last_refresh: Instant::now() - Duration::from_secs(999),
            capture_cmdline: None,
            host_alive: false,
            signaling_alive: false,
        };
        s.refresh();
        s
    }

    fn note(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
        if self.log.len() > 200 {
            let excess = self.log.len() - 200;
            self.log.drain(0..excess);
        }
    }

    fn filtered(&self) -> Vec<&WinInfo> {
        let f = self.filter.to_lowercase();
        self.windows
            .iter()
            .filter(|w| f.is_empty() || w.title.to_lowercase().contains(&f))
            .collect()
    }

    fn refresh(&mut self) {
        self.windows = list_windows();
        self.capture_cmdline = capture_running();
        self.host_alive = proc_alive("couchlink-host --signaling") || proc_alive("target/release/couchlink-host");
        self.signaling_alive = proc_alive("couchlink-signaling");
        self.last_refresh = Instant::now();
        if self.list_state.selected().is_none() && !self.filtered().is_empty() {
            self.list_state.select(Some(0));
        }
    }

    fn move_selection(&mut self, delta: i32) {
        let len = self.filtered().len();
        if len == 0 {
            self.list_state.select(None);
            return;
        }
        let cur = self.list_state.selected().unwrap_or(0) as i32;
        let next = (cur + delta).rem_euclid(len as i32) as usize;
        self.list_state.select(Some(next));
    }

    fn switch_to_selected(&mut self) {
        let title = {
            let items = self.filtered();
            let idx = self.list_state.selected().unwrap_or(0);
            items.get(idx).map(|w| w.title.clone())
        };
        let Some(title) = title else {
            self.note("no window selected");
            return;
        };
        self.note(format!("switching capture -> {title:?} ..."));
        match switch_capture_window(&self.root, &title) {
            Ok((ok, out)) => {
                for line in out.lines() {
                    if !line.trim().is_empty() {
                        self.note(line.trim().to_string());
                    }
                }
                if ok {
                    self.note(format!("capture now targeting {title:?}"));
                } else {
                    self.note("ensure-win-capture.sh exited non-zero");
                }
            }
            Err(e) => self.note(format!("switch failed: {e}")),
        }
        self.refresh();
    }

    fn restart(&mut self) {
        self.note("force-restarting capture ...");
        match restart_capture(&self.root) {
            Ok((ok, out)) => {
                for line in out.lines() {
                    if !line.trim().is_empty() {
                        self.note(line.trim().to_string());
                    }
                }
                if !ok {
                    self.note("ensure-win-capture.sh exited non-zero");
                }
            }
            Err(e) => self.note(format!("restart failed: {e}")),
        }
        self.refresh();
    }
}

enum Mode {
    Normal,
    Filter,
}

fn status_line(label: &str, ok: bool) -> Span<'static> {
    let (glyph, color) = if ok { ("●", Color::Green) } else { ("●", Color::Red) };
    Span::styled(format!("{glyph} {label}"), Style::default().fg(color))
}

fn draw(f: &mut Frame, app: &mut App, mode: &Mode) {
    let size = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Min(6), Constraint::Length(8), Constraint::Length(1)])
        .split(size);

    draw_status(f, app, chunks[0]);
    draw_windows(f, app, mode, chunks[1]);
    draw_log(f, app, chunks[2]);
    draw_help(f, chunks[3]);
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let capture_line = match &app.capture_cmdline {
        Some(cl) => {
            let target = cl
                .split("--window")
                .nth(1)
                .map(|s| s.trim().trim_matches('"').to_string())
                .unwrap_or_else(|| "(unknown)".into());
            Line::from(vec![
                status_line("capture", true),
                Span::raw(format!("  target = {target}")),
            ])
        }
        None => Line::from(vec![status_line("capture", false), Span::raw("  not running")]),
    };
    let lines = vec![
        Line::from(vec![
            status_line("host", app.host_alive),
            Span::raw("   "),
            status_line("signaling", app.signaling_alive),
        ]),
        capture_line,
    ];
    let p = Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" couchlink-tui "));
    f.render_widget(p, area);
}

fn draw_windows(f: &mut Frame, app: &mut App, mode: &Mode, area: Rect) {
    let title = match mode {
        Mode::Filter => format!(" windows — filter: {}_ ", app.filter),
        Mode::Normal => " windows (enter = switch capture target) ".to_string(),
    };
    let items: Vec<ListItem> = app
        .filtered()
        .iter()
        .map(|w| ListItem::new(w.title.clone()))
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");
    f.render_stateful_widget(list, area, &mut app.list_state);
}

fn draw_log(f: &mut Frame, app: &App, area: Rect) {
    let n = area.height.saturating_sub(2) as usize;
    let start = app.log.len().saturating_sub(n);
    let text = app.log[start..].join("\n");
    let p = Paragraph::new(text)
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).title(" log "));
    f.render_widget(p, area);
}

fn draw_help(f: &mut Frame, area: Rect) {
    let p = Paragraph::new("j/k move · / filter · enter switch · r restart capture · R refresh · q quit");
    f.render_widget(p, area);
}

fn main() -> Result<()> {
    let root = repo_root()?;
    let mut app = App::new(root);
    let mut mode = Mode::Normal;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = (|| -> Result<()> {
        loop {
            terminal.draw(|f| draw(f, &mut app, &mode))?;

            if app.last_refresh.elapsed() > Duration::from_secs(3) {
                app.refresh();
            }

            if !event::poll(Duration::from_millis(300))? {
                continue;
            }
            let Event::Key(key) = event::read()? else { continue };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match mode {
                Mode::Normal => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('j') | KeyCode::Down => app.move_selection(1),
                    KeyCode::Char('k') | KeyCode::Up => app.move_selection(-1),
                    KeyCode::Char('/') => mode = Mode::Filter,
                    KeyCode::Char('r') => app.restart(),
                    KeyCode::Char('R') => app.refresh(),
                    KeyCode::Enter => app.switch_to_selected(),
                    _ => {}
                },
                Mode::Filter => match key.code {
                    KeyCode::Esc => {
                        app.filter.clear();
                        mode = Mode::Normal;
                    }
                    KeyCode::Enter => mode = Mode::Normal,
                    KeyCode::Backspace => {
                        app.filter.pop();
                    }
                    KeyCode::Char(c) => app.filter.push(c),
                    _ => {}
                },
            }
        }
        Ok(())
    })();

    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    result
}
