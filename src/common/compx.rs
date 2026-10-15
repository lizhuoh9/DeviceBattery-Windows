//! Battery query for Compx-chip 2.4 GHz dongles (VID 0x373B / 0x3554), used by many 4K/8K
//! gaming mice (ATK, VXE, MCHOSE, ...).
//!
//! The dongle takes a 16-byte vendor output report with ID 0x08: command 0x04 asks for the
//! battery, and the last byte is `0x55 - sum(report ID + bytes 0...14)`. The answer has the
//! same ID and command: byte 6 = level (%), byte 7 = charging flag (byte 0 = report ID).

use std::time::Duration;

use super::channel::{BatteryReader, ReportChannel};
use super::model::{ChargeState, DeviceKind, DeviceReading, LinkStatus};
use super::strings::Strings;

pub const VENDOR_IDS: [u16; 2] = [0x373B, 0x3554];
const REPORT_ID: u8 = 0x08;
const BATTERY_COMMAND: u8 = 0x04;
const PAYLOAD_LENGTH: usize = 16;
/// Request length including the report ID.
pub const REQUEST_LENGTH: usize = PAYLOAD_LENGTH + 1;
/// Product IDs known to answer, besides dongles named as mouse dongles.
const KNOWN_PRODUCT_IDS: [u16; 1] = [0x11D9];
const SOURCE: &str = "Compx 2.4G";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatteryReply {
    Level {
        level: u8,
        charging: bool,
    },
    /// The dongle answered but the mouse is off or asleep.
    Asleep,
}

/// Report 0x08 carrying command 0x04 and its checksum.
pub fn battery_request() -> Vec<u8> {
    let mut payload = vec![0u8; PAYLOAD_LENGTH];
    payload[0] = BATTERY_COMMAND;
    let sum = payload[..PAYLOAD_LENGTH - 1]
        .iter()
        .fold(u32::from(REPORT_ID), |total, byte| total + u32::from(*byte));
    payload[PAYLOAD_LENGTH - 1] = 0x55u32.wrapping_sub(sum).to_le_bytes()[0];
    let mut request = vec![REPORT_ID];
    request.extend(payload);
    request
}

/// Whether a HID collection is a mouse dongle's vendor collection that takes the query.
/// Other products on the same chip (keyboards) use different commands, so they are left alone.
pub fn is_battery_collection(
    vendor_id: u16,
    product_id: u16,
    usage_page: u16,
    output_length: usize,
    product: &str,
) -> bool {
    if !VENDOR_IDS.contains(&vendor_id) || usage_page < 0xFF00 || output_length < REQUEST_LENGTH {
        return false;
    }
    let name = product.to_lowercase();
    KNOWN_PRODUCT_IDS.contains(&product_id)
        || (name.contains("mouse") && !name.contains("keyboard"))
}

/// Whether `report` answers the battery command.
pub fn is_battery_reply(report: &[u8]) -> bool {
    report.len() > 7 && report[0] == REPORT_ID && report[1] == BATTERY_COMMAND
}

/// Decodes a battery reply; `None` when it is not one or carries an impossible level.
pub fn parse_battery(report: &[u8]) -> Option<BatteryReply> {
    if !is_battery_reply(report) {
        return None;
    }
    let (level, charging) = (report[6], report[7] != 0);
    match (level, charging) {
        (101.., _) => None,
        (0, false) => Some(BatteryReply::Asleep),
        _ => Some(BatteryReply::Level { level, charging }),
    }
}

/// Turns the dongle's USB product string into a device name.
pub fn display_name(product: &str, strings: &Strings) -> String {
    let trimmed = product.trim();
    // ASCII lowercasing keeps byte offsets, so `lower`'s indices are valid in `trimmed`.
    let lower = trimmed.to_ascii_lowercase();
    let cleaned = match lower.rfind("dongle") {
        // Drop a trailing "dongle" / "dongle-L" and what follows it.
        Some(index)
            if lower[index..]
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-') =>
        {
            trimmed[..index].trim_end()
        }
        _ => trimmed,
    };
    if cleaned.is_empty() {
        strings.wireless_mouse()
    } else {
        cleaned.to_owned()
    }
}

/// Reads the battery of the one mouse behind a Compx dongle.
pub struct CompxReader {
    channel: Box<dyn ReportChannel>,
    id: String,
    name: String,
}

impl CompxReader {
    pub fn new(
        channel: Box<dyn ReportChannel>,
        id: impl Into<String>,
        name: impl Into<String>,
    ) -> Self {
        Self {
            channel,
            id: id.into(),
            name: name.into(),
        }
    }

    fn reading(&self, level: Option<u8>, charge: ChargeState, now: u64) -> DeviceReading {
        let status = if level.is_some() {
            LinkStatus::Online
        } else {
            LinkStatus::Asleep
        };
        DeviceReading::new(
            self.id.clone(),
            self.name.clone(),
            DeviceKind::Mouse,
            level,
            charge,
            status,
            SOURCE,
            now,
        )
    }
}

impl BatteryReader for CompxReader {
    fn read(&mut self, now: u64) -> Vec<DeviceReading> {
        let reply = self.channel.exchange(
            &battery_request(),
            Duration::from_millis(600),
            &is_battery_reply,
        );
        let reading = match reply.ok().as_deref().and_then(parse_battery) {
            Some(BatteryReply::Level { level, charging }) => self.reading(
                Some(level),
                ChargeState::from_charging(charging, level),
                now,
            ),
            _ => self.reading(None, ChargeState::Unknown, now),
        };
        vec![reading]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::channel::ChannelError;
    use crate::common::channel::testing::{FakeChannel, report};
    use crate::common::strings::AppLanguage;

    #[test]
    fn battery_request_has_command_0x04_and_the_checksum() {
        let request = battery_request();
        assert_eq!(request.len(), 17);
        assert_eq!(&request[..2], &[0x08, 0x04]);
        assert!(request[2..16].iter().all(|byte| *byte == 0));
        assert_eq!(request[16], 0x49);
    }

    #[test]
    fn parses_level_charging_and_asleep() {
        assert_eq!(
            parse_battery(&report(&[0x08, 0x04, 0, 0, 0, 0, 76, 1], 17)),
            Some(BatteryReply::Level {
                level: 76,
                charging: true
            })
        );
        assert_eq!(
            parse_battery(&report(&[0x08, 0x04, 0, 0, 0, 0, 0, 0], 17)),
            Some(BatteryReply::Asleep)
        );
        assert_eq!(
            parse_battery(&report(&[0x08, 0x04, 0, 0, 0, 0, 0xC8, 0], 17)),
            None
        );
        assert!(!is_battery_reply(&[0x08, 0x05, 0, 0, 0, 0, 50, 0]));
        assert!(!is_battery_reply(&[0x08, 0x04]));
    }

    #[test]
    fn picks_only_mouse_dongle_vendor_collections() {
        assert!(is_battery_collection(
            0x373B,
            0x11D9,
            0xFF02,
            17,
            "Wireless mouse 8k dongle-L"
        ));
        assert!(is_battery_collection(
            0x3554,
            0xF58A,
            0xFF02,
            17,
            "VXE MOUSE"
        ));
        assert!(!is_battery_collection(
            0x373B,
            0x1234,
            0xFF02,
            64,
            "AULA F75 Keyboard"
        ));
        assert!(!is_battery_collection(
            0x373B,
            0x11D9,
            0x0001,
            49,
            "Wireless mouse 8k dongle-L"
        ));
        assert!(!is_battery_collection(
            0x373B,
            0x11D9,
            0xFF04,
            8,
            "Wireless mouse 8k dongle-L"
        ));
        assert!(!is_battery_collection(0x046D, 0x11D9, 0xFF05, 49, "mouse"));
    }

    #[test]
    fn cleans_the_dongle_name() {
        let strings = Strings::new(AppLanguage::English);
        assert_eq!(
            display_name("Wireless mouse 8k dongle-L", &strings),
            "Wireless mouse 8k"
        );
        assert_eq!(display_name("ATK Dongle", &strings), "ATK");
        assert_eq!(display_name("  ", &strings), "Wireless Mouse");
    }

    #[test]
    fn reader_reports_online_asleep_and_full() {
        let make = |reply: Result<Vec<u8>, ChannelError>| {
            let channel = FakeChannel::new(move |_| reply.clone());
            CompxReader::new(Box::new(channel), "compx.000000000001", "Wireless mouse 8k")
                .read(1_000)
        };
        let online = make(Ok(report(&[0x08, 0x04, 0, 0, 0, 0, 76, 0], 17)));
        assert_eq!(online[0].level, Some(76));
        assert_eq!(online[0].status, LinkStatus::Online);
        assert_eq!(online[0].charge, ChargeState::Discharging);
        assert_eq!(
            make(Err(ChannelError::Timeout))[0].status,
            LinkStatus::Asleep
        );
        assert_eq!(
            make(Ok(report(&[0x08, 0x04, 0, 0, 0, 0, 100, 1], 17)))[0].charge,
            ChargeState::Full
        );
    }
}
