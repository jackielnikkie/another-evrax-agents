//! External tunnel processes (Cloudflare quick tunnel) managed by evrax.
//!
//! The tunnel always runs as an external child process: evrax only spawns it,
//! reads its output for the public URL, and kills it on stop/drop. Nothing of
//! the tunnel traffic flows through the TUI process itself.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Managed cloudflared binary download (Windows amd64).
pub const CLOUDFLARED_URL: &str = "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe";

/// Name of the named tunnel created for custom domains.
pub const TUNNEL_NAME: &str = "evrax";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TunnelStatus {
    #[default]
    Off,
    Downloading,
    Starting,
    /// Waiting for the browser login to complete; `url` is the auth link.
    Login {
        url: String,
    },
    Running {
        url: String,
    },
    Error {
        message: String,
    },
}

struct Inner {
    child: Option<Child>,
    status: TunnelStatus,
    cancel: bool,
    /// Generation counter: a restarted tunnel invalidates readers/watchdogs of
    /// the previous child so they can never touch the new one.
    generation: u64,
}

/// Handle to at most one external tunnel process. Cheap to clone; the process
/// is killed when the last handle is dropped or `stop` is called.
#[derive(Clone)]
pub struct TunnelManager {
    inner: Arc<Mutex<Inner>>,
}

impl Default for TunnelManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TunnelManager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                child: None,
                status: TunnelStatus::Off,
                cancel: false,
                generation: 0,
            })),
        }
    }

    pub fn status(&self) -> TunnelStatus {
        self.inner
            .lock()
            .map(|g| g.status.clone())
            .unwrap_or(TunnelStatus::Off)
    }

    pub fn url(&self) -> Option<String> {
        match self.status() {
            TunnelStatus::Running { url } | TunnelStatus::Login { url } => Some(url),
            _ => None,
        }
    }

    pub fn is_active(&self) -> bool {
        !matches!(self.status(), TunnelStatus::Off)
    }

    /// Kill the tunnel process (if any) and go back to `Off`.
    pub fn stop(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.cancel = true;
            g.generation += 1;
            if let Some(mut child) = g.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            g.status = TunnelStatus::Off;
        }
    }

    /// Start a Cloudflare quick tunnel pointing at the local remote server.
    /// Runs entirely in the background; poll `status()` for progress. Any
    /// previous tunnel process is killed first (no orphaned cloudflared).
    pub fn start_quick(&self, port: u16) {
        let Some(generation) = begin_generation(&self.inner) else {
            return;
        };
        let inner = self.inner.clone();
        std::thread::spawn(move || run_quick(inner, port, generation));
    }

    /// Start a named tunnel for a custom domain (browser login on first use,
    /// then create + route dns + run). Kills any previous process first.
    pub fn start_domain(&self, port: u16, hostname: String) {
        let Some(generation) = begin_generation(&self.inner) else {
            return;
        };
        let inner = self.inner.clone();
        std::thread::spawn(move || run_domain(inner, port, hostname, generation));
    }
}

/// Kill any previous child and claim the next generation for a new start.
fn begin_generation(inner: &Arc<Mutex<Inner>>) -> Option<u64> {
    let mut g = inner.lock().ok()?;
    g.cancel = true;
    if let Some(mut old) = g.child.take() {
        let _ = old.kill();
        let _ = old.wait();
    }
    g.cancel = false;
    g.generation += 1;
    g.status = TunnelStatus::Starting;
    Some(g.generation)
}

impl Drop for TunnelManager {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) == 1 {
            self.stop();
        }
    }
}

fn set_status(inner: &Arc<Mutex<Inner>>, generation: u64, status: TunnelStatus) {
    if let Ok(mut g) = inner.lock()
        && g.generation == generation
        && !g.cancel
    {
        g.status = status;
    }
}

/// True while `generation` is still the current, non-canceled tunnel.
fn is_current(inner: &Arc<Mutex<Inner>>, generation: u64) -> bool {
    inner
        .lock()
        .map(|g| g.generation == generation && !g.cancel)
        .unwrap_or(false)
}

fn run_quick(inner: Arc<Mutex<Inner>>, port: u16, generation: u64) {
    let bin = match ensure_cloudflared(&inner, generation) {
        Ok(bin) => bin,
        Err(message) => {
            if is_current(&inner, generation) {
                set_status(&inner, generation, TunnelStatus::Error { message });
            }
            return;
        }
    };
    if !is_current(&inner, generation) {
        return;
    }

    let mut cmd = Command::new(&bin);
    cmd.arg("tunnel")
        .arg("--url")
        .arg(format!("http://127.0.0.1:{port}"))
        .arg("--no-autoupdate")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    hide_console(&mut cmd);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            set_status(
                &inner,
                generation,
                TunnelStatus::Error {
                    message: format!("failed to start cloudflared: {err}"),
                },
            );
            return;
        }
    };
    let stderr = child.stderr.take();
    {
        let Ok(mut g) = inner.lock() else { return };
        if g.cancel || g.generation != generation {
            let _ = child.kill();
            return;
        }
        g.child = Some(child);
    }
    set_status(&inner, generation, TunnelStatus::Starting);

    // Watchdog: a quick tunnel prints its URL within seconds; bail out so the
    // dialog never shows "starting" forever.
    {
        let watchdog = inner.clone();
        std::thread::spawn(move || {
            for _ in 0..84 {
                std::thread::sleep(Duration::from_millis(300));
                match watchdog.lock() {
                    Ok(g) => {
                        if g.generation != generation || g.cancel {
                            return;
                        }
                        if !matches!(g.status, TunnelStatus::Starting) {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
            if let Ok(mut g) = watchdog.lock()
                && g.generation == generation
                && matches!(g.status, TunnelStatus::Starting)
            {
                if let Some(child) = g.child.as_mut() {
                    let _ = child.kill();
                }
                g.status = TunnelStatus::Error {
                    message: "timed out waiting for a cloudflared URL".into(),
                };
            }
        });
    }

    let Some(stderr) = stderr else {
        set_status(
            &inner,
            generation,
            TunnelStatus::Error {
                message: "cloudflared produced no output".into(),
            },
        );
        return;
    };

    let mut last_line = String::new();
    for line in BufReader::new(stderr).lines() {
        let Ok(line) = line else { break };
        if !is_current(&inner, generation) {
            return;
        }
        last_line = line;
        if let Some(url) = extract_url(&last_line) {
            set_status(&inner, generation, TunnelStatus::Running { url });
        }
    }

    // stderr hit EOF: this generation's process is gone.
    if !is_current(&inner, generation) {
        return;
    }
    let was_running = matches!(
        inner.lock().map(|g| g.status.clone()),
        Ok(TunnelStatus::Running { .. })
    );
    let mut finished = None;
    if let Ok(mut g) = inner.lock()
        && g.generation == generation
    {
        finished = g.child.take();
    }
    if let Some(child) = finished.as_mut() {
        let _ = child.wait();
    }
    let message = if was_running {
        "cloudflared stopped".to_string()
    } else {
        let trimmed = last_line.trim();
        if trimmed.is_empty() {
            "cloudflared exited without a URL".to_string()
        } else {
            trimmed.chars().take(80).collect()
        }
    };
    set_status(&inner, generation, TunnelStatus::Error { message });
}

/// Named-tunnel flow: browser login (first time), create tunnel, route DNS,
/// then run `cloudflared tunnel run` and expose `https://<hostname>`.
fn run_domain(inner: Arc<Mutex<Inner>>, port: u16, hostname: String, generation: u64) {
    let bin = match ensure_cloudflared(&inner, generation) {
        Ok(bin) => bin,
        Err(message) => {
            if is_current(&inner, generation) {
                set_status(&inner, generation, TunnelStatus::Error { message });
            }
            return;
        }
    };
    if !is_current(&inner, generation) {
        return;
    }
    if let Err(message) = ensure_login(&inner, &bin, generation) {
        if is_current(&inner, generation) {
            set_status(&inner, generation, TunnelStatus::Error { message });
        }
        return;
    }
    if !is_current(&inner, generation) {
        return;
    }
    if let Err(message) = provision_tunnel(&bin, &hostname) {
        if is_current(&inner, generation) {
            set_status(&inner, generation, TunnelStatus::Error { message });
        }
        return;
    }
    if !is_current(&inner, generation) {
        return;
    }

    let mut cmd = Command::new(&bin);
    cmd.arg("tunnel")
        .arg("run")
        .arg("--url")
        .arg(format!("http://127.0.0.1:{port}"))
        .arg(TUNNEL_NAME)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    hide_console(&mut cmd);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            set_status(
                &inner,
                generation,
                TunnelStatus::Error {
                    message: format!("failed to start cloudflared: {err}"),
                },
            );
            return;
        }
    };
    let stderr = child.stderr.take();
    {
        let Ok(mut g) = inner.lock() else { return };
        if g.cancel || g.generation != generation {
            let _ = child.kill();
            return;
        }
        g.child = Some(child);
    }
    set_status(&inner, generation, TunnelStatus::Starting);

    // No URL line is printed for named tunnels: a process that survives the
    // startup window is considered live on the configured hostname.
    {
        let watchdog = inner.clone();
        let url = format!("https://{hostname}");
        std::thread::spawn(move || {
            for _ in 0..34 {
                std::thread::sleep(Duration::from_millis(300));
                match watchdog.lock() {
                    Ok(g) => {
                        if g.generation != generation || g.cancel {
                            return;
                        }
                        if !matches!(g.status, TunnelStatus::Starting) {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
            if let Ok(mut g) = watchdog.lock()
                && g.generation == generation
                && matches!(g.status, TunnelStatus::Starting)
            {
                g.status = TunnelStatus::Running { url };
            }
        });
    }

    let Some(stderr) = stderr else {
        set_status(
            &inner,
            generation,
            TunnelStatus::Error {
                message: "cloudflared produced no output".into(),
            },
        );
        return;
    };

    let mut last_line = String::new();
    for line in BufReader::new(stderr).lines() {
        let Ok(line) = line else { break };
        if !is_current(&inner, generation) {
            return;
        }
        last_line = line;
    }

    if !is_current(&inner, generation) {
        return;
    }
    let was_running = matches!(
        inner.lock().map(|g| g.status.clone()),
        Ok(TunnelStatus::Running { .. })
    );
    let mut finished = None;
    if let Ok(mut g) = inner.lock()
        && g.generation == generation
    {
        finished = g.child.take();
    }
    if let Some(child) = finished.as_mut() {
        let _ = child.wait();
    }
    let message = if was_running {
        "cloudflared stopped".to_string()
    } else {
        let trimmed = last_line.trim();
        if trimmed.is_empty() {
            "cloudflared exited without a URL".to_string()
        } else {
            trimmed.chars().take(80).collect()
        }
    };
    set_status(&inner, generation, TunnelStatus::Error { message });
}

/// One-time browser login: skips when `cert.pem` already exists, otherwise runs
/// `cloudflared tunnel login` and surfaces the auth URL as `Login{url}` until
/// the browser flow finishes.
fn ensure_login(
    inner: &Arc<Mutex<Inner>>,
    bin: &std::path::Path,
    generation: u64,
) -> Result<(), String> {
    if cloudflared_dir()
        .map(|d| d.join("cert.pem").exists())
        .unwrap_or(false)
    {
        return Ok(());
    }
    let mut cmd = Command::new(bin);
    cmd.args(["tunnel", "login"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    hide_console(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to run cloudflared login: {e}"))?;
    let stderr = child.stderr.take();
    {
        let Ok(mut g) = inner.lock() else {
            return Err("dibatalkan".into());
        };
        if g.cancel || g.generation != generation {
            let _ = child.kill();
            return Err("dibatalkan".into());
        }
        if let Some(mut old) = g.child.take() {
            let _ = old.kill();
            let _ = old.wait();
        }
        g.child = Some(child);
    }
    set_status(inner, generation, TunnelStatus::Starting);

    let Some(stderr) = stderr else {
        return Err("cloudflared login produced no output".into());
    };

    // Login waits for the user's browser; allow up to 8 minutes.
    {
        let watchdog = inner.clone();
        std::thread::spawn(move || {
            for _ in 0..1600 {
                std::thread::sleep(Duration::from_millis(300));
                match watchdog.lock() {
                    Ok(g) => {
                        if g.generation != generation || g.cancel {
                            return;
                        }
                        if !matches!(
                            g.status,
                            TunnelStatus::Starting | TunnelStatus::Login { .. }
                        ) {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
            if let Ok(mut g) = watchdog.lock()
                && g.generation == generation
                && matches!(
                    g.status,
                    TunnelStatus::Starting | TunnelStatus::Login { .. }
                )
            {
                if let Some(child) = g.child.as_mut() {
                    let _ = child.kill();
                }
                g.status = TunnelStatus::Error {
                    message: "cloudflare login timed out (browser flow not finished)".into(),
                };
            }
        });
    }

    for line in BufReader::new(stderr).lines() {
        let Ok(line) = line else { break };
        if !is_current(inner, generation) {
            return Err("dibatalkan".into());
        }
        if let Some(url) = extract_url(&line) {
            set_status(inner, generation, TunnelStatus::Login { url });
        }
    }

    if !is_current(inner, generation) {
        return Err("dibatalkan".into());
    }
    let mut finished = None;
    if let Ok(mut g) = inner.lock()
        && g.generation == generation
    {
        finished = g.child.take();
    }
    if let Some(child) = finished.as_mut() {
        let _ = child.wait();
    }
    if cloudflared_dir()
        .map(|d| d.join("cert.pem").exists())
        .unwrap_or(false)
    {
        set_status(inner, generation, TunnelStatus::Starting);
        Ok(())
    } else {
        Err("cloudflare login not finished yet - try again".into())
    }
}

/// Create the named tunnel and route the hostname (both idempotent).
fn provision_tunnel(bin: &std::path::Path, hostname: &str) -> Result<(), String> {
    let out = run_capture(bin, &["tunnel", "create", TUNNEL_NAME])?;
    if !out.status.success() {
        let text = output_text(&out);
        if !text.contains("already exists") {
            return Err(format!(
                "cloudflared tunnel create failed: {}",
                summarize(&text)
            ));
        }
    }
    let out = run_capture(bin, &["tunnel", "route", "dns", TUNNEL_NAME, hostname])?;
    if !out.status.success() {
        let text = output_text(&out);
        if !(text.contains("already exists") || text.contains("already configured")) {
            return Err(format!("dns route failed: {}", summarize(&text)));
        }
    }
    Ok(())
}

fn run_capture(bin: &std::path::Path, args: &[&str]) -> Result<std::process::Output, String> {
    let mut cmd = Command::new(bin);
    cmd.args(args);
    hide_console(&mut cmd);
    cmd.output()
        .map_err(|e| format!("failed to start cloudflared: {e}"))
}

fn output_text(out: &std::process::Output) -> String {
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    text
}

fn summarize(text: &str) -> String {
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().chars().take(90).collect())
        .unwrap_or_else(|| "unknown error".to_string())
}

fn cloudflared_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(|dir| PathBuf::from(dir).join(".cloudflared"))
}

/// First `https://…` token of a cloudflared log line, if any.
fn extract_url(line: &str) -> Option<String> {
    let start = line.find("https://")?;
    let rest = &line[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '|' || c == '│' || c == '"' || c == '\'')
        .unwrap_or(rest.len());
    let url = rest[..end].trim_end_matches(['.', ',', ')']);
    if url.len() > "https://".len() + 3 && url.contains('.') {
        Some(url.to_string())
    } else {
        None
    }
}

fn managed_bin_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|dir| {
        PathBuf::from(dir)
            .join("evrax")
            .join("bin")
            .join("cloudflared.exe")
    })
}

/// Managed binary first, then a `cloudflared` on PATH.
fn resolve_cloudflared() -> Option<PathBuf> {
    if let Some(path) = managed_bin_path()
        && path.exists()
    {
        return Some(path);
    }
    let probe = Command::new("cloudflared").arg("--version").output();
    if probe.map(|o| o.status.success()).unwrap_or(false) {
        return Some(PathBuf::from("cloudflared"));
    }
    None
}

/// Download cloudflared into `%APPDATA%\evrax\bin` when it is not present yet.
/// Status becomes `Downloading` while curl (or PowerShell) fetches it.
fn ensure_cloudflared(inner: &Arc<Mutex<Inner>>, generation: u64) -> Result<PathBuf, String> {
    if let Some(bin) = resolve_cloudflared() {
        return Ok(bin);
    }
    let dest = managed_bin_path().ok_or_else(|| "APPDATA not found".to_string())?;
    set_status(inner, generation, TunnelStatus::Downloading);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }
    let dest_str = dest.to_string_lossy().to_string();

    let mut curl = Command::new("curl.exe");
    curl.args([
        "-L",
        "--fail",
        "--silent",
        "--show-error",
        "--max-time",
        "300",
        "-o",
        &dest_str,
        CLOUDFLARED_URL,
    ]);
    hide_console(&mut curl);
    let downloaded = match curl.status() {
        Ok(status) if status.success() => true,
        Ok(_) => false,
        // No curl.exe: fall back to PowerShell.
        Err(_) => {
            let script = format!(
                "Invoke-WebRequest -UseBasicParsing -Uri '{CLOUDFLARED_URL}' -OutFile '{dest_str}'"
            );
            let mut ps = Command::new("powershell");
            ps.args(["-NoProfile", "-Command", &script]);
            hide_console(&mut ps);
            ps.status().map(|s| s.success()).unwrap_or(false)
        }
    };
    if !is_current(inner, generation) {
        return Err("dibatalkan".into());
    }
    let size = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    if downloaded && size > 1_000_000 {
        Ok(dest)
    } else {
        Err("cloudflared download failed".into())
    }
}

fn hide_console(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_url_reads_quick_tunnel_box_line() {
        let line = "2026-10-18T21:05:53Z INF |  https://brave-forest-1234.trycloudflare.com                                     |";
        assert_eq!(
            extract_url(line),
            Some("https://brave-forest-1234.trycloudflare.com".into())
        );
    }

    #[test]
    fn extract_url_ignores_lines_without_url() {
        assert_eq!(
            extract_url("INF Requesting new quick Tunnel on trycloudflare.com..."),
            None
        );
        assert_eq!(extract_url("INF +----------------------------+"), None);
        assert_eq!(extract_url(""), None);
    }

    #[test]
    fn manager_starts_off_and_stops_cleanly() {
        let manager = TunnelManager::new();
        assert!(!manager.is_active());
        assert_eq!(manager.status(), TunnelStatus::Off);
        assert_eq!(manager.url(), None);
        manager.stop();
        assert!(!manager.is_active());
    }
}
