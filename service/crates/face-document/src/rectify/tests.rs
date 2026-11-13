//! Rectification tests on synthetic scenes (cards drawn through a pinhole-camera homography).

use face_imaging::geom::Point;
use image::Rgb;

use super::*;
use crate::testing::{
    CardPose, MARKER, SyntheticFace, background, camera_homography, card_scene, card_texture,
    render_card,
};

/// Largest corner error after matching the detected corners to the true ones (any cyclic
/// order, same winding).
fn corner_error(found: &Quad, truth: &[Point; 4]) -> f32 {
    (0..4)
        .map(|shift| {
            (0..4)
                .map(|k| found.0[k].distance(truth[(k + shift) % 4]))
                .fold(0.0f32, f32::max)
        })
        .fold(f32::MAX, f32::min)
}

fn diagonal(q: &[Point; 4]) -> f32 {
    q[0].distance(q[2]).max(q[1].distance(q[3]))
}

fn pose(rotation_deg: f64, tilt_x_deg: f64, tilt_y_deg: f64) -> CardPose {
    CardPose {
        rotation_deg,
        tilt_x_deg,
        tilt_y_deg,
        width_fraction: 0.55,
        center: (0.5, 0.5),
    }
}

#[test]
fn finds_id1_cards_under_perspective_at_several_rotations() {
    let poses = [
        pose(0.0, 0.0, 0.0),
        pose(12.0, 10.0, -8.0),
        pose(90.0, -12.0, 6.0),
        pose(180.0, 8.0, 12.0),
        pose(205.0, -10.0, -10.0),
        pose(270.0, 15.0, 0.0),
        pose(-20.0, 0.0, 15.0),
    ];
    for (i, p) in poses.iter().enumerate() {
        let scene = card_scene(900, 700, p, ID1_ASPECT, 100 + i as u64);
        let d = detect_document(&scene.image);
        let c = d.candidate.as_ref().expect("a candidate");
        let truth = scene.corners();
        let err = corner_error(&c.quad, &truth);
        let rel = err / diagonal(&truth);
        eprintln!(
            "pose {i}: confidence {:.3}, corner error {err:.2} px ({rel:.4}), aspect {:.3}, support {:.3}, fit {:.3}",
            d.confidence, c.aspect, c.edge_support, c.fit
        );
        assert!(
            d.detected() && d.confidence >= 0.8,
            "pose {i}: confidence {} ({c:?})",
            d.confidence
        );
        assert!(rel <= 0.006, "pose {i}: corner error {err} px ({rel})");
        assert!(
            (c.aspect / ID1_ASPECT - 1.0).abs() < 0.08,
            "pose {i}: {c:?}"
        );
        // Image-space order: clockwise, starting nearest the image origin.
        let q = c.quad.0;
        assert!(q.iter().all(|p| p.x + p.y >= q[0].x + q[0].y - 1e-3));
    }
}

#[test]
fn finds_a_td3_page_and_a_large_high_resolution_card() {
    let scene = card_scene(900, 700, &pose(-8.0, 6.0, 6.0), TD3_ASPECT, 7);
    let d = detect_document(&scene.image);
    assert!(d.detected() && d.confidence >= 0.8, "{d:?}");
    let truth = scene.corners();
    let err = corner_error(&d.candidate.unwrap().quad, &truth);
    assert!(err / diagonal(&truth) <= 0.006, "corner error {err} px");

    // 2400 px wide: detection runs on a 1024 px working copy; corners come back in input pixels.
    let big = CardPose {
        width_fraction: 0.6,
        ..pose(6.0, -9.0, 7.0)
    };
    let scene = card_scene(2400, 1800, &big, ID1_ASPECT, 8);
    let d = detect_document(&scene.image);
    let truth = scene.corners();
    let err = corner_error(&d.candidate.as_ref().unwrap().quad, &truth);
    eprintln!(
        "2400 px scene: confidence {:.3}, corner error {err:.2} px",
        d.confidence
    );
    assert!(d.detected(), "{d:?}");
    assert!(err / diagonal(&truth) <= 0.006, "corner error {err} px");
}

/// Fill an ellipse (axis-aligned semi-axes `rx`, `ry`) with a flat colour.
fn fill_ellipse(img: &mut RgbImage, cx: f32, cy: f32, rx: f32, ry: f32, colour: [u8; 3]) {
    for (x, y, p) in img.enumerate_pixels_mut() {
        let (dx, dy) = ((x as f32 + 0.5 - cx) / rx, (y as f32 + 0.5 - cy) / ry);
        if dx * dx + dy * dy <= 1.0 {
            *p = Rgb(colour);
        }
    }
}

#[test]
fn tolerates_low_contrast_and_a_finger_over_one_edge() {
    let p = pose(-6.0, 8.0, 10.0);
    let card = card_texture(860, ID1_ASPECT, true, true);
    let h = camera_homography(860.0, 542.0, 900.0, 700.0, &p);
    // Light background: about 60 grey levels below the card instead of about 130.
    let mut img = render_card(&card.image, &h, &background(900, 700, 21, 70.0));
    let truth = [(0.0, 0.0), (860.0, 0.0), (860.0, 542.0), (0.0, 542.0)]
        .map(|(u, v)| h.apply(Point::new(u, v)).unwrap());
    // A thumb across the bottom edge, 40% along it.
    let on_edge = truth[3] + (truth[2] - truth[3]) * 0.4;
    fill_ellipse(&mut img, on_edge.x, on_edge.y, 28.0, 60.0, [196, 150, 122]);
    let d = detect_document(&img);
    let c = d.candidate.as_ref().expect("a candidate");
    let err = corner_error(&c.quad, &truth);
    eprintln!(
        "low contrast + finger: confidence {:.3}, corner error {err:.2} px, support {:.3}",
        d.confidence, c.edge_support
    );
    assert!(d.detected() && d.confidence >= 0.6, "{d:?}");
    assert!(err / diagonal(&truth) <= 0.01, "corner error {err} px");
}

#[test]
fn low_confidence_without_a_card_shaped_outline() {
    // A tight portrait: a face on a plain background.
    let mut portrait = background(480, 640, 3, 60.0);
    SyntheticFace::new(240.0, 280.0, 110.0).draw(&mut portrait);
    let d = detect_document(&portrait);
    assert!(!d.detected() && d.confidence < 0.3, "{d:?}");

    // A square card: right outline, wrong aspect ratio.
    let square = card_texture(600, 1.0, true, false);
    let h = camera_homography(600.0, 600.0, 900.0, 700.0, &pose(5.0, 0.0, 0.0));
    let img = render_card(&square.image, &h, &background(900, 700, 4, 0.0));
    let d = detect_document(&img);
    assert!(d.confidence < 0.2, "{d:?}");

    // Nothing at all.
    let flat = RgbImage::from_pixel(640, 480, Rgb([120, 120, 120]));
    assert_eq!(detect_document(&flat).confidence, 0.0);
}

#[test]
fn busy_texture_is_not_a_document() {
    let mut img = background(900, 700, 31, 20.0);
    let mut rng = crate::testing::Rng::new(77);
    for _ in 0..900 {
        let (x, y) = (rng.range(0.0, 900.0), rng.range(0.0, 700.0));
        let (angle, len) = (rng.range(0.0, std::f64::consts::PI), rng.range(8.0, 45.0));
        let shade = rng.range(30.0, 230.0) as u8;
        for t in 0..(len as i32) {
            let px = x + f64::from(t) * angle.cos();
            let py = y + f64::from(t) * angle.sin();
            if (0.0..899.0).contains(&px) && (0.0..699.0).contains(&py) {
                img.put_pixel(px as u32, py as u32, Rgb([shade, shade, shade]));
                img.put_pixel(px as u32 + 1, py as u32, Rgb([shade, shade, shade]));
            }
        }
    }
    let d = detect_document(&img);
    eprintln!("busy texture: confidence {:.3}", d.confidence);
    assert!(!d.detected(), "{d:?}");
}

#[test]
fn rectified_frame_never_upscales_and_keeps_the_card_aspect() {
    let scene = card_scene(1200, 900, &pose(10.0, 18.0, -12.0), ID1_ASPECT, 11);
    let d = detect_document(&scene.image);
    let quad = d.candidate.unwrap().quad;
    let frame = rectified_frame(&quad).unwrap();
    for gy in 0..=10 {
        for gx in 0..=10 {
            let x = f64::from(frame.width) * f64::from(gx) / 10.0;
            let y = f64::from(frame.height) * f64::from(gy) / 10.0;
            let s = frame.to_input.min_stretch(x, y).unwrap();
            assert!(s >= 0.999, "upscaled at ({x}, {y}): {s}");
        }
    }
    let side = |a: usize, b: usize| quad.0[a].distance(quad.0[b]);
    assert!(frame.width as f32 <= side(0, 1).max(side(3, 2)) + 1.0);
    assert!(frame.height as f32 <= side(0, 3).max(side(1, 2)) + 1.0);
    let aspect =
        f64::from(frame.width.max(frame.height)) / f64::from(frame.width.min(frame.height));
    assert!((aspect / ID1_ASPECT - 1.0).abs() < 0.08, "{aspect}");
}

#[test]
fn rectified_card_shows_the_marker_where_the_texture_has_it() {
    let scene = card_scene(1000, 760, &pose(4.0, 9.0, -7.0), ID1_ASPECT, 12);
    let d = detect_document(&scene.image);
    let out = rectify(&scene.image, &d.candidate.unwrap().quad).unwrap();
    let (w, h) = (out.width() as f32, out.height() as f32);
    let (mut sx, mut sy, mut n) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y, p) in out.enumerate_pixels() {
        let d2: i32 =
            p.0.iter()
                .zip(MARKER)
                .map(|(a, b)| (i32::from(*a) - i32::from(b)).pow(2))
                .sum();
        if d2 < 40 * 40 {
            (sx, sy, n) = (sx + x as f32 + 0.5, sy + y as f32 + 0.5, n + 1.0);
        }
    }
    assert!(n > 50.0, "marker not found");
    let m = scene.card.marker;
    let (ex, ey) = (
        (m.x + 0.5 * m.width) / scene.card.image.width() as f32,
        (m.y + 0.5 * m.height) / scene.card.image.height() as f32,
    );
    assert!((sx / n / w - ex).abs() < 0.01, "{} vs {ex}", sx / n / w);
    assert!((sy / n / h - ey).abs() < 0.015, "{} vs {ey}", sy / n / h);
}

#[test]
fn coverage_of_a_rectangle_by_a_quad() {
    let quad = Quad([
        Point::new(0.0, 0.0),
        Point::new(100.0, 0.0),
        Point::new(100.0, 50.0),
        Point::new(0.0, 50.0),
    ]);
    assert!((coverage(&quad, &Rect::new(10.0, 10.0, 20.0, 20.0)) - 1.0).abs() < 1e-9);
    assert!((coverage(&quad, &Rect::new(90.0, 40.0, 20.0, 20.0)) - 0.25).abs() < 1e-9);
    assert_eq!(coverage(&quad, &Rect::new(200.0, 0.0, 5.0, 5.0)), 0.0);
    assert!((quad_area(&quad) - 5000.0).abs() < 1e-9);
}
