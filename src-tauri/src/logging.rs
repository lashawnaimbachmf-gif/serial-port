use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use chrono::{DateTime, Local, SecondsFormat};
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Rx,
    Tx,
}

impl Direction {
    fn label(self) -> &'static str {
        match self {
            Self::Rx => "RX",
            Self::Tx => "TX",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionLogStatus {
    pub path: String,
    pub interval_secs: u64,
    pub started_at_ms: i64,
    pub last_saved_at_ms: Option<i64>,
    pub bytes_logged: u64,
    pub entries: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub file_size: u64,
    pub error: Option<String>,
    pub closed: bool,
}

pub struct Logger {
    file: File,
    path: PathBuf,
    interval_secs: u64,
    started_at: DateTime<Local>,
    last_saved_at: Option<DateTime<Local>>,
    last_sync: Instant,
    dirty: bool,
    error: Option<String>,
    bytes_logged: u64,
    entries: u64,
    rx_bytes: u64,
    tx_bytes: u64,
    rx_entries: u64,
    tx_entries: u64,
    file_bytes: u64,
}

impl Logger {
    pub fn create(
        dir: &Path,
        port_name: &str,
        serial_description: &str,
        flow_label: &str,
        interval_secs: u64,
    ) -> Result<Self, String> {
        fs::create_dir_all(dir)
            .map_err(|e| format!("无法创建日志文件夹“{}”：{e}", dir.display()))?;

        let now = Local::now();
        let date_part = now.format("%Y-%m-%d_%H-%M-%S").to_string();
        let safe_port = sanitize_filename_component(port_name);
        let mut last_error = None;

        for attempt in 1..=999u16 {
            let suffix = if attempt == 1 {
                String::new()
            } else {
                format!("_{attempt:02}")
            };
            let path = dir.join(format!("{safe_port}_{date_part}{suffix}.log"));
            let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    last_error = Some(error);
                    continue;
                }
                Err(error) => {
                    return Err(format!("无法创建日志文件“{}”：{error}", path.display()));
                }
            };

            let header = build_header(now, port_name, serial_description, flow_label);
            if let Err(error) = file.write_all(header.as_bytes()) {
                drop(file);
                let _ = fs::remove_file(&path);
                return Err(format!("写入日志文件头失败：{error}"));
            }
            if let Err(error) = file.sync_data() {
                drop(file);
                let _ = fs::remove_file(&path);
                return Err(format!("同步日志文件失败：{error}"));
            }

            return Ok(Self {
                file,
                path,
                interval_secs,
                started_at: now,
                last_saved_at: Some(now),
                last_sync: Instant::now(),
                dirty: false,
                error: None,
                bytes_logged: 0,
                entries: 0,
                rx_bytes: 0,
                tx_bytes: 0,
                rx_entries: 0,
                tx_entries: 0,
                file_bytes: header.len() as u64,
            });
        }

        Err(format!(
            "同一秒内的日志文件过多，无法创建新文件：{}",
            last_error
                .map(|e| e.to_string())
                .unwrap_or_else(|| "文件名冲突".to_string())
        ))
    }

    pub fn append(&mut self, direction: Direction, timestamp: DateTime<Local>, bytes: &[u8]) {
        if bytes.is_empty() || self.error.is_some() {
            // Once a write has failed, this session's log can no longer be
            // presented as complete. Keep receiving, but do not hide the
            // failure behind a later successful write.
            return;
        }

        let line = format!(
            "{} | {} {:>6} B | {}\r\n",
            timestamp.format("%Y-%m-%d %H:%M:%S%.3f"),
            direction.label(),
            bytes.len(),
            escape_bytes(bytes)
        );

        if let Err(error) = self.file.write_all(line.as_bytes()) {
            self.error = Some(format!("日志写入失败（{}）：{error}", self.path.display()));
            self.dirty = true;
            return;
        }

        let line_len = line.len() as u64;
        self.file_bytes = self.file_bytes.saturating_add(line_len);
        self.bytes_logged = self.bytes_logged.saturating_add(bytes.len() as u64);
        self.entries = self.entries.saturating_add(1);
        match direction {
            Direction::Rx => {
                self.rx_bytes = self.rx_bytes.saturating_add(bytes.len() as u64);
                self.rx_entries = self.rx_entries.saturating_add(1);
            }
            Direction::Tx => {
                self.tx_bytes = self.tx_bytes.saturating_add(bytes.len() as u64);
                self.tx_entries = self.tx_entries.saturating_add(1);
            }
        }
        self.dirty = true;
    }

    pub fn flush_if_due(&mut self) {
        if !self.dirty {
            return;
        }
        if self.last_sync.elapsed() < Duration::from_secs(self.interval_secs) {
            return;
        }
        self.sync_now();
    }

    pub fn sync_now(&mut self) {
        match self.file.sync_data() {
            Ok(()) => {
                self.last_saved_at = Some(Local::now());
                self.last_sync = Instant::now();
                self.dirty = false;
            }
            Err(error) => {
                self.error = Some(format!(
                    "日志写入磁盘失败（{}）：{error}",
                    self.path.display()
                ));
                self.dirty = true;
            }
        }
    }

    pub fn set_interval(&mut self, interval_secs: u64) {
        self.interval_secs = interval_secs.clamp(1, 3600);
    }

    pub fn status(&self) -> SessionLogStatus {
        let file_size = self
            .file
            .metadata()
            .map(|metadata| metadata.len())
            .unwrap_or(self.file_bytes);
        SessionLogStatus {
            path: self.path.to_string_lossy().into_owned(),
            interval_secs: self.interval_secs,
            started_at_ms: self.started_at.timestamp_millis(),
            last_saved_at_ms: self.last_saved_at.map(|time| time.timestamp_millis()),
            bytes_logged: self.bytes_logged,
            entries: self.entries,
            rx_bytes: self.rx_bytes,
            tx_bytes: self.tx_bytes,
            file_size,
            error: self.error.clone(),
            closed: false,
        }
    }

    pub fn finish(mut self) -> SessionLogStatus {
        let closed_at = Local::now();
        let footer = format!(
            "# ---\r\n# closed: {} | RX {} B / {} frames | TX {} B / {} frames\r\n",
            closed_at.to_rfc3339_opts(SecondsFormat::Millis, false),
            self.rx_bytes,
            self.rx_entries,
            self.tx_bytes,
            self.tx_entries
        );
        if self.error.is_none() {
            if let Err(error) = self.file.write_all(footer.as_bytes()) {
                self.error = Some(format!(
                    "写入日志结束标记失败（{}）：{error}",
                    self.path.display()
                ));
            }
        }
        if let Err(error) = self.file.sync_data() {
            if self.error.is_none() {
                self.error = Some(format!(
                    "关闭前同步日志失败（{}）：{error}",
                    self.path.display()
                ));
            }
        } else {
            self.last_saved_at = Some(Local::now());
        }

        let mut status = self.status();
        status.closed = true;
        status.file_size = self
            .file
            .metadata()
            .map(|metadata| metadata.len())
            .unwrap_or(self.file_bytes);
        status
    }
}

fn build_header(
    now: DateTime<Local>,
    port_name: &str,
    serial_description: &str,
    flow_label: &str,
) -> String {
    let mut header = String::new();
    header.push_str("# Serial Desk session log\r\n");
    header.push_str(&format!(
        "# created: {}\r\n",
        now.to_rfc3339_opts(SecondsFormat::Millis, false)
    ));
    header.push_str(&format!(
        "# port: {}\r\n# serial: {} | flow: {}\r\n",
        one_line(port_name),
        one_line(serial_description),
        one_line(flow_label)
    ));
    header.push_str("# format: utf-8 text, one frame per line\r\n");
    header.push_str("# escapes: \\\\ = backslash, \\r, \\n, \\t, \\xNN = raw byte\r\n");
    header.push_str(
        "# note: control characters and invalid utf-8 are escaped; every payload byte is recoverable\r\n",
    );
    header.push_str("# ---\r\n");
    header
}

fn one_line(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect()
}

/// A lossless, one-line representation of a byte sequence.
///
/// Valid UTF-8 is kept readable. Backslashes and control characters are
/// escaped, and invalid UTF-8 bytes are emitted as `\xNN`. Decoding the
/// escapes and re-encoding the literal characters reconstructs the original
/// bytes exactly.
pub fn escape_bytes(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len());
    let mut rest = bytes;

    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                push_escaped_str(&mut output, valid);
                break;
            }
            Err(error) => {
                let valid_len = error.valid_up_to();
                if valid_len > 0 {
                    // SAFETY: from_utf8 reported this prefix as valid UTF-8.
                    let valid = unsafe { std::str::from_utf8_unchecked(&rest[..valid_len]) };
                    push_escaped_str(&mut output, valid);
                }

                match error.error_len() {
                    Some(invalid_len) => {
                        for byte in &rest[valid_len..valid_len + invalid_len] {
                            push_hex_byte(&mut output, *byte);
                        }
                        rest = &rest[valid_len + invalid_len..];
                    }
                    None => {
                        // Incomplete sequence at the end of this frame. The next
                        // frame may complete it, but a log line must stand alone.
                        for byte in &rest[valid_len..] {
                            push_hex_byte(&mut output, *byte);
                        }
                        break;
                    }
                }
            }
        }
    }

    output
}

fn push_escaped_str(output: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '\\' => output.push_str("\\\\"),
            '\r' => output.push_str("\\r"),
            '\n' => output.push_str("\\n"),
            '\t' => output.push_str("\\t"),
            ch if ch.is_control() || is_unsafe_format_char(ch) => {
                let mut buffer = [0u8; 4];
                for byte in ch.encode_utf8(&mut buffer).as_bytes() {
                    push_hex_byte(output, *byte);
                }
            }
            ch => output.push(ch),
        }
    }
}

fn is_unsafe_format_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{2028}'
            | '\u{2029}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
    )
}

fn push_hex_byte(output: &mut String, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    output.push('\\');
    output.push('x');
    output.push(HEX[(byte >> 4) as usize] as char);
    output.push(HEX[(byte & 0x0F) as usize] as char);
}

fn sanitize_filename_component(value: &str) -> String {
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
        cleaned.chars().take(48).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unescape(value: &str) -> Vec<u8> {
        let chars: Vec<char> = value.chars().collect();
        let mut output = Vec::new();
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
            match chars.get(index).copied() {
                Some('\\') => {
                    output.push(b'\\');
                    index += 1;
                }
                Some('r') => {
                    output.push(b'\r');
                    index += 1;
                }
                Some('n') => {
                    output.push(b'\n');
                    index += 1;
                }
                Some('t') => {
                    output.push(b'\t');
                    index += 1;
                }
                Some('x') => {
                    let high = chars.get(index + 1).and_then(|c| c.to_digit(16));
                    let low = chars.get(index + 2).and_then(|c| c.to_digit(16));
                    if let (Some(high), Some(low)) = (high, low) {
                        output.push(((high << 4) | low) as u8);
                        index += 3;
                    } else {
                        output.push(b'\\');
                        output.push(b'x');
                        index += 1;
                    }
                }
                Some(other) => {
                    output.push(b'\\');
                    let mut buffer = [0u8; 4];
                    output.extend_from_slice(other.encode_utf8(&mut buffer).as_bytes());
                    index += 1;
                }
                None => output.push(b'\\'),
            }
        }
        output
    }

    #[test]
    fn escaping_is_lossless_for_text_and_binary() {
        let cases: Vec<Vec<u8>> = vec![
            b"AT+VER?\r\n".to_vec(),
            "中文日志，包含标点。".as_bytes().to_vec(),
            vec![0x00, 0x01, 0x08, 0x1B, 0x7F, 0xFF, 0xFE, 0x80],
            vec![0xE4, 0xB8],       // truncated UTF-8 sequence
            vec![0xE4, 0xB8, 0xAD], // complete "中"
            b"back\\slash\tand more".to_vec(),
            vec![0xE2, 0x80, 0xA8], // U+2028 line separator
        ];

        for input in cases {
            let escaped = escape_bytes(&input);
            assert!(
                !escaped.contains('\n'),
                "escaped log line must not contain LF"
            );
            assert!(
                !escaped.contains('\r'),
                "escaped log line must not contain CR"
            );
            assert_eq!(unescape(&escaped), input, "escaped={escaped}");
        }
    }

    #[test]
    fn escapes_control_characters_and_invalid_utf8() {
        assert_eq!(escape_bytes(b"a\nb\tc"), "a\\nb\\tc");
        assert_eq!(escape_bytes(b"a\\b"), "a\\\\b");
        assert_eq!(escape_bytes(&[0x00, 0x1B, 0x7F]), "\\x00\\x1B\\x7F");
        assert_eq!(escape_bytes(&[0xE4, 0xB8]), "\\xE4\\xB8");
        assert_eq!(escape_bytes("中".as_bytes()), "中");
    }

    #[test]
    fn timed_flush_persists_data_before_close() {
        let dir = std::env::temp_dir().join(format!(
            "serial-desk-timed-log-{}-{}",
            std::process::id(),
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();

        let mut logger = Logger::create(&dir, "COM9", "9600 bps | 8-N-1", "none", 30).unwrap();
        let initial_save = logger.status().last_saved_at_ms;
        logger.set_interval(1);
        logger.append(Direction::Rx, Local::now(), b"persist-me\r\n");
        std::thread::sleep(Duration::from_millis(1_050));
        logger.flush_if_due();

        let status = logger.status();
        assert!(status.last_saved_at_ms >= initial_save);
        let contents = fs::read_to_string(&status.path).unwrap();
        assert!(contents.contains("persist-me\\r\\n"));
        assert!(!status.closed);

        let final_status = logger.finish();
        assert!(final_status.closed);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn creates_a_session_log_with_header_and_exact_payload() {
        let dir = std::env::temp_dir().join(format!(
            "serial-desk-log-{}-{}",
            std::process::id(),
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();

        let mut logger = Logger::create(&dir, "COM8", "115200 bps | 8-N-1", "none", 30).unwrap();
        let timestamp = Local::now();
        logger.append(Direction::Rx, timestamp, b"OK\r\n");
        logger.append(Direction::Tx, timestamp, b"AT\r\n");
        let status = logger.finish();

        let contents = fs::read_to_string(&status.path).unwrap();
        assert!(contents.starts_with("# Serial Desk session log\r\n"));
        assert!(contents.contains("# port: COM8\r\n# serial: 115200 bps | 8-N-1 | flow: none"));
        assert!(contents.contains("| RX      4 B | OK\\r\\n"));
        assert!(contents.contains("| TX      4 B | AT\\r\\n"));
        assert!(contents.contains("# closed:"));
        assert!(contents.contains("RX 4 B / 1 frames"));
        assert!(status.closed);
        assert_eq!(status.bytes_logged, 8);
        assert_eq!(status.entries, 2);
        assert!(status.error.is_none());
        assert!(status.file_size > 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn filename_component_is_safe() {
        assert_eq!(sanitize_filename_component("COM8"), "COM8");
        assert_eq!(sanitize_filename_component("../COM:8?"), "COM_8");
        assert_eq!(sanitize_filename_component(""), "serial");
    }
}
