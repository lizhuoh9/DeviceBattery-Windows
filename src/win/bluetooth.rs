//! Bluetooth batteries Windows already knows, read from device properties.

use std::collections::HashMap;
use std::mem::size_of;

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_ALLCLASSES, DIGCF_PRESENT, HDEVINFO, SP_DEVINFO_DATA, SetupDiDestroyDeviceInfoList,
    SetupDiEnumDeviceInfo, SetupDiGetClassDevsW, SetupDiGetDeviceInstanceIdW,
    SetupDiGetDevicePropertyW,
};
use windows::Win32::Devices::Properties::{
    DEVPKEY_Device_FriendlyName, DEVPKEY_NAME, DEVPROP_TYPE_BOOLEAN, DEVPROP_TYPE_BYTE,
    DEVPROP_TYPE_STRING, DEVPROPTYPE,
};
use windows::Win32::Foundation::DEVPROPKEY;
use windows::core::{GUID, PCWSTR};

use super::text::{from_wide, wide};
use crate::common::bluetooth::{self, BatteryNode, address_from_instance_id};
use crate::common::model::{DeviceReading, LinkStatus};

/// Battery level in percent, as shown in Settings → Bluetooth & devices.
const BATTERY: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x104ea319_6ee2_4701_bd47_8ddbf425bbe5),
    pid: 2,
};
/// Whether the device is currently connected.
const IS_CONNECTED: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x83da6326_97a6_4088_9453_a1923f573b29),
    pid: 15,
};

/// A Bluetooth device node and the properties we use.
#[derive(Clone, Debug)]
pub struct Node {
    pub instance_id: String,
    pub name: String,
    pub battery: Option<u8>,
    pub connected: Option<bool>,
}

impl Node {
    /// The node that stands for the device itself (not one of its services).
    fn is_device_node(&self) -> bool {
        let upper = self.instance_id.to_ascii_uppercase();
        upper.starts_with(r"BTHENUM\DEV_") || upper.starts_with(r"BTHLE\DEV_")
    }
}

/// Bluetooth devices that report a battery level. A connected device is online; a device that
/// is paired but not connected right now is asleep, so the widget still shows the level Windows
/// kept for it.
pub fn read(now: u64) -> Vec<DeviceReading> {
    let nodes = nodes();
    let names = device_names(&nodes);
    bluetooth::readings(&battery_nodes(&nodes), &names, now)
}

/// Friendly names of the devices' main nodes, keyed by Bluetooth address.
fn device_names(nodes: &[Node]) -> Vec<(String, String)> {
    nodes
        .iter()
        .filter(|node| node.is_device_node() && !node.name.is_empty())
        .filter_map(|node| {
            Some((
                address_from_instance_id(&node.instance_id)?,
                node.name.clone(),
            ))
        })
        .collect()
}

/// One entry per node that carries a battery level, asleep when its address is not connected.
fn battery_nodes(nodes: &[Node]) -> Vec<BatteryNode> {
    let connected = connectivity(nodes);
    nodes
        .iter()
        .filter_map(|node| {
            let level = node.battery?;
            let address = address_from_instance_id(&node.instance_id)?;
            let status = match connected.get(&address) {
                Some(false) => LinkStatus::Asleep,
                _ => LinkStatus::Online,
            };
            Some(BatteryNode {
                instance_id: node.instance_id.clone(),
                name: node.name.clone(),
                level: level.min(100),
                status,
            })
        })
        .collect()
}

/// Whether each address is connected. A device has several nodes and only some of them report
/// the state, so one connected node is enough to call the whole device connected: an address
/// counts as disconnected only when every node that reports a state says so.
fn connectivity(nodes: &[Node]) -> HashMap<String, bool> {
    let mut states: HashMap<String, bool> = HashMap::new();
    for node in nodes {
        let (Some(address), Some(state)) = (
            address_from_instance_id(&node.instance_id),
            node.connected,
        ) else {
            continue;
        };
        states
            .entry(address)
            .and_modify(|known| *known |= state)
            .or_insert(state);
    }
    states
}

/// Every present Bluetooth device node (classic and Low Energy).
pub fn nodes() -> Vec<Node> {
    let mut nodes = Vec::new();
    for enumerator in ["BTHENUM", "BTHLE"] {
        let name = wide(enumerator);
        // SAFETY: the enumerator string is NUL-terminated; the set is destroyed below.
        let Ok(set) = (unsafe {
            SetupDiGetClassDevsW(
                None,
                PCWSTR(name.as_ptr()),
                None,
                DIGCF_ALLCLASSES | DIGCF_PRESENT,
            )
        }) else {
            continue;
        };
        for index in 0u32.. {
            let mut info = SP_DEVINFO_DATA {
                cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
                ..Default::default()
            };
            // SAFETY: `info` is a properly sized out-parameter.
            if unsafe { SetupDiEnumDeviceInfo(set, index, &mut info) }.is_err() {
                break;
            }
            let friendly = string_property(set, &info, &DEVPKEY_Device_FriendlyName);
            nodes.push(Node {
                instance_id: instance_id(set, &info),
                name: friendly
                    .or_else(|| string_property(set, &info, &DEVPKEY_NAME))
                    .unwrap_or_default(),
                battery: property(set, &info, &BATTERY, DEVPROP_TYPE_BYTE)
                    .and_then(|bytes| bytes.first().copied()),
                connected: property(set, &info, &IS_CONNECTED, DEVPROP_TYPE_BOOLEAN)
                    .and_then(|bytes| bytes.first().map(|value| *value != 0)),
            });
        }
        // SAFETY: the set came from SetupDiGetClassDevsW above.
        unsafe {
            let _ = SetupDiDestroyDeviceInfoList(set);
        }
    }
    nodes
}

fn instance_id(set: HDEVINFO, info: &SP_DEVINFO_DATA) -> String {
    let mut buffer = [0u16; 512];
    // SAFETY: the buffer is large enough for any device instance ID (max 200 characters).
    let ok = unsafe { SetupDiGetDeviceInstanceIdW(set, info, Some(&mut buffer), None) }.is_ok();
    if ok {
        from_wide(&buffer)
    } else {
        String::new()
    }
}

/// The raw bytes of a property, when it exists with the expected type.
fn property(
    set: HDEVINFO,
    info: &SP_DEVINFO_DATA,
    key: &DEVPROPKEY,
    expected: DEVPROPTYPE,
) -> Option<Vec<u8>> {
    let mut kind = DEVPROPTYPE::default();
    let mut buffer = vec![0u8; 512];
    let mut required = 0u32;
    // SAFETY: all out-parameters point at live locals; the buffer length is passed implicitly.
    let ok = unsafe {
        SetupDiGetDevicePropertyW(
            set,
            info,
            key,
            &mut kind,
            Some(&mut buffer),
            Some(&mut required),
            0,
        )
    }
    .is_ok();
    if !ok || kind != expected {
        return None;
    }
    buffer.truncate((required as usize).min(buffer.len()));
    Some(buffer)
}

fn string_property(set: HDEVINFO, info: &SP_DEVINFO_DATA, key: &DEVPROPKEY) -> Option<String> {
    let bytes = property(set, info, key, DEVPROP_TYPE_STRING)?;
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let text = from_wide(&units).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE_NODE: &str = r"BTHENUM\DEV_AABBCCDDEE10\7&1&0&BLUETOOTHDEVICE_AABBCCDDEE10";
    const HANDS_FREE: &str =
        r"BTHENUM\{0000111E-0000-1000-8000-00805F9B34FB}_LOCALMFG&004C\7&1&0&AABBCCDDEE10_C00000000";
    const LOW_ENERGY: &str = r"BTHLE\DEV_AABBCCDDEE30\7&1&0&AABBCCDDEE30";

    fn node(instance_id: &str, battery: Option<u8>, connected: Option<bool>) -> Node {
        Node {
            instance_id: instance_id.into(),
            name: "Galaxy Buds2".into(),
            battery,
            connected,
        }
    }

    fn paired_but_disconnected() -> Vec<Node> {
        vec![
            node(DEVICE_NODE, None, Some(false)),
            node(HANDS_FREE, Some(40), Some(false)),
            node(LOW_ENERGY, Some(85), Some(true)),
        ]
    }

    #[test]
    fn a_paired_but_disconnected_device_is_listed_as_asleep() {
        let batteries = battery_nodes(&paired_but_disconnected());
        assert_eq!(batteries.len(), 2);
        assert_eq!(
            (
                batteries[0].level,
                batteries[0].status,
                batteries[1].level,
                batteries[1].status
            ),
            (40, LinkStatus::Asleep, 85, LinkStatus::Online)
        );
    }

    #[test]
    fn one_connected_node_makes_the_whole_device_connected() {
        let nodes = vec![
            node(DEVICE_NODE, None, Some(false)),
            node(HANDS_FREE, Some(40), Some(true)),
        ];
        assert_eq!(battery_nodes(&nodes)[0].status, LinkStatus::Online);
    }

    #[test]
    fn a_device_that_reports_no_state_is_online() {
        let nodes = vec![node(LOW_ENERGY, Some(85), None)];
        assert_eq!(battery_nodes(&nodes)[0].status, LinkStatus::Online);
    }

    #[test]
    fn names_come_from_device_nodes_only() {
        // A service node of another device carries no name of its own, whatever it is called.
        let nodes = vec![
            node(DEVICE_NODE, None, Some(true)),
            node(
                r"BTHENUM\{0000111E-0000-1000-8000-00805F9B34FB}_LOCALMFG&004C\7&1&0&AABBCCDDEE99_C00000000",
                Some(40),
                Some(true),
            ),
        ];
        assert_eq!(
            device_names(&nodes),
            vec![("AABBCCDDEE10".to_owned(), "Galaxy Buds2".to_owned())]
        );
    }
}
