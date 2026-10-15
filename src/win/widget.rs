//! The widget window: a layered (per-pixel alpha) tool window that sits below other windows,
//! never takes focus, and is moved by dragging. A click refreshes; a right-click opens the
//! menu. The tray icon offers the same menu.

use std::cell::RefCell;
use std::mem::size_of;
use std::sync::mpsc::{self, Receiver};

use windows::Win32::Devices::HumanInterfaceDevice::HidD_GetHidGuid;
use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromPoint};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DBT_DEVICEARRIVAL, DBT_DEVICEREMOVECOMPLETE, DBT_DEVTYP_DEVICEINTERFACE,
    DEV_BROADCAST_DEVICEINTERFACE_W, DEVICE_NOTIFY_WINDOW_HANDLE, DefWindowProcW, DestroyWindow,
    DispatchMessageW, FindWindowW, GWLP_USERDATA, GetCursorPos, GetMessageW, GetWindowLongPtrW,
    GetWindowRect, HWND_BOTTOM, HWND_NOTOPMOST, HWND_TOPMOST, IDC_ARROW, KillTimer, LoadCursorW,
    MSG, PBT_APMRESUMEAUTOMATIC, PostMessageW, PostQuitMessage, RegisterClassExW,
    RegisterDeviceNotificationW, RegisterWindowMessageW, SPI_GETWORKAREA, SW_HIDE,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetCursor, SetTimer, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, SystemParametersInfoW, TranslateMessage, WINDOWPOS, WM_APP, WM_CAPTURECHANGED,
    WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY, WM_DEVICECHANGE, WM_DISPLAYCHANGE, WM_DPICHANGED,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCDESTROY, WM_POWERBROADCAST, WM_RBUTTONUP,
    WM_SETTINGCHANGE, WM_TIMER, WM_WINDOWPOSCHANGING, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use super::layered::Painter;
use super::menu::{self, MenuCommand};
use super::monitor::{Command, Monitor, now_seconds};
use super::render::{Renderer, SHADOW};
use super::rows::row_models;
use super::settings::{self, Settings};
use super::tray::Tray;
use super::{diagnostics, dialog};
use crate::common::alerts::LowBatteryPolicy;
use crate::common::model::DeviceReading;
use crate::common::store::display_order;
use crate::common::strings::Strings;

const CLASS_NAME: PCWSTR = w!("DeviceBatteryWidget");
const WM_APP_ROWS: u32 = WM_APP + 1;
const WM_APP_TRAY: u32 = WM_APP + 2;
const WM_APP_SHOW: u32 = WM_APP + 3;
const TIMER_DEVICES: usize = 1;
const TIMER_WAKE: usize = 2;
const DEVICE_SETTLE_MS: u32 = 1500;
const WAKE_DELAY_MS: u32 = 5000;
const DRAG_THRESHOLD: i32 = 4;
const SCREEN_MARGIN: f32 = 16.0;
/// Tray events in NOTIFYICON_VERSION_4 mode (shellapi.h).
const NIN_SELECT: u32 = 0x400;
const NIN_KEYSELECT: u32 = 0x401;
/// Sent after TrackMouseEvent(TME_LEAVE) when the pointer leaves the window (winuser.h).
const WM_MOUSELEAVE: u32 = 0x02A3;

struct Drag {
    cursor: POINT,
    origin: POINT,
    moved: bool,
}

struct App {
    window: HWND,
    strings: Strings,
    settings: Settings,
    painter: Painter,
    /// Latest rows from the monitor, before names and hiding are applied.
    reported: Vec<DeviceReading>,
    /// What is shown.
    rows: Vec<DeviceReading>,
    inbox: Receiver<Vec<DeviceReading>>,
    monitor: Option<Monitor>,
    tray: Tray,
    alerts: LowBatteryPolicy,
    hovered: Option<usize>,
    tracking_leave: bool,
    drag: Option<Drag>,
    shown_once: bool,
    dark: bool,
    taskbar_created: u32,
}

/// Runs the widget until the user quits; returns the process exit code.
pub fn run(strings: Strings) -> i32 {
    // SAFETY: the mutex name is a static string; the handle lives until the process exits.
    let _instance = unsafe { CreateMutexW(None, true, w!("Local\\DeviceBattery.SingleInstance")) };
    // SAFETY: plain call.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        // Already running: bring that widget forward instead.
        // SAFETY: posting to another process's window is allowed.
        unsafe {
            if let Ok(existing) = FindWindowW(CLASS_NAME, PCWSTR::null()) {
                let _ = PostMessageW(Some(existing), WM_APP_SHOW, WPARAM(0), LPARAM(0));
            }
        }
        return 0;
    }
    let Ok(renderer) = Renderer::new() else {
        return 1;
    };
    let Some(window) = create_window() else {
        return 1;
    };
    let (sender, inbox) = mpsc::channel();
    let app = App {
        window,
        strings,
        settings: Settings::load(),
        painter: Painter::new(renderer),
        reported: Vec::new(),
        rows: Vec::new(),
        inbox,
        monitor: None,
        tray: Tray::new(window, WM_APP_TRAY),
        alerts: LowBatteryPolicy::default(),
        hovered: None,
        tracking_leave: false,
        drag: None,
        shown_once: false,
        dark: settings::prefers_dark_theme(),
        // SAFETY: registering a well-known message name.
        taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
    };
    let cell = Box::new(RefCell::new(app));
    // SAFETY: the box is reclaimed in WM_NCDESTROY, the last message the window receives.
    unsafe {
        SetWindowLongPtrW(window, GWLP_USERDATA, Box::into_raw(cell) as isize);
    }
    with_app(window, |app| {
        app.tray.add();
        app.place_initially();
        app.apply_z_order();
        app.monitor = Some(Monitor::start(window, WM_APP_ROWS, strings, sender));
    });
    register_device_notifications(window);

    let mut message = MSG::default();
    // SAFETY: standard message loop on the thread that owns the window.
    unsafe {
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    0
}

fn create_window() -> Option<HWND> {
    // SAFETY: the class name is static; the procedure has the system calling convention.
    unsafe {
        let module = GetModuleHandleW(PCWSTR::null()).ok()?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(procedure),
            hInstance: module.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        RegisterClassExW(&class);
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            CLASS_NAME,
            w!("Device Battery"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(module.into()),
            None,
        )
        .ok()
    }
}

fn register_device_notifications(window: HWND) {
    // SAFETY: the filter struct is fully initialized and only read during the call.
    unsafe {
        let filter = DEV_BROADCAST_DEVICEINTERFACE_W {
            dbcc_size: size_of::<DEV_BROADCAST_DEVICEINTERFACE_W>() as u32,
            dbcc_devicetype: DBT_DEVTYP_DEVICEINTERFACE.0,
            dbcc_classguid: HidD_GetHidGuid(),
            ..Default::default()
        };
        let _ = RegisterDeviceNotificationW(
            HANDLE(window.0),
            (&raw const filter).cast(),
            DEVICE_NOTIFY_WINDOW_HANDLE,
        );
    }
}

/// Runs `body` with the app state, unless it is already borrowed (a nested message during a
/// menu or dialog); returns whether it ran.
fn with_app(window: HWND, body: impl FnOnce(&mut App)) -> bool {
    // SAFETY: the pointer is 0 or the RefCell boxed in `run`, alive until WM_NCDESTROY.
    let cell =
        unsafe { (GetWindowLongPtrW(window, GWLP_USERDATA) as *const RefCell<App>).as_ref() };
    match cell.map(RefCell::try_borrow_mut) {
        Some(Ok(mut app)) => {
            body(&mut app);
            true
        }
        _ => false,
    }
}

extern "system" fn procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCDESTROY {
        // SAFETY: reclaims the box leaked in `run`; nothing touches it afterwards.
        unsafe {
            let pointer = SetWindowLongPtrW(window, GWLP_USERDATA, 0) as *mut RefCell<App>;
            if !pointer.is_null() {
                drop(Box::from_raw(pointer));
            }
            PostQuitMessage(0);
        }
    }
    match handle(window, message, wparam, lparam) {
        Some(result) => result,
        // SAFETY: default processing for everything we do not handle.
        None => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

fn handle(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    let done = Some(LRESULT(0));
    match message {
        WM_APP_ROWS => {
            if !with_app(window, App::take_rows) {
                // Busy with a menu or dialog: try again once it is closed.
                // SAFETY: posting to our own window.
                unsafe {
                    let _ = PostMessageW(Some(window), WM_APP_ROWS, WPARAM(0), LPARAM(0));
                }
            }
            done
        }
        WM_APP_SHOW => {
            with_app(window, |app| app.set_widget_visible(true));
            done
        }
        WM_APP_TRAY => {
            match (lparam.0 & 0xFFFF) as u32 {
                NIN_SELECT | NIN_KEYSELECT => {
                    with_app(window, |app| {
                        let visible = !app.settings.widget_visible;
                        app.set_widget_visible(visible);
                    });
                }
                WM_CONTEXTMENU => show_menu(window, None),
                _ => {}
            }
            done
        }
        WM_LBUTTONDOWN => {
            with_app(window, App::begin_drag);
            done
        }
        WM_MOUSEMOVE => {
            with_app(window, |app| app.mouse_moved(lparam));
            done
        }
        WM_LBUTTONUP => {
            with_app(window, App::end_drag);
            done
        }
        WM_CAPTURECHANGED => {
            with_app(window, |app| app.drag = None);
            done
        }
        WM_MOUSELEAVE => {
            with_app(window, |app| {
                app.tracking_leave = false;
                if app.hovered.take().is_some() {
                    app.redraw();
                }
            });
            done
        }
        WM_RBUTTONUP => {
            let mut row = None;
            with_app(window, |app| row = app.row_at(lparam));
            show_menu(window, row);
            done
        }
        WM_WINDOWPOSCHANGING => {
            with_app(window, |app| {
                if !app.settings.keep_on_top {
                    // SAFETY: for this message lParam points at a WINDOWPOS we may edit.
                    if let Some(position) = unsafe { (lparam.0 as *mut WINDOWPOS).as_mut() } {
                        position.hwndInsertAfter = HWND_BOTTOM;
                        position.flags &= !SWP_NOZORDER;
                    }
                }
            });
            done
        }
        WM_DEVICECHANGE => {
            if matches!(
                wparam.0 as u32,
                DBT_DEVICEARRIVAL | DBT_DEVICEREMOVECOMPLETE
            ) {
                // SAFETY: a timer on our own window; re-arming it debounces bursts of arrivals.
                unsafe {
                    SetTimer(Some(window), TIMER_DEVICES, DEVICE_SETTLE_MS, None);
                }
            }
            Some(LRESULT(1))
        }
        WM_POWERBROADCAST => {
            if wparam.0 as u32 == PBT_APMRESUMEAUTOMATIC {
                // SAFETY: as above.
                unsafe {
                    SetTimer(Some(window), TIMER_WAKE, WAKE_DELAY_MS, None);
                }
            }
            Some(LRESULT(1))
        }
        WM_TIMER => {
            let command = match wparam.0 {
                TIMER_DEVICES => Command::DevicesChanged,
                _ => Command::Refresh,
            };
            // SAFETY: the timer was created on this window.
            unsafe {
                let _ = KillTimer(Some(window), wparam.0);
            }
            with_app(window, |app| app.send(command));
            done
        }
        WM_SETTINGCHANGE => {
            with_app(window, |app| {
                let dark = settings::prefers_dark_theme();
                if dark != app.dark {
                    app.dark = dark;
                    app.redraw();
                }
            });
            done
        }
        WM_DPICHANGED | WM_DISPLAYCHANGE => {
            with_app(window, |app| {
                app.keep_on_screen();
                app.redraw();
            });
            done
        }
        WM_CLOSE => {
            // SAFETY: destroying our own window.
            unsafe {
                let _ = DestroyWindow(window);
            }
            done
        }
        WM_DESTROY => {
            with_app(window, App::shut_down);
            done
        }
        other => {
            let mut handled = false;
            with_app(window, |app| {
                if other == app.taskbar_created {
                    app.tray.add();
                    handled = true;
                }
            });
            handled.then_some(LRESULT(0))
        }
    }
}

/// Shows the menu, then runs the chosen command. Menus and the rename dialog pump
/// messages, so no app borrow is held while they are open.
fn show_menu(window: HWND, row: Option<usize>) {
    let mut items = Vec::new();
    let mut target: Option<DeviceReading> = None;
    with_app(window, |app| {
        target = row.and_then(|index| app.rows.get(index).cloned());
        items = menu::items(&app.strings, &app.settings, target.as_ref());
    });
    let Some(command) = menu::track(window, &items) else {
        return;
    };
    let mut rename: Option<(DeviceReading, Strings)> = None;
    with_app(window, |app| match command {
        MenuCommand::Rename => rename = target.clone().map(|reading| (reading, app.strings)),
        other => app.execute(other, target.as_ref()),
    });
    if let Some((reading, strings)) = rename
        && let Some(name) = dialog::ask_name(window, &strings, &reading.name)
    {
        with_app(window, |app| {
            app.settings.set_custom_name(&reading.id, Some(name));
            app.settings_changed();
        });
    }
}

impl App {
    fn send(&self, command: Command) {
        if let Some(monitor) = &self.monitor {
            monitor.send(command);
        }
    }

    fn take_rows(&mut self) {
        while let Ok(rows) = self.inbox.try_recv() {
            self.reported = rows;
        }
        self.rebuild_rows();
        if self.settings.low_battery_alerts {
            for alert in self.alerts.evaluate(&self.rows) {
                self.tray.notify(
                    &self.strings.low_battery_title(&alert.name),
                    &self.strings.low_battery_body(alert.level),
                );
            }
        }
        let summary: Vec<String> = self
            .rows
            .iter()
            .map(|row| match row.level {
                Some(level) => format!("{} {level}%", row.name),
                None => format!("{} —", row.name),
            })
            .collect();
        let tooltip = if summary.is_empty() {
            "Device Battery".to_owned()
        } else {
            summary.join("\n")
        };
        self.tray.set_tooltip(&tooltip);
        self.redraw();
        if !self.shown_once {
            self.shown_once = true;
            self.set_widget_visible(self.settings.widget_visible);
        }
    }

    /// Applies custom names and hidden devices to what the monitor reported.
    fn rebuild_rows(&mut self) {
        let mut rows: Vec<DeviceReading> = self
            .reported
            .iter()
            .filter(|row| !self.settings.hidden.contains(&row.id))
            .map(|row| match self.settings.custom_names.get(&row.id) {
                Some(name) => row.renamed(name),
                None => row.clone(),
            })
            .collect();
        rows.sort_by(display_order);
        self.rows = rows;
        self.hovered = self.hovered.filter(|index| *index < self.rows.len());
    }

    fn execute(&mut self, command: MenuCommand, target: Option<&DeviceReading>) {
        match command {
            MenuCommand::Refresh => self.send(Command::Refresh),
            MenuCommand::Hide => {
                if let Some(reading) = target {
                    self.settings.hidden.insert(reading.id.clone());
                }
                self.settings_changed();
            }
            MenuCommand::ShowHidden => {
                self.settings.hidden.clear();
                self.settings_changed();
            }
            MenuCommand::ToggleWidget => {
                let visible = !self.settings.widget_visible;
                self.set_widget_visible(visible);
            }
            MenuCommand::ToggleOnTop => {
                self.settings.keep_on_top = !self.settings.keep_on_top;
                self.settings.save();
                self.apply_z_order();
            }
            MenuCommand::ToggleAlerts => {
                self.settings.low_battery_alerts = !self.settings.low_battery_alerts;
                self.settings.save();
            }
            MenuCommand::ToggleStartup => {
                let _ = settings::set_starts_with_windows(!settings::starts_with_windows());
            }
            MenuCommand::DeviceReport => {
                let _ = diagnostics::open_report(self.strings);
            }
            MenuCommand::Quit => {
                // SAFETY: destroying our own window ends the message loop.
                unsafe {
                    let _ = DestroyWindow(self.window);
                }
            }
            MenuCommand::Rename => {}
        }
    }

    fn settings_changed(&mut self) {
        self.settings.save();
        self.rebuild_rows();
        self.redraw();
    }

    fn set_widget_visible(&mut self, visible: bool) {
        self.settings.widget_visible = visible;
        self.settings.save();
        // SAFETY: showing or hiding our own window.
        unsafe {
            let _ = ShowWindow(
                self.window,
                if visible { SW_SHOWNOACTIVATE } else { SW_HIDE },
            );
        }
        if visible {
            self.apply_z_order();
        }
    }

    /// Floats above other windows, or stays at the bottom of the z-order (the default).
    fn apply_z_order(&self) {
        let after = if self.settings.keep_on_top {
            HWND_TOPMOST
        } else {
            HWND_NOTOPMOST
        };
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
        // SAFETY: repositioning our own window in the z-order only.
        unsafe {
            let _ = SetWindowPos(self.window, Some(after), 0, 0, 0, 0, flags);
            if !self.settings.keep_on_top {
                let _ = SetWindowPos(self.window, Some(HWND_BOTTOM), 0, 0, 0, 0, flags);
            }
        }
    }

    fn scale(&self) -> f32 {
        // SAFETY: plain query on our own window.
        let dpi = unsafe { GetDpiForWindow(self.window) };
        (if dpi == 0 { 96 } else { dpi }) as f32 / 96.0
    }

    fn window_origin(&self) -> POINT {
        let mut rect = RECT::default();
        // SAFETY: `rect` is a live out-parameter.
        let _ = unsafe { GetWindowRect(self.window, &mut rect) };
        POINT {
            x: rect.left,
            y: rect.top,
        }
    }

    fn move_to(&self, origin: POINT) {
        let flags = SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE;
        // SAFETY: moving our own window.
        unsafe {
            let _ = SetWindowPos(self.window, None, origin.x, origin.y, 0, 0, flags);
        }
    }

    /// The saved spot, or the top-right corner of the primary work area.
    fn place_initially(&mut self) {
        let saved = self.settings.position.map(|(x, y)| POINT { x, y });
        match saved.filter(|point| on_screen(*point)) {
            Some(point) => self.move_to(point),
            None => self.move_to(default_origin()),
        }
    }

    fn keep_on_screen(&mut self) {
        if !on_screen(self.window_origin()) {
            self.move_to(default_origin());
        }
    }

    fn row_at(&self, lparam: LPARAM) -> Option<usize> {
        let (x, y) = client_point(lparam);
        let scale = self.scale();
        Renderer::row_at(x as f32 / scale, y as f32 / scale, self.rows.len())
    }

    fn begin_drag(&mut self) {
        let mut cursor = POINT::default();
        // SAFETY: capture keeps mouse messages coming while the button is held.
        unsafe {
            let _ = GetCursorPos(&mut cursor);
            SetCapture(self.window);
        }
        self.drag = Some(Drag {
            cursor,
            origin: self.window_origin(),
            moved: false,
        });
    }

    fn mouse_moved(&mut self, lparam: LPARAM) {
        // SAFETY: the class cursor is not used for layered windows; set it explicitly.
        unsafe {
            if let Ok(arrow) = LoadCursorW(None, IDC_ARROW) {
                SetCursor(Some(arrow));
            }
        }
        if !self.tracking_leave {
            let mut track = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.window,
                dwHoverTime: 0,
            };
            // SAFETY: `track` is fully initialized.
            self.tracking_leave = unsafe { TrackMouseEvent(&mut track) }.is_ok();
        }
        if let Some(drag) = &mut self.drag {
            let mut cursor = POINT::default();
            // SAFETY: `cursor` is a live out-parameter.
            let _ = unsafe { GetCursorPos(&mut cursor) };
            let (dx, dy) = (cursor.x - drag.cursor.x, cursor.y - drag.cursor.y);
            if drag.moved || dx.abs() > DRAG_THRESHOLD || dy.abs() > DRAG_THRESHOLD {
                drag.moved = true;
                let origin = POINT {
                    x: drag.origin.x + dx,
                    y: drag.origin.y + dy,
                };
                self.move_to(origin);
            }
            return;
        }
        let hovered = self.row_at(lparam);
        if hovered != self.hovered {
            self.hovered = hovered;
            self.redraw();
        }
    }

    fn end_drag(&mut self) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        // SAFETY: releases the capture taken in `begin_drag`.
        let _ = unsafe { ReleaseCapture() };
        if drag.moved {
            let origin = self.window_origin();
            self.settings.position = Some((origin.x, origin.y));
            self.settings.save();
        } else {
            self.send(Command::Refresh);
        }
    }

    fn redraw(&mut self) {
        let rows = row_models(&self.rows, now_seconds(), &self.strings, self.hovered);
        let origin = self.window_origin();
        let scale = self.scale();
        let empty = self.strings.no_devices();
        self.painter
            .paint(self.window, origin, scale, &rows, &empty, self.dark);
    }

    fn shut_down(&mut self) {
        self.tray.remove();
        if let Some(monitor) = self.monitor.take() {
            monitor.stop();
        }
        self.settings.save();
    }
}

/// The top-right corner of the primary work area, leaving a small margin.
fn default_origin() -> POINT {
    let mut work = RECT::default();
    // SAFETY: `work` is a live out-parameter of the expected type.
    let _ = unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some((&raw mut work).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    // SAFETY: plain query.
    let scale = unsafe { GetDpiForSystem() } as f32 / 96.0;
    let (width, _) = Renderer::size(1);
    let margin = SCREEN_MARGIN - SHADOW;
    POINT {
        x: work.right - ((width + margin) * scale).round() as i32,
        y: work.top + (margin * scale).round() as i32,
    }
}

fn on_screen(point: POINT) -> bool {
    // SAFETY: plain query.
    !unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONULL) }.is_invalid()
}

/// Client coordinates packed in a mouse message's lParam.
fn client_point(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}
