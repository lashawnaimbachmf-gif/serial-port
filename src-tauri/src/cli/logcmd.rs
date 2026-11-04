//! `sdesk log` — inspection of session logs written by the GUI or by
//! `sdesk capture`. Every command reads the same documented format.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use chrono::Local;
use serde_json::{json, Value};

use super::args::{self, Opts};
use super::frame::{Dir, Format, Frame, FrameWriter};
use super::logfile::{self, LineKind, LogLine};
use super::util;
use super::{CliResult, Fail};
use crate::logging::escape_bytes;

pub const HELP: &str = "用法:
  sdesk log path [--dir DIR] [--json]
  sdesk log list [--dir DIR] [--port PORT] [--limit N] [--json]
  sdesk log show <FILE> [--latest] [筛选] [--limit N] [--offset N] [--format ...] [--json]
  sdesk log tail [<FILE> | --latest] [--lines N] [--follow] [--timeout SECS] [筛选] [--format ...] [--json]

子命令:
  path   打印日志目录（默认与 GUI 相同）
  list   列出日志文件：端口、开始时间、大小、帧数、是否正常关闭
  show   输出指定日志中的数据帧
  tail   输出最后 N 帧；--follow 持续跟踪新写入的数据

筛选:
  --direction rx|tx|both   只看某个方向
  --rx / --tx              同上，二选一
  --since <TIME>           起始时间：\"2026-10-19 12:00:00\"、\"12:00:00\" 或 \"-5m\"
  --until <TIME>           结束时间（含）
  --grep <TEXT>            在转义后的数据文本中做子串匹配
  --ignore-case            --grep 忽略大小写

输出:
  --format text|json|hex|raw   默认 text；json 为 JSONL
  --json                       等价于 --format json
  --lines <N>                  tail 的初始行数，默认 50
  --limit / --offset           show 的结果截取；list 的条数限制

时间轴以日志中记录的本地时间为准（毫秒精度）。日志格式见 docs/LOG_FORMAT.md。

退出码: 0 成功；1 文件错误；2 参数错误；3 没有匹配的数据。";

pub fn cmd_log(argv: &[String]) -> CliResult<i32> {
    let Some(sub) = argv.first() else {
        println!("{HELP}");
        return Ok(0);
    };
    let rest = &argv[1..];

    match sub.as_str() {
        "path" => cmd_path(rest),
        "list" | "ls" => cmd_list(rest),
        "show" => cmd_show(rest),
        "tail" => cmd_tail(rest),
        "-h" | "--help" | "help" => {
            println!("{HELP}");
            Ok(0)
        }
        other => Err(Fail::usage(format!(
            "未知的 log 子命令“{other}”。可用：path、list、show、tail。"
        ))),
    }
}

fn cmd_path(argv: &[String]) -> CliResult<i32> {
    let opts = args::parse(
        argv,
        &[
            ("help", false),
            ("json", false),
            ("dir", true),
            ("log-dir", true),
        ],
    )?;
    if opts.has("help") {
        println!("{HELP}");
        return Ok(0);
    }

    let dir = log_dir(&opts);
    if opts.has("json") {
        println!(
            "{}",
            json!({
                "type": "logPath",
                "path": dir.to_string_lossy(),
                "exists": dir.is_dir(),
            })
        );
    } else {
        println!("{}", dir.display());
    }
    Ok(0)
}

fn cmd_list(argv: &[String]) -> CliResult<i32> {
    let opts = args::parse(
        argv,
        &[
            ("help", false),
            ("json", false),
            ("dir", true),
            ("log-dir", true),
            ("limit", true),
            ("port", true),
        ],
    )?;
    if opts.has("help") {
        println!("{HELP}");
        return Ok(0);
    }

    let dir = log_dir(&opts);
    let files = collect_files(&dir)?;
    let filter_port = opts.value("port").map(normalize_port);
    let limit = parse_count(&opts, "limit")?.unwrap_or(u64::MAX) as usize;

    let mut metas = Vec::new();
    for file in files {
        match logfile::scan(&file) {
            Ok(meta) => metas.push(meta),
            Err(error) => eprintln!("sdesk: 跳过无法读取的日志“{}”：{error}", file.display()),
        }
    }

    if let Some(port) = filter_port {
        metas.retain(|meta| matches_port(meta, &port));
    }
    metas.sort_by_key(|meta| std::cmp::Reverse(meta.modified_ms));
    metas.truncate(limit);

    if opts.has("json") {
        let items: Vec<Value> = metas
            .iter()
            .map(|meta| {
                json!({
                    "type": "log",
                    "file": meta.name,
                    "path": meta.path.to_string_lossy(),
                    "port": meta.header.port,
                    "serial": meta.header.serial,
                    "created": meta.header.created_text,
                    "createdMs": meta.header.created_ms,
                    "size": meta.size,
                    "frames": meta.frames,
                    "rxBytes": meta.rx_bytes,
                    "txBytes": meta.tx_bytes,
                    "closed": meta.closed,
                    "invalidLines": meta.invalid,
                    "modifiedMs": meta.modified_ms,
                })
            })
            .collect();
        println!("{}", super::json_pretty(&items)?);
        return Ok(0);
    }

    if metas.is_empty() {
        if dir.is_dir() {
            println!("日志目录中没有 .log 文件：{}", dir.display());
        } else {
            println!("日志目录不存在：{}", dir.display());
        }
        return Ok(0);
    }

    println!(
        "{:<44} {:<7} {:<20} {:>9} {:>7} STATE",
        "FILE", "PORT", "STARTED", "SIZE", "FRAMES"
    );
    for meta in &metas {
        let started = meta
            .header
            .created_ms
            .map(|ms| util::format_local(ms).chars().take(19).collect::<String>())
            .unwrap_or_else(|| "-".to_string());
        let state = if meta.invalid > 0 {
            format!("invalid({})", meta.invalid)
        } else if meta.closed {
            "closed".to_string()
        } else {
            "OPEN".to_string()
        };
        println!(
            "{:<44} {:<7} {:<20} {:>9} {:>7} {}",
            meta.name,
            meta.header.port.as_deref().unwrap_or("-"),
            started,
            util::human_size(meta.size),
            meta.frames,
            state
        );
    }
    Ok(0)
}

fn cmd_show(argv: &[String]) -> CliResult<i32> {
    let opts = args::parse(
        argv,
        &[
            ("help", false),
            ("json", false),
            ("format", true),
            ("latest", false),
            ("dir", true),
            ("log-dir", true),
            ("direction", true),
            ("rx", false),
            ("tx", false),
            ("since", true),
            ("until", true),
            ("grep", true),
            ("ignore-case", false),
            ("limit", true),
            ("offset", true),
        ],
    )?;
    if opts.has("help") {
        println!("{HELP}");
        return Ok(0);
    }

    let format = Format::from_opts(&opts)?;
    let path = prepare_file(&opts)?;
    let filters = Filters::from_opts(&opts)?;
    let limit = parse_count(&opts, "limit")?;
    let offset = parse_count(&opts, "offset")?.unwrap_or(0);
    let mut writer = FrameWriter::new(format);

    let file = File::open(&path)
        .map_err(|error| Fail::runtime(format!("无法打开日志“{}”：{error}", path.display())))?;
    let mut reader = BufReader::new(file);
    let (_header, pending) = logfile::read_header(&mut reader)
        .map_err(|error| Fail::runtime(format!("读取日志失败：{error}")))?;

    let mut window = ShowWindow {
        matched: 0,
        emitted: 0,
        offset,
        limit,
        invalid: 0,
    };
    let mut stop = false;

    if let Some(line) = pending {
        stop = process_show_line(&line, &filters, &mut writer, &mut window)?;
    }

    let mut line = String::new();
    while !stop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|error| Fail::runtime(format!("读取日志失败：{error}")))?;
        if read == 0 {
            break;
        }
        stop = process_show_line(
            line.trim_end_matches(['\r', '\n']),
            &filters,
            &mut writer,
            &mut window,
        )?;
    }

    writer
        .flush()
        .map_err(|error| Fail::runtime(format!("写入标准输出失败：{error}")))?;
    if window.invalid > 0 {
        eprintln!("sdesk: 跳过 {} 行无法解析的日志。", window.invalid);
    }
    if window.emitted == 0 {
        return Ok(3);
    }
    Ok(0)
}

fn cmd_tail(argv: &[String]) -> CliResult<i32> {
    let opts = args::parse(
        argv,
        &[
            ("help", false),
            ("json", false),
            ("format", true),
            ("latest", false),
            ("dir", true),
            ("log-dir", true),
            ("direction", true),
            ("rx", false),
            ("tx", false),
            ("since", true),
            ("until", true),
            ("grep", true),
            ("ignore-case", false),
            ("lines", true),
            ("follow", false),
            ("timeout", true),
        ],
    )?;
    if opts.has("help") {
        println!("{HELP}");
        return Ok(0);
    }

    let follow = opts.has("follow");
    let timeout = args::optional_seconds(&opts, "timeout")?;
    if timeout.is_some() && !follow {
        return Err(Fail::usage(
            "--timeout 只用于 --follow；否则请用 --lines 读取固定行数。".to_string(),
        ));
    }

    let format = Format::from_opts(&opts)?;
    let path = prepare_file(&opts)?;
    let filters = Filters::from_opts(&opts)?;
    let lines = parse_count(&opts, "lines")?.unwrap_or(50) as usize;
    let mut writer = FrameWriter::new(format);
    let mut invalid = 0u64;
    let mut emitted = 0u64;

    let (initial, mut position, initial_invalid) = tail_frames(&path, lines, &filters)?;
    invalid += initial_invalid;
    for frame in initial {
        write_log_frame(&mut writer, frame)?;
        emitted += 1;
    }

    if follow {
        let deadline = timeout.map(|timeout| Instant::now() + timeout);
        loop {
            if util::interrupted() {
                break;
            }
            if let Some(deadline) = deadline {
                if Instant::now() >= deadline {
                    break;
                }
            }
            thread::sleep(Duration::from_millis(100));

            let (new_lines, new_position) = logfile::read_from(&path, position)
                .map_err(|error| Fail::runtime(format!("跟踪日志失败：{error}")))?;
            position = new_position;
            for line in &new_lines {
                process_frame_line(line, &filters, &mut writer, &mut emitted, &mut invalid)?;
            }
        }
    }

    writer
        .flush()
        .map_err(|error| Fail::runtime(format!("写入标准输出失败：{error}")))?;
    if invalid > 0 {
        eprintln!("sdesk: 跳过 {invalid} 行无法解析的日志。");
    }
    if emitted == 0 && !follow {
        return Ok(3);
    }
    Ok(0)
}

/// Reads the last `count` frames rather than the last `count` lines.
///
/// Log files end with metadata (`# ---`, `# closed`), and filters can discard
/// many lines, so a fixed line window would often return nothing useful. The
/// window grows until it holds enough frames or reaches the start of the file.
fn tail_frames(
    path: &Path,
    count: usize,
    filters: &Filters,
) -> CliResult<(Vec<LogLine>, u64, u64)> {
    if count == 0 {
        let (_, end) = logfile::tail_lines_with_end(path, 0)
            .map_err(|error| Fail::runtime(format!("读取日志失败：{error}")))?;
        return Ok((Vec::new(), end, 0));
    }

    let mut wanted = count.max(16);
    loop {
        let (lines, end) = logfile::tail_lines_with_end(path, wanted)
            .map_err(|error| Fail::runtime(format!("读取日志失败：{error}")))?;

        let mut frames: Vec<LogLine> = Vec::new();
        let mut invalid = 0u64;
        for line in &lines {
            match logfile::parse_line(line) {
                LineKind::Frame(frame) => {
                    if filters.matches(&frame) {
                        frames.push(frame);
                    }
                }
                LineKind::Invalid => invalid += 1,
                LineKind::Meta => {}
            }
        }

        let reached_start = lines.len() < wanted;
        if frames.len() >= count {
            let start = frames.len() - count;
            frames.drain(..start);
            return Ok((frames, end, invalid));
        }
        if reached_start {
            return Ok((frames, end, invalid));
        }
        wanted = wanted.saturating_mul(4);
    }
}

struct ShowWindow {
    matched: u64,
    emitted: u64,
    offset: u64,
    limit: Option<u64>,
    invalid: u64,
}

fn process_show_line(
    line: &str,
    filters: &Filters,
    writer: &mut FrameWriter,
    window: &mut ShowWindow,
) -> CliResult<bool> {
    let frame = match logfile::parse_line(line) {
        LineKind::Frame(frame) => frame,
        LineKind::Invalid => {
            window.invalid += 1;
            return Ok(false);
        }
        LineKind::Meta => return Ok(false),
    };

    if !filters.matches(&frame) {
        return Ok(false);
    }
    window.matched += 1;
    if window.matched <= window.offset {
        return Ok(false);
    }
    if let Some(limit) = window.limit {
        if window.emitted >= limit {
            return Ok(true);
        }
    }

    write_log_frame(writer, frame)?;
    window.emitted += 1;
    Ok(window
        .limit
        .map(|limit| window.emitted >= limit)
        .unwrap_or(false))
}

fn process_frame_line(
    line: &str,
    filters: &Filters,
    writer: &mut FrameWriter,
    emitted: &mut u64,
    invalid: &mut u64,
) -> CliResult<()> {
    let frame = match logfile::parse_line(line) {
        LineKind::Frame(frame) => frame,
        LineKind::Invalid => {
            *invalid += 1;
            return Ok(());
        }
        LineKind::Meta => return Ok(()),
    };
    if !filters.matches(&frame) {
        return Ok(());
    }
    write_log_frame(writer, frame)?;
    *emitted += 1;
    Ok(())
}

fn write_log_frame(writer: &mut FrameWriter, frame: LogLine) -> CliResult<()> {
    let output = Frame::from_parts(frame.ts_ms, frame.ts, frame.dir, frame.bytes);
    writer
        .write_frame(&output)
        .map_err(|error| Fail::runtime(format!("写入标准输出失败：{error}")))
}

struct Filters {
    direction: Option<Dir>,
    since: Option<i64>,
    until: Option<i64>,
    grep: Option<String>,
    ignore_case: bool,
}

impl Filters {
    fn from_opts(opts: &Opts) -> CliResult<Self> {
        let direction = if opts.has("rx") || opts.has("tx") {
            if opts.has("rx") && opts.has("tx") {
                return Err(Fail::usage("--rx 与 --tx 不能同时使用。".to_string()));
            }
            Some(if opts.has("rx") { Dir::Rx } else { Dir::Tx })
        } else {
            match opts.value("direction") {
                None => None,
                Some(raw) => match raw.trim().to_ascii_lowercase().as_str() {
                    "rx" => Some(Dir::Rx),
                    "tx" => Some(Dir::Tx),
                    "both" | "all" => None,
                    other => {
                        return Err(Fail::usage(format!(
                            "未知方向“{other}”。可选：rx、tx、both。"
                        )));
                    }
                },
            }
        };

        let now = Local::now();
        let since = opts
            .value("since")
            .map(|raw| args::parse_time_spec(raw, now))
            .transpose()
            .map_err(|error| Fail::usage(format!("--since: {error}")))?;
        let until = opts
            .value("until")
            .map(|raw| args::parse_time_spec(raw, now))
            .transpose()
            .map_err(|error| Fail::usage(format!("--until: {error}")))?;

        Ok(Self {
            direction,
            since,
            until,
            grep: opts.value("grep").map(str::to_string),
            ignore_case: opts.has("ignore-case"),
        })
    }

    fn matches(&self, frame: &LogLine) -> bool {
        if let Some(direction) = self.direction {
            if frame.dir != direction {
                return false;
            }
        }
        if let Some(since) = self.since {
            if frame.ts_ms < since {
                return false;
            }
        }
        if let Some(until) = self.until {
            if frame.ts_ms > until {
                return false;
            }
        }
        if let Some(pattern) = &self.grep {
            let haystack = escape_bytes(&frame.bytes);
            let found = if self.ignore_case {
                haystack.to_lowercase().contains(&pattern.to_lowercase())
            } else {
                haystack.contains(pattern.as_str())
            };
            if !found {
                return false;
            }
        }
        true
    }
}

fn log_dir(opts: &Opts) -> PathBuf {
    util::resolve_log_dir(opts.value("dir").or_else(|| opts.value("log-dir")))
}

fn prepare_file(opts: &Opts) -> CliResult<PathBuf> {
    let explicit = opts.positional().first();
    match (explicit, opts.has("latest")) {
        (Some(_), true) => Err(Fail::usage(
            "文件参数与 --latest 不能同时使用。".to_string(),
        )),
        (Some(path), false) => Ok(PathBuf::from(path)),
        (None, true) => latest_log(&log_dir(opts)),
        (None, false) => Err(Fail::usage(
            "需要日志文件参数，或使用 --latest 选择最新的日志。".to_string(),
        )),
    }
}

fn latest_log(dir: &Path) -> CliResult<PathBuf> {
    let files = collect_files(dir)?;
    let latest = files
        .into_iter()
        .filter_map(|path| {
            let modified = std::fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0);
            let name = path.file_name()?.to_string_lossy().into_owned();
            Some((modified, name, path))
        })
        .max_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)))
        .map(|(_, _, path)| path);

    latest.ok_or_else(|| {
        Fail::nothing_found(format!("日志目录中没有 .log 文件：{}。", dir.display()))
    })
}

fn collect_files(dir: &Path) -> CliResult<Vec<PathBuf>> {
    match logfile::log_files(dir) {
        Ok(files) => Ok(files),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(Fail::runtime(format!(
            "无法读取日志目录“{}”：{error}",
            dir.display()
        ))),
    }
}

fn matches_port(meta: &logfile::LogMeta, wanted: &str) -> bool {
    if let Some(port) = meta.header.port.as_deref() {
        return port.eq_ignore_ascii_case(wanted);
    }
    meta.name
        .to_ascii_uppercase()
        .starts_with(&format!("{}_", wanted.to_ascii_uppercase()))
}

fn normalize_port(raw: &str) -> String {
    let trimmed = raw.trim();
    if !trimmed.is_empty() && trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        format!("COM{trimmed}")
    } else {
        trimmed.to_string()
    }
}

fn parse_count(opts: &Opts, name: &str) -> CliResult<Option<u64>> {
    let Some(raw) = opts.value(name) else {
        return Ok(None);
    };
    let value: u64 = raw
        .trim()
        .parse()
        .map_err(|_| Fail::usage(format!("--{name} 需要一个非负整数，收到“{raw}”。")))?;
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(argv: &[&str]) -> Opts {
        let argv: Vec<String> = argv.iter().map(|value| value.to_string()).collect();
        args::parse(
            &argv,
            &[
                ("direction", true),
                ("rx", false),
                ("tx", false),
                ("since", true),
                ("until", true),
                ("grep", true),
                ("ignore-case", false),
                ("json", false),
                ("format", true),
            ],
        )
        .unwrap()
    }

    fn frame(dir: Dir, text: &str) -> LogLine {
        LogLine {
            ts_ms: 1_000,
            ts: "2026-10-19 12:00:01.000".to_string(),
            dir,
            bytes: text.as_bytes().to_vec(),
        }
    }

    #[test]
    fn direction_filters_match_the_requested_direction() {
        let filters = Filters::from_opts(&opts(&["--direction", "rx"])).unwrap();
        assert!(filters.matches(&frame(Dir::Rx, "OK")));
        assert!(!filters.matches(&frame(Dir::Tx, "OK")));
    }

    #[test]
    fn grep_matches_escaped_text_and_honours_case() {
        let filters = Filters::from_opts(&opts(&["--grep", "error"])).unwrap();
        assert!(!filters.matches(&frame(Dir::Rx, "ERROR 3")));
        let filters = Filters::from_opts(&opts(&["--grep", "ERROR", "--ignore-case"])).unwrap();
        assert!(filters.matches(&frame(Dir::Rx, "error 3")));
    }

    #[test]
    fn relative_time_filter_matches_recent_frames() {
        let now = Local::now();
        let filters = Filters::from_opts(&opts(&["--since", "-1h"])).unwrap();
        let mut recent = frame(Dir::Rx, "OK");
        recent.ts_ms = now.timestamp_millis() - 1_000;
        assert!(filters.matches(&recent));
        let mut old = frame(Dir::Rx, "OK");
        old.ts_ms = now.timestamp_millis() - 2 * 3600 * 1000;
        assert!(!filters.matches(&old));
    }

    #[test]
    fn rx_and_tx_together_is_rejected() {
        assert!(Filters::from_opts(&opts(&["--rx", "--tx"])).is_err());
    }
}
