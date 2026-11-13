//! Five-point alignment to the 112x112 ArcFace template (docs/architecture.md section 3.3).

use face_imaging::RgbImage;
use face_imaging::geom::{Affine, Point};
use face_imaging::transform::{Border, warp_affine_inverse};

/// Side of the aligned face crop.
pub const ALIGNED_SIZE: u32 = 112;

/// The ArcFace 112x112 template, in ArcFace order: subject's right eye centre (image-left),
/// subject's left eye centre, nose tip, subject's right mouth corner, subject's left mouth
/// corner. Pinned in docs/architecture.md section 3.3.
pub const ARCFACE_TEMPLATE: [Point; 5] = [
    Point::new(38.2946, 51.6963),
    Point::new(73.5318, 51.5014),
    Point::new(56.0252, 71.7366),
    Point::new(41.5493, 92.3655),
    Point::new(70.7299, 92.2041),
];

/// Least-squares similarity transform (rotation, uniform scale and translation; no reflection)
/// that maps `src` onto `dst` (Umeyama 1991).
///
/// In two dimensions Umeyama's SVD solution has a closed form: with both point sets centred and
/// written as complex numbers, the rotation-with-scale is `sum(conj(s) * d) / sum(|s|^2)`. This
/// equals the SVD form with the reflection correction (`det(U V^T) < 0` gives `S = diag(1, -1)`),
/// so mirrored input still yields a proper rotation. `None` for fewer than two points, mismatched
/// lengths or coincident source points.
pub fn umeyama(src: &[Point], dst: &[Point]) -> Option<Affine> {
    let n = src.len();
    if n < 2 || dst.len() != n {
        return None;
    }
    let mean = |pts: &[Point]| {
        let (sx, sy) = pts.iter().fold((0.0f64, 0.0f64), |(ax, ay), p| {
            (ax + f64::from(p.x), ay + f64::from(p.y))
        });
        (sx / n as f64, sy / n as f64)
    };
    let (msx, msy) = mean(src);
    let (mdx, mdy) = mean(dst);
    let (mut re, mut im, mut var) = (0.0f64, 0.0f64, 0.0f64);
    for (s, d) in src.iter().zip(dst) {
        let (sx, sy) = (f64::from(s.x) - msx, f64::from(s.y) - msy);
        let (dx, dy) = (f64::from(d.x) - mdx, f64::from(d.y) - mdy);
        re += sx * dx + sy * dy;
        im += sx * dy - sy * dx;
        var += sx * sx + sy * sy;
    }
    if !(var.is_finite() && var > 1e-12) {
        return None;
    }
    let (a, b) = (re / var, im / var);
    let tx = mdx - (a * msx - b * msy);
    let ty = mdy - (b * msx + a * msy);
    let m = [a, -b, tx, b, a, ty];
    m.iter()
        .all(|v| v.is_finite())
        .then(|| Affine(m.map(|v| v as f32)))
}

/// A face warped to the ArcFace template.
#[derive(Clone, Debug)]
pub struct AlignedFace {
    /// The 112x112 crop.
    pub image: RgbImage,
    /// Source image to crop.
    pub transform: Affine,
    /// The five source points mapped into the crop (close to [`ARCFACE_TEMPLATE`]).
    pub points: [Point; 5],
}

/// Align a face by its five points (ArcFace order) to the 112x112 template, sampling like
/// OpenCV `warpAffine` (bilinear, black border) so the result matches the lab's reference crops.
/// `None` when the points are degenerate.
pub fn align_face(image: &RgbImage, landmarks5: &[Point; 5]) -> Option<AlignedFace> {
    let transform = umeyama(landmarks5, &ARCFACE_TEMPLATE)?;
    let inverse = transform.invert()?;
    Some(AlignedFace {
        image: warp_affine_inverse(
            image,
            &inverse,
            ALIGNED_SIZE,
            ALIGNED_SIZE,
            Border::Constant([0, 0, 0]),
        ),
        transform,
        points: landmarks5.map(|p| transform.apply(p)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use face_document::testing::Rng;
    use image::Rgb;

    fn similarity(scale: f64, angle_deg: f64, tx: f64, ty: f64) -> Affine {
        let (s, c) = angle_deg.to_radians().sin_cos();
        Affine([scale * c, -scale * s, tx, scale * s, scale * c, ty].map(|v| v as f32))
    }

    fn close(a: &Affine, b: &Affine, tol: f32) -> bool {
        a.0.iter()
            .zip(b.0.iter())
            .all(|(x, y)| (x - y).abs() <= tol)
    }

    #[test]
    fn recovers_known_similarity_transforms_exactly() {
        let mut rng = Rng::new(3);
        for _ in 0..50 {
            let t = similarity(
                rng.range(0.2, 6.0),
                rng.range(-180.0, 180.0),
                rng.range(-500.0, 500.0),
                rng.range(-500.0, 500.0),
            );
            let src = ARCFACE_TEMPLATE;
            let dst = src.map(|p| t.apply(p));
            let fit = umeyama(&src, &dst).unwrap();
            assert!(close(&fit, &t, 2e-3), "{fit:?} vs {t:?}");
            // And the inverse direction, as used for alignment: image points onto the template.
            let back = umeyama(&dst, &src).unwrap();
            for (d, s) in dst.iter().zip(&src) {
                assert!(back.apply(*d).distance(*s) < 1e-3, "{back:?}");
            }
        }
    }

    /// Independent least-squares solution of the same 4-parameter problem (normal equations).
    fn normal_equations(src: &[Point], dst: &[Point]) -> [f64; 4] {
        // Unknowns (a, b, tx, ty): u = a x - b y + tx, v = b x + a y + ty.
        let mut ata = [[0.0f64; 4]; 4];
        let mut atb = [0.0f64; 4];
        for (s, d) in src.iter().zip(dst) {
            let (x, y) = (f64::from(s.x), f64::from(s.y));
            for (row, rhs) in [
                ([x, -y, 1.0, 0.0], f64::from(d.x)),
                ([y, x, 0.0, 1.0], f64::from(d.y)),
            ] {
                for i in 0..4 {
                    atb[i] += row[i] * rhs;
                    for j in 0..4 {
                        ata[i][j] += row[i] * row[j];
                    }
                }
            }
        }
        // Gauss-Jordan on the 4x4 system.
        let mut m = [[0.0f64; 5]; 4];
        for i in 0..4 {
            m[i][..4].copy_from_slice(&ata[i]);
            m[i][4] = atb[i];
        }
        for col in 0..4 {
            let p = (col..4)
                .max_by(|&a, &b| m[a][col].abs().total_cmp(&m[b][col].abs()))
                .unwrap();
            m.swap(col, p);
            let pivot = m[col];
            for (r, row) in m.iter_mut().enumerate() {
                if r != col {
                    let f = row[col] / pivot[col];
                    for k in 0..5 {
                        row[k] -= f * pivot[k];
                    }
                }
            }
        }
        [0, 1, 2, 3].map(|i| m[i][4] / m[i][i])
    }

    #[test]
    fn matches_an_independent_least_squares_solution_on_noisy_points() {
        let mut rng = Rng::new(9);
        for _ in 0..20 {
            let t = similarity(rng.range(1.0, 4.0), rng.range(-40.0, 40.0), 300.0, 200.0);
            let src: Vec<Point> = ARCFACE_TEMPLATE
                .iter()
                .map(|p| {
                    let q = t.apply(*p);
                    Point::new(
                        q.x + rng.range(-2.0, 2.0) as f32,
                        q.y + rng.range(-2.0, 2.0) as f32,
                    )
                })
                .collect();
            let fit = umeyama(&src, &ARCFACE_TEMPLATE).unwrap();
            let [a, b, tx, ty] = normal_equations(&src, &ARCFACE_TEMPLATE);
            let expected = Affine([a, -b, tx, b, a, ty].map(|v| v as f32));
            assert!(close(&fit, &expected, 1e-3), "{fit:?} vs {expected:?}");
        }
    }

    #[test]
    fn mirrored_points_still_give_a_proper_rotation() {
        let mirrored = ARCFACE_TEMPLATE.map(|p| Point::new(112.0 - p.x, p.y));
        let fit = umeyama(&mirrored, &ARCFACE_TEMPLATE).unwrap();
        let [a, b, _, d, e, _] = fit.0;
        assert!(
            a * e - b * d > 0.0,
            "determinant must stay positive: {fit:?}"
        );
        assert!(umeyama(&[Point::new(1.0, 1.0); 5], &ARCFACE_TEMPLATE).is_none());
        assert!(umeyama(&ARCFACE_TEMPLATE[..1], &ARCFACE_TEMPLATE[..1]).is_none());
    }

    /// Image with small Gaussian dots centred (pixel centres at integers) on `points`.
    fn dots(w: u32, h: u32, points: &[Point]) -> RgbImage {
        let mut img = RgbImage::new(w, h);
        for (x, y, p) in img.enumerate_pixels_mut() {
            let v: f32 = points
                .iter()
                .map(|q| {
                    let d2 = (x as f32 - q.x).powi(2) + (y as f32 - q.y).powi(2);
                    250.0 * (-d2 / (2.0 * 1.8 * 1.8)).exp()
                })
                .sum();
            *p = Rgb([v.min(255.0) as u8; 3]);
        }
        img
    }

    #[test]
    fn aligned_crop_puts_the_five_points_on_the_template() {
        // A face 2.6x larger than the template, rotated 23 degrees, somewhere in a 640x480 image.
        let t = similarity(2.6, 23.0, 250.0, 60.0);
        let landmarks = ARCFACE_TEMPLATE.map(|p| t.apply(p));
        let img = dots(640, 480, &landmarks);
        let aligned = align_face(&img, &landmarks).unwrap();
        assert_eq!(aligned.image.dimensions(), (112, 112));
        for (p, q) in aligned.points.iter().zip(ARCFACE_TEMPLATE) {
            assert!(p.distance(q) < 1e-3);
        }
        // Intensity-weighted centroid around each template point.
        for q in ARCFACE_TEMPLATE {
            let (mut sx, mut sy, mut sw) = (0.0f32, 0.0f32, 0.0f32);
            for (x, y, p) in aligned.image.enumerate_pixels() {
                if (x as f32 - q.x).abs() <= 4.0 && (y as f32 - q.y).abs() <= 4.0 {
                    let v = f32::from(p.0[0]);
                    (sx, sy, sw) = (sx + v * x as f32, sy + v * y as f32, sw + v);
                }
            }
            let c = Point::new(sx / sw, sy / sw);
            assert!(c.distance(q) < 0.35, "{c:?} vs {q:?}");
        }
    }
}
