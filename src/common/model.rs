//! Battery readings as produced by the readers and shown by the widget.

use serde::{Deserialize, Serialize};

/// What a device is; drives its glyph and where it sorts in the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceKind {
    Mouse,
    Keyboard,
    Trackpad,
    Headphones,
    Gamepad,
    Speaker,
    Other,
}

impl DeviceKind {
    /// Position in the list: mice first, then keyboards, and so on.
    pub fn order(self) -> usize {
        match self {
            DeviceKind::Mouse => 0,
            DeviceKind::Keyboard => 1,
            DeviceKind::Trackpad => 2,
            DeviceKind::Headphones => 3,
            DeviceKind::Gamepad => 4,
            DeviceKind::Speaker => 5,
            DeviceKind::Other => 6,
        }
    }
}

/// Charging state as reported by the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChargeState {
    Discharging,
    Charging,
    Full,
    Unknown,
}

impl ChargeState {
    /// Maps a plain "is it charging" flag, treating a charging device at 100% as full.
    pub fn from_charging(charging: bool, level: u8) -> Self {
        match (charging, level >= 100) {
            (false, _) => ChargeState::Discharging,
            (true, true) => ChargeState::Full,
            (true, false) => ChargeState::Charging,
        }
    }
}

/// Whether the device could be reached on the latest read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkStatus {
    /// Answered with a battery level.
    Online,
    /// Paired but not reachable right now: sleeping, switched off or out of range.
    Asleep,
}

/// A battery reading for one device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceReading {
    /// Stable across launches, e.g. `logi.1A2B3C4D` or `bt.AABBCCDDEE10`.
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    /// 0...=100, or `None` when the device could not report one this time.
    pub level: Option<u8>,
    pub charge: ChargeState,
    pub status: LinkStatus,
    /// Where the reading came from, e.g. `Logi Bolt`.
    pub source: String,
    /// When the level was read, in seconds since the Unix epoch.
    pub date: u64,
}

impl DeviceReading {
    /// A reading; `level` is clamped to 0...=100.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        kind: DeviceKind,
        level: Option<u8>,
        charge: ChargeState,
        status: LinkStatus,
        source: impl Into<String>,
        date: u64,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            kind,
            level: level.map(|value| value.min(100)),
            charge,
            status,
            source: source.into(),
            date,
        }
    }

    /// A copy with a different display name.
    pub fn renamed(&self, name: &str) -> Self {
        Self {
            name: name.to_owned(),
            ..self.clone()
        }
    }
}
