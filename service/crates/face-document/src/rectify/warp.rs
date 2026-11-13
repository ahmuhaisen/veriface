//! Resampling: the projective warp used for rectification (continuous pixel coordinates, sampled
//! from the full-resolution source) and size helpers. Downscaling uses `face-imaging`.

use face_imaging::RgbImage;
use face_imaging::transform::{Filter, resize};

use super::projective::Projective;

/// Warp `src` into a `width` x `height` image. Output pixel `(i, j)` takes the bilinear sample of
/// `src` at `out_to_src((i + 0.5, j + 0.5))`, in continuous pixel coordinates. Samples that fall
/// outside `src` are clamped to its border.
pub fn warp_projective(
    src: &RgbImage,
    out_to_src: &Projective,
    width: u32,
    height: u32,
) -> RgbImage {
    let mut out = RgbImage::new(width, height);
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    if sw == 0 || sh == 0 || width == 0 || height == 0 {
        return out;
    }
    let raw = src.as_raw();
    let m = &out_to_src.0;
    for (j, row) in out.chunks_exact_mut(width as usize * 3).enumerate() {
        let y = j as f64 + 0.5;
        for (i, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let x = i as f64 + 0.5;
            let w = m[2][0] * x + m[2][1] * y + m[2][2];
            if w.abs() < 1e-12 {
                continue;
            }
            let sx = (m[0][0] * x + m[0][1] * y + m[0][2]) / w - 0.5;
            let sy = (m[1][0] * x + m[1][1] * y + m[1][2]) / w - 0.5;
            bilinear_clamped(raw, sw, sh, sx, sy, px);
        }
    }
    out
}

fn bilinear_clamped(raw: &[u8], w: usize, h: usize, x: f64, y: f64, out: &mut [u8]) {
    if !x.is_finite() || !y.is_finite() {
        return;
    }
    let x = x.clamp(0.0, (w - 1) as f64);
    let y = y.clamp(0.0, (h - 1) as f64);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f64, y - y0 as f64);
    let (i00, i10, i01, i11) = (
        (y0 * w + x0) * 3,
        (y0 * w + x1) * 3,
        (y1 * w + x0) * 3,
        (y1 * w + x1) * 3,
    );
    for (c, o) in out.iter_mut().enumerate() {
        let top = f64::from(raw[i00 + c]) * (1.0 - fx) + f64::from(raw[i10 + c]) * fx;
        let bottom = f64::from(raw[i01 + c]) * (1.0 - fx) + f64::from(raw[i11 + c]) * fx;
        *o = (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8;
    }
}

/// Area-average downscale (`face_imaging::transform::resize` with the area filter). The target
/// size is clamped to `[1, source size]`, so this never upscales.
pub(crate) fn downscale_area(src: &RgbImage, dst_w: u32, dst_h: u32) -> RgbImage {
    let w = dst_w.clamp(1, src.width().max(1));
    let h = dst_h.clamp(1, src.height().max(1));
    resize(src, w, h, Filter::Area)
}

/// The largest size with the aspect ratio of `width` x `height` that fits in `max_w` x `max_h`,
/// never larger than the original.
pub(crate) fn fit_within(width: u32, height: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if width <= max_w && height <= max_h {
        return (width, height);
    }
    let s = (f64::from(max_w) / f64::from(width)).min(f64::from(max_h) / f64::from(height));
    (
        ((f64::from(width) * s).floor() as u32).max(1),
        ((f64::from(height) * s).floor() as u32).max(1),
    )
}

/// The size with the long side at most `long_side`, keeping the aspect ratio; never upscales.
pub(crate) fn fit_long_side(width: u32, height: u32, long_side: u32) -> (u32, u32) {
    let long = width.max(height);
    if long <= long_side {
        return (width, height);
    }
    let s = f64::from(long_side) / f64::from(long);
    (
        ((f64::from(width) * s).round() as u32).max(1),
        ((f64::from(height) * s).round() as u32).max(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn area_downscale_averages_blocks() {
        let mut img = RgbImage::new(4, 2);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = Rgb([if (x + y) % 2 == 0 { 200 } else { 0 }, 50, (x * 60) as u8]);
        }
        let small = downscale_area(&img, 2, 1);
        assert_eq!(small.dimensions(), (2, 1));
        assert_eq!(small.get_pixel(0, 0).0, [100, 50, 30]);
        assert_eq!(small.get_pixel(1, 0).0, [100, 50, 150]);
        // Non-integer factor keeps the mean.
        let flat = RgbImage::from_pixel(7, 5, Rgb([90, 90, 90]));
        let s = downscale_area(&flat, 3, 2);
        assert!(s.pixels().all(|p| p.0 == [90, 90, 90]));
        // Never upscales.
        assert_eq!(downscale_area(&flat, 70, 50).dimensions(), (7, 5));
    }

    #[test]
    fn identity_warp_copies_pixels_exactly() {
        let mut img = RgbImage::new(5, 4);
        for (i, p) in img.pixels_mut().enumerate() {
            *p = Rgb([i as u8 * 7, 255 - i as u8 * 3, 9]);
        }
        let out = warp_projective(&img, &Projective::IDENTITY, 5, 4);
        assert_eq!(out, img);
        let shifted = warp_projective(&img, &Projective::translate(2.0, 1.0), 3, 3);
        assert_eq!(shifted.get_pixel(0, 0), img.get_pixel(2, 1));
        assert_eq!(shifted.get_pixel(2, 2), img.get_pixel(4, 3));
    }

    #[test]
    fn size_helpers_never_upscale() {
        assert_eq!(fit_within(100, 200, 360, 480), (100, 200));
        assert_eq!(fit_within(720, 960, 360, 480), (360, 480));
        assert_eq!(fit_long_side(4000, 3000, 1024), (1024, 768));
        assert_eq!(fit_long_side(800, 600, 1024), (800, 600));
    }
}
