//! Draws the widget with Direct2D and DirectWrite.
//!
//! The same drawing serves the live window (a GDI DC bound to a layered window) and the
//! preview images (a WIC bitmap), so previews show exactly what users see.

use std::f32::consts::TAU;

use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_HOLLOW,
    D2D1_FIGURE_END_OPEN, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_LARGE, D2D1_ARC_SIZE_SMALL, D2D1_CAP_STYLE_ROUND,
    D2D1_DASH_STYLE_SOLID, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_LINE_JOIN_ROUND,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE,
    D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES, D2D1_SWEEP_DIRECTION_CLOCKWISE,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory,
    ID2D1PathGeometry, ID2D1RenderTarget, ID2D1StrokeStyle,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
    DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP,
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::core::{BOOL, PCWSTR, Result};
use windows_numerics::Vector2;

use super::text::{utf16, wide};
use crate::common::model::DeviceKind;
use crate::common::presentation::RowTone;

pub const CARD_WIDTH: f32 = 284.0;
/// Margin around the card for its shadow; part of the window bitmap.
pub const SHADOW: f32 = 12.0;
pub const ROW_HEIGHT: f32 = 46.0;
const CORNER: f32 = 14.0;
const PADDING: f32 = 8.0;
const ROW_INSET: f32 = 8.0;
const RING: f32 = 32.0;
const RING_STROKE: f32 = 3.0;

/// One device row, ready to draw.
#[derive(Clone, Debug)]
pub struct RowModel {
    pub name: String,
    pub detail: String,
    pub percent: String,
    pub has_level: bool,
    pub fraction: Option<f32>,
    pub tone: RowTone,
    pub kind: DeviceKind,
    pub hovered: bool,
}

pub struct Renderer {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    stroke: ID2D1StrokeStyle,
    name: IDWriteTextFormat,
    detail: IDWriteTextFormat,
    percent: IDWriteTextFormat,
    sign: IDWriteTextFormat,
    empty: IDWriteTextFormat,
    glyph: IDWriteTextFormat,
    badge: IDWriteTextFormat,
}

impl Renderer {
    pub fn new() -> Result<Self> {
        // SAFETY: factory creation and text-format setup only read the arguments given.
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let stroke = factory.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    dashCap: D2D1_CAP_STYLE_ROUND,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    miterLimit: 10.0,
                    dashStyle: D2D1_DASH_STYLE_SOLID,
                    dashOffset: 0.0,
                },
                None,
            )?;
            let fonts = Fonts::detect(&dwrite);
            let format = |family: &str, weight: DWRITE_FONT_WEIGHT, size: f32| {
                text_format(&dwrite, family, weight, size)
            };
            let name = format(&fonts.text, DWRITE_FONT_WEIGHT_MEDIUM, 13.0)?;
            let detail = format(&fonts.text, DWRITE_FONT_WEIGHT_NORMAL, 11.5)?;
            let percent = format(&fonts.display, DWRITE_FONT_WEIGHT_SEMI_BOLD, 19.0)?;
            let sign = format(&fonts.display, DWRITE_FONT_WEIGHT_SEMI_BOLD, 11.0)?;
            let empty = format(&fonts.text, DWRITE_FONT_WEIGHT_NORMAL, 12.0)?;
            let glyph = format(&fonts.icons, DWRITE_FONT_WEIGHT_NORMAL, 14.0)?;
            let badge = format(&fonts.icons, DWRITE_FONT_WEIGHT_NORMAL, 7.5)?;
            for single_line in [&name, &detail] {
                single_line.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                let ellipsis = dwrite.CreateEllipsisTrimmingSign(single_line)?;
                let trimming = DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: 0,
                    delimiterCount: 0,
                };
                single_line.SetTrimming(&trimming, &ellipsis)?;
            }
            for (format, alignment) in [
                (&name, DWRITE_TEXT_ALIGNMENT_LEADING),
                (&detail, DWRITE_TEXT_ALIGNMENT_LEADING),
                (&percent, DWRITE_TEXT_ALIGNMENT_TRAILING),
                (&sign, DWRITE_TEXT_ALIGNMENT_TRAILING),
                (&empty, DWRITE_TEXT_ALIGNMENT_CENTER),
                (&glyph, DWRITE_TEXT_ALIGNMENT_CENTER),
                (&badge, DWRITE_TEXT_ALIGNMENT_CENTER),
            ] {
                format.SetTextAlignment(alignment)?;
                format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            }
            Ok(Self {
                factory,
                dwrite,
                stroke,
                name,
                detail,
                percent,
                sign,
                empty,
                glyph,
                badge,
            })
        }
    }

    pub fn factory(&self) -> &ID2D1Factory {
        &self.factory
    }

    /// A render target that draws into a GDI DC with per-pixel alpha.
    pub fn dc_target(&self) -> Result<ID2D1DCRenderTarget> {
        // SAFETY: the properties struct is fully initialized.
        unsafe { self.factory.CreateDCRenderTarget(&target_properties(0.0)) }
    }

    /// The bitmap size (card plus shadow margin) in DIPs for `rows` rows.
    pub fn size(rows: usize) -> (f32, f32) {
        (CARD_WIDTH + 2.0 * SHADOW, card_height(rows) + 2.0 * SHADOW)
    }

    /// The row under a point given in DIPs from the bitmap's top-left corner.
    pub fn row_at(x: f32, y: f32, rows: usize) -> Option<usize> {
        let inside = (SHADOW..SHADOW + CARD_WIDTH).contains(&x) && y >= SHADOW + PADDING;
        let index = ((y - SHADOW - PADDING) / ROW_HEIGHT).floor();
        (inside && index >= 0.0 && (index as usize) < rows).then_some(index as usize)
    }

    /// Draws the whole widget at the target's origin; the caller brackets it with
    /// BeginDraw / EndDraw.
    pub fn draw(
        &self,
        target: &ID2D1RenderTarget,
        rows: &[RowModel],
        empty: &str,
        dark: bool,
    ) -> Result<()> {
        let palette = Palette::new(dark);
        let height = card_height(rows.len());
        // SAFETY: drawing calls only read the geometry, brushes and strings passed in.
        unsafe {
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            for step in 1..=6u8 {
                let spread = f32::from(step) * 1.5;
                let shadow = rounded(
                    SHADOW - spread,
                    SHADOW - spread + 3.0,
                    CARD_WIDTH + 2.0 * spread,
                    height + 2.0 * spread,
                    CORNER + spread,
                );
                target.FillRoundedRectangle(
                    &shadow,
                    &target.CreateSolidColorBrush(&palette.shadow, None)?,
                );
            }
            let card = rounded(SHADOW, SHADOW, CARD_WIDTH, height, CORNER);
            target.FillRoundedRectangle(
                &card,
                &target.CreateSolidColorBrush(&palette.background, None)?,
            );
            let edge = rounded(
                SHADOW + 0.5,
                SHADOW + 0.5,
                CARD_WIDTH - 1.0,
                height - 1.0,
                CORNER - 0.5,
            );
            target.DrawRoundedRectangle(
                &edge,
                &target.CreateSolidColorBrush(&palette.border, None)?,
                1.0,
                None,
            );

            if rows.is_empty() {
                let rect = rect(SHADOW, SHADOW, CARD_WIDTH, height);
                let brush = target.CreateSolidColorBrush(&palette.secondary, None)?;
                target.DrawText(
                    &utf16(empty),
                    &self.empty,
                    &rect,
                    &brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
            for (index, row) in rows.iter().enumerate() {
                self.draw_row(
                    target,
                    row,
                    SHADOW + PADDING + index as f32 * ROW_HEIGHT,
                    &palette,
                )?;
            }
        }
        Ok(())
    }

    fn draw_row(
        &self,
        target: &ID2D1RenderTarget,
        row: &RowModel,
        top: f32,
        palette: &Palette,
    ) -> Result<()> {
        let left = SHADOW + PADDING;
        let width = CARD_WIDTH - 2.0 * PADDING;
        let center = Vector2 {
            X: left + ROW_INSET + RING / 2.0,
            Y: top + ROW_HEIGHT / 2.0,
        };
        let radius = RING / 2.0 - RING_STROKE / 2.0;
        let accent = palette.accent(row.tone);
        // SAFETY: drawing calls only read the geometry, brushes and strings passed in.
        unsafe {
            if row.hovered {
                let highlight = rounded(left, top, width, ROW_HEIGHT, 9.0);
                target.FillRoundedRectangle(
                    &highlight,
                    &target.CreateSolidColorBrush(&palette.hover, None)?,
                );
            }

            let ellipse = D2D1_ELLIPSE {
                point: center,
                radiusX: radius,
                radiusY: radius,
            };
            target.DrawEllipse(
                &ellipse,
                &target.CreateSolidColorBrush(&palette.track, None)?,
                RING_STROKE,
                None,
            );
            if let Some(fraction) = row.fraction.filter(|fraction| *fraction > 0.0) {
                let brush = target.CreateSolidColorBrush(&accent, None)?;
                if fraction >= 0.999 {
                    target.DrawEllipse(&ellipse, &brush, RING_STROKE, None);
                } else {
                    let arc = self.arc(center, radius, fraction)?;
                    target.DrawGeometry(&arc, &brush, RING_STROKE, &self.stroke);
                }
            }

            let glyph_color = if row.tone == RowTone::Inactive {
                palette.tertiary
            } else {
                palette.primary
            };
            let ring_rect = rect(center.X - RING / 2.0, center.Y - RING / 2.0, RING, RING);
            target.DrawText(
                &utf16(glyph(row.kind)),
                &self.glyph,
                &ring_rect,
                &target.CreateSolidColorBrush(&glyph_color, None)?,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            self.draw_badge(target, row.tone, center, palette)?;

            let percent_right = left + width - ROW_INSET - 2.0;
            let sign_width = if row.has_level {
                self.measure("%", &self.sign)?
            } else {
                0.0
            };
            let number_width = self.measure(&row.percent, &self.percent)?;
            let percent_color = match row.tone {
                RowTone::Normal => palette.primary,
                RowTone::Inactive => palette.secondary,
                tone => palette.accent(tone),
            };
            let number_rect = rect(
                percent_right - sign_width - number_width - 4.0,
                top,
                number_width + 4.0,
                ROW_HEIGHT,
            );
            target.DrawText(
                &utf16(&row.percent),
                &self.percent,
                &number_rect,
                &target.CreateSolidColorBrush(&percent_color, None)?,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            if row.has_level {
                let sign_color = if row.tone == RowTone::Normal {
                    palette.secondary
                } else {
                    percent_color
                };
                let sign_rect = rect(
                    percent_right - sign_width,
                    top + 2.5,
                    sign_width,
                    ROW_HEIGHT,
                );
                target.DrawText(
                    &utf16("%"),
                    &self.sign,
                    &sign_rect,
                    &target.CreateSolidColorBrush(&sign_color, None)?,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            let text_left = center.X + RING / 2.0 + 10.0;
            let text_width =
                (percent_right - sign_width - number_width - 8.0 - text_left).max(10.0);
            let name_rect = rect(text_left, top + 5.0, text_width, 19.0);
            target.DrawText(
                &utf16(&row.name),
                &self.name,
                &name_rect,
                &target.CreateSolidColorBrush(&palette.primary, None)?,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            let detail_color = match row.tone {
                RowTone::Critical => palette.critical,
                RowTone::Low => palette.low,
                _ => palette.secondary,
            };
            let detail_rect = rect(text_left, top + 23.0, text_width, 17.0);
            target.DrawText(
                &utf16(&row.detail),
                &self.detail,
                &detail_rect,
                &target.CreateSolidColorBrush(&detail_color, None)?,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        Ok(())
    }

    /// A small filled circle on the ring's lower-right edge: charging bolt or sleeping moon.
    fn draw_badge(
        &self,
        target: &ID2D1RenderTarget,
        tone: RowTone,
        center: Vector2,
        palette: &Palette,
    ) -> Result<()> {
        let (symbol, fill) = match tone {
            RowTone::Charging => ("\u{E945}", palette.charging),
            RowTone::Inactive => ("\u{E708}", palette.badge_inactive),
            _ => return Ok(()),
        };
        let badge_center = Vector2 {
            X: center.X + RING / 2.0 - 4.0,
            Y: center.Y + RING / 2.0 - 4.0,
        };
        let radius = 7.0;
        let ellipse = D2D1_ELLIPSE {
            point: badge_center,
            radiusX: radius,
            radiusY: radius,
        };
        // SAFETY: drawing calls only read the geometry, brushes and strings passed in.
        unsafe {
            target.FillEllipse(
                &ellipse,
                &target.CreateSolidColorBrush(&palette.background_opaque, None)?,
            );
            let inner = D2D1_ELLIPSE {
                point: badge_center,
                radiusX: radius - 1.5,
                radiusY: radius - 1.5,
            };
            target.FillEllipse(&inner, &target.CreateSolidColorBrush(&fill, None)?);
            let box_rect = rect(
                badge_center.X - radius,
                badge_center.Y - radius,
                radius * 2.0,
                radius * 2.0,
            );
            target.DrawText(
                &utf16(symbol),
                &self.badge,
                &box_rect,
                &target.CreateSolidColorBrush(&palette.on_badge, None)?,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        Ok(())
    }

    /// An arc from 12 o'clock, clockwise, covering `fraction` of the circle.
    fn arc(&self, center: Vector2, radius: f32, fraction: f32) -> Result<ID2D1PathGeometry> {
        let angle = fraction * TAU;
        let start = Vector2 {
            X: center.X,
            Y: center.Y - radius,
        };
        let end = Vector2 {
            X: center.X + radius * angle.sin(),
            Y: center.Y - radius * angle.cos(),
        };
        // SAFETY: the sink is closed before the geometry is used.
        unsafe {
            let geometry = self.factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            sink.BeginFigure(start, D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: end,
                size: D2D_SIZE_F {
                    width: radius,
                    height: radius,
                },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: if fraction > 0.5 {
                    D2D1_ARC_SIZE_LARGE
                } else {
                    D2D1_ARC_SIZE_SMALL
                },
            });
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            sink.Close()?;
            Ok(geometry)
        }
    }

    /// Width of `text` laid out in `format`, in DIPs.
    fn measure(&self, text: &str, format: &IDWriteTextFormat) -> Result<f32> {
        // SAFETY: the layout only reads the string and format.
        unsafe {
            let layout = self
                .dwrite
                .CreateTextLayout(&utf16(text), format, 500.0, 100.0)?;
            let mut metrics = Default::default();
            layout.GetMetrics(&mut metrics)?;
            Ok(metrics.widthIncludingTrailingWhitespace)
        }
    }
}

/// Render-target properties for 32-bit premultiplied BGRA at `dpi` (0 = default).
pub fn target_properties(dpi: f32) -> D2D1_RENDER_TARGET_PROPERTIES {
    D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: dpi,
        dpiY: dpi,
        usage: D2D1_RENDER_TARGET_USAGE_NONE,
        minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
    }
}

fn card_height(rows: usize) -> f32 {
    rows.max(1) as f32 * ROW_HEIGHT + 2.0 * PADDING
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: x,
        top: y,
        right: x + width,
        bottom: y + height,
    }
}

fn rounded(x: f32, y: f32, width: f32, height: f32, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT {
        rect: rect(x, y, width, height),
        radiusX: radius,
        radiusY: radius,
    }
}

/// Segoe Fluent Icons / Segoe MDL2 Assets code points.
fn glyph(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Mouse => "\u{E962}",
        DeviceKind::Keyboard => "\u{E765}",
        DeviceKind::Trackpad => "\u{EFA5}",
        DeviceKind::Headphones => "\u{E7F6}",
        DeviceKind::Gamepad => "\u{E7FC}",
        DeviceKind::Speaker => "\u{E7F5}",
        DeviceKind::Other => "\u{E702}",
    }
}

/// Font families, with fallbacks for Windows 10.
struct Fonts {
    text: String,
    display: String,
    icons: String,
}

impl Fonts {
    fn detect(dwrite: &IDWriteFactory) -> Self {
        let mut collection: Option<IDWriteFontCollection> = None;
        // SAFETY: `collection` receives the system font collection.
        let _ = unsafe { dwrite.GetSystemFontCollection(&mut collection, false) };
        let has = |family: &str| {
            let Some(collection) = &collection else {
                return false;
            };
            let name = wide(family);
            let (mut index, mut exists) = (0u32, BOOL(0));
            // SAFETY: the family name is NUL-terminated; out-parameters are live locals.
            unsafe { collection.FindFamilyName(PCWSTR(name.as_ptr()), &mut index, &mut exists) }
                .is_ok()
                && exists.as_bool()
        };
        let pick = |preferred: &str, fallback: &str| {
            if has(preferred) { preferred } else { fallback }.to_owned()
        };
        Self {
            text: pick("Segoe UI Variable Text", "Segoe UI"),
            display: pick("Segoe UI Variable Display", "Segoe UI"),
            icons: pick("Segoe Fluent Icons", "Segoe MDL2 Assets"),
        }
    }
}

fn text_format(
    dwrite: &IDWriteFactory,
    family: &str,
    weight: DWRITE_FONT_WEIGHT,
    size: f32,
) -> Result<IDWriteTextFormat> {
    let family = wide(family);
    let locale = wide("");
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe {
        dwrite.CreateTextFormat(
            PCWSTR(family.as_ptr()),
            None,
            weight,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            PCWSTR(locale.as_ptr()),
        )
    }
}

/// Colors for light or dark mode. Healthy levels stay neutral so only rows that need
/// attention carry color.
struct Palette {
    background: D2D1_COLOR_F,
    background_opaque: D2D1_COLOR_F,
    border: D2D1_COLOR_F,
    shadow: D2D1_COLOR_F,
    primary: D2D1_COLOR_F,
    secondary: D2D1_COLOR_F,
    tertiary: D2D1_COLOR_F,
    track: D2D1_COLOR_F,
    neutral: D2D1_COLOR_F,
    hover: D2D1_COLOR_F,
    low: D2D1_COLOR_F,
    critical: D2D1_COLOR_F,
    charging: D2D1_COLOR_F,
    badge_inactive: D2D1_COLOR_F,
    on_badge: D2D1_COLOR_F,
}

impl Palette {
    fn new(dark: bool) -> Self {
        let ink = if dark { 1.0 } else { 0.0 };
        let shade = |alpha: f32| color(ink, ink, ink, alpha);
        if dark {
            Self {
                background: color(0.125, 0.125, 0.13, 0.93),
                background_opaque: color(0.125, 0.125, 0.13, 1.0),
                border: shade(0.10),
                shadow: color(0.0, 0.0, 0.0, 0.07),
                primary: shade(1.0),
                secondary: shade(0.64),
                tertiary: shade(0.36),
                track: shade(0.14),
                neutral: shade(0.9),
                hover: shade(0.07),
                low: color(1.0, 0.58, 0.16, 1.0),
                critical: color(1.0, 0.42, 0.42, 1.0),
                charging: color(0.42, 0.8, 0.37, 1.0),
                badge_inactive: shade(0.4),
                on_badge: color(1.0, 1.0, 1.0, 1.0),
            }
        } else {
            Self {
                background: color(0.976, 0.976, 0.976, 0.93),
                background_opaque: color(0.976, 0.976, 0.976, 1.0),
                border: shade(0.08),
                shadow: color(0.0, 0.0, 0.0, 0.025),
                primary: color(0.1, 0.1, 0.1, 1.0),
                secondary: shade(0.58),
                tertiary: shade(0.32),
                track: shade(0.10),
                neutral: shade(0.68),
                hover: shade(0.05),
                low: color(0.79, 0.31, 0.06, 1.0),
                critical: color(0.82, 0.2, 0.22, 1.0),
                charging: color(0.06, 0.55, 0.06, 1.0),
                badge_inactive: shade(0.38),
                on_badge: color(1.0, 1.0, 1.0, 1.0),
            }
        }
    }

    fn accent(&self, tone: RowTone) -> D2D1_COLOR_F {
        match tone {
            RowTone::Normal => self.neutral,
            RowTone::Low => self.low,
            RowTone::Critical => self.critical,
            RowTone::Charging => self.charging,
            RowTone::Inactive => self.tertiary,
        }
    }
}

fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}
