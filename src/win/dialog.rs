//! A small modal "Rename device" window: a hint, a text field, OK and Cancel.

use std::mem::size_of;
use std::sync::Once;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    COLOR_WINDOW, CreateFontIndirectW, DeleteObject, HBRUSH, HFONT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForSystem, GetDpiForWindow, SystemParametersInfoForDpi,
};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, ES_AUTOHSCROLL, GWLP_USERDATA, GetMessageW, GetWindowLongPtrW,
    GetWindowTextW, HMENU, IDCANCEL, IDOK, IsDialogMessageW, MSG, NONCLIENTMETRICSW,
    PostQuitMessage, RegisterClassExW, SPI_GETNONCLIENTMETRICS, SPI_GETWORKAREA, SW_SHOW,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SendMessageW, SetForegroundWindow, SetWindowLongPtrW,
    ShowWindow, SystemParametersInfoW, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE,
    WM_COMMAND, WM_SETFONT, WNDCLASSEXW, WS_CAPTION, WS_CHILD, WS_EX_CLIENTEDGE,
    WS_EX_DLGMODALFRAME, WS_EX_TOPMOST, WS_POPUP, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};
use windows::core::{PCWSTR, w};

use super::text::{from_wide, wide};
use crate::common::strings::Strings;

const CLASS_NAME: PCWSTR = w!("DeviceBatteryRename");
/// Edit-control message that selects a range of text (winuser.h).
const EM_SETSEL: u32 = 0x00B1;
/// Client area in DIPs.
const WIDTH: i32 = 340;
const HEIGHT: i32 = 140;

struct State {
    edit: HWND,
    /// `None` while open; then `Some(None)` for Cancel or `Some(Some(text))` for OK.
    result: Option<Option<String>>,
}

/// Asks for a new name; `None` when cancelled. An empty answer means "default name".
pub fn ask_name(owner: HWND, strings: &Strings, current: &str) -> Option<String> {
    register_class();
    // SAFETY: every window created here is destroyed before returning; `state` outlives the
    // modal loop that lets the window procedure reach it.
    unsafe {
        let dpi = match GetDpiForWindow(owner) {
            0 => GetDpiForSystem(),
            dpi => dpi,
        };
        let scale = |value: i32| value * dpi as i32 / 96;
        let style = WS_POPUP | WS_CAPTION | WS_SYSMENU;
        let ex_style = WS_EX_DLGMODALFRAME | WS_EX_TOPMOST;
        let mut frame = RECT {
            left: 0,
            top: 0,
            right: scale(WIDTH),
            bottom: scale(HEIGHT),
        };
        let _ = AdjustWindowRectExForDpi(&mut frame, style, false, ex_style, dpi);
        let (width, height) = (frame.right - frame.left, frame.bottom - frame.top);
        let mut work = RECT::default();
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some((&raw mut work).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        let x = work.left + (work.right - work.left - width) / 2;
        let y = work.top + (work.bottom - work.top - height) / 3;

        let module = GetModuleHandleW(PCWSTR::null()).ok().map(Into::into);
        let title = wide(&strings.rename_title());
        let Ok(window) = CreateWindowExW(
            ex_style,
            CLASS_NAME,
            PCWSTR(title.as_ptr()),
            style,
            x,
            y,
            width,
            height,
            Some(owner),
            None,
            module,
            None,
        ) else {
            return None;
        };

        let font = message_font(dpi);
        let child = |class: PCWSTR,
                     text: &str,
                     style: WINDOW_STYLE,
                     ex: WINDOW_EX_STYLE,
                     rect: (i32, i32, i32, i32),
                     id: i32| {
            let text = wide(text);
            let handle = CreateWindowExW(
                ex,
                class,
                PCWSTR(text.as_ptr()),
                WS_CHILD | WS_VISIBLE | style,
                scale(rect.0),
                scale(rect.1),
                scale(rect.2),
                scale(rect.3),
                Some(window),
                Some(HMENU(id as isize as *mut _)),
                module,
                None,
            )
            .unwrap_or_default();
            if let Some(font) = font {
                SendMessageW(
                    handle,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
            handle
        };
        child(
            w!("STATIC"),
            &strings.rename_hint(),
            WINDOW_STYLE(0),
            WINDOW_EX_STYLE(0),
            (16, 14, 308, 20),
            0,
        );
        let edit = child(
            w!("EDIT"),
            current,
            WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            WS_EX_CLIENTEDGE,
            (16, 42, 308, 26),
            100,
        );
        child(
            w!("BUTTON"),
            &strings.ok(),
            WS_TABSTOP | WINDOW_STYLE(BS_DEFPUSHBUTTON as u32),
            WINDOW_EX_STYLE(0),
            (152, 92, 84, 28),
            IDOK.0,
        );
        child(
            w!("BUTTON"),
            &strings.cancel(),
            WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32),
            WINDOW_EX_STYLE(0),
            (244, 92, 84, 28),
            IDCANCEL.0,
        );

        let mut state = State { edit, result: None };
        SetWindowLongPtrW(window, GWLP_USERDATA, (&raw mut state) as isize);
        let _ = ShowWindow(window, SW_SHOW);
        let _ = SetForegroundWindow(window);
        let _ = SetFocus(Some(edit));
        SendMessageW(edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));

        let mut message = MSG::default();
        while state.result.is_none() {
            let status = GetMessageW(&mut message, None, 0, 0);
            if status.0 <= 0 {
                // WM_QUIT (or an error): leave it for the main loop.
                PostQuitMessage(message.wParam.0 as i32);
                break;
            }
            if !IsDialogMessageW(window, &message).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        SetWindowLongPtrW(window, GWLP_USERDATA, 0);
        let _ = DestroyWindow(window);
        if let Some(font) = font {
            let _ = DeleteObject(font.into());
        }
        state.result.flatten()
    }
}

fn register_class() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        // SAFETY: the class name is static and the procedure has the system calling convention.
        unsafe {
            let module = GetModuleHandleW(PCWSTR::null()).ok();
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(procedure),
                hInstance: module.map(Into::into).unwrap_or_default(),
                hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as isize as *mut _),
                lpszClassName: CLASS_NAME,
                ..Default::default()
            };
            RegisterClassExW(&class);
        }
    });
}

/// The system message font at `dpi`, so the dialog matches other Windows dialogs.
fn message_font(dpi: u32) -> Option<HFONT> {
    let mut metrics = NONCLIENTMETRICSW {
        cbSize: size_of::<NONCLIENTMETRICSW>() as u32,
        ..Default::default()
    };
    // SAFETY: `metrics` is sized via cbSize as the API requires.
    unsafe {
        SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS.0,
            metrics.cbSize,
            Some((&raw mut metrics).cast()),
            0,
            dpi,
        )
        .ok()?;
        let font = CreateFontIndirectW(&metrics.lfMessageFont);
        (!font.is_invalid()).then_some(font)
    }
}

extern "system" fn procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: the pointer is either 0 or points at the `State` living in `ask_name`'s frame
    // for as long as the window can receive messages.
    unsafe {
        let state = GetWindowLongPtrW(window, GWLP_USERDATA) as *mut State;
        if let Some(state) = state.as_mut() {
            match message {
                WM_COMMAND => {
                    let id = (wparam.0 & 0xFFFF) as i32;
                    if id == IDOK.0 {
                        let mut buffer = [0u16; 256];
                        let length = GetWindowTextW(state.edit, &mut buffer).max(0) as usize;
                        state.result = Some(Some(from_wide(&buffer[..length.min(buffer.len())])));
                        return LRESULT(0);
                    }
                    if id == IDCANCEL.0 {
                        state.result = Some(None);
                        return LRESULT(0);
                    }
                }
                WM_CLOSE => {
                    state.result = Some(None);
                    return LRESULT(0);
                }
                _ => {}
            }
        }
        DefWindowProcW(window, message, wparam, lparam)
    }
}
