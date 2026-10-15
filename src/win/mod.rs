//! Windows integration: HID access, Bluetooth batteries, the widget window and the tray icon.

mod bluetooth;
mod diagnostics;
mod dialog;
mod hid;
mod layered;
mod menu;
mod monitor;
mod preview;
mod receivers;
mod render;
mod rows;
mod settings;
mod text;
mod tray;
mod widget;

use std::io::Write as _;

use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

use crate::common::strings::{AppLanguage, Strings};

/// Entry point. Besides the widget, two command-line modes exist:
/// `--render-preview <png> [--language en|ko|zh-Hans]` and `--list-devices [file]`.
pub fn run() {
    install_crash_log();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // SAFETY: called once, first thing on the main thread.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        // The manifest already asks for this; it matters when the manifest is missing.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let language = value_after(&args, "--language")
        .and_then(|code| AppLanguage::from_code(&code))
        .unwrap_or_else(|| AppLanguage::resolve(&text::user_languages()));
    let strings = Strings::new(language);

    let code = if let Some(path) = value_after(&args, "--render-preview") {
        i32::from(preview::render(&path, &strings).is_err())
    } else if args.iter().any(|arg| arg == "--list-devices") {
        match value_after(&args, "--list-devices") {
            Some(path) => diagnostics::write(&path, strings),
            None => diagnostics::open_report(strings),
        }
    } else {
        widget::run(strings)
    };
    std::process::exit(code);
}

/// The argument following `flag`, unless it is another flag.
fn value_after(args: &[String], flag: &str) -> Option<String> {
    let index = args.iter().position(|arg| arg == flag)?;
    args.get(index + 1)
        .filter(|value| !value.starts_with("--"))
        .cloned()
}

/// A GUI app has no console: record panics in %APPDATA%\DeviceBattery\crash.log instead.
fn install_crash_log() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let dir = settings::data_dir();
        if std::fs::create_dir_all(&dir).is_ok()
            && let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("crash.log"))
        {
            let _ = writeln!(file, "[{}] {info}", monitor::now_seconds());
        }
        previous(info);
    }));
}
