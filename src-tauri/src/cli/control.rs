//! Local control channel for a running `capture`.
//!
//! Only one process can hold a COM port, so a second `sdesk send` cannot open
//! the port that a capture already owns. Instead, `capture` starts a loopback
//! TCP listener, writes its port and a random token to a state file, and
//! `send` talks to that listener. The listener binds `127.0.0.1` only and every
//! request must present the token.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{util, CliResult, Fail};
use crate::serial::SessionCore;

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionState {
    pub version: u32,
    pub port: String,
    pub pid: u32,
    pub control_port: u16,
    pub token: String,
    pub log_path: Option<String>,
    pub started_at_ms: i64,
    pub baud_rate: u32,
}

pub fn state_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("com.serialdesk.app").join("cli-sessions")
}

pub fn state_path(port: &str) -> PathBuf {
    state_dir().join(format!("{}.json", sanitize(port)))
}

fn sanitize(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('_');
    if cleaned.is_empty() {
        "serial".to_string()
    } else {
        cleaned.to_ascii_uppercase()
    }
}

fn write_state(path: &Path, state: &SessionState) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建会话状态目录“{}”：{error}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(state)
        .map_err(|error| format!("无法序列化会话状态：{error}"))?;
    fs::write(path, text).map_err(|error| {
        format!(
            "无法写入会话状态文件“{}”：{error}。发送命令将无法找到正在运行的 capture。",
            path.display()
        )
    })
}

fn random_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    let mut first = RandomState::new().build_hasher();
    first.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    first.write_u32(std::process::id());
    let mut second = RandomState::new().build_hasher();
    second.write_u64(first.finish());
    format!("{:016x}{:016x}", first.finish(), second.finish())
}

pub struct ControlServer {
    port: u16,
    state_path: PathBuf,
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl ControlServer {
    pub fn start(
        core: Arc<SessionCore>,
        stop: Arc<AtomicBool>,
        close_requested: Arc<AtomicBool>,
        port_name: &str,
        log_path: Option<String>,
        baud_rate: u32,
    ) -> Result<Self, String> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|error| format!("无法启动本地控制通道：{error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("无法配置本地控制通道：{error}"))?;
        let port = listener
            .local_addr()
            .map_err(|error| format!("无法读取控制通道地址：{error}"))?
            .port();

        let token = random_token();
        let state = SessionState {
            version: PROTOCOL_VERSION,
            port: port_name.to_string(),
            pid: std::process::id(),
            control_port: port,
            token: token.clone(),
            log_path,
            started_at_ms: util::now_ms(),
            baud_rate,
        };
        let state_path = state_path(port_name);
        write_state(&state_path, &state)?;

        let running = Arc::new(AtomicBool::new(true));
        let thread_running = running.clone();
        let thread_token = token.clone();
        let thread_port = port_name.to_string();
        let spawned = thread::Builder::new()
            .name("sdesk-control".to_string())
            .spawn(move || {
                serve(
                    listener,
                    core,
                    stop,
                    close_requested,
                    thread_running,
                    thread_token,
                    thread_port,
                )
            });

        match spawned {
            Ok(handle) => Ok(Self {
                port,
                state_path,
                running,
                handle: Some(handle),
            }),
            Err(error) => {
                let _ = fs::remove_file(&state_path);
                Err(format!("无法启动控制通道线程：{error}"))
            }
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        let _ = fs::remove_file(&self.state_path);
    }
}

fn serve(
    listener: TcpListener,
    core: Arc<SessionCore>,
    stop: Arc<AtomicBool>,
    close_requested: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    token: String,
    port: String,
) {
    while running.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = handle_connection(stream, &core, &stop, &close_requested, &token, &port);
            }
            Err(ref error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(25));
            }
            Err(_) => break,
        }
    }
}

fn handle_connection(
    mut stream: TcpStream,
    core: &SessionCore,
    stop: &AtomicBool,
    close_requested: &AtomicBool,
    token: &str,
    port: &str,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;

    let response = respond(&line, core, stop, close_requested, token, port);
    let mut text = serde_json::to_string(&response)
        .unwrap_or_else(|_| "{\"ok\":false,\"error\":\"无法序列化响应\"}".to_string());
    text.push('\n');
    stream.write_all(text.as_bytes())?;
    stream.flush()
}

fn respond(
    line: &str,
    core: &SessionCore,
    stop: &AtomicBool,
    close_requested: &AtomicBool,
    token: &str,
    port: &str,
) -> Value {
    let request: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => return json!({"ok": false, "error": format!("请求不是有效 JSON：{error}")}),
    };
    if request.get("token").and_then(Value::as_str) != Some(token) {
        return json!({"ok": false, "error": "控制令牌无效"});
    }

    match request.get("cmd").and_then(Value::as_str).unwrap_or("") {
        "ping" => json!({
            "ok": true,
            "port": port,
            "pid": std::process::id(),
            "apiVersion": PROTOCOL_VERSION,
        }),
        "status" => json!({"ok": true, "session": core.status()}),
        "send" => {
            let Some(data) = request.get("data").and_then(Value::as_str) else {
                return json!({"ok": false, "error": "send 需要 data（十六进制字符串）"});
            };
            let bytes = match super::args::parse_hex(data) {
                Ok(bytes) => bytes,
                Err(error) => return json!({"ok": false, "error": error}),
            };
            match core.send(&bytes) {
                Ok(report) => json!({
                    "ok": true,
                    "bytesSent": report.bytes_sent,
                    "txBytesTotal": report.tx_bytes_total,
                    "timestampMs": report.timestamp_ms,
                }),
                Err(error) => json!({"ok": false, "error": error}),
            }
        }
        "close" => {
            close_requested.store(true, Ordering::SeqCst);
            stop.store(true, Ordering::SeqCst);
            json!({"ok": true, "closing": true})
        }
        other => json!({"ok": false, "error": format!("未知控制命令“{other}”")}),
    }
}

fn connect(state: &SessionState, timeout: Duration) -> Result<TcpStream, String> {
    let address = format!("127.0.0.1:{}", state.control_port);
    let socket = address
        .parse()
        .map_err(|error| format!("控制通道地址无效：{error}"))?;
    TcpStream::connect_timeout(&socket, timeout)
        .map_err(|error| format!("无法连接会话控制通道 {address}：{error}"))
}

pub fn request(state: &SessionState, command: &str, data: Option<&str>) -> Result<Value, String> {
    let mut stream = connect(state, Duration::from_secs(2))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| format!("无法设置控制通道超时：{error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| format!("无法设置控制通道超时：{error}"))?;

    let mut payload = json!({"token": state.token, "cmd": command});
    if let Some(data) = data {
        payload["data"] = json!(data);
    }
    let mut text = payload.to_string();
    text.push('\n');

    stream
        .write_all(text.as_bytes())
        .map_err(|error| format!("写入控制通道失败：{error}"))?;
    stream
        .flush()
        .map_err(|error| format!("写入控制通道失败：{error}"))?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|error| format!("读取控制通道响应失败：{error}"))?;
    if line.trim().is_empty() {
        return Err("控制通道没有返回响应。".to_string());
    }
    serde_json::from_str(&line).map_err(|error| format!("控制通道响应不是有效 JSON：{error}"))
}

pub fn is_alive(state: &SessionState) -> bool {
    request(state, "ping", None)
        .map(|value| value.get("ok").and_then(Value::as_bool).unwrap_or(false))
        .unwrap_or(false)
}

pub fn read_states() -> Vec<(PathBuf, SessionState)> {
    let dir = state_dir();
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut states = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path
            .extension()
            .map(|extension| extension.eq_ignore_ascii_case("json"))
            .unwrap_or(false)
        {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(state) = serde_json::from_str::<SessionState>(&text) else {
            continue;
        };
        if state.version != PROTOCOL_VERSION {
            continue;
        }
        states.push((path, state));
    }
    states.sort_by_key(|(_, state)| std::cmp::Reverse(state.started_at_ms));
    states
}

pub fn load_state_file(path: &Path) -> Result<SessionState, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("无法读取会话文件“{}”：{error}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|error| format!("会话文件“{}”格式无效：{error}", path.display()))
}

pub const SESSIONS_HELP: &str = "用法: sdesk sessions [--json] [--prune]

列出正在运行的 capture 会话。每个 capture 会写一个状态文件到
%LOCALAPPDATA%\\com.serialdesk.app\\cli-sessions。

选项:
  --json    输出 JSON 数组
  --prune   删除已经失效（进程已退出）的状态文件";

pub fn cmd_sessions(argv: &[String]) -> CliResult<i32> {
    let opts = super::args::parse(argv, &[("json", false), ("prune", false)])?;
    let prune = opts.has("prune");

    let mut rows: Vec<(PathBuf, SessionState, bool)> = Vec::new();
    let mut pruned = 0usize;
    for (path, state) in read_states() {
        let alive = is_alive(&state);
        if !alive && prune {
            let _ = fs::remove_file(&path);
            pruned += 1;
            continue;
        }
        rows.push((path, state, alive));
    }

    if opts.has("json") {
        let items: Vec<Value> = rows
            .iter()
            .map(|(path, state, alive)| {
                json!({
                    "type": "session",
                    "port": state.port,
                    "pid": state.pid,
                    "controlPort": state.control_port,
                    "baudRate": state.baud_rate,
                    "logPath": state.log_path,
                    "startedAtMs": state.started_at_ms,
                    "startedAt": util::format_local(state.started_at_ms),
                    "alive": alive,
                    "statePath": path.to_string_lossy(),
                })
            })
            .collect();
        println!("{}", super::json_pretty(&items)?);
    } else if rows.is_empty() {
        println!("没有运行中的 capture 会话。");
    } else {
        println!(
            "{:<6} {:<7} {:<21} {:<8} {:<8} LOG",
            "PORT", "PID", "STARTED", "BAUD", "STATE"
        );
        for (_, state, alive) in &rows {
            println!(
                "{:<6} {:<7} {:<21} {:<8} {:<8} {}",
                state.port,
                state.pid,
                util::format_local(state.started_at_ms),
                state.baud_rate,
                if *alive { "running" } else { "stale" },
                state.log_path.as_deref().unwrap_or("-")
            );
        }
    }

    if pruned > 0 {
        eprintln!("已清理 {pruned} 个失效的会话状态文件。");
    }
    Ok(0)
}

pub const STOP_HELP: &str = "用法: sdesk stop --port <PORT> [--json]
       sdesk stop --all [--json]
       sdesk stop [--json]        （只有一个会话时）

请求运行中的 capture 正常收尾：停止读线程、写入日志结束标记并关闭串口。
这是停止后台 capture 的首选方式，比直接结束进程更安全。

选项:
  --port <PORT>  指定要停止的会话
  --all          停止所有会话
  --json         输出 JSON";

pub fn cmd_stop(argv: &[String]) -> CliResult<i32> {
    let opts = super::args::parse(argv, &[("port", true), ("all", false), ("json", false)])?;
    let all = opts.has("all");
    let port = opts.get_port();

    let states = read_states();
    if states.is_empty() {
        if opts.has("json") {
            println!("[]");
        } else {
            println!("没有运行中的 capture 会话。");
        }
        return Ok(0);
    }

    let targets: Vec<(PathBuf, SessionState)> = if all {
        states
    } else if let Some(port) = port.as_deref() {
        let wanted = normalize_port(port);
        let matched: Vec<_> = states
            .into_iter()
            .filter(|(_, state)| state.port.eq_ignore_ascii_case(&wanted))
            .collect();
        if matched.is_empty() {
            return Err(Fail::runtime(format!(
                "没有找到端口为 {wanted} 的运行中会话。"
            )));
        }
        matched
    } else if states.len() == 1 {
        states
    } else {
        let ports: Vec<String> = states.iter().map(|(_, state)| state.port.clone()).collect();
        return Err(Fail::usage(format!(
            "有多个运行中的会话，请用 --port 或 --all 指定：{}。",
            ports.join("、")
        )));
    };

    let mut results = Vec::new();
    for (path, state) in targets {
        let mut stopped = false;
        let mut error = None;
        match request(&state, "close", None) {
            Ok(value) if value.get("ok").and_then(Value::as_bool).unwrap_or(false) => {
                stopped = true;
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                while std::time::Instant::now() < deadline && is_alive(&state) {
                    thread::sleep(Duration::from_millis(50));
                }
                if !is_alive(&state) {
                    let _ = fs::remove_file(&path);
                }
            }
            Ok(value) => {
                error = Some(
                    value
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("会话拒绝关闭")
                        .to_string(),
                );
            }
            Err(message) => {
                // A crashed capture leaves its state file behind. If the
                // listener is gone, there is nothing left to stop; clean the
                // stale file rather than reporting a failure.
                if is_alive(&state) {
                    error = Some(message);
                } else {
                    let _ = fs::remove_file(&path);
                    stopped = true;
                }
            }
        }

        if opts.has("json") {
            results.push(json!({
                "type": "stop",
                "port": state.port,
                "pid": state.pid,
                "stopped": stopped,
                "error": error,
            }));
        } else if stopped {
            println!("已停止 {}（PID {}）。", state.port, state.pid);
        } else {
            eprintln!(
                "sdesk: 无法停止 {}：{}",
                state.port,
                error.as_deref().unwrap_or("未知错误")
            );
        }
    }

    if opts.has("json") {
        println!("{}", super::json_pretty(&results)?);
    }

    let failed = results
        .iter()
        .filter(|value| !value["stopped"].as_bool().unwrap_or(false))
        .count();
    if failed > 0 {
        return Err(Fail::runtime(format!("{failed} 个会话未能停止。")));
    }
    Ok(0)
}

fn normalize_port(raw: &str) -> String {
    let trimmed = raw.trim();
    if !trimmed.is_empty() && trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        format!("COM{trimmed}")
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_file_names_are_safe_and_case_insensitive() {
        assert_eq!(sanitize("COM10"), "COM10");
        assert_eq!(sanitize("com3"), "COM3");
        assert_eq!(sanitize("../COM:8"), "COM_8");
        assert_eq!(sanitize(""), "serial");
        assert!(state_path("COM3").ends_with("COM3.json"));
    }

    #[test]
    fn tokens_are_not_repeated() {
        let first = random_token();
        let second = random_token();
        assert_eq!(first.len(), 32);
        assert_ne!(first, second);
    }
}
