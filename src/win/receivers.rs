//! Recognizes supported receivers among the HID collections and builds a reader for each.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::hid::{Collection, HidChannel};
use crate::common::asus::{self, AsusOmniReader};
use crate::common::channel::{BatteryReader, ChannelError, ReportChannel};
use crate::common::compx::{self, CompxReader};
use crate::common::hid_path::{instance_label, interface_key};
use crate::common::hidpp::{self, ReceiverKind};
use crate::common::logitech::LogitechReader;
use crate::common::model::DeviceReading;
use crate::common::strings::Strings;

/// One receiver interface and the reader that speaks its protocol.
pub struct Session {
    /// Source key in the reading store; equal for all collections of the interface.
    pub key: String,
    pub description: String,
    channel: Arc<Mutex<LazyChannel>>,
    reader: Box<dyn BatteryReader>,
}

impl Session {
    /// Reads the devices behind the receiver; its collections are open only meanwhile.
    pub fn read(&mut self, now: u64) -> Vec<DeviceReading> {
        let readings = self.reader.read(now);
        if let Ok(mut channel) = self.channel.lock() {
            channel.close();
        }
        readings
    }
}

/// Opens the collections on first use and closes them after each read, so nothing stays
/// open (or buffers input) between refreshes.
struct LazyChannel {
    routes: Vec<(u8, Collection)>,
    open: Option<HidChannel>,
}

impl LazyChannel {
    fn close(&mut self) {
        self.open = None;
    }
}

impl ReportChannel for LazyChannel {
    fn exchange(
        &mut self,
        request: &[u8],
        timeout: Duration,
        accept: &dyn Fn(&[u8]) -> bool,
    ) -> Result<Vec<u8>, ChannelError> {
        if self.open.is_none() {
            self.open = Some(HidChannel::open(&self.routes)?);
        }
        let channel = self.open.as_mut().ok_or(ChannelError::Closed)?;
        channel.exchange(request, timeout, accept)
    }
}

/// The reader's handle on the shared lazy channel.
struct SharedChannel(Arc<Mutex<LazyChannel>>);

impl ReportChannel for SharedChannel {
    fn exchange(
        &mut self,
        request: &[u8],
        timeout: Duration,
        accept: &dyn Fn(&[u8]) -> bool,
    ) -> Result<Vec<u8>, ChannelError> {
        let mut channel = self.0.lock().map_err(|_| ChannelError::Closed)?;
        channel.exchange(request, timeout, accept)
    }
}

/// Builds a session for every supported receiver interface among `collections`.
pub fn discover(collections: &[Collection], strings: Strings) -> Vec<Session> {
    let mut interfaces: BTreeMap<String, Vec<&Collection>> = BTreeMap::new();
    for collection in collections {
        interfaces
            .entry(interface_key(&collection.path))
            .or_default()
            .push(collection);
    }
    interfaces
        .into_iter()
        .filter_map(|(key, group)| session(&key, &group, strings))
        .collect()
}

fn session(key: &str, group: &[&Collection], strings: Strings) -> Option<Session> {
    let first = group.first()?;
    let (routes, description, build): (Vec<(u8, Collection)>, String, ReaderBuilder) =
        match first.vendor_id {
            hidpp::VENDOR_ID if (0xC500..=0xC5FF).contains(&first.product_id) => {
                let short = find(group, hidpp::USAGE_PAGE, 0x0001)?;
                let long = find(group, hidpp::USAGE_PAGE, 0x0002)?;
                let receiver = ReceiverKind::from_product_id(first.product_id);
                let id = stable_id(first);
                (
                    vec![
                        (hidpp::SHORT_REPORT, short.clone()),
                        (hidpp::LONG_REPORT, long.clone()),
                    ],
                    format!(
                        "{} receiver {:04X}:{:04X}",
                        receiver.source_name(),
                        first.vendor_id,
                        first.product_id
                    ),
                    Box::new(move |channel| {
                        Box::new(LogitechReader::new(channel, receiver, id, strings))
                    }),
                )
            }
            asus::VENDOR_ID if first.product_id == asus::RECEIVER_PRODUCT_ID => {
                let routes: Vec<(u8, Collection)> = asus::COLLECTIONS
                    .iter()
                    .filter_map(|(page, report_id)| {
                        group
                            .iter()
                            .find(|c| c.usage_page == *page && c.output_length > 2)
                            .map(|c| (*report_id, (*c).clone()))
                    })
                    .collect();
                routes.iter().find(|(report_id, _)| *report_id == 0x01)?;
                let id = stable_id(first);
                (
                    routes,
                    format!(
                        "ROG Omni receiver {:04X}:{:04X}",
                        first.vendor_id, first.product_id
                    ),
                    Box::new(move |channel| Box::new(AsusOmniReader::new(channel, id, strings))),
                )
            }
            vendor if compx::VENDOR_IDS.contains(&vendor) => {
                let collection = group
                    .iter()
                    .filter(|c| {
                        compx::is_battery_collection(
                            c.vendor_id,
                            c.product_id,
                            c.usage_page,
                            c.output_length,
                            &c.product,
                        )
                    })
                    .min_by_key(|c| (c.output_length != compx::REQUEST_LENGTH, c.output_length))?;
                let id = format!("compx.{}", stable_id(collection));
                let name = compx::display_name(&collection.product, &strings);
                (
                    vec![(0x08, (*collection).clone())],
                    format!(
                        "Compx dongle {:04X}:{:04X} \"{}\"",
                        collection.vendor_id, collection.product_id, collection.product
                    ),
                    Box::new(move |channel| Box::new(CompxReader::new(channel, id, name))),
                )
            }
            _ => return None,
        };
    let channel = Arc::new(Mutex::new(LazyChannel { routes, open: None }));
    let reader = build(Box::new(SharedChannel(Arc::clone(&channel))));
    Some(Session {
        key: format!("hid.{key}"),
        description,
        channel,
        reader,
    })
}

type ReaderBuilder = Box<dyn FnOnce(Box<dyn ReportChannel>) -> Box<dyn BatteryReader>>;

fn find<'a>(group: &[&'a Collection], usage_page: u16, usage: u16) -> Option<&'a Collection> {
    group
        .iter()
        .copied()
        .find(|c| c.usage_page == usage_page && c.usage == usage && c.output_length > 0)
}

/// The USB serial when there is one, otherwise the interface instance (stable per port).
fn stable_id(collection: &Collection) -> String {
    if collection.serial.is_empty() {
        instance_label(&collection.path)
    } else {
        collection.serial.clone()
    }
}
