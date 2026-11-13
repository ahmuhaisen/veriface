//! Projective transforms (homographies) in continuous pixel coordinates.

use face_imaging::geom::{Affine, Point};

/// A 2-D projective transform. `(x, y)` maps to `((a x + b y + c) / w, (d x + e y + f) / w)` with
/// `w = g x + h y + i`, for the row-major matrix `[[a, b, c], [d, e, f], [g, h, i]]`.
///
/// Coordinates are continuous image coordinates: pixel `(i, j)` covers `[i, i+1) x [j, j+1)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projective(pub [[f64; 3]; 3]);

impl Projective {
    pub const IDENTITY: Projective =
        Projective([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);

    pub fn scale(sx: f64, sy: f64) -> Projective {
        Projective([[sx, 0.0, 0.0], [0.0, sy, 0.0], [0.0, 0.0, 1.0]])
    }

    pub fn translate(tx: f64, ty: f64) -> Projective {
        Projective([[1.0, 0.0, tx], [0.0, 1.0, ty], [0.0, 0.0, 1.0]])
    }

    pub fn from_affine(a: &Affine) -> Projective {
        let [a0, b, c, d, e, f] = a.0.map(f64::from);
        Projective([[a0, b, c], [d, e, f], [0.0, 0.0, 1.0]])
    }

    /// Map `(x, y)`. `None` on the vanishing line or for non-finite results.
    pub fn apply_f64(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let m = &self.0;
        let w = m[2][0] * x + m[2][1] * y + m[2][2];
        if !w.is_finite() || w.abs() < 1e-12 {
            return None;
        }
        let px = (m[0][0] * x + m[0][1] * y + m[0][2]) / w;
        let py = (m[1][0] * x + m[1][1] * y + m[1][2]) / w;
        (px.is_finite() && py.is_finite()).then_some((px, py))
    }

    /// Map a point. `None` on the vanishing line or for non-finite results.
    pub fn apply(&self, p: Point) -> Option<Point> {
        self.apply_f64(f64::from(p.x), f64::from(p.y))
            .map(|(x, y)| Point::new(x as f32, y as f32))
    }

    /// `self` followed by `next`: applying the result equals applying `self`, then `next`.
    pub fn then(&self, next: &Projective) -> Projective {
        let (a, b) = (&next.0, &self.0);
        let mut m = [[0.0; 3]; 3];
        for (r, row) in m.iter_mut().enumerate() {
            for (c, v) in row.iter_mut().enumerate() {
                *v = a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c];
            }
        }
        Projective(m)
    }

    /// Inverse transform, or `None` when singular.
    pub fn invert(&self) -> Option<Projective> {
        let m = &self.0;
        let c00 = m[1][1] * m[2][2] - m[1][2] * m[2][1];
        let c01 = m[1][2] * m[2][0] - m[1][0] * m[2][2];
        let c02 = m[1][0] * m[2][1] - m[1][1] * m[2][0];
        let det = m[0][0] * c00 + m[0][1] * c01 + m[0][2] * c02;
        let scale = m.iter().flatten().fold(0.0f64, |acc, v| acc.max(v.abs()));
        if !det.is_finite() || det.abs() <= 1e-14 * scale.powi(3) {
            return None;
        }
        let inv = 1.0 / det;
        Some(Projective([
            [
                c00 * inv,
                (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv,
                (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv,
            ],
            [
                c01 * inv,
                (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv,
                (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv,
            ],
            [
                c02 * inv,
                (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv,
                (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv,
            ],
        ]))
    }

    /// The homography mapping `src[k]` onto `dst[k]` for k = 0..4 (normalised DLT). `None` when
    /// the points are degenerate (three of them collinear, or coincident).
    pub fn from_points(src: &[Point; 4], dst: &[Point; 4]) -> Option<Projective> {
        let s = src.map(|p| (f64::from(p.x), f64::from(p.y)));
        let d = dst.map(|p| (f64::from(p.x), f64::from(p.y)));
        Projective::from_points_f64(&s, &d)
    }

    pub(crate) fn from_points_f64(
        src: &[(f64, f64); 4],
        dst: &[(f64, f64); 4],
    ) -> Option<Projective> {
        let ts = normalising(src)?;
        let td = normalising(dst)?;
        let mut a = [[0.0f64; 9]; 8];
        for (k, (s, d)) in src.iter().zip(dst).enumerate() {
            let (x, y) = ts.apply_f64(s.0, s.1)?;
            let (u, v) = td.apply_f64(d.0, d.1)?;
            a[2 * k] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
            a[2 * k + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
        }
        let h = solve8(a)?;
        let hn = Projective([[h[0], h[1], h[2]], [h[3], h[4], h[5]], [h[6], h[7], 1.0]]);
        let m = ts.then(&hn).then(&td.invert()?);
        let w = m.0[2][2];
        if !w.is_finite() || w.abs() < 1e-15 {
            return None;
        }
        Some(Projective(m.0.map(|row| row.map(|v| v / w))))
    }

    /// Smallest singular value of the Jacobian at `(x, y)`: the least a unit step at `(x, y)` is
    /// stretched by the mapping. A mapping from an output image to a source image never upscales
    /// when this is at least 1 everywhere on the output.
    pub fn min_stretch(&self, x: f64, y: f64) -> Option<f64> {
        let m = &self.0;
        let w = m[2][0] * x + m[2][1] * y + m[2][2];
        let (px, py) = self.apply_f64(x, y)?;
        let j00 = (m[0][0] - px * m[2][0]) / w;
        let j01 = (m[0][1] - px * m[2][1]) / w;
        let j10 = (m[1][0] - py * m[2][0]) / w;
        let j11 = (m[1][1] - py * m[2][1]) / w;
        let p = j00 * j00 + j10 * j10;
        let q = j01 * j01 + j11 * j11;
        let r = j00 * j01 + j10 * j11;
        let mean = 0.5 * (p + q);
        let dev = (0.25 * (p - q) * (p - q) + r * r).sqrt();
        let s = (mean - dev).max(0.0).sqrt();
        s.is_finite().then_some(s)
    }
}

/// Hartley normalisation: centroid to the origin, mean distance sqrt(2).
fn normalising(pts: &[(f64, f64); 4]) -> Option<Projective> {
    let (sx, sy) = pts
        .iter()
        .fold((0.0, 0.0), |(ax, ay), p| (ax + p.0, ay + p.1));
    let (cx, cy) = (sx / 4.0, sy / 4.0);
    let mean = pts.iter().map(|p| (p.0 - cx).hypot(p.1 - cy)).sum::<f64>() / 4.0;
    if !mean.is_finite() || mean <= 1e-9 {
        return None;
    }
    let s = std::f64::consts::SQRT_2 / mean;
    Some(Projective([
        [s, 0.0, -s * cx],
        [0.0, s, -s * cy],
        [0.0, 0.0, 1.0],
    ]))
}

/// Solve an 8x8 linear system given as an augmented matrix (Gaussian elimination, partial
/// pivoting).
fn solve8(mut a: [[f64; 9]; 8]) -> Option<[f64; 8]> {
    for col in 0..8 {
        let pivot = (col..8).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        let pivot_row = a[col];
        for row in a.iter_mut().skip(col + 1) {
            let f = row[col] / pivot_row[col];
            if f != 0.0 {
                for (v, p) in row.iter_mut().zip(pivot_row.iter()).skip(col) {
                    *v -= f * p;
                }
            }
        }
    }
    let mut x = [0.0f64; 8];
    for row in (0..8).rev() {
        let tail: f64 = a[row][row + 1..8]
            .iter()
            .zip(&x[row + 1..8])
            .map(|(c, v)| c * v)
            .sum();
        x[row] = (a[row][8] - tail) / a[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f64, f64), b: (f64, f64), tol: f64) -> bool {
        (a.0 - b.0).abs() <= tol && (a.1 - b.1).abs() <= tol
    }

    #[test]
    fn four_point_homography_maps_corners_exactly_and_inverts() {
        let src = [(0.0, 0.0), (400.0, 0.0), (400.0, 250.0), (0.0, 250.0)];
        let dst = [
            (103.5, 87.25),
            (612.0, 140.0),
            (580.5, 470.0),
            (66.0, 401.0),
        ];
        let h = Projective::from_points_f64(&src, &dst).unwrap();
        for (s, d) in src.iter().zip(&dst) {
            assert!(close(h.apply_f64(s.0, s.1).unwrap(), *d, 1e-6));
        }
        let inv = h.invert().unwrap();
        let p = h.apply_f64(123.0, 77.0).unwrap();
        assert!(close(inv.apply_f64(p.0, p.1).unwrap(), (123.0, 77.0), 1e-6));
        let round = h.then(&inv);
        assert!(close(
            round.apply_f64(10.0, 20.0).unwrap(),
            (10.0, 20.0),
            1e-9
        ));
    }

    #[test]
    fn degenerate_points_are_rejected() {
        let src = [(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (0.0, 1.0)];
        let dst = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        assert!(Projective::from_points_f64(&src, &dst).is_none());
        assert!(Projective::scale(0.0, 1.0).invert().is_none());
    }

    #[test]
    fn min_stretch_of_a_similarity_is_its_scale() {
        let a = Affine([0.0, -2.0, 5.0, 2.0, 0.0, 1.0]); // rotation by 90 degrees, scale 2
        let p = Projective::from_affine(&a);
        assert!((p.min_stretch(3.0, 4.0).unwrap() - 2.0).abs() < 1e-12);
        let squash = Projective::scale(1.0, 0.5);
        assert!((squash.min_stretch(0.0, 0.0).unwrap() - 0.5).abs() < 1e-12);
    }
}
