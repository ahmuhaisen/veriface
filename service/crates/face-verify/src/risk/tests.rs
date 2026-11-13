//! Risk-check tests on synthetic images.

use face_document::testing::{
    CardPose, Rng, SyntheticFace, background, camera_homography, card_texture, render_card,
    synthetic_geometry,
};
use face_document::{FaceGeometry, Pose};
use face_imaging::RgbImage;
use image::Rgb;

use super::*;
use crate::align::align_face;

fn draw(face: SyntheticFace, w: u32, h: u32, seed: u64) -> (RgbImage, FaceGeometry) {
    let mut img = background(w, h, seed, 30.0);
    face.draw(&mut img);
    let g = synthetic_geometry(&img, face.center).expect("synthetic face");
    (img, g)
}

#[test]
fn duplicate_score_combines_the_signals() {
    let identical = DuplicateSignals {
        similarity: 0.99,
        ssim: 0.97,
        points_rms: 0.1,
        pose_delta_deg: Some(0.2),
    };
    assert_eq!(identical.score(), 1.0);
    let genuine = DuplicateSignals {
        similarity: 0.62,
        ssim: 0.41,
        points_rms: 2.6,
        pose_delta_deg: Some(7.0),
    };
    assert_eq!(genuine.score(), 0.0);
    // Two frontal captures with matching geometry are not duplicates on geometry alone.
    let frontal = DuplicateSignals {
        similarity: 0.6,
        ssim: 0.4,
        points_rms: 0.0,
        pose_delta_deg: Some(0.0),
    };
    assert!((frontal.score() - 0.2).abs() < 1e-9);
    // A recapture: similarity above the genuine range, fairly similar crops.
    let recapture = DuplicateSignals {
        similarity: 0.91,
        ssim: 0.7,
        points_rms: 0.6,
        pose_delta_deg: None,
    };
    assert!(recapture.score() > 0.5, "{}", recapture.score());
}

#[test]
fn identical_images_score_high_and_different_captures_low() {
    let (img, g) = draw(SyntheticFace::new(320.0, 200.0, 70.0), 640, 480, 1);
    let a = align_face(&img, &g.landmarks5).unwrap();
    let same = DuplicateSignals::measure(1.0, &a, &g, &a, &g).unwrap();
    assert!(same.score() >= 0.95, "{same:?}");

    // Another capture: different size, roll, position, background and lighting texture, a few
    // degrees of yaw, and a genuine-range similarity.
    let other = SyntheticFace {
        roll_deg: 9.0,
        ..SyntheticFace::new(250.0, 230.0, 55.0)
    };
    let (mut img2, mut g2) = draw(other, 640, 480, 2);
    let mut rng = Rng::new(5);
    for p in img2.pixels_mut() {
        let d = rng.range(-35.0, 35.0);
        *p = Rgb(p.0.map(|v| (f64::from(v) + d).clamp(0.0, 255.0) as u8));
    }
    g2.pose = Some(Pose {
        yaw: 7.0,
        pitch: -3.0,
        roll: 9.0,
    });
    let b = align_face(&img2, &g2.landmarks5).unwrap();
    let different = DuplicateSignals::measure(0.58, &a, &g, &b, &g2).unwrap();
    assert!(
        different.score() <= 0.3,
        "{different:?} -> {}",
        different.score()
    );
}

#[test]
fn a_card_around_the_face_is_a_document_present() {
    // The "selfie" is a photo of a card whose portrait is the face.
    let card = card_texture(860, 1.586, true, false);
    let pose = CardPose {
        rotation_deg: -7.0,
        tilt_x_deg: 10.0,
        tilt_y_deg: -12.0,
        width_fraction: 0.7,
        center: (0.5, 0.5),
    };
    let h = camera_homography(860.0, 542.0, 960.0, 720.0, &pose);
    let img = render_card(&card.image, &h, &background(960, 720, 9, 0.0));
    let eyes = card.face.unwrap().landmarks5();
    let mid = h.apply(eyes[0].midpoint(eyes[1])).unwrap();
    let g = synthetic_geometry(&img, mid).expect("card portrait");
    let p = document_presence(&img, &face_box(&g));
    assert!(p >= 0.7, "{p}");

    // The same face without a card around it.
    let (plain, pg) = draw(SyntheticFace::new(480.0, 330.0, 45.0), 960, 720, 10);
    assert!(document_presence(&plain, &face_box(&pg)) <= 0.2);

    // A card in the picture that does not enclose the face.
    let blank = card_texture(860, 1.586, false, false);
    let left = CardPose {
        width_fraction: 0.4,
        center: (0.28, 0.5),
        ..CardPose::default()
    };
    let hl = camera_homography(860.0, 542.0, 960.0, 720.0, &left);
    let mut beside = render_card(&blank.image, &hl, &background(960, 720, 11, 0.0));
    let face = SyntheticFace::new(720.0, 300.0, 50.0);
    face.draw(&mut beside);
    let bg = synthetic_geometry(&beside, face.center).unwrap();
    let p = document_presence(&beside, &face_box(&bg));
    assert!(p <= 0.2, "{p}");
}

/// A large synthetic face with mild sensor noise.
fn portrait(seed: u64) -> (RgbImage, FaceGeometry) {
    let (mut img, g) = draw(SyntheticFace::new(320.0, 220.0, 120.0), 640, 640, seed);
    let mut rng = Rng::new(seed + 100);
    for p in img.pixels_mut() {
        let d = rng.range(-6.0, 6.0);
        *p = Rgb(p.0.map(|v| (f64::from(v) + d).clamp(0.0, 255.0) as u8));
    }
    (img, g)
}

fn add(img: &mut RgbImage, f: impl Fn(u32, u32) -> f64) {
    for (x, y, p) in img.enumerate_pixels_mut() {
        let d = f(x, y);
        *p = Rgb(p.0.map(|v| (f64::from(v) + d).clamp(0.0, 255.0) as u8));
    }
}

#[test]
fn screen_artifacts_flag_moire_and_pixel_grids_but_not_jpeg_blocks() {
    let (clean, g) = portrait(20);
    let face = face_box(&g);
    let base = screen_artifacts(&clean, &face).unwrap();
    assert!(base <= 0.2, "clean: {base}");

    let tau = 2.0 * std::f64::consts::PI;
    let mut moire = clean.clone();
    add(&mut moire, |x, y| {
        12.0 * (tau * (0.31 * f64::from(x) + 0.21 * f64::from(y))).sin()
    });
    let m = screen_artifacts(&moire, &face).unwrap();
    assert!(m >= 0.8, "moire: {m}");

    // A screen's pixel lattice seen slightly rotated: period about 3 px in two directions.
    let mut grid = clean.clone();
    add(&mut grid, |x, y| {
        let (x, y) = (f64::from(x), f64::from(y));
        8.0 * ((tau * (0.33 * x + 0.04 * y)).cos() + (tau * (-0.04 * x + 0.33 * y)).cos())
    });
    let gr = screen_artifacts(&grid, &face).unwrap();
    assert!(gr >= 0.8, "pixel grid: {gr}");

    // 8x8 block offsets, as heavy JPEG compression leaves them.
    let mut rng = Rng::new(3);
    let offsets: Vec<f64> = (0..80 * 80).map(|_| rng.range(-6.0, 6.0)).collect();
    let mut blocky = clean.clone();
    add(&mut blocky, |x, y| {
        offsets[(y as usize / 8) * 80 + x as usize / 8]
    });
    let b = screen_artifacts(&blocky, &face).unwrap();
    eprintln!("screen artifacts: clean {base}, moire {m}, grid {gr}, blocks {b}");
    assert!(b <= 0.3, "jpeg blocks: {b}");

    // Too small to analyse.
    let tiny = Rect::new(10.0, 10.0, 30.0, 30.0);
    assert!(screen_artifacts(&clean, &tiny).is_none());
}

#[test]
fn powers_of_two() {
    assert_eq!(prev_power_of_two(0), 0);
    assert_eq!(prev_power_of_two(1), 1);
    assert_eq!(prev_power_of_two(255), 128);
    assert_eq!(prev_power_of_two(256), 256);
}
