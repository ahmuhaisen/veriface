//! Single-channel working images: luma, Gaussian blur, bilinear sampling.

use face_imaging::RgbImage;

/// Single-channel f32 image, row-major. Index coordinates: pixel centres at integers.
#[derive(Clone, Debug)]
pub(crate) struct Gray {
    pub(crate) w: usize,
    pub(crate) h: usize,
    pub(crate) data: Vec<f32>,
}

impl Gray {
    pub(crate) fn new(w: usize, h: usize) -> Gray {
        Gray {
            w,
            h,
            data: vec![0.0; w * h],
        }
    }

    /// BT.601 luma.
    pub(crate) fn from_rgb(img: &RgbImage) -> Gray {
        let data = img
            .as_raw()
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| 0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2]))
            .collect();
        Gray {
            w: img.width() as usize,
            h: img.height() as usize,
            data,
        }
    }

    #[inline]
    pub(crate) fn at(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.w + x]
    }

    /// Bilinear sample at index coordinates, clamped to the border.
    pub(crate) fn sample(&self, x: f64, y: f64) -> f32 {
        if self.w == 0 || self.h == 0 {
            return 0.0;
        }
        let x = x.clamp(0.0, (self.w - 1) as f64);
        let y = y.clamp(0.0, (self.h - 1) as f64);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
        let top = self.at(x0, y0) * (1.0 - fx) + self.at(x1, y0) * fx;
        let bottom = self.at(x0, y1) * (1.0 - fx) + self.at(x1, y1) * fx;
        top * (1.0 - fy) + bottom * fy
    }

    /// Separable Gaussian blur, border replicated.
    pub(crate) fn blur(&self, sigma: f32) -> Gray {
        if sigma <= 0.0 || self.w == 0 || self.h == 0 {
            return self.clone();
        }
        let radius = (3.0 * sigma).ceil() as isize;
        let mut kernel: Vec<f32> = (-radius..=radius)
            .map(|i| (-0.5 * (i as f32 / sigma).powi(2)).exp())
            .collect();
        let sum: f32 = kernel.iter().sum();
        kernel.iter_mut().for_each(|k| *k /= sum);

        // Each tap sum runs in kernel order from -0.0, exactly like `Iterator::sum` did, so the
        // fast interior paths give bit-identical results to the clamped edge paths.
        let (w, h) = (self.w, self.h);
        let r = radius as usize;
        let taps = kernel.len();
        let clamp = |i: isize, n: usize| i.clamp(0, n as isize - 1) as usize;
        let mut tmp = Gray::new(w, h);
        for (row, out) in self.data.chunks_exact(w).zip(tmp.data.chunks_exact_mut(w)) {
            for (x, o) in out.iter_mut().enumerate() {
                *o = if x >= r && x + r < w {
                    let mut acc = -0.0f32;
                    for (kv, v) in kernel.iter().zip(&row[x - r..x - r + taps]) {
                        acc += kv * v;
                    }
                    acc
                } else {
                    let mut acc = -0.0f32;
                    for (k, kv) in kernel.iter().enumerate() {
                        acc += kv * row[clamp(x as isize + k as isize - radius, w)];
                    }
                    acc
                };
            }
        }
        let mut out = Gray::new(w, h);
        for (y, out_row) in out.data.chunks_exact_mut(w).enumerate() {
            out_row.fill(-0.0);
            for (k, kv) in kernel.iter().enumerate() {
                let yy = clamp(y as isize + k as isize - radius, h);
                let src = &tmp.data[yy * w..(yy + 1) * w];
                for (o, v) in out_row.iter_mut().zip(src) {
                    *o += kv * v;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_preserves_constant_images_and_mean() {
        let mut g = Gray::new(20, 10);
        g.data.iter_mut().for_each(|v| *v = 77.0);
        let b = g.blur(1.5);
        assert!(b.data.iter().all(|v| (v - 77.0).abs() < 1e-3));

        let mut step = Gray::new(40, 5);
        for y in 0..5 {
            for x in 20..40 {
                step.data[y * 40 + x] = 100.0;
            }
        }
        let b = step.blur(2.0);
        // Symmetric around the step: halfway value between the two pixels at the edge.
        let mid = 0.5 * (b.at(19, 2) + b.at(20, 2));
        assert!((mid - 50.0).abs() < 1.0, "{mid}");
        assert!(b.at(0, 2) < 0.01 && (b.at(39, 2) - 100.0).abs() < 0.01);
    }

    /// The original formulation (clamped taps summed with `Iterator::sum`).
    fn reference_blur(g: &Gray, sigma: f32) -> Gray {
        let radius = (3.0 * sigma).ceil() as isize;
        let mut kernel: Vec<f32> = (-radius..=radius)
            .map(|i| (-0.5 * (i as f32 / sigma).powi(2)).exp())
            .collect();
        let sum: f32 = kernel.iter().sum();
        kernel.iter_mut().for_each(|k| *k /= sum);
        let (w, h) = (g.w as isize, g.h as isize);
        let mut tmp = Gray::new(g.w, g.h);
        for y in 0..h {
            for x in 0..w {
                tmp.data[(y * w + x) as usize] = kernel
                    .iter()
                    .enumerate()
                    .map(|(k, kv)| {
                        kv * g.data[(y * w + (x + k as isize - radius).clamp(0, w - 1)) as usize]
                    })
                    .sum();
            }
        }
        let mut out = Gray::new(g.w, g.h);
        for y in 0..h {
            for x in 0..w {
                out.data[(y * w + x) as usize] = kernel
                    .iter()
                    .enumerate()
                    .map(|(k, kv)| {
                        kv * tmp.data[((y + k as isize - radius).clamp(0, h - 1) * w + x) as usize]
                    })
                    .sum();
            }
        }
        out
    }

    #[test]
    fn fast_blur_is_bit_identical_to_the_reference() {
        let mut g = Gray::new(37, 23);
        for (i, v) in g.data.iter_mut().enumerate() {
            *v = ((i * 7919) % 256) as f32 * 0.73;
        }
        for sigma in [0.6, 1.5, 4.0] {
            let (a, b) = (g.blur(sigma), reference_blur(&g, sigma));
            assert!(
                a.data
                    .iter()
                    .zip(&b.data)
                    .all(|(x, y)| x.to_bits() == y.to_bits()),
                "sigma {sigma}"
            );
        }
    }

    #[test]
    fn bilinear_sample_interpolates_and_clamps() {
        let g = Gray {
            w: 2,
            h: 2,
            data: vec![0.0, 10.0, 20.0, 30.0],
        };
        assert!((g.sample(0.5, 0.5) - 15.0).abs() < 1e-5);
        assert!((g.sample(-3.0, 0.0) - 0.0).abs() < 1e-5);
        assert!((g.sample(5.0, 5.0) - 30.0).abs() < 1e-5);
    }
}
