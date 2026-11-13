//! Planar geometry for outline detection: connected components, convex hulls, polygon
//! simplification, robust line fits and polygon clipping.

use std::ops::{Add, Mul, Sub};

use face_imaging::geom::Point;

/// A 2-D vector in f64.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct V2 {
    pub(crate) x: f64,
    pub(crate) y: f64,
}

impl V2 {
    pub(crate) const fn new(x: f64, y: f64) -> V2 {
        V2 { x, y }
    }

    pub(crate) fn from_point(p: Point) -> V2 {
        V2::new(f64::from(p.x), f64::from(p.y))
    }

    pub(crate) fn to_point(self) -> Point {
        Point::new(self.x as f32, self.y as f32)
    }

    pub(crate) fn dot(self, o: V2) -> f64 {
        self.x * o.x + self.y * o.y
    }

    pub(crate) fn cross(self, o: V2) -> f64 {
        self.x * o.y - self.y * o.x
    }

    pub(crate) fn norm(self) -> f64 {
        self.x.hypot(self.y)
    }
}

impl Add for V2 {
    type Output = V2;
    fn add(self, o: V2) -> V2 {
        V2::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for V2 {
    type Output = V2;
    fn sub(self, o: V2) -> V2 {
        V2::new(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f64> for V2 {
    type Output = V2;
    fn mul(self, k: f64) -> V2 {
        V2::new(self.x * k, self.y * k)
    }
}

/// An 8-connected component of a binary mask.
#[derive(Clone, Debug)]
pub(crate) struct Blob {
    pub(crate) label: u32,
    pub(crate) min_x: usize,
    pub(crate) max_x: usize,
    pub(crate) min_y: usize,
    pub(crate) max_y: usize,
    pub(crate) pixels: usize,
}

impl Blob {
    pub(crate) fn bbox_area(&self) -> usize {
        (self.max_x - self.min_x + 1) * (self.max_y - self.min_y + 1)
    }
}

/// Label the 8-connected components of `mask`. Labels start at 1; 0 is background.
pub(crate) fn label_components(mask: &[bool], w: usize, h: usize) -> (Vec<u32>, Vec<Blob>) {
    let mut labels = vec![0u32; w * h];
    let mut blobs = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if !mask[start] || labels[start] != 0 {
            continue;
        }
        let label = blobs.len() as u32 + 1;
        let mut blob = Blob {
            label,
            min_x: usize::MAX,
            max_x: 0,
            min_y: usize::MAX,
            max_y: 0,
            pixels: 0,
        };
        labels[start] = label;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            blob.min_x = blob.min_x.min(x);
            blob.max_x = blob.max_x.max(x);
            blob.min_y = blob.min_y.min(y);
            blob.max_y = blob.max_y.max(y);
            blob.pixels += 1;
            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    let n = ny * w + nx;
                    if mask[n] && labels[n] == 0 {
                        labels[n] = label;
                        stack.push(n);
                    }
                }
            }
        }
        blobs.push(blob);
    }
    (labels, blobs)
}

/// Leftmost and rightmost pixel of `blob` on each of its rows: every convex-hull vertex of the
/// blob is among them.
pub(crate) fn row_extremes(labels: &[u32], w: usize, blob: &Blob) -> Vec<V2> {
    let mut pts = Vec::with_capacity(2 * (blob.max_y - blob.min_y + 1));
    for y in blob.min_y..=blob.max_y {
        let row = &labels[y * w + blob.min_x..=y * w + blob.max_x];
        if let Some(first) = row.iter().position(|&l| l == blob.label) {
            let last = row.iter().rposition(|&l| l == blob.label).unwrap_or(first);
            pts.push(V2::new((blob.min_x + first) as f64, y as f64));
            if last != first {
                pts.push(V2::new((blob.min_x + last) as f64, y as f64));
            }
        }
    }
    pts
}

/// Convex hull (Andrew's monotone chain). In image coordinates (y down) the result runs
/// clockwise on screen and has a positive [`signed_area`]. Collinear points are dropped.
pub(crate) fn convex_hull(mut pts: Vec<V2>) -> Vec<V2> {
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let turn = |o: V2, a: V2, b: V2| (a - o).cross(b - o);
    let mut lower: Vec<V2> = Vec::with_capacity(pts.len());
    for &p in &pts {
        while lower.len() >= 2 && turn(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<V2> = Vec::with_capacity(pts.len());
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && turn(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// Signed area (shoelace). Positive for polygons running clockwise on screen (y down).
pub(crate) fn signed_area(poly: &[V2]) -> f64 {
    let n = poly.len();
    0.5 * (0..n)
        .map(|i| poly[i].cross(poly[(i + 1) % n]))
        .sum::<f64>()
}

pub(crate) fn perimeter(poly: &[V2]) -> f64 {
    let n = poly.len();
    (0..n).map(|i| (poly[(i + 1) % n] - poly[i]).norm()).sum()
}

/// True when the polygon is strictly convex with a positive orientation.
pub(crate) fn is_convex(poly: &[V2]) -> bool {
    let n = poly.len();
    n >= 3
        && (0..n).all(|i| {
            let (a, b, c) = (poly[i], poly[(i + 1) % n], poly[(i + 2) % n]);
            (b - a).cross(c - b) > 0.0
        })
}

/// Douglas-Peucker simplification of a closed polygon. The first split is at the two vertices
/// farthest apart.
pub(crate) fn simplify_closed(poly: &[V2], eps: f64) -> Vec<V2> {
    let n = poly.len();
    if n <= 3 {
        return poly.to_vec();
    }
    let (mut a, mut b, mut best) = (0, 0, -1.0);
    for i in 0..n {
        for j in i + 1..n {
            let d = (poly[j] - poly[i]).norm();
            if d > best {
                (a, b, best) = (i, j, d);
            }
        }
    }
    let first: Vec<V2> = poly[a..=b].to_vec();
    let second: Vec<V2> = poly[b..].iter().chain(&poly[..=a]).copied().collect();
    let mut out = simplify_open(&first, eps);
    let tail = simplify_open(&second, eps);
    out.extend_from_slice(&tail[1..tail.len() - 1]);
    out
}

fn simplify_open(pts: &[V2], eps: f64) -> Vec<V2> {
    let n = pts.len();
    if n <= 2 {
        return pts.to_vec();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0, n - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (mut idx, mut dmax) = (a, 0.0);
        for (i, p) in pts.iter().enumerate().take(b).skip(a + 1) {
            let d = segment_distance(*p, pts[a], pts[b]);
            if d > dmax {
                (idx, dmax) = (i, d);
            }
        }
        if dmax > eps {
            keep[idx] = true;
            stack.push((a, idx));
            stack.push((idx, b));
        }
    }
    pts.iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(*p))
        .collect()
}

fn segment_distance(p: V2, a: V2, b: V2) -> f64 {
    let ab = b - a;
    let len2 = ab.dot(ab);
    if len2 <= 0.0 {
        return (p - a).norm();
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (p - (a + ab * t)).norm()
}

/// An infinite line through `point` along the unit vector `dir`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Line {
    pub(crate) point: V2,
    pub(crate) dir: V2,
}

impl Line {
    pub(crate) fn through(a: V2, b: V2) -> Option<Line> {
        let d = b - a;
        let len = d.norm();
        (len > 1e-9).then(|| Line {
            point: a,
            dir: d * (1.0 / len),
        })
    }

    /// Signed distance of `p` from the line.
    pub(crate) fn distance(&self, p: V2) -> f64 {
        (p - self.point).cross(self.dir)
    }

    pub(crate) fn intersect(&self, o: &Line) -> Option<V2> {
        let den = self.dir.cross(o.dir);
        if den.abs() < 1e-9 {
            return None;
        }
        let t = (o.point - self.point).cross(o.dir) / den;
        Some(self.point + self.dir * t)
    }

    /// Total-least-squares line through `pts`.
    pub(crate) fn fit(pts: &[V2]) -> Option<Line> {
        if pts.len() < 2 {
            return None;
        }
        let n = pts.len() as f64;
        let c = pts.iter().fold(V2::default(), |a, p| a + *p) * (1.0 / n);
        let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
        for p in pts {
            let d = *p - c;
            sxx += d.x * d.x;
            sxy += d.x * d.y;
            syy += d.y * d.y;
        }
        if sxx + syy <= 1e-12 {
            return None;
        }
        let theta = 0.5 * (2.0 * sxy).atan2(sxx - syy);
        Some(Line {
            point: c,
            dir: V2::new(theta.cos(), theta.sin()),
        })
    }

    /// Line fit with outlier rejection (residual above 2.5 robust sigma, at least 1 px).
    /// Returns the line and its number of inliers.
    pub(crate) fn fit_robust(pts: &[V2]) -> Option<(Line, usize)> {
        let mut line = Line::fit(pts)?;
        let mut inliers = pts.to_vec();
        for _ in 0..3 {
            let mut res: Vec<f64> = pts.iter().map(|p| line.distance(*p).abs()).collect();
            let mid = res.len() / 2;
            let mad = *res.select_nth_unstable_by(mid, f64::total_cmp).1;
            let limit = (2.5 * 1.4826 * mad).max(1.0);
            let kept: Vec<V2> = pts
                .iter()
                .copied()
                .filter(|p| line.distance(*p).abs() <= limit)
                .collect();
            if kept.len() < 2 || kept.len() == inliers.len() {
                break;
            }
            line = Line::fit(&kept)?;
            inliers = kept;
        }
        Some((line, inliers.len()))
    }
}

/// Area of the part of `poly` (any simple polygon) inside the convex polygon `clip` (positive
/// orientation), by Sutherland-Hodgman clipping.
pub(crate) fn clipped_area(poly: &[V2], clip: &[V2]) -> f64 {
    let mut out: Vec<V2> = poly.to_vec();
    let n = clip.len();
    for i in 0..n {
        if out.is_empty() {
            break;
        }
        let (a, b) = (clip[i], clip[(i + 1) % n]);
        let inside = |p: V2| (b - a).cross(p - a) >= 0.0;
        let input = std::mem::take(&mut out);
        let m = input.len();
        for j in 0..m {
            let (p, q) = (input[j], input[(j + 1) % m]);
            match (inside(p), inside(q)) {
                (true, true) => out.push(q),
                (true, false) => out.extend(
                    Line::through(p, q)
                        .and_then(|l| Line::through(a, b).and_then(|e| l.intersect(&e))),
                ),
                (false, true) => {
                    out.extend(
                        Line::through(p, q)
                            .and_then(|l| Line::through(a, b).and_then(|e| l.intersect(&e))),
                    );
                    out.push(q);
                }
                (false, false) => {}
            }
        }
    }
    if out.len() < 3 {
        0.0
    } else {
        signed_area(&out).abs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hull_of_a_square_is_clockwise_on_screen() {
        let pts = vec![
            V2::new(0.0, 0.0),
            V2::new(2.0, 0.0),
            V2::new(1.0, 1.0),
            V2::new(2.0, 2.0),
            V2::new(0.0, 2.0),
            V2::new(1.0, 0.0),
        ];
        let hull = convex_hull(pts);
        assert_eq!(hull.len(), 4);
        assert!(signed_area(&hull) > 0.0);
        assert!((signed_area(&hull) - 4.0).abs() < 1e-12);
        assert!(is_convex(&hull));
    }

    #[test]
    fn simplification_reduces_a_rounded_rectangle_to_four_corners() {
        // 200 x 100 rectangle with corner radius 10, sampled densely.
        let mut poly = Vec::new();
        let r = 10.0;
        let corners = [
            (190.0, 10.0, -90.0),
            (190.0, 90.0, 0.0),
            (10.0, 90.0, 90.0),
            (10.0, 10.0, 180.0),
        ];
        for (cx, cy, start) in corners {
            for k in 0..=9 {
                let a = (start + 10.0 * f64::from(k)).to_radians();
                poly.push(V2::new(cx + r * a.cos(), cy + r * a.sin()));
            }
        }
        let hull = convex_hull(poly);
        // A tight tolerance keeps both ends of each rounded corner; a looser one leaves four
        // vertices (the quad fit steps through tolerances for this reason).
        assert_eq!(simplify_closed(&hull, 0.005 * perimeter(&hull)).len(), 8);
        let s = simplify_closed(&hull, 0.02 * perimeter(&hull));
        assert_eq!(s.len(), 4, "{s:?}");
    }

    #[test]
    fn robust_line_fit_ignores_outliers() {
        let mut pts: Vec<V2> = (0..50)
            .map(|i| V2::new(f64::from(i), 0.5 * f64::from(i) + 3.0))
            .collect();
        pts.push(V2::new(10.0, 40.0));
        pts.push(V2::new(20.0, -30.0));
        let (line, inliers) = Line::fit_robust(&pts).unwrap();
        assert_eq!(inliers, 50);
        assert!(line.distance(V2::new(100.0, 53.0)).abs() < 1e-6);
    }

    #[test]
    fn intersection_and_clipping() {
        let a = Line::through(V2::new(0.0, 0.0), V2::new(1.0, 1.0)).unwrap();
        let b = Line::through(V2::new(0.0, 2.0), V2::new(2.0, 0.0)).unwrap();
        let p = a.intersect(&b).unwrap();
        assert!((p - V2::new(1.0, 1.0)).norm() < 1e-12);

        let square = [
            V2::new(0.0, 0.0),
            V2::new(10.0, 0.0),
            V2::new(10.0, 10.0),
            V2::new(0.0, 10.0),
        ];
        let half_out = [
            V2::new(5.0, 2.0),
            V2::new(15.0, 2.0),
            V2::new(15.0, 6.0),
            V2::new(5.0, 6.0),
        ];
        assert!((clipped_area(&half_out, &square) - 20.0).abs() < 1e-9);
        assert!((clipped_area(&square, &square) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn components_and_row_extremes() {
        let (w, h) = (6, 4);
        let mut mask = vec![false; w * h];
        for &(x, y) in &[(1, 1), (2, 1), (3, 2), (5, 3)] {
            mask[y * w + x] = true;
        }
        let (labels, blobs) = label_components(&mask, w, h);
        assert_eq!(blobs.len(), 2);
        assert_eq!(blobs[0].pixels, 3);
        let pts = row_extremes(&labels, w, &blobs[0]);
        assert_eq!(
            pts,
            vec![V2::new(1.0, 1.0), V2::new(2.0, 1.0), V2::new(3.0, 2.0)]
        );
    }
}
