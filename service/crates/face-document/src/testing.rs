//! Synthetic document scenes and fake vision seams for tests in this crate and in `face-verify`
//! (feature `test-support`). Everything is drawn procedurally: there are no real faces or
//! documents here.
//!
//! - [`SyntheticFace`]: a flat-colour face (skin ellipse, two eye discs, a mouth bar) with
//!   ArcFace-like proportions and known five points.
//! - [`card_texture`] / [`card_scene`]: an ID-1 (or any aspect) card with a portrait, a ghost
//!   portrait, text lines and a corner marker, placed in a scene by a pinhole-camera homography.
//! - [`ColourFaceFinder`] / [`ColourPortraitAnalyzer`]: fakes for the vision seams that find the
//!   synthetic faces by colour. The finder only reports faces confidently when upright, like a
//!   real detector, so orientation search can be tested.

use face_core::{CheckId, Measurements};
use face_imaging::RgbImage;
use face_imaging::geom::{Point, Rect};
use image::Rgb;

use crate::face::{
    BoxError, DetectedFace, FaceAnalysis, FaceFinder, FaceGeometry, PortraitAnalyzer, Pose,
};
use crate::rectify::Projective;
use crate::rectify::shapes::label_components;

pub const SKIN: [u8; 3] = [226, 178, 140];
pub const EYE: [u8; 3] = [30, 50, 170];
pub const MOUTH: [u8; 3] = [180, 34, 44];
pub const CARD: [u8; 3] = [238, 234, 222];
pub const PHOTO: [u8; 3] = [188, 206, 222];
pub const INK: [u8; 3] = [60, 62, 74];
pub const HEADER: [u8; 3] = [40, 120, 110];
pub const MARKER: [u8; 3] = [40, 160, 70];

/// Small deterministic PRNG (xorshift64*).
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next_f64()
    }
}

// Face-local geometry, in inter-eye distances, origin at the eye midpoint, y down. Close to the
// ArcFace template proportions (nose 0.57, mouth corners +-0.41 at 1.16 below the eyes).
const EYE_X: f64 = 0.5;
const EYE_R: f64 = 0.14;
const MOUTH_Y: f64 = 1.15;
const MOUTH_HALF: f64 = 0.30;
const MOUTH_R: f64 = 0.09;
const NOSE_Y: f64 = 0.57;
const SKIN_CY: f64 = 0.45;
const SKIN_RX: f64 = 1.0;
const SKIN_RY: f64 = 1.45;

/// A flat-colour synthetic face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SyntheticFace {
    /// Eye midpoint.
    pub center: Point,
    /// Inter-eye distance in pixels.
    pub ied: f32,
    /// In-plane rotation, degrees, clockwise in the image.
    pub roll_deg: f32,
}

impl SyntheticFace {
    pub fn new(x: f32, y: f32, ied: f32) -> SyntheticFace {
        SyntheticFace {
            center: Point::new(x, y),
            ied,
            roll_deg: 0.0,
        }
    }

    fn local(&self, x: f64, y: f64) -> (f64, f64) {
        let (dx, dy) = (x - f64::from(self.center.x), y - f64::from(self.center.y));
        let (s, c) = (-f64::from(self.roll_deg)).to_radians().sin_cos();
        let ied = f64::from(self.ied);
        ((dx * c - dy * s) / ied, (dx * s + dy * c) / ied)
    }

    fn image_point(&self, lx: f64, ly: f64) -> Point {
        let (s, c) = f64::from(self.roll_deg).to_radians().sin_cos();
        let ied = f64::from(self.ied);
        Point::new(
            (f64::from(self.center.x) + ied * (lx * c - ly * s)) as f32,
            (f64::from(self.center.y) + ied * (lx * s + ly * c)) as f32,
        )
    }

    /// Colour of the face at a point, or `None` outside it.
    pub fn colour_at(&self, x: f64, y: f64) -> Option<[u8; 3]> {
        let (lx, ly) = self.local(x, y);
        if (lx.abs() - MOUTH_HALF).max(0.0).hypot(ly - MOUTH_Y) <= MOUTH_R {
            return Some(MOUTH);
        }
        if (lx.abs() - EYE_X).hypot(ly) <= EYE_R {
            return Some(EYE);
        }
        let e = (lx / SKIN_RX).powi(2) + ((ly - SKIN_CY) / SKIN_RY).powi(2);
        (e <= 1.0).then_some(SKIN)
    }

    /// The five points in ArcFace order (subject's right eye = image-left eye first).
    pub fn landmarks5(&self) -> [Point; 5] {
        let corner = MOUTH_HALF + MOUTH_R;
        [
            self.image_point(-EYE_X, 0.0),
            self.image_point(EYE_X, 0.0),
            self.image_point(0.0, NOSE_Y),
            self.image_point(-corner, MOUTH_Y),
            self.image_point(corner, MOUTH_Y),
        ]
    }

    /// Bounding box of the skin ellipse.
    pub fn bbox(&self) -> Rect {
        let (s, c) = f64::from(self.roll_deg).to_radians().sin_cos();
        let ied = f64::from(self.ied);
        let (a, b) = (SKIN_RX * ied, SKIN_RY * ied);
        let hx = ((a * c).powi(2) + (b * s).powi(2)).sqrt();
        let hy = ((a * s).powi(2) + (b * c).powi(2)).sqrt();
        let centre = self.image_point(0.0, SKIN_CY);
        let (cx, cy) = (f64::from(centre.x), f64::from(centre.y));
        Rect::new(
            (cx - hx) as f32,
            (cy - hy) as f32,
            (2.0 * hx) as f32,
            (2.0 * hy) as f32,
        )
    }

    /// Draw onto `image` with 3x3 supersampling (blending with what is already there).
    pub fn draw(&self, image: &mut RgbImage) {
        let b = self.bbox();
        let (w, h) = image.dimensions();
        let x0 = (b.x.floor().max(0.0) as u32).min(w);
        let y0 = (b.y.floor().max(0.0) as u32).min(h);
        let x1 = (b.right().ceil().max(0.0) as u32).min(w);
        let y1 = (b.bottom().ceil().max(0.0) as u32).min(h);
        for y in y0..y1 {
            for x in x0..x1 {
                let under = image.get_pixel(x, y).0;
                let mut acc = [0.0f64; 3];
                for sy in 0..3 {
                    for sx in 0..3 {
                        let px = f64::from(x) + (f64::from(sx) + 0.5) / 3.0;
                        let py = f64::from(y) + (f64::from(sy) + 0.5) / 3.0;
                        let c = self.colour_at(px, py).unwrap_or(under);
                        for (a, v) in acc.iter_mut().zip(c) {
                            *a += f64::from(v);
                        }
                    }
                }
                image.put_pixel(x, y, Rgb(acc.map(|a| (a / 9.0).round() as u8)));
            }
        }
    }
}

fn fill_rect(image: &mut RgbImage, r: Rect, colour: [u8; 3]) {
    let (w, h) = image.dimensions();
    let x0 = (r.x.round().max(0.0) as u32).min(w);
    let y0 = (r.y.round().max(0.0) as u32).min(h);
    let x1 = (r.right().round().max(0.0) as u32).min(w);
    let y1 = (r.bottom().round().max(0.0) as u32).min(h);
    for y in y0..y1 {
        for x in x0..x1 {
            image.put_pixel(x, y, Rgb(colour));
        }
    }
}

/// A synthetic card in its own (texture) coordinates.
#[derive(Clone, Debug)]
pub struct CardTexture {
    pub image: RgbImage,
    /// The main portrait.
    pub face: Option<SyntheticFace>,
    /// The small secondary ("ghost") portrait.
    pub ghost: Option<SyntheticFace>,
    /// Green marker near the card's top-left corner.
    pub marker: Rect,
}

/// A card `width` px wide with the given aspect ratio: header band, corner marker, portrait photo
/// (with a face when `face`), text lines, and a small ghost face when `ghost`.
pub fn card_texture(width: u32, aspect: f64, face: bool, ghost: bool) -> CardTexture {
    let height = ((f64::from(width) / aspect).round() as u32).max(1);
    let (w, h) = (width as f32, height as f32);
    let mut image = RgbImage::from_pixel(width, height, Rgb(CARD));
    fill_rect(
        &mut image,
        Rect::new(0.04 * w, 0.05 * h, 0.92 * w, 0.11 * h),
        HEADER,
    );
    let marker = Rect::new(0.04 * w, 0.20 * h, 0.05 * w, 0.07 * h);
    fill_rect(&mut image, marker, MARKER);
    fill_rect(
        &mut image,
        Rect::new(0.05 * w, 0.30 * h, 0.31 * w, 0.62 * h),
        PHOTO,
    );
    for (i, len) in [0.46f32, 0.38, 0.50, 0.30].iter().enumerate() {
        let y = (0.32 + 0.11 * i as f32) * h;
        fill_rect(&mut image, Rect::new(0.42 * w, y, len * w, 0.045 * h), INK);
    }
    let main = SyntheticFace::new(0.205 * w, 0.52 * h, 0.075 * w);
    let face = face.then(|| {
        main.draw(&mut image);
        main
    });
    let ghost = ghost.then(|| {
        // 45% of the main portrait: counted as a candidate, never "large" (see
        // LARGE_FACE_RATIO), so it never competes in the orientation search.
        let g = SyntheticFace::new(0.84 * w, 0.80 * h, 0.45 * main.ied);
        g.draw(&mut image);
        g
    });
    CardTexture {
        image,
        face,
        ghost,
        marker,
    }
}

/// Procedural background: a grey-green base with a brightness gradient, soft blotches and mild
/// noise. Its hue stays far from the synthetic face colours. `brightness` shifts all channels.
pub fn background(width: u32, height: u32, seed: u64, brightness: f64) -> RgbImage {
    let mut rng = Rng::new(seed);
    let base = [
        rng.range(60.0, 90.0),
        rng.range(95.0, 125.0),
        rng.range(60.0, 85.0),
    ]
    .map(|v| v + brightness);
    let (gx, gy) = (rng.range(-20.0, 20.0), rng.range(-20.0, 20.0));
    let size = f64::from(width.max(height));
    let blobs: Vec<(f64, f64, f64, f64)> = (0..5)
        .map(|_| {
            (
                rng.range(0.0, f64::from(width)),
                rng.range(0.0, f64::from(height)),
                rng.range(0.08, 0.25) * size,
                rng.range(-25.0, 25.0),
            )
        })
        .collect();
    // Smooth field on an 8-px grid, interpolated bilinearly.
    let (fw, fh) = (width as usize / 8 + 2, height as usize / 8 + 2);
    let field: Vec<f64> = (0..fw * fh)
        .map(|i| {
            let (x, y) = (((i % fw) * 8) as f64, ((i / fw) * 8) as f64);
            gx * (2.0 * x / f64::from(width) - 1.0)
                + gy * (2.0 * y / f64::from(height) - 1.0)
                + blobs
                    .iter()
                    .map(|(bx, by, r, a)| {
                        a * (-((x - bx).powi(2) + (y - by).powi(2)) / (2.0 * r * r)).exp()
                    })
                    .sum::<f64>()
        })
        .collect();
    let mut img = RgbImage::new(width, height);
    for (x, y, p) in img.enumerate_pixels_mut() {
        let (fx, fy) = (f64::from(x) / 8.0, f64::from(y) / 8.0);
        let (ix, iy) = (fx as usize, fy as usize);
        let (tx, ty) = (fx - ix as f64, fy - iy as f64);
        let at = |cx: usize, cy: usize| field[cy * fw + cx];
        let l = (at(ix, iy) * (1.0 - tx) + at(ix + 1, iy) * tx) * (1.0 - ty)
            + (at(ix, iy + 1) * (1.0 - tx) + at(ix + 1, iy + 1) * tx) * ty;
        *p = Rgb(base.map(|b| (b + l + rng.range(-4.0, 4.0)).round().clamp(0.0, 255.0) as u8));
    }
    img
}

/// Camera pose of a card for [`camera_homography`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CardPose {
    /// In-plane rotation, degrees clockwise in the image.
    pub rotation_deg: f64,
    /// Out-of-plane tilts about the card's horizontal and vertical axes, degrees.
    pub tilt_x_deg: f64,
    pub tilt_y_deg: f64,
    /// Width of the (untilted) card in the image, as a fraction of the image width.
    pub width_fraction: f64,
    /// Card centre in the image, as fractions of the image size.
    pub center: (f64, f64),
}

impl Default for CardPose {
    fn default() -> CardPose {
        CardPose {
            rotation_deg: 0.0,
            tilt_x_deg: 0.0,
            tilt_y_deg: 0.0,
            width_fraction: 0.6,
            center: (0.5, 0.5),
        }
    }
}

fn mul3(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut m = [[0.0; 3]; 3];
    for (r, row) in m.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    m
}

/// Homography from card-texture pixels (a `card_w` x `card_h` texture) to the pixels of a
/// `scene_w` x `scene_h` image, for a pinhole camera (focal length = the image's long side)
/// looking at the card in `pose`.
pub fn camera_homography(
    card_w: f64,
    card_h: f64,
    scene_w: f64,
    scene_h: f64,
    pose: &CardPose,
) -> Projective {
    let f = scene_w.max(scene_h);
    let d = f * card_w / (pose.width_fraction * scene_w);
    let (ax, ay, az) = (
        pose.tilt_x_deg.to_radians(),
        pose.tilt_y_deg.to_radians(),
        pose.rotation_deg.to_radians(),
    );
    let rx = [
        [1.0, 0.0, 0.0],
        [0.0, ax.cos(), -ax.sin()],
        [0.0, ax.sin(), ax.cos()],
    ];
    let ry = [
        [ay.cos(), 0.0, ay.sin()],
        [0.0, 1.0, 0.0],
        [-ay.sin(), 0.0, ay.cos()],
    ];
    let rz = [
        [az.cos(), -az.sin(), 0.0],
        [az.sin(), az.cos(), 0.0],
        [0.0, 0.0, 1.0],
    ];
    let r = mul3(&rz, &mul3(&rx, &ry));
    let tx = (pose.center.0 - 0.5) * scene_w * d / f;
    let ty = (pose.center.1 - 0.5) * scene_h * d / f;
    let t = [
        tx - r[0][0] * card_w / 2.0 - r[0][1] * card_h / 2.0,
        ty - r[1][0] * card_w / 2.0 - r[1][1] * card_h / 2.0,
        d - r[2][0] * card_w / 2.0 - r[2][1] * card_h / 2.0,
    ];
    let m = [
        [r[0][0], r[0][1], t[0]],
        [r[1][0], r[1][1], t[1]],
        [r[2][0], r[2][1], t[2]],
    ];
    let k = [
        [f, 0.0, scene_w / 2.0],
        [0.0, f, scene_h / 2.0],
        [0.0, 0.0, 1.0],
    ];
    Projective(mul3(&k, &m))
}

/// Render `card` over `background` through `card_to_scene` (texture pixels to scene pixels),
/// with 3x3 supersampling and ID-1 rounded corners (radius 5.9% of the card height).
pub fn render_card(card: &RgbImage, card_to_scene: &Projective, background: &RgbImage) -> RgbImage {
    let mut out = background.clone();
    let Some(inv) = card_to_scene.invert() else {
        return out;
    };
    let (cw, ch) = (f64::from(card.width()), f64::from(card.height()));
    let radius = 0.059 * ch;
    let inside = |u: f64, v: f64| {
        if !(0.0..=cw).contains(&u) || !(0.0..=ch).contains(&v) {
            return false;
        }
        let dx = (radius - u).max(u - (cw - radius)).max(0.0);
        let dy = (radius - v).max(v - (ch - radius)).max(0.0);
        dx.hypot(dy) <= radius
    };
    let corners: Vec<(f64, f64)> = [(0.0, 0.0), (cw, 0.0), (cw, ch), (0.0, ch)]
        .iter()
        .filter_map(|&(u, v)| card_to_scene.apply_f64(u, v))
        .collect();
    let (w, h) = background.dimensions();
    let clampx = |v: f64| (v.max(0.0) as u32).min(w);
    let clampy = |v: f64| (v.max(0.0) as u32).min(h);
    let x0 = clampx(corners.iter().map(|c| c.0).fold(f64::MAX, f64::min).floor());
    let x1 = clampx(corners.iter().map(|c| c.0).fold(f64::MIN, f64::max).ceil() + 1.0);
    let y0 = clampy(corners.iter().map(|c| c.1).fold(f64::MAX, f64::min).floor());
    let y1 = clampy(corners.iter().map(|c| c.1).fold(f64::MIN, f64::max).ceil() + 1.0);
    for y in y0..y1 {
        for x in x0..x1 {
            let under = background.get_pixel(x, y).0;
            let mut acc = [0.0f64; 3];
            for sy in 0..3 {
                for sx in 0..3 {
                    let px = f64::from(x) + (f64::from(sx) + 0.5) / 3.0;
                    let py = f64::from(y) + (f64::from(sy) + 0.5) / 3.0;
                    let c = match inv.apply_f64(px, py) {
                        Some((u, v)) if inside(u, v) => {
                            let tx = (u as u32).min(card.width() - 1);
                            let ty = (v as u32).min(card.height() - 1);
                            card.get_pixel(tx, ty).0
                        }
                        _ => under,
                    };
                    for (a, v) in acc.iter_mut().zip(c) {
                        *a += f64::from(v);
                    }
                }
            }
            out.put_pixel(x, y, Rgb(acc.map(|a| (a / 9.0).round() as u8)));
        }
    }
    out
}

/// A rendered scene with a card in it.
#[derive(Clone, Debug)]
pub struct CardScene {
    pub image: RgbImage,
    pub card: CardTexture,
    /// Texture pixels to scene pixels.
    pub card_to_scene: Projective,
}

impl CardScene {
    /// The card's own corners in the scene: its top-left, top-right, bottom-right, bottom-left.
    pub fn corners(&self) -> [Point; 4] {
        let (w, h) = (
            self.card.image.width() as f32,
            self.card.image.height() as f32,
        );
        [
            Point::new(0.0, 0.0),
            Point::new(w, 0.0),
            Point::new(w, h),
            Point::new(0.0, h),
        ]
        .map(|p| self.card_to_scene.apply(p).unwrap_or_default())
    }

    /// The main face's eye positions in the scene.
    pub fn face_eyes(&self) -> Option<[Point; 2]> {
        let f = self.card.face?;
        let l = f.landmarks5();
        Some([
            self.card_to_scene.apply(l[0])?,
            self.card_to_scene.apply(l[1])?,
        ])
    }

    /// Bounding box of the main face's skin ellipse in the scene.
    pub fn face_box(&self) -> Option<Rect> {
        let b = self.card.face?.bbox();
        let pts = [
            Point::new(b.x, b.y),
            Point::new(b.right(), b.y),
            Point::new(b.right(), b.bottom()),
            Point::new(b.x, b.bottom()),
        ]
        .map(|p| self.card_to_scene.apply(p).unwrap_or_default());
        let x0 = pts.iter().map(|p| p.x).fold(f32::MAX, f32::min);
        let y0 = pts.iter().map(|p| p.y).fold(f32::MAX, f32::min);
        let x1 = pts.iter().map(|p| p.x).fold(f32::MIN, f32::max);
        let y1 = pts.iter().map(|p| p.y).fold(f32::MIN, f32::max);
        Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
    }
}

/// A `width` x `height` scene with a `card_width`-px texture card of the given aspect ratio
/// (with face and ghost) placed by `pose`, over a [`background`].
pub fn card_scene(width: u32, height: u32, pose: &CardPose, aspect: f64, seed: u64) -> CardScene {
    let card = card_texture(860, aspect, true, true);
    let card_to_scene = camera_homography(
        f64::from(card.image.width()),
        f64::from(card.image.height()),
        f64::from(width),
        f64::from(height),
        pose,
    );
    let bg = background(width, height, seed, 0.0);
    CardScene {
        image: render_card(&card.image, &card_to_scene, &bg),
        card,
        card_to_scene,
    }
}

/// A synthetic face located by colour.
#[derive(Clone, Debug, PartialEq)]
pub struct FoundFace {
    pub bbox: Rect,
    /// Eye centres: the face's own right eye (image-left when upright) first.
    pub eyes: [Point; 2],
    /// Mouth corners in the same order.
    pub mouth_corners: [Point; 2],
    /// Angle of the face's "down" direction from the image's down axis, degrees
    /// (0 upright, +-90 on its side, +-180 upside down).
    pub tilt_deg: f64,
}

impl FoundFace {
    /// Geometry with the nose placed at the synthetic face's proportions.
    pub fn geometry(&self) -> FaceGeometry {
        let mid = self.eyes[0].midpoint(self.eyes[1]);
        let mouth = self.mouth_corners[0].midpoint(self.mouth_corners[1]);
        let k = (NOSE_Y / MOUTH_Y) as f32;
        let nose = Point::new(mid.x + k * (mouth.x - mid.x), mid.y + k * (mouth.y - mid.y));
        let d = self.eyes[1] - self.eyes[0];
        FaceGeometry {
            bbox: self.bbox,
            landmarks5: [
                self.eyes[0],
                self.eyes[1],
                nose,
                self.mouth_corners[0],
                self.mouth_corners[1],
            ],
            inter_eye_distance: d.norm(),
            pose: Some(Pose {
                yaw: 0.0,
                pitch: 0.0,
                roll: d.y.atan2(d.x).to_degrees(),
            }),
        }
    }
}

struct Part {
    sum: (f64, f64),
    count: usize,
    face_label: u32,
}

/// Synthetic faces in `image`, found by colour, in any orientation.
pub fn find_synthetic_faces(image: &RgbImage) -> Vec<FoundFace> {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let near = |p: &[u8; 3], c: [u8; 3], tol: i32| {
        p.iter()
            .zip(c)
            .map(|(a, b)| (i32::from(*a) - i32::from(b)).pow(2))
            .sum::<i32>()
            < tol * tol
    };
    let px: &[[u8; 3]] = image.as_raw().as_chunks::<3>().0;
    let eye_mask: Vec<bool> = px.iter().map(|p| near(p, EYE, 60)).collect();
    let mouth_mask: Vec<bool> = px.iter().map(|p| near(p, MOUTH, 60)).collect();
    let face_mask: Vec<bool> = px
        .iter()
        .zip(eye_mask.iter().zip(&mouth_mask))
        .map(|(p, (e, m))| *e || *m || near(p, SKIN, 50))
        .collect();
    let (face_labels, face_blobs) = label_components(&face_mask, w, h);
    let parts = |mask: &[bool]| {
        let (labels, blobs) = label_components(mask, w, h);
        let mut parts: Vec<Part> = blobs
            .iter()
            .map(|_| Part {
                sum: (0.0, 0.0),
                count: 0,
                face_label: 0,
            })
            .collect();
        for (i, &l) in labels.iter().enumerate() {
            if l > 0 {
                let p = &mut parts[l as usize - 1];
                p.sum.0 += (i % w) as f64 + 0.5;
                p.sum.1 += (i / w) as f64 + 0.5;
                p.count += 1;
                p.face_label = face_labels[i];
            }
        }
        (labels, parts)
    };
    let (_, eyes) = parts(&eye_mask);
    let (mouth_labels, mouths) = parts(&mouth_mask);
    let centroid = |p: &Part| {
        Point::new(
            (p.sum.0 / p.count as f64) as f32,
            (p.sum.1 / p.count as f64) as f32,
        )
    };

    let mut out = Vec::new();
    for blob in face_blobs.iter().filter(|b| b.pixels >= 30) {
        let mut face_eyes: Vec<&Part> = eyes
            .iter()
            .filter(|e| e.face_label == blob.label && e.count >= 3)
            .collect();
        face_eyes.sort_by_key(|e| std::cmp::Reverse(e.count));
        let Some((m_index, mouth)) = mouths
            .iter()
            .enumerate()
            .filter(|(_, m)| m.face_label == blob.label && m.count >= 3)
            .max_by_key(|(_, m)| m.count)
        else {
            continue;
        };
        if face_eyes.len() < 2 {
            continue;
        }
        let (e1, e2) = (centroid(face_eyes[0]), centroid(face_eyes[1]));
        let mid = e1.midpoint(e2);
        let m = centroid(mouth);
        let (dx, dy) = (f64::from(m.x - mid.x), f64::from(m.y - mid.y));
        let tilt_deg = dx.atan2(dy).to_degrees();
        // The face's own left-to-right axis (image +x when upright).
        let len = dx.hypot(dy).max(1e-9);
        let axis = (dy / len, -dx / len);
        let proj = |p: Point| f64::from(p.x) * axis.0 + f64::from(p.y) * axis.1;
        let eyes_sorted = if proj(e1) <= proj(e2) {
            [e1, e2]
        } else {
            [e2, e1]
        };
        let (mut lo, mut hi) = ((f64::MAX, Point::default()), (f64::MIN, Point::default()));
        for (i, &l) in mouth_labels.iter().enumerate() {
            if l as usize == m_index + 1 {
                let p = Point::new((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                let v = proj(p);
                if v < lo.0 {
                    lo = (v, p);
                }
                if v > hi.0 {
                    hi = (v, p);
                }
            }
        }
        out.push(FoundFace {
            bbox: Rect::new(
                blob.min_x as f32,
                blob.min_y as f32,
                (blob.max_x - blob.min_x + 1) as f32,
                (blob.max_y - blob.min_y + 1) as f32,
            ),
            eyes: eyes_sorted,
            mouth_corners: [lo.1, hi.1],
            tilt_deg,
        });
    }
    out
}

/// Fake [`FaceFinder`]: reports synthetic faces with score 0.95 when upright (within 45 degrees),
/// 0.4 when on their side, and not at all when upside down.
#[derive(Clone, Copy, Debug, Default)]
pub struct ColourFaceFinder;

impl FaceFinder for ColourFaceFinder {
    fn find_faces(&self, image: &RgbImage) -> Result<Vec<DetectedFace>, BoxError> {
        Ok(find_synthetic_faces(image)
            .into_iter()
            .filter_map(|f| {
                let score = match f.tilt_deg.abs() {
                    t if t <= 45.0 => 0.95,
                    t if t <= 135.0 => 0.4,
                    _ => return None,
                };
                Some(DetectedFace {
                    bbox: f.bbox,
                    score,
                    eyes: f.eyes,
                })
            })
            .collect())
    }
}

/// The upright synthetic face whose eye midpoint is nearest `near`, as geometry.
pub fn synthetic_geometry(image: &RgbImage, near: Point) -> Option<FaceGeometry> {
    find_synthetic_faces(image)
        .into_iter()
        .filter(|f| f.tilt_deg.abs() <= 45.0)
        .min_by(|a, b| {
            let da = a.eyes[0].midpoint(a.eyes[1]).distance(near);
            let db = b.eyes[0].midpoint(b.eyes[1]).distance(near);
            da.total_cmp(&db)
        })
        .map(|f| f.geometry())
}

/// Fake [`PortraitAnalyzer`]: measures the synthetic face nearest the detection and returns
/// passing values for the `portrait`-subject checks.
#[derive(Clone, Copy, Debug, Default)]
pub struct ColourPortraitAnalyzer;

impl PortraitAnalyzer for ColourPortraitAnalyzer {
    fn analyze_portrait(
        &self,
        crop: &RgbImage,
        face: &DetectedFace,
    ) -> Result<FaceAnalysis, BoxError> {
        let geometry = synthetic_geometry(crop, face.eye_midpoint());
        Ok(FaceAnalysis {
            measurements: portrait_measurements(geometry.as_ref()),
            face: geometry,
        })
    }
}

/// Passing `portrait`-subject measurements for a face, or "no landmarks" without one.
pub fn portrait_measurements(face: Option<&FaceGeometry>) -> Measurements {
    let mut m = Measurements::new();
    let Some(face) = face else {
        m.set_undetermined(CheckId::InterEyeDistance, "no landmarks");
        return m;
    };
    m.set_value(
        CheckId::InterEyeDistance,
        f64::from(face.inter_eye_distance),
    );
    for (id, v) in [
        (CheckId::HeadPoseYaw, 0.0),
        (CheckId::HeadPosePitch, 0.0),
        (CheckId::HeadPoseRoll, 0.0),
        (CheckId::EyesVisible, 1.0),
        (CheckId::EyesOpen, 0.9),
        (CheckId::MouthClosed, 0.1),
        (CheckId::FaceOcclusion, 0.0),
        (CheckId::LuminanceMean, 0.6),
        (CheckId::UnderExposure, 0.0),
        (CheckId::OverExposure, 0.0),
        (CheckId::DynamicRange, 7.0),
        (CheckId::IlluminationUniformity, 0.9),
        (CheckId::Sharpness, 0.8),
    ] {
        m.set_value(id, v);
    }
    m
}

/// Passing `selfie`-subject measurements for an image with one face.
pub fn selfie_measurements(image: &RgbImage, face: &FaceGeometry) -> Measurements {
    let mut m = Measurements::new();
    m.set_value(
        CheckId::ImageResolution,
        f64::from(image.width().min(image.height())),
    );
    m.set_value(
        CheckId::InterEyeDistance,
        f64::from(face.inter_eye_distance),
    );
    for (id, v) in [
        (CheckId::CompressionQuality, 95.0),
        (CheckId::FacePresent, 1.0),
        (CheckId::SingleFace, 1.0),
        (CheckId::HeadInFrame, 0.2),
        (CheckId::HeadPoseYaw, 0.0),
        (CheckId::HeadPosePitch, 0.0),
        (CheckId::HeadPoseRoll, 0.0),
        (CheckId::EyesVisible, 1.0),
        (CheckId::EyesOpen, 0.9),
        (CheckId::MouthClosed, 0.1),
        (CheckId::ExpressionNeutral, 0.1),
        (CheckId::FaceOcclusion, 0.0),
        (CheckId::LuminanceMean, 0.5),
        (CheckId::UnderExposure, 0.0),
        (CheckId::OverExposure, 0.0),
        (CheckId::DynamicRange, 7.0),
        (CheckId::IlluminationUniformity, 0.8),
        (CheckId::Sharpness, 0.8),
    ] {
        m.set_value(id, v);
    }
    m
}
