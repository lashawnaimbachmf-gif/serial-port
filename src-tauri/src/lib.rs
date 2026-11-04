mod logging;
#[allow(dead_code)]
mod logview;
mod serial;
mod settings;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use serde::Serialize;
use tauri::{
    ipc::{Channel, InvokeResponseBody},
    Manager, RunEvent, State, WindowEvent,
};

use crate::{
    logging::SessionLogStatus,
    serial::{
        encode_frame, ActiveSession, CloseReport, FrameKind, FrameSink, OpenReport, PortInfo,
        SendReport, SessionStatus,
    },
    settings::Settings,
};

pub struct AppState {
    settings: Mutex<Settings>,
    settings_path: PathBuf,
    session: Mutex<Option<ActiveSession>>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Bootstrap {
    settings: Settings,
    ports: Vec<PortInfo>,
    warnings: Vec<String>,
    version: String,
}

impl AppState {
    fn shutdown(&self) {
        let mut session_guard = lock(&self.session);
        if let Some(session) = session_guard.take() {
            let _ = session.close();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Encodes session frames into the `SD01` binary protocol the webview reads.
///
/// A failing UI channel must not tear down a valid hardware connection:
/// status polling also reveals any fatal hardware error, so send errors are
/// deliberately ignored here.
struct UiSink {
    channel: Mutex<Channel<InvokeResponseBody>>,
}

impl FrameSink for UiSink {
    fn frame(&self, kind: FrameKind, sequence: u32, timestamp_ms: i64, payload: &[u8]) {
        let frame = encode_frame(kind, sequence, timestamp_ms, payload);
        let _ = lock(&self.channel).send(InvokeResponseBody::Raw(frame));
    }
}

#[tauri::command]
fn bootstrap(state: State<'_, AppState>) -> Bootstrap {
    let mut warnings = state.warnings.clone();
    let ports = match serial::list_ports() {
        Ok(ports) => ports,
        Err(error) => {
            warnings.push(error);
            Vec::new()
        }
    };
    Bootstrap {
        settings: lock(&state.settings).clone(),
        ports,
        warnings,
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

#[tauri::command]
fn list_ports() -> Result<Vec<PortInfo>, String> {
    serial::list_ports()
}

#[tauri::command]
fn save_settings(settings: Settings, state: State<'_, AppState>) -> Result<(), String> {
    settings.validate()?;
    let settings_path = state.settings_path.clone();
    settings::save(&settings_path, &settings)?;

    let interval = settings.log_interval_secs;
    *lock(&state.settings) = settings;

    // The interval is safe to change live; it only affects how often the
    // existing log file is flushed. Address and format changes apply to the
    // next session.
    if let Some(session) = lock(&state.session).as_ref() {
        session.core().set_log_interval(interval);
    }
    Ok(())
}

#[tauri::command]
fn pick_log_dir(initial: Option<String>) -> Result<Option<String>, String> {
    let mut dialog = rfd::FileDialog::new().set_title("选择日志保存文件夹");
    if let Some(initial) = initial {
        let path = PathBuf::from(initial);
        if path.is_dir() {
            dialog = dialog.set_directory(path);
        }
    }
    Ok(dialog
        .pick_folder()
        .map(|path| path.to_string_lossy().into_owned()))
}

#[tauri::command]
fn open_log_folder(state: State<'_, AppState>) -> Result<(), String> {
    let folder = PathBuf::from(lock(&state.settings).log_dir.clone());
    if !folder.is_absolute() {
        return Err("日志保存文件夹不是有效路径。".to_string());
    }
    std::fs::create_dir_all(&folder)
        .map_err(|error| format!("无法创建日志文件夹“{}”：{error}", folder.display()))?;
    std::process::Command::new("explorer.exe")
        .arg(&folder)
        .spawn()
        .map_err(|error| format!("无法打开日志文件夹：{error}"))?;
    Ok(())
}

#[tauri::command]
fn pick_log_file(state: State<'_, AppState>) -> Option<String> {
    let log_dir = lock(&state.settings).log_dir.clone();
    let mut dialog = rfd::FileDialog::new()
        .set_title("打开保存的串口日志")
        .add_filter("Serial Desk 日志", &["log"]);
    let directory = PathBuf::from(&log_dir);
    if directory.is_dir() {
        dialog = dialog.set_directory(directory);
    }
    dialog
        .pick_file()
        .map(|path| path.to_string_lossy().into_owned())
}

#[tauri::command]
async fn read_log_file(path: String) -> Result<logview::LogViewDocument, String> {
    tauri::async_runtime::spawn_blocking(move || logview::read(&PathBuf::from(path)))
        .await
        .map_err(|error| format!("读取日志任务异常：{error}"))?
}

#[tauri::command]
async fn open_port(
    settings: Settings,
    data_channel: Channel<InvokeResponseBody>,
    app: tauri::AppHandle,
) -> Result<OpenReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        settings.validate()?;
        let state = app.state::<AppState>();
        let mut session_guard = lock(&state.session);
        if session_guard.is_some() {
            return Err("已有串口连接，请先关闭当前连接。".to_string());
        }

        let session = ActiveSession::open(
            &settings,
            Arc::new(UiSink {
                channel: Mutex::new(data_channel),
            }),
        )?;
        let status = session.status();
        let info = status
            .info
            .ok_or_else(|| "无法读取串口会话信息。".to_string())?;
        let report = OpenReport {
            info,
            log: status.log,
        };
        *session_guard = Some(session);
        Ok(report)
    })
    .await
    .map_err(|error| format!("打开串口任务异常：{error}"))?
}

#[tauri::command]
async fn close_port(app: tauri::AppHandle) -> Result<CloseReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let mut session_guard = lock(&state.session);
        let session = session_guard
            .take()
            .ok_or_else(|| "当前没有打开的串口。".to_string())?;
        Ok(session.close())
    })
    .await
    .map_err(|error| format!("关闭串口任务异常：{error}"))?
}

#[tauri::command]
async fn send_data(bytes: Vec<u8>, app: tauri::AppHandle) -> Result<SendReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let core = {
            let session_guard = lock(&state.session);
            session_guard
                .as_ref()
                .map(|session| session.core())
                .ok_or_else(|| "当前没有打开的串口。".to_string())?
        };
        core.send(&bytes)
    })
    .await
    .map_err(|error| format!("发送任务异常：{error}"))?
}

#[tauri::command]
fn get_session_status(state: State<'_, AppState>) -> SessionStatus {
    let core = {
        let session_guard = lock(&state.session);
        session_guard.as_ref().map(|session| session.core())
    };
    match core {
        Some(core) => core.status(),
        None => SessionStatus {
            connected: false,
            info: None,
            rx_bytes: 0,
            tx_bytes: 0,
            rx_frames: 0,
            tx_frames: 0,
            fatal_error: None,
            log: None,
        },
    }
}

#[tauri::command]
fn save_log_now(state: State<'_, AppState>) -> Result<SessionLogStatus, String> {
    let core = {
        let session_guard = lock(&state.session);
        session_guard
            .as_ref()
            .map(|session| session.core())
            .ok_or_else(|| "当前没有打开的串口。".to_string())?
    };
    core.sync_log_now()
}

#[tauri::command]
fn show_main_window(app: tauri::AppHandle) {
    // The frontend calls this as soon as its first frame is ready. Relying on
    // `on_page_load` alone was not reliable on Windows: the window stayed
    // hidden until the 3 s fallback, which made startup feel slow.
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn build_app() -> tauri::App {
    tauri::Builder::default()
        .setup(|app| {
            let config_dir = app
                .path()
                .app_config_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("serial-desk"));
            let settings_path = config_dir.join("settings.json");
            let fallback_log_dir = settings::default_log_dir(app.path().document_dir().ok());
            let mut warnings = Vec::new();

            let mut loaded_settings = if settings_path.exists() {
                match settings::load(&settings_path, &fallback_log_dir) {
                    Ok(settings) => settings,
                    Err(error) => {
                        warnings.push(format!("{error}。本次启动使用默认设置，原文件未被覆盖。"));
                        Settings::default_with_log_dir(fallback_log_dir.clone())
                    }
                }
            } else {
                Settings::default_with_log_dir(fallback_log_dir.clone())
            };
            loaded_settings.sanitize(&fallback_log_dir);

            if !settings_path.exists() {
                if let Err(error) = settings::save(&settings_path, &loaded_settings) {
                    warnings.push(format!("无法保存初始设置：{error}"));
                }
            }

            app.manage(AppState {
                settings: Mutex::new(loaded_settings),
                settings_path,
                session: Mutex::new(None),
                warnings,
            });

            // Defensive fallback: the page-load event below normally reveals
            // the window without a white flash. If the page cannot load, do not
            // leave an invisible process behind.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(3));
                if let Some(window) = handle.get_webview_window("main") {
                    let _ = window.show();
                }
            });

            Ok(())
        })
        .on_page_load(|window, payload| {
            if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                let _ = window.show();
                let _ = window.set_focus();
            }
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { .. } = event {
                if let Some(state) = window.try_state::<AppState>() {
                    state.shutdown();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            list_ports,
            save_settings,
            pick_log_dir,
            open_log_folder,
            pick_log_file,
            read_log_file,
            open_port,
            close_port,
            send_data,
            get_session_status,
            save_log_now,
            show_main_window
        ])
        .build(tauri::generate_context!())
        .expect("failed to build Serial Desk")
}

pub fn run() {
    let app = build_app();
    app.run(|app_handle, event| {
        if matches!(event, RunEvent::ExitRequested { .. } | RunEvent::Exit) {
            if let Some(state) = app_handle.try_state::<AppState>() {
                state.shutdown();
            }
        }
    });
}
