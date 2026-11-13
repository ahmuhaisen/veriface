//! Structural similarity (Wang, Bovik, Sheikh and Simoncelli 2004).

use face_imaging::RgbImage;

const WINDOW_RADIUS: usize = 5;
const WINDOW_SIGMA: f64 = 1.5;
const C1: f64 = (0.01 * 255.0) * (0.01 * 255.0);
const C2: f64 = (0.03 * 255.0) * (0.03 * 255.0);

/// Mean SSIM of the BT.601 luma of two equal-size images: 11x11 Gaussian window (sigma 1.5),
/// K1 = 0.01, K2 = 0.03, L = 255, over the region where the window fits. `None` when the sizes
/// differ or the images are smaller than the window.
pub fn ssim(a: &RgbImage, b: &RgbImage) -> Option<f64> {
    let (w, h) = (a.width() as usize, a.height() as usize);
    let size = 2 * WINDOW_RADIUS + 1;
    if a.dimensions() != b.dimensions() || w < size || h < size {
        return None;
    }
    let (la, lb) = (luma(a), luma(b));
    let product = |x: &[f64], y: &[f64]| x.iter().zip(y).map(|(p, q)| p * q).collect::<Vec<_>>();
    let mu_a = filter(&la, w, h);
    let mu_b = filter(&lb, w, h);
    let aa = filter(&product(&la, &la), w, h);
    let bb = filter(&product(&lb, &lb), w, h);
    let ab = filter(&product(&la, &lb), w, h);
    let n = mu_a.len();
    let total: f64 = (0..n)
        .map(|i| {
            let (ma, mb) = (mu_a[i], mu_b[i]);
            let va = aa[i] - ma * ma;
            let vb = bb[i] - mb * mb;
            let cov = ab[i] - ma * mb;
            ((2.0 * ma * mb + C1) * (2.0 * cov + C2)) / ((ma * ma + mb * mb + C1) * (va + vb + C2))
        })
        .sum();
    Some(total / n as f64)
}

fn luma(img: &RgbImage) -> Vec<f64> {
    img.as_raw()
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| 0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2]))
        .collect()
}

/// Separable Gaussian filter, "valid" region only: (w - 10) x (h - 10) values.
fn filter(src: &[f64], w: usize, h: usize) -> Vec<f64> {
    let size = 2 * WINDOW_RADIUS + 1;
    let mut kernel: Vec<f64> = (0..size)
        .map(|i| {
            let d = i as f64 - WINDOW_RADIUS as f64;
            (-0.5 * (d / WINDOW_SIGMA).powi(2)).exp()
        })
        .collect();
    let sum: f64 = kernel.iter().sum();
    kernel.iter_mut().for_each(|k| *k /= sum);
    let (ow, oh) = (w - size + 1, h - size + 1);
    let mut horizontal = vec![0.0; ow * h];
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        for x in 0..ow {
            horizontal[y * ow + x] = kernel
                .iter()
                .zip(&row[x..x + size])
                .map(|(k, v)| k * v)
                .sum();
        }
    }
    let mut out = vec![0.0; ow * oh];
    for y in 0..oh {
        for x in 0..ow {
            out[y * ow + x] = kernel
                .iter()
                .enumerate()
                .map(|(k, kv)| kv * horizontal[(y + k) * ow + x])
                .sum();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use face_document::testing::Rng;
    use image::Rgb;

    fn noise(seed: u64, w: u32, h: u32) -> RgbImage {
        let mut rng = Rng::new(seed);
        RgbImage::from_fn(w, h, |x, y| {
            let base = 128.0 + 60.0 * ((x as f64) * 0.2).sin() * ((y as f64) * 0.15).cos();
            Rgb([(base + rng.range(-30.0, 30.0)).clamp(0.0, 255.0) as u8; 3])
        })
    }

    #[test]
    fn identical_images_score_one_and_different_ones_less() {
        let a = noise(1, 64, 48);
        assert!((ssim(&a, &a).unwrap() - 1.0).abs() < 1e-9);
        let b = noise(2, 64, 48);
        let s = ssim(&a, &b).unwrap();
        assert!(s < 0.9 && s > 0.0, "{s}");
        let inverted = RgbImage::from_fn(64, 48, |x, y| {
            let p = a.get_pixel(x, y).0;
            Rgb(p.map(|v| 255 - v))
        });
        assert!(ssim(&a, &inverted).unwrap() < 0.0);
        assert!(ssim(&a, &RgbImage::new(64, 40)).is_none());
        assert!(ssim(&RgbImage::new(8, 8), &RgbImage::new(8, 8)).is_none());
    }
}
