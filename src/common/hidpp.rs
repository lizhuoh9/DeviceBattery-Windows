//! Logitech HID++ framing, battery decoding and receiver pairing registers
//! (after Logitech's HID++ 2.0 specification and Solaar).
//!
//! Short reports are `10 <device> <sub-ID or feature index> <function|software ID> p0 p1 p2`,
//! long reports start with `11` and carry 16 parameter bytes. Device 0xFF is the receiver.

use super::channel::{byte_at, hex_upper, padded};
use super::model::{ChargeState, DeviceKind};

pub const VENDOR_ID: u16 = 0x046D;
/// Usage page of the receiver collections that carry HID++.
pub const USAGE_PAGE: u16 = 0xFF00;
pub const SHORT_REPORT: u8 = 0x10;
pub const LONG_REPORT: u8 = 0x11;
pub const SHORT_LENGTH: usize = 7;
pub const LONG_LENGTH: usize = 20;
const RECEIVER_INDEX: u8 = 0xFF;
/// Tags our requests so replies to other software (Logi Options+) are not mistaken for ours.
const SOFTWARE_ID: u8 = 0x0B;
const READ_LONG_REGISTER: u8 = 0x83;
/// HID++ 1.0 notification: a device connected to or disconnected from the receiver.
pub const CONNECTION_NOTIFICATION: u8 = 0x41;
/// The receiver answers this for a paired device it cannot reach right now.
pub const DEVICE_ASLEEP_ERROR: u8 = 0x04;

/// How a report answers a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    /// Parameter bytes, starting at byte 4 of the report.
    Success(Vec<u8>),
    /// HID++ 2.0 error from the device (`FF` frame).
    FeatureError(u8),
    /// HID++ 1.0 error (`8F` frame). From a receiver, 0x04 means the device is asleep.
    Unreachable(u8),
}

/// A HID++ 2.0 feature call; `function` is 0...=15.
pub fn request(device: u8, feature_index: u8, function: u8, params: &[u8]) -> Vec<u8> {
    let long = params.len() > 3;
    let mut bytes = vec![
        if long { LONG_REPORT } else { SHORT_REPORT },
        device,
        feature_index,
        (function << 4) | SOFTWARE_ID,
    ];
    bytes.extend_from_slice(params);
    padded(&bytes, if long { LONG_LENGTH } else { SHORT_LENGTH })
}

/// A HID++ 1.0 long-register read addressed to the receiver.
pub fn register_read(address: u8, params: &[u8]) -> Vec<u8> {
    let mut bytes = vec![SHORT_REPORT, RECEIVER_INDEX, READ_LONG_REGISTER, address];
    bytes.extend_from_slice(params);
    padded(&bytes, SHORT_LENGTH)
}

/// Classifies `report` as the answer to `request`; `None` when it answers something else.
pub fn reply(request: &[u8], report: &[u8]) -> Option<Reply> {
    if report.len() < 7 || request.len() < 5 {
        return None;
    }
    if (report[0] != SHORT_REPORT && report[0] != LONG_REPORT) || report[1] != request[1] {
        return None;
    }
    if report[2] == request[2] && report[3] == request[3] {
        // Register replies also echo the sub-register they answer.
        if request[2] == READ_LONG_REGISTER && report[4] != request[4] {
            return None;
        }
        return Some(Reply::Success(report[4..].to_vec()));
    }
    let echoes_request = report[3] == request[2] && report[4] == request[3];
    match report[2] {
        0x8F if echoes_request => Some(Reply::Unreachable(report[5])),
        0xFF if echoes_request => Some(Reply::FeatureError(report[5])),
        _ => None,
    }
}

/// HID++ 2.0 feature IDs used here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    DeviceName = 0x0005,
    BatteryStatus = 0x1000,
    BatteryVoltage = 0x1001,
    UnifiedBattery = 0x1004,
}

impl Feature {
    /// Battery features in order of preference.
    pub const BATTERIES: [Feature; 3] = [
        Feature::UnifiedBattery,
        Feature::BatteryStatus,
        Feature::BatteryVoltage,
    ];

    pub fn id(self) -> u16 {
        self as u16
    }

    /// Function that returns the current battery state.
    pub fn status_function(self) -> u8 {
        if self == Feature::UnifiedBattery {
            1
        } else {
            0
        }
    }
}

/// A decoded HID++ battery state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatteryState {
    pub level: u8,
    pub charge: ChargeState,
}

/// Decodes a battery reply of `feature`.
pub fn decode_battery(feature: Feature, params: &[u8]) -> BatteryState {
    match feature {
        Feature::UnifiedBattery => decode_unified(params),
        Feature::BatteryVoltage => decode_voltage(params),
        _ => decode_status(params),
    }
}

/// UNIFIED_BATTERY (0x1004) get_status: state of charge, level flags, charging status.
pub fn decode_unified(params: &[u8]) -> BatteryState {
    let charge = match byte_at(params, 2) {
        0 => ChargeState::Discharging,
        1 | 2 => ChargeState::Charging,
        3 => ChargeState::Full,
        _ => ChargeState::Unknown,
    };
    let state_of_charge = byte_at(params, 0);
    let level = if state_of_charge > 0 {
        state_of_charge
    } else {
        approximate_level(byte_at(params, 1))
    };
    BatteryState {
        level: if charge == ChargeState::Full {
            100
        } else {
            level
        },
        charge,
    }
}

/// BATTERY_STATUS (0x1000) getBatteryLevelStatus: level, next level, status.
pub fn decode_status(params: &[u8]) -> BatteryState {
    let charge = match byte_at(params, 2) {
        0 => ChargeState::Discharging,
        1 | 2 | 4 => ChargeState::Charging,
        3 => ChargeState::Full,
        _ => ChargeState::Unknown,
    };
    let level = byte_at(params, 0);
    BatteryState {
        level: if charge == ChargeState::Full {
            100
        } else {
            level
        },
        charge,
    }
}

/// BATTERY_VOLTAGE (0x1001) getBatteryInfo: millivolts (big-endian) and flags.
pub fn decode_voltage(params: &[u8]) -> BatteryState {
    let millivolts = u16::from_be_bytes([byte_at(params, 0), byte_at(params, 1)]);
    let flags = byte_at(params, 2);
    let charge = match (flags & 0x80 != 0, flags & 0x03) {
        (false, _) => ChargeState::Discharging,
        (true, 1) => ChargeState::Full,
        (true, _) => ChargeState::Charging,
    };
    let level = if charge == ChargeState::Full {
        100
    } else {
        percent_for_millivolts(millivolts)
    };
    BatteryState { level, charge }
}

/// Remaining charge of a single Li-ion cell (Solaar's table).
pub fn percent_for_millivolts(millivolts: u16) -> u8 {
    const CURVE: [(u16, u8); 13] = [
        (4186, 100),
        (4067, 90),
        (3989, 80),
        (3922, 70),
        (3859, 60),
        (3811, 50),
        (3778, 40),
        (3751, 30),
        (3717, 20),
        (3671, 10),
        (3646, 5),
        (3579, 2),
        (3500, 0),
    ];
    CURVE
        .iter()
        .find(|(threshold, _)| *threshold < millivolts)
        .map_or(0, |(_, percent)| *percent)
}

/// Devices that only report coarse levels: full / good / low / critical.
fn approximate_level(flags: u8) -> u8 {
    if flags & 0x08 != 0 {
        90
    } else if flags & 0x04 != 0 {
        50
    } else if flags & 0x02 != 0 {
        20
    } else if flags & 0x01 != 0 {
        5
    } else {
        0
    }
}

/// Receiver families differ in how they list paired devices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiverKind {
    Bolt,
    Unifying,
    Other,
}

impl ReceiverKind {
    pub fn from_product_id(product_id: u16) -> Self {
        match product_id {
            0xC548 => ReceiverKind::Bolt,
            0xC52B | 0xC532 => ReceiverKind::Unifying,
            _ => ReceiverKind::Other,
        }
    }

    pub fn source_name(self) -> &'static str {
        match self {
            ReceiverKind::Bolt => "Logi Bolt",
            ReceiverKind::Unifying => "Logi Unifying",
            ReceiverKind::Other => "Logitech",
        }
    }
}

/// A device paired with a receiver, as listed by its pairing registers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairedSlot {
    pub index: u8,
    pub kind: DeviceKind,
    pub serial: Option<String>,
    pub codename: Option<String>,
}

const RECEIVER_INFO: u8 = 0xB5;

/// Bolt: 0x5N. Unifying: 0x2(N-1). Other receivers do not expose it.
pub fn pairing_request(slot: u8, receiver: ReceiverKind) -> Option<Vec<u8>> {
    match receiver {
        ReceiverKind::Bolt => Some(register_read(RECEIVER_INFO, &[0x50 + slot, 0x00])),
        ReceiverKind::Unifying => Some(register_read(RECEIVER_INFO, &[0x20 + slot - 1, 0x00])),
        ReceiverKind::Other => None,
    }
}

/// Unifying keeps the serial in the extended pairing register, 0x3(N-1).
pub fn extended_pairing_request(slot: u8) -> Vec<u8> {
    register_read(RECEIVER_INFO, &[0x30 + slot - 1, 0x00])
}

/// Bolt: 0x6N with parameter 1. Unifying: 0x4(N-1).
pub fn name_request(slot: u8, receiver: ReceiverKind) -> Option<Vec<u8>> {
    match receiver {
        ReceiverKind::Bolt => Some(register_read(RECEIVER_INFO, &[0x60 + slot, 0x01])),
        ReceiverKind::Unifying => Some(register_read(RECEIVER_INFO, &[0x40 + slot - 1, 0x00])),
        ReceiverKind::Other => None,
    }
}

/// Parses pairing info; `params` start with the echoed sub-register.
pub fn parse_pairing(
    params: &[u8],
    slot: u8,
    receiver: ReceiverKind,
    extended: Option<&[u8]>,
) -> Option<PairedSlot> {
    if params.len() < 8 {
        return None;
    }
    match receiver {
        ReceiverKind::Bolt => Some(PairedSlot {
            index: slot,
            kind: kind_from_pairing_nibble(params[1] & 0x0F),
            serial: Some(hex_upper(&params[4..8])),
            codename: None,
        }),
        ReceiverKind::Unifying => Some(PairedSlot {
            index: slot,
            kind: kind_from_pairing_nibble(params[7] & 0x0F),
            serial: extended
                .filter(|bytes| bytes.len() >= 5)
                .map(|bytes| hex_upper(&bytes[1..5])),
            codename: None,
        }),
        ReceiverKind::Other => None,
    }
}

pub fn parse_codename(params: &[u8], receiver: ReceiverKind) -> Option<String> {
    let (length_index, max_length) = if receiver == ReceiverKind::Bolt {
        (2, 14)
    } else {
        (1, 16)
    };
    let start = length_index + 1;
    let declared = usize::from(*params.get(length_index)?);
    let length = declared
        .min(max_length)
        .min(params.len().saturating_sub(start));
    if length == 0 {
        return None;
    }
    let name = String::from_utf8_lossy(&params[start..start + length])
        .trim_matches(|c: char| c.is_whitespace() || c.is_control())
        .to_owned();
    (!name.is_empty()).then_some(name)
}

/// HID++ 1.0 device kind nibble from pairing info or connection notifications.
pub fn kind_from_pairing_nibble(nibble: u8) -> DeviceKind {
    match nibble {
        0x01 | 0x03 => DeviceKind::Keyboard,
        0x02 | 0x08 => DeviceKind::Mouse,
        0x09 => DeviceKind::Trackpad,
        0x0B | 0x0C => DeviceKind::Gamepad,
        0x0D => DeviceKind::Headphones,
        _ => DeviceKind::Other,
    }
}

/// HID++ 2.0 DEVICE_NAME getDeviceType code.
pub fn kind_from_device_type(device_type: u8) -> DeviceKind {
    match device_type {
        0 | 2 => DeviceKind::Keyboard,
        3 | 5 => DeviceKind::Mouse,
        4 => DeviceKind::Trackpad,
        8 => DeviceKind::Headphones,
        11 | 12 => DeviceKind::Gamepad,
        14 => DeviceKind::Speaker,
        _ => DeviceKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::channel::testing::report;

    #[test]
    fn feature_call_is_a_short_report_tagged_with_our_software_id() {
        assert_eq!(
            request(1, 0, 1, &[0, 0, 0x5A]),
            vec![0x10, 0x01, 0x00, 0x1B, 0x00, 0x00, 0x5A]
        );
    }

    #[test]
    fn register_read_goes_to_the_receiver() {
        assert_eq!(
            register_read(0xB5, &[0x51, 0x00]),
            vec![0x10, 0xFF, 0x83, 0xB5, 0x51, 0x00, 0x00]
        );
    }

    #[test]
    fn classifies_success_unreachable_and_feature_errors() {
        let ping = request(1, 0, 1, &[]);
        let success = report(&[0x11, 0x01, 0x00, 0x1B, 0x04, 0x05, 0x5A], 20);
        assert_eq!(
            reply(&ping, &success),
            Some(Reply::Success(success[4..].to_vec()))
        );
        // Captured from a sleeping MX Keys S behind a Bolt receiver.
        assert_eq!(
            reply(&ping, &[0x10, 0x01, 0x8F, 0x00, 0x1B, 0x04, 0x00]),
            Some(Reply::Unreachable(0x04))
        );
        let battery = request(2, 0x08, 1, &[]);
        assert_eq!(
            reply(&battery, &report(&[0x11, 0x02, 0xFF, 0x08, 0x1B, 0x07], 20)),
            Some(Reply::FeatureError(0x07))
        );
    }

    #[test]
    fn matches_register_replies_on_the_echoed_sub_register() {
        let pairing = register_read(0xB5, &[0x51, 0x00]);
        let capture = [
            0x11, 0xFF, 0x83, 0xB5, 0x51, 0x41, 0x80, 0xB3, 0x1A, 0x2B, 0x3C, 0x4D, 0x01, 0x00,
            0x80, 0x14, 0x01, 0x00, 0x00, 0x00,
        ];
        assert_eq!(
            reply(&pairing, &capture),
            Some(Reply::Success(capture[4..].to_vec()))
        );
        let mut other = capture;
        other[4] = 0x52;
        assert_eq!(reply(&pairing, &other), None);
        assert_eq!(
            reply(&pairing, &[0x10, 0xFF, 0x8F, 0x83, 0xB5, 0x08, 0x00]),
            Some(Reply::Unreachable(0x08))
        );
    }

    #[test]
    fn ignores_reports_for_other_devices_functions_or_report_ids() {
        let battery = request(1, 0x08, 1, &[]);
        assert_eq!(
            reply(&battery, &report(&[0x11, 0x02, 0x08, 0x1B], 20)),
            None
        );
        assert_eq!(
            reply(&battery, &report(&[0x11, 0x01, 0x08, 0x10], 20)),
            None
        );
        assert_eq!(
            reply(&battery, &report(&[0x20, 0x01, 0x08, 0x1B], 20)),
            None
        );
        assert_eq!(reply(&battery, &[0x10, 0x01]), None);
    }

    #[test]
    fn decodes_the_three_battery_features() {
        assert_eq!(
            decode_unified(&[0x4F, 0x04, 0x00, 0x00]),
            BatteryState {
                level: 79,
                charge: ChargeState::Discharging
            }
        );
        assert_eq!(
            decode_unified(&[0x30, 0x04, 0x01, 0x01]),
            BatteryState {
                level: 48,
                charge: ChargeState::Charging
            }
        );
        assert_eq!(
            decode_unified(&[0x64, 0x08, 0x03, 0x01]),
            BatteryState {
                level: 100,
                charge: ChargeState::Full
            }
        );
        for (flags, level) in [(0x08, 90), (0x04, 50), (0x02, 20), (0x01, 5)] {
            assert_eq!(decode_unified(&[0x00, flags, 0x00, 0x00]).level, level);
        }
        assert_eq!(
            decode_status(&[0x32, 0x1E, 0x01]),
            BatteryState {
                level: 50,
                charge: ChargeState::Charging
            }
        );
        assert_eq!(
            decode_status(&[0x64, 0x00, 0x03]),
            BatteryState {
                level: 100,
                charge: ChargeState::Full
            }
        );
        assert_eq!(
            decode_status(&[0x0A, 0x05, 0x06]).charge,
            ChargeState::Unknown
        );
        assert_eq!(
            decode_voltage(&[0x0F, 0x50, 0x00]),
            BatteryState {
                level: 60,
                charge: ChargeState::Discharging
            }
        );
        assert_eq!(
            decode_voltage(&[0x10, 0x04, 0x80]),
            BatteryState {
                level: 90,
                charge: ChargeState::Charging
            }
        );
        assert_eq!(
            decode_voltage(&[0x10, 0x68, 0x81]),
            BatteryState {
                level: 100,
                charge: ChargeState::Full
            }
        );
        assert_eq!(decode_voltage(&[0x0D, 0x00, 0x00]).level, 0);
    }

    #[test]
    fn parses_bolt_and_unifying_pairing_registers() {
        let bolt = [
            0x51, 0x41, 0x80, 0xB3, 0x1A, 0x2B, 0x3C, 0x4D, 0x01, 0x00, 0x80, 0x14,
        ];
        assert_eq!(
            parse_pairing(&bolt, 1, ReceiverKind::Bolt, None),
            Some(PairedSlot {
                index: 1,
                kind: DeviceKind::Keyboard,
                serial: Some("1A2B3C4D".into()),
                codename: None
            })
        );
        let name = [
            0x61, 0x01, 0x09, b'M', b'X', b' ', b'K', b'E', b'Y', b'S', b' ', b'B', 0, 0, 0, 0,
        ];
        assert_eq!(
            parse_codename(&name, ReceiverKind::Bolt).as_deref(),
            Some("MX KEYS B")
        );

        let unifying = [
            0x20, 0x00, 0x08, 0x40, 0x82, 0x04, 0x00, 0x02, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let extended = [0x30, 0x12, 0x34, 0x56, 0x78, 0, 0, 0];
        assert_eq!(
            parse_pairing(&unifying, 1, ReceiverKind::Unifying, Some(&extended)),
            Some(PairedSlot {
                index: 1,
                kind: DeviceKind::Mouse,
                serial: Some("12345678".into()),
                codename: None
            })
        );
        let unifying_name = [0x40, 0x04, b'M', b'X', b'3', b'S', 0, 0];
        assert_eq!(
            parse_codename(&unifying_name, ReceiverKind::Unifying).as_deref(),
            Some("MX3S")
        );
    }

    #[test]
    fn register_addresses_differ_between_bolt_and_unifying() {
        assert_eq!(
            pairing_request(2, ReceiverKind::Bolt),
            Some(register_read(0xB5, &[0x52, 0x00]))
        );
        assert_eq!(
            name_request(2, ReceiverKind::Bolt),
            Some(register_read(0xB5, &[0x62, 0x01]))
        );
        assert_eq!(
            pairing_request(2, ReceiverKind::Unifying),
            Some(register_read(0xB5, &[0x21, 0x00]))
        );
        assert_eq!(
            name_request(2, ReceiverKind::Unifying),
            Some(register_read(0xB5, &[0x41, 0x00]))
        );
        assert_eq!(pairing_request(2, ReceiverKind::Other), None);
        assert_eq!(ReceiverKind::from_product_id(0xC548), ReceiverKind::Bolt);
        assert_eq!(ReceiverKind::from_product_id(0xC539), ReceiverKind::Other);
        assert_eq!(kind_from_device_type(3), DeviceKind::Mouse);
        assert_eq!(kind_from_pairing_nibble(0x09), DeviceKind::Trackpad);
    }
}
