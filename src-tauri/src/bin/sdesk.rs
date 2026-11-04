//! `sdesk` — the Serial Desk command line interface.
//!
//! The GUI is a Tauri application, but the byte handling, the session logger
//! and the log format are plain Rust. This binary compiles those same source
//! files without linking Tauri or WebView2, so an AI agent (or a build
//! machine) can capture, send and read logs with no desktop dependency.

#[path = "../cli/mod.rs"]
mod cli;
#[allow(dead_code)]
#[path = "../logging.rs"]
mod logging;
#[allow(dead_code)]
#[path = "../serial.rs"]
mod serial;
#[allow(dead_code)]
#[path = "../settings.rs"]
mod settings;

fn main() {
    std::process::exit(cli::run());
}
