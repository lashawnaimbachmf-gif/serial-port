use std::{
    io::{self, Read, Write},
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use serialport::{DataBits, FlowControl, Parity, SerialPort, SerialPortType, StopBits};

use crate::{
    logging::{Direction, Logger, SessionLogStatus},
    settings::Settings,
};

const READ_TIMEOUT_MS: u64 = 12;
const WRITE_TIMEOUT_MS: u32 = 60_000;
const AGGREGATE_WINDOW_MS: u64 = 12;
const MAX_FRAME_BYTES: usize = 64 * 1024;
const READ_BUFFER_BYTES: usize = 16 * 1024;
const SEND_LIMIT_BYTES: usize = 1024 * 1024;
const FRAME_MAGIC: &[u8; 4] = b"SD01";
const FRAME_HEADER_BYTES: usize = 20;

/// What a frame carries. The numeric codes are part of the UI frame protocol
/// (`SD01`) and must not change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    Rx,
    Tx,
    Error,
}

impl FrameKind {
    pub fn code(self) -> u8 {
        match self {
            Self::Rx => 1,
            Self::Tx => 2,
            Self::Error => 3,
        }
    }
}

/// Receives every frame the serial session produces.
///
/// The GUI encodes frames into the binary `SD01` protocol for its webview; the
/// CLI prints them as text or JSONL. Both must observe the same bytes in the
/// same order, so the session core owns no presentation code.
pub trait FrameSink: Send + Sync {
    fn frame(&self, kind: FrameKind, sequence: u32, timestamp_ms: i64, payload: &[u8]);
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortInfo {
    pub name: String,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub port_name: String,
    pub baud_rate: u32,
    pub data_bits: u8,
    pub parity: String,
    pub stop_bits: String,
    pub flow_control: String,
    pub framing: String,
    pub flow_label: String,
    pub opened_at_ms: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatus {
    pub connected: bool,
    pub info: Option<SessionInfo>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_frames: u64,
    pub tx_frames: u64,
    pub fatal_error: Option<String>,
    pub log: Option<SessionLogStatus>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenReport {
    pub info: SessionInfo,
    pub log: Option<SessionLogStatus>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendReport {
    pub bytes_sent: u64,
    pub timestamp_ms: i64,
    pub tx_bytes_total: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseReport {
    pub log: Option<SessionLogStatus>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_frames: u64,
    pub tx_frames: u64,
    pub fatal_error: Option<String>,
    pub duration_ms: u64,
}

struct Shared {
    sink: Arc<dyn FrameSink>,
    logger: Mutex<Option<Logger>>,
    fatal_error: Mutex<Option<String>>,
    sequence: AtomicU32,
    rx_bytes: AtomicU64,
    tx_bytes: AtomicU64,
    rx_frames: AtomicU64,
    tx_frames: AtomicU64,
}

pub struct SessionCore {
    writer: Mutex<Option<Box<dyn SerialPort>>>,
    stop: AtomicBool,
    shared: Arc<Shared>,
    info: SessionInfo,
    opened_at: DateTime<Local>,
}

pub struct ActiveSession {
    core: Arc<SessionCore>,
    reader: JoinHandle<()>,
    ticker: JoinHandle<()>,
}

impl ActiveSession {
    pub fn open(settings: &Settings, sink: Arc<dyn FrameSink>) -> Result<Self, String> {
        settings.validate()?;
        let port_name = settings.port_name.trim();
        if port_name.is_empty() {
            return Err("请选择一个串口。".to_string());
        }

        let data_bits = data_bits(settings.data_bits)?;
        let parity = parity(&settings.parity)?;
        let stop_bits = stop_bits(&settings.stop_bits)?;
        let flow_control = flow_control(&settings.flow_control)?;
        let serial_description = format!(
            "{} bps | {}",
            settings.baud_rate,
            framing(settings.data_bits, &settings.parity, &settings.stop_bits)
        );
        let flow_label = flow_label(&settings.flow_control).to_string();

        let port = serialport::new(port_name, settings.baud_rate)
            .data_bits(data_bits)
            .parity(parity)
            .stop_bits(stop_bits)
            .flow_control(flow_control)
            .timeout(Duration::from_millis(READ_TIMEOUT_MS))
            .open_native()
            .map_err(|error| {
                let detail = error
                    .to_string()
                    .trim_end_matches(|ch: char| ch.is_whitespace() || ch == '。' || ch == '.')
                    .to_string();
                format!(
                    "无法打开串口 {port_name}：{detail}。请确认端口未被其他程序占用，且设备仍然连接。"
                )
            })?;

        let reader_port = port
            .try_clone_native()
            .map_err(|error| format!("无法建立串口读取通道：{error}。请尝试重新插拔设备。"))?;

        configure_comm_timeouts(&port)?;
        configure_comm_timeouts(&reader_port)?;

        let port: Box<dyn SerialPort> = Box::new(port);
        let reader_port: Box<dyn SerialPort> = Box::new(reader_port);

        // The logger is opened after the port succeeds but before the reader
        // starts, so no received byte can exist before its log is ready.
        let logger = if settings.auto_log {
            Some(Logger::create(
                std::path::Path::new(settings.log_dir.trim()),
                port_name,
                &serial_description,
                &flow_label,
                settings.log_interval_secs,
            )?)
        } else {
            None
        };

        let opened_at = Local::now();
        let info = SessionInfo {
            port_name: port_name.to_string(),
            baud_rate: settings.baud_rate,
            data_bits: settings.data_bits,
            parity: settings.parity.clone(),
            stop_bits: settings.stop_bits.clone(),
            flow_control: settings.flow_control.clone(),
            framing: framing(settings.data_bits, &settings.parity, &settings.stop_bits),
            flow_label: flow_label.clone(),
            opened_at_ms: opened_at.timestamp_millis(),
        };

        let shared = Arc::new(Shared {
            sink,
            logger: Mutex::new(logger),
            fatal_error: Mutex::new(None),
            sequence: AtomicU32::new(0),
            rx_bytes: AtomicU64::new(0),
            tx_bytes: AtomicU64::new(0),
            rx_frames: AtomicU64::new(0),
            tx_frames: AtomicU64::new(0),
        });

        let core = Arc::new(SessionCore {
            writer: Mutex::new(Some(port)),
            stop: AtomicBool::new(false),
            shared: shared.clone(),
            info: info.clone(),
            opened_at,
        });

        let reader_core = core.clone();
        let reader = thread::Builder::new()
            .name("serial-desk-reader".to_string())
            .spawn(move || read_loop(reader_port, reader_core))
            .map_err(|error| format!("无法启动串口读取线程：{error}"))?;

        let ticker_core = core.clone();
        let ticker = match thread::Builder::new()
            .name("serial-desk-log-ticker".to_string())
            .spawn(move || log_loop(ticker_core))
        {
            Ok(handle) => handle,
            Err(error) => {
                core.stop.store(true, Ordering::SeqCst);
                let _ = reader.join();
                if let Some(logger) = lock(&core.shared.logger).take() {
                    let _ = logger.finish();
                }
                return Err(format!("无法启动日志保存线程：{error}"));
            }
        };

        Ok(Self {
            core,
            reader,
            ticker,
        })
    }

    pub fn core(&self) -> Arc<SessionCore> {
        self.core.clone()
    }

    pub fn status(&self) -> SessionStatus {
        self.core.status()
    }

    pub fn close(self) -> CloseReport {
        let Self {
            core,
            reader,
            ticker,
        } = self;

        core.stop.store(true, Ordering::SeqCst);
        let fatal_error = lock(&core.shared.fatal_error).clone();

        // Wait for the read and log threads first. Then take the writer, which
        // also waits for any in-flight send that cloned the core handle.
        let _ = reader.join();
        let _ = ticker.join();
        let writer = lock(&core.writer).take();
        drop(writer);

        let rx_bytes = core.shared.rx_bytes.load(Ordering::SeqCst);
        let tx_bytes = core.shared.tx_bytes.load(Ordering::SeqCst);
        let rx_frames = core.shared.rx_frames.load(Ordering::SeqCst);
        let tx_frames = core.shared.tx_frames.load(Ordering::SeqCst);
        let log = lock(&core.shared.logger)
            .take()
            .map(|logger| logger.finish());
        let duration_ms = (Local::now() - core.opened_at).num_milliseconds().max(0) as u64;

        CloseReport {
            log,
            rx_bytes,
            tx_bytes,
            rx_frames,
            tx_frames,
            fatal_error,
            duration_ms,
        }
    }
}

impl SessionCore {
    pub fn status(&self) -> SessionStatus {
        let fatal_error = lock(&self.shared.fatal_error).clone();
        SessionStatus {
            connected: true,
            info: Some(self.info.clone()),
            rx_bytes: self.shared.rx_bytes.load(Ordering::SeqCst),
            tx_bytes: self.shared.tx_bytes.load(Ordering::SeqCst),
            rx_frames: self.shared.rx_frames.load(Ordering::SeqCst),
            tx_frames: self.shared.tx_frames.load(Ordering::SeqCst),
            fatal_error,
            log: lock(&self.shared.logger)
                .as_ref()
                .map(|logger| logger.status()),
        }
    }

    pub fn send(&self, bytes: &[u8]) -> Result<SendReport, String> {
        if bytes.is_empty() {
            return Err("没有可发送的数据。".to_string());
        }
        if bytes.len() > SEND_LIMIT_BYTES {
            return Err(format!("单次发送不能超过 {} KB。", SEND_LIMIT_BYTES / 1024));
        }
        if self.stop.load(Ordering::SeqCst) {
            return Err("串口已经关闭。".to_string());
        }
        if lock(&self.shared.fatal_error).is_some() {
            return Err("串口连接已中断，请重新连接后再发送。".to_string());
        }

        let started_at = Local::now();
        let mut writer_guard = lock(&self.writer);
        let writer = writer_guard
            .as_mut()
            .ok_or_else(|| "串口已经关闭。".to_string())?;

        if self.stop.load(Ordering::SeqCst) {
            return Err("串口已经关闭。".to_string());
        }

        let mut written = 0usize;
        let mut write_error = None;
        while written < bytes.len() {
            match writer.write(&bytes[written..]) {
                Ok(0) => {
                    write_error = Some(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "串口驱动未接受任何字节",
                    ));
                    break;
                }
                Ok(count) => written += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    write_error = Some(error);
                    break;
                }
            }
        }

        let timestamp_ms = started_at.timestamp_millis();
        if written > 0 {
            let sent = &bytes[..written];
            self.shared
                .tx_bytes
                .fetch_add(written as u64, Ordering::SeqCst);
            self.shared.tx_frames.fetch_add(1, Ordering::SeqCst);
            self.shared.log(Direction::Tx, started_at, sent);
            self.shared.emit(FrameKind::Tx, timestamp_ms, sent);
        }

        if let Some(error) = write_error {
            return Err(format!(
                "发送失败，已写入 {written}/{} 字节：{error}",
                bytes.len()
            ));
        }

        Ok(SendReport {
            bytes_sent: written as u64,
            timestamp_ms,
            tx_bytes_total: self.shared.tx_bytes.load(Ordering::SeqCst),
        })
    }

    pub fn set_log_interval(&self, interval_secs: u64) {
        if let Some(logger) = lock(&self.shared.logger).as_mut() {
            logger.set_interval(interval_secs);
        }
    }

    pub fn sync_log_now(&self) -> Result<SessionLogStatus, String> {
        let mut logger_guard = lock(&self.shared.logger);
        let logger = logger_guard
            .as_mut()
            .ok_or_else(|| "本次连接未启用日志记录。".to_string())?;
        logger.sync_now();
        Ok(logger.status())
    }
}

impl Shared {
    fn log(&self, direction: Direction, timestamp: DateTime<Local>, bytes: &[u8]) {
        if let Some(logger) = lock(&self.logger).as_mut() {
            logger.append(direction, timestamp, bytes);
        }
    }

    fn flush_logs(&self) {
        if let Some(logger) = lock(&self.logger).as_mut() {
            logger.flush_if_due();
        }
    }

    fn set_fatal(&self, message: String) {
        let mut fatal = lock(&self.fatal_error);
        if fatal.is_none() {
            *fatal = Some(message);
        }
    }

    fn emit(&self, kind: FrameKind, timestamp_ms: i64, payload: &[u8]) {
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
        self.sink.frame(kind, sequence, timestamp_ms, payload);
    }

    fn emit_error(&self, message: &str) {
        self.emit(
            FrameKind::Error,
            Local::now().timestamp_millis(),
            message.as_bytes(),
        );
    }
}

fn read_loop(mut port: Box<dyn SerialPort>, core: Arc<SessionCore>) {
    let shared = core.shared.clone();
    let mut buffer = [0u8; READ_BUFFER_BYTES];

    loop {
        if core.stop.load(Ordering::SeqCst) {
            break;
        }

        match port.read(&mut buffer) {
            Ok(0) => continue,
            Ok(count) => {
                let timestamp = Local::now();
                let mut payload = Vec::with_capacity(count.min(MAX_FRAME_BYTES));
                payload.extend_from_slice(&buffer[..count]);
                let deadline = Instant::now() + Duration::from_millis(AGGREGATE_WINDOW_MS);
                let mut terminal_error = None;

                while payload.len() < MAX_FRAME_BYTES {
                    if Instant::now() >= deadline {
                        break;
                    }
                    let remaining = MAX_FRAME_BYTES - payload.len();
                    let capacity = remaining.min(buffer.len());
                    match port.read(&mut buffer[..capacity]) {
                        Ok(0) => break,
                        Ok(extra) => {
                            payload.extend_from_slice(&buffer[..extra]);
                        }
                        Err(ref error)
                            if error.kind() == io::ErrorKind::TimedOut
                                || error.kind() == io::ErrorKind::WouldBlock =>
                        {
                            break;
                        }
                        Err(ref error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(error) => {
                            terminal_error = Some(error);
                            break;
                        }
                    }
                }

                let timestamp_ms = timestamp.timestamp_millis();
                shared
                    .rx_bytes
                    .fetch_add(payload.len() as u64, Ordering::SeqCst);
                shared.rx_frames.fetch_add(1, Ordering::SeqCst);
                shared.log(Direction::Rx, timestamp, &payload);
                shared.emit(FrameKind::Rx, timestamp_ms, &payload);

                if let Some(error) = terminal_error {
                    let message = format!("串口读取中断：{error}。请检查设备连接。");
                    shared.set_fatal(message.clone());
                    shared.emit_error(&message);
                    break;
                }
            }
            Err(ref error)
                if error.kind() == io::ErrorKind::TimedOut
                    || error.kind() == io::ErrorKind::WouldBlock => {}
            Err(ref error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                let message = format!("串口读取失败：{error}。设备可能已被拔出。");
                shared.set_fatal(message.clone());
                shared.emit_error(&message);
                break;
            }
        }
    }
}

fn log_loop(core: Arc<SessionCore>) {
    while !core.stop.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(250));
        if core.stop.load(Ordering::SeqCst) {
            break;
        }
        core.shared.flush_logs();
    }
}

pub fn list_ports() -> Result<Vec<PortInfo>, String> {
    let ports = serialport::available_ports().map_err(|error| format!("枚举串口失败：{error}"))?;
    let mut infos: Vec<PortInfo> = ports
        .into_iter()
        .map(|port| {
            let detail = match &port.port_type {
                SerialPortType::UsbPort(usb) => usb
                    .product
                    .clone()
                    .or_else(|| usb.manufacturer.clone())
                    .unwrap_or_else(|| "USB 串口".to_string()),
                SerialPortType::PciPort => "PCI 串口".to_string(),
                SerialPortType::BluetoothPort => "蓝牙串口".to_string(),
                SerialPortType::Unknown => "串口设备".to_string(),
            };
            PortInfo {
                name: port.port_name.clone(),
                detail: clean_port_detail(detail, &port.port_name),
            }
        })
        .collect();

    infos.sort_by_key(|left| natural_port_key(&left.name));
    Ok(infos)
}

fn natural_port_key(name: &str) -> (u8, u32, String) {
    if let Some(number) = name
        .strip_prefix("COM")
        .and_then(|tail| tail.parse::<u32>().ok())
    {
        (0, number, String::new())
    } else {
        (1, 0, name.to_ascii_lowercase())
    }
}

fn clean_port_detail(detail: String, port_name: &str) -> String {
    let suffix = format!(" ({port_name})");
    let cleaned = detail
        .strip_suffix(&suffix)
        .map(|value| value.trim_end().to_string())
        .unwrap_or(detail);
    if cleaned.trim().is_empty() {
        "串口设备".to_string()
    } else {
        cleaned
    }
}

pub fn encode_frame(kind: FrameKind, sequence: u32, timestamp_ms: i64, payload: &[u8]) -> Vec<u8> {
    debug_assert!(!FRAME_MAGIC.is_empty());
    let mut frame = Vec::with_capacity(FRAME_HEADER_BYTES + payload.len());
    frame.extend_from_slice(FRAME_MAGIC);
    frame.extend_from_slice(&(timestamp_ms as f64).to_le_bytes());
    frame.extend_from_slice(&sequence.to_le_bytes());
    frame.push(kind.code());
    frame.push(0);
    frame.extend_from_slice(&[0u8, 0u8]);
    frame.extend_from_slice(payload);
    frame
}

fn data_bits(value: u8) -> Result<DataBits, String> {
    match value {
        5 => Ok(DataBits::Five),
        6 => Ok(DataBits::Six),
        7 => Ok(DataBits::Seven),
        8 => Ok(DataBits::Eight),
        _ => Err("数据位只能是 5、6、7 或 8。".to_string()),
    }
}

fn parity(value: &str) -> Result<Parity, String> {
    match value {
        "none" => Ok(Parity::None),
        "even" => Ok(Parity::Even),
        "odd" => Ok(Parity::Odd),
        _ => Err("校验位设置无效。".to_string()),
    }
}

fn stop_bits(value: &str) -> Result<StopBits, String> {
    match value {
        "1" => Ok(StopBits::One),
        "2" => Ok(StopBits::Two),
        _ => Err("停止位设置无效。".to_string()),
    }
}

fn flow_control(value: &str) -> Result<FlowControl, String> {
    match value {
        "none" => Ok(FlowControl::None),
        "hardware" => Ok(FlowControl::Hardware),
        "software" => Ok(FlowControl::Software),
        _ => Err("流控设置无效。".to_string()),
    }
}

fn framing(data_bits: u8, parity: &str, stop_bits: &str) -> String {
    let parity_code = match parity {
        "none" => "N",
        "even" => "E",
        "odd" => "O",
        _ => "?",
    };
    format!("{data_bits}-{parity_code}-{stop_bits}")
}

fn flow_label(value: &str) -> &'static str {
    match value {
        "hardware" => "RTS/CTS",
        "software" => "XON/XOFF",
        _ => "none",
    }
}

/// serialport applies its single timeout to both reads and writes (12 ms in
/// our configuration). That is appropriate for a polling read, but far too
/// short for a write: a large packet can be cut off while the driver is still
/// draining its buffer. Set explicit, separate timeouts through the native
/// handle. Reads still return every 12 ms while idle; writes are allowed 60 s
/// to reach the driver, and a timeout is always surfaced as a partial-write
/// error rather than being treated as success.
#[cfg(windows)]
fn configure_comm_timeouts(port: &serialport::COMPort) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::{
        Devices::Communication::{SetCommTimeouts, COMMTIMEOUTS},
        Foundation::HANDLE,
    };

    let timeouts = COMMTIMEOUTS {
        ReadIntervalTimeout: u32::MAX,
        ReadTotalTimeoutMultiplier: u32::MAX,
        ReadTotalTimeoutConstant: READ_TIMEOUT_MS as u32,
        WriteTotalTimeoutMultiplier: 0,
        WriteTotalTimeoutConstant: WRITE_TIMEOUT_MS,
    };

    let success = unsafe {
        SetCommTimeouts(
            port.as_raw_handle() as HANDLE,
            &timeouts as *const COMMTIMEOUTS,
        )
    };
    if success == 0 {
        return Err(format!(
            "配置串口读写超时失败：{}",
            io::Error::last_os_error()
        ));
    }
    Ok(())
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
    fn frame_header_is_stable_and_raw_payload_is_untouched() {
        let payload = [0x00, 0xFF, 0x0D, 0x0A, 0xE4, 0xB8];
        let frame = encode_frame(FrameKind::Rx, 7, 1_700_000_000_123, &payload);
        assert_eq!(frame.len(), FRAME_HEADER_BYTES + payload.len());
        assert_eq!(&frame[0..4], b"SD01");
        assert_eq!(
            f64::from_le_bytes(frame[4..12].try_into().unwrap()) as i64,
            1_700_000_000_123
        );
        assert_eq!(u32::from_le_bytes(frame[12..16].try_into().unwrap()), 7);
        assert_eq!(frame[16], FrameKind::Rx.code());
        assert_eq!(&frame[FRAME_HEADER_BYTES..], &payload);
    }

    #[test]
    fn natural_sort_puts_com10_after_com8() {
        let names = ["COM10", "COM8", "COM2"];
        let mut actual = names.map(natural_port_key);
        actual.sort();
        let sorted: Vec<_> = actual.into_iter().map(|(_, number, _)| number).collect();
        assert_eq!(sorted, vec![2, 8, 10]);
    }

    #[test]
    fn framing_description_matches_common_notation() {
        assert_eq!(framing(8, "none", "1"), "8-N-1");
        assert_eq!(framing(7, "even", "2"), "7-E-2");
    }

    #[test]
    fn validates_serial_mappings() {
        assert!(data_bits(8).is_ok());
        assert!(data_bits(9).is_err());
        assert!(parity("none").is_ok());
        assert!(parity("mark").is_err());
        assert!(stop_bits("1").is_ok());
        assert!(stop_bits("1.5").is_err());
        assert!(flow_control("hardware").is_ok());
        assert!(flow_control("magic").is_err());
    }

    #[test]
    fn cleans_duplicate_port_suffix_from_usb_product_names() {
        assert_eq!(
            clean_port_detail("USB-SERIAL CH340 (COM10)".to_string(), "COM10"),
            "USB-SERIAL CH340"
        );
        assert_eq!(
            clean_port_detail("Silicon Labs CP210x USB to UART Bridge".to_string(), "COM8"),
            "Silicon Labs CP210x USB to UART Bridge"
        );
        assert_eq!(clean_port_detail(" (COM8)".to_string(), "COM8"), "串口设备");
    }
}
