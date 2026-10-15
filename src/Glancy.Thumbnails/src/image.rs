//! 8-bit RGBA image with straight (non-premultiplied) alpha: the common format between decoders and compositing.

#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// RGBA, row by row from the top, no padding.
    pub data: Vec<u8>,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Image({}x{})", self.width, self.height)
    }
}

impl Image {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height, data: vec![0; width as usize * height as usize * 4] }
    }

    pub fn filled(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        let mut data = Vec::with_capacity(width as usize * height as usize * 4);
        for _ in 0..width as usize * height as usize {
            data.extend_from_slice(&rgba);
        }
        Self { width, height, data }
    }

    pub fn from_rgba(width: u32, height: u32, data: Vec<u8>) -> Self {
        assert_eq!(data.len(), width as usize * height as usize * 4, "wrong RGBA buffer size");
        Self { width, height, data }
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [self.data[i], self.data[i + 1], self.data[i + 2], self.data[i + 3]]
    }

    pub fn set_pixel(&mut self, x: u32, y: u32, p: [u8; 4]) {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        self.data[i..i + 4].copy_from_slice(&p);
    }

    pub fn is_opaque(&self) -> bool {
        self.data.as_chunks::<4>().0.iter().all(|p| p[3] == 255)
    }

    /// Swaps R and B (for BGRA sources).
    pub fn swap_rb(&mut self) {
        for p in self.data.as_chunks_mut::<4>().0.iter_mut() {
            p.swap(0, 2);
        }
    }

    /// Converts premultiplied pixels to straight alpha.
    pub fn unpremultiply(&mut self) {
        for p in self.data.as_chunks_mut::<4>().0.iter_mut() {
            let a = p[3] as u32;
            if a != 0 && a != 255 {
                for c in &mut p[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
    }
}

/// ARGB color packed as in the registry (`0xAARRGGBB`) → RGBA.
pub fn argb_to_rgba(argb: u32) -> [u8; 4] {
    [(argb >> 16) as u8, (argb >> 8) as u8, argb as u8, (argb >> 24) as u8]
}

/// "Over" operator with straight alpha: `top` on top of `bottom`.
#[inline]
pub fn over(top: [u8; 4], bottom: [u8; 4]) -> [u8; 4] {
    let ta = top[3] as u32;
    if ta == 255 {
        return top;
    }
    if ta == 0 {
        return bottom;
    }
    let ba = bottom[3] as u32;
    // Work at a 255*255 scale so no precision is lost.
    let out_a = ta * 255 + ba * (255 - ta); // ×255
    if out_a == 0 {
        return [0, 0, 0, 0];
    }
    let mut out = [0u8; 4];
    for c in 0..3 {
        let v = top[c] as u32 * ta * 255 + bottom[c] as u32 * ba * (255 - ta);
        out[c] = ((v + out_a / 2) / out_a).min(255) as u8;
    }
    out[3] = ((out_a + 127) / 255) as u8;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn over_opaque_and_transparent() {
        assert_eq!(over([10, 20, 30, 255], [1, 2, 3, 255]), [10, 20, 30, 255]);
        assert_eq!(over([10, 20, 30, 0], [1, 2, 3, 255]), [1, 2, 3, 255]);
    }

    #[test]
    fn over_half_on_white() {
        let p = over([0, 0, 0, 128], [255, 255, 255, 255]);
        assert_eq!(p[3], 255);
        assert!((p[0] as i32 - 127).abs() <= 1, "{p:?}");
    }

    #[test]
    fn over_on_transparent_keeps_color() {
        assert_eq!(over([200, 100, 50, 64], [0, 0, 0, 0]), [200, 100, 50, 64]);
    }

    #[test]
    fn argb_conversion() {
        assert_eq!(argb_to_rgba(0x80FF2010), [0xFF, 0x20, 0x10, 0x80]);
    }

    #[test]
    fn unpremultiply_roundtrip() {
        let mut img = Image::from_rgba(1, 1, vec![50, 25, 0, 100]);
        img.unpremultiply();
        assert_eq!(img.pixel(0, 0), [128, 64, 0, 100]);
    }
}
