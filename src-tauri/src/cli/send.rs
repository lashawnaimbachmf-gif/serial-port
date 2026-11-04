//! `sdesk send` — delivers bytes through a running capture session, or opens
//! the port directly when no capture owns it.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use serde_json::json;
use serialport::{DataBits, FlowControl, Parity, StopBits};

use super::args::{self, Opts};
use super::control::{self, SessionState};
use super::frame;
use super::util;
use super::{CliResult, Fail};

const SEND_LIMIT_BYTES: usize = 1024 * 1024;

pub const HELP: &str =
    "用法: sdesk send [--port <PORT>] (--data <TEXT> | --data-hex <HEX> | --file <PATH>)
                      [--line-ending none|cr|lf|crlf] [--direct] [--json]

向正在运行的 capture 会话发送数据；如果没有会话，则直接打开串口发送后关闭。

数据来源（三选一）:
  --data <TEXT>      文本，支持转义：\\\\ \\r \\n \\t \\0 \\xNN，例如 \"AT\\r\\n\"
  --data-hex <HEX>   十六进制，只允许 0-9 A-F 与空白，必须偶数位
  --file <PATH>      从文件读取原始字节

选项:
  --port <PORT>      目标串口；只有一个会话时可省略
  --session <FILE>   直接指定会话状态文件
  --line-ending ...  在数据末尾追加结束符，默认 none
  --direct           跳过会话，直接打开串口发送（设备可能因 DTR 复位）
  --delay-ms <N>     直接模式下打开串口后等待 N 毫秒再写
  --baud <N>         直接模式的波特率，默认 115200
  --data-bits <5-8>  默认 8
  --parity <none|even|odd>   默认 none
  --stop-bits <1|2>  默认 1
  --flow <none|hardware|software>  默认 none
  --json             输出 JSON

退出码: 0 成功；1 串口/会话错误；2 参数错误。";

pub fn cmd_send(argv: &[String]) -> CliResult<i32> {
    let opts = args::parse(
        argv,
        &[
            ("help", false),
            ("port", true),
            ("session", true),
            ("data", true),
            ("data-hex", true),
            ("file", true),
            ("line-ending", true),
            ("delay-ms", true),
            ("direct", false),
            ("json", false),
            ("baud", true),
            ("data-bits", true),
            ("parity", true),
            ("stop-bits", true),
            ("flow", true),
        ],
    )?;
    if opts.has("help") {
        println!("{HELP}");
        return Ok(0);
    }

    let payload = build_payload(&opts)?;
    let port = opts.get_port();

    if !opts.has("direct") {
        if let Some(state) = locate_session(&opts, port.as_deref())? {
            let hex = frame::hex_string(&payload).replace(' ', "");
            match control::request(&state, "send", Some(&hex)) {
                Ok(value) if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true) => {
                    return report(&opts, "session", &state.port, &payload, Some(&value), true);
                }
                Ok(value) => {
                    let message = value
                        .get("error")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("会话拒绝发送")
                        .to_string();
                    return Err(Fail::runtime(message));
                }
                Err(error) => {
                    eprintln!(
                        "sdesk: 无法联系 {} 的 capture 会话（{error}）。",
                        state.port
                    );
                    if !matches!(
                        control::request(&state, "ping", None),
                        Ok(ref value) if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true)
                    ) {
                        eprintln!(
                            "sdesk: 该会话可能已失效；可用 sdesk sessions --prune 清理，现尝试直接打开串口。"
                        );
                    }
                }
            }
        }
    }

    direct_send(&opts, port.as_deref(), &payload)
}

fn build_payload(opts: &Opts) -> CliResult<Vec<u8>> {
    let sources = [
        opts.value("data").is_some(),
        opts.value("data-hex").is_some(),
        opts.value("file").is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count();

    if sources == 0 {
        return Err(Fail::usage(
            "需要 --data、--data-hex 或 --file 之一来指定发送内容。".to_string(),
        ));
    }
    if sources > 1 {
        return Err(Fail::usage(
            "--data、--data-hex、--file 只能指定一个。".to_string(),
        ));
    }

    let mut bytes = if let Some(text) = opts.value("data") {
        args::decode_text(text).map_err(|error| Fail::usage(format!("--data: {error}")))?
    } else if let Some(hex) = opts.value("data-hex") {
        args::parse_hex(hex).map_err(|error| Fail::usage(format!("--data-hex: {error}")))?
    } else {
        let path = opts.value("file").unwrap_or_default();
        fs::read(Path::new(path))
            .map_err(|error| Fail::runtime(format!("无法读取文件“{path}”：{error}")))?
    };

    let ending = opts.value("line-ending").unwrap_or("none");
    match ending {
        "none" => {}
        "cr" => bytes.push(b'\r'),
        "lf" => bytes.push(b'\n'),
        "crlf" => bytes.extend_from_slice(b"\r\n"),
        other => {
            return Err(Fail::usage(format!(
                "未知结束符“{other}”。可选：none、cr、lf、crlf。"
            )));
        }
    }

    if bytes.is_empty() {
        return Err(Fail::usage("发送内容为空。".to_string()));
    }
    if bytes.len() > SEND_LIMIT_BYTES {
        return Err(Fail::usage(format!(
            "单次发送不能超过 {} KB（当前 {} 字节）。",
            SEND_LIMIT_BYTES / 1024,
            bytes.len()
        )));
    }
    Ok(bytes)
}

fn locate_session(opts: &Opts, port: Option<&str>) -> CliResult<Option<SessionState>> {
    if let Some(file) = opts.value("session") {
        let state = control::load_state_file(Path::new(file)).map_err(Fail::runtime)?;
        return Ok(Some(state));
    }

    let states = control::read_states();
    let wanted = port.map(normalize_port);
    let mut matched: Vec<SessionState> = match &wanted {
        Some(port) => states
            .into_iter()
            .map(|(_, state)| state)
            .filter(|state| state.port.eq_ignore_ascii_case(port))
            .collect(),
        None => states.into_iter().map(|(_, state)| state).collect(),
    };

    match matched.len() {
        0 => Ok(None),
        1 => Ok(matched.pop()),
        _ => {
            let ports: Vec<String> = matched.iter().map(|state| state.port.clone()).collect();
            Err(Fail::usage(format!(
                "检测到多个运行中的会话，请用 --port 指定：{}。",
                ports.join("、")
            )))
        }
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

fn direct_send(opts: &Opts, port: Option<&str>, payload: &[u8]) -> CliResult<i32> {
    let port = util::pick_port(port)?;
    let baud_rate = args::parse_u32(opts, "baud", 115_200, 50, 4_000_000)?;
    let data_bits = match args::parse_u32(opts, "data-bits", 8, 5, 8)? {
        5 => DataBits::Five,
        6 => DataBits::Six,
        7 => DataBits::Seven,
        8 => DataBits::Eight,
        _ => unreachable!(),
    };
    let parity = match opts.value("parity").unwrap_or("none") {
        "none" => Parity::None,
        "even" => Parity::Even,
        "odd" => Parity::Odd,
        other => {
            return Err(Fail::usage(format!(
                "未知校验位“{other}”。可选：none、even、odd。"
            )));
        }
    };
    let stop_bits = match opts.value("stop-bits").unwrap_or("1") {
        "1" => StopBits::One,
        "2" => StopBits::Two,
        other => {
            return Err(Fail::usage(format!("未知停止位“{other}”。可选：1、2。")));
        }
    };
    let flow = match opts.value("flow").unwrap_or("none") {
        "none" => FlowControl::None,
        "hardware" => FlowControl::Hardware,
        "software" => FlowControl::Software,
        other => {
            return Err(Fail::usage(format!(
                "未知流控“{other}”。可选：none、hardware、software。"
            )));
        }
    };
    let delay_ms = args::parse_u32(opts, "delay-ms", 0, 0, 60_000)?;

    let mut handle = serialport::new(&port, baud_rate)
        .data_bits(data_bits)
        .parity(parity)
        .stop_bits(stop_bits)
        .flow_control(flow)
        .timeout(Duration::from_secs(5))
        .open_native()
        .map_err(|error| {
            let detail = error
                .to_string()
                .trim_end_matches(|ch: char| ch.is_whitespace() || ch == '。' || ch == '.')
                .to_string();
            Fail::runtime(format!(
                "无法打开串口 {port}：{detail}。若已经有 capture 在运行，请去掉 --direct，或先运行 sdesk stop --port {port}。"
            ))
        })?;

    if delay_ms > 0 {
        std::thread::sleep(Duration::from_millis(delay_ms as u64));
    }

    let mut written = 0usize;
    while written < payload.len() {
        match handle.write(&payload[written..]) {
            Ok(0) => {
                return Err(Fail::runtime(format!(
                    "串口驱动未接受任何字节（已写入 {written}/{}）。",
                    payload.len()
                )));
            }
            Ok(count) => written += count,
            Err(ref error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return Err(Fail::runtime(format!(
                    "发送失败，已写入 {written}/{} 字节：{error}",
                    payload.len()
                )));
            }
        }
    }

    handle
        .flush()
        .map_err(|error| Fail::runtime(format!("数据已交给驱动，但刷新串口失败：{error}")))?;
    drop(handle);

    eprintln!("sdesk: 直接打开串口发送；打开动作可能改变 DTR/RTS 电平并使开发板复位。");
    report(opts, "direct", &port, payload, None, false)
}

fn report(
    opts: &Opts,
    mode: &str,
    port: &str,
    payload: &[u8],
    response: Option<&serde_json::Value>,
    logged: bool,
) -> CliResult<i32> {
    if opts.has("json") {
        let mut value = json!({
            "type": "send",
            "ok": true,
            "mode": mode,
            "port": port,
            "bytes": payload.len(),
            "hex": frame::hex_string(payload),
            "logged": logged,
        });
        if let Some(response) = response {
            value["txBytesTotal"] = response["txBytesTotal"].clone();
            value["timestampMs"] = response["timestampMs"].clone();
        }
        println!("{}", super::json_line(&value)?);
    } else {
        println!(
            "已发送 {} 字节到 {}（{}）：{}",
            payload.len(),
            port,
            if logged { "会话" } else { "直接" },
            frame::hex_string(payload)
        );
    }
    Ok(0)
}
