//! Read a saved session log for the GUI's log-replay view.
//!
//! Parsing is shared with the CLI by including its parser source directly.
//! There is no second interpretation of the on-disk format: the same strict
//! timestamp, direction, byte-count and escaping rules apply here.

#[allow(dead_code)]
mod frame;

#[allow(dead_code)]
#[path = "../cli/logfile.rs"]
mod logfile;

use std::{
    collections::VecDeque,
    fmt::Write as _,
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

use serde::Serialize;

use self::{
    frame::Dir,
    logfile::{parse_line, scan, LineKind},
};

/// The terminal keeps an 8 MiB display buffer; keep the replay payload within
/// the same budget so opening a log does not create an unbounded webview
/// message. Older frames are dropped from the front and reported explicitly.
const MAX_VIEW_FRAMES: usize = 50_000;
const MAX_VIEW_BYTES: usize = 8 * 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogViewFrame {
    pub dir: &'static str,
    pub ts_ms: i64,
    pub bytes: usize,
    pub hex: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogViewDocument {
    pub path: String,
    pub file: String,
    pub port: Option<String>,
    pub serial: Option<String>,
    pub created: Option<String>,
    pub created_ms: Option<i64>,
    pub closed: bool,
    pub total_frames: u64,
    pub returned_frames: usize,
    pub rx_frames: u64,
    pub tx_frames: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub size: u64,
    pub invalid_lines: u64,
    pub modified_ms: i64,
    pub truncated: bool,
    pub frames: Vec<LogViewFrame>,
}

/// Loads a saved log and returns a bounded, exact-byte view of it.
pub fn read(path: &Path) -> Result<LogViewDocument, String> {
    let meta = scan(path).map_err(|error| format!("无法打开日志“{}”：{error}", path.display()))?;

    let file =
        File::open(path).map_err(|error| format!("无法读取日志“{}”：{error}", path.display()))?;
    let reader = BufReader::new(file);

    let mut frames: VecDeque<LogViewFrame> = VecDeque::new();
    let mut selected_bytes = 0usize;
    let mut total_frames = 0u64;
    let mut rx_frames = 0u64;
    let mut tx_frames = 0u64;
    let mut rx_bytes = 0u64;
    let mut tx_bytes = 0u64;

    for line in reader.lines() {
        let line = line.map_err(|error| format!("读取日志失败：{error}"))?;
        let LineKind::Frame(frame) = parse_line(&line) else {
            continue;
        };

        total_frames += 1;
        match frame.dir {
            Dir::Rx => {
                rx_frames += 1;
                rx_bytes += frame.bytes.len() as u64;
            }
            Dir::Tx => {
                tx_frames += 1;
                tx_bytes += frame.bytes.len() as u64;
            }
            Dir::Error => {}
        }

        while !frames.is_empty()
            && (frames.len() >= MAX_VIEW_FRAMES
                || selected_bytes + frame.bytes.len() > MAX_VIEW_BYTES)
        {
            if let Some(dropped) = frames.pop_front() {
                selected_bytes = selected_bytes.saturating_sub(dropped.bytes);
            }
        }

        let view = LogViewFrame {
            dir: match frame.dir {
                Dir::Rx => "rx",
                Dir::Tx => "tx",
                Dir::Error => "error",
            },
            ts_ms: frame.ts_ms,
            bytes: frame.bytes.len(),
            hex: hex(&frame.bytes),
        };
        selected_bytes += frame.bytes.len();
        frames.push_back(view);
    }

    let returned_frames = frames.len();
    let truncated = total_frames > returned_frames as u64;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    Ok(LogViewDocument {
        path: path.to_string_lossy().into_owned(),
        file: file_name,
        port: meta.header.port,
        serial: meta.header.serial,
        created: meta.header.created_text,
        created_ms: meta.header.created_ms,
        closed: meta.closed,
        total_frames,
        returned_frames,
        rx_frames,
        tx_frames,
        rx_bytes,
        tx_bytes,
        size: meta.size,
        invalid_lines: meta.invalid,
        modified_ms: meta.modified_ms,
        truncated,
        frames: frames.into_iter().collect(),
    })
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len().saturating_mul(3));
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 {
            output.push(' ');
        }
        let _ = write!(output, "{byte:02X}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_exact_frames_and_metadata() {
        let dir = std::env::temp_dir().join(format!(
            "serialdesk-logview-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("COM99_2026-10-19_12-00-00.log");
        std::fs::write(
            &path,
            concat!(
                "# Serial Desk session log\r\n",
                "# created: 2026-10-19T12:00:00.000+08:00\r\n",
                "# port: COM99\r\n",
                "# serial: 115200 bps | 8-N-1 | flow: none\r\n",
                "# ---\r\n",
                "2026-10-19 12:00:01.100 | RX      5 B | \\x00\\x01\\x7F\\x80\\xFF\r\n",
                "2026-10-19 12:00:01.200 | TX      6 B | 中文\r\n",
                "# ---\r\n",
                "# closed: 2026-10-19T12:00:02.000+08:00 | RX 5 B / 1 frames | TX 6 B / 1 frames\r\n",
            ),
        )
        .unwrap();

        let document = read(&path).unwrap();
        assert_eq!(document.port.as_deref(), Some("COM99"));
        assert!(document.closed);
        assert_eq!(document.total_frames, 2);
        assert_eq!(document.returned_frames, 2);
        assert_eq!(document.rx_frames, 1);
        assert_eq!(document.tx_frames, 1);
        assert_eq!(document.rx_bytes, 5);
        assert_eq!(document.tx_bytes, 6);
        assert_eq!(document.frames[0].hex, "00 01 7F 80 FF");
        assert_eq!(document.frames[0].bytes, 5);
        assert_eq!(document.frames[1].dir, "tx");
        assert_eq!(document.frames[1].hex, "E4 B8 AD E6 96 87");
        assert!(!document.truncated);
        assert_eq!(document.invalid_lines, 0);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
