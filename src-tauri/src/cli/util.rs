//! Small platform and path helpers shared by the commands.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::{CliResult, Fail};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
static INTERRUPT_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Makes Chinese help text and captured UTF-8 readable in a Windows console.
pub fn init_console() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Console::{SetConsoleCP, SetConsoleOutputCP};
        let _ = SetConsoleOutputCP(65001);
        let _ = SetConsoleCP(65001);
    }
}

/// Registers a Ctrl+C handler that asks the session to close cleanly.
///
/// The first Ctrl+C sets [`interrupted`] so the capture can stop the reader,
/// write the log footer and release the port. A second Ctrl+C is passed back to
/// the default handler, which terminates the process immediately — the escape
/// hatch if a device driver refuses to return.
pub fn install_interrupt_handler() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

        unsafe extern "system" fn handler(kind: u32) -> i32 {
            // 0 = CTRL_C_EVENT, 1 = CTRL_BREAK_EVENT.
            if kind > 1 {
                return 0;
            }
            INTERRUPT_COUNT.fetch_add(1, Ordering::SeqCst);
            INTERRUPTED.store(true, Ordering::SeqCst);
            if INTERRUPT_COUNT.load(Ordering::SeqCst) > 1 {
                return 0;
            }
            1
        }

        let _ = SetConsoleCtrlHandler(Some(handler), 1);
    }
}

pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as i64)
        .unwrap_or(0)
}

pub fn absolutize(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

/// Resolves the log directory in the same order the GUI would.
///
/// An explicit `--log-dir` wins. Otherwise the GUI's saved setting is used, so
/// `sdesk log tail --latest` finds the logs the desktop application wrote. The
/// final fallback matches the GUI default on a standard Windows profile.
pub fn resolve_log_dir(explicit: Option<&str>) -> PathBuf {
    if let Some(raw) = explicit {
        return absolutize(Path::new(raw));
    }
    if let Some(dir) = gui_log_dir() {
        return dir;
    }
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Documents").join("Serial Desk").join("Logs")
}

pub fn gui_settings_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|base| {
        PathBuf::from(base)
            .join("com.serialdesk.app")
            .join("settings.json")
    })
}

fn gui_log_dir() -> Option<PathBuf> {
    let path = gui_settings_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let dir = value.get("logDir")?.as_str()?.trim();
    if dir.is_empty() {
        None
    } else {
        Some(PathBuf::from(dir))
    }
}

fn normalize_port(raw: &str) -> String {
    let trimmed = raw.trim();
    if !trimmed.is_empty() && trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        format!("COM{trimmed}")
    } else {
        trimmed.to_string()
    }
}

/// Chooses the port named by `--port`, or the only port present.
///
/// Refusing to guess when several ports exist is deliberate: sending an
/// embedded command to the wrong device is worse than an extra flag.
pub fn pick_port(explicit: Option<&str>) -> CliResult<String> {
    let ports = crate::serial::list_ports().map_err(Fail::runtime)?;

    if let Some(raw) = explicit {
        let wanted = normalize_port(raw);
        if let Some(port) = ports
            .iter()
            .find(|port| port.name.eq_ignore_ascii_case(&wanted))
        {
            return Ok(port.name.clone());
        }
        let available: Vec<&str> = ports.iter().map(|port| port.name.as_str()).collect();
        let hint = if available.is_empty() {
            "本机没有可用串口。".to_string()
        } else {
            format!("可用串口：{}。", available.join("、"))
        };
        return Err(Fail::runtime(format!("找不到串口“{raw}”。{hint}")));
    }

    match ports.len() {
        1 => Ok(ports[0].name.clone()),
        0 => Err(Fail::runtime("本机没有发现串口。")),
        _ => {
            let names: Vec<&str> = ports.iter().map(|port| port.name.as_str()).collect();
            Err(Fail::usage(format!(
                "检测到多个串口，请用 --port 指定：{}。",
                names.join("、")
            )))
        }
    }
}

pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    let value = bytes as f64;
    if value < KB {
        format!("{bytes} B")
    } else if value < MB {
        format!("{:.1} KB", value / KB)
    } else {
        format!("{:.1} MB", value / MB)
    }
}

/// Formats epoch milliseconds as local wall-clock time with milliseconds.
pub fn format_local(ms: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(ms).single() {
        Some(value) => value.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        None => ms.to_string(),
    }
}
