//! 1-bit images (fax, CALS, JBIG): downscaled while they are decoded.
//!
//! An A0 sheet scanned at 400 dpi exceeds 200 megapixels; it is never stored whole. Each line is summed into
//! `k`×`k` pixel cells and the result is a grayscale image (fraction of black per cell), with `k` chosen so
//! that the longer side stays at least twice the thumbnail size. As a bonus, the later resize starts from
//! smoothed grays instead of a bit pattern.

use crate::image::Image;

pub struct Bilevel {
    width: u32,
    k: u32,
    out_width: u32,
    /// Black pixels per output column in the current group of lines.
    acc: Vec<u32>,
    rows_in_group: u32,
    lines: u32,
    /// Grayscale output (0 = black), row by row.
    gray: Vec<u8>,
}

impl Bilevel {
    /// `height_hint` = height if known (0 otherwise); `size` = side of the requested thumbnail.
    pub fn new(width: u32, height_hint: u32, size: u32) -> Self {
        let k = (width.max(height_hint) / size.max(1).saturating_mul(2)).max(1);
        let out_width = width.div_ceil(k);
        Self { width, k, out_width, acc: vec![0; out_width as usize], rows_in_group: 0, lines: 0, gray: Vec::new() }
    }

    pub fn lines(&self) -> u32 {
        self.lines
    }

    /// Line given by its color changes (increasing positions; starts white).
    pub fn push_changes(&mut self, changes: &[u32]) {
        let width = self.width;
        let mut black = false;
        let mut start = 0u32;
        for &c in changes.iter().chain(std::iter::once(&width)) {
            let end = c.min(width);
            if black {
                self.add_black(start, end);
            }
            start = start.max(end);
            black = !black;
        }
        self.end_line();
    }

    fn add_black(&mut self, start: u32, end: u32) {
        let mut x = start;
        while x < end {
            let col = x / self.k;
            let col_end = ((col + 1) * self.k).min(end);
            if let Some(a) = self.acc.get_mut(col as usize) {
                *a += col_end - x;
            }
            x = col_end;
        }
    }

    fn end_line(&mut self) {
        self.lines += 1;
        self.rows_in_group += 1;
        if self.rows_in_group == self.k {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if self.rows_in_group == 0 {
            return;
        }
        for (col, a) in self.acc.iter_mut().enumerate() {
            let cols = self.k.min(self.width - col as u32 * self.k);
            let cell = cols * self.rows_in_group;
            self.gray.push(255 - (*a as u64 * 255 / cell.max(1) as u64) as u8);
            *a = 0;
        }
        self.rows_in_group = 0;
    }

    /// Opaque grayscale image; `None` if no line arrived.
    pub fn finish(mut self) -> Option<Image> {
        self.flush();
        let height = (self.gray.len() / self.out_width.max(1) as usize) as u32;
        if height == 0 {
            return None;
        }
        let data = self.gray.iter().flat_map(|&g| [g, g, g, 255]).collect();
        Some(Image::from_rgba(self.out_width, height, data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_resolution_changes() {
        let mut b = Bilevel::new(10, 2, 256);
        b.push_changes(&[2, 5]);
        b.push_changes(&[0, 1, 9]);
        let img = b.finish().unwrap();
        assert_eq!((img.width, img.height), (10, 2));
        assert_eq!(img.pixel(1, 0), [255, 255, 255, 255]);
        assert_eq!(img.pixel(2, 0), [0, 0, 0, 255]);
        assert_eq!(img.pixel(5, 0)[0], 255);
        assert_eq!((img.pixel(0, 1)[0], img.pixel(9, 1)[0], img.pixel(1, 1)[0]), (0, 0, 255));
    }

    #[test]
    fn reduction_averages_cells() {
        // 8×3 with k = 2 (2 px box): the left half is black.
        let mut b = Bilevel::new(8, 3, 2);
        for _ in 0..3 {
            b.push_changes(&[0, 4]);
        }
        let img = b.finish().unwrap();
        assert_eq!((img.width, img.height), (4, 2));
        assert_eq!(img.pixel(0, 0)[0], 0);
        assert_eq!(img.pixel(3, 1)[0], 255);
        // Out-of-range changes or no changes: no panic.
        let mut b = Bilevel::new(5, 0, 256);
        b.push_changes(&[3, 2, 99]);
        b.push_changes(&[]);
        assert_eq!(b.lines(), 2);
        assert!(Bilevel::new(5, 0, 256).finish().is_none());
    }
}
