//! Bluetooth batteries Windows already knows, read from device properties.

use std::collections::HashSet;
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
use crate::common::model::DeviceReading;

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

/// Connected Bluetooth devices that report a battery level.
pub fn read(now: u64) -> Vec<DeviceReading> {
    let nodes = nodes();
    let names: Vec<(String, String)> = nodes
        .iter()
        .filter(|node| node.is_device_node() && !node.name.is_empty())
        .filter_map(|node| {
            Some((
                address_from_instance_id(&node.instance_id)?,
                node.name.clone(),
            ))
        })
        .collect();
    let disconnected: HashSet<String> = nodes
        .iter()
        .filter(|node| node.connected == Some(false))
        .filter_map(|node| address_from_instance_id(&node.instance_id))
        .collect();
    let batteries: Vec<BatteryNode> = nodes
        .iter()
        .filter_map(|node| {
            let level = node.battery?;
            let address = address_from_instance_id(&node.instance_id)?;
            (!disconnected.contains(&address)).then(|| BatteryNode {
                instance_id: node.instance_id.clone(),
                name: node.name.clone(),
                level: level.min(100),
            })
        })
        .collect();
    bluetooth::readings(&batteries, &names, now)
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
