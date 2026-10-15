#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    device_battery::win::run();

    #[cfg(not(windows))]
    {
        eprintln!(
            "Device Battery for Windows only runs on Windows; the macOS app is in the repository root."
        );
        std::process::exit(1);
    }
}
