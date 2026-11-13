//! Luminance, masks, histograms, gradients and frequency helpers.
//! Owner: vision agent.
//!
//! # Luma and luminance
//!
//! - **Contract luma** ([`luminance8`]): `definitions.luma` in `contracts/checks.yaml`, OFIQ's
//!   luminance: each sRGB channel linearised (IEC 61966-2-1), `Y = 0.2126 R + 0.7152 G + 0.0722 B`,
//!   stored as `floor(255 Y + 0.5)`. Used by luminance mean, exposure, dynamic range and
//!   illumination uniformity. [`luminance`] is the unquantised `Y` (0-1).
//! - **Gamma luma `Y'`** ([`luma`], [`luma8`]): BT.601 `0.299 R' + 0.587 G' + 0.114 B'` on the
//!   8-bit values, 0-255. Used for texture measures (sharpness, background, edges).
//!
//! Planes ([`Plane`]) and masks ([`Mask`]) are row-major with pixel centres at integer
//! coordinates, like the rest of `face-imaging`.

use image::RgbImage;

use crate::geom::Point;

/// sRGB-encoded 8-bit value to linear light (IEC 61966-2-1).
pub fn srgb_to_linear(v: u8) -> f32 {
    srgb_lut()[usize::from(v)]
}

fn srgb_lut() -> &'static [f32; 256] {
    static LUT: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        let mut lut = [0.0f32; 256];
        for (i, v) in lut.iter_mut().enumerate() {
            let x = i as f64 / 255.0;
            let y = if x <= 0.04045 {
                x / 12.92
            } else {
                ((x + 0.055) / 1.055).powf(2.4)
            };
            *v = y as f32;
        }
        lut
    })
}

// ----------------------------------------------------------------------------------- planes

/// A single-channel `f32` image.
#[derive(Clone, Debug, PartialEq)]
pub struct Plane {
    width: u32,
    height: u32,
    data: Vec<f32>,
}

impl Plane {
    /// A plane filled with `value`.
    pub fn filled(width: u32, height: u32, value: f32) -> Plane {
        Plane {
            width,
            height,
            data: vec![value; width as usize * height as usize],
        }
    }

    /// Wrap row-major data. `None` when the length does not match.
    pub fn from_vec(width: u32, height: u32, data: Vec<f32>) -> Option<Plane> {
        (data.len() == width as usize * height as usize).then_some(Plane {
            width,
            height,
            data,
        })
    }

    pub fn from_fn(width: u32, height: u32, mut f: impl FnMut(u32, u32) -> f32) -> Plane {
        let mut data = Vec::with_capacity(width as usize * height as usize);
        for y in 0..height {
            for x in 0..width {
                data.push(f(x, y));
            }
        }
        Plane {
            width,
            height,
            data,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn as_slice(&self) -> &[f32] {
        &self.data
    }

    pub fn as_mut_slice(&mut self) -> &mut [f32] {
        &mut self.data
    }

    pub fn into_vec(self) -> Vec<f32> {
        self.data
    }

    /// Value at `(x, y)`; panics when out of bounds.
    pub fn get(&self, x: u32, y: u32) -> f32 {
        self.data[y as usize * self.width as usize + x as usize]
    }

    pub fn set(&mut self, x: u32, y: u32, v: f32) {
        let w = self.width as usize;
        self.data[y as usize * w + x as usize] = v;
    }

    /// Value with coordinates clamped into the plane (replicate border).
    pub fn get_clamped(&self, x: i64, y: i64) -> f32 {
        let xi = x.clamp(0, i64::from(self.width) - 1) as usize;
        let yi = y.clamp(0, i64::from(self.height) - 1) as usize;
        self.data[yi * self.width as usize + xi]
    }

    pub fn row(&self, y: u32) -> &[f32] {
        let w = self.width as usize;
        &self.data[y as usize * w..(y as usize + 1) * w]
    }
}

/// Relative luminance `Y` in 0-1 (linear light). See the module docs.
pub fn luminance(img: &RgbImage) -> Plane {
    let lut = srgb_lut();
    let data = img
        .pixels()
        .map(|p| {
            0.2126 * lut[usize::from(p[0])]
                + 0.7152 * lut[usize::from(p[1])]
                + 0.0722 * lut[usize::from(p[2])]
        })
        .collect();
    Plane {
        width: img.width(),
        height: img.height(),
        data,
    }
}

/// Relative luminance quantised to 8 bits as OFIQ does: `floor(255 Y + 0.5)`, row-major.
pub fn luminance8(img: &RgbImage) -> Vec<u8> {
    let lut = srgb_lut();
    img.pixels()
        .map(|p| {
            let y = 0.2126 * f64::from(lut[usize::from(p[0])])
                + 0.7152 * f64::from(lut[usize::from(p[1])])
                + 0.0722 * f64::from(lut[usize::from(p[2])]);
            (y * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8
        })
        .collect()
}

/// BT.601 luma of one pixel, 0-255 (not rounded).
#[inline]
pub fn luma_of(r: u8, g: u8, b: u8) -> f32 {
    0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b)
}

/// Luma `Y'` (BT.601) in 0-255. See the module docs.
pub fn luma(img: &RgbImage) -> Plane {
    let data = img.pixels().map(|p| luma_of(p[0], p[1], p[2])).collect();
    Plane {
        width: img.width(),
        height: img.height(),
        data,
    }
}

/// Luma `Y'` (BT.601) rounded to 8 bits, row-major.
pub fn luma8(img: &RgbImage) -> Vec<u8> {
    img.pixels()
        .map(|p| luma_of(p[0], p[1], p[2]).round().clamp(0.0, 255.0) as u8)
        .collect()
}

// ------------------------------------------------------------------------------------ masks

/// A binary mask.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    width: u32,
    height: u32,
    data: Vec<bool>,
}

impl Mask {
    /// All pixels off.
    pub fn empty(width: u32, height: u32) -> Mask {
        Mask {
            width,
            height,
            data: vec![false; width as usize * height as usize],
        }
    }

    /// All pixels on.
    pub fn full(width: u32, height: u32) -> Mask {
        Mask {
            width,
            height,
            data: vec![true; width as usize * height as usize],
        }
    }

    pub fn from_fn(width: u32, height: u32, mut f: impl FnMut(u32, u32) -> bool) -> Mask {
        let mut data = Vec::with_capacity(width as usize * height as usize);
        for y in 0..height {
            for x in 0..width {
                data.push(f(x, y));
            }
        }
        Mask {
            width,
            height,
            data,
        }
    }

    /// Pixels whose centre lies inside `polygon` (even-odd rule; vertices in pixel coordinates).
    pub fn from_polygon(width: u32, height: u32, polygon: &[Point]) -> Mask {
        let mut mask = Mask::empty(width, height);
        if polygon.len() < 3 {
            return mask;
        }
        let (mut ymin, mut ymax) = (f32::MAX, f32::MIN);
        for p in polygon {
            ymin = ymin.min(p.y);
            ymax = ymax.max(p.y);
        }
        if !(ymin.is_finite() && ymax.is_finite()) {
            return mask;
        }
        let y0 = ymin.ceil().max(0.0) as u32;
        let y1 = (ymax.floor().min(height as f32 - 1.0)).max(-1.0);
        if y1 < 0.0 {
            return mask;
        }
        let mut xs: Vec<f32> = Vec::new();
        for y in y0..=(y1 as u32) {
            let yc = y as f32;
            xs.clear();
            for i in 0..polygon.len() {
                let (a, b) = (polygon[i], polygon[(i + 1) % polygon.len()]);
                // Half-open rule so shared vertices are counted once.
                if (a.y <= yc && b.y > yc) || (b.y <= yc && a.y > yc) {
                    let t = (yc - a.y) / (b.y - a.y);
                    xs.push(a.x + t * (b.x - a.x));
                }
            }
            xs.sort_by(f32::total_cmp);
            for pair in xs.as_chunks::<2>().0 {
                let xa = pair[0].ceil().max(0.0);
                let xb = pair[1].floor().min(width as f32 - 1.0);
                if xb < xa {
                    continue;
                }
                for x in xa as u32..=xb as u32 {
                    mask.set(x, y, true);
                }
            }
        }
        mask
    }

    /// Pixels whose centre lies inside the ellipse with the given centre, semi-axes and rotation
    /// (radians, clockwise in the image; `semi_x` is along the rotated x axis).
    pub fn from_ellipse(
        width: u32,
        height: u32,
        center: Point,
        semi_x: f32,
        semi_y: f32,
        rotation: f32,
    ) -> Mask {
        if semi_x <= 0.0 || semi_y <= 0.0 || !center.x.is_finite() || !center.y.is_finite() {
            return Mask::empty(width, height);
        }
        let (cos, sin) = (rotation.cos(), rotation.sin());
        let r = semi_x.max(semi_y);
        let x0 = (center.x - r).floor().max(0.0) as u32;
        let y0 = (center.y - r).floor().max(0.0) as u32;
        let x1 = ((center.x + r).floor().max(-1.0) + 1.0).min(width as f32) as u32;
        let y1 = ((center.y + r).floor().max(-1.0) + 1.0).min(height as f32) as u32;
        let mut mask = Mask::empty(width, height);
        for y in y0..y1 {
            for x in x0..x1 {
                let (dx, dy) = (x as f32 - center.x, y as f32 - center.y);
                let u = dx * cos + dy * sin;
                let v = -dx * sin + dy * cos;
                if (u / semi_x).powi(2) + (v / semi_y).powi(2) <= 1.0 {
                    mask.set(x, y, true);
                }
            }
        }
        mask
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn as_slice(&self) -> &[bool] {
        &self.data
    }

    pub fn get(&self, x: u32, y: u32) -> bool {
        self.data[y as usize * self.width as usize + x as usize]
    }

    pub fn set(&mut self, x: u32, y: u32, v: bool) {
        let w = self.width as usize;
        self.data[y as usize * w + x as usize] = v;
    }

    /// Number of pixels that are on.
    pub fn count(&self) -> usize {
        self.data.iter().filter(|v| **v).count()
    }

    fn zip_with(&self, other: &Mask, f: impl Fn(bool, bool) -> bool) -> Mask {
        assert_eq!(
            (self.width, self.height),
            (other.width, other.height),
            "mask sizes differ"
        );
        Mask {
            width: self.width,
            height: self.height,
            data: self
                .data
                .iter()
                .zip(&other.data)
                .map(|(a, b)| f(*a, *b))
                .collect(),
        }
    }

    /// Intersection. Panics when the sizes differ.
    pub fn and(&self, other: &Mask) -> Mask {
        self.zip_with(other, |a, b| a && b)
    }

    /// Union. Panics when the sizes differ.
    pub fn or(&self, other: &Mask) -> Mask {
        self.zip_with(other, |a, b| a || b)
    }

    /// `self` without `other`. Panics when the sizes differ.
    pub fn and_not(&self, other: &Mask) -> Mask {
        self.zip_with(other, |a, b| a && !b)
    }

    /// Erosion with a `(2r+1)` square; pixels outside the mask count as off.
    pub fn erode(&self, radius: u32) -> Mask {
        self.morph(radius, true)
    }

    /// Dilation with a `(2r+1)` square.
    pub fn dilate(&self, radius: u32) -> Mask {
        self.morph(radius, false)
    }

    fn morph(&self, radius: u32, erode: bool) -> Mask {
        if radius == 0 {
            return self.clone();
        }
        let (w, h, r) = (self.width as usize, self.height as usize, radius as usize);
        // Separable: horizontal then vertical, with running counts.
        let pass = |src: &[bool], len: usize, stride: usize, lines: usize, line_stride: usize| {
            let mut out = vec![false; src.len()];
            for l in 0..lines {
                let base = l * line_stride;
                let mut on = 0usize;
                // Window [i - r, i + r]; prime with [0, r).
                for k in 0..r.min(len) {
                    on += usize::from(src[base + k * stride]);
                }
                for i in 0..len {
                    if i + r < len {
                        on += usize::from(src[base + (i + r) * stride]);
                    }
                    if i > r {
                        on -= usize::from(src[base + (i - r - 1) * stride]);
                    }
                    let lo = i.saturating_sub(r);
                    let hi = (i + r).min(len - 1);
                    let window = hi - lo + 1;
                    out[base + i * stride] = if erode {
                        // Outside pixels are off, so a clipped window cannot be all on.
                        on == window && window == 2 * r + 1
                    } else {
                        on > 0
                    };
                }
            }
            out
        };
        let horizontal = pass(&self.data, w, 1, h, w);
        let vertical = pass(&horizontal, h, w, w, 1);
        Mask {
            width: self.width,
            height: self.height,
            data: vertical,
        }
    }
}

// ------------------------------------------------------------------------------- histograms

/// A 256-bin histogram of 8-bit values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Histogram {
    counts: [u64; 256],
    total: u64,
}

impl Histogram {
    /// Histogram of row-major `values` (`width` columns), restricted to `mask` when given.
    /// Panics when `mask` does not match the value grid.
    pub fn from_u8(values: &[u8], width: u32, mask: Option<&Mask>) -> Histogram {
        let mut counts = [0u64; 256];
        match mask {
            Some(m) => {
                assert_eq!(
                    m.as_slice().len(),
                    values.len(),
                    "mask does not match values"
                );
                assert_eq!(m.width(), width, "mask width does not match");
                for (v, on) in values.iter().zip(m.as_slice()) {
                    if *on {
                        counts[usize::from(*v)] += 1;
                    }
                }
            }
            None => {
                for v in values {
                    counts[usize::from(*v)] += 1;
                }
            }
        }
        let total = counts.iter().sum();
        Histogram { counts, total }
    }

    pub fn counts(&self) -> &[u64; 256] {
        &self.counts
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    /// Mean bin value (0-255).
    pub fn mean(&self) -> Option<f64> {
        (self.total > 0).then(|| {
            self.counts
                .iter()
                .enumerate()
                .map(|(i, c)| i as f64 * *c as f64)
                .sum::<f64>()
                / self.total as f64
        })
    }

    /// Population variance of the bin values.
    pub fn variance(&self) -> Option<f64> {
        let mean = self.mean()?;
        Some(
            self.counts
                .iter()
                .enumerate()
                .map(|(i, c)| (i as f64 - mean).powi(2) * *c as f64)
                .sum::<f64>()
                / self.total as f64,
        )
    }

    /// The smallest bin value `v` such that at least `p` (0-1) of the samples are `<= v`.
    pub fn percentile(&self, p: f64) -> Option<u8> {
        if self.total == 0 || !(0.0..=1.0).contains(&p) {
            return None;
        }
        let target = ((p * self.total as f64).ceil() as u64).max(1);
        let mut acc = 0u64;
        for (i, c) in self.counts.iter().enumerate() {
            acc += c;
            if acc >= target {
                return Some(i as u8);
            }
        }
        Some(255)
    }

    /// Fraction of samples in `lo..=hi`.
    pub fn fraction_in(&self, lo: u8, hi: u8) -> Option<f64> {
        (self.total > 0 && lo <= hi).then(|| {
            self.counts[usize::from(lo)..=usize::from(hi)]
                .iter()
                .sum::<u64>() as f64
                / self.total as f64
        })
    }

    /// Shannon entropy of the normalised histogram, in bits (0-8).
    pub fn entropy_bits(&self) -> Option<f64> {
        (self.total > 0).then(|| {
            let n = self.total as f64;
            -self
                .counts
                .iter()
                .filter(|c| **c > 0)
                .map(|c| {
                    let p = *c as f64 / n;
                    p * p.log2()
                })
                .sum::<f64>()
        })
    }
}

/// Mean and population variance of `plane` over `mask` (or everywhere). `None` when empty.
pub fn mean_variance(plane: &Plane, mask: Option<&Mask>) -> Option<(f64, f64)> {
    let (mut n, mut sum, mut sum2) = (0u64, 0.0f64, 0.0f64);
    for (i, v) in plane.data.iter().enumerate() {
        if mask.is_none_or(|m| m.data[i]) {
            let v = f64::from(*v);
            n += 1;
            sum += v;
            sum2 += v * v;
        }
    }
    if n == 0 {
        return None;
    }
    let mean = sum / n as f64;
    Some((mean, (sum2 / n as f64 - mean * mean).max(0.0)))
}

/// Percentiles (each 0-1, linear interpolation between order statistics) of `plane` over `mask`.
pub fn percentiles(plane: &Plane, mask: Option<&Mask>, ps: &[f64]) -> Option<Vec<f32>> {
    let mut values: Vec<f32> = plane
        .data
        .iter()
        .enumerate()
        .filter(|(i, v)| v.is_finite() && mask.is_none_or(|m| m.data[*i]))
        .map(|(_, v)| *v)
        .collect();
    if values.is_empty() {
        return None;
    }
    values.sort_by(f32::total_cmp);
    let last = (values.len() - 1) as f64;
    Some(
        ps.iter()
            .map(|p| {
                let pos = p.clamp(0.0, 1.0) * last;
                let (i, f) = (pos.floor() as usize, (pos - pos.floor()) as f32);
                let j = (i + 1).min(values.len() - 1);
                values[i] + (values[j] - values[i]) * f
            })
            .collect(),
    )
}

// -------------------------------------------------------------------------------- gradients

/// 3x3 Sobel derivatives (un-normalised: a unit step gives 4), replicate border.
pub fn sobel(plane: &Plane) -> (Plane, Plane) {
    let (w, h) = (plane.width, plane.height);
    let mut gx = Plane::filled(w, h, 0.0);
    let mut gy = Plane::filled(w, h, 0.0);
    for y in 0..h {
        for x in 0..w {
            let (xi, yi) = (i64::from(x), i64::from(y));
            let p = |dx: i64, dy: i64| plane.get_clamped(xi + dx, yi + dy);
            let dx = (p(1, -1) + 2.0 * p(1, 0) + p(1, 1)) - (p(-1, -1) + 2.0 * p(-1, 0) + p(-1, 1));
            let dy = (p(-1, 1) + 2.0 * p(0, 1) + p(1, 1)) - (p(-1, -1) + 2.0 * p(0, -1) + p(1, -1));
            gx.set(x, y, dx);
            gy.set(x, y, dy);
        }
    }
    (gx, gy)
}

/// Gradient magnitude `sqrt(gx² + gy²) / 8` from [`sobel`]: the 1/8 normalisation makes it
/// intensity change per pixel (a ramp of slope 1 gives 1).
pub fn gradient_magnitude(plane: &Plane) -> Plane {
    let (gx, gy) = sobel(plane);
    let data = gx
        .data
        .iter()
        .zip(&gy.data)
        .map(|(a, b)| a.hypot(*b) / 8.0)
        .collect();
    Plane {
        width: plane.width,
        height: plane.height,
        data,
    }
}

/// Mean of [`gradient_magnitude`] over `mask`.
pub fn mean_gradient_magnitude(plane: &Plane, mask: Option<&Mask>) -> Option<f64> {
    mean_variance(&gradient_magnitude(plane), mask).map(|(m, _)| m)
}

/// Tenengrad: mean of `gx² + gy²` (un-normalised Sobel) over `mask`.
pub fn tenengrad(plane: &Plane, mask: Option<&Mask>) -> Option<f64> {
    let (gx, gy) = sobel(plane);
    let energy = Plane {
        width: plane.width,
        height: plane.height,
        data: gx
            .data
            .iter()
            .zip(&gy.data)
            .map(|(a, b)| a * a + b * b)
            .collect(),
    };
    mean_variance(&energy, mask).map(|(m, _)| m)
}

/// Variance of the 4-neighbour Laplacian over `mask` (replicate border).
pub fn laplacian_variance(plane: &Plane, mask: Option<&Mask>) -> Option<f64> {
    let lap = Plane::from_fn(plane.width, plane.height, |x, y| {
        let (xi, yi) = (i64::from(x), i64::from(y));
        plane.get_clamped(xi - 1, yi)
            + plane.get_clamped(xi + 1, yi)
            + plane.get_clamped(xi, yi - 1)
            + plane.get_clamped(xi, yi + 1)
            - 4.0 * plane.get(x, y)
    });
    mean_variance(&lap, mask).map(|(_, v)| v)
}

/// No-reference perceptual blur of Crété-Roffet et al. (2007), "The blur effect: perception and
/// estimation with a new no-reference perceptual blur metric". 0 = sharp, 1 = maximally blurred.
///
/// The image is re-blurred with a `kernel`-tap horizontal and vertical box filter (the paper uses
/// 9); blur is the share of the neighbour-difference energy that survives re-blurring, taking the
/// worse of the two directions. Differences are summed where both pixels of a pair are inside
/// `mask`. `None` when the region has no variation.
pub fn blur_crete_roffet(plane: &Plane, mask: Option<&Mask>, kernel: u32) -> Option<f64> {
    let (w, h) = (plane.width as usize, plane.height as usize);
    if w < 2 || h < 2 {
        return None;
    }
    let k = kernel.max(1) as i64;
    let half = k / 2;
    let box_h = Plane::from_fn(plane.width, plane.height, |x, y| {
        let s: f32 = (-half..k - half)
            .map(|d| plane.get_clamped(i64::from(x) + d, i64::from(y)))
            .sum();
        s / k as f32
    });
    let box_v = Plane::from_fn(plane.width, plane.height, |x, y| {
        let s: f32 = (-half..k - half)
            .map(|d| plane.get_clamped(i64::from(x), i64::from(y) + d))
            .sum();
        s / k as f32
    });
    let inside = |i: usize| mask.is_none_or(|m| m.data[i]);
    let (mut sf_h, mut sv_h, mut sf_v, mut sv_v) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if !inside(i) {
                continue;
            }
            if x > 0 && inside(i - 1) {
                let df = (plane.data[i] - plane.data[i - 1]).abs();
                let db = (box_h.data[i] - box_h.data[i - 1]).abs();
                sf_h += f64::from(df);
                sv_h += f64::from((df - db).max(0.0));
            }
            if y > 0 && inside(i - w) {
                let df = (plane.data[i] - plane.data[i - w]).abs();
                let db = (box_v.data[i] - box_v.data[i - w]).abs();
                sf_v += f64::from(df);
                sv_v += f64::from((df - db).max(0.0));
            }
        }
    }
    if sf_h <= 1e-9 || sf_v <= 1e-9 {
        return None;
    }
    let b_h = (sf_h - sv_h) / sf_h;
    let b_v = (sf_v - sv_v) / sf_v;
    Some(b_h.max(b_v).clamp(0.0, 1.0))
}

// -------------------------------------------------------------------------------- frequency

/// In-place iterative radix-2 complex FFT. `re.len()` must be a power of two.
fn fft_in_place(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && im.len() == n);
    if n <= 1 {
        return;
    }
    // Bit-reversal permutation.
    let bits = n.trailing_zeros();
    for i in 0..n {
        let j = i.reverse_bits() >> (usize::BITS - bits);
        if j > i {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f64::consts::PI / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f64, 0.0f64);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
        }
        len <<= 1;
    }
}

/// Why a frequency analysis could not run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpectrumError {
    #[error("plane size {0}x{1} is not a power of two in both dimensions")]
    NotPowerOfTwo(u32, u32),
}

/// Centred magnitude spectrum `|F(u, v)|` of `plane` (DC moved to `(width/2, height/2)`), after
/// subtracting the mean and, when `hann` is set, applying a separable Hann window to suppress edge
/// leakage. Both dimensions must be powers of two (resize or crop first). Magnitudes are divided
/// by `width * height`, so a pure sinusoid of amplitude `A` shows two peaks of `A / 2`
/// (without the window).
///
/// Use it for moiré / pixel-grid detection: periodic recapture artefacts appear as isolated peaks
/// away from the centre.
pub fn magnitude_spectrum(plane: &Plane, hann: bool) -> Result<Plane, SpectrumError> {
    let (w, h) = (plane.width as usize, plane.height as usize);
    if !(w.is_power_of_two() && h.is_power_of_two()) {
        return Err(SpectrumError::NotPowerOfTwo(plane.width, plane.height));
    }
    let mean = plane.data.iter().map(|v| f64::from(*v)).sum::<f64>() / (w * h) as f64;
    let window = |n: usize, i: usize| {
        if hann && n > 1 {
            0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos()
        } else {
            1.0
        }
    };
    let mut re: Vec<f64> = (0..w * h)
        .map(|i| (f64::from(plane.data[i]) - mean) * window(w, i % w) * window(h, i / w))
        .collect();
    let mut im = vec![0.0f64; w * h];
    // Rows.
    for y in 0..h {
        fft_in_place(&mut re[y * w..(y + 1) * w], &mut im[y * w..(y + 1) * w]);
    }
    // Columns.
    let (mut cr, mut ci) = (vec![0.0f64; h], vec![0.0f64; h]);
    for x in 0..w {
        for y in 0..h {
            cr[y] = re[y * w + x];
            ci[y] = im[y * w + x];
        }
        fft_in_place(&mut cr, &mut ci);
        for y in 0..h {
            re[y * w + x] = cr[y];
            im[y * w + x] = ci[y];
        }
    }
    let norm = (w * h) as f64;
    let mut out = Plane::filled(plane.width, plane.height, 0.0);
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = ((x + w / 2) % w, (y + h / 2) % h);
            let m = re[y * w + x].hypot(im[y * w + x]) / norm;
            out.data[sy * w + sx] = m as f32;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn luma_is_bt601_and_luminance_follows_ofiq() {
        let img = RgbImage::from_fn(4, 1, |x, _| match x {
            0 => Rgb([0, 0, 0]),
            1 => Rgb([255, 255, 255]),
            2 => Rgb([128, 128, 128]),
            _ => Rgb([255, 0, 0]),
        });
        let y = luminance(&img);
        assert!(y.get(0, 0).abs() < 1e-7);
        assert!((y.get(1, 0) - 1.0).abs() < 1e-6);
        // sRGB 128 is 21.6 % linear light.
        assert!((y.get(2, 0) - 0.2158605).abs() < 1e-5);
        assert!((y.get(3, 0) - 0.2126).abs() < 1e-6);
        assert_eq!(luminance8(&img), vec![0, 255, 55, 54]);
        let l = luma(&img);
        assert!((l.get(2, 0) - 128.0).abs() < 1e-3);
        // BT.601: pure red is 0.299 * 255 = 76.2.
        assert!((l.get(3, 0) - 76.245).abs() < 1e-3);
        assert_eq!(luma8(&img), vec![0, 255, 128, 76]);
        assert!((srgb_to_linear(255) - 1.0).abs() < 1e-7);
    }

    #[test]
    fn histogram_statistics() {
        let values = [0u8, 0, 10, 10, 10, 10, 255, 255];
        let h = Histogram::from_u8(&values, 4, None);
        assert_eq!(h.total(), 8);
        assert!((h.mean().unwrap() - (40.0 + 510.0) / 8.0).abs() < 1e-9);
        assert_eq!(h.percentile(0.25), Some(0));
        assert_eq!(h.percentile(0.5), Some(10));
        assert_eq!(h.percentile(1.0), Some(255));
        assert!((h.fraction_in(0, 25).unwrap() - 0.75).abs() < 1e-12);
        // p = (1/4, 1/2, 1/4): 1.5 bits.
        assert!((h.entropy_bits().unwrap() - 1.5).abs() < 1e-12);
        let mask = Mask::from_fn(4, 2, |x, y| y == 1 && x >= 2);
        let hm = Histogram::from_u8(&values, 4, Some(&mask));
        assert_eq!(hm.total(), 2);
        assert_eq!(hm.mean(), Some(255.0));
        assert_eq!(hm.variance(), Some(0.0));
        assert_eq!(hm.entropy_bits(), Some(0.0));
        let empty = Histogram::from_u8(&values, 4, Some(&Mask::empty(4, 2)));
        assert_eq!(empty.mean(), None);
        assert_eq!(empty.fraction_in(0, 255), None);
    }

    #[test]
    fn masks_polygons_ellipses_and_morphology() {
        // Square with corners at pixel centres 2..=5 covers 4x4 = 16 pixels (half-open rows).
        let poly = [
            Point::new(1.5, 1.5),
            Point::new(5.5, 1.5),
            Point::new(5.5, 5.5),
            Point::new(1.5, 5.5),
        ];
        let m = Mask::from_polygon(10, 10, &poly);
        assert_eq!(m.count(), 16);
        assert!(m.get(2, 2) && m.get(5, 5) && !m.get(6, 5) && !m.get(1, 2));
        let e = Mask::from_ellipse(21, 21, Point::new(10.0, 10.0), 5.0, 2.0, 0.0);
        assert!(e.get(15, 10) && !e.get(10, 13) && e.get(10, 12));
        let er = Mask::from_ellipse(
            21,
            21,
            Point::new(10.0, 10.0),
            5.0,
            2.0,
            std::f32::consts::FRAC_PI_2,
        );
        assert!(er.get(10, 15) && !er.get(15, 10));
        let full = Mask::full(5, 5);
        assert_eq!(full.erode(1).count(), 9);
        assert_eq!(
            Mask::from_fn(5, 5, |x, y| x == 2 && y == 2)
                .dilate(1)
                .count(),
            9
        );
        assert_eq!(m.and(&Mask::empty(10, 10)).count(), 0);
        assert_eq!(m.or(&Mask::empty(10, 10)), m);
        assert_eq!(m.and_not(&m).count(), 0);
    }

    #[test]
    fn mean_variance_and_percentiles_over_masks() {
        let p = Plane::from_fn(4, 1, |x, _| x as f32);
        let (m, v) = mean_variance(&p, None).unwrap();
        assert!((m - 1.5).abs() < 1e-12 && (v - 1.25).abs() < 1e-12);
        let mask = Mask::from_fn(4, 1, |x, _| x >= 2);
        assert_eq!(mean_variance(&p, Some(&mask)).unwrap().0, 2.5);
        let q = percentiles(&p, None, &[0.0, 0.5, 1.0]).unwrap();
        assert_eq!(q, vec![0.0, 1.5, 3.0]);
        assert!(mean_variance(&p, Some(&Mask::empty(4, 1))).is_none());
    }

    #[test]
    fn gradients_respond_to_edges_and_ramps() {
        let ramp = Plane::from_fn(16, 16, |x, _| 3.0 * x as f32);
        let g = gradient_magnitude(&ramp);
        assert!((g.get(8, 8) - 3.0).abs() < 1e-5);
        let flat = Plane::filled(16, 16, 7.0);
        assert_eq!(tenengrad(&flat, None), Some(0.0));
        assert_eq!(laplacian_variance(&flat, None), Some(0.0));
        let edge = Plane::from_fn(16, 16, |x, _| if x < 8 { 0.0 } else { 100.0 });
        assert!(tenengrad(&edge, None).unwrap() > 0.0);
        assert!(laplacian_variance(&edge, None).unwrap() > 0.0);
        assert!(mean_gradient_magnitude(&edge, None).unwrap() > 0.0);
    }

    /// Deterministic white-noise texture in 0-255.
    fn noise(w: u32, h: u32) -> Plane {
        Plane::from_fn(w, h, |x, y| {
            let mut s = (x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663)) as u64;
            s = s
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((s >> 33) % 256) as f32
        })
    }

    fn box_blur(p: &Plane, r: i64) -> Plane {
        Plane::from_fn(p.width(), p.height(), |x, y| {
            let mut s = 0.0;
            for dy in -r..=r {
                for dx in -r..=r {
                    s += p.get_clamped(i64::from(x) + dx, i64::from(y) + dy);
                }
            }
            s / ((2 * r + 1) * (2 * r + 1)) as f32
        })
    }

    #[test]
    fn crete_roffet_blur_increases_monotonically_with_blur() {
        let sharp = noise(96, 96);
        let b0 = blur_crete_roffet(&sharp, None, 9).unwrap();
        let b1 = blur_crete_roffet(&box_blur(&sharp, 1), None, 9).unwrap();
        let b2 = blur_crete_roffet(&box_blur(&sharp, 3), None, 9).unwrap();
        assert!(b0 < 0.3, "{b0}");
        assert!(b0 < b1 && b1 < b2, "{b0} {b1} {b2}");
        let b3 = blur_crete_roffet(&box_blur(&box_blur(&sharp, 3), 3), None, 9).unwrap();
        assert!(b2 < b3 && b3 > b0 + 0.1, "{b0} {b3}");
        // Contrast does not matter: the metric is a ratio.
        let dim =
            Plane::from_vec(96, 96, sharp.as_slice().iter().map(|v| v * 0.2).collect()).unwrap();
        assert!((blur_crete_roffet(&dim, None, 9).unwrap() - b0).abs() < 1e-6);
        assert!(blur_crete_roffet(&Plane::filled(8, 8, 1.0), None, 9).is_none());
    }

    #[test]
    fn fft_matches_a_naive_dft_and_finds_a_sinusoid() {
        let (w, h) = (8usize, 4usize);
        let p = Plane::from_fn(w as u32, h as u32, |x, y| ((x * 7 + y * 3) % 11) as f32);
        let spec = magnitude_spectrum(&p, false).unwrap();
        let mean = p.as_slice().iter().map(|v| f64::from(*v)).sum::<f64>() / (w * h) as f64;
        for v in 0..h {
            for u in 0..w {
                let (mut re, mut im) = (0.0f64, 0.0f64);
                for y in 0..h {
                    for x in 0..w {
                        let a = -2.0
                            * std::f64::consts::PI
                            * (u as f64 * x as f64 / w as f64 + v as f64 * y as f64 / h as f64);
                        let val = f64::from(p.get(x as u32, y as u32)) - mean;
                        re += val * a.cos();
                        im += val * a.sin();
                    }
                }
                let want = re.hypot(im) / (w * h) as f64;
                let (sx, sy) = ((u + w / 2) % w, (v + h / 2) % h);
                let got = f64::from(spec.get(sx as u32, sy as u32));
                assert!((got - want).abs() < 1e-4, "({u},{v}) {got} vs {want}");
            }
        }
        // A horizontal sinusoid with 8 cycles across 64 px peaks at u = +-8 from the centre.
        let s = Plane::from_fn(64, 64, |x, _| {
            100.0 * (2.0 * std::f32::consts::PI * 8.0 * x as f32 / 64.0).sin()
        });
        let spec = magnitude_spectrum(&s, false).unwrap();
        assert!((spec.get(32 + 8, 32) - 50.0).abs() < 1e-2);
        assert!((spec.get(32 - 8, 32) - 50.0).abs() < 1e-2);
        assert!(spec.get(32 + 5, 32) < 1e-2);
        assert!(magnitude_spectrum(&Plane::filled(6, 8, 0.0), true).is_err());
    }
}
