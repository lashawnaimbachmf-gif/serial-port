//! `sdesk capture` — headless serial session.
//!
//! Frames are written to the session log before they are printed, exactly as
//! in the GUI. Printing can therefore block without losing data from the log.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::args::{self, Opts};
use super::control::ControlServer;
use super::frame::{Format, Frame, FrameWriter};
use super::util;
use super::{CliResult, Fail};
use crate::serial::{ActiveSession, FrameKind, FrameSink};
use crate::settings::Settings;

pub const HELP: &str = "用法: sdesk capture [--port <PORT>] [串口参数] [日志与输出选项]

打开串口，把收发帧输出到 stdout，并写入与会话日志。Ctrl+C 结束；也可在
另一个进程用 sdesk stop --port <PORT> 让它正常收尾。

串口:
  --port <PORT>      端口名，如 COM10；只有一个串口时可省略
  --baud <N>         波特率，默认 115200（50–4000000）
  --data-bits <5-8>  数据位，默认 8
  --parity <none|even|odd>   校验位，默认 none
  --stop-bits <1|2>  停止位，默认 1
  --flow <none|hardware|software>   流控，默认 none

日志:
  --log-dir <DIR>    日志目录，默认使用 GUI 的设置或“文档\\Serial Desk\\Logs”
  --log-interval <SECS>   磁盘同步间隔，默认 5 秒（5–3600）
  --no-log           不写日志

输出:
  --format text|json|hex|raw   默认 text；json 为 JSONL
  --json             等价于 --format json
  --quiet            不在 stdout 输出帧（日志照常记录），适合只要日志的场景

结束条件:
  --until <TEXT>     收到匹配的 RX 数据后正常退出（支持 \\r \\n \\xNN 转义）
  --timeout <SECS>   最长运行时间，例如 30、500ms、2m
  --no-control       不启动本地控制通道（之后不能用 sdesk send 写入）

示例:
  sdesk capture --port COM10 --json
  sdesk capture --port COM10 --quiet --until \"boot complete\" --timeout 30
  sdesk capture --port COM10 --format raw > dump.bin

退出码: 0 正常结束；1 串口或输出错误；2 参数错误；3 --until 超时未匹配。";

pub fn cmd_capture(argv: &[String]) -> CliResult<i32> {
    let opts = args::parse(
        argv,
        &[
            ("help", false),
            ("port", true),
            ("baud", true),
            ("data-bits", true),
            ("parity", true),
            ("stop-bits", true),
            ("flow", true),
            ("log-dir", true),
            ("no-log", false),
            ("log-interval", true),
            ("format", true),
            ("json", false),
            ("quiet", false),
            ("timeout", true),
            ("until", true),
            ("no-control", false),
        ],
    )?;
    if opts.has("help") {
        println!("{HELP}");
        return Ok(0);
    }

    let format = Format::from_opts(&opts)?;
    let settings = build_settings(&opts)?;
    let until = match opts.value("until") {
        Some(raw) => {
            let bytes =
                args::decode_text(raw).map_err(|error| Fail::usage(format!("--until: {error}")))?;
            if bytes.is_empty() {
                return Err(Fail::usage("--until 不能为空。".to_string()));
            }
            Some(bytes)
        }
        None => None,
    };
    let has_until = until.is_some();
    let timeout = args::optional_seconds(&opts, "timeout")?;

    util::install_interrupt_handler();

    let stop = Arc::new(AtomicBool::new(false));
    let close_requested = Arc::new(AtomicBool::new(false));
    let sink = Arc::new(CaptureSink::new(
        format,
        opts.has("quiet"),
        stop.clone(),
        until,
    ));

    let session = ActiveSession::open(&settings, sink.clone()).map_err(Fail::runtime)?;
    let status = session.status();
    let info = status
        .info
        .clone()
        .ok_or_else(|| Fail::runtime("无法读取串口会话信息。".to_string()))?;
    let log_path = status.log.as_ref().map(|log| log.path.clone());

    let control = if opts.has("no-control") {
        None
    } else {
        match ControlServer::start(
            session.core(),
            stop.clone(),
            close_requested.clone(),
            &settings.port_name,
            log_path.clone(),
            settings.baud_rate,
        ) {
            Ok(server) => Some(server),
            Err(error) => {
                eprintln!("sdesk: {error}");
                eprintln!("sdesk: 继续采集，但本次会话无法通过 sdesk send 写入。");
                None
            }
        }
    };

    emit_session(&sink, format, &settings, &info, &log_path, control.as_ref());

    let started = Instant::now();
    let deadline = timeout.map(|timeout| started + timeout);
    let reason: &str;

    loop {
        if sink.matched() {
            reason = "until";
            break;
        }
        if close_requested.load(Ordering::SeqCst) {
            reason = "closed";
            break;
        }
        if stop.load(Ordering::SeqCst) {
            reason = "signal";
            break;
        }
        if util::interrupted() {
            reason = "signal";
            break;
        }
        if let Some(deadline) = deadline {
            if Instant::now() >= deadline {
                reason = "timeout";
                break;
            }
        }
        if sink.failure().is_some() {
            reason = "output";
            break;
        }

        let current = session.status();
        if current.fatal_error.is_some() {
            reason = "error";
            break;
        }
        if current
            .log
            .as_ref()
            .and_then(|log| log.error.as_ref())
            .is_some()
        {
            reason = "log-error";
            break;
        }

        thread::sleep(Duration::from_millis(50));
    }

    let report = session.close();
    drop(control);

    let elapsed_ms = started.elapsed().as_millis() as u64;
    emit_summary(
        &sink,
        reason,
        &settings.port_name,
        has_until,
        elapsed_ms,
        &report,
    );

    if let Some(failure) = sink.failure() {
        eprintln!("sdesk: {failure}");
    }
    if let Some(error) = report.fatal_error.as_deref() {
        eprintln!("sdesk: 串口错误：{error}");
    }
    if let Some(error) = report.log.as_ref().and_then(|log| log.error.as_deref()) {
        eprintln!("sdesk: 日志错误：{error}");
    }

    if report.fatal_error.is_some() || matches!(reason, "output" | "log-error" | "error") {
        return Ok(1);
    }
    if reason == "timeout" && has_until {
        return Ok(3);
    }
    Ok(0)
}

fn build_settings(opts: &Opts) -> CliResult<Settings> {
    let port = util::pick_port(opts.value("port"))?;
    let log_dir = util::resolve_log_dir(opts.value("log-dir"));

    let mut settings = Settings::default_with_log_dir(log_dir);
    settings.port_name = port;
    settings.baud_rate = args::parse_u32(opts, "baud", 115_200, 50, 4_000_000)?;
    settings.data_bits = args::parse_u32(opts, "data-bits", 8, 5, 8)? as u8;
    settings.parity = opts
        .value("parity")
        .unwrap_or("none")
        .trim()
        .to_ascii_lowercase();
    settings.stop_bits = opts.value("stop-bits").unwrap_or("1").trim().to_string();
    settings.flow_control = opts
        .value("flow")
        .unwrap_or("none")
        .trim()
        .to_ascii_lowercase();
    settings.auto_log = !opts.has("no-log");
    settings.log_interval_secs = match opts.value("log-interval") {
        Some(raw) => raw
            .trim()
            .parse::<u64>()
            .map_err(|_| Fail::usage(format!("--log-interval 需要一个整数，收到“{raw}”。")))?,
        None => 5,
    };

    settings.validate().map_err(Fail::usage)?;
    Ok(settings)
}

fn emit_session(
    sink: &CaptureSink,
    format: Format,
    settings: &Settings,
    info: &crate::serial::SessionInfo,
    log_path: &Option<String>,
    control: Option<&ControlServer>,
) {
    let event = json!({
        "type": "session",
        "apiVersion": 1,
        "port": settings.port_name,
        "baudRate": settings.baud_rate,
        "framing": info.framing,
        "flow": info.flow_control,
        "logPath": log_path,
        "logIntervalSecs": if settings.auto_log { Some(settings.log_interval_secs) } else { None },
        "controlPort": control.map(|server| server.port()),
        "pid": std::process::id(),
        "startedAtMs": util::now_ms(),
        "format": format_label(format),
    });

    let mut text = format!(
        "已打开 {} | {} bps | {} | flow: {}",
        settings.port_name, settings.baud_rate, info.framing, info.flow_control
    );
    match log_path {
        Some(path) => text.push_str(&format!(
            "\n日志：{path}（每 {} 秒同步）",
            settings.log_interval_secs
        )),
        None => text.push_str("\n日志：已关闭（--no-log）"),
    }
    match control {
        Some(server) => text.push_str(&format!(
            "\n控制：127.0.0.1:{}（sdesk send --port {} ...）",
            server.port(),
            settings.port_name
        )),
        None => text.push_str("\n控制：已禁用（--no-control）"),
    }
    text.push_str("\n按 Ctrl+C 结束，或用 sdesk stop --port ");
    text.push_str(&settings.port_name);
    text.push_str(" 正常收尾。");

    if let Err(error) = sink.emit_event(&event, &text) {
        // Do not abort the session: the log already has every frame. The main
        // loop notices the sink failure and closes cleanly.
        eprintln!("sdesk: {error}");
    }
}

fn format_label(format: Format) -> &'static str {
    match format {
        Format::Text => "text",
        Format::Json => "json",
        Format::Hex => "hex",
        Format::Raw => "raw",
    }
}

fn emit_summary(
    sink: &CaptureSink,
    reason: &str,
    port: &str,
    has_until: bool,
    elapsed_ms: u64,
    report: &crate::serial::CloseReport,
) {
    let event = json!({
        "type": "summary",
        "reason": reason,
        "port": port,
        "untilMatched": sink.matched(),
        "durationMs": elapsed_ms,
        "rxBytes": report.rx_bytes,
        "txBytes": report.tx_bytes,
        "rxFrames": report.rx_frames,
        "txFrames": report.tx_frames,
        "log": report.log,
        "error": report.fatal_error,
    });

    let mut text = format!(
        "已关闭 {port}：RX {} B / {} 帧，TX {} B / {} 帧，用时 {:.1} s",
        report.rx_bytes,
        report.rx_frames,
        report.tx_bytes,
        report.tx_frames,
        elapsed_ms as f64 / 1000.0
    );
    if let Some(path) = report.log.as_ref().map(|log| log.path.as_str()) {
        text.push_str(&format!("\n日志：{path}（已写入结束标记）"));
    }
    if reason == "timeout" && has_until {
        text.push_str("\n未在超时前匹配 --until。");
    }

    if let Err(error) = sink.emit_event(&event, &text) {
        eprintln!("sdesk: {error}");
    }
}

struct CaptureSink {
    writer: Mutex<FrameWriter>,
    quiet: bool,
    stop: Arc<AtomicBool>,
    failure: Mutex<Option<String>>,
    matcher: Mutex<Option<Matcher>>,
    matched: AtomicBool,
}

impl CaptureSink {
    fn new(format: Format, quiet: bool, stop: Arc<AtomicBool>, until: Option<Vec<u8>>) -> Self {
        Self {
            writer: Mutex::new(FrameWriter::new(format)),
            quiet,
            stop,
            failure: Mutex::new(None),
            matcher: Mutex::new(until.map(Matcher::new)),
            matched: AtomicBool::new(false),
        }
    }

    fn matched(&self) -> bool {
        self.matched.load(Ordering::SeqCst)
    }

    fn failure(&self) -> Option<String> {
        lock(&self.failure).clone()
    }

    fn emit_event(&self, event: &Value, text: &str) -> Result<(), String> {
        let mut writer = lock(&self.writer);
        if writer.is_json() {
            writer
                .write_event(event)
                .map_err(|error| format!("写入标准输出失败：{error}"))
        } else {
            eprintln!("{text}");
            Ok(())
        }
    }

    fn fail(&self, message: String) {
        let mut failure = lock(&self.failure);
        if failure.is_none() {
            *failure = Some(message);
        }
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl FrameSink for CaptureSink {
    fn frame(&self, kind: FrameKind, sequence: u32, timestamp_ms: i64, payload: &[u8]) {
        let is_error = kind == FrameKind::Error;

        if !self.quiet {
            let frame = Frame::live(kind, sequence, timestamp_ms, payload);
            let mut writer = lock(&self.writer);
            // Raw mode only carries RX bytes, so an error frame cannot be
            // represented there; it goes to stderr below instead.
            let visible_in_stream = !writer.is_raw();
            if let Err(error) = writer.write_frame(&frame) {
                drop(writer);
                self.fail(format!("写入标准输出失败：{error}"));
            } else if is_error && !visible_in_stream {
                eprintln!("sdesk: {}", String::from_utf8_lossy(payload));
            }
        } else if is_error {
            eprintln!("sdesk: {}", String::from_utf8_lossy(payload));
        }

        if kind == FrameKind::Rx {
            let mut matcher = lock(&self.matcher);
            if let Some(matcher) = matcher.as_mut() {
                if matcher.feed(payload) {
                    self.matched.store(true, Ordering::SeqCst);
                    self.stop.store(true, Ordering::SeqCst);
                }
            }
        }
    }
}

/// Substring matcher that also sees patterns split across two frames.
struct Matcher {
    needle: Vec<u8>,
    tail: Vec<u8>,
}

impl Matcher {
    fn new(needle: Vec<u8>) -> Self {
        Self {
            needle,
            tail: Vec::new(),
        }
    }

    fn feed(&mut self, data: &[u8]) -> bool {
        if self.needle.is_empty() {
            return true;
        }

        let mut window = Vec::with_capacity(self.tail.len() + data.len());
        window.extend_from_slice(&self.tail);
        window.extend_from_slice(data);

        let found = window
            .windows(self.needle.len())
            .any(|candidate| candidate == self.needle.as_slice());

        let keep = self.needle.len().saturating_sub(1);
        self.tail = window[window.len().saturating_sub(keep)..].to_vec();
        found
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matcher_finds_text_split_across_frames() {
        let mut matcher = Matcher::new(b"boot complete".to_vec());
        assert!(!matcher.feed(b"starting bo"));
        assert!(matcher.feed(b"ot complete\r\n"));
    }

    #[test]
    fn matcher_does_not_match_unrelated_data() {
        let mut matcher = Matcher::new(b"OK\r\n".to_vec());
        assert!(!matcher.feed(b"NO"));
        assert!(!matcher.feed(b"K"));
        assert!(matcher.feed(b"\r\n"));
    }

    #[test]
    fn matcher_keeps_enough_tail_for_the_next_frame() {
        let mut matcher = Matcher::new(b"abc".to_vec());
        assert!(!matcher.feed(b"xxab"));
        assert!(matcher.feed(b"c"));
    }
}
