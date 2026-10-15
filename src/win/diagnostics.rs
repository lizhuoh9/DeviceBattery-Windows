//! `DeviceBattery.exe --list-devices [file]` writes what the widget sees: every HID
//! collection, the receivers it recognizes, what they report, and Bluetooth battery nodes.
//! Without a file it opens the report in Notepad, so users can attach it to an issue.

use std::fmt::Write as _;

use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, w};

use super::monitor::now_seconds;
use super::text::wide;
use super::{bluetooth, hid, receivers};
use crate::common::model::DeviceReading;
use crate::common::strings::Strings;

/// Writes the report to `path`; returns the process exit code.
pub fn write(path: &str, strings: Strings) -> i32 {
    if std::fs::write(path, build(strings)).is_ok() {
        0
    } else {
        1
    }
}

/// Writes the report and opens it in the default text editor.
pub fn open_report(strings: Strings) -> i32 {
    let path = std::env::temp_dir()
        .join("DeviceBattery-devices.txt")
        .to_string_lossy()
        .into_owned();
    if write(&path, strings) != 0 {
        return 1;
    }
    let file = wide(&path);
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
    0
}

fn build(strings: Strings) -> String {
    let now = now_seconds();
    let collections = hid::enumerate();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Device Battery {} for Windows — device report",
        env!("CARGO_PKG_VERSION")
    );
    let _ = writeln!(out, "\n== HID collections ({}) ==", collections.len());
    for c in &collections {
        let _ = writeln!(
            out,
            "{:04X}:{:04X}  page {:04X} usage {:04X}  in {:>3} out {:>3}  \"{}\"  serial \"{}\"\n    {}",
            c.vendor_id,
            c.product_id,
            c.usage_page,
            c.usage,
            c.input_length,
            c.output_length,
            c.product,
            c.serial,
            c.path
        );
    }

    let mut sessions = receivers::discover(&collections, strings);
    let _ = writeln!(out, "\n== Receivers recognized ({}) ==", sessions.len());
    for session in &mut sessions {
        let _ = writeln!(out, "{}  [{}]", session.description, session.key);
        for reading in session.read(now) {
            let _ = writeln!(out, "    {}", describe(&reading));
        }
    }

    let nodes = bluetooth::nodes();
    let _ = writeln!(out, "\n== Bluetooth ==");
    for reading in bluetooth::read(now) {
        let _ = writeln!(out, "    {}", describe(&reading));
    }
    for node in nodes
        .iter()
        .filter(|node| node.battery.is_some() || node.connected.is_some())
    {
        let _ = writeln!(
            out,
            "node \"{}\" battery {:?} connected {:?}\n    {}",
            node.name, node.battery, node.connected, node.instance_id
        );
    }
    out
}

fn describe(reading: &DeviceReading) -> String {
    format!(
        "{} \"{}\" {:?}: level {:?}, {:?}, {:?} via {}",
        reading.id,
        reading.name,
        reading.kind,
        reading.level,
        reading.charge,
        reading.status,
        reading.source
    )
}
