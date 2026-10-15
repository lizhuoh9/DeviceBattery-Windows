//! Paints the widget into a 32-bit bitmap and hands it to a layered window, which gives it
//! per-pixel transparency (rounded corners, soft shadow) over whatever is behind it.

use std::mem::size_of;

use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::Direct2D::ID2D1DCRenderTarget;
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HDC,
    ReleaseDC, SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::{ULW_ALPHA, UpdateLayeredWindow};

use super::render::{Renderer, RowModel};

pub struct Painter {
    renderer: Renderer,
    /// Recreated after a device loss (driver update, sleep).
    target: Option<ID2D1DCRenderTarget>,
}

impl Painter {
    pub fn new(renderer: Renderer) -> Self {
        Self {
            renderer,
            target: None,
        }
    }

    /// Redraws `window` at `origin` (physical pixels) with `rows`, at `scale` × 96 DPI.
    pub fn paint(
        &mut self,
        window: HWND,
        origin: POINT,
        scale: f32,
        rows: &[RowModel],
        empty: &str,
        dark: bool,
    ) {
        let (width, height) = Renderer::size(rows.len());
        let size = SIZE {
            cx: (width * scale).ceil() as i32,
            cy: (height * scale).ceil() as i32,
        };
        if self.target.is_none() {
            self.target = self.renderer.dc_target().ok();
        }
        let Some(target) = self.target.clone() else {
            return;
        };
        // SAFETY: GDI objects are selected out and deleted before returning; the DIB stays
        // selected into the memory DC while Direct2D draws into it.
        unsafe {
            let screen = GetDC(None);
            let memory = CreateCompatibleDC(Some(screen));
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: size.cx,
                    biHeight: -size.cy,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            if let Ok(bitmap) =
                CreateDIBSection(Some(memory), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            {
                let previous = SelectObject(memory, bitmap.into());
                if self.draw(&target, memory, size, scale, rows, empty, dark) {
                    present(window, screen, memory, origin, size);
                } else {
                    self.target = None;
                }
                SelectObject(memory, previous);
                let _ = DeleteObject(bitmap.into());
            }
            let _ = DeleteDC(memory);
            ReleaseDC(None, screen);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw(
        &self,
        target: &ID2D1DCRenderTarget,
        memory: HDC,
        size: SIZE,
        scale: f32,
        rows: &[RowModel],
        empty: &str,
        dark: bool,
    ) -> bool {
        let bounds = RECT {
            left: 0,
            top: 0,
            right: size.cx,
            bottom: size.cy,
        };
        // SAFETY: the DC holds a DIB of exactly `bounds`; drawing is bracketed by
        // BeginDraw / EndDraw.
        unsafe {
            if target.BindDC(memory, &bounds).is_err() {
                return false;
            }
            target.SetDpi(96.0 * scale, 96.0 * scale);
            target.BeginDraw();
            target.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));
            let painted = self.renderer.draw(target, rows, empty, dark).is_ok();
            target.EndDraw(None, None).is_ok() && painted
        }
    }
}

fn present(window: HWND, screen: HDC, memory: HDC, origin: POINT, size: SIZE) {
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    // SAFETY: both DCs are valid for the duration of the call.
    unsafe {
        let _ = UpdateLayeredWindow(
            window,
            Some(screen),
            Some(&origin),
            Some(&size),
            Some(memory),
            Some(&POINT::default()),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
    }
}
