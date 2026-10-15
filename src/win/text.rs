//! UTF-16 helpers and the user's languages.

use windows::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
use windows::core::PWSTR;

/// `text` as a NUL-terminated UTF-16 string.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `text` as UTF-16 without a terminator, for APIs that take a length.
pub fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// A UTF-16 buffer up to its first NUL.
pub fn from_wide(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// Copies `text` into a fixed-size UTF-16 field, truncated and NUL-terminated.
pub fn copy_into(field: &mut [u16], text: &str) {
    let units: Vec<u16> = text
        .encode_utf16()
        .take(field.len().saturating_sub(1))
        .collect();
    field[..units.len()].copy_from_slice(&units);
    field[units.len()] = 0;
}

/// The user's display languages, most preferred first (e.g. `ko-KR`, `en-US`).
pub fn user_languages() -> Vec<String> {
    let mut count = 0u32;
    let mut length = 0u32;
    // SAFETY: the first call only reports the buffer length; the second fills a buffer of
    // exactly that length.
    unsafe {
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, None, &mut length).is_err() {
            return Vec::new();
        }
        let mut buffer = vec![0u16; length as usize];
        if GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut count,
            Some(PWSTR(buffer.as_mut_ptr())),
            &mut length,
        )
        .is_err()
        {
            return Vec::new();
        }
        buffer
            .split(|unit| *unit == 0)
            .filter(|part| !part.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    }
}
