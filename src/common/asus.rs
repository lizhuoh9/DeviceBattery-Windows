//! Battery queries for devices on an ASUS ROG Omni receiver (0B05:1ACE), interface 2.
//!
//! Every packet is a 64-byte vendor report, byte 0 being the report ID:
//! - `01 A0` lists the paired devices: little-endian product IDs every 4 bytes from byte 5.
//! - `03 12 07` asks a mouse: level in byte 5, charging in byte 10.
//! - `02 12 01` asks a keyboard: level in byte 6, charging in byte 9.
//! - The receiver answers `xx FF AA` when the addressed device is asleep or absent.
//!
//! Sources: G-Helper's AsusMouse, rogctl's protocol notes, battery-tray's keyboard capture.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use super::channel::{BatteryReader, ReportChannel, byte_at, padded};
use super::model::{ChargeState, DeviceKind, DeviceReading, LinkStatus};
use super::strings::Strings;

pub const VENDOR_ID: u16 = 0x0B05;
pub const RECEIVER_PRODUCT_ID: u16 = 0x1ACE;
/// Usage pages of the three vendor collections, carrying report IDs 1, 2 and 3.
pub const COLLECTIONS: [(u16, u8); 3] = [(0xFF02, 0x01), (0xFF00, 0x02), (0xFF01, 0x03)];
const PACKET_LENGTH: usize = 64;
const TIMEOUT: Duration = Duration::from_millis(500);
const SOURCE: &str = "ROG Omni";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatteryReply {
    Level { level: u8, charging: bool },
    Asleep,
}

/// Where a device class keeps its battery query and answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Mouse,
    Keyboard,
}

impl Layout {
    pub const ALL: [Layout; 2] = [Layout::Mouse, Layout::Keyboard];

    pub fn request(self) -> Vec<u8> {
        match self {
            Layout::Mouse => padded(&[0x03, 0x12, 0x07], PACKET_LENGTH),
            Layout::Keyboard => padded(&[0x02, 0x12, 0x01], PACKET_LENGTH),
        }
    }

    fn kind(self) -> DeviceKind {
        match self {
            Layout::Mouse => DeviceKind::Mouse,
            Layout::Keyboard => DeviceKind::Keyboard,
        }
    }

    fn level_byte(self) -> usize {
        if self == Layout::Mouse { 5 } else { 6 }
    }

    fn charging_byte(self) -> usize {
        if self == Layout::Mouse { 10 } else { 9 }
    }
}

pub fn paired_request() -> Vec<u8> {
    padded(&[0x01, 0xA0], PACKET_LENGTH)
}

pub fn is_paired_reply(report: &[u8]) -> bool {
    report.len() > 5 && report[0] == 0x01 && report[1] == 0xA0
}

/// Product IDs of the paired devices, in receiver order.
pub fn parse_paired(report: &[u8]) -> Vec<u16> {
    if !is_paired_reply(report) {
        return Vec::new();
    }
    report[5..]
        .chunks(4)
        .filter(|entry| entry.len() >= 2)
        .map(|entry| u16::from_le_bytes([entry[0], entry[1]]))
        .take_while(|id| *id != 0)
        .collect()
}

/// Whether `report` answers `layout`'s query, including the asleep rejection.
pub fn is_reply(report: &[u8], layout: Layout) -> bool {
    let request = layout.request();
    report.len() > 2
        && report[0] == request[0]
        && ((report[1] == request[1] && report[2] == request[2]) || is_rejection(report))
}

pub fn parse_battery(report: &[u8], layout: Layout) -> Option<BatteryReply> {
    if !is_reply(report, layout) {
        return None;
    }
    if is_rejection(report) {
        return Some(BatteryReply::Asleep);
    }
    let level = byte_at(report, layout.level_byte());
    let charging = byte_at(report, layout.charging_byte()) != 0;
    if level > 100 {
        return None;
    }
    if level == 0 && !charging {
        return Some(BatteryReply::Asleep);
    }
    Some(BatteryReply::Level { level, charging })
}

fn is_rejection(report: &[u8]) -> bool {
    report.len() > 2 && report[1] == 0xFF && report[2] == 0xAA
}

/// Known mice by the product ID the receiver reports (from G-Helper / rogctl).
pub fn known_mouse(product_id: u16) -> Option<&'static str> {
    Some(match product_id {
        0x1B65 => "ROG Harpe Ace Mini",
        0x1A94 => "ROG Harpe Ace Aim Lab Edition",
        0x1B68 | 0x1B69 => "ROG Harpe Ace Extreme",
        0x1B1A | 0x1B18 => "ROG Keris II Ace",
        0x1C0E | 0x1D4E => "ROG Keris II Origin",
        0x1A68 | 0x1A6A => "ROG Keris Wireless Aimpoint",
        0x1A72 => "ROG Gladius III Aimpoint",
        0x1AD7 => "ROG Strix Impact III Wireless",
        _ => return None,
    })
}

/// Reads every device paired with one ROG Omni receiver.
pub struct AsusOmniReader {
    channel: Box<dyn ReportChannel>,
    receiver_id: String,
    strings: Strings,
    /// Device classes learned from the command each product answered.
    learned: HashMap<u16, DeviceKind>,
    last_paired: Vec<u16>,
}

impl AsusOmniReader {
    pub fn new(
        channel: Box<dyn ReportChannel>,
        receiver_id: impl Into<String>,
        strings: Strings,
    ) -> Self {
        Self {
            channel,
            receiver_id: receiver_id.into(),
            strings,
            learned: HashMap::new(),
            last_paired: Vec::new(),
        }
    }

    fn paired_products(&mut self) -> Option<Vec<u16>> {
        let reply = self
            .channel
            .exchange(&paired_request(), TIMEOUT, &is_paired_reply)
            .ok()?;
        Some(parse_paired(&reply))
    }

    fn read_device(
        &mut self,
        product_id: u16,
        claimed: &HashSet<DeviceKind>,
        now: u64,
    ) -> DeviceReading {
        for layout in self.layouts(product_id, claimed) {
            let accept = |report: &[u8]| is_reply(report, layout);
            let Ok(reply) = self.channel.exchange(&layout.request(), TIMEOUT, &accept) else {
                continue;
            };
            if let Some(BatteryReply::Level { level, charging }) = parse_battery(&reply, layout) {
                self.learned.insert(product_id, layout.kind());
                return self.reading(
                    product_id,
                    layout.kind(),
                    Some(level),
                    ChargeState::from_charging(charging, level),
                    now,
                );
            }
        }
        self.asleep_reading(product_id, now)
    }

    /// Which queries to try: the known class only, or every class no other device holds.
    /// The queries carry no product ID, so each class answers for one paired device only.
    fn layouts(&self, product_id: u16, claimed: &HashSet<DeviceKind>) -> Vec<Layout> {
        match self.kind_of(product_id) {
            Some(DeviceKind::Mouse) => vec![Layout::Mouse],
            Some(DeviceKind::Keyboard) => vec![Layout::Keyboard],
            _ => Layout::ALL
                .into_iter()
                .filter(|layout| !claimed.contains(&layout.kind()))
                .collect(),
        }
    }

    fn kind_of(&self, product_id: u16) -> Option<DeviceKind> {
        self.learned
            .get(&product_id)
            .copied()
            .or_else(|| known_mouse(product_id).map(|_| DeviceKind::Mouse))
    }

    fn asleep_reading(&self, product_id: u16, now: u64) -> DeviceReading {
        let kind = self.kind_of(product_id).unwrap_or(DeviceKind::Other);
        self.reading(product_id, kind, None, ChargeState::Unknown, now)
    }

    fn reading(
        &self,
        product_id: u16,
        kind: DeviceKind,
        level: Option<u8>,
        charge: ChargeState,
        now: u64,
    ) -> DeviceReading {
        DeviceReading::new(
            format!("rog.{}.{product_id:04X}", self.receiver_id),
            known_mouse(product_id).map_or_else(|| self.strings.rog_device(kind), str::to_owned),
            kind,
            level,
            charge,
            if level.is_some() {
                LinkStatus::Online
            } else {
                LinkStatus::Asleep
            },
            SOURCE,
            now,
        )
    }
}

impl BatteryReader for AsusOmniReader {
    fn read(&mut self, now: u64) -> Vec<DeviceReading> {
        let Some(paired) = self.paired_products() else {
            // The receiver itself did not answer: keep what it listed before.
            let previous = self.last_paired.clone();
            return previous
                .iter()
                .map(|id| self.asleep_reading(*id, now))
                .collect();
        };
        self.last_paired = paired.clone();
        let mut claimed: HashSet<DeviceKind> =
            paired.iter().filter_map(|id| self.kind_of(*id)).collect();
        paired
            .iter()
            .map(|id| {
                let reading = self.read_device(*id, &claimed, now);
                claimed.insert(reading.kind);
                reading
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::channel::ChannelError;
    use crate::common::channel::testing::{FakeChannel, report};
    use crate::common::strings::AppLanguage;

    const NOW: u64 = 2_000;

    fn reader(
        respond: impl FnMut(&[u8]) -> Result<Vec<u8>, ChannelError> + Send + 'static,
    ) -> (
        AsusOmniReader,
        std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
    ) {
        let channel = FakeChannel::new(respond);
        let sent = std::sync::Arc::clone(&channel.sent);
        (
            AsusOmniReader::new(
                Box::new(channel),
                "OMNI0000TEST",
                Strings::new(AppLanguage::SimplifiedChinese),
            ),
            sent,
        )
    }

    fn paired_keyboard() -> Vec<u8> {
        report(&[0x01, 0xA0, 0, 0x01, 0, 0x06, 0x1B, 0x02, 0x04], 64)
    }

    #[test]
    fn requests_are_64_byte_vendor_reports() {
        assert_eq!(paired_request(), report(&[0x01, 0xA0], 64));
        assert_eq!(Layout::Mouse.request(), report(&[0x03, 0x12, 0x07], 64));
        assert_eq!(Layout::Keyboard.request(), report(&[0x02, 0x12, 0x01], 64));
    }

    #[test]
    fn parses_paired_product_ids_until_zero() {
        assert_eq!(parse_paired(&paired_keyboard()), vec![0x1B06]);
        let many = report(
            &[
                0x01, 0xA0, 0, 0, 0, 0x65, 0x1B, 0, 0, 0x1A, 0x1B, 0, 0, 0, 0,
            ],
            64,
        );
        assert_eq!(parse_paired(&many), vec![0x1B65, 0x1B1A]);
    }

    #[test]
    fn decodes_mouse_and_keyboard_replies() {
        let mouse = report(
            &[
                0x03, 0x12, 0x07, 0, 0, 0x50, 0x02, 0x14, 0xD8, 0x0F, 0x00, 0, 0x01,
            ],
            64,
        );
        assert_eq!(
            parse_battery(&mouse, Layout::Mouse),
            Some(BatteryReply::Level {
                level: 80,
                charging: false
            })
        );
        let keyboard = report(&[0x02, 0x12, 0x01, 0, 0, 0, 0x57, 0, 0, 0x01], 64);
        assert_eq!(
            parse_battery(&keyboard, Layout::Keyboard),
            Some(BatteryReply::Level {
                level: 87,
                charging: true
            })
        );
        for layout in Layout::ALL {
            let rejected = report(&[layout.request()[0], 0xFF, 0xAA], 64);
            assert_eq!(parse_battery(&rejected, layout), Some(BatteryReply::Asleep));
        }
        assert_eq!(
            parse_battery(&report(&[0x03, 0x12, 0x07, 0, 0, 0xFF], 64), Layout::Mouse),
            None
        );
        assert!(!is_reply(&report(&[0x02, 0x12, 0x07], 64), Layout::Mouse));
    }

    #[test]
    fn an_unknown_device_is_identified_by_the_command_it_answers() {
        let (mut omni, sent) = reader(|request| {
            Ok(match request[0] {
                0x01 => paired_keyboard(),
                0x03 => report(&[0x03, 0xFF, 0xAA], 64),
                _ => report(&[0x02, 0x12, 0x01, 0, 0, 0, 64, 0, 0, 0], 64),
            })
        });
        let first = omni.read(NOW);
        assert_eq!(first[0].kind, DeviceKind::Keyboard);
        assert_eq!(first[0].level, Some(64));
        assert_eq!(first[0].name, "ROG 无线键盘");
        assert_eq!(first[0].id, "rog.OMNI0000TEST.1B06");
        let before = sent.lock().expect("sent").len();
        omni.read(NOW);
        let sent = sent.lock().expect("sent");
        assert_eq!(sent.len() - before, 2);
        assert_eq!(sent.last().map(|r| r[0]), Some(0x02));
    }

    #[test]
    fn an_unknown_device_next_to_a_known_mouse_is_only_asked_as_a_keyboard() {
        let paired = report(
            &[
                0x01, 0xA0, 0, 0x02, 0, 0x65, 0x1B, 0x01, 0x04, 0x06, 0x1B, 0x02, 0x04,
            ],
            64,
        );
        let (mut omni, sent) = reader(move |request| {
            Ok(match request[0] {
                0x01 => paired.clone(),
                0x03 => report(&[0x03, 0x12, 0x07, 0, 0, 55], 64),
                _ => report(&[0x02, 0x12, 0x01, 0, 0, 0, 64], 64),
            })
        });
        let readings = omni.read(NOW);
        assert_eq!(
            readings.iter().map(|r| r.kind).collect::<Vec<_>>(),
            vec![DeviceKind::Mouse, DeviceKind::Keyboard]
        );
        assert_eq!(
            readings.iter().map(|r| r.level).collect::<Vec<_>>(),
            vec![Some(55), Some(64)]
        );
        assert_eq!(
            sent.lock()
                .expect("sent")
                .iter()
                .map(|r| r[0])
                .collect::<Vec<_>>(),
            vec![0x01, 0x03, 0x02]
        );
    }

    #[test]
    fn a_silent_receiver_keeps_its_devices_as_asleep() {
        let answering = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let flag = std::sync::Arc::clone(&answering);
        let (mut omni, _) = reader(move |request| {
            if !flag.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(ChannelError::Timeout);
            }
            Ok(if request[0] == 0x01 {
                report(&[0x01, 0xA0, 0, 0x01, 0, 0x65, 0x1B, 0x01, 0x04], 64)
            } else {
                report(&[0x03, 0x12, 0x07, 0, 0, 55], 64)
            })
        });
        omni.read(NOW);
        answering.store(false, std::sync::atomic::Ordering::SeqCst);
        let readings = omni.read(NOW + 60);
        assert_eq!(readings.len(), 1);
        assert_eq!(readings[0].status, LinkStatus::Asleep);
        assert_eq!(readings[0].name, "ROG Harpe Ace Mini");
    }
}
