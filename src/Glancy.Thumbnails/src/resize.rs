//! High-quality resampling with correct alpha (filtered in premultiplied space, so no halos appear).

use crate::image::Image;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    Nearest,
    Linear,
    Cubic,
}

impl Filter {
    fn support(self) -> f32 {
        match self {
            Filter::Nearest => 0.5,
            Filter::Linear => 1.0,
            Filter::Cubic => 2.0,
        }
    }

    fn eval(self, x: f32) -> f32 {
        let x = x.abs();
        match self {
            Filter::Nearest => {
                if x < 0.5 { 1.0 } else { 0.0 }
            }
            Filter::Linear => {
                if x < 1.0 { 1.0 - x } else { 0.0 }
            }
            Filter::Cubic => {
                // Catmull-Rom (a = -0.5), the usual "bicubic".
                const A: f32 = -0.5;
                if x < 1.0 {
                    ((A + 2.0) * x - (A + 3.0)) * x * x + 1.0
                } else if x < 2.0 {
                    (((x - 5.0) * x + 8.0) * x - 4.0) * A
                } else {
                    0.0
                }
            }
        }
    }
}

/// Size that fits in a `max`×`max` box preserving the aspect ratio. `upscale` allows enlarging.
pub fn fit(width: u32, height: u32, max: u32, upscale: bool) -> (u32, u32) {
    if width == 0 || height == 0 || max == 0 {
        return (width.max(1).min(max.max(1)), height.max(1).min(max.max(1)));
    }
    let longest = width.max(height);
    if longest <= max && !upscale {
        return (width, height);
    }
    let s = max as f64 / longest as f64;
    let w = ((width as f64 * s).round() as u32).clamp(1, max);
    let h = ((height as f64 * s).round() as u32).clamp(1, max);
    (w, h)
}

pub fn resize(src: &Image, width: u32, height: u32, filter: Filter) -> Image {
    let width = width.max(1);
    let height = height.max(1);
    if src.width == width && src.height == height {
        return src.clone();
    }
    if filter == Filter::Nearest {
        return nearest(src, width, height);
    }
    // Large reductions: first box downsampling by whole blocks (fast and exact) down to ~2× the target,
    // then the fine filter. This is what Pillow does with "reducing_gap".
    let kx = (src.width / width.saturating_mul(2)).max(1);
    let ky = (src.height / height.saturating_mul(2)).max(1);
    // Box downsampling works on the original bytes: a full-size f32 copy never exists.
    let (buf, w, h) = if kx > 1 || ky > 1 { box_reduce(src, kx, ky) } else { (premultiplied(src), src.width, src.height) };
    let horiz = if w != width { convolve_h(&buf, w, h, width, filter) } else { buf };
    let vert = if h != height { convolve_v(&horiz, width, h, height, filter) } else { horiz };
    from_premultiplied(&vert, width, height)
}

fn nearest(src: &Image, width: u32, height: u32) -> Image {
    let mut out = Image::new(width, height);
    for y in 0..height {
        let sy = ((y as u64 * 2 + 1) * src.height as u64 / (height as u64 * 2)).min(src.height as u64 - 1) as u32;
        for x in 0..width {
            let sx = ((x as u64 * 2 + 1) * src.width as u64 / (width as u64 * 2)).min(src.width as u64 - 1) as u32;
            out.set_pixel(x, y, src.pixel(sx, sy));
        }
    }
    out
}

fn premultiplied(src: &Image) -> Vec<f32> {
    let mut out = Vec::with_capacity(src.data.len());
    for p in src.data.as_chunks::<4>().0.iter() {
        let a = p[3] as f32;
        let k = a / 255.0;
        out.extend_from_slice(&[p[0] as f32 * k, p[1] as f32 * k, p[2] as f32 * k, a]);
    }
    out
}

fn from_premultiplied(buf: &[f32], width: u32, height: u32) -> Image {
    let mut data = Vec::with_capacity(width as usize * height as usize * 4);
    for p in buf.as_chunks::<4>().0.iter() {
        let a = p[3].clamp(0.0, 255.0);
        if a < 0.5 {
            data.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        let k = 255.0 / a;
        for c in &p[..3] {
            data.push((c * k).round().clamp(0.0, 255.0) as u8);
        }
        data.push(a.round() as u8);
    }
    Image::from_rgba(width, height, data)
}

/// `kx`×`ky` box downsampling in premultiplied space, directly from 8-bit RGBA.
fn box_reduce(src: &Image, kx: u32, ky: u32) -> (Vec<f32>, u32, u32) {
    let (w, h) = (src.width as usize, src.height as usize);
    let nw = src.width.div_ceil(kx);
    let nh = src.height.div_ceil(ky);
    let mut out = vec![0f32; nw as usize * nh as usize * 4];
    let mut counts = vec![0u32; nw as usize * nh as usize];
    for y in 0..h {
        let oy = y / ky as usize;
        let row = &src.data[y * w * 4..(y + 1) * w * 4];
        for (x, p) in row.as_chunks::<4>().0.iter().enumerate() {
            let o = oy * nw as usize + x / kx as usize;
            let a = p[3] as f32;
            let k = a / 255.0;
            out[o * 4] += p[0] as f32 * k;
            out[o * 4 + 1] += p[1] as f32 * k;
            out[o * 4 + 2] += p[2] as f32 * k;
            out[o * 4 + 3] += a;
            counts[o] += 1;
        }
    }
    for (o, n) in counts.iter().enumerate() {
        let n = *n as f32;
        for c in 0..4 {
            out[o * 4 + c] /= n;
        }
    }
    (out, nw, nh)
}

/// Coefficients for one axis: for each output, the first input index and normalized weights.
fn coefficients(in_size: u32, out_size: u32, filter: Filter) -> Vec<(usize, Vec<f32>)> {
    let scale = in_size as f32 / out_size as f32;
    let filterscale = scale.max(1.0);
    let support = filter.support() * filterscale;
    (0..out_size)
        .map(|o| {
            let center = (o as f32 + 0.5) * scale;
            let min = ((center - support + 0.5).floor().max(0.0)) as usize;
            let max = ((center + support + 0.5).floor() as usize).min(in_size as usize).max(min + 1);
            let mut w: Vec<f32> = (min..max).map(|i| filter.eval((i as f32 - center + 0.5) / filterscale)).collect();
            let sum: f32 = w.iter().sum();
            if sum.abs() > f32::EPSILON {
                for v in &mut w {
                    *v /= sum;
                }
            } else {
                // No useful weights (should not happen): nearest sample.
                w.iter_mut().for_each(|v| *v = 0.0);
                let nearest = ((center as usize).min(in_size as usize - 1)).saturating_sub(min).min(w.len() - 1);
                w[nearest] = 1.0;
            }
            (min, w)
        })
        .collect()
}

fn convolve_h(buf: &[f32], w: u32, h: u32, out_w: u32, filter: Filter) -> Vec<f32> {
    let coeffs = coefficients(w, out_w, filter);
    let mut out = vec![0f32; out_w as usize * h as usize * 4];
    for y in 0..h as usize {
        let row = &buf[y * w as usize * 4..(y + 1) * w as usize * 4];
        let orow = &mut out[y * out_w as usize * 4..(y + 1) * out_w as usize * 4];
        for (x, (start, weights)) in coeffs.iter().enumerate() {
            let mut acc = [0f32; 4];
            for (k, wt) in weights.iter().enumerate() {
                let i = (start + k) * 4;
                for c in 0..4 {
                    acc[c] += row[i + c] * wt;
                }
            }
            orow[x * 4..x * 4 + 4].copy_from_slice(&acc);
        }
    }
    out
}

fn convolve_v(buf: &[f32], w: u32, h: u32, out_h: u32, filter: Filter) -> Vec<f32> {
    let coeffs = coefficients(h, out_h, filter);
    let stride = w as usize * 4;
    let mut out = vec![0f32; stride * out_h as usize];
    for (y, (start, weights)) in coeffs.iter().enumerate() {
        let orow = &mut out[y * stride..(y + 1) * stride];
        for (k, wt) in weights.iter().enumerate() {
            let row = &buf[(start + k) * stride..(start + k + 1) * stride];
            for (o, v) in orow.iter_mut().zip(row) {
                *o += v * wt;
            }
        }
    }
    // Bicubic can go out of range; premultiplied color must never exceed alpha.
    for p in out.as_chunks_mut::<4>().0.iter_mut() {
        p[3] = p[3].clamp(0.0, 255.0);
        for c in 0..3 {
            p[c] = p[c].clamp(0.0, p[3]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_aspect() {
        assert_eq!(fit(1000, 500, 256, false), (256, 128));
        assert_eq!(fit(500, 1000, 256, false), (128, 256));
        assert_eq!(fit(100, 50, 256, false), (100, 50));
        assert_eq!(fit(100, 50, 256, true), (256, 128));
        assert_eq!(fit(10000, 1, 256, false), (256, 1));
    }

    #[test]
    fn uniform_color_stays_uniform() {
        for f in [Filter::Nearest, Filter::Linear, Filter::Cubic] {
            let src = Image::filled(301, 157, [10, 200, 30, 255]);
            let out = resize(&src, 64, 33, f);
            assert_eq!((out.width, out.height), (64, 33));
            for p in out.data.as_chunks::<4>().0.iter() {
                assert_eq!(*p, [10, 200, 30, 255], "{f:?}");
            }
        }
    }

    #[test]
    fn transparent_pixels_do_not_bleed_color() {
        // Half opaque red, half fully transparent green: no green may appear when downscaling.
        let mut src = Image::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                src.set_pixel(x, y, if x < 32 { [255, 0, 0, 255] } else { [0, 255, 0, 0] });
            }
        }
        let out = resize(&src, 8, 8, Filter::Cubic);
        for p in out.data.as_chunks::<4>().0.iter() {
            if p[3] > 0 {
                assert!(p[1] < 3, "green halo: {p:?}");
            }
        }
        // The boundary ends up semi-transparent.
        assert_eq!(out.pixel(1, 4)[3], 255);
        assert!(out.pixel(7, 4)[3] < 10);
    }

    #[test]
    fn huge_reduction_averages() {
        // 1 px white/black checkerboard downscaled to 4×4 → mid gray.
        let mut src = Image::new(400, 400);
        for y in 0..400 {
            for x in 0..400 {
                let v = if (x + y) % 2 == 0 { 255 } else { 0 };
                src.set_pixel(x, y, [v, v, v, 255]);
            }
        }
        let out = resize(&src, 4, 4, Filter::Linear);
        for p in out.data.as_chunks::<4>().0.iter() {
            assert!((p[0] as i32 - 128).abs() <= 2, "{p:?}");
        }
    }

    #[test]
    fn upscale_works() {
        let src = Image::filled(2, 2, [1, 2, 3, 255]);
        let out = resize(&src, 10, 10, Filter::Cubic);
        assert_eq!(out.pixel(5, 5), [1, 2, 3, 255]);
    }
}
