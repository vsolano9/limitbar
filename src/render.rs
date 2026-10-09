//! Software painter for a per-pixel-alpha layered window: anti-aliased rounded
//! bars written straight into a premultiplied BGRA DIB, and GDI text rendered
//! as a grayscale coverage mask, then tinted and blended.

use std::ffi::c_void;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::*;

use crate::wide;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: f32,
}

pub const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Rgba {
    Rgba { r, g, b, a }
}

impl Rgba {
    pub fn fade(self, k: f32) -> Rgba {
        Rgba { a: self.a * k, ..self }
    }
}

/// A top-down 32-bit DIB selected into its own memory DC.
pub struct Surface {
    pub dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    pub w: i32,
    pub h: i32,
}

impl Surface {
    pub fn new(w: i32, h: i32) -> Option<Surface> {
        unsafe {
            let dc = CreateCompatibleDC(null_mut());
            if dc.is_null() {
                return None;
            }
            let mut bmi: BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w;
            bmi.bmiHeader.biHeight = -h;
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = BI_RGB;
            let mut bits: *mut c_void = null_mut();
            let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if bmp.is_null() || bits.is_null() {
                DeleteDC(dc);
                return None;
            }
            let old = SelectObject(dc, bmp as HGDIOBJ);
            Some(Surface { dc, bmp, old, bits: bits as *mut u32, w, h })
        }
    }

    pub fn pixels(&mut self) -> &mut [u32] {
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.w * self.h) as usize) }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bmp as HGDIOBJ);
            DeleteDC(self.dc);
        }
    }
}

pub struct Font(pub HFONT);

impl Font {
    /// GDI substitutes a similar face when `face` is not installed.
    pub fn new(face: &str, px: i32, weight: i32) -> Font {
        let face = wide(face);
        let f = unsafe {
            CreateFontW(
                -px,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_TT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                ANTIALIASED_QUALITY as u32,
                DEFAULT_PITCH as u32,
                face.as_ptr(),
            )
        };
        Font(f)
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        unsafe { DeleteObject(self.0 as HGDIOBJ) };
    }
}

pub fn text_width(dc: HDC, font: &Font, s: &str) -> i32 {
    let w = wide(s);
    let mut size = SIZE { cx: 0, cy: 0 };
    unsafe {
        SelectObject(dc, font.0 as HGDIOBJ);
        GetTextExtentPoint32W(dc, w.as_ptr(), (w.len() - 1) as i32, &mut size);
    }
    size.cx
}

#[derive(Clone, Copy, PartialEq)]
pub enum Align {
    Left,
    Right,
}

pub struct Painter {
    pub out: Surface,
    mask: Surface,
}

#[inline]
fn blend(dst: &mut u32, c: Rgba, coverage: f32) {
    let a = (c.a * coverage).clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let d = *dst;
    let inv = 1.0 - a;
    let ch = |shift: u32, src: u8| -> u32 {
        let dv = ((d >> shift) & 0xff) as f32;
        ((src as f32 * a + dv * inv).round() as u32).min(255)
    };
    let da = ((d >> 24) & 0xff) as f32;
    let oa = ((255.0 * a + da * inv).round() as u32).min(255);
    *dst = (oa << 24) | (ch(16, c.r) << 16) | (ch(8, c.g) << 8) | ch(0, c.b);
}

impl Painter {
    pub fn new(w: i32, h: i32) -> Option<Painter> {
        let p = Painter { out: Surface::new(w, h)?, mask: Surface::new(w, h)? };
        unsafe {
            SetBkMode(p.mask.dc, TRANSPARENT as _);
            SetTextColor(p.mask.dc, 0x00FF_FFFF);
        }
        Some(p)
    }

    /// Fills with an almost invisible floor (alpha 1/255) so the whole widget
    /// receives the mouse: fully transparent layered pixels are click-through.
    pub fn clear(&mut self) {
        self.out.pixels().fill(0x0100_0000);
    }

    /// Draws `s` vertically centred in the `x..x+w` / `y..y+h` box.
    #[allow(clippy::too_many_arguments)]
    pub fn text(&mut self, font: &Font, s: &str, x: i32, y: i32, w: i32, h: i32, align: Align, c: Rgba) {
        let (sw, sh) = (self.out.w, self.out.h);
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w).min(sw);
        let y1 = (y + h).min(sh);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        {
            let m = self.mask.pixels();
            for row in y0..y1 {
                m[(row * sw + x0) as usize..(row * sw + x1) as usize].fill(0);
            }
        }
        let text = wide(s);
        let mut rc = RECT { left: x, top: y, right: x + w, bottom: y + h };
        let flags = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_NOCLIP | if align == Align::Right { DT_RIGHT } else { DT_LEFT };
        unsafe {
            SelectObject(self.mask.dc, font.0 as HGDIOBJ);
            DrawTextW(self.mask.dc, text.as_ptr(), (text.len() - 1) as i32, &mut rc, flags);
            GdiFlush();
        }
        let mask = self.mask.pixels() as *const [u32];
        let out = self.out.pixels();
        for row in y0..y1 {
            for col in x0..x1 {
                let idx = (row * sw + col) as usize;
                // SAFETY: `mask` and `out` are two distinct DIBs of the same size.
                let g = ((unsafe { (*mask)[idx] } >> 8) & 0xff) as f32 / 255.0;
                if g > 0.0 {
                    // Slight gamma lift: GDI grayscale coverage on black reads thin.
                    blend(&mut out[idx], c, g.powf(0.8));
                }
            }
        }
    }

    /// Anti-aliased rounded rectangle via a signed distance field.
    pub fn round_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, c: Rgba) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let r = r.min(w / 2.0).min(h / 2.0);
        let (sw, sh) = (self.out.w, self.out.h);
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        let (hx, hy) = (w / 2.0 - r, h / 2.0 - r);
        let px0 = (x.floor() as i32).max(0);
        let py0 = (y.floor() as i32).max(0);
        let px1 = ((x + w).ceil() as i32).min(sw);
        let py1 = ((y + h).ceil() as i32).min(sh);
        let out = self.out.pixels();
        for py in py0..py1 {
            for px in px0..px1 {
                let qx = ((px as f32 + 0.5 - cx).abs() - hx).max(0.0);
                let qy = ((py as f32 + 0.5 - cy).abs() - hy).max(0.0);
                let d = (qx * qx + qy * qy).sqrt() - r;
                // Also clip to the exact box so sub-pixel widths stay honest.
                let ex = (x + w - px as f32).min(px as f32 + 1.0 - x).clamp(0.0, 1.0);
                let ey = (y + h - py as f32).min(py as f32 + 1.0 - y).clamp(0.0, 1.0);
                let cov = (0.5 - d).clamp(0.0, 1.0) * ex * ey;
                if cov > 0.0 {
                    blend(&mut out[(py * sw + px) as usize], c, cov);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_is_premultiplied_over() {
        let mut px = 0x0100_0000;
        blend(&mut px, rgba(255, 0, 0, 1.0), 1.0);
        assert_eq!(px, 0xFFFF_0000);
        let mut px = 0;
        blend(&mut px, rgba(255, 255, 255, 0.5), 1.0);
        assert_eq!(px >> 24, 128);
        assert_eq!(px & 0xff, 128);
        let mut px = 0x1234_5678;
        blend(&mut px, rgba(9, 9, 9, 0.0), 1.0);
        assert_eq!(px, 0x1234_5678);
    }
}
