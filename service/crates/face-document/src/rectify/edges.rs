//! Canny-style edges: Sobel gradients, non-maximum suppression, hysteresis, dilation.

use super::raster::Gray;

/// Sobel gradients in grey levels per pixel (kernels divided by 8), border replicated.
#[derive(Clone, Debug)]
pub(crate) struct Gradients {
    pub(crate) gx: Gray,
    pub(crate) gy: Gray,
}

impl Gradients {
    pub(crate) fn sobel(g: &Gray) -> Gradients {
        let (w, h) = (g.w, g.h);
        let mut gx = Gray::new(w, h);
        let mut gy = Gray::new(w, h);
        if w == 0 || h == 0 {
            return Gradients { gx, gy };
        }
        for y in 0..h {
            let (ym, yp) = (y.saturating_sub(1), (y + 1).min(h - 1));
            for x in 0..w {
                let (xm, xp) = (x.saturating_sub(1), (x + 1).min(w - 1));
                let dx = (g.at(xp, ym) + 2.0 * g.at(xp, y) + g.at(xp, yp))
                    - (g.at(xm, ym) + 2.0 * g.at(xm, y) + g.at(xm, yp));
                let dy = (g.at(xm, yp) + 2.0 * g.at(x, yp) + g.at(xp, yp))
                    - (g.at(xm, ym) + 2.0 * g.at(x, ym) + g.at(xp, ym));
                gx.data[y * w + x] = dx / 8.0;
                gy.data[y * w + x] = dy / 8.0;
            }
        }
        Gradients { gx, gy }
    }

    pub(crate) fn width(&self) -> usize {
        self.gx.w
    }

    pub(crate) fn height(&self) -> usize {
        self.gx.h
    }

    /// Bilinear gradient at index coordinates.
    pub(crate) fn sample(&self, x: f64, y: f64) -> (f32, f32) {
        (self.gx.sample(x, y), self.gy.sample(x, y))
    }

    /// Gradient magnitude kept only at local maxima across the edge (4 quantised directions);
    /// zero elsewhere and on the image border.
    pub(crate) fn non_max_suppression(&self) -> Vec<f32> {
        let (w, h) = (self.width(), self.height());
        let mag: Vec<f32> = self
            .gx
            .data
            .iter()
            .zip(&self.gy.data)
            .map(|(x, y)| x.hypot(*y))
            .collect();
        let mut out = vec![0.0; w * h];
        if w < 3 || h < 3 {
            return out;
        }
        const TAN_22_5: f32 = 0.414_213_57;
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let i = y * w + x;
                let m = mag[i];
                if m <= 0.0 {
                    continue;
                }
                let (gx, gy) = (self.gx.data[i], self.gy.data[i]);
                let (ax, ay) = (gx.abs(), gy.abs());
                let (a, b) = if ay <= ax * TAN_22_5 {
                    (mag[i - 1], mag[i + 1])
                } else if ax <= ay * TAN_22_5 {
                    (mag[i - w], mag[i + w])
                } else if (gx > 0.0) == (gy > 0.0) {
                    (mag[i - w - 1], mag[i + w + 1])
                } else {
                    (mag[i - w + 1], mag[i + w - 1])
                };
                if m > a && m >= b {
                    out[i] = m;
                }
            }
        }
        out
    }
}

/// Hysteresis: pixels >= `high`, plus pixels >= `low` 8-connected to them.
pub(crate) fn hysteresis(nms: &[f32], w: usize, h: usize, low: f32, high: f32) -> Vec<bool> {
    let mut out = vec![false; w * h];
    let mut stack = Vec::new();
    for (start, &m) in nms.iter().enumerate() {
        if m < high || out[start] {
            continue;
        }
        out[start] = true;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    let n = ny * w + nx;
                    if !out[n] && nms[n] >= low {
                        out[n] = true;
                        stack.push(n);
                    }
                }
            }
        }
    }
    out
}

/// Binary dilation with a `(2r+1)` square.
pub(crate) fn dilate(mask: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    if w == 0 || h == 0 {
        return mask.to_vec();
    }
    let mut tmp = vec![false; w * h];
    for y in 0..h {
        let row = &mask[y * w..(y + 1) * w];
        for x in 0..w {
            let (lo, hi) = (x.saturating_sub(r), (x + r).min(w - 1));
            tmp[y * w + x] = row[lo..=hi].iter().any(|&b| b);
        }
    }
    let mut out = vec![false; w * h];
    for y in 0..h {
        let (lo, hi) = (y.saturating_sub(r), (y + r).min(h - 1));
        for x in 0..w {
            out[y * w + x] = (lo..=hi).any(|yy| tmp[yy * w + x]);
        }
    }
    out
}

/// The `p`-quantile (0-1) of `values` (reordered in place). `values` must not be empty.
pub(crate) fn percentile(values: &mut [f32], p: f32) -> f32 {
    let k = ((values.len() - 1) as f32 * p.clamp(0.0, 1.0)).round() as usize;
    *values.select_nth_unstable_by(k, f32::total_cmp).1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vertical_step(w: usize, h: usize, at: usize) -> Gray {
        let mut g = Gray::new(w, h);
        for y in 0..h {
            for x in at..w {
                g.data[y * w + x] = 120.0;
            }
        }
        g
    }

    #[test]
    fn sobel_measures_grey_levels_per_pixel_and_nms_thins_to_one_pixel() {
        let g = vertical_step(20, 9, 10).blur(1.0);
        let grad = Gradients::sobel(&g);
        let nms = grad.non_max_suppression();
        for y in 2..7 {
            let row: Vec<usize> = (0..20).filter(|&x| nms[y * 20 + x] > 1.0).collect();
            assert_eq!(row.len(), 1, "row {y}: {row:?}");
            assert!(row[0] == 9 || row[0] == 10);
        }
        // A blurred step of 120 levels has a peak gradient of roughly 120 / (sigma * sqrt(2 pi)).
        let peak = grad.gx.data.iter().fold(0.0f32, |a, v| a.max(*v));
        assert!((30.0..60.0).contains(&peak), "{peak}");
    }

    #[test]
    fn hysteresis_follows_weak_edges_connected_to_strong_ones() {
        let w = 6;
        let nms = vec![
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, //
            0.0, 9.0, 4.0, 4.0, 0.0, 0.0, //
            0.0, 0.0, 0.0, 0.0, 0.0, 4.0, //
        ];
        let e = hysteresis(&nms, w, 3, 3.0, 8.0);
        assert!(e[w + 1] && e[w + 2] && e[w + 3]);
        assert!(!e[2 * w + 5], "isolated weak pixel must not survive");
    }

    #[test]
    fn dilation_and_percentile() {
        let mut m = vec![false; 25];
        m[12] = true;
        let d = dilate(&m, 5, 5, 1);
        assert_eq!(d.iter().filter(|&&b| b).count(), 9);
        let mut v = vec![5.0, 1.0, 3.0, 2.0, 4.0];
        assert_eq!(percentile(&mut v, 0.5), 3.0);
        assert_eq!(percentile(&mut v, 1.0), 5.0);
    }
}
