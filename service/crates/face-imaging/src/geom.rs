//! Shared geometry. Image coordinates: origin top-left, x right, y down, in pixels (f32).

use std::ops::{Add, Mul, Sub};

/// A 2-D point in image pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Point { x, y }
    }

    pub fn distance(self, other: Point) -> f32 {
        (self - other).norm()
    }

    pub fn norm(self) -> f32 {
        self.x.hypot(self.y)
    }

    pub fn midpoint(self, other: Point) -> Point {
        Point::new((self.x + other.x) * 0.5, (self.y + other.y) * 0.5)
    }
}

impl Add for Point {
    type Output = Point;
    fn add(self, o: Point) -> Point {
        Point::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for Point {
    type Output = Point;
    fn sub(self, o: Point) -> Point {
        Point::new(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f32> for Point {
    type Output = Point;
    fn mul(self, k: f32) -> Point {
        Point::new(self.x * k, self.y * k)
    }
}

/// Axis-aligned rectangle in image pixels (`x`, `y` = top-left corner).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// Rectangle from two corners (any order).
    pub fn from_corners(a: Point, b: Point) -> Self {
        let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
        let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    pub fn center(&self) -> Point {
        Point::new(self.x + self.width * 0.5, self.y + self.height * 0.5)
    }

    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    pub fn area(&self) -> f32 {
        self.width.max(0.0) * self.height.max(0.0)
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x <= self.right() && p.y >= self.y && p.y <= self.bottom()
    }

    /// Intersection, or `None` when the rectangles do not overlap.
    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
    }

    /// Intersection over union, in [0, 1].
    pub fn iou(&self, o: &Rect) -> f32 {
        let inter = self.intersect(o).map_or(0.0, |r| r.area());
        let union = self.area() + o.area() - inter;
        if union <= 0.0 { 0.0 } else { inter / union }
    }

    /// Grow (or shrink, with a negative factor) around the centre: `width * (1 + factor)`.
    pub fn expand(&self, factor: f32) -> Rect {
        let c = self.center();
        let (w, h) = (self.width * (1.0 + factor), self.height * (1.0 + factor));
        Rect::new(c.x - w * 0.5, c.y - h * 0.5, w, h)
    }
}

/// 2-D affine transform: `[a, b, c, d, e, f]` maps (x, y) to (a*x + b*y + c, d*x + e*y + f).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine(pub [f32; 6]);

impl Affine {
    pub const IDENTITY: Affine = Affine([1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);

    pub fn apply(&self, p: Point) -> Point {
        let [a, b, c, d, e, f] = self.0;
        Point::new(a * p.x + b * p.y + c, d * p.x + e * p.y + f)
    }

    /// Inverse transform, or `None` when singular.
    pub fn invert(&self) -> Option<Affine> {
        let [a, b, c, d, e, f] = self.0;
        let det = a * e - b * d;
        if det.abs() < f32::EPSILON {
            return None;
        }
        let inv = 1.0 / det;
        Some(Affine([
            e * inv,
            -b * inv,
            (b * f - c * e) * inv,
            -d * inv,
            a * inv,
            (c * d - a * f) * inv,
        ]))
    }

    /// `self` after `first`: applying the result equals applying `first`, then `self`.
    pub fn after(&self, first: &Affine) -> Affine {
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = first.0;
        Affine([
            a * a2 + b * d2,
            a * b2 + b * e2,
            a * c2 + b * f2 + c,
            d * a2 + e * d2,
            d * b2 + e * e2,
            d * c2 + e * f2 + f,
        ])
    }
}

/// Four corners: top-left, top-right, bottom-right, bottom-left (clockwise in image space).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad(pub [Point; 4]);

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Point, b: Point) -> bool {
        (a - b).norm() < 1e-4
    }

    #[test]
    fn affine_inverse_round_trips() {
        let t = Affine([0.8, -0.6, 10.0, 0.6, 0.8, -5.0]);
        let inv = t.invert().unwrap();
        let p = Point::new(3.0, 7.0);
        assert!(close(inv.apply(t.apply(p)), p));
        assert!(close(inv.after(&t).apply(p), p));
    }

    #[test]
    fn rect_iou_and_intersection() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);
        assert_eq!(a.intersect(&b), Some(Rect::new(5.0, 5.0, 5.0, 5.0)));
        assert!((a.iou(&b) - 25.0 / 175.0).abs() < 1e-6);
        assert_eq!(a.intersect(&Rect::new(20.0, 20.0, 1.0, 1.0)), None);
    }
}
