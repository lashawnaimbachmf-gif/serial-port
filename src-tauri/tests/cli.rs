//! End-to-end tests for the `sdesk` binary.
//!
//! These run the real executable and assert on exit codes and machine-readable
//! output. They do not need hardware: the serial-side behaviour is covered by
//! the unit tests and the GUI hardware report. What they do cover is the part
//! an AI agent depends on — that JSONL output, filters and exit codes stay
//! stable.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_sdesk");

fn run(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("could not run {BIN}: {error}"))
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn json_lines(output: &Output) -> Vec<serde_json::Value> {
    stdout(output)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("invalid JSON line {line:?}: {error}"))
        })
        .collect()
}

/// A log written in the exact format the application produces.
fn sample_log(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("COM99_2026-10-19_12-02-05.log");
    let body = concat!(
        "# Serial Desk session log\r\n",
        "# created: 2026-10-19T12:02:05.559+08:00\r\n",
        "# port: COM99\r\n",
        "# serial: 115200 bps | 8-N-1 | flow: none\r\n",
        "# format: utf-8 text, one frame per line\r\n",
        "# escapes: \\\\ = backslash, \\r, \\n, \\t, \\xNN = raw byte\r\n",
        "# note: control characters and invalid utf-8 are escaped; every payload byte is recoverable\r\n",
        "# ---\r\n",
        "2026-10-19 12:02:06.100 | TX      7 B | HELLO\\r\\n\r\n",
        "2026-10-19 12:02:06.200 | RX      5 B | \\x00\\x01\\x7F\\x80\\xFF\r\n",
        "2026-10-19 12:02:06.300 | RX      6 B | 中文\r\n",
        "2026-10-19 12:02:06.400 | RX      1 B | \\xFF\r\n",
        "# ---\r\n",
        "# closed: 2026-10-19T12:02:11.319+08:00 | RX 12 B / 3 frames | TX 7 B / 1 frames\r\n",
    );
    fs::write(&path, body).expect("write sample log");
    path
}

fn show_args(path: &Path, extra: &[&str]) -> Vec<String> {
    let mut args = vec![
        "log".to_string(),
        "show".to_string(),
        path.to_string_lossy().into_owned(),
    ];
    args.extend(extra.iter().map(|value| value.to_string()));
    args
}

fn run_owned(args: &[String]) -> Output {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run(&refs)
}

#[test]
fn ports_json_is_an_array() {
    let output = run(&["ports", "--json"]);
    assert!(output.status.success(), "stderr={}", stderr(&output));
    let value: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("ports --json must be valid JSON");
    assert!(value.is_array());
}

#[test]
fn show_emits_lossless_json_frames_in_log_order() {
    let dir = TempDir::new().unwrap();
    let path = sample_log(dir.path());

    let output = run_owned(&show_args(&path, &["--json"]));
    assert_eq!(output.status.code(), Some(0), "stderr={}", stderr(&output));

    let frames = json_lines(&output);
    assert_eq!(frames.len(), 4);

    assert_eq!(frames[0]["type"], "frame");
    assert_eq!(frames[0]["source"], "log");
    assert_eq!(frames[0]["dir"], "TX");
    assert_eq!(frames[0]["bytes"], 7);
    assert_eq!(frames[0]["text"], "HELLO\\r\\n");

    assert_eq!(frames[1]["dir"], "RX");
    assert_eq!(frames[1]["bytes"], 5);
    assert_eq!(frames[1]["text"], "\\x00\\x01\\x7F\\x80\\xFF");
    assert_eq!(frames[1]["hex"], "00 01 7F 80 FF");

    assert_eq!(frames[2]["text"], "中文");
    assert_eq!(frames[2]["bytes"], 6);
    assert_eq!(frames[3]["hex"], "FF");
}

#[test]
fn direction_time_and_grep_filters_narrow_the_result() {
    let dir = TempDir::new().unwrap();
    let path = sample_log(dir.path());

    let rx = run_owned(&show_args(&path, &["--json", "--direction", "rx"]));
    assert_eq!(json_lines(&rx).len(), 3);

    let since = run_owned(&show_args(
        &path,
        &["--json", "--since", "2026-10-19 12:02:06.250"],
    ));
    let frames = json_lines(&since);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["text"], "中文");

    let grep = run_owned(&show_args(&path, &["--json", "--grep", "\\xFF"]));
    assert_eq!(
        json_lines(&grep).len(),
        2,
        "matches raw FF and the escaped byte"
    );

    let missing = run_owned(&show_args(&path, &["--json", "--grep", "no-such-text"]));
    assert_eq!(missing.status.code(), Some(3));
    assert!(json_lines(&missing).is_empty());
}

#[test]
fn limit_and_offset_slice_frames() {
    let dir = TempDir::new().unwrap();
    let path = sample_log(dir.path());

    let limited = run_owned(&show_args(&path, &["--json", "--limit", "2"]));
    assert_eq!(json_lines(&limited).len(), 2);
    assert_eq!(json_lines(&limited)[1]["text"], "\\x00\\x01\\x7F\\x80\\xFF");

    let offset = run_owned(&show_args(
        &path,
        &["--json", "--offset", "3", "--limit", "1"],
    ));
    let frames = json_lines(&offset);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["hex"], "FF");

    let beyond = run_owned(&show_args(&path, &["--json", "--offset", "99"]));
    assert_eq!(beyond.status.code(), Some(3));
}

#[test]
fn tail_reads_only_the_last_frames() {
    let dir = TempDir::new().unwrap();
    let path = sample_log(dir.path());

    let output = run(&[
        "log",
        "tail",
        path.to_str().unwrap(),
        "--lines",
        "2",
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(0), "stderr={}", stderr(&output));
    let frames = json_lines(&output);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["text"], "中文");
    assert_eq!(frames[1]["hex"], "FF");
}

#[test]
fn list_reports_header_counts_and_closed_state() {
    let dir = TempDir::new().unwrap();
    let path = sample_log(dir.path());

    let output = run(&[
        "log",
        "list",
        "--dir",
        dir.path().to_str().unwrap(),
        "--json",
    ]);
    assert!(output.status.success(), "stderr={}", stderr(&output));

    let items: Vec<serde_json::Value> =
        serde_json::from_str(stdout(&output).trim()).expect("list --json must be an array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["port"], "COM99");
    assert_eq!(items[0]["frames"], 4);
    assert_eq!(items[0]["rxBytes"], 12);
    assert_eq!(items[0]["txBytes"], 7);
    assert_eq!(items[0]["closed"], true);
    assert_eq!(items[0]["invalidLines"], 0);
    assert_eq!(
        items[0]["file"].as_str().unwrap(),
        path.file_name().unwrap().to_string_lossy()
    );
}

#[test]
fn latest_resolves_the_newest_log_in_the_directory() {
    let dir = TempDir::new().unwrap();
    let path = sample_log(dir.path());

    let output = run(&[
        "log",
        "tail",
        "--latest",
        "--dir",
        dir.path().to_str().unwrap(),
        "--lines",
        "1",
        "--json",
    ]);
    assert!(output.status.success(), "stderr={}", stderr(&output));
    assert_eq!(json_lines(&output).len(), 1);
    assert!(path.exists());
}

#[test]
fn usage_and_missing_data_have_stable_exit_codes() {
    let dir = TempDir::new().unwrap();
    let path = sample_log(dir.path());

    let unknown = run(&["send", "--data", "AT", "--bogus"]);
    assert_eq!(unknown.status.code(), Some(2));

    let no_source = run(&["send", "--port", "COM99"]);
    assert_eq!(no_source.status.code(), Some(2));

    let bad_hex = run(&["send", "--data-hex", "ABC"]);
    assert_eq!(bad_hex.status.code(), Some(2));

    let missing_file = run(&[
        "log",
        "show",
        dir.path().join("missing.log").to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(missing_file.status.code(), Some(1));
    assert!(stderr(&missing_file).contains("无法打开日志"));

    let no_filter_match = run_owned(&show_args(&path, &["--since", "-1s"]));
    assert_eq!(no_filter_match.status.code(), Some(3));
}

#[test]
fn stop_json_without_sessions_is_an_empty_array() {
    let dir = TempDir::new().unwrap();
    let output = Command::new(BIN)
        .args(["stop", "--json"])
        .env("LOCALAPPDATA", dir.path())
        .output()
        .expect("run sdesk stop");
    assert_eq!(output.status.code(), Some(0), "stderr={}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "[]");
}

#[test]
fn help_is_available_for_every_command() {
    for command in [
        "capture", "send", "sessions", "stop", "log", "ports", "version",
    ] {
        let output = run(&["help", command]);
        assert!(
            output.status.success(),
            "help {command} failed: {}",
            stderr(&output)
        );
        assert!(!stdout(&output).trim().is_empty());
    }
}
