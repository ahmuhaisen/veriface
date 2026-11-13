//! Image handling for Veriface. Pure Rust, memory-safe decoders only.
//!
//! Module ownership (see AGENTS.md):
//! - `geom`: lead. Shared geometry types used by every crate.
//! - `codec`: service platform agent. Decode with limits and EXIF orientation, JPEG quality
//!   estimate, encode to a byte budget.
//! - `transform`, `stats`: vision agent. Resize, crop, letterbox, rotate, affine/projective warp;
//!   luminance, histograms, gradients, frequency helpers.
//!
//! The canonical in-memory image is [`RgbImage`]: 8-bit sRGB, row-major, no metadata.

pub mod codec;
pub mod geom;
pub mod stats;
pub mod transform;

pub use image::RgbImage;
