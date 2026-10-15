//! Text and tone for one device row, independent of the UI toolkit.

use super::model::{ChargeState, DeviceReading, LinkStatus};
use super::strings::Strings;

pub const LOW_THRESHOLD: u8 = 20;
pub const CRITICAL_THRESHOLD: u8 = 10;

/// The visual emphasis of a row; the renderer maps it onto colors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowTone {
    Normal,
    Low,
    Critical,
    Charging,
    Inactive,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RowPresentation {
    pub percent: String,
    pub detail: String,
    pub tone: RowTone,
    /// 0...=1 for the ring; `None` draws an empty ring.
    pub fraction: Option<f32>,
}

impl RowPresentation {
    pub fn new(reading: &DeviceReading, now: u64, strings: &Strings) -> Self {
        Self {
            percent: reading
                .level
                .map_or_else(|| "—".to_owned(), |level| level.to_string()),
            detail: detail(reading, now, strings),
            tone: tone(reading),
            fraction: reading.level.map(|level| f32::from(level) / 100.0),
        }
    }
}

fn tone(reading: &DeviceReading) -> RowTone {
    if reading.status == LinkStatus::Asleep {
        return RowTone::Inactive;
    }
    if matches!(reading.charge, ChargeState::Charging | ChargeState::Full) {
        return RowTone::Charging;
    }
    match reading.level {
        Some(level) if level <= CRITICAL_THRESHOLD => RowTone::Critical,
        Some(level) if level <= LOW_THRESHOLD => RowTone::Low,
        _ => RowTone::Normal,
    }
}

fn detail(reading: &DeviceReading, now: u64, strings: &Strings) -> String {
    if reading.status == LinkStatus::Asleep {
        return match reading.level {
            Some(_) => strings.asleep_since(&strings.age(reading.date, now)),
            None => strings.asleep_until_wake(),
        };
    }
    match (reading.charge, reading.level) {
        (ChargeState::Charging, _) => strings.charging(),
        (ChargeState::Full, _) => strings.fully_charged(),
        (_, Some(level)) if level <= CRITICAL_THRESHOLD => strings.critical_battery(),
        (_, Some(level)) if level <= LOW_THRESHOLD => strings.low_battery(),
        _ => strings.source(&reading.source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::model::DeviceKind;
    use crate::common::strings::{AppLanguage, BLUETOOTH_SOURCE};

    const NOW: u64 = 100_000;

    fn reading(
        level: Option<u8>,
        charge: ChargeState,
        status: LinkStatus,
        source: &str,
        age: u64,
    ) -> DeviceReading {
        DeviceReading::new(
            "x",
            "X",
            DeviceKind::Mouse,
            level,
            charge,
            status,
            source,
            NOW - age,
        )
    }

    fn detail_in(language: AppLanguage, reading: &DeviceReading) -> String {
        RowPresentation::new(reading, NOW, &Strings::new(language)).detail
    }

    #[test]
    fn tone_follows_level_charging_and_reachability() {
        let zh = Strings::new(AppLanguage::SimplifiedChinese);
        let cases = [
            (
                Some(80),
                ChargeState::Discharging,
                LinkStatus::Online,
                RowTone::Normal,
            ),
            (
                Some(20),
                ChargeState::Discharging,
                LinkStatus::Online,
                RowTone::Low,
            ),
            (
                Some(10),
                ChargeState::Discharging,
                LinkStatus::Online,
                RowTone::Critical,
            ),
            (
                Some(5),
                ChargeState::Charging,
                LinkStatus::Online,
                RowTone::Charging,
            ),
            (
                Some(100),
                ChargeState::Full,
                LinkStatus::Online,
                RowTone::Charging,
            ),
            (
                Some(80),
                ChargeState::Discharging,
                LinkStatus::Asleep,
                RowTone::Inactive,
            ),
        ];
        for (level, charge, status, expected) in cases {
            assert_eq!(
                RowPresentation::new(&reading(level, charge, status, "Logi Bolt", 0), NOW, &zh)
                    .tone,
                expected
            );
        }
    }

    #[test]
    fn detail_lines_in_three_languages() {
        let asleep = reading(
            Some(86),
            ChargeState::Discharging,
            LinkStatus::Asleep,
            "Logi Bolt",
            3 * 3600,
        );
        assert_eq!(
            detail_in(AppLanguage::English, &asleep),
            "Asleep · 3 hr ago"
        );
        assert_eq!(
            detail_in(AppLanguage::Korean, &asleep),
            "절전 중 · 3시간 전"
        );
        assert_eq!(
            detail_in(AppLanguage::SimplifiedChinese, &asleep),
            "休眠 · 3 小时前"
        );

        let low = reading(
            Some(7),
            ChargeState::Discharging,
            LinkStatus::Online,
            "Logi Bolt",
            0,
        );
        assert_eq!(
            detail_in(AppLanguage::English, &low),
            "Very low — charge soon"
        );
        assert_eq!(
            detail_in(AppLanguage::Korean, &low),
            "배터리 매우 부족 · 충전 필요"
        );

        let charging = reading(
            Some(40),
            ChargeState::Charging,
            LinkStatus::Online,
            "Logi Bolt",
            0,
        );
        assert_eq!(
            detail_in(AppLanguage::SimplifiedChinese, &charging),
            "充电中"
        );

        let unknown = reading(
            None,
            ChargeState::Unknown,
            LinkStatus::Asleep,
            "Logi Bolt",
            0,
        );
        assert_eq!(
            detail_in(AppLanguage::English, &unknown),
            "Asleep — updates when it wakes"
        );

        let bluetooth = reading(
            Some(80),
            ChargeState::Unknown,
            LinkStatus::Online,
            BLUETOOTH_SOURCE,
            0,
        );
        assert_eq!(detail_in(AppLanguage::Korean, &bluetooth), "블루투스");
        assert_eq!(
            detail_in(
                AppLanguage::English,
                &reading(
                    Some(80),
                    ChargeState::Discharging,
                    LinkStatus::Online,
                    "Logi Bolt",
                    0
                )
            ),
            "Logi Bolt"
        );
    }

    #[test]
    fn percent_text_and_fraction() {
        let zh = Strings::new(AppLanguage::SimplifiedChinese);
        let known = RowPresentation::new(
            &reading(
                Some(86),
                ChargeState::Discharging,
                LinkStatus::Online,
                "x",
                0,
            ),
            NOW,
            &zh,
        );
        assert_eq!((known.percent.as_str(), known.fraction), ("86", Some(0.86)));
        let unknown = RowPresentation::new(
            &reading(None, ChargeState::Unknown, LinkStatus::Asleep, "x", 0),
            NOW,
            &zh,
        );
        assert_eq!((unknown.percent.as_str(), unknown.fraction), ("—", None));
    }
}
