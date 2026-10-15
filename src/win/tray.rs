//! The notification-area icon: a menu on right-click, the widget on left-click, a tooltip
//! listing every device, and balloon notifications for low batteries.

use std::mem::size_of;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_WARNING, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NIM_SETVERSION, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, HICON, IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTCOLOR, LoadIconW, LoadImageW,
    SM_CXSMICON, SM_CYSMICON,
};
use windows::core::PCWSTR;

use super::text::copy_into;

pub struct Tray {
    data: NOTIFYICONDATAW,
}

impl Tray {
    pub fn new(window: HWND, callback_message: u32) -> Self {
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: window,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
            uCallbackMessage: callback_message,
            hIcon: app_icon(),
            ..Default::default()
        };
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        copy_into(&mut data.szTip, "Device Battery");
        Self { data }
    }

    /// Adds the icon (again after Explorer restarts).
    pub fn add(&self) {
        // SAFETY: `data` is a fully initialized NOTIFYICONDATAW.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_ADD, &self.data);
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &self.data);
        }
    }

    pub fn set_tooltip(&mut self, text: &str) {
        copy_into(&mut self.data.szTip, text);
        // SAFETY: as above.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &self.data);
        }
    }

    /// Shows a notification balloon (a toast on Windows 10 and 11).
    pub fn notify(&self, title: &str, body: &str) {
        let mut data = self.data;
        data.uFlags |= NIF_INFO;
        data.dwInfoFlags = NIIF_WARNING;
        copy_into(&mut data.szInfoTitle, title);
        copy_into(&mut data.szInfo, body);
        // SAFETY: as above.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }

    pub fn remove(&self) {
        // SAFETY: as above.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.data);
        }
    }
}

/// The icon embedded in the executable (resource 1), at the small-icon size.
fn app_icon() -> HICON {
    // SAFETY: loading a resource by ordinal from our own module; falls back to the stock icon.
    unsafe {
        let module = GetModuleHandleW(PCWSTR::null()).ok();
        let loaded = LoadImageW(
            module.map(Into::into),
            // MAKEINTRESOURCE(1): an integer resource ID passed where a string pointer goes.
            PCWSTR(std::ptr::without_provenance(1)),
            IMAGE_ICON,
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
            LR_DEFAULTCOLOR,
        );
        match loaded {
            Ok(handle) => HICON(handle.0),
            Err(_) => LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
        }
    }
}
