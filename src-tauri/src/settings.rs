use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub const MIN_BAUD_RATE: u32 = 50;
pub const MAX_BAUD_RATE: u32 = 4_000_000;
pub const MIN_LOG_INTERVAL_SECS: u64 = 5;
pub const MAX_LOG_INTERVAL_SECS: u64 = 3600;

fn default_receive_encoding() -> String {
    "utf-8".to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub port_name: String,
    pub baud_rate: u32,
    pub data_bits: u8,
    pub parity: String,
    pub stop_bits: String,
    pub flow_control: String,
    pub view_mode: String,
    #[serde(default = "default_receive_encoding")]
    pub receive_encoding: String,
    pub show_timestamps: bool,
    pub auto_scroll: bool,
    pub send_mode: String,
    pub line_ending: String,
    pub clear_after_send: bool,
    pub auto_log: bool,
    pub log_interval_secs: u64,
    pub log_dir: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            port_name: String::new(),
            baud_rate: 115_200,
            data_bits: 8,
            parity: "none".to_string(),
            stop_bits: "1".to_string(),
            flow_control: "none".to_string(),
            view_mode: "text".to_string(),
            receive_encoding: default_receive_encoding(),
            show_timestamps: true,
            auto_scroll: true,
            send_mode: "text".to_string(),
            line_ending: "none".to_string(),
            clear_after_send: false,
            auto_log: true,
            log_interval_secs: 30,
            log_dir: String::new(),
        }
    }
}

impl Settings {
    pub fn default_with_log_dir(log_dir: PathBuf) -> Self {
        Self {
            log_dir: log_dir.to_string_lossy().into_owned(),
            ..Self::default()
        }
    }

    /// Repairs a settings file that was edited by hand or written by an older
    /// version. Invalid values fall back to safe defaults instead of preventing
    /// the application from starting.
    pub fn sanitize(&mut self, fallback_log_dir: &Path) {
        if !(MIN_BAUD_RATE..=MAX_BAUD_RATE).contains(&self.baud_rate) {
            self.baud_rate = 115_200;
        }
        if !(5..=8).contains(&self.data_bits) {
            self.data_bits = 8;
        }
        if !matches!(self.parity.as_str(), "none" | "even" | "odd") {
            self.parity = "none".to_string();
        }
        if !matches!(self.stop_bits.as_str(), "1" | "2") {
            self.stop_bits = "1".to_string();
        }
        if !matches!(self.flow_control.as_str(), "none" | "hardware" | "software") {
            self.flow_control = "none".to_string();
        }
        if !matches!(self.view_mode.as_str(), "text" | "hex") {
            self.view_mode = "text".to_string();
        }
        self.receive_encoding = self.receive_encoding.trim().to_ascii_lowercase();
        if !matches!(self.receive_encoding.as_str(), "utf-8" | "gbk") {
            self.receive_encoding = default_receive_encoding();
        }
        if !matches!(self.send_mode.as_str(), "text" | "hex") {
            self.send_mode = "text".to_string();
        }
        if !matches!(self.line_ending.as_str(), "none" | "cr" | "lf" | "crlf") {
            self.line_ending = "none".to_string();
        }
        if !(MIN_LOG_INTERVAL_SECS..=MAX_LOG_INTERVAL_SECS).contains(&self.log_interval_secs) {
            self.log_interval_secs = 30;
        }

        let log_dir = PathBuf::from(self.log_dir.trim());
        if self.log_dir.trim().is_empty() || !log_dir.is_absolute() {
            self.log_dir = fallback_log_dir.to_string_lossy().into_owned();
        }
    }

    /// Strict validation for values coming from the UI. Nothing is silently
    /// clamped here: an invalid setting is shown to the user instead of being
    /// changed behind their back.
    pub fn validate(&self) -> Result<(), String> {
        if !(MIN_BAUD_RATE..=MAX_BAUD_RATE).contains(&self.baud_rate) {
            return Err(format!(
                "波特率需在 {MIN_BAUD_RATE}–{MAX_BAUD_RATE} bps 之间。"
            ));
        }
        if !(5..=8).contains(&self.data_bits) {
            return Err("数据位只能是 5、6、7 或 8。".to_string());
        }
        if !matches!(self.parity.as_str(), "none" | "even" | "odd") {
            return Err("校验位设置无效。".to_string());
        }
        if !matches!(self.stop_bits.as_str(), "1" | "2") {
            return Err("停止位设置无效。".to_string());
        }
        if !matches!(self.flow_control.as_str(), "none" | "hardware" | "software") {
            return Err("流控设置无效。".to_string());
        }
        if !matches!(self.view_mode.as_str(), "text" | "hex") {
            return Err("显示格式设置无效。".to_string());
        }
        if !matches!(self.receive_encoding.as_str(), "utf-8" | "gbk") {
            return Err("文本显示编码设置无效。".to_string());
        }
        if !matches!(self.send_mode.as_str(), "text" | "hex") {
            return Err("发送格式设置无效。".to_string());
        }
        if !matches!(self.line_ending.as_str(), "none" | "cr" | "lf" | "crlf") {
            return Err("结束符设置无效。".to_string());
        }
        if !(MIN_LOG_INTERVAL_SECS..=MAX_LOG_INTERVAL_SECS).contains(&self.log_interval_secs) {
            return Err(format!(
                "日志保存间隔需在 {MIN_LOG_INTERVAL_SECS}–{MAX_LOG_INTERVAL_SECS} 秒之间。"
            ));
        }
        let log_dir = self.log_dir.trim();
        if log_dir.is_empty() {
            return Err("请选择日志保存文件夹。".to_string());
        }
        if !Path::new(log_dir).is_absolute() {
            return Err("日志保存文件夹必须是绝对路径。".to_string());
        }
        Ok(())
    }
}

pub fn load(path: &Path, default_log_dir: &Path) -> Result<Settings, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("读取设置失败：{e}"))?;
    let mut settings: Settings =
        serde_json::from_str(&text).map_err(|e| format!("设置文件格式无效：{e}"))?;
    settings.sanitize(default_log_dir);
    Ok(settings)
}

pub fn save(path: &Path, settings: &Settings) -> Result<(), String> {
    settings.validate()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建设置目录失败：{e}"))?;
    }

    let json =
        serde_json::to_string_pretty(settings).map_err(|e| format!("序列化设置失败：{e}"))?;
    let temp_path = path.with_extension(format!("json.tmp-{}", std::process::id()));
    fs::write(&temp_path, json).map_err(|e| format!("写入临时设置文件失败：{e}"))?;

    if let Err(first_error) = fs::rename(&temp_path, path) {
        // Standard Windows rename replaces an existing file. If the platform
        // refuses, keep the previous settings file intact rather than deleting
        // it and risking a half-written configuration.
        let _ = fs::remove_file(&temp_path);
        return Err(format!("保存设置文件失败：{first_error}"));
    }
    Ok(())
}

pub fn default_log_dir(documents: Option<PathBuf>) -> PathBuf {
    let base = documents
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Serial Desk").join("Logs")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> Settings {
        Settings::default_with_log_dir(PathBuf::from("C:\\Logs"))
    }

    #[test]
    fn defaults_are_valid_and_log_safe() {
        let settings = valid();
        assert!(settings.validate().is_ok());
        assert_eq!(settings.baud_rate, 115_200);
        assert_eq!(settings.stop_bits, "1");
        assert_eq!(settings.receive_encoding, "utf-8");
        assert!(settings.auto_log);
    }

    #[test]
    fn rejects_unsupported_framing_options() {
        let mut settings = valid();
        settings.stop_bits = "1.5".to_string();
        assert!(settings.validate().is_err());

        let mut settings = valid();
        settings.parity = "mark".to_string();
        assert!(settings.validate().is_err());

        let mut settings = valid();
        settings.receive_encoding = "gb18030".to_string();
        assert!(settings.validate().is_err());
    }

    #[test]
    fn old_settings_without_encoding_default_to_utf8() {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "serial-desk-settings-migration-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        let mut value = serde_json::to_value(valid()).unwrap();
        value.as_object_mut().unwrap().remove("receiveEncoding");
        fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();

        let loaded = load(&path, Path::new("C:\\Fallback")).unwrap();
        assert_eq!(loaded.receive_encoding, "utf-8");
        assert!(loaded.validate().is_ok());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_relative_log_dir_and_bad_interval() {
        let mut settings = valid();
        settings.log_dir = "relative".to_string();
        assert!(settings.validate().is_err());

        let mut settings = valid();
        settings.log_interval_secs = 4;
        assert!(settings.validate().is_err());

        let mut settings = valid();
        settings.baud_rate = 0;
        assert!(settings.validate().is_err());
    }

    #[test]
    fn sanitizes_broken_values_for_startup() {
        let fallback = PathBuf::from("C:\\Fallback");
        let mut settings = valid();
        settings.baud_rate = 0;
        settings.data_bits = 99;
        settings.stop_bits = "7".to_string();
        settings.parity = "wat".to_string();
        settings.receive_encoding = "shift-jis".to_string();
        settings.log_interval_secs = 999_999;
        settings.log_dir = "".to_string();
        settings.sanitize(&fallback);
        assert_eq!(settings.baud_rate, 115_200);
        assert_eq!(settings.data_bits, 8);
        assert_eq!(settings.stop_bits, "1");
        assert_eq!(settings.parity, "none");
        assert_eq!(settings.receive_encoding, "utf-8");
        assert_eq!(settings.log_interval_secs, 30);
        assert_eq!(settings.log_dir, "C:\\Fallback");
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn round_trips_json() {
        let mut dir = std::env::temp_dir();
        dir.push(format!("serial-desk-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        let settings = valid();
        save(&path, &settings).unwrap();
        let loaded = load(&path, Path::new("C:\\Fallback")).unwrap();
        assert_eq!(loaded.baud_rate, settings.baud_rate);
        assert_eq!(loaded.log_dir, settings.log_dir);
        assert_eq!(loaded.receive_encoding, "utf-8");

        let _ = fs::remove_dir_all(&dir);
    }
}
