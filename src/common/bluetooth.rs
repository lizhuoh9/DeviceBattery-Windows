//! Turns what Windows knows about Bluetooth devices into readings.
//!
//! Windows publishes a device's battery as the property `{104EA319-…} 2` on one of its device
//! nodes (the Hands-Free node for classic headsets, the device node for Bluetooth LE). A
//! device has several nodes, so readings are keyed by the Bluetooth address found in the
//! node's instance ID, and service suffixes are trimmed from the node name.

use super::model::{ChargeState, DeviceKind, DeviceReading, LinkStatus};
use super::strings::BLUETOOTH_SOURCE;

/// A device node that carries a battery level, and whether the device is connected right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatteryNode {
    pub instance_id: String,
    pub name: String,
    pub level: u8,
    pub status: LinkStatus,
}

/// The Bluetooth address in a device instance ID: its last run of exactly 12 hex digits,
/// e.g. `BTHENUM\{0000111E-…}_LOCALMFG&004C\7&2B8E&0&AABBCCDDEE10_C00000000`.
pub fn address_from_instance_id(instance_id: &str) -> Option<String> {
    instance_id
        .split(|c: char| !c.is_ascii_hexdigit())
        .rfind(|run| run.len() == 12)
        .map(str::to_ascii_uppercase)
}

/// The device name without the service a node stands for ("… Hands-Free", "… Stereo").
pub fn clean_name(name: &str) -> String {
    const SUFFIXES: [&str; 6] = [
        " Hands-Free AG Audio",
        " Hands-Free AG",
        " Hands-Free",
        " Stereo",
        " Avrcp Transport",
        " LE",
    ];
    let trimmed = name.trim();
    SUFFIXES
        .iter()
        .find_map(|suffix| trimmed.strip_suffix(suffix))
        .unwrap_or(trimmed)
        .trim()
        .to_owned()
}

/// A best guess of the device class from its name.
pub fn kind_from_name(name: &str) -> DeviceKind {
    let lower = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| lower.contains(word));
    if has(&["mouse", "mx master", "mx anywhere"]) {
        DeviceKind::Mouse
    } else if has(&["keyboard", "keys"]) {
        DeviceKind::Keyboard
    } else if has(&["trackpad", "touchpad"]) {
        DeviceKind::Trackpad
    } else if has(&[
        "airpods",
        "buds",
        "headphone",
        "headset",
        "earphone",
        "wh-",
        "wf-",
        "beats",
    ]) {
        DeviceKind::Headphones
    } else if has(&[
        "controller",
        "gamepad",
        "xbox",
        "dualsense",
        "dualshock",
        "joy-con",
    ]) {
        DeviceKind::Gamepad
    } else if has(&["speaker", "soundlink", "boom"]) {
        DeviceKind::Speaker
    } else {
        DeviceKind::Other
    }
}

/// One reading per Bluetooth address, carrying the status its node was given. `names` are
/// friendly names of the devices' main nodes, keyed by address; nodes without an address are
/// skipped.
pub fn readings(nodes: &[BatteryNode], names: &[(String, String)], now: u64) -> Vec<DeviceReading> {
    let mut readings: Vec<DeviceReading> = Vec::new();
    for node in nodes {
        let Some(address) = address_from_instance_id(&node.instance_id) else {
            continue;
        };
        let id = format!("bt.{address}");
        if readings.iter().any(|reading| reading.id == id) {
            continue;
        }
        let name = names
            .iter()
            .find(|(candidate, _)| *candidate == address)
            .map(|(_, name)| clean_name(name))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| clean_name(&node.name));
        let kind = kind_from_name(&name);
        readings.push(DeviceReading::new(
            id,
            name,
            kind,
            Some(node.level),
            ChargeState::Unknown,
            node.status,
            BLUETOOTH_SOURCE,
            now,
        ));
    }
    readings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_address_in_classic_and_le_instance_ids() {
        let hands_free = r"BTHENUM\{0000111E-0000-1000-8000-00805F9B34FB}_LOCALMFG&004C\7&2B8E7B4E&0&AABBCCDDEE10_C00000000";
        assert_eq!(
            address_from_instance_id(hands_free).as_deref(),
            Some("AABBCCDDEE10")
        );
        let device = r"BTHENUM\DEV_AABBCCDDEE10\7&1A2B3C4D&0&BLUETOOTHDEVICE_AABBCCDDEE10";
        assert_eq!(
            address_from_instance_id(device).as_deref(),
            Some("AABBCCDDEE10")
        );
        let low_energy = r"BTHLE\DEV_aabbccddee30\7&1A2B3C4D&0&aabbccddee30";
        assert_eq!(
            address_from_instance_id(low_energy).as_deref(),
            Some("AABBCCDDEE30")
        );
        assert_eq!(
            address_from_instance_id(r"USB\VID_046D&PID_C548\5&ABC"),
            None
        );
    }

    #[test]
    fn trims_service_suffixes() {
        assert_eq!(
            clean_name("Galaxy Buds2 Hands-Free AG Audio"),
            "Galaxy Buds2"
        );
        assert_eq!(clean_name("WH-1000XM5 Hands-Free"), "WH-1000XM5");
        assert_eq!(clean_name(" MX Master 3S "), "MX Master 3S");
    }

    #[test]
    fn guesses_kinds_from_names() {
        assert_eq!(kind_from_name("MX Master 3S"), DeviceKind::Mouse);
        assert_eq!(kind_from_name("MX Keys S"), DeviceKind::Keyboard);
        assert_eq!(kind_from_name("Galaxy Buds2"), DeviceKind::Headphones);
        assert_eq!(
            kind_from_name("Xbox Wireless Controller"),
            DeviceKind::Gamepad
        );
        assert_eq!(kind_from_name("Pixel Watch"), DeviceKind::Other);
    }

    #[test]
    fn one_reading_per_address_named_after_the_main_node() {
        let nodes = vec![
            BatteryNode {
                instance_id: r"BTHENUM\{0000111E-0000-1000-8000-00805F9B34FB}_LOCALMFG&0075\7&1&0&AABBCCDDEE10_C00000000".into(),
                name: "Galaxy Buds2 Hands-Free AG Audio".into(),
                level: 60,
                status: LinkStatus::Online,
            },
            BatteryNode {
                instance_id: r"BTHENUM\{0000110B-0000-1000-8000-00805F9B34FB}_LOCALMFG&0075\7&1&0&AABBCCDDEE10_C00000000".into(),
                name: "Galaxy Buds2 Stereo".into(),
                level: 60,
                status: LinkStatus::Online,
            },
            BatteryNode { instance_id: r"BTHLE\DEV_AABBCCDDEE30\7&1&0&AABBCCDDEE30".into(), name: "MX Keys S".into(), level: 85, status: LinkStatus::Online },
        ];
        let names = vec![("AABBCCDDEE10".to_owned(), "My Buds".to_owned())];
        let readings = readings(&nodes, &names, 500);
        assert_eq!(readings.len(), 2);
        assert_eq!(
            (
                readings[0].id.as_str(),
                readings[0].name.as_str(),
                readings[0].level
            ),
            ("bt.AABBCCDDEE10", "My Buds", Some(60))
        );
        assert_eq!(
            (readings[1].name.as_str(), readings[1].kind),
            ("MX Keys S", DeviceKind::Keyboard)
        );
        assert_eq!(readings[1].source, BLUETOOTH_SOURCE);
    }

    #[test]
    fn a_reading_carries_the_status_of_its_node() {
        let node = |instance_id: &str, status| BatteryNode {
            instance_id: instance_id.into(),
            name: "Galaxy Buds2".into(),
            level: 40,
            status,
        };
        let nodes = vec![
            node(
                r"BTHENUM\DEV_AABBCCDDEE10\7&1&0&BLUETOOTHDEVICE_AABBCCDDEE10",
                LinkStatus::Asleep,
            ),
            node(r"BTHLE\DEV_AABBCCDDEE30\7&1&0&AABBCCDDEE30", LinkStatus::Online),
        ];
        let readings = readings(&nodes, &[], 500);
        assert_eq!(readings.len(), 2);
        assert_eq!(readings[0].status, LinkStatus::Asleep);
        assert_eq!(readings[0].level, Some(40));
        assert_eq!(readings[1].status, LinkStatus::Online);
    }
}
