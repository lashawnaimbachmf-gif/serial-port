//! Parser for the session log format documented in `docs/LOG_FORMAT.md`.
//!
//! The parser is intentionally strict: a frame line must carry a valid
//! timestamp, direction and byte count, and the decoded payload must match the
//! recorded byte count exactly. A corrupted line is reported, never silently
//! treated as data.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};

use super::frame::Dir;

#[derive(Clone, Debug)]
pub struct LogLine {
    pub ts_ms: i64,
    pub ts: String,
    pub dir: Dir,
    pub bytes: Vec<u8>,
}

pub enum LineKind {
    Frame(LogLine),
    Meta,
    Invalid,
}

#[derive(Clone, Debug, Default)]
pub struct Header {
    pub port: Option<String>,
    pub serial: Option<String>,
    pub created_text: Option<String>,
    pub created_ms: Option<i64>,
}

pub struct LogMeta {
    pub path: PathBuf,
    pub name: String,
    pub header: Header,
    pub closed: bool,
    pub frames: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub size: u64,
    pub invalid: u64,
    pub modified_ms: i64,
}

/// Reverses the escaping produced by `logging::escape_bytes`.
pub fn unescape(text: &str) -> Vec<u8> {
    let chars: Vec<char> = text.chars().collect();
    let mut output = Vec::with_capacity(text.len());
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
                let high = chars.get(index + 1).and_then(|ch| ch.to_digit(16));
                let low = chars.get(index + 2).and_then(|ch| ch.to_digit(16));
                match (high, low) {
                    (Some(high), Some(low)) => {
                        output.push(((high << 4) | low) as u8);
                        index += 3;
                    }
                    _ => {
                        output.push(b'\\');
                        output.push(b'x');
                        index += 1;
                    }
                }
            }
            Some(other) => {
                // The current writer never emits an unknown escape. Preserve
                // it literally rather than inventing a byte.
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

pub fn parse_line(line: &str) -> LineKind {
    let trimmed = strip_bom(line.trim_end_matches(['\r', '\n']));
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return LineKind::Meta;
    }

    let Some((ts_part, rest)) = trimmed.split_once(" | ") else {
        return LineKind::Invalid;
    };
    if ts_part.len() != 23 {
        return LineKind::Invalid;
    }
    let Ok(naive) = NaiveDateTime::parse_from_str(ts_part, "%Y-%m-%d %H:%M:%S%.3f") else {
        return LineKind::Invalid;
    };

    let dir = match rest.get(..2) {
        Some("RX") => Dir::Rx,
        Some("TX") => Dir::Tx,
        _ => return LineKind::Invalid,
    };
    let rest = &rest[2..];
    let rest = rest.trim_start_matches(' ');
    let Some((count_part, payload_part)) = rest.split_once(" B | ") else {
        return LineKind::Invalid;
    };
    let Ok(count) = count_part.trim().parse::<usize>() else {
        return LineKind::Invalid;
    };

    let bytes = unescape(payload_part);
    if bytes.len() != count {
        return LineKind::Invalid;
    }

    LineKind::Frame(LogLine {
        ts_ms: local_millis(naive),
        ts: ts_part.to_string(),
        dir,
        bytes,
    })
}

pub fn read_header<R: BufRead>(reader: &mut R) -> io::Result<(Header, Option<String>)> {
    let mut header = Header::default();
    let mut closed = false;
    let mut line = String::new();

    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok((header, None));
        }
        let trimmed = strip_bom(line.trim_end_matches(['\r', '\n']));
        if !trimmed.starts_with('#') {
            return Ok((header, Some(trimmed.to_string())));
        }
        parse_meta_line(trimmed, &mut header, &mut closed);
    }
}

fn strip_bom(line: &str) -> &str {
    line.strip_prefix('\u{feff}').unwrap_or(line)
}

fn parse_meta_line(line: &str, header: &mut Header, closed: &mut bool) {
    let Some((key, value)) = line.trim_start_matches('#').trim().split_once(':') else {
        return;
    };
    let value = value.trim();
    match key.trim() {
        "created" => {
            header.created_text = Some(value.to_string());
            header.created_ms = DateTime::parse_from_rfc3339(value)
                .ok()
                .map(|parsed| parsed.timestamp_millis());
        }
        "port" => header.port = Some(value.to_string()),
        "serial" => header.serial = Some(value.to_string()),
        "closed" => *closed = true,
        _ => {}
    }
}

pub fn scan(path: &Path) -> io::Result<LogMeta> {
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    let size = metadata.len();
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_millis() as i64)
        .unwrap_or(0);

    let mut header = Header::default();
    let mut closed = false;
    let mut frames = 0u64;
    let mut rx_bytes = 0u64;
    let mut tx_bytes = 0u64;
    let mut invalid = 0u64;

    let reader = BufReader::new(file);
    for line in reader.lines() {
        let line = line?;
        let trimmed = strip_bom(line.trim_end_matches('\r'));
        if trimmed.starts_with('#') {
            parse_meta_line(trimmed, &mut header, &mut closed);
            continue;
        }
        match parse_line(trimmed) {
            LineKind::Frame(frame) => {
                frames += 1;
                match frame.dir {
                    Dir::Rx => rx_bytes += frame.bytes.len() as u64,
                    Dir::Tx => tx_bytes += frame.bytes.len() as u64,
                    Dir::Error => {}
                }
            }
            LineKind::Meta => {}
            LineKind::Invalid => invalid += 1,
        }
    }

    Ok(LogMeta {
        path: path.to_path_buf(),
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        header,
        closed,
        frames,
        rx_bytes,
        tx_bytes,
        size,
        invalid,
        modified_ms,
    })
}

/// Returns the last `count` lines, reading backwards from the end of the file.
///
/// Also returns the file length observed at open time. A follower can start
/// reading exactly where the tail stopped, so no line is printed twice or
/// skipped.
pub fn tail_lines_with_end(path: &Path, count: usize) -> io::Result<(Vec<String>, u64)> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    if count == 0 || length == 0 {
        return Ok((Vec::new(), length));
    }

    const CHUNK: usize = 64 * 1024;
    let mut position = length;
    let mut buffer: Vec<u8> = Vec::new();
    let mut newlines = 0usize;

    while position > 0 && newlines < count + 1 {
        let size = CHUNK.min(position as usize);
        position -= size as u64;
        file.seek(SeekFrom::Start(position))?;
        let mut block = vec![0u8; size];
        file.read_exact(&mut block)?;
        newlines += block.iter().filter(|byte| **byte == b'\n').count();
        block.extend_from_slice(&buffer);
        buffer = block;
    }

    let text = String::from_utf8_lossy(&buffer);
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect();

    // When the first chunk starts in the middle of a line, that fragment is
    // not a complete record. Dropping it is safe: the loop above always reads
    // at least one more newline than requested.
    if position > 0 {
        lines.remove(0);
    }
    if lines.last().map(|line| line.is_empty()).unwrap_or(false) {
        lines.pop();
    }

    let start = lines.len().saturating_sub(count);
    Ok((lines[start..].to_vec(), length))
}

fn local_millis(naive: NaiveDateTime) -> i64 {
    match Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(value) => value.timestamp_millis(),
        chrono::LocalResult::Ambiguous(earlier, _) => earlier.timestamp_millis(),
        // A wall-clock time inside a DST gap has no local instant. Logs are
        // written from `Local::now()`, which cannot land in the gap, so this
        // path only matters for hand-edited files.
        chrono::LocalResult::None => {
            DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc).timestamp_millis()
        }
    }
}

/// Reads complete lines appended after `position`.
///
/// A partial trailing line is left for the next call. Returns the new position.
pub fn read_from(path: &Path, position: u64) -> io::Result<(Vec<String>, u64)> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let position = if length < position { 0 } else { position };
    if position >= length {
        return Ok((Vec::new(), position));
    }

    file.seek(SeekFrom::Start(position))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;

    let Some(last_newline) = bytes.iter().rposition(|byte| *byte == b'\n') else {
        return Ok((Vec::new(), position));
    };
    let consumed = last_newline + 1;
    let text = String::from_utf8_lossy(&bytes[..consumed]);
    let lines = text
        .split('\n')
        .filter(|line| !line.is_empty())
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect();

    Ok((lines, position + consumed as u64))
}

pub fn log_files(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_log = path
            .extension()
            .map(|extension| extension.eq_ignore_ascii_case("log"))
            .unwrap_or(false);
        if is_log {
            files.push(path);
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::escape_bytes;

    fn sample_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sdesk-logfile-{tag}-{}-{}",
            std::process::id(),
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_a_frame_line_and_strict_byte_count() {
        let line = "2026-10-19 12:02:06.521 | RX     24 B | DSH-LOOP-20261019-B8E4\\r\\n";
        let LineKind::Frame(frame) = parse_line(line) else {
            panic!("line should parse");
        };
        assert_eq!(frame.dir, Dir::Rx);
        assert_eq!(frame.bytes.len(), 24);
        assert_eq!(frame.ts, "2026-10-19 12:02:06.521");

        let wrong = "2026-10-19 12:02:06.521 | RX     25 B | abc";
        assert!(matches!(parse_line(wrong), LineKind::Invalid));
    }

    #[test]
    fn tolerates_a_utf8_bom_on_the_first_line() {
        let frame = "\u{feff}2026-10-19 12:02:06.521 | RX      2 B | OK";
        assert!(matches!(parse_line(frame), LineKind::Frame(_)));

        let source = "\u{feff}# Serial Desk session log\n# port: COM7\n# ---\n";
        let mut reader = BufReader::new(source.as_bytes());
        let (header, pending) = read_header(&mut reader).unwrap();
        assert_eq!(header.port.as_deref(), Some("COM7"));
        assert!(pending.is_none());
    }

    #[test]
    fn escaped_payload_round_trips_through_the_log_format() {
        let payload: Vec<u8> = vec![0x00, 0x41, 0x0D, 0x0A, 0xFF, 0xE4, 0xB8, 0xAD];
        let line = format!(
            "2026-10-19 12:02:06.521 | TX {:>6} B | {}",
            payload.len(),
            escape_bytes(&payload)
        );
        let LineKind::Frame(frame) = parse_line(&line) else {
            panic!("line should parse");
        };
        assert_eq!(frame.bytes, payload);
    }

    #[test]
    fn header_and_closed_marker_are_recognised() {
        let dir = sample_dir("header");
        let path = dir.join("COM9_2026-10-19_12-02-05.log");
        let body = "# Serial Desk session log\r\n# created: 2026-10-19T12:02:05.559+08:00\r\n# port: COM9\r\n# serial: 115200 bps | 8-N-1 | flow: none\r\n# ---\r\n2026-10-19 12:02:06.521 | RX      2 B | OK\r\n# ---\r\n# closed: 2026-10-19T12:02:11.319+08:00 | RX 2 B / 1 frames | TX 0 B / 0 frames\r\n";
        std::fs::write(&path, body).unwrap();

        let meta = scan(&path).unwrap();
        assert_eq!(meta.header.port.as_deref(), Some("COM9"));
        assert_eq!(
            meta.header.serial.as_deref(),
            Some("115200 bps | 8-N-1 | flow: none")
        );
        assert!(meta.header.created_ms.is_some());
        assert!(meta.closed);
        assert_eq!(meta.frames, 1);
        assert_eq!(meta.rx_bytes, 2);
        assert_eq!(meta.invalid, 0);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tail_returns_the_last_complete_lines() {
        let dir = sample_dir("tail");
        let path = dir.join("x.log");
        let mut body = String::from("# header\r\n");
        for index in 0..500 {
            body.push_str(&format!(
                "2026-10-19 12:02:06.{:03} | RX      3 B | L{:02}\r\n",
                index % 1000,
                index % 100
            ));
        }
        std::fs::write(&path, body).unwrap();

        let (lines, _) = tail_lines_with_end(&path, 5).unwrap();
        assert_eq!(lines.len(), 5);
        assert!(lines.last().unwrap().contains("L99"));
        assert!(lines[0].starts_with("2026-10-19"));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
