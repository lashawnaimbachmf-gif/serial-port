//! Hand-rolled argument parsing and value decoding.
//!
//! The CLI deliberately avoids a dependency on an argument-parsing crate:
//! the build must stay offline-friendly, and an AI agent benefits from errors
//! that name the exact option that is wrong.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Local, LocalResult, NaiveDateTime, NaiveTime, TimeZone};

use super::{CliResult, Fail};

pub struct Opts {
    flags: HashMap<&'static str, bool>,
    values: HashMap<&'static str, String>,
    positional: Vec<String>,
}

impl Opts {
    pub fn has(&self, name: &str) -> bool {
        self.flags.contains_key(name)
    }

    pub fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    pub fn positional(&self) -> &[String] {
        &self.positional
    }

    pub fn get_port(&self) -> Option<String> {
        self.value("port").map(str::to_string)
    }
}

/// Parses `--name value`, `--name=value` and `--flag`.
///
/// `spec` lists every accepted option and whether it takes a value. Anything
/// else is an error, so a typo can never be silently ignored.
pub fn parse(argv: &[String], spec: &[(&'static str, bool)]) -> CliResult<Opts> {
    let mut flags = HashMap::new();
    let mut values: HashMap<&'static str, String> = HashMap::new();
    let mut positional = Vec::new();
    let mut literal = false;
    let mut index = 0usize;

    while index < argv.len() {
        let token = &argv[index];
        index += 1;

        if literal {
            positional.push(token.clone());
            continue;
        }
        if token == "--" {
            literal = true;
            continue;
        }
        if !token.starts_with("--") {
            if token.starts_with('-') && token.len() > 1 {
                return Err(Fail::usage(format!(
                    "未知选项“{token}”。选项必须以 -- 开头，或运行 sdesk help <命令>。"
                )));
            }
            positional.push(token.clone());
            continue;
        }

        let body = &token[2..];
        let (name, inline) = match body.split_once('=') {
            Some((name, value)) => (name, Some(value.to_string())),
            None => (body, None),
        };
        let Some((spec_name, takes_value)) = spec.iter().find(|(known, _)| *known == name) else {
            return Err(Fail::usage(format!("未知选项“--{name}”。")));
        };
        let spec_name = *spec_name;

        if *takes_value {
            let value = match inline {
                Some(value) => value,
                None => {
                    if index >= argv.len() {
                        return Err(Fail::usage(format!("选项“--{name}”需要一个值。")));
                    }
                    let value = argv[index].clone();
                    index += 1;
                    value
                }
            };
            if value.is_empty() {
                return Err(Fail::usage(format!("选项“--{name}”的值不能为空。")));
            }
            if values.insert(spec_name, value).is_some() {
                return Err(Fail::usage(format!("选项“--{name}”重复出现。")));
            }
        } else {
            if inline.is_some() {
                return Err(Fail::usage(format!("选项“--{name}”不接受值。")));
            }
            flags.insert(spec_name, true);
        }
    }

    Ok(Opts {
        flags,
        values,
        positional,
    })
}

/// Decodes the escape syntax used by `--data` and `--until`.
///
/// `\\`, `\r`, `\n`, `\t`, `\0` and `\xNN` are recognised. An unknown escape
/// is an error: silently sending a different byte than the caller asked for is
/// worse than refusing to send.
pub fn decode_text(input: &str) -> Result<Vec<u8>, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut output = Vec::with_capacity(input.len());
    let mut index = 0usize;

    while index < chars.len() {
        let ch = chars[index];
        if ch != '\\' {
            let mut buffer = [0u8; 4];
            output.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
            index += 1;
            continue;
        }

        index += 1;
        let Some(marker) = chars.get(index).copied() else {
            return Err("文本以反斜杠结尾；字面反斜杠请写成 \\\\。".to_string());
        };
        match marker {
            '\\' => {
                output.push(b'\\');
                index += 1;
            }
            'r' => {
                output.push(b'\r');
                index += 1;
            }
            'n' => {
                output.push(b'\n');
                index += 1;
            }
            't' => {
                output.push(b'\t');
                index += 1;
            }
            '0' => {
                output.push(0);
                index += 1;
            }
            'x' => {
                let high = chars.get(index + 1).and_then(|ch| ch.to_digit(16));
                let low = chars.get(index + 2).and_then(|ch| ch.to_digit(16));
                let (Some(high), Some(low)) = (high, low) else {
                    return Err("转义 \\x 后必须跟两位十六进制数字，例如 \\x0D。".to_string());
                };
                output.push(((high << 4) | low) as u8);
                index += 3;
            }
            other => {
                return Err(format!(
                    "未知转义“\\{other}”。支持 \\\\ \\r \\n \\t \\0 \\xNN。"
                ));
            }
        }
    }

    Ok(output)
}

/// Strict hexadecimal payload: hex digits and whitespace only, even length.
pub fn parse_hex(input: &str) -> Result<Vec<u8>, String> {
    let mut nibbles = Vec::with_capacity(input.len() / 2);
    for (offset, ch) in input.chars().enumerate() {
        if ch.is_whitespace() {
            continue;
        }
        let Some(digit) = ch.to_digit(16) else {
            return Err(format!(
                "HEX 数据在第 {} 个字符处包含非法字符“{ch}”。只允许 0-9、A-F 和空白。",
                offset + 1
            ));
        };
        nibbles.push(digit as u8);
    }

    if nibbles.is_empty() {
        return Ok(Vec::new());
    }
    if nibbles.len() % 2 != 0 {
        return Err(format!(
            "HEX 数据必须是偶数位（当前 {} 位）。",
            nibbles.len()
        ));
    }

    let mut output = Vec::with_capacity(nibbles.len() / 2);
    for pair in nibbles.chunks_exact(2) {
        output.push((pair[0] << 4) | pair[1]);
    }
    Ok(output)
}

/// Parses a duration such as `30`, `500ms`, `1.5s`, `2m`.
pub fn parse_seconds(raw: &str) -> Result<Duration, String> {
    let text = raw.trim();
    if text.is_empty() {
        return Err("时间不能为空。".to_string());
    }
    let (number, factor) = if let Some(rest) = text.strip_suffix("ms") {
        (rest.trim(), 0.001f64)
    } else if let Some(rest) = text.strip_suffix('s') {
        (rest.trim(), 1.0f64)
    } else if let Some(rest) = text.strip_suffix('m') {
        (rest.trim(), 60.0f64)
    } else if let Some(rest) = text.strip_suffix('h') {
        (rest.trim(), 3600.0f64)
    } else {
        (text, 1.0f64)
    };

    let value: f64 = number
        .parse()
        .map_err(|_| format!("无法解析时间“{raw}”。示例：30、500ms、2m。"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(format!("时间“{raw}”无效。"));
    }
    let seconds = value * factor;
    if seconds > 7.0 * 24.0 * 3600.0 {
        return Err(format!("时间“{raw}”过长（上限 7 天）。"));
    }
    Ok(Duration::from_secs_f64(seconds))
}

/// Parses an absolute timestamp, `HH:MM:SS`, or a relative `-5m` / `-30s`.
pub fn parse_time_spec(raw: &str, now: DateTime<Local>) -> Result<i64, String> {
    let text = raw.trim();
    if text.is_empty() {
        return Err("时间不能为空。".to_string());
    }

    if let Some(rest) = text.strip_prefix('-') {
        let duration = parse_seconds(rest)?;
        return Ok(now.timestamp_millis() - duration.as_millis() as i64);
    }

    if let Ok(parsed) = DateTime::parse_from_rfc3339(text) {
        return Ok(parsed.timestamp_millis());
    }

    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(text, format) {
            return local_millis(naive, now)
                .ok_or_else(|| format!("本地时间“{raw}”不存在（夏令时跳变）。"));
        }
    }

    for format in ["%H:%M:%S%.f", "%H:%M:%S"] {
        if let Ok(time) = NaiveTime::parse_from_str(text, format) {
            let naive = now.date_naive().and_time(time);
            return local_millis(naive, now)
                .ok_or_else(|| format!("本地时间“{raw}”不存在（夏令时跳变）。"));
        }
    }

    Err(format!(
        "无法解析时间“{raw}”。支持 “2026-10-19 12:00:00”、“12:00:00” 和相对写法 “-5m”。"
    ))
}

fn local_millis(naive: NaiveDateTime, now: DateTime<Local>) -> Option<i64> {
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(value) => Some(value.timestamp_millis()),
        LocalResult::Ambiguous(earlier, _) => Some(earlier.timestamp_millis()),
        // A time inside a DST gap does not exist; fall back to interpreting
        // the wall clock against the current offset rather than dropping it.
        LocalResult::None => Some(
            naive
                .and_local_timezone(*now.offset())
                .single()?
                .timestamp_millis(),
        ),
    }
}

pub fn optional_seconds(opts: &Opts, name: &str) -> CliResult<Option<Duration>> {
    match opts.value(name) {
        Some(raw) => parse_seconds(raw)
            .map(Some)
            .map_err(|error| Fail::usage(format!("--{name}: {error}"))),
        None => Ok(None),
    }
}

/// Reads a bounded integer option, rejecting rather than clamping bad values.
pub fn parse_u32(opts: &Opts, name: &str, default: u32, min: u32, max: u32) -> CliResult<u32> {
    let Some(raw) = opts.value(name) else {
        return Ok(default);
    };
    let value: u32 = raw
        .trim()
        .parse()
        .map_err(|_| Fail::usage(format!("--{name} 需要一个数字，收到“{raw}”。")))?;
    if value < min || value > max {
        return Err(Fail::usage(format!(
            "--{name} 需在 {min}–{max} 之间，收到 {value}。"
        )));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_escapes_and_utf8() {
        assert_eq!(decode_text("AT\\r\\n").unwrap(), b"AT\r\n");
        assert_eq!(decode_text("\\x00\\xFF").unwrap(), vec![0x00, 0xFF]);
        assert_eq!(decode_text("中文").unwrap(), "中文".as_bytes());
        assert_eq!(decode_text("a\\\\b").unwrap(), b"a\\b");
        assert_eq!(decode_text("\\0").unwrap(), vec![0]);
    }

    #[test]
    fn rejects_unknown_or_truncated_escapes() {
        assert!(decode_text("\\q").is_err());
        assert!(decode_text("tail\\").is_err());
        assert!(decode_text("\\x1").is_err());
        assert!(decode_text("\\xGG").is_err());
    }

    #[test]
    fn hex_requires_even_digits_and_hex_characters() {
        assert_eq!(parse_hex("41 54 0d 0a").unwrap(), b"AT\r\n");
        assert!(parse_hex("4").is_err());
        assert!(parse_hex("GG").is_err());
        assert!(parse_hex("   ").unwrap().is_empty());
    }

    #[test]
    fn parses_durations_with_units() {
        assert_eq!(parse_seconds("30").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_seconds("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_seconds("2m").unwrap(), Duration::from_secs(120));
        assert!(parse_seconds("soon").is_err());
        assert!(parse_seconds("-1").is_err());
    }

    #[test]
    fn parses_option_forms_and_rejects_unknown() {
        let argv: Vec<String> = vec![
            "--port".into(),
            "COM10".into(),
            "--json".into(),
            "--baud=9600".into(),
        ];
        let opts = parse(&argv, &[("port", true), ("json", false), ("baud", true)]).unwrap();
        assert_eq!(opts.value("port"), Some("COM10"));
        assert!(opts.has("json"));
        assert_eq!(opts.value("baud"), Some("9600"));

        assert!(parse(&argv, &[("port", true)]).is_err());
        assert!(parse(&["--json=1".into()], &[("json", false)]).is_err());
        assert!(parse(&["-x".into()], &[]).is_err());
    }

    #[test]
    fn parses_absolute_and_relative_times() {
        let now = Local
            .with_ymd_and_hms(2026, 10, 19, 14, 30, 0)
            .single()
            .unwrap();
        let absolute = parse_time_spec("2026-10-19 12:00:00", now).unwrap();
        assert!(absolute < now.timestamp_millis());
        let relative = parse_time_spec("-5m", now).unwrap();
        assert_eq!(relative, now.timestamp_millis() - 300_000);
        assert!(parse_time_spec("not a time", now).is_err());
    }

    #[test]
    fn positional_arguments_after_double_dash_are_not_options() {
        let argv: Vec<String> = vec!["--".into(), "--weird-name".into()];
        let opts = parse(&argv, &[]).unwrap();
        assert_eq!(opts.positional(), ["--weird-name"]);
    }
}
