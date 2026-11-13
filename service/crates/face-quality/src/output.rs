//! Output crops (docs/architecture.md section 3.1 step 9, profile `output.style`).
//!
//! - `portrait-3x4`: the eye midpoint at (0.50 W, 0.42 H), chin-to-crown height about 0.72 H. If
//!   that rectangle leaves the image it is shifted, and shrunk when it is larger than the image;
//!   it is never padded. The crop is resampled to exactly the requested size.
//! - `face-crop`: the head box plus a 10 % margin, widened to the requested aspect ratio, shifted
//!   and shrunk to fit; never padded and never upscaled (the result is at most the requested
//!   size).

use face_core::OutputStyle;
use face_imaging::RgbImage;
use face_imaging::geom::Rect;
use face_imaging::transform::{self, Filter};

use crate::geometry::HeadGeometry;

/// Eye midpoint position in a `portrait-3x4` crop (fraction of width).
pub const PORTRAIT_EYE_X: f32 = 0.50;
/// Eye midpoint position in a `portrait-3x4` crop (fraction of height).
pub const PORTRAIT_EYE_Y: f32 = 0.42;
/// Chin-to-crown height in a `portrait-3x4` crop (fraction of height).
pub const PORTRAIT_HEAD_HEIGHT: f32 = 0.72;
/// `face-crop` margin around the head box, as a fraction of the head height.
pub const FACE_CROP_MARGIN: f32 = 0.10;

/// Fit a `w` x `h` rectangle whose anchor point should be at `anchor` (image coordinates) and
/// sits at fractions `(fx, fy)` of the rectangle: shrink (aspect kept) to fit the image, then
/// shift inside it.
fn fit(anchor: (f32, f32), (fx, fy): (f32, f32), w: f32, h: f32, iw: f32, ih: f32) -> Rect {
    let s = (iw / w).min(ih / h).min(1.0);
    let (w, h) = (w * s, h * s);
    let x = (anchor.0 - fx * w).clamp(0.0, (iw - w).max(0.0));
    let y = (anchor.1 - fy * h).clamp(0.0, (ih - h).max(0.0));
    Rect::new(x, y, w, h)
}

/// The crop rectangle (image coordinates) for `style` and an output of `width` x `height`.
pub fn output_rect(
    head: &HeadGeometry,
    style: OutputStyle,
    width: u32,
    height: u32,
    image_width: u32,
    image_height: u32,
) -> Option<Rect> {
    if width == 0 || height == 0 || image_width == 0 || image_height == 0 {
        return None;
    }
    let aspect = width as f32 / height as f32;
    let (iw, ih) = (image_width as f32, image_height as f32);
    if !(head.height.is_finite() && head.height > 2.0) {
        return None;
    }
    let rect = match style {
        OutputStyle::Portrait3x4 => {
            let h = head.height / PORTRAIT_HEAD_HEIGHT;
            fit(
                (head.eye_mid.x, head.eye_mid.y),
                (PORTRAIT_EYE_X, PORTRAIT_EYE_Y),
                h * aspect,
                h,
                iw,
                ih,
            )
        }
        OutputStyle::FaceCrop => {
            let b = head.head_box;
            let m = FACE_CROP_MARGIN * head.height;
            let (mut w, mut h) = (b.width + 2.0 * m, b.height + 2.0 * m);
            if w / h < aspect {
                w = h * aspect;
            } else {
                h = w / aspect;
            }
            let c = b.center();
            fit((c.x, c.y), (0.5, 0.5), w, h, iw, ih)
        }
    };
    (rect.width >= 1.0 && rect.height >= 1.0).then_some(rect)
}

/// Render the output image for `style`. `portrait-3x4` is exactly `width` x `height`;
/// `face-crop` is at most that size (never upscaled). `None` when the head geometry is unusable.
pub fn render_output(
    image: &RgbImage,
    head: &HeadGeometry,
    style: OutputStyle,
    width: u32,
    height: u32,
) -> Option<RgbImage> {
    let rect = output_rect(head, style, width, height, image.width(), image.height())?;
    let crop = transform::crop(image, &rect)?.image;
    let (cw, ch) = crop.dimensions();
    let (tw, th) = match style {
        OutputStyle::Portrait3x4 => (width, height),
        OutputStyle::FaceCrop => {
            let s = (width as f32 / cw as f32)
                .min(height as f32 / ch as f32)
                .min(1.0);
            (
                ((cw as f32 * s).round() as u32).clamp(1, width),
                ((ch as f32 * s).round() as u32).clamp(1, height),
            )
        }
    };
    if (tw, th) == (cw, ch) {
        return Some(crop);
    }
    let filter = if tw < cw || th < ch {
        Filter::Area
    } else {
        Filter::Bilinear
    };
    Some(transform::resize(&crop, tw, th, filter))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{CrownSource, FaceFrame};
    use face_imaging::geom::Point;
    use face_imaging::transform::RotatedRect;

    fn head(eye: Point, height: f32, width: f32) -> HeadGeometry {
        let frame = FaceFrame {
            origin: eye,
            u: Point::new(1.0, 0.0),
            v: Point::new(0.0, 1.0),
            iod: width / 3.0,
        };
        let center = Point::new(eye.x, eye.y);
        let rect = RotatedRect {
            center,
            width,
            height,
            rotation: 0.0,
        };
        HeadGeometry {
            frame,
            eye_mid: eye,
            chin: Point::new(eye.x, eye.y + height / 2.0),
            crown: Point::new(eye.x, eye.y - height / 2.0),
            height,
            width,
            head: rect,
            head_box: rect.bounding_box(),
            crown_source: CrownSource::Anthropometric,
        }
    }

    #[test]
    fn portrait_3x4_places_the_eyes_and_scales_the_head() {
        let h = head(Point::new(500.0, 500.0), 288.0, 200.0);
        let r = output_rect(&h, OutputStyle::Portrait3x4, 480, 640, 1000, 1000).unwrap();
        // Crop height 288 / 0.72 = 400, width 300; eyes at (0.5, 0.42) of it.
        assert!((r.height - 400.0).abs() < 1e-3 && (r.width - 300.0).abs() < 1e-3);
        assert!((r.x - 350.0).abs() < 1e-3 && (r.y - 332.0).abs() < 1e-3);
        let img = RgbImage::new(1000, 1000);
        assert_eq!(
            render_output(&img, &h, OutputStyle::Portrait3x4, 480, 640)
                .unwrap()
                .dimensions(),
            (480, 640)
        );
    }

    #[test]
    fn portrait_3x4_shifts_and_shrinks_instead_of_padding() {
        // Face near the top-left corner: shifted inside.
        let h = head(Point::new(60.0, 50.0), 288.0, 200.0);
        let r = output_rect(&h, OutputStyle::Portrait3x4, 480, 640, 1000, 1000).unwrap();
        assert_eq!((r.x, r.y), (0.0, 0.0));
        assert!((r.height - 400.0).abs() < 1e-3);
        // Image smaller than the ideal crop: shrunk to fit, aspect kept.
        let r = output_rect(&h, OutputStyle::Portrait3x4, 480, 640, 240, 300).unwrap();
        assert!(r.width <= 240.0 + 1e-3 && r.height <= 300.0 + 1e-3);
        assert!((r.width / r.height - 0.75).abs() < 1e-4);
        assert!(
            r.x >= 0.0 && r.y >= 0.0 && r.right() <= 240.0 + 1e-3 && r.bottom() <= 300.0 + 1e-3
        );
    }

    #[test]
    fn face_crop_never_upscales() {
        let h = head(Point::new(100.0, 100.0), 80.0, 60.0);
        let img = RgbImage::new(200, 200);
        let out = render_output(&img, &h, OutputStyle::FaceCrop, 360, 480).unwrap();
        assert!(out.width() <= 360 && out.height() <= 480);
        // Head box 60x80 + 8 px margins = 76x96, heightened to 3:4 = 76x101, kept at native size.
        assert!(
            (out.width() as i32 - 76).abs() <= 1 && (out.height() as i32 - 101).abs() <= 1,
            "{:?}",
            out.dimensions()
        );
        // A large face is downscaled to fit the requested size.
        let big = head(Point::new(1000.0, 1000.0), 1200.0, 900.0);
        let img = RgbImage::new(2000, 2000);
        let out = render_output(&img, &big, OutputStyle::FaceCrop, 360, 480).unwrap();
        assert!(out.width() <= 360 && out.height() <= 480 && out.height() >= 470);
    }
}
