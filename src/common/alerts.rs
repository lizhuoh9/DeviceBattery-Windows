//! Decides when to warn: once at 20% and once more at 10% per discharge, re-armed when the
//! device charges or climbs back above 25%.

use std::collections::HashMap;

use super::model::{ChargeState, DeviceReading, LinkStatus};
use super::presentation::{CRITICAL_THRESHOLD, LOW_THRESHOLD};

const THRESHOLDS: [u8; 2] = [LOW_THRESHOLD, CRITICAL_THRESHOLD];
const REARM_LEVEL: u8 = 25;

/// A device that just crossed a low-battery threshold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LowBatteryAlert {
    pub id: String,
    pub name: String,
    pub level: u8,
    pub threshold: u8,
}

#[derive(Debug, Default)]
pub struct LowBatteryPolicy {
    /// Device ID → lowest threshold already announced.
    announced: HashMap<String, u8>,
}

impl LowBatteryPolicy {
    pub fn evaluate(&mut self, readings: &[DeviceReading]) -> Vec<LowBatteryAlert> {
        readings
            .iter()
            .filter_map(|reading| self.evaluate_one(reading))
            .collect()
    }

    fn evaluate_one(&mut self, reading: &DeviceReading) -> Option<LowBatteryAlert> {
        let level = reading
            .level
            .filter(|_| reading.status == LinkStatus::Online)?;
        if matches!(reading.charge, ChargeState::Charging | ChargeState::Full)
            || level > REARM_LEVEL
        {
            self.announced.remove(&reading.id);
            return None;
        }
        let threshold = THRESHOLDS
            .iter()
            .copied()
            .filter(|threshold| level <= *threshold)
            .min()?;
        if self
            .announced
            .get(&reading.id)
            .is_some_and(|announced| threshold >= *announced)
        {
            return None;
        }
        self.announced.insert(reading.id.clone(), threshold);
        Some(LowBatteryAlert {
            id: reading.id.clone(),
            name: reading.name.clone(),
            level,
            threshold,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::model::DeviceKind;

    fn reading(level: u8, charge: ChargeState, status: LinkStatus) -> DeviceReading {
        DeviceReading::new(
            "m",
            "Mouse",
            DeviceKind::Mouse,
            Some(level),
            charge,
            status,
            "t",
            0,
        )
    }

    fn thresholds(policy: &mut LowBatteryPolicy, level: u8) -> Vec<u8> {
        policy
            .evaluate(&[reading(level, ChargeState::Discharging, LinkStatus::Online)])
            .into_iter()
            .map(|alert| alert.threshold)
            .collect()
    }

    #[test]
    fn alerts_once_per_threshold_while_discharging() {
        let mut policy = LowBatteryPolicy::default();
        assert!(thresholds(&mut policy, 30).is_empty());
        assert_eq!(thresholds(&mut policy, 19), [20]);
        assert!(thresholds(&mut policy, 18).is_empty());
        assert_eq!(thresholds(&mut policy, 9), [10]);
        assert!(thresholds(&mut policy, 5).is_empty());
    }

    #[test]
    fn a_big_drop_alerts_only_for_the_lowest_threshold() {
        assert_eq!(thresholds(&mut LowBatteryPolicy::default(), 8), [10]);
    }

    #[test]
    fn charging_rearms_and_asleep_changes_nothing() {
        let mut policy = LowBatteryPolicy::default();
        thresholds(&mut policy, 15);
        assert!(
            policy
                .evaluate(&[reading(15, ChargeState::Discharging, LinkStatus::Asleep)])
                .is_empty()
        );
        policy.evaluate(&[reading(40, ChargeState::Charging, LinkStatus::Online)]);
        assert_eq!(thresholds(&mut policy, 15), [20]);
    }
}
