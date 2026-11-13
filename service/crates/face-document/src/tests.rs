//! Pipeline tests on synthetic scenes, with the colour-based fake face finder and analyzer.

use face_core::{
    Catalogue, CheckId, DOCUMENT_ASSESSMENT, Measurement, Profile, Status, Verdict, evaluate,
};
use face_imaging::RgbImage;
use face_imaging::geom::Rect;

use crate::rectify::ID1_ASPECT;
use crate::testing::{
    CardPose, CardScene, ColourFaceFinder, ColourPortraitAnalyzer, SyntheticFace, background,
    camera_homography, card_scene, card_texture, render_card,
};
use crate::{
    BoxError, DetectedFace, DocumentError, DocumentPortrait, FaceFinder, OutputSize, ProcessedAs,
    ReferenceKind, Rotation, extract_portrait,
};

const STANDARD: &str = include_str!("../../../../profiles/standard.yaml");
const OUTPUT: OutputSize = OutputSize {
    width: 360,
    height: 480,
};

fn run(image: &RgbImage, kind: ReferenceKind) -> DocumentPortrait {
    extract_portrait(
        image,
        kind,
        &ColourFaceFinder,
        &ColourPortraitAnalyzer,
        Some(OUTPUT),
    )
    .unwrap()
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

fn value(m: &face_core::Measurements, id: CheckId) -> Option<f64> {
    m.value(id)
}

/// Centre distance and IoU between two boxes.
fn box_agreement(a: &Rect, b: &Rect) -> (f32, f32) {
    (a.center().distance(b.center()), a.iou(b))
}

fn portrait_image(face: SyntheticFace, w: u32, h: u32) -> RgbImage {
    let mut img = background(w, h, 5, 60.0);
    face.draw(&mut img);
    img
}

#[test]
fn finds_the_upright_portrait_on_rotated_cards_and_maps_it_back_to_the_input() {
    for (i, rotation) in [0.0, 90.0, 180.0, 270.0, 12.0, 200.0]
        .into_iter()
        .enumerate()
    {
        let scene: CardScene = card_scene(
            900,
            700,
            &pose(rotation, 8.0, -6.0),
            ID1_ASPECT,
            40 + i as u64,
        );
        let r = run(&scene.image, ReferenceKind::Document);
        let info = r.document.as_ref().unwrap();
        let expected = match ((rotation / 90.0).round() as i32).rem_euclid(4) {
            0 => Rotation::R0,
            1 => Rotation::R270,
            2 => Rotation::R180,
            _ => Rotation::R90,
        };
        assert_eq!(r.kind, ProcessedAs::Document);
        assert!(
            info.detected && info.corners.is_some(),
            "rotation {rotation}: {info:?}"
        );
        assert_eq!(info.orientation_degrees, expected, "rotation {rotation}");
        // Corners in DOCUMENT order: the card's own top-left first, whatever its rotation.
        let truth = scene.corners();
        let corners = info.corners.unwrap().0;
        for (k, (c, t)) in corners.iter().zip(&truth).enumerate() {
            assert!(
                c.distance(*t) < 1.0,
                "rotation {rotation}: corner {k} {c:?} vs {t:?}"
            );
        }
        // Main portrait and ghost portrait are both candidates; the large one is selected.
        assert_eq!(value(&r.measurements, CheckId::PortraitFound), Some(2.0));
        let portrait = r.portrait.as_ref().expect("portrait");
        let face = portrait.crop_face.as_ref().expect("landmarks on the crop");
        let (dist, iou) = box_agreement(&portrait.source_box, &scene.face_box().unwrap());
        assert!(
            dist < 6.0 && iou > 0.6,
            "rotation {rotation}: source box {:?} vs {:?}",
            portrait.source_box,
            scene.face_box()
        );
        // The analysed crop is upright: the subject's right eye is image-left.
        assert!(face.landmarks5[0].x < face.landmarks5[1].x);
        assert!(face.landmarks5[3].y > face.landmarks5[0].y);
        // Never upscaled: the inter-eye distance on the crop is not larger than in the input.
        let eyes = scene.face_eyes().unwrap();
        let input_ied = eyes[0].distance(eyes[1]);
        assert!(
            face.inter_eye_distance <= 1.03 * input_ied
                && face.inter_eye_distance >= 0.85 * input_ied,
            "rotation {rotation}: ied {} vs input {input_ied}",
            face.inter_eye_distance
        );
        // The API geometry is in input coordinates: the subject's right and left eye land on
        // the card portrait's eyes in the input, and the box encloses them.
        let input_face = portrait.face.as_ref().expect("input-space geometry");
        assert!(
            input_face.landmarks5[0].distance(eyes[0]) < 2.0,
            "rotation {rotation}"
        );
        assert!(
            input_face.landmarks5[1].distance(eyes[1]) < 2.0,
            "rotation {rotation}"
        );
        assert!((input_face.inter_eye_distance - input_ied).abs() < 2.0);
        assert!(input_face.bbox.contains(eyes[0]) && input_face.bbox.contains(eyes[1]));
        let back = portrait.crop_to_input.apply(face.landmarks5[0]).unwrap();
        assert!(back.distance(input_face.landmarks5[0]) < 1e-3);
    }
}

#[test]
fn document_order_matches_the_upright_frame() {
    use crate::portrait::document_order;
    use crate::rectify::rectified_frame;
    use face_imaging::geom::{Point, Quad};
    // An outline in image-space order, and its rectified frame.
    let quad = Quad([
        Point::new(100.0, 80.0),
        Point::new(520.0, 120.0),
        Point::new(500.0, 400.0),
        Point::new(90.0, 360.0),
    ]);
    let frame = rectified_frame(&quad).unwrap();
    let (fw, fh) = (f64::from(frame.width), f64::from(frame.height));
    for r in Rotation::ALL {
        // The upright frame's corners, mapped back to the input.
        let upright = r.transform(fw, fh);
        let to_input = upright.invert().unwrap().then(&frame.to_input);
        let (uw, uh) = r.rotated_size(frame.width, frame.height);
        let (uw, uh) = (uw as f32, uh as f32);
        let expected = [
            Point::new(0.0, 0.0),
            Point::new(uw, 0.0),
            Point::new(uw, uh),
            Point::new(0.0, uh),
        ]
        .map(|p| to_input.apply(p).unwrap());
        let got = document_order(quad, r).0;
        for (g, e) in got.iter().zip(&expected) {
            assert!(g.distance(*e) < 1e-2, "{r:?}: {got:?} vs {expected:?}");
        }
    }
}

#[test]
fn reports_measurements_that_pass_the_standard_profile_and_a_never_upscaled_face_crop() {
    let cat = Catalogue::embedded();
    let profile = Profile::from_yaml(STANDARD, cat).unwrap();
    // Large enough for the profile's INTER_EYE_DISTANCE >= 60.
    let p = CardPose {
        width_fraction: 0.6,
        ..pose(3.0, 6.0, 5.0)
    };
    let scene = card_scene(1800, 1400, &p, ID1_ASPECT, 50);
    let r = run(&scene.image, ReferenceKind::Document);
    let a = evaluate(
        cat,
        DOCUMENT_ASSESSMENT,
        &profile.document.checks,
        &r.measurements,
    );
    assert_eq!(a.verdict, Verdict::Accepted, "{:#?}", a.checks);
    assert_eq!(a.status(CheckId::DocumentDetected), Some(Status::Pass));
    assert_eq!(a.status(CheckId::ImageResolution), Some(Status::Pass));
    // COMPRESSION_QUALITY comes from the codec, so it is not measured here.
    assert_eq!(
        a.status(CheckId::CompressionQuality),
        Some(Status::Undetermined)
    );

    let out = r.output.as_ref().unwrap();
    assert!(out.width() <= OUTPUT.width && out.height() <= OUTPUT.height);
    let crop = &r.portrait.as_ref().unwrap().crop;
    assert!(out.width() <= crop.width() && out.height() <= crop.height());
    let aspect = out.width() as f32 / out.height() as f32;
    assert!((aspect - 0.75).abs() < 0.02, "{aspect}");

    let json = serde_json::to_value(r.document.as_ref().unwrap()).unwrap();
    assert_eq!(json["detected"], true);
    assert_eq!(json["orientationDegrees"], 0);
    assert_eq!(json["corners"].as_array().unwrap().len(), 4);
    assert_eq!(json["corners"][0].as_array().unwrap().len(), 2);
}

#[test]
fn small_portraits_are_cropped_at_native_resolution() {
    let face = SyntheticFace::new(200.0, 150.0, 30.0);
    let r = run(&portrait_image(face, 400, 320), ReferenceKind::Portrait);
    let out = r.output.unwrap();
    // 5.2 IED = 156 px high: smaller than the 480 px maximum, so not upscaled.
    assert!(
        out.height() <= 157 && out.width() <= 118,
        "{:?}",
        out.dimensions()
    );
    let p = r.portrait.unwrap();
    for g in [p.face.unwrap(), p.crop_face.unwrap()] {
        assert!(
            (g.inter_eye_distance - 30.0).abs() < 1.0,
            "{}",
            g.inter_eye_distance
        );
    }
}

#[test]
fn portrait_kind_skips_rectification() {
    let face = SyntheticFace::new(240.0, 260.0, 90.0);
    let img = portrait_image(face, 480, 640);
    let r = run(&img, ReferenceKind::Portrait);
    assert_eq!(r.kind, ProcessedAs::Portrait);
    assert!(r.document.is_none());
    assert!(matches!(
        r.measurements.get(CheckId::DocumentDetected),
        Some(Measurement::Undetermined(_))
    ));
    assert_eq!(value(&r.measurements, CheckId::PortraitFound), Some(1.0));
    let p = r.portrait.unwrap();
    let (dist, iou) = box_agreement(&p.source_box, &face.bbox());
    assert!(
        dist < 3.0 && iou > 0.8,
        "{:?} vs {:?}",
        p.source_box,
        face.bbox()
    );
    // No rectification: the crop is a pixel copy, so the eyes are where they were drawn, and the
    // input-space geometry is the crop's shifted by the crop offset.
    let g = p.face.unwrap();
    assert!(g.landmarks5[0].distance(face.landmarks5()[0]) < 0.6);
    let back = p
        .crop_to_input
        .apply(p.crop_face.unwrap().landmarks5[0])
        .unwrap();
    assert!(back.distance(g.landmarks5[0]) < 1e-3);
}

#[test]
fn portrait_kind_still_searches_orientations() {
    let face = SyntheticFace {
        roll_deg: 90.0,
        ..SyntheticFace::new(320.0, 240.0, 70.0)
    };
    let r = run(&portrait_image(face, 640, 480), ReferenceKind::Portrait);
    // The face lies on its side (rolled clockwise); a counter-clockwise turn makes it upright.
    assert_eq!(value(&r.measurements, CheckId::PortraitFound), Some(1.0));
    let p = r.portrait.unwrap();
    let crop = p.crop_face.unwrap();
    assert!(
        crop.landmarks5[3].y > crop.landmarks5[0].y + 40.0,
        "upright on the crop"
    );
    // In input coordinates the face is still on its side: the mouth is image-left of the eyes.
    let input = p.face.unwrap();
    assert!(input.landmarks5[3].x < input.landmarks5[0].x - 40.0);
    // Eyes are disc centroids (sub-pixel); the fake's mouth corners are extreme pixel centres
    // inside the mouth bar, so they sit up to about 2 px from the drawn corner.
    for (k, (got, drawn)) in input.landmarks5.iter().zip(face.landmarks5()).enumerate() {
        let tolerance = if k < 2 { 1.0 } else { 2.5 };
        assert!(got.distance(drawn) < tolerance, "{k}: {got:?} vs {drawn:?}");
    }
}

#[test]
fn auto_uses_the_document_when_one_is_found() {
    let scene = card_scene(900, 700, &pose(-15.0, 10.0, 0.0), ID1_ASPECT, 60);
    let r = run(&scene.image, ReferenceKind::Auto);
    assert_eq!(r.kind, ProcessedAs::Document);
    assert!(r.document.as_ref().unwrap().detected);
    assert!(value(&r.measurements, CheckId::DocumentDetected).unwrap() >= 0.8);
}

#[test]
fn auto_falls_back_to_portrait_without_a_document() {
    let face = SyntheticFace::new(240.0, 260.0, 90.0);
    let r = run(&portrait_image(face, 480, 640), ReferenceKind::Auto);
    assert_eq!(r.kind, ProcessedAs::Portrait);
    assert!(r.document.is_none());
    assert!(r.portrait.is_some());
    // Rectification was attempted and found no plausible outline.
    assert_eq!(value(&r.measurements, CheckId::DocumentDetected), Some(0.0));
}

#[test]
fn auto_falls_back_to_portrait_when_the_document_has_no_face() {
    // A blank card, with the only face beside it.
    let card = card_texture(860, ID1_ASPECT, false, false);
    let h = camera_homography(
        860.0,
        542.0,
        1000.0,
        700.0,
        &CardPose {
            width_fraction: 0.5,
            center: (0.33, 0.5),
            ..CardPose::default()
        },
    );
    let mut img = render_card(&card.image, &h, &background(1000, 700, 61, 0.0));
    let face = SyntheticFace::new(820.0, 300.0, 60.0);
    face.draw(&mut img);

    let auto = run(&img, ReferenceKind::Auto);
    assert_eq!(auto.kind, ProcessedAs::Portrait);
    assert_eq!(
        value(&auto.measurements, CheckId::DocumentDetected),
        Some(0.0)
    );
    let (dist, _) = box_agreement(&auto.portrait.unwrap().source_box, &face.bbox());
    assert!(dist < 4.0);

    // As a document, the card is rectified and has no portrait.
    let doc = run(&img, ReferenceKind::Document);
    assert_eq!(doc.kind, ProcessedAs::Document);
    assert!(doc.document.as_ref().unwrap().detected);
    assert_eq!(value(&doc.measurements, CheckId::PortraitFound), Some(0.0));
    assert!(doc.portrait.is_none() && doc.output.is_none());
    let cat = Catalogue::embedded();
    let profile = Profile::from_yaml(STANDARD, cat).unwrap();
    let a = evaluate(
        cat,
        DOCUMENT_ASSESSMENT,
        &profile.document.checks,
        &doc.measurements,
    );
    assert_eq!(a.verdict, Verdict::Rejected);
    assert_eq!(a.reasons, vec![CheckId::PortraitFound]);
    assert_eq!(
        a.status(CheckId::InterEyeDistance),
        Some(Status::Undetermined)
    );
}

#[test]
fn document_kind_without_an_outline_still_extracts_the_portrait() {
    let face = SyntheticFace::new(240.0, 260.0, 90.0);
    let r = run(&portrait_image(face, 480, 640), ReferenceKind::Document);
    assert_eq!(r.kind, ProcessedAs::Document);
    let info = r.document.unwrap();
    assert!(!info.detected && info.corners.is_none());
    assert_eq!(info.confidence, 0.0);
    assert_eq!(info.orientation_degrees, Rotation::R0);
    assert_eq!(value(&r.measurements, CheckId::DocumentDetected), Some(0.0));
    assert!(r.portrait.is_some());
}

#[test]
fn seam_failures_are_errors() {
    struct Broken;
    impl FaceFinder for Broken {
        fn find_faces(&self, _: &RgbImage) -> Result<Vec<DetectedFace>, BoxError> {
            Err("session failed".into())
        }
    }
    let img = portrait_image(SyntheticFace::new(100.0, 100.0, 40.0), 200, 240);
    let e = extract_portrait(
        &img,
        ReferenceKind::Portrait,
        &Broken,
        &ColourPortraitAnalyzer,
        None,
    )
    .unwrap_err();
    assert!(matches!(e, DocumentError::FaceFinder(_)));
    assert_eq!(e.to_string(), "face finder failed: session failed");
}

#[test]
fn flat_scan_rejects_the_portrait_frame_as_a_document() {
    // A scan cropped to the card: the card's own outline is the image border, and the best
    // outline is the portrait photo's frame, whose face is far too large for a document.
    let card = card_texture(860, ID1_ASPECT, true, true);
    let doc = run(&card.image, ReferenceKind::Document);
    assert_eq!(doc.kind, ProcessedAs::Document);
    let info = doc.document.as_ref().unwrap();
    assert!(!info.detected && info.corners.is_none(), "{info:?}");
    assert_eq!(info.confidence, 0.0);
    assert_eq!(
        value(&doc.measurements, CheckId::DocumentDetected),
        Some(0.0)
    );
    let p = doc
        .portrait
        .as_ref()
        .expect("portrait found on the scan as it is");
    let (dist, _) = box_agreement(&p.source_box, &card.face.unwrap().bbox());
    assert!(dist < 3.0);

    let auto = run(&card.image, ReferenceKind::Auto);
    assert_eq!(auto.kind, ProcessedAs::Portrait);
    assert!(auto.portrait.is_some());
}
