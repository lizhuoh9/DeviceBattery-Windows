//! Turns readings into the rows the renderer draws.

use super::render::RowModel;
use crate::common::model::DeviceReading;
use crate::common::presentation::RowPresentation;
use crate::common::strings::Strings;

pub fn row_models(
    readings: &[DeviceReading],
    now: u64,
    strings: &Strings,
    hovered: Option<usize>,
) -> Vec<RowModel> {
    readings
        .iter()
        .enumerate()
        .map(|(index, reading)| {
            let presentation = RowPresentation::new(reading, now, strings);
            RowModel {
                name: reading.name.clone(),
                detail: presentation.detail,
                percent: presentation.percent,
                has_level: reading.level.is_some(),
                fraction: presentation.fraction,
                tone: presentation.tone,
                kind: reading.kind,
                hovered: hovered == Some(index),
            }
        })
        .collect()
}
