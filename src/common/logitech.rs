//! Reads every device paired with one Logitech receiver over HID++.
//!
//! Pairings come from the receiver's registers (Bolt / Unifying) or, for other receivers,
//! from pinging device indices 1...=6. Each device's battery feature and name are looked up
//! when it connects; while it stays connected a read costs one request per device.

use std::collections::HashMap;
use std::time::Duration;

use super::channel::{BatteryReader, ReportChannel, byte_at};
use super::hidpp::{self, BatteryState, Feature, PairedSlot, ReceiverKind, Reply};
use super::model::{ChargeState, DeviceKind, DeviceReading, LinkStatus};
use super::strings::Strings;

const RECEIVER_TIMEOUT: Duration = Duration::from_millis(800);
const DEVICE_TIMEOUT: Duration = Duration::from_millis(1500);
const REDISCOVERY_INTERVAL: u64 = 30 * 60;
/// Device names are short; this bounds the name loop against a bogus length byte.
const MAX_NAME_LENGTH: usize = 64;

#[derive(Clone, Copy, Debug)]
enum BatteryLookup {
    Found(Feature, u8),
    Absent,
}

/// What a device told us about itself; kept while it sleeps.
#[derive(Clone, Debug, Default)]
struct Identity {
    name: Option<String>,
    kind: Option<DeviceKind>,
    battery_index: Option<u8>,
}

pub struct LogitechReader {
    channel: Box<dyn ReportChannel>,
    receiver: ReceiverKind,
    receiver_id: String,
    strings: Strings,
    slots: Vec<PairedSlot>,
    discovered_at: Option<u64>,
    /// Battery feature per device, valid for its current connection only.
    batteries: HashMap<String, BatteryLookup>,
    identities: HashMap<String, Identity>,
}

impl LogitechReader {
    pub fn new(
        channel: Box<dyn ReportChannel>,
        receiver: ReceiverKind,
        receiver_id: impl Into<String>,
        strings: Strings,
    ) -> Self {
        Self {
            channel,
            receiver,
            receiver_id: receiver_id.into(),
            strings,
            slots: Vec::new(),
            discovered_at: None,
            batteries: HashMap::new(),
            identities: HashMap::new(),
        }
    }

    /// Forces the pairing list to be read again on the next `read`.
    pub fn invalidate_pairings(&mut self) {
        self.discovered_at = None;
    }

    /// Looks at a report the receiver sent on its own. Returns true when it is worth a read:
    /// a device connected or disconnected, or broadcast a battery change.
    pub fn notice(&mut self, report: &[u8]) -> bool {
        if report.len() < 7
            || (report[0] != hidpp::SHORT_REPORT && report[0] != hidpp::LONG_REPORT)
            || !(1..=6).contains(&report[1])
        {
            return false;
        }
        let slot = self
            .slots
            .iter()
            .find(|slot| slot.index == report[1])
            .cloned();
        if report[2] == hidpp::CONNECTION_NOTIFICATION {
            // Whatever is in that slot now gets its features looked up again.
            match slot {
                Some(slot) => {
                    self.batteries.remove(&self.id_of(&slot));
                }
                None => self.invalidate_pairings(),
            }
            return true;
        }
        let Some(slot) = slot else { return false };
        let battery_index = self
            .identities
            .get(&self.id_of(&slot))
            .and_then(|identity| identity.battery_index);
        report[3] & 0x0F == 0 && battery_index == Some(report[2])
    }

    // MARK: Pairings

    fn discover_if_needed(&mut self, now: u64) {
        let interval = if self.receiver == ReceiverKind::Other {
            0
        } else {
            REDISCOVERY_INTERVAL
        };
        if self
            .discovered_at
            .is_some_and(|at| now.saturating_sub(at) < interval)
        {
            return;
        }
        let found = if self.receiver == ReceiverKind::Other {
            self.probe_slots()
        } else {
            self.read_pairing_registers()
        };
        // A silent receiver keeps the previous list; try again next time.
        if let Some(found) = found {
            self.slots = found;
            self.discovered_at = Some(now);
        }
    }

    fn read_pairing_registers(&mut self) -> Option<Vec<PairedSlot>> {
        let mut found = Vec::new();
        for index in 1..=6u8 {
            let Some(request) = hidpp::pairing_request(index, self.receiver) else {
                continue;
            };
            let reply = self.send(&request, RECEIVER_TIMEOUT)?;
            let Reply::Success(params) = reply else {
                continue;
            };
            let extended = if self.receiver == ReceiverKind::Unifying {
                self.register_params(&hidpp::extended_pairing_request(index))
            } else {
                None
            };
            let Some(mut slot) =
                hidpp::parse_pairing(&params, index, self.receiver, extended.as_deref())
            else {
                continue;
            };
            if let Some(name_request) = hidpp::name_request(index, self.receiver) {
                slot.codename = self
                    .register_params(&name_request)
                    .and_then(|params| hidpp::parse_codename(&params, self.receiver));
            }
            found.push(slot);
        }
        Some(found)
    }

    /// Receivers without pairing registers: a device answers, or the receiver says it is asleep.
    fn probe_slots(&mut self) -> Option<Vec<PairedSlot>> {
        let mut found = Vec::new();
        let mut receiver_answered = false;
        for index in 1..=6u8 {
            match self.ping(index) {
                Some(Reply::Success(_)) | Some(Reply::Unreachable(hidpp::DEVICE_ASLEEP_ERROR)) => {
                    receiver_answered = true;
                    found.push(PairedSlot {
                        index,
                        kind: DeviceKind::Other,
                        serial: None,
                        codename: None,
                    });
                }
                Some(Reply::Unreachable(_)) | Some(Reply::FeatureError(_)) => {
                    receiver_answered = true
                }
                None => {}
            }
        }
        receiver_answered.then_some(found)
    }

    fn register_params(&mut self, request: &[u8]) -> Option<Vec<u8>> {
        match self.send(request, RECEIVER_TIMEOUT)? {
            Reply::Success(params) => Some(params),
            _ => None,
        }
    }

    // MARK: Devices

    fn read_slot(&mut self, slot: &PairedSlot, now: u64) -> Option<DeviceReading> {
        let key = self.id_of(slot);
        if !self.batteries.contains_key(&key) {
            let learned = match self.ping(slot.index) {
                Some(Reply::Success(_)) => self.learn(slot.index, &key),
                _ => None,
            };
            let Some(battery) = learned else {
                return Some(self.reading(slot, None, now));
            };
            self.batteries.insert(key.clone(), battery);
        }
        let BatteryLookup::Found(feature, feature_index) = *self.batteries.get(&key)? else {
            return None;
        };

        let request = hidpp::request(slot.index, feature_index, feature.status_function(), &[]);
        let Some(Reply::Success(params)) = self.send(&request, DEVICE_TIMEOUT) else {
            // Gone or changed: look its features up again once it answers, so a cached index
            // is never sent to a different device or firmware.
            self.batteries.remove(&key);
            return Some(self.reading(slot, None, now));
        };
        let state = hidpp::decode_battery(feature, &params);
        // A live device cannot be at 0% unless it is charging; treat that as "unknown".
        let plausible =
            state.level > 0 || matches!(state.charge, ChargeState::Charging | ChargeState::Full);
        Some(self.reading(slot, plausible.then_some(state), now))
    }

    /// Looks up the battery feature, name and type of a freshly connected device;
    /// `None` when it stopped answering.
    fn learn(&mut self, device: u8, key: &str) -> Option<BatteryLookup> {
        let mut battery = BatteryLookup::Absent;
        for feature in Feature::BATTERIES {
            let index = self.feature_index(feature, device)?;
            if index != 0 {
                battery = BatteryLookup::Found(feature, index);
                break;
            }
        }
        let mut identity = self.identities.get(key).cloned().unwrap_or_default();
        if let BatteryLookup::Found(_, index) = battery {
            identity.battery_index = Some(index);
        }
        if let Some(name_index) = self
            .feature_index(Feature::DeviceName, device)
            .filter(|index| *index != 0)
        {
            identity.name = self.device_name(device, name_index).or(identity.name);
            identity.kind = self.device_kind(device, name_index).or(identity.kind);
        }
        self.identities.insert(key.to_owned(), identity);
        Some(battery)
    }

    /// The feature's index on the device: 0 when it lacks it, `None` when the call failed.
    fn feature_index(&mut self, feature: Feature, device: u8) -> Option<u8> {
        let request = hidpp::request(device, 0, 0, &feature.id().to_be_bytes());
        match self.send(&request, DEVICE_TIMEOUT)? {
            Reply::Success(params) => Some(byte_at(&params, 0)),
            _ => None,
        }
    }

    fn device_name(&mut self, device: u8, feature_index: u8) -> Option<String> {
        let Reply::Success(count) = self.send(
            &hidpp::request(device, feature_index, 0, &[]),
            DEVICE_TIMEOUT,
        )?
        else {
            return None;
        };
        let length = usize::from(byte_at(&count, 0)).min(MAX_NAME_LENGTH);
        let mut bytes = Vec::with_capacity(length);
        while bytes.len() < length {
            let offset = u8::try_from(bytes.len()).ok()?;
            let request = hidpp::request(device, feature_index, 1, &[offset]);
            let Reply::Success(chunk) = self.send(&request, DEVICE_TIMEOUT)? else {
                return None;
            };
            let part: Vec<u8> = chunk.into_iter().take(length - bytes.len()).collect();
            if part.is_empty() {
                break;
            }
            bytes.extend(part);
        }
        let end = bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bytes.len());
        let name = String::from_utf8_lossy(&bytes[..end]).trim().to_owned();
        (!name.is_empty()).then_some(name)
    }

    fn device_kind(&mut self, device: u8, feature_index: u8) -> Option<DeviceKind> {
        let Reply::Success(params) = self.send(
            &hidpp::request(device, feature_index, 2, &[]),
            DEVICE_TIMEOUT,
        )?
        else {
            return None;
        };
        let kind = hidpp::kind_from_device_type(byte_at(&params, 0));
        (kind != DeviceKind::Other).then_some(kind)
    }

    // MARK: Helpers

    fn ping(&mut self, device: u8) -> Option<Reply> {
        self.send(&hidpp::request(device, 0, 1, &[0, 0, 0x5A]), DEVICE_TIMEOUT)
    }

    fn send(&mut self, request: &[u8], timeout: Duration) -> Option<Reply> {
        let accept = |report: &[u8]| hidpp::reply(request, report).is_some();
        let report = self.channel.exchange(request, timeout, &accept).ok()?;
        hidpp::reply(request, &report)
    }

    fn id_of(&self, slot: &PairedSlot) -> String {
        match &slot.serial {
            Some(serial) => format!("logi.{serial}"),
            None => format!("logi.{}.{}", self.receiver_id, slot.index),
        }
    }

    fn reading(&self, slot: &PairedSlot, battery: Option<BatteryState>, now: u64) -> DeviceReading {
        let id = self.id_of(slot);
        let identity = self.identities.get(&id);
        let name = identity
            .and_then(|identity| identity.name.clone())
            .or_else(|| slot.codename.clone())
            .unwrap_or_else(|| self.strings.logitech_device());
        DeviceReading::new(
            id.clone(),
            name,
            identity
                .and_then(|identity| identity.kind)
                .unwrap_or(slot.kind),
            battery.map(|state| state.level),
            battery.map_or(ChargeState::Unknown, |state| state.charge),
            if battery.is_some() {
                LinkStatus::Online
            } else {
                LinkStatus::Asleep
            },
            self.receiver.source_name(),
            now,
        )
    }
}

impl BatteryReader for LogitechReader {
    fn read(&mut self, now: u64) -> Vec<DeviceReading> {
        self.discover_if_needed(now);
        let slots = self.slots.clone();
        slots
            .iter()
            .filter_map(|slot| self.read_slot(slot, now))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::common::channel::ChannelError;
    use crate::common::channel::testing::{FakeChannel, report};
    use crate::common::strings::AppLanguage;

    /// A Bolt receiver with an MX Keys S in slot 1, scripted after real captures.
    struct BoltSimulator {
        online: bool,
        features: HashMap<u16, u8>,
        battery_call: (u8, u8),
        battery: Vec<u8>,
        name: String,
        exposes_registers: bool,
    }

    impl Default for BoltSimulator {
        fn default() -> Self {
            Self {
                online: true,
                features: HashMap::from([(0x1004, 0x08), (0x0005, 0x03)]),
                battery_call: (0x08, 1),
                battery: vec![0x56, 0x04, 0x00, 0x00],
                name: "MX Keys S".into(),
                exposes_registers: true,
            }
        }
    }

    impl BoltSimulator {
        fn respond(&self, request: &[u8]) -> Result<Vec<u8>, ChannelError> {
            let (device, sub, tag) = (request[1], request[2], request[3]);
            if device == 0xFF {
                return Ok(self.register(request));
            }
            if device != 1 {
                return Ok(vec![0x10, device, 0x8F, sub, tag, 0x09, 0x00]);
            }
            if !self.online {
                return Ok(vec![0x10, device, 0x8F, sub, tag, 0x04, 0x00]);
            }
            let reply = |params: &[u8]| {
                let mut bytes = vec![0x11, device, sub, tag];
                bytes.extend_from_slice(params);
                report(&bytes, 20)
            };
            Ok(match (sub, tag >> 4) {
                (0x00, 0) => {
                    let feature = u16::from_be_bytes([request[4], request[5]]);
                    reply(&[self.features.get(&feature).copied().unwrap_or(0)])
                }
                (0x00, 1) => reply(&[0x04, 0x05, request[6]]),
                (0x03, 0) => reply(&[self.name.len() as u8]),
                (0x03, 1) => {
                    let chunk: Vec<u8> = self
                        .name
                        .bytes()
                        .skip(usize::from(request[4]))
                        .take(16)
                        .collect();
                    reply(&chunk)
                }
                (0x03, 2) => reply(&[0x00]),
                (index, function) if (index, function) == self.battery_call => reply(&self.battery),
                _ => report(&[0x11, device, 0xFF, sub, tag, 0x02], 20),
            })
        }

        fn register(&self, request: &[u8]) -> Vec<u8> {
            match (self.exposes_registers, request[4]) {
                (true, 0x51) => vec![
                    0x11, 0xFF, 0x83, 0xB5, 0x51, 0x41, 0x80, 0xB3, 0x1A, 0x2B, 0x3C, 0x4D, 0x01,
                    0x00, 0x80, 0x14, 0x01, 0x00, 0x00, 0x00,
                ],
                (true, 0x61) => {
                    let mut bytes = vec![0x11, 0xFF, 0x83, 0xB5, 0x61, 0x01, 0x09];
                    bytes.extend_from_slice(b"MX KEYS B");
                    report(&bytes, 20)
                }
                (registers, _) => vec![
                    0x10,
                    0xFF,
                    0x8F,
                    0x83,
                    0xB5,
                    if registers { 0x08 } else { 0x02 },
                    0x00,
                ],
            }
        }
    }

    struct Harness {
        device: Arc<Mutex<BoltSimulator>>,
        sent: Arc<Mutex<Vec<Vec<u8>>>>,
        reader: LogitechReader,
    }

    fn harness(kind: ReceiverKind, device: BoltSimulator) -> Harness {
        let device = Arc::new(Mutex::new(device));
        let simulator = Arc::clone(&device);
        let channel =
            FakeChannel::new(move |request| simulator.lock().expect("simulator").respond(request));
        let sent = Arc::clone(&channel.sent);
        let reader = LogitechReader::new(
            Box::new(channel),
            kind,
            "loc1130000",
            Strings::new(AppLanguage::SimplifiedChinese),
        );
        Harness {
            device,
            sent,
            reader,
        }
    }

    const NOW: u64 = 3_000;

    #[test]
    fn reads_an_awake_keyboard_through_the_bolt_receiver() {
        let mut h = harness(ReceiverKind::Bolt, BoltSimulator::default());
        let readings = h.reader.read(NOW);
        assert_eq!(
            readings,
            vec![DeviceReading::new(
                "logi.1A2B3C4D",
                "MX Keys S",
                DeviceKind::Keyboard,
                Some(86),
                ChargeState::Discharging,
                LinkStatus::Online,
                "Logi Bolt",
                NOW
            )]
        );
    }

    #[test]
    fn a_sleeping_keyboard_is_listed_by_its_receiver_codename() {
        let mut h = harness(
            ReceiverKind::Bolt,
            BoltSimulator {
                online: false,
                ..Default::default()
            },
        );
        let readings = h.reader.read(NOW);
        assert_eq!(readings.len(), 1);
        assert_eq!(readings[0].name, "MX KEYS B");
        assert_eq!(readings[0].kind, DeviceKind::Keyboard);
        assert_eq!(readings[0].status, LinkStatus::Asleep);
        assert_eq!(readings[0].level, None);
    }

    #[test]
    fn pairings_and_features_are_looked_up_once() {
        let mut h = harness(ReceiverKind::Bolt, BoltSimulator::default());
        h.reader.read(NOW);
        let before = h.sent.lock().expect("sent").len();
        let second = h.reader.read(NOW + 60);
        assert_eq!(h.sent.lock().expect("sent").len() - before, 1);
        assert_eq!(second[0].level, Some(86));
    }

    #[test]
    fn falls_back_to_the_battery_status_feature() {
        let device = BoltSimulator {
            features: HashMap::from([(0x1000, 0x06), (0x0005, 0x03)]),
            battery_call: (0x06, 0),
            battery: vec![0x1E, 0x0A, 0x01],
            ..Default::default()
        };
        let readings = harness(ReceiverKind::Bolt, device).reader.read(NOW);
        assert_eq!(readings[0].level, Some(30));
        assert_eq!(readings[0].charge, ChargeState::Charging);
    }

    #[test]
    fn a_device_without_a_battery_feature_is_left_out() {
        let device = BoltSimulator {
            features: HashMap::from([(0x0005, 0x03)]),
            ..Default::default()
        };
        assert!(
            harness(ReceiverKind::Bolt, device)
                .reader
                .read(NOW)
                .is_empty()
        );
    }

    #[test]
    fn keeps_the_hidpp_name_after_the_device_falls_asleep() {
        let mut h = harness(ReceiverKind::Bolt, BoltSimulator::default());
        h.reader.read(NOW);
        h.device.lock().expect("device").online = false;
        let readings = h.reader.read(NOW + 60);
        assert_eq!(readings[0].name, "MX Keys S");
        assert_eq!(readings[0].status, LinkStatus::Asleep);
    }

    #[test]
    fn revalidates_features_when_a_device_returns() {
        let mut h = harness(ReceiverKind::Bolt, BoltSimulator::default());
        h.reader.read(NOW);
        h.device.lock().expect("device").online = false;
        h.reader.read(NOW + 60);
        {
            // New firmware moved the battery feature while the keyboard was away.
            let mut device = h.device.lock().expect("device");
            device.features = HashMap::from([(0x1004, 0x09), (0x0005, 0x03)]);
            device.battery_call = (0x09, 1);
            device.online = true;
        }
        let readings = h.reader.read(NOW + 120);
        assert_eq!(readings[0].status, LinkStatus::Online);
        assert_eq!(readings[0].level, Some(86));
    }

    #[test]
    fn an_all_zero_battery_reply_is_not_an_empty_battery() {
        let device = BoltSimulator {
            battery: vec![0, 0, 0, 0],
            ..Default::default()
        };
        let readings = harness(ReceiverKind::Bolt, device).reader.read(NOW);
        assert_eq!(readings[0].level, None);
        assert_eq!(readings[0].status, LinkStatus::Asleep);
    }

    #[test]
    fn only_connection_changes_and_battery_broadcasts_ask_for_a_refresh() {
        let mut h = harness(ReceiverKind::Bolt, BoltSimulator::default());
        h.reader.read(NOW);
        assert!(
            h.reader
                .notice(&report(&[0x11, 0x01, 0x08, 0x00, 0x50, 0x04, 0x01], 20))
        );
        assert!(
            !h.reader
                .notice(&report(&[0x11, 0x01, 0x0A, 0x00, 0x01], 20))
        );
        assert!(
            !h.reader
                .notice(&report(&[0x11, 0x01, 0x08, 0x1B, 0x50], 20))
        );
        assert!(h.reader.notice(&[0x10, 0x01, 0x41, 0x10, 0x01, 0x80, 0xB3]));
    }

    #[test]
    fn a_connection_from_an_unlisted_slot_rereads_the_pairings() {
        let mut h = harness(ReceiverKind::Bolt, BoltSimulator::default());
        h.reader.read(NOW);
        let register_reads = |sent: &Arc<Mutex<Vec<Vec<u8>>>>| {
            sent.lock()
                .expect("sent")
                .iter()
                .filter(|r| r[1] == 0xFF)
                .count()
        };
        let before = register_reads(&h.sent);
        h.reader.read(NOW + 60);
        assert_eq!(register_reads(&h.sent), before);
        assert!(h.reader.notice(&[0x10, 0x02, 0x41, 0x10, 0x02, 0x40, 0x40]));
        h.reader.read(NOW + 120);
        assert!(register_reads(&h.sent) > before);
    }

    #[test]
    fn receivers_without_pairing_registers_are_probed_by_index() {
        let device = BoltSimulator {
            exposes_registers: false,
            ..Default::default()
        };
        let readings = harness(ReceiverKind::Other, device).reader.read(NOW);
        assert_eq!(
            readings.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["logi.loc1130000.1"]
        );
        assert_eq!(readings[0].name, "MX Keys S");
        assert_eq!(readings[0].source, "Logitech");
    }
}
