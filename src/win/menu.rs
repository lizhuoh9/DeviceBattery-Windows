//! The context menu shared by the widget and the tray icon.

use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, MF_CHECKED, MF_SEPARATOR, MF_STRING,
    PostMessageW, SetForegroundWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_NULL,
};
use windows::core::PCWSTR;

use super::settings::{self, Settings};
use super::text::wide;
use crate::common::model::DeviceReading;
use crate::common::strings::Strings;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCommand {
    Refresh = 1,
    Rename,
    Hide,
    ShowHidden,
    ToggleWidget,
    ToggleOnTop,
    ToggleAlerts,
    ToggleStartup,
    DeviceReport,
    Quit,
}

impl MenuCommand {
    const ALL: [MenuCommand; 10] = [
        MenuCommand::Refresh,
        MenuCommand::Rename,
        MenuCommand::Hide,
        MenuCommand::ShowHidden,
        MenuCommand::ToggleWidget,
        MenuCommand::ToggleOnTop,
        MenuCommand::ToggleAlerts,
        MenuCommand::ToggleStartup,
        MenuCommand::DeviceReport,
        MenuCommand::Quit,
    ];

    fn from_id(id: i32) -> Option<Self> {
        Self::ALL.into_iter().find(|command| *command as i32 == id)
    }
}

pub enum MenuItem {
    Command(MenuCommand, String, bool),
    Separator,
}

/// The menu for the widget (with `target`, the row under the pointer) or the tray icon.
pub fn items(
    strings: &Strings,
    settings: &Settings,
    target: Option<&DeviceReading>,
) -> Vec<MenuItem> {
    let command = |command, label, checked| MenuItem::Command(command, label, checked);
    let mut items = vec![command(MenuCommand::Refresh, strings.refresh_now(), false)];
    if let Some(reading) = target {
        items.push(MenuItem::Separator);
        items.push(command(
            MenuCommand::Rename,
            strings.rename(&reading.name),
            false,
        ));
        items.push(command(
            MenuCommand::Hide,
            strings.hide(&reading.name),
            false,
        ));
    }
    if !settings.hidden.is_empty() {
        items.push(command(
            MenuCommand::ShowHidden,
            strings.show_hidden(settings.hidden.len()),
            false,
        ));
    }
    items.push(MenuItem::Separator);
    items.push(command(
        MenuCommand::ToggleWidget,
        strings.show_widget(),
        settings.widget_visible,
    ));
    items.push(command(
        MenuCommand::ToggleOnTop,
        strings.keep_on_top(),
        settings.keep_on_top,
    ));
    items.push(command(
        MenuCommand::ToggleAlerts,
        strings.low_battery_alerts(),
        settings.low_battery_alerts,
    ));
    items.push(command(
        MenuCommand::ToggleStartup,
        strings.start_with_windows(),
        settings::starts_with_windows(),
    ));
    items.push(MenuItem::Separator);
    items.push(command(
        MenuCommand::DeviceReport,
        strings.device_report(),
        false,
    ));
    items.push(command(MenuCommand::Quit, strings.quit(), false));
    items
}

/// Shows the menu at the pointer and returns the chosen command. Pumps messages while open.
pub fn track(window: HWND, items: &[MenuItem]) -> Option<MenuCommand> {
    // SAFETY: the menu is destroyed before returning; label strings outlive AppendMenuW.
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        for item in items {
            match item {
                MenuItem::Separator => {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                }
                MenuItem::Command(command, label, checked) => {
                    let label = wide(label);
                    let flags = if *checked {
                        MF_STRING | MF_CHECKED
                    } else {
                        MF_STRING
                    };
                    let _ = AppendMenuW(menu, flags, *command as usize, PCWSTR(label.as_ptr()));
                }
            }
        }
        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        // Required so the menu closes when the user clicks elsewhere.
        let _ = SetForegroundWindow(window);
        let chosen = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            cursor.x,
            cursor.y,
            None,
            window,
            None,
        );
        let _ = PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        MenuCommand::from_id(chosen.0)
    }
}
