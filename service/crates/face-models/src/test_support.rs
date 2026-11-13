//! Helpers for tests that need the real ONNX Runtime library, models, test images and goldens.
//! Compiled only with the `test-support` feature.
//!
//! Every helper that needs files returns `None` (after printing a `SKIPPED:` line) when they are
//! absent, so tests can skip cleanly on machines without them.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use face_imaging::RgbImage;
use face_imaging::codec::{DecodeLimits, Format, decode};

use crate::{Runtime, RuntimeOptions, bundled_onnxruntime_path, init_onnxruntime};

/// `service/`.
pub fn service_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// The repository root.
pub fn repo_dir() -> PathBuf {
    service_dir().join("..")
}

/// `VERIFACE_MODELS`, else `<repo>/models`.
pub fn models_dir() -> PathBuf {
    std::env::var_os("VERIFACE_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_dir().join("models"))
}

/// `<repo>/testdata`.
pub fn testdata_dir() -> PathBuf {
    repo_dir().join("testdata")
}

/// `ORT_DYLIB_PATH`, else the library fetched by `service/scripts/fetch-onnxruntime.*`; `None`
/// when the file does not exist.
pub fn ort_library() -> Option<PathBuf> {
    let path = std::env::var_os("ORT_DYLIB_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| bundled_onnxruntime_path(&service_dir().join("third_party")));
    path.is_file().then_some(path)
}

/// Print a skip notice (visible with `--nocapture`, and in the captured output of failures).
pub fn skip(reason: &str) {
    println!("SKIPPED: {reason}");
    eprintln!("SKIPPED: {reason}");
}

/// A process-wide runtime with every model in the manifest, or `None` (skip) when the ONNX Runtime
/// library or the models are missing. Panics when they are present but fail to load: that is a
/// real failure, not a skip.
pub fn runtime() -> Option<&'static Runtime> {
    static RUNTIME: OnceLock<Option<Runtime>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            let Some(lib) = ort_library() else {
                skip("ONNX Runtime library not found; run service/scripts/fetch-onnxruntime.ps1 (or .sh) or set ORT_DYLIB_PATH");
                return None;
            };
            let models = models_dir();
            if !models.join("manifest.json").is_file() {
                skip(&format!("no models/manifest.json in {}", models.display()));
                return None;
            }
            init_onnxruntime(&lib, None)
                .unwrap_or_else(|e| panic!("ONNX Runtime failed to load: {e}"));
            Some(
                Runtime::load(&models, &RuntimeOptions::default())
                    .unwrap_or_else(|e| panic!("models failed to load: {e}")),
            )
        })
        .as_ref()
}

/// Decode an image file with the service's decoder (EXIF orientation applied).
pub fn load_rgb(path: &Path) -> RgbImage {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let limits = DecodeLimits::new(64 << 20, 100_000_000, &Format::ALL);
    decode(&bytes, &limits)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .image
}

/// A class-id mask PNG (grayscale, or RGB with the id in every channel): width, height, ids.
pub fn load_mask(path: &Path) -> (u32, u32, Vec<u8>) {
    let img = image::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let (w, h) = (img.width(), img.height());
    let ids = match img {
        image::DynamicImage::ImageLuma8(g) => g.into_raw(),
        other => other.to_rgb8().pixels().map(|p| p[0]).collect(),
    };
    (w, h, ids)
}

/// Read a little-endian `float32` or `float16` `.npy` file (C order): shape and values.
pub fn read_npy(path: &Path) -> Result<(Vec<usize>, Vec<f32>), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        return Err(format!("{}: not an .npy file", path.display()));
    }
    let (header_len, start) = match bytes[6] {
        1 => (usize::from(u16::from_le_bytes([bytes[8], bytes[9]])), 10),
        2 | 3 => (
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            12,
        ),
        v => return Err(format!("{}: npy version {v}", path.display())),
    };
    let header = std::str::from_utf8(&bytes[start..start + header_len])
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if header.contains("'fortran_order': True") {
        return Err(format!(
            "{}: Fortran order is not supported",
            path.display()
        ));
    }
    let descr = header
        .split("'descr':")
        .nth(1)
        .and_then(|s| s.split('\'').nth(1))
        .ok_or_else(|| format!("{}: no descr", path.display()))?;
    let shape_text = header
        .split("'shape':")
        .nth(1)
        .and_then(|s| s.split('(').nth(1))
        .and_then(|s| s.split(')').next())
        .ok_or_else(|| format!("{}: no shape", path.display()))?;
    let shape: Vec<usize> = shape_text
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().map_err(|e| format!("shape: {e}")))
        .collect::<Result<_, _>>()?;
    let data = &bytes[start + header_len..];
    let values = match descr {
        "<f4" => data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect(),
        "<f2" => data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| f16_to_f32(u16::from_le_bytes(*b)))
            .collect::<Vec<f32>>(),
        other => return Err(format!("{}: dtype {other} not supported", path.display())),
    };
    let n: usize = shape.iter().product();
    if values.len() != n {
        return Err(format!(
            "{}: {} values for shape {shape:?}",
            path.display(),
            values.len()
        ));
    }
    Ok((shape, values))
}

/// IEEE 754 half to single precision.
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = u32::from(h >> 15) << 31;
    let exp = u32::from((h >> 10) & 0x1f);
    let frac = u32::from(h & 0x3ff);
    let bits = match (exp, frac) {
        (0, 0) => sign,
        (0, f) => {
            // Subnormal: normalise.
            let mut e = 127 - 15 + 1;
            let mut f = f;
            while f & 0x400 == 0 {
                f <<= 1;
                e -= 1;
            }
            sign | ((e as u32) << 23) | ((f & 0x3ff) << 13)
        }
        (0x1f, f) => sign | 0x7f80_0000 | (f << 13),
        (e, f) => sign | ((e + 127 - 15) << 23) | (f << 13),
    };
    f32::from_bits(bits)
}

/// Largest `|a - b| - (atol + rtol |b|)` over all values (<= 0 means `allclose`).
pub fn allclose_excess(actual: &[f32], expected: &[f32], atol: f32, rtol: f32) -> f32 {
    assert_eq!(actual.len(), expected.len(), "length mismatch");
    actual
        .iter()
        .zip(expected)
        .map(|(a, b)| (a - b).abs() - (atol + rtol * b.abs()))
        .fold(f32::MIN, f32::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats_convert() {
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0xc000), -2.0);
        assert_eq!(f16_to_f32(0x7bff), 65504.0);
        assert!((f16_to_f32(0x0001) - 5.960_464_5e-8).abs() < 1e-12);
        assert!(f16_to_f32(0x7e00).is_nan());
    }
}
