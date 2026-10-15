//! Document page drawn with GDI: a white portrait sheet (A4 proportions) with a gray border that fills the height
//! of the box, and the text in dark gray with antialiasing. Outside the sheet the thumbnail is transparent.

use crate::bail;
use crate::error::Result;
use crate::image::Image;
use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET,
    DIB_RGB_COLORS, DRAW_TEXT_FORMAT, DT_EDITCONTROL, DT_END_ELLIPSIS, DT_EXPANDTABS, DT_NOPREFIX, DT_SINGLELINE,
    DT_WORDBREAK, DeleteDC, DeleteObject, DrawTextW, FF_MODERN, FF_SWISS, FIXED_PITCH, FONT_WEIGHT, FW_BOLD, FW_NORMAL, FW_SEMIBOLD, GdiFlush,
    HBITMAP, HDC, HFONT, HGDIOBJ, SelectObject, SetBkMode, SetTextColor, TRANSPARENT, VARIABLE_PITCH,
};
use windows::core::{HSTRING, PCWSTR};

/// Already extracted document.
pub struct Doc {
    /// Bold title (email subject, page `<title>`).
    pub title: Option<String>,
    /// Small lines below the title (sender, date…).
    pub meta: Vec<String>,
    pub body: String,
    /// Monospaced (code, logs, NFO) or proportional (prose, with line wrapping).
    pub mono: bool,
}

const BORDER: [u8; 4] = [200, 200, 200, 255];

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

/// Memory DC with a 32-bit DIB selected; frees every GDI object on exit, including on error.
struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    old_bitmap: HGDIOBJ,
    old_font: Option<HGDIOBJ>,
    fonts: Vec<HFONT>,
    bits: *mut u8,
    width: u32,
    height: u32,
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            if let Some(f) = self.old_font {
                SelectObject(self.dc, f);
            }
            if !self.old_bitmap.is_invalid() {
                SelectObject(self.dc, self.old_bitmap);
            }
            let _ = DeleteDC(self.dc);
            if !self.bitmap.is_invalid() {
                let _ = DeleteObject(self.bitmap);
            }
            for f in &self.fonts {
                let _ = DeleteObject(*f);
            }
        }
    }
}

impl Surface {
    fn new(width: u32, height: u32) -> Result<Self> {
        let dc = unsafe { CreateCompatibleDC(None) };
        if dc.is_invalid() {
            bail!("Couldn't create the drawing context");
        }
        let mut s = Surface {
            dc,
            bitmap: HBITMAP::default(),
            old_bitmap: HGDIOBJ::default(),
            old_font: None,
            fonts: Vec::new(),
            bits: std::ptr::null_mut(),
            width,
            height,
        };
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        s.bitmap = unsafe { CreateDIBSection(s.dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)? };
        if bits.is_null() {
            bail!("Couldn't create the page bitmap");
        }
        s.bits = bits as *mut u8;
        s.old_bitmap = unsafe { SelectObject(s.dc, s.bitmap) };
        // White sheet.
        unsafe { std::ptr::write_bytes(s.bits, 0xFF, width as usize * height as usize * 4) };
        unsafe {
            SetBkMode(s.dc, TRANSPARENT);
        }
        Ok(s)
    }

    fn font(&mut self, face: &str, em: i32, weight: FONT_WEIGHT, mono: bool) -> Result<HFONT> {
        let pitch = if mono { FIXED_PITCH.0 as u32 | FF_MODERN.0 as u32 } else { VARIABLE_PITCH.0 as u32 | FF_SWISS.0 as u32 };
        let name = HSTRING::from(face);
        let f = unsafe {
            CreateFontW(-em, 0, 0, 0, weight.0 as i32, 0, 0, 0, DEFAULT_CHARSET.0 as u32, 0, 0, ANTIALIASED_QUALITY.0 as u32, pitch, PCWSTR(name.as_ptr()))
        };
        if f.is_invalid() {
            bail!("Couldn't create the font {face}");
        }
        self.fonts.push(f);
        Ok(f)
    }

    /// Draws `text` in `rect` with the given font and color; returns the height used.
    fn text(&mut self, text: &str, font: HFONT, color: COLORREF, rect: &mut RECT, format: DRAW_TEXT_FORMAT) -> i32 {
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        if wide.is_empty() {
            return 0;
        }
        unsafe {
            let old = SelectObject(self.dc, font);
            self.old_font.get_or_insert(old);
            SetTextColor(self.dc, color);
            DrawTextW(self.dc, &mut wide, rect, format | DT_NOPREFIX)
        }
    }

    /// Sheet pixels (opaque) as RGBA.
    fn pixels(&self) -> Image {
        unsafe {
            let _ = GdiFlush();
        }
        let n = self.width as usize * self.height as usize * 4;
        let mut data = unsafe { std::slice::from_raw_parts(self.bits, n) }.to_vec();
        for p in data.as_chunks_mut::<4>().0.iter_mut() {
            p.swap(0, 2);
            p[3] = 255;
        }
        Image::from_rgba(self.width, self.height, data)
    }
}

pub fn render(doc: &Doc, size: u32) -> Result<Image> {
    let side = size.max(16);
    let (pw, ph) = (((side as f32) / std::f32::consts::SQRT_2).round().max(8.0) as u32, side);
    let mut s = Surface::new(pw, ph)?;
    // About 40 lines on a 256 px sheet.
    let em = ((ph as f32) / 46.0).round().max(3.0) as i32;
    // At thumbnail size thin strokes end up light gray: they are thickened slightly.
    let regular = if em < 8 { FW_SEMIBOLD } else { FW_NORMAL };
    let margin = (pw as f32 * 0.09).round() as i32;
    let mut rect = RECT { left: margin, top: margin, right: pw as i32 - margin, bottom: ph as i32 - margin };
    let (face, mono) = if doc.mono { ("Consolas", true) } else { ("Segoe UI", false) };
    if let Some(title) = doc.title.as_deref().filter(|t| !t.trim().is_empty()) {
        let f = s.font("Segoe UI", em * 3 / 2, FW_BOLD, false)?;
        let mut r = rect;
        let h = s.text(title.trim(), f, rgb(10, 10, 10), &mut r, DT_SINGLELINE | DT_END_ELLIPSIS);
        rect.top += h + em / 2;
    }
    if !doc.meta.is_empty() {
        let f = s.font("Segoe UI", em, regular, false)?;
        for line in &doc.meta {
            let mut r = rect;
            rect.top += s.text(line, f, rgb(95, 95, 95), &mut r, DT_SINGLELINE | DT_END_ELLIPSIS);
        }
        rect.top += em / 2;
    }
    if rect.top < rect.bottom {
        let f = s.font(face, em, regular, mono)?;
        // Monospaced text is not wrapped (it preserves tables and NFO ASCII art), except very long lines such as those
        // of minified JSON or a log, which would otherwise be cut off after a few words.
        let long_lines = doc.body.lines().any(|l| l.chars().count() > 100);
        let wrap = if mono && !long_lines { DRAW_TEXT_FORMAT(0) } else { DT_WORDBREAK | DT_EDITCONTROL };
        s.text(&doc.body, f, rgb(25, 25, 25), &mut rect, DT_EXPANDTABS | wrap);
    }
    let mut page = s.pixels();
    drop(s);
    // Sheet border.
    let bw = (side / 256).max(1);
    for y in 0..ph {
        for x in 0..pw {
            if x < bw || y < bw || x >= pw - bw || y >= ph - bw {
                page.set_pixel(x, y, BORDER);
            }
        }
    }
    let mut img = Image::new(side, side);
    let x0 = (side - pw) / 2;
    for y in 0..ph {
        let src = (y * pw * 4) as usize;
        let dst = ((y * side + x0) * 4) as usize;
        img.data[dst..dst + (pw * 4) as usize].copy_from_slice(&page.data[src..src + (pw * 4) as usize]);
    }
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_layout() {
        let doc = Doc { title: Some("Título".into()), meta: vec!["From: a@b.c".into()], body: "texto ".repeat(200), mono: false };
        let img = render(&doc, 256).unwrap();
        assert_eq!((img.width, img.height), (256, 256));
        let x0 = (256 - 181) / 2;
        assert_eq!(img.pixel(x0 - 1, 100)[3], 0);
        assert_eq!(img.pixel(x0, 100), BORDER);
        assert_eq!(img.pixel(x0 + 3, 3), [255, 255, 255, 255]);
        // The (bold) title is darker than the text.
        let darkest = |y0: u32, y1: u32| (y0..y1).flat_map(|y| (x0..x0 + 181).map(move |x| (x, y))).map(|(x, y)| img.pixel(x, y)[0]).min().unwrap();
        assert!(darkest(16, 26) < 80);
        assert!(darkest(60, 200) < 150);
    }

    #[test]
    fn tiny_and_empty() {
        let doc = Doc { title: None, meta: Vec::new(), body: String::new(), mono: true };
        let img = render(&doc, 1).unwrap();
        assert_eq!((img.width, img.height), (16, 16));
    }
}
