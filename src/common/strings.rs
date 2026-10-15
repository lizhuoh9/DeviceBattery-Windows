//! All user-facing text, in English, Korean and Simplified Chinese.
//!
//! Every entry lists the three languages side by side, so a translation cannot be left out.

use super::model::DeviceKind;

/// A language the widget's text is available in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppLanguage {
    English,
    Korean,
    SimplifiedChinese,
}

impl AppLanguage {
    pub const ALL: [AppLanguage; 3] = [
        AppLanguage::English,
        AppLanguage::Korean,
        AppLanguage::SimplifiedChinese,
    ];

    /// The first of `preferred` (BCP 47 tags, most preferred first) that the widget speaks;
    /// English otherwise. Traditional Chinese falls back to Simplified Chinese.
    pub fn resolve<S: AsRef<str>>(preferred: &[S]) -> AppLanguage {
        preferred
            .iter()
            .map(|tag| tag.as_ref().to_ascii_lowercase())
            .find_map(|tag| {
                if tag.starts_with("en") {
                    Some(AppLanguage::English)
                } else if tag.starts_with("ko") {
                    Some(AppLanguage::Korean)
                } else if tag.starts_with("zh") {
                    Some(AppLanguage::SimplifiedChinese)
                } else {
                    None
                }
            })
            .unwrap_or(AppLanguage::English)
    }

    /// Parses `en`, `ko` or `zh-Hans` (as used by `--language`).
    pub fn from_code(code: &str) -> Option<AppLanguage> {
        match code {
            "en" => Some(AppLanguage::English),
            "ko" => Some(AppLanguage::Korean),
            "zh-Hans" | "zh" => Some(AppLanguage::SimplifiedChinese),
            _ => None,
        }
    }
}

/// The user-facing text in one language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Strings {
    pub language: AppLanguage,
}

impl Strings {
    pub fn new(language: AppLanguage) -> Self {
        Self { language }
    }

    /// The text for the current language.
    pub fn pick(&self, english: &str, korean: &str, chinese: &str) -> String {
        match self.language {
            AppLanguage::English => english,
            AppLanguage::Korean => korean,
            AppLanguage::SimplifiedChinese => chinese,
        }
        .to_owned()
    }

    // MARK: Status lines

    pub fn charging(&self) -> String {
        self.pick("Charging", "충전 중", "充电中")
    }
    pub fn fully_charged(&self) -> String {
        self.pick("Fully charged", "충전 완료", "已充满")
    }
    pub fn low_battery(&self) -> String {
        self.pick("Low battery", "배터리 부족", "电量偏低")
    }
    pub fn critical_battery(&self) -> String {
        self.pick(
            "Very low — charge soon",
            "배터리 매우 부족 · 충전 필요",
            "电量极低，请尽快充电",
        )
    }
    pub fn asleep_until_wake(&self) -> String {
        self.pick(
            "Asleep — updates when it wakes",
            "절전 중 · 깨우면 업데이트",
            "休眠中，唤醒后更新",
        )
    }
    pub fn asleep_since(&self, age: &str) -> String {
        self.pick(
            &format!("Asleep · {age}"),
            &format!("절전 중 · {age}"),
            &format!("休眠 · {age}"),
        )
    }

    /// Sources are brand names, except Bluetooth.
    pub fn source(&self, raw: &str) -> String {
        if raw == BLUETOOTH_SOURCE {
            self.pick("Bluetooth", "블루투스", "蓝牙")
        } else {
            raw.to_owned()
        }
    }

    /// "just now", "5 min ago", "2 hr ago", "3 days ago".
    pub fn age(&self, date: u64, now: u64) -> String {
        let seconds = now.saturating_sub(date);
        match seconds {
            0..60 => self.pick("just now", "방금", "刚刚"),
            60..3600 => {
                let minutes = seconds / 60;
                self.pick(
                    &format!("{minutes} min ago"),
                    &format!("{minutes}분 전"),
                    &format!("{minutes} 分钟前"),
                )
            }
            3600..86_400 => {
                let hours = seconds / 3600;
                self.pick(
                    &format!("{hours} hr ago"),
                    &format!("{hours}시간 전"),
                    &format!("{hours} 小时前"),
                )
            }
            _ => {
                let days = seconds / 86_400;
                let english = if days == 1 {
                    "1 day ago".to_owned()
                } else {
                    format!("{days} days ago")
                };
                self.pick(&english, &format!("{days}일 전"), &format!("{days} 天前"))
            }
        }
    }

    // MARK: Names for devices that do not report one

    pub fn rog_device(&self, kind: DeviceKind) -> String {
        match kind {
            DeviceKind::Mouse => self.pick("ROG Mouse", "ROG 무선 마우스", "ROG 无线鼠标"),
            DeviceKind::Keyboard => self.pick("ROG Keyboard", "ROG 무선 키보드", "ROG 无线键盘"),
            _ => self.pick("ROG Device", "ROG 무선 기기", "ROG 无线设备"),
        }
    }
    pub fn logitech_device(&self) -> String {
        self.pick("Logitech Device", "Logitech 기기", "Logitech 设备")
    }
    pub fn wireless_mouse(&self) -> String {
        self.pick("Wireless Mouse", "무선 마우스", "无线鼠标")
    }
    pub fn bluetooth_device(&self) -> String {
        self.pick("Bluetooth Device", "블루투스 기기", "蓝牙设备")
    }

    // MARK: Widget, tray and menus

    pub fn no_devices(&self) -> String {
        self.pick(
            "No wireless devices found",
            "무선 기기를 찾지 못했습니다",
            "没有发现无线设备",
        )
    }
    pub fn refresh_now(&self) -> String {
        self.pick("Refresh Now", "지금 새로고침", "立即刷新")
    }
    pub fn rename(&self, name: &str) -> String {
        self.pick(
            &format!("Rename “{name}”…"),
            &format!("“{name}” 이름 변경…"),
            &format!("重命名“{name}”…"),
        )
    }
    pub fn hide(&self, name: &str) -> String {
        self.pick(
            &format!("Hide “{name}”"),
            &format!("“{name}” 숨기기"),
            &format!("隐藏“{name}”"),
        )
    }
    pub fn show_hidden(&self, count: usize) -> String {
        self.pick(
            &format!("Show Hidden Devices ({count})"),
            &format!("숨긴 기기 보기 ({count})"),
            &format!("显示已隐藏的设备（{count}）"),
        )
    }
    pub fn show_widget(&self) -> String {
        self.pick("Show Widget", "위젯 표시", "显示挂件")
    }
    pub fn keep_on_top(&self) -> String {
        self.pick(
            "Keep Above Other Windows",
            "다른 창 위에 유지",
            "置于其他窗口之上",
        )
    }
    pub fn low_battery_alerts(&self) -> String {
        self.pick("Low Battery Alerts", "배터리 부족 알림", "低电量提醒")
    }
    pub fn start_with_windows(&self) -> String {
        self.pick("Start with Windows", "Windows 시작 시 실행", "开机时启动")
    }
    pub fn device_report(&self) -> String {
        self.pick("Device Report…", "기기 진단 보고서…", "设备诊断报告…")
    }
    pub fn quit(&self) -> String {
        self.pick("Quit Device Battery", "Device Battery 종료", "退出")
    }

    // MARK: Dialogs and notifications

    pub fn rename_title(&self) -> String {
        self.pick("Rename Device", "기기 이름 변경", "重命名设备")
    }
    pub fn rename_hint(&self) -> String {
        self.pick(
            "Leave empty to restore the default name.",
            "비워 두면 기본 이름으로 돌아갑니다.",
            "留空则恢复默认名称。",
        )
    }
    pub fn ok(&self) -> String {
        self.pick("OK", "확인", "好")
    }
    pub fn cancel(&self) -> String {
        self.pick("Cancel", "취소", "取消")
    }
    pub fn low_battery_title(&self, name: &str) -> String {
        self.pick(
            &format!("{name} battery low"),
            &format!("{name} 배터리 부족"),
            &format!("{name} 电量低"),
        )
    }
    pub fn low_battery_body(&self, level: u8) -> String {
        self.pick(
            &format!("{level}% left. Time to charge."),
            &format!("{level}% 남았습니다. 충전해 주세요."),
            &format!("剩余 {level}%，记得充电。"),
        )
    }

    /// Sample name used by the preview renderer.
    pub fn my_earbuds(&self) -> String {
        self.pick("My Galaxy Buds", "내 Galaxy Buds", "我的 Galaxy Buds")
    }
}

/// Source label of Bluetooth readings; `Strings::source` translates it for display.
pub const BLUETOOTH_SOURCE: &str = "Bluetooth";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_the_first_supported_preferred_language() {
        let cases: [(&[&str], AppLanguage); 7] = [
            (&["ko-KR", "en-US"], AppLanguage::Korean),
            (&["zh-Hans-CN"], AppLanguage::SimplifiedChinese),
            (&["zh-Hant-TW", "en"], AppLanguage::SimplifiedChinese),
            (&["en-GB"], AppLanguage::English),
            (&["fr-FR", "ko-KR"], AppLanguage::Korean),
            (&["fr-FR", "de-DE"], AppLanguage::English),
            (&[], AppLanguage::English),
        ];
        for (preferred, expected) in cases {
            assert_eq!(AppLanguage::resolve(preferred), expected, "{preferred:?}");
        }
    }

    #[test]
    fn ages_in_every_language() {
        let now = 1_000_000;
        let cases = [
            (20, "just now", "방금", "刚刚"),
            (60, "1 min ago", "1분 전", "1 分钟前"),
            (2 * 3600, "2 hr ago", "2시간 전", "2 小时前"),
            (86_400, "1 day ago", "1일 전", "1 天前"),
            (3 * 86_400, "3 days ago", "3일 전", "3 天前"),
        ];
        for (age, english, korean, chinese) in cases {
            assert_eq!(
                Strings::new(AppLanguage::English).age(now - age, now),
                english
            );
            assert_eq!(
                Strings::new(AppLanguage::Korean).age(now - age, now),
                korean
            );
            assert_eq!(
                Strings::new(AppLanguage::SimplifiedChinese).age(now - age, now),
                chinese
            );
        }
    }

    #[test]
    fn generic_names_are_distinct_in_every_language() {
        for language in AppLanguage::ALL {
            let strings = Strings::new(language);
            let names = [
                strings.rog_device(DeviceKind::Mouse),
                strings.rog_device(DeviceKind::Keyboard),
                strings.rog_device(DeviceKind::Other),
                strings.logitech_device(),
                strings.wireless_mouse(),
                strings.bluetooth_device(),
            ];
            assert!(names.iter().all(|name| !name.is_empty()));
            let unique: std::collections::HashSet<_> = names.iter().collect();
            assert_eq!(unique.len(), names.len());
            assert!(names[0].starts_with("ROG"));
        }
    }

    #[test]
    fn translates_only_the_bluetooth_source() {
        let korean = Strings::new(AppLanguage::Korean);
        assert_eq!(korean.source(BLUETOOTH_SOURCE), "블루투스");
        assert_eq!(korean.source("Logi Bolt"), "Logi Bolt");
    }
}
