//! Frame representation and the text / JSONL / hex / raw writers.

use std::io::{self, BufWriter, Stdout, Write};

use serde_json::{json, Value};

use super::args::Opts;
use super::{CliResult, Fail};
use crate::logging::escape_bytes;
use crate::serial::FrameKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Rx,
    Tx,
    Error,
}

impl Dir {
    pub fn label(self) -> &'static str {
        match self {
            Self::Rx => "RX",
            Self::Tx => "TX",
            Self::Error => "ERR",
        }
    }

    pub fn from_kind(kind: FrameKind) -> Self {
        match kind {
            FrameKind::Rx => Self::Rx,
            FrameKind::Tx => Self::Tx,
            FrameKind::Error => Self::Error,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Live,
    Log,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Log => "log",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
    Hex,
    Raw,
}

impl Format {
    /// `--json` is shorthand for `--format json`; combining both is an error.
    pub fn from_opts(opts: &Opts) -> CliResult<Self> {
        let explicit = opts
            .value("format")
            .map(|value| value.trim().to_ascii_lowercase());
        let format = match explicit.as_deref() {
            None => {
                if opts.has("json") {
                    Self::Json
                } else {
                    Self::Text
                }
            }
            Some("json") => Self::Json,
            Some("text") => Self::Text,
            Some("hex") => Self::Hex,
            Some("raw") => Self::Raw,
            Some(other) => {
                return Err(Fail::usage(format!(
                    "未知输出格式“{other}”。可选：text、json、hex、raw。"
                )));
            }
        };

        if opts.has("json") && matches!(format, Self::Text | Self::Hex | Self::Raw) {
            return Err(Fail::usage(
                "--json 与 --format 不能同时指定为不同格式。".to_string(),
            ));
        }
        Ok(format)
    }
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub seq: Option<u32>,
    pub ts_ms: i64,
    pub ts: String,
    pub dir: Dir,
    pub source: Source,
    pub bytes: Vec<u8>,
}

impl Frame {
    pub fn live(kind: FrameKind, sequence: u32, ts_ms: i64, payload: &[u8]) -> Self {
        Self {
            seq: Some(sequence),
            ts_ms,
            ts: super::util::format_local(ts_ms),
            dir: Dir::from_kind(kind),
            source: Source::Live,
            bytes: payload.to_vec(),
        }
    }

    pub fn from_parts(ts_ms: i64, ts: String, dir: Dir, bytes: Vec<u8>) -> Self {
        Self {
            seq: None,
            ts_ms,
            ts,
            dir,
            source: Source::Log,
            bytes,
        }
    }

    pub fn json(&self) -> Value {
        json!({
            "type": "frame",
            "source": self.source.label(),
            "seq": self.seq,
            "ts": self.ts,
            "tsMs": self.ts_ms,
            "dir": self.dir.label(),
            "bytes": self.bytes.len(),
            "text": escape_bytes(&self.bytes),
            "hex": hex_string(&self.bytes),
        })
    }
}

pub struct FrameWriter {
    format: Format,
    out: BufWriter<Stdout>,
}

impl FrameWriter {
    pub fn new(format: Format) -> Self {
        Self {
            format,
            out: BufWriter::new(io::stdout()),
        }
    }

    pub fn is_json(&self) -> bool {
        matches!(self.format, Format::Json)
    }

    pub fn is_raw(&self) -> bool {
        matches!(self.format, Format::Raw)
    }

    pub fn write_frame(&mut self, frame: &Frame) -> io::Result<()> {
        match self.format {
            Format::Text => writeln!(self.out, "{}", text_line(frame))?,
            Format::Hex => writeln!(self.out, "{}", hex_line(frame))?,
            Format::Json => {
                let line = serde_json::to_string(&frame.json()).map_err(io::Error::other)?;
                writeln!(self.out, "{line}")?;
            }
            Format::Raw => {
                // Raw mode is a byte pipe: only received data belongs in it.
                if frame.dir == Dir::Rx {
                    self.out.write_all(&frame.bytes)?;
                }
            }
        }
        self.out.flush()
    }

    /// Writes a non-frame event. In raw mode stdout is a byte pipe, so events
    /// go to stderr instead of corrupting the data stream.
    pub fn write_event(&mut self, value: &Value) -> io::Result<()> {
        let line = serde_json::to_string(value).map_err(io::Error::other)?;
        if self.is_raw() {
            eprintln!("{line}");
            return Ok(());
        }
        writeln!(self.out, "{line}")?;
        self.out.flush()
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

/// The exact line layout the session logger writes.
///
/// Keeping this identical means `sdesk ... --format text` output can be pasted
/// into a session log without changing its structure.
pub fn text_line(frame: &Frame) -> String {
    format!(
        "{} | {:<3}{:>6} B | {}",
        frame.ts,
        frame.dir.label(),
        frame.bytes.len(),
        escape_bytes(&frame.bytes)
    )
}

pub fn hex_line(frame: &Frame) -> String {
    format!(
        "{} | {:<3}{:>6} B | {}",
        frame.ts,
        frame.dir.label(),
        frame.bytes.len(),
        hex_string(&frame.bytes)
    )
}

pub fn hex_string(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut output = String::with_capacity(bytes.len() * 3);
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 {
            output.push(' ');
        }
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0F) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(bytes: &[u8]) -> Frame {
        Frame {
            seq: Some(1),
            ts_ms: 1_700_000_000_123,
            ts: "2026-10-19 12:02:06.521".to_string(),
            dir: Dir::Rx,
            source: Source::Live,
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn hex_is_uppercase_and_space_separated() {
        assert_eq!(hex_string(&[0x00, 0x1B, 0xFF]), "00 1B FF");
        assert_eq!(hex_string(&[]), "");
    }

    #[test]
    fn json_frame_keeps_text_lossless_and_bytes_exact() {
        let value = frame(&[0x41, 0x0D, 0x0A, 0xFF]).json();
        assert_eq!(value["type"], "frame");
        assert_eq!(value["dir"], "RX");
        assert_eq!(value["bytes"], 4);
        assert_eq!(value["text"], "A\\r\\n\\xFF");
        assert_eq!(value["hex"], "41 0D 0A FF");
        assert_eq!(value["seq"], 1);
        assert_eq!(value["source"], "live");
    }

    #[test]
    fn text_line_matches_the_session_log_layout() {
        let value = Frame {
            seq: None,
            ts_ms: 1_700_000_000_000,
            ts: "2026-10-19 12:02:06.521".to_string(),
            dir: Dir::Tx,
            source: Source::Log,
            bytes: b"OK\r\n".to_vec(),
        };
        assert_eq!(
            text_line(&value),
            "2026-10-19 12:02:06.521 | TX      4 B | OK\\r\\n"
        );
    }

    #[test]
    fn format_flags_conflict_is_rejected() {
        let argv = vec!["--json".to_string(), "--format".into(), "hex".into()];
        let opts = super::super::args::parse(&argv, &[("json", false), ("format", true)]).unwrap();
        assert!(Format::from_opts(&opts).is_err());
    }
}
