//! User settings and remembered readings in %APPDATA%\DeviceBattery, and "Start with Windows".

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ, RegCloseKey, RegDeleteValueW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows::core::PCWSTR;

use super::text::{from_wide, wide};
use crate::common::model::DeviceReading;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "DeviceBattery";
const THEME_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub custom_names: HashMap<String, String>,
    pub hidden: HashSet<String>,
    pub keep_on_top: bool,
    pub low_battery_alerts: bool,
    pub widget_visible: bool,
    /// Top-left corner of the widget window, in physical pixels.
    pub position: Option<(i32, i32)>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            custom_names: HashMap::new(),
            hidden: HashSet::new(),
            keep_on_top: false,
            low_battery_alerts: true,
            widget_visible: true,
            position: None,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        read_json("settings.json").unwrap_or_default()
    }

    pub fn save(&self) {
        write_json("settings.json", self);
    }

    pub fn set_custom_name(&mut self, id: &str, name: Option<String>) {
        match name
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
        {
            Some(name) => {
                self.custom_names.insert(id.to_owned(), name);
            }
            None => {
                self.custom_names.remove(id);
            }
        }
    }
}

pub fn load_readings() -> Vec<DeviceReading> {
    read_json("readings.json").unwrap_or_default()
}

pub fn save_readings(readings: &[DeviceReading]) {
    write_json("readings.json", &readings);
}

/// Where crash logs and settings live.
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("LOCALAPPDATA"))
        .unwrap_or_default();
    PathBuf::from(base).join("DeviceBattery")
}

fn read_json<T: for<'de> Deserialize<'de>>(name: &str) -> Option<T> {
    let text = fs::read_to_string(data_dir().join(name)).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_json<T: Serialize>(name: &str, value: &T) {
    let dir = data_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Ok(text) = serde_json::to_string_pretty(value) {
        // Write then rename, so a crash never leaves half a file behind.
        let temporary = dir.join(format!("{name}.tmp"));
        if fs::write(&temporary, text).is_ok() {
            let _ = fs::rename(&temporary, dir.join(name));
        }
    }
}

/// Whether Windows starts the widget at sign-in.
pub fn starts_with_windows() -> bool {
    read_registry_string(RUN_KEY, RUN_VALUE).is_some()
}

pub fn set_starts_with_windows(enabled: bool) -> bool {
    let Some(key) = open_key(RUN_KEY, KEY_WRITE.0) else {
        return false;
    };
    let value = wide(RUN_VALUE);
    // SAFETY: `key` is open, the value name is NUL-terminated, and the data is UTF-16 bytes
    // including the terminator, as REG_SZ requires.
    let result = unsafe {
        if enabled {
            let command = wide(&format!("\"{}\"", executable_path()));
            let bytes: Vec<u8> = command.iter().flat_map(|unit| unit.to_le_bytes()).collect();
            RegSetValueExW(key, PCWSTR(value.as_ptr()), None, REG_SZ, Some(&bytes))
        } else {
            RegDeleteValueW(key, PCWSTR(value.as_ptr()))
        }
    };
    close_key(key);
    result == ERROR_SUCCESS
}

/// Whether apps use the dark theme (Settings → Personalization → Colors).
pub fn prefers_dark_theme() -> bool {
    let Some(key) = open_key(THEME_KEY, KEY_READ.0) else {
        return false;
    };
    let value = wide("AppsUseLightTheme");
    let mut data = 1u32;
    let mut size = size_of_u32();
    // SAFETY: `data` is a 4-byte buffer and `size` says so.
    let result = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR(value.as_ptr()),
            None,
            None,
            Some((&raw mut data).cast::<u8>()),
            Some(&mut size),
        )
    };
    close_key(key);
    result == ERROR_SUCCESS && data == 0
}

fn size_of_u32() -> u32 {
    std::mem::size_of::<u32>() as u32
}

pub fn executable_path() -> String {
    let mut buffer = [0u16; 1024];
    // SAFETY: the buffer is passed as a slice, so its length is known to the call.
    let length = unsafe { GetModuleFileNameW(None, &mut buffer) } as usize;
    from_wide(&buffer[..length.min(buffer.len())])
}

fn read_registry_string(path: &str, name: &str) -> Option<String> {
    let key = open_key(path, KEY_READ.0)?;
    let value = wide(name);
    let mut buffer = [0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: the buffer and its byte size describe the same memory.
    let result = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR(value.as_ptr()),
            None,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    close_key(key);
    (result == ERROR_SUCCESS).then(|| from_wide(&buffer))
}

fn open_key(path: &str, access: u32) -> Option<HKEY> {
    let path = wide(path);
    let mut key = HKEY::default();
    // SAFETY: the path is NUL-terminated and `key` receives the opened handle.
    let result = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(path.as_ptr()),
            None,
            windows::Win32::System::Registry::REG_SAM_FLAGS(access),
            &mut key,
        )
    };
    (result == ERROR_SUCCESS).then_some(key)
}

fn close_key(key: HKEY) {
    // SAFETY: `key` came from RegOpenKeyExW.
    unsafe {
        let _ = RegCloseKey(key);
    }
}
