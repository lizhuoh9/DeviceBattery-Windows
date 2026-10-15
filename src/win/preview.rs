//! `DeviceBattery.exe --render-preview out.png [--language en|ko|zh-Hans]` draws the widget
//! with sample devices in light and dark mode, side by side, into a PNG. Used for the README
//! and to check the real Direct2D rendering on Windows in CI.

use windows::Win32::Foundation::{E_FAIL, GENERIC_WRITE};
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatPng, GUID_WICPixelFormat32bppPBGRA, IWICBitmap,
    IWICImagingFactory, WICBitmapCacheOnLoad, WICBitmapEncoderNoCache,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::core::{Error, PCWSTR, Result};
use windows_numerics::Matrix3x2;

use super::monitor::now_seconds;
use super::render::{Renderer, target_properties};
use super::rows::row_models;
use super::text::wide;
use crate::common::model::{ChargeState, DeviceKind, DeviceReading, LinkStatus};
use crate::common::strings::{BLUETOOTH_SOURCE, Strings};

const SCALE: f32 = 2.0;
const GAP: f32 = 24.0;

pub fn render(path: &str, strings: &Strings) -> Result<()> {
    let renderer = Renderer::new()?;
    let now = now_seconds();
    let rows = row_models(&samples(strings, now), now, strings, None);
    let (card_width, card_height) = Renderer::size(rows.len());
    let width = ((card_width * 2.0 + GAP * 3.0) * SCALE).ceil() as u32;
    let height = ((card_height + GAP * 2.0) * SCALE).ceil() as u32;
    // SAFETY: COM is initialized by `run`; every object is used on this thread only.
    unsafe {
        let wic: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let bitmap = wic.CreateBitmap(
            width,
            height,
            &GUID_WICPixelFormat32bppPBGRA,
            WICBitmapCacheOnLoad,
        )?;
        let target = renderer
            .factory()
            .CreateWicBitmapRenderTarget(&bitmap, &target_properties(96.0 * SCALE))?;
        target.BeginDraw();
        target.Clear(Some(&D2D1_COLOR_F {
            r: 0.42,
            g: 0.55,
            b: 0.68,
            a: 1.0,
        }));
        target.SetTransform(&Matrix3x2::translation(GAP, GAP));
        renderer.draw(&target, &rows, "", false)?;
        target.SetTransform(&Matrix3x2::translation(GAP * 2.0 + card_width, GAP));
        renderer.draw(&target, &rows, "", true)?;
        target.EndDraw(None, None)?;
        save_png(&wic, &bitmap, path, width, height)
    }
}

fn save_png(
    wic: &IWICImagingFactory,
    bitmap: &IWICBitmap,
    path: &str,
    width: u32,
    height: u32,
) -> Result<()> {
    let path = wide(path);
    // SAFETY: the stream, encoder and frame are committed in order before they are released.
    unsafe {
        let stream = wic.CreateStream()?;
        stream.InitializeFromFilename(PCWSTR(path.as_ptr()), GENERIC_WRITE.0)?;
        let encoder = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let (mut frame, mut options) = (None, None);
        encoder.CreateNewFrame(&mut frame, &mut options)?;
        let frame = frame.ok_or_else(|| Error::from(E_FAIL))?;
        frame.Initialize(options.as_ref())?;
        frame.SetSize(width, height)?;
        let mut format = GUID_WICPixelFormat32bppPBGRA;
        frame.SetPixelFormat(&mut format)?;
        frame.WriteSource(bitmap, std::ptr::null())?;
        frame.Commit()?;
        encoder.Commit()
    }
}

fn samples(strings: &Strings, now: u64) -> Vec<DeviceReading> {
    let sample = |name: String, kind, level, charge, status, source: &str, age: u64| {
        DeviceReading::new(
            name.clone(),
            name,
            kind,
            level,
            charge,
            status,
            source,
            now - age,
        )
    };
    vec![
        sample(
            "Wireless mouse 8k".into(),
            DeviceKind::Mouse,
            Some(76),
            ChargeState::Discharging,
            LinkStatus::Online,
            "Compx 2.4G",
            0,
        ),
        sample(
            "ROG Harpe Ace Mini".into(),
            DeviceKind::Mouse,
            Some(8),
            ChargeState::Discharging,
            LinkStatus::Online,
            "ROG Omni",
            0,
        ),
        sample(
            "MX Keys S".into(),
            DeviceKind::Keyboard,
            Some(86),
            ChargeState::Discharging,
            LinkStatus::Asleep,
            "Logi Bolt",
            3 * 3600,
        ),
        sample(
            strings.rog_device(DeviceKind::Keyboard),
            DeviceKind::Keyboard,
            Some(43),
            ChargeState::Charging,
            LinkStatus::Online,
            "ROG Omni",
            0,
        ),
        sample(
            strings.my_earbuds(),
            DeviceKind::Headphones,
            Some(18),
            ChargeState::Unknown,
            LinkStatus::Online,
            BLUETOOTH_SOURCE,
            0,
        ),
        sample(
            "MX Master 3S".into(),
            DeviceKind::Mouse,
            None,
            ChargeState::Unknown,
            LinkStatus::Asleep,
            "Logi Bolt",
            0,
        ),
    ]
}
