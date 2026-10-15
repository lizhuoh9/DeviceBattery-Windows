//! Windows HID device paths.
//!
//! Every top-level collection of a USB interface is its own device, with a path such as
//! `\\?\hid#vid_046d&pid_c548&mi_02&col01#7&2b1f4a1c&0&0000#{4d1e55b2-…}`. Collections of
//! the same interface share everything but the `&colNN` part and the last instance number.

/// A key that is equal for all collections of one USB interface.
pub fn interface_key(path: &str) -> String {
    let lower = path.to_ascii_lowercase();
    let parts: Vec<&str> = lower.split('#').collect();
    let (Some(hardware), Some(instance)) = (parts.get(1), parts.get(2)) else {
        return lower;
    };
    let hardware = match hardware.find("&col") {
        Some(index) => &hardware[..index],
        None => hardware,
    };
    let instance = match instance.rfind('&') {
        Some(index) if hardware != parts[1] => &instance[..index],
        _ => instance,
    };
    format!("{hardware}#{instance}")
}

/// The interface instance part of a path, stable for a receiver in a given USB port.
pub fn instance_label(path: &str) -> String {
    interface_key(path)
        .split('#')
        .nth(1)
        .map(|instance| instance.replace('&', "-"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUID: &str = "{4d1e55b2-f16f-11cf-88cb-001111000030}";

    #[test]
    fn collections_of_one_interface_share_a_key() {
        let short = format!(r"\\?\HID#VID_046D&PID_C548&MI_02&Col01#7&2b1f4a1c&0&0000#{GUID}");
        let long = format!(r"\\?\hid#vid_046d&pid_c548&mi_02&col02#7&2b1f4a1c&0&0001#{GUID}");
        assert_eq!(interface_key(&short), interface_key(&long));
        assert_eq!(
            interface_key(&short),
            "vid_046d&pid_c548&mi_02#7&2b1f4a1c&0"
        );
    }

    #[test]
    fn different_interfaces_or_receivers_differ() {
        let keyboard = format!(r"\\?\hid#vid_046d&pid_c548&mi_00#7&11111111&0&0000#{GUID}");
        let hidpp = format!(r"\\?\hid#vid_046d&pid_c548&mi_02&col01#7&2b1f4a1c&0&0000#{GUID}");
        let other_port = format!(r"\\?\hid#vid_046d&pid_c548&mi_02&col01#7&99999999&0&0000#{GUID}");
        assert_ne!(interface_key(&keyboard), interface_key(&hidpp));
        assert_ne!(interface_key(&hidpp), interface_key(&other_port));
    }

    #[test]
    fn a_single_collection_keeps_its_whole_instance() {
        let path = format!(r"\\?\hid#vid_373b&pid_11d9&mi_00#8&abc&0&0000#{GUID}");
        assert_eq!(interface_key(&path), "vid_373b&pid_11d9&mi_00#8&abc&0&0000");
        assert_eq!(instance_label(&path), "8-abc-0-0000");
    }

    #[test]
    fn unexpected_paths_are_returned_whole() {
        assert_eq!(interface_key("weird"), "weird");
    }
}
