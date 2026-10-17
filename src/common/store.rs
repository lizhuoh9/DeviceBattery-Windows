//! The widget's model: what each source reported last, plus the last known level of every
//! device seen with one.
//!
//! A device that cannot be reached keeps its last known level (and the name and kind learned
//! while it was awake), so a sleeping keyboard still shows "86%, 3 hr ago", even after its
//! receiver was replugged or the app restarted.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use super::model::{DeviceKind, DeviceReading, LinkStatus};

#[derive(Debug, Default)]
pub struct ReadingStore {
    /// Latest batch per source (one receiver, or Bluetooth).
    sources: HashMap<String, Vec<DeviceReading>>,
    /// Latest reading with a level, per device ID.
    memory: HashMap<String, DeviceReading>,
}

impl ReadingStore {
    pub fn new(remembered: Vec<DeviceReading>) -> Self {
        let mut memory: HashMap<String, DeviceReading> = HashMap::new();
        for reading in remembered
            .into_iter()
            .filter(|reading| reading.level.is_some())
        {
            let newer = memory
                .get(&reading.id)
                .is_none_or(|existing| reading.date > existing.date);
            if newer {
                memory.insert(reading.id.clone(), reading);
            }
        }
        Self {
            sources: HashMap::new(),
            memory,
        }
    }

    /// Replaces everything `source` reported before with `readings`.
    ///
    /// A level is remembered together with the moment it was read, so the age shown always
    /// belongs to the level shown. An online device refreshes it every time; an offline one
    /// only the first time it is seen, so a device that is already asleep when the app starts
    /// keeps counting from that first sighting instead of claiming it was read "just now" on
    /// every refresh.
    pub fn apply(&mut self, readings: Vec<DeviceReading>, source: &str) {
        for reading in &readings {
            if reading.level.is_none() {
                continue;
            }
            let first_sighting = !self.memory.contains_key(&reading.id);
            if reading.status == LinkStatus::Online || first_sighting {
                self.memory.insert(reading.id.clone(), reading.clone());
            }
        }
        self.sources.insert(source.to_owned(), readings);
    }

    /// Forgets what `source` reported (its receiver was unplugged); levels stay remembered.
    pub fn remove_source(&mut self, source: &str) {
        self.sources.remove(source);
    }

    /// One row per device: custom names applied, hidden devices removed, sorted by kind,
    /// then name, then ID.
    pub fn rows(
        &self,
        custom_names: &HashMap<String, String>,
        hidden: &HashSet<String>,
    ) -> Vec<DeviceReading> {
        let mut best: HashMap<&str, &DeviceReading> = HashMap::new();
        for reading in self
            .sources
            .values()
            .flatten()
            .filter(|reading| !hidden.contains(&reading.id))
        {
            let chosen = match best.get(reading.id.as_str()) {
                Some(current) => better(current, reading),
                None => reading,
            };
            best.insert(reading.id.as_str(), chosen);
        }
        let mut rows: Vec<DeviceReading> = best
            .values()
            .map(|reading| {
                let shown = merged(reading, self.memory.get(&reading.id));
                match custom_names.get(&shown.id) {
                    Some(name) => shown.renamed(name),
                    None => shown,
                }
            })
            .collect();
        rows.sort_by(display_order);
        rows
    }

    /// Every device worth remembering across launches.
    pub fn memory_snapshot(&self) -> Vec<DeviceReading> {
        self.memory.values().cloned().collect()
    }
}

/// When two sources report the same device, an answering one wins, then the newer.
fn better<'a>(lhs: &'a DeviceReading, rhs: &'a DeviceReading) -> &'a DeviceReading {
    let (left_online, right_online) = (
        lhs.status == LinkStatus::Online,
        rhs.status == LinkStatus::Online,
    );
    if left_online != right_online {
        return if left_online { lhs } else { rhs };
    }
    if rhs.date > lhs.date { rhs } else { lhs }
}

/// An unreachable device shows what we knew, with the newer status and source. The level and its
/// time both come from that remembered reading, so the age shown always belongs to the level
/// shown: a level Windows cached while the app was not running is not preferred over it, because
/// the moment it was read is unknown.
fn merged(reading: &DeviceReading, remembered: Option<&DeviceReading>) -> DeviceReading {
    match remembered {
        Some(remembered) if reading.status != LinkStatus::Online => DeviceReading {
            id: reading.id.clone(),
            name: remembered.name.clone(),
            kind: if remembered.kind == DeviceKind::Other {
                reading.kind
            } else {
                remembered.kind
            },
            level: remembered.level,
            charge: remembered.charge,
            status: reading.status,
            source: reading.source.clone(),
            date: remembered.date,
        },
        _ => reading.clone(),
    }
}

/// The list order: kind, then name, then ID.
pub fn display_order(lhs: &DeviceReading, rhs: &DeviceReading) -> Ordering {
    lhs.kind
        .order()
        .cmp(&rhs.kind.order())
        .then_with(|| lhs.name.to_lowercase().cmp(&rhs.name.to_lowercase()))
        .then_with(|| lhs.id.cmp(&rhs.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::model::ChargeState;

    const EARLIER: u64 = 10_000;
    const LATER: u64 = 20_000;

    fn reading(
        id: &str,
        name: &str,
        kind: DeviceKind,
        level: Option<u8>,
        status: LinkStatus,
        date: u64,
    ) -> DeviceReading {
        DeviceReading::new(
            id,
            name,
            kind,
            level,
            ChargeState::Discharging,
            status,
            "test",
            date,
        )
    }

    fn online(id: &str, level: u8, date: u64) -> DeviceReading {
        reading(
            id,
            "Device",
            DeviceKind::Mouse,
            Some(level),
            LinkStatus::Online,
            date,
        )
    }

    fn rows(store: &ReadingStore) -> Vec<DeviceReading> {
        store.rows(&HashMap::new(), &HashSet::new())
    }

    #[test]
    fn an_asleep_device_keeps_its_last_level_name_and_kind() {
        let mut store = ReadingStore::default();
        store.apply(
            vec![reading(
                "logi.1",
                "MX Keys S",
                DeviceKind::Keyboard,
                Some(86),
                LinkStatus::Online,
                EARLIER,
            )],
            "bolt",
        );
        store.apply(
            vec![reading(
                "logi.1",
                "MX KEYS B",
                DeviceKind::Other,
                None,
                LinkStatus::Asleep,
                LATER,
            )],
            "bolt",
        );
        let row = &rows(&store)[0];
        assert_eq!(
            (row.name.as_str(), row.kind, row.level, row.status, row.date),
            (
                "MX Keys S",
                DeviceKind::Keyboard,
                Some(86),
                LinkStatus::Asleep,
                EARLIER
            )
        );
    }

    #[test]
    fn remembered_readings_fill_in_asleep_devices() {
        let mut store = ReadingStore::new(vec![reading(
            "rog.1",
            "ROG Keyboard",
            DeviceKind::Keyboard,
            Some(70),
            LinkStatus::Online,
            EARLIER,
        )]);
        assert!(rows(&store).is_empty());
        store.apply(
            vec![reading(
                "rog.1",
                "ROG Device",
                DeviceKind::Other,
                None,
                LinkStatus::Asleep,
                LATER,
            )],
            "omni",
        );
        assert_eq!(rows(&store)[0].name, "ROG Keyboard");
        assert_eq!(rows(&store)[0].level, Some(70));
    }

    #[test]
    fn devices_a_source_stops_reporting_disappear() {
        let mut store = ReadingStore::default();
        store.apply(
            vec![online("bt.1", 50, EARLIER), online("bt.2", 60, EARLIER)],
            "bt",
        );
        store.apply(vec![online("compx.1", 70, EARLIER)], "compx");
        store.apply(vec![online("bt.2", 58, LATER)], "bt");
        let ids: HashSet<String> = rows(&store).into_iter().map(|row| row.id).collect();
        assert_eq!(
            ids,
            HashSet::from(["bt.2".to_owned(), "compx.1".to_owned()])
        );
        store.remove_source("compx");
        assert_eq!(rows(&store).len(), 1);
    }

    #[test]
    fn a_removed_receiver_keeps_the_last_level_for_later_reads() {
        let mut store = ReadingStore::new(vec![reading(
            "logi.1",
            "MX Keys S",
            DeviceKind::Keyboard,
            Some(60),
            LinkStatus::Online,
            EARLIER,
        )]);
        store.apply(
            vec![reading(
                "logi.1",
                "MX Keys S",
                DeviceKind::Keyboard,
                Some(80),
                LinkStatus::Online,
                LATER,
            )],
            "hid.1",
        );
        store.remove_source("hid.1");
        assert!(rows(&store).is_empty());
        assert_eq!(store.memory_snapshot()[0].level, Some(80));
        store.apply(
            vec![reading(
                "logi.1",
                "MX KEYS B",
                DeviceKind::Other,
                None,
                LinkStatus::Asleep,
                LATER + 60,
            )],
            "hid.2",
        );
        assert_eq!(rows(&store)[0].level, Some(80));
        assert_eq!(rows(&store)[0].date, LATER);
    }

    #[test]
    fn a_device_seen_by_two_sources_shows_the_best_reading() {
        let mut store = ReadingStore::default();
        store.apply(vec![online("logi.1", 70, LATER)], "a");
        store.apply(
            vec![reading(
                "logi.1",
                "Device",
                DeviceKind::Mouse,
                None,
                LinkStatus::Asleep,
                LATER,
            )],
            "b",
        );
        assert_eq!(rows(&store)[0].status, LinkStatus::Online);
        store.remove_source("a");
        assert_eq!(rows(&store)[0].status, LinkStatus::Asleep);
        assert_eq!(rows(&store)[0].level, Some(70));
        store.remove_source("b");
        assert!(rows(&store).is_empty());
    }

    #[test]
    fn rows_apply_names_hide_devices_and_sort() {
        let mut store = ReadingStore::default();
        store.apply(
            vec![
                reading(
                    "k",
                    "Keyboard",
                    DeviceKind::Keyboard,
                    Some(50),
                    LinkStatus::Online,
                    EARLIER,
                ),
                reading(
                    "m2",
                    "b mouse",
                    DeviceKind::Mouse,
                    Some(50),
                    LinkStatus::Online,
                    EARLIER,
                ),
                reading(
                    "m1",
                    "A mouse",
                    DeviceKind::Mouse,
                    Some(50),
                    LinkStatus::Online,
                    EARLIER,
                ),
                reading(
                    "h",
                    "Buds",
                    DeviceKind::Headphones,
                    Some(50),
                    LinkStatus::Online,
                    EARLIER,
                ),
                reading(
                    "m4",
                    "Same",
                    DeviceKind::Mouse,
                    Some(1),
                    LinkStatus::Online,
                    EARLIER,
                ),
                reading(
                    "m3",
                    "Same",
                    DeviceKind::Mouse,
                    Some(2),
                    LinkStatus::Online,
                    EARLIER,
                ),
            ],
            "s",
        );
        let names = HashMap::from([("k".to_owned(), "My keyboard".to_owned())]);
        let hidden = HashSet::from(["h".to_owned()]);
        let order: Vec<String> = store
            .rows(&names, &hidden)
            .into_iter()
            .map(|row| format!("{}:{}", row.id, row.name))
            .collect();
        assert_eq!(
            order,
            [
                "m1:A mouse",
                "m2:b mouse",
                "m3:Same",
                "m4:Same",
                "k:My keyboard"
            ]
        );
    }

    #[test]
    fn a_device_first_seen_asleep_keeps_the_time_it_was_first_seen() {
        let mut store = ReadingStore::default();
        let asleep = |date| {
            reading(
                "bt.1",
                "Buds",
                DeviceKind::Headphones,
                Some(40),
                LinkStatus::Asleep,
                date,
            )
        };
        // Wake up to a device that is already asleep: its level is real, but we have no idea when
        // it was read, so the first sighting becomes the timestamp.
        store.apply(vec![asleep(EARLIER)], "bt");
        assert_eq!(rows(&store)[0].date, EARLIER);
        // A later refresh reports it asleep again; the age must keep growing from the first
        // sighting instead of restarting at "just now" every minute.
        store.apply(vec![asleep(LATER)], "bt");
        let row = &rows(&store)[0];
        assert_eq!(
            (row.level, row.status, row.date),
            (Some(40), LinkStatus::Asleep, EARLIER)
        );
    }

    #[test]
    fn an_online_reading_still_replaces_what_was_remembered() {
        let mut store = ReadingStore::default();
        store.apply(
            vec![reading(
                "bt.1",
                "Buds",
                DeviceKind::Headphones,
                Some(40),
                LinkStatus::Asleep,
                EARLIER,
            )],
            "bt",
        );
        store.apply(
            vec![reading(
                "bt.1",
                "Buds",
                DeviceKind::Headphones,
                Some(38),
                LinkStatus::Online,
                LATER,
            )],
            "bt",
        );
        let row = &rows(&store)[0];
        assert_eq!(
            (row.level, row.status, row.date),
            (Some(38), LinkStatus::Online, LATER)
        );
    }
}
