//! Implementation of the `sdesk` command line interface.
//!
//! Design goals, in order:
//!
//! 1. an AI agent can discover what is available and parse the result without
//!    screenshots or a GUI;
//! 2. byte handling is identical to the GUI (same session core, same log
//!    format, same escaping);
//! 3. text output stays readable for a human reading a terminal.

mod args;
mod capture;
mod control;
mod frame;
mod logcmd;
mod logfile;
mod send;
mod util;

/// A user-facing failure with a stable process exit code.
#[derive(Debug)]
pub struct Fail {
    message: String,
    code: i32,
}

impl Fail {
    /// Runtime failure: bad hardware, missing file, broken pipe. Exit 1.
    pub fn runtime(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: 1,
        }
    }

    /// The command was called incorrectly. Exit 2.
    pub fn usage(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: 2,
        }
    }

    /// The operation succeeded but produced no matching data. Exit 3.
    pub fn nothing_found(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: 3,
        }
    }
}

impl From<String> for Fail {
    fn from(message: String) -> Self {
        Self::runtime(message)
    }
}

pub type CliResult<T> = Result<T, Fail>;

pub fn run() -> i32 {
    util::init_console();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match dispatch(&argv) {
        Ok(code) => code,
        Err(fail) => {
            eprintln!("sdesk: {}", fail.message);
            fail.code
        }
    }
}

fn dispatch(argv: &[String]) -> CliResult<i32> {
    let Some(command) = argv.first() else {
        print_root_help();
        return Ok(0);
    };
    let rest = &argv[1..];

    if matches!(command.as_str(), "-h" | "--help" | "help") {
        match rest.first() {
            Some(topic) => print_command_help(topic),
            None => print_root_help(),
        }
        return Ok(0);
    }
    if matches!(command.as_str(), "-V" | "--version" | "version") {
        println!("sdesk {} (Serial Desk)", env!("CARGO_PKG_VERSION"));
        return Ok(0);
    }

    match command.as_str() {
        "ports" => cmd_ports(rest),
        "capture" => capture::cmd_capture(rest),
        "send" => send::cmd_send(rest),
        "sessions" => control::cmd_sessions(rest),
        "stop" => control::cmd_stop(rest),
        "log" => logcmd::cmd_log(rest),
        other if other.starts_with('-') => Err(Fail::usage(format!(
            "未知选项“{other}”。\n运行 sdesk --help 查看用法。"
        ))),
        other => Err(Fail::usage(format!(
            "未知命令“{other}”。\n运行 sdesk --help 查看命令列表。"
        ))),
    }
}

fn cmd_ports(argv: &[String]) -> CliResult<i32> {
    let opts = args::parse(argv, &[("json", false)])?;
    let ports = crate::serial::list_ports().map_err(Fail::runtime)?;

    if opts.has("json") {
        let text = json_pretty(&ports)?;
        println!("{text}");
    } else if ports.is_empty() {
        println!("未发现串口。");
    } else {
        for port in &ports {
            println!("{:<8} {}", port.name, port.detail);
        }
    }
    Ok(0)
}

pub fn json_pretty<T: serde::Serialize>(value: &T) -> CliResult<String> {
    serde_json::to_string_pretty(value)
        .map_err(|error| Fail::runtime(format!("序列化 JSON 失败：{error}")))
}

pub fn json_line<T: serde::Serialize>(value: &T) -> CliResult<String> {
    serde_json::to_string(value)
        .map_err(|error| Fail::runtime(format!("序列化 JSON 失败：{error}")))
}

fn print_root_help() {
    let help = format!(
        "sdesk {version} — Serial Desk 命令行工具

用法:
  sdesk <命令> [选项]

命令:
  ports                 列出本机串口
  capture               打开串口，实时输出收发数据并记录日志
  send                  向运行中的 capture 会话（或直接向串口）发送数据
  sessions              列出正在运行的 capture 会话
  stop                  停止运行中的 capture 会话
  log                   查看会话日志：list / show / tail / path
  help [命令]           显示帮助
  version               显示版本

全局选项:
  -h, --help            显示帮助
  -V, --version         显示版本

机器可读输出:
  每个命令都支持 --json。capture 与 log show/tail 在 --json 下输出 JSONL
  （每行一个 JSON 对象，type 为 session / frame / summary），字段见
  docs/CLI.md。stdout 只放数据，提示与错误一律写入 stderr。

退出码:
  0  成功
  1  运行错误（串口不可用、文件不可读、输出中断等）
  2  参数错误
  3  超时或没有匹配的数据（串口本身工作正常）

典型 AI 工作流:
  sdesk ports --json
  sdesk capture --port COM10 --baud 115200 --json        # 后台运行
  sdesk send --port COM10 --data \"AT+GMR\\r\\n\"
  sdesk log tail --latest --lines 40 --json
  sdesk stop --all

运行 sdesk help <命令> 查看单个命令的全部选项。",
        version = env!("CARGO_PKG_VERSION")
    );
    println!("{help}");
}

fn print_command_help(topic: &str) {
    let text = match topic {
        "ports" => "用法: sdesk ports [--json]\n\n列出本机串口。--json 输出 [{name, detail}]。",
        "capture" => capture::HELP,
        "send" => send::HELP,
        "sessions" => control::SESSIONS_HELP,
        "stop" => control::STOP_HELP,
        "log" => logcmd::HELP,
        "version" => "用法: sdesk version\n\n显示版本号。",
        _ => {
            print_root_help();
            return;
        }
    };
    println!("{text}");
}
