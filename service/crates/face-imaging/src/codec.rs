//! Decode with limits, EXIF orientation, JPEG quality estimate, encode to a byte budget.
//! Owner: service platform agent.
//!
//! [`decode`] is the only entry point for untrusted bytes. It checks, in this order:
//!
//! 1. the byte size;
//! 2. the format, sniffed from the magic bytes (never from a declared Content-Type), against the
//!    caller's allow-list;
//! 3. the pixel count, read from the file header before any pixel data is decoded;
//! 4. the `image` crate's allocation limit, during the full decode.
//!
//! It then applies the EXIF orientation, composites alpha over white, converts to [`RgbImage`]
//! and drops every metadata block (EXIF, ICC, XMP, text chunks).
//!
//! Each [`DecodeError`] maps to exactly one API problem code:
//!
//! | Error | HTTP | Problem code |
//! |---|---|---|
//! | [`DecodeError::TooManyBytes`] | 413 | `payload-too-large` |
//! | [`DecodeError::UnsupportedFormat`] | 415 | `unsupported-media-type` |
//! | [`DecodeError::TooManyPixels`] | 422 | `image-too-large` |
//! | [`DecodeError::Corrupt`] | 422 | `image-decode-failed` |

use std::fmt;
use std::io::Cursor;

use image::codecs::jpeg::{JpegDecoder, JpegEncoder};
use image::codecs::png::{PngDecoder, PngEncoder};
use image::codecs::webp::WebPDecoder;
use image::metadata::Orientation;
use image::{
    DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageError, Limits, RgbImage,
};

/// An accepted image container format. Detected with [`sniff`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    Jpeg,
    Png,
    Webp,
}

impl Format {
    /// Every supported format.
    pub const ALL: [Format; 3] = [Format::Jpeg, Format::Png, Format::Webp];

    /// The name used in profiles (`input.formats`): `jpeg`, `png` or `webp`.
    pub const fn name(self) -> &'static str {
        match self {
            Format::Jpeg => "jpeg",
            Format::Png => "png",
            Format::Webp => "webp",
        }
    }

    /// The IANA media type.
    pub const fn media_type(self) -> &'static str {
        match self {
            Format::Jpeg => "image/jpeg",
            Format::Png => "image/png",
            Format::Webp => "image/webp",
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Detect the container format from its magic bytes.
pub fn sniff(bytes: &[u8]) -> Option<Format> {
    const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Format::Jpeg)
    } else if bytes.starts_with(PNG_SIGNATURE) {
        Some(Format::Png)
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(Format::Webp)
    } else {
        None
    }
}

/// Limits applied by [`decode`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeLimits {
    /// Largest accepted file, in bytes.
    pub max_bytes: usize,
    /// Largest accepted `width x height`, checked against the header before pixels are decoded.
    pub max_pixels: u64,
    /// Accepted formats. Anything else is [`DecodeError::UnsupportedFormat`].
    pub formats: Vec<Format>,
    /// Allocation limit handed to the `image` crate's decoders (`image::Limits::max_alloc`).
    pub max_alloc: u64,
}

impl DecodeLimits {
    /// The `image` crate's own default allocation limit (512 MiB).
    pub const DEFAULT_MAX_ALLOC: u64 = 512 * 1024 * 1024;

    pub fn new(max_bytes: usize, max_pixels: u64, formats: &[Format]) -> Self {
        DecodeLimits {
            max_bytes,
            max_pixels,
            formats: formats.to_vec(),
            max_alloc: Self::DEFAULT_MAX_ALLOC,
        }
    }
}

/// A decoded image: upright (EXIF orientation applied), RGB8, alpha composited over white, no
/// metadata.
#[derive(Clone)]
pub struct Decoded {
    pub image: RgbImage,
    /// The detected container format.
    pub format: Format,
    /// Input for `COMPRESSION_QUALITY` (0-100): estimated from the quantisation tables for JPEG
    /// and from the VP8 quantisers for lossy WebP; 100 for PNG and lossless WebP; `None` when it
    /// could not be estimated.
    pub jpeg_quality: Option<f32>,
}

// Hand-written so that pixels can never end up in a log line through `{:?}`.
impl fmt::Debug for Decoded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Decoded")
            .field("width", &self.image.width())
            .field("height", &self.image.height())
            .field("format", &self.format)
            .field("jpeg_quality", &self.jpeg_quality)
            .finish_non_exhaustive()
    }
}

/// Why [`decode`] rejected an input. See the module documentation for the API mapping.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    /// The file is larger than [`DecodeLimits::max_bytes`].
    #[error("the image is {size} bytes; the limit is {limit} bytes")]
    TooManyBytes { size: usize, limit: usize },
    /// Not JPEG, PNG or WebP; a format the caller does not accept; or a variant of a format the
    /// decoder does not support (e.g. arithmetic-coded JPEG).
    #[error("unsupported image format: {reason}")]
    UnsupportedFormat {
        detected: Option<Format>,
        reason: String,
    },
    /// The header declares more than [`DecodeLimits::max_pixels`] pixels, or decoding would exceed
    /// the allocation limit.
    #[error(
        "the image is {width}x{height} pixels, which exceeds the decode limits (at most {limit} pixels)"
    )]
    TooManyPixels { width: u32, height: u32, limit: u64 },
    /// The file is damaged or not a valid image.
    #[error("the image could not be decoded: {reason}")]
    Corrupt { reason: String },
}

/// Decode untrusted image bytes. See the module documentation for the order of the checks.
pub fn decode(bytes: &[u8], limits: &DecodeLimits) -> Result<Decoded, DecodeError> {
    if bytes.len() > limits.max_bytes {
        return Err(DecodeError::TooManyBytes {
            size: bytes.len(),
            limit: limits.max_bytes,
        });
    }

    let format = match sniff(bytes) {
        Some(format) if limits.formats.contains(&format) => format,
        Some(format) => {
            return Err(DecodeError::UnsupportedFormat {
                detected: Some(format),
                reason: format!("{format} is not accepted here"),
            });
        }
        None => {
            return Err(DecodeError::UnsupportedFormat {
                detected: None,
                reason: "not a JPEG, PNG or WebP file".to_owned(),
            });
        }
    };

    // Our own header parse, so an oversized image is rejected before any decoder exists.
    let jpeg = (format == Format::Jpeg).then(|| parse_jpeg_header(bytes));
    let declared = match format {
        Format::Jpeg => jpeg.as_ref().and_then(|h| h.dimensions),
        Format::Png => png_dimensions(bytes),
        Format::Webp => webp_dimensions(bytes),
    };
    if let Some((width, height)) = declared {
        check_pixels(width, height, limits)?;
    }

    let mut image_limits = Limits::default();
    image_limits.max_alloc = Some(limits.max_alloc);
    let cursor = Cursor::new(bytes);
    let opened = |e: ImageError| from_image_error(e, format, declared, limits);
    let (image, quality) = match format {
        Format::Jpeg => {
            let decoder = JpegDecoder::new(cursor).map_err(opened)?;
            let quality = jpeg.as_ref().and_then(JpegHeader::estimate_quality);
            (
                decode_pixels(decoder, format, limits, image_limits)?,
                quality,
            )
        }
        Format::Png => {
            let decoder = PngDecoder::with_limits(cursor, image_limits.clone()).map_err(opened)?;
            (
                decode_pixels(decoder, format, limits, image_limits)?,
                Some(100.0),
            )
        }
        Format::Webp => {
            let decoder = WebPDecoder::new(cursor).map_err(opened)?;
            let quality = webp_quality(bytes);
            (
                decode_pixels(decoder, format, limits, image_limits)?,
                quality,
            )
        }
    };

    Ok(Decoded {
        image,
        format,
        jpeg_quality: quality,
    })
}

fn check_pixels(width: u32, height: u32, limits: &DecodeLimits) -> Result<(), DecodeError> {
    if width == 0 || height == 0 {
        return Err(DecodeError::Corrupt {
            reason: format!("the header declares a {width}x{height} image"),
        });
    }
    if u64::from(width) * u64::from(height) > limits.max_pixels {
        return Err(DecodeError::TooManyPixels {
            width,
            height,
            limit: limits.max_pixels,
        });
    }
    Ok(())
}

fn decode_pixels(
    mut decoder: impl ImageDecoder,
    format: Format,
    limits: &DecodeLimits,
    image_limits: Limits,
) -> Result<RgbImage, DecodeError> {
    let (width, height) = decoder.dimensions();
    // Repeated with the decoder's own reading of the header (defence in depth).
    check_pixels(width, height, limits)?;
    // The decoded buffer is the largest allocation. Not every decoder charges it against
    // `max_alloc`, so check it here.
    if decoder.total_bytes() > limits.max_alloc {
        return Err(DecodeError::TooManyPixels {
            width,
            height,
            limit: limits.max_pixels,
        });
    }
    let map = |e: ImageError| from_image_error(e, format, Some((width, height)), limits);
    decoder.set_limits(image_limits).map_err(map)?;
    // A malformed EXIF block must not fail an otherwise valid image: keep it as it is.
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).map_err(map)?;
    image.apply_orientation(orientation);
    Ok(flatten_to_rgb(image))
}

fn from_image_error(
    e: ImageError,
    format: Format,
    dimensions: Option<(u32, u32)>,
    limits: &DecodeLimits,
) -> DecodeError {
    match (e, dimensions) {
        (ImageError::Limits(_), Some((width, height))) => DecodeError::TooManyPixels {
            width,
            height,
            limit: limits.max_pixels,
        },
        (ImageError::Unsupported(u), _) => DecodeError::UnsupportedFormat {
            detected: Some(format),
            reason: u.to_string(),
        },
        (other, _) => DecodeError::Corrupt {
            reason: other.to_string(),
        },
    }
}

/// RGB8 with alpha composited over white.
fn flatten_to_rgb(image: DynamicImage) -> RgbImage {
    if !image.color().has_alpha() {
        return image.into_rgb8();
    }
    let rgba = image.into_rgba8();
    let mut out = RgbImage::new(rgba.width(), rgba.height());
    for (dst, src) in out.pixels_mut().zip(rgba.pixels()) {
        let [r, g, b, a] = src.0;
        dst.0 = [over_white(r, a), over_white(g, a), over_white(b, a)];
    }
    out
}

fn over_white(channel: u8, alpha: u8) -> u8 {
    let (c, a) = (u32::from(channel), u32::from(alpha));
    // At most (255 * 255 + 127) / 255 = 255.
    ((c * a + 255 * (255 - a) + 127) / 255) as u8
}

// ------------------------------------------------------------------------------------ headers

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    // The IHDR chunk must come first, right after the 8-byte signature.
    if bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    Some((be_u32(bytes.get(16..20)?), be_u32(bytes.get(20..24)?)))
}

fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let (fourcc, payload) = riff_chunks(bytes.get(12..)?).next()?;
    match &fourcc {
        // Extended format: 24-bit canvas width-1 and height-1 at offset 4.
        b"VP8X" => Some((
            le_u24(payload.get(4..7)?) + 1,
            le_u24(payload.get(7..10)?) + 1,
        )),
        // Lossy key frame: 14-bit width and height after the start code.
        b"VP8 " => {
            let width = u32::from(u16::from_le_bytes([*payload.get(6)?, *payload.get(7)?]));
            let height = u32::from(u16::from_le_bytes([*payload.get(8)?, *payload.get(9)?]));
            Some((width & 0x3FFF, height & 0x3FFF))
        }
        // Lossless: signature 0x2F, then 14-bit width-1 and height-1.
        b"VP8L" if payload.first() == Some(&0x2F) => {
            let bits = u32::from_le_bytes(payload.get(1..5)?.try_into().ok()?);
            Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
        }
        _ => None,
    }
}

fn be_u32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn le_u24(b: &[u8]) -> u32 {
    u32::from(b[0]) | (u32::from(b[1]) << 8) | (u32::from(b[2]) << 16)
}

/// RIFF chunks as `(fourcc, payload)`. A truncated last chunk yields what is present.
fn riff_chunks(mut data: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    std::iter::from_fn(move || {
        let fourcc: [u8; 4] = data.get(0..4)?.try_into().ok()?;
        let size = u32::from_le_bytes(data.get(4..8)?.try_into().ok()?) as usize;
        let end = size.saturating_add(8).min(data.len());
        let payload = &data[8..end];
        let next = size.saturating_add(8).saturating_add(size & 1);
        data = data.get(next..).unwrap_or(&[]);
        Some((fourcc, payload))
    })
}

// ------------------------------------------------------------------------ JPEG quality estimate

/// Zigzag position -> natural (row-major) position of the 8x8 block.
const UNZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// ITU-T T.81 Annex K, table K.1 (luminance), natural order. The IJG reference tables.
const STD_LUMINANCE: [u16; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113,
    92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];

/// ITU-T T.81 Annex K, table K.2 (chrominance), natural order.
const STD_CHROMINANCE: [u16; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99,
    47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

#[derive(Clone, Copy, Debug)]
struct QuantTable {
    /// Natural order.
    values: [u16; 64],
    sixteen_bit: bool,
}

#[derive(Debug, Default)]
struct JpegHeader {
    /// Quantisation tables by id, as defined before the first scan.
    tables: [Option<QuantTable>; 4],
    /// Quantisation table id of each frame component, in frame order.
    component_tables: Vec<u8>,
    /// `(width, height)` from the frame header, when it declares both.
    dimensions: Option<(u32, u32)>,
}

impl JpegHeader {
    /// The IJG quality (1-100) whose scaled standard tables are closest (least squares) to this
    /// file's luminance and chrominance tables. Exact for libjpeg/IJG-style encoders; an estimate
    /// for encoders with custom tables.
    fn estimate_quality(&self) -> Option<f32> {
        let luma_id = usize::from(*self.component_tables.first().unwrap_or(&0));
        let luma = self.tables.get(luma_id)?.as_ref()?;
        let chroma_id = if self.component_tables.is_empty() {
            Some(1) // no frame header seen: assume the usual layout
        } else {
            self.component_tables.get(1).map(|&id| usize::from(id))
        };
        let chroma = chroma_id
            .filter(|&id| id != luma_id)
            .and_then(|id| self.tables.get(id)?.as_ref());
        let mut best = (u64::MAX, 0u8);
        for quality in 1..=100u8 {
            let scale = ijg_scale(quality);
            let mut error = table_error(luma, &STD_LUMINANCE, scale);
            if let Some(chroma) = chroma {
                error += table_error(chroma, &STD_CHROMINANCE, scale);
            }
            if error < best.0 {
                best = (error, quality);
            }
        }
        Some(f32::from(best.1))
    }
}

/// libjpeg's `jpeg_quality_scaling`.
fn ijg_scale(quality: u8) -> u32 {
    let q = u32::from(quality.clamp(1, 100));
    if q < 50 { 5000 / q } else { 200 - 2 * q }
}

fn table_error(actual: &QuantTable, standard: &[u16; 64], scale: u32) -> u64 {
    let max = if actual.sixteen_bit { 32767 } else { 255 };
    actual
        .values
        .iter()
        .zip(standard)
        .map(|(&a, &s)| {
            let expected = ((u32::from(s) * scale + 50) / 100).clamp(1, max);
            let d = i64::from(a) - i64::from(expected);
            d.unsigned_abs().pow(2)
        })
        .sum()
}

/// Walk the JPEG markers up to the first scan, collecting quantisation tables and the frame
/// header. Stops quietly at anything malformed; the decoder reports real errors.
fn parse_jpeg_header(bytes: &[u8]) -> JpegHeader {
    let mut header = JpegHeader::default();
    let mut i = 2; // after SOI
    loop {
        if bytes.get(i) != Some(&0xFF) {
            return header;
        }
        while bytes.get(i) == Some(&0xFF) {
            i += 1; // marker prefix and fill bytes
        }
        let Some(&marker) = bytes.get(i) else {
            return header;
        };
        i += 1;
        match marker {
            0x01 | 0xD0..=0xD8 => continue, // standalone markers (TEM, RSTn, SOI)
            0xD9 | 0xDA => return header,   // EOI, SOS
            _ => {}
        }
        let Some(&[hi, lo]) = bytes.get(i..i + 2) else {
            return header;
        };
        // The length counts its own two bytes.
        let len = usize::from(u16::from_be_bytes([hi, lo]));
        let Some(segment) = bytes.get(i + 2..i + len).filter(|_| len >= 2) else {
            return header;
        };
        i += len;
        match marker {
            0xDB => parse_dqt(segment, &mut header.tables),
            // SOFn, excluding DHT (C4), JPG (C8) and DAC (CC).
            0xC0..=0xCF
                if !matches!(marker, 0xC4 | 0xC8 | 0xCC) && header.component_tables.is_empty() =>
            {
                parse_sof(segment, &mut header);
            }
            _ => {}
        }
    }
}

fn parse_dqt(segment: &[u8], tables: &mut [Option<QuantTable>; 4]) {
    let mut rest = segment;
    while let Some((&pq_tq, tail)) = rest.split_first() {
        let (precision, id) = (pq_tq >> 4, usize::from(pq_tq & 0x0F));
        let size = if precision == 0 { 64 } else { 128 };
        let (Some(raw), true) = (tail.get(..size), precision <= 1 && id < 4) else {
            return;
        };
        let mut values = [0u16; 64];
        if precision == 0 {
            for (k, v) in raw.iter().enumerate() {
                values[UNZIGZAG[k]] = u16::from(*v);
            }
        } else {
            for (k, pair) in raw.as_chunks::<2>().0.iter().enumerate() {
                values[UNZIGZAG[k]] = u16::from_be_bytes(*pair);
            }
        }
        tables[id] = Some(QuantTable {
            values,
            sixteen_bit: precision == 1,
        });
        rest = &tail[size..];
    }
}

fn parse_sof(segment: &[u8], header: &mut JpegHeader) {
    // P(1) Y(2) X(2) Nf(1), then Nf x (Ci, HiVi, Tqi).
    let Some(&[_, y0, y1, x0, x1, count]) = segment.get(..6) else {
        return;
    };
    let (height, width) = (
        u32::from(u16::from_be_bytes([y0, y1])),
        u32::from(u16::from_be_bytes([x0, x1])),
    );
    if width > 0 && height > 0 {
        header.dimensions = Some((width, height));
    }
    header.component_tables = segment[6..]
        .as_chunks::<3>()
        .0
        .iter()
        .take(usize::from(count))
        .map(|&[_, _, table]| table & 0x0F)
        .collect();
}

// -------------------------------------------------------------------- WebP quality estimate

/// `COMPRESSION_QUALITY` input for WebP: 100 for lossless, an estimate for lossy.
fn webp_quality(bytes: &[u8]) -> Option<f32> {
    match first_webp_bitstream(bytes)? {
        WebpBitstream::Lossless => Some(100.0),
        WebpBitstream::Lossy(vp8) => vp8_segment_quantizers(vp8).map(|q| webp_lossy_quality(&q)),
    }
}

enum WebpBitstream<'a> {
    Lossy(&'a [u8]),
    Lossless,
}

/// The first image bitstream: top level for simple and extended files, the first frame for
/// animations.
fn first_webp_bitstream(bytes: &[u8]) -> Option<WebpBitstream<'_>> {
    let pick = |fourcc: &[u8; 4], payload| match fourcc {
        b"VP8 " => Some(WebpBitstream::Lossy(payload)),
        b"VP8L" => Some(WebpBitstream::Lossless),
        _ => None,
    };
    for (fourcc, payload) in riff_chunks(bytes.get(12..)?) {
        if let Some(found) = pick(&fourcc, payload) {
            return Some(found);
        }
        if &fourcc == b"ANMF" {
            // 16-byte frame header, then the frame's own chunks.
            return riff_chunks(payload.get(16..)?).find_map(|(f, p)| pick(&f, p));
        }
    }
    None
}

/// The VP8 boolean entropy decoder (RFC 6386, section 7). Reads past the end as zeros.
struct BoolDecoder<'a> {
    data: &'a [u8],
    pos: usize,
    value: u32,
    range: u32,
    bit_count: u32,
}

impl<'a> BoolDecoder<'a> {
    fn new(data: &'a [u8]) -> Self {
        let mut d = BoolDecoder {
            data,
            pos: 0,
            value: 0,
            range: 255,
            bit_count: 0,
        };
        d.value = (u32::from(d.next_byte()) << 8) | u32::from(d.next_byte());
        d
    }

    fn next_byte(&mut self) -> u8 {
        let byte = self.data.get(self.pos).copied().unwrap_or(0);
        self.pos = self.pos.saturating_add(1);
        byte
    }

    fn read_bool(&mut self, probability: u32) -> bool {
        let split = 1 + (((self.range - 1) * probability) >> 8);
        let big_split = split << 8;
        let bit = if self.value >= big_split {
            self.range -= split;
            self.value -= big_split;
            true
        } else {
            self.range = split;
            false
        };
        while self.range < 128 {
            self.value <<= 1;
            self.range <<= 1;
            self.bit_count += 1;
            if self.bit_count == 8 {
                self.bit_count = 0;
                self.value |= u32::from(self.next_byte());
            }
        }
        bit
    }

    fn flag(&mut self) -> bool {
        self.read_bool(128)
    }

    fn literal(&mut self, bits: u32) -> u32 {
        (0..bits).fold(0, |v, _| (v << 1) | u32::from(self.flag()))
    }

    fn optional_signed(&mut self, bits: u32) -> i32 {
        if !self.flag() {
            return 0;
        }
        let magnitude = self.literal(bits) as i32;
        if self.flag() { -magnitude } else { magnitude }
    }
}

/// The luma AC quantiser index (0-127) of every segment of a lossy VP8 key frame, read from the
/// frame header (RFC 6386, sections 9.2-9.6).
fn vp8_segment_quantizers(vp8: &[u8]) -> Option<Vec<u8>> {
    let tag = le_u24(vp8.get(0..3)?);
    let key_frame = tag & 1 == 0;
    if !key_frame || vp8.get(3..6)? != [0x9D, 0x01, 0x2A] {
        return None;
    }
    let first_partition = (tag >> 5) as usize;
    let end = first_partition.saturating_add(10).min(vp8.len());
    let mut b = BoolDecoder::new(vp8.get(10..end)?);

    let _color_space = b.literal(1);
    let _clamping_type = b.literal(1);
    // (quantiser per segment, absolute values?)
    let mut segments: Option<([i32; 4], bool)> = None;
    if b.flag() {
        let update_map = b.flag();
        if b.flag() {
            let absolute = b.flag();
            let mut quantizers = [0i32; 4];
            for q in &mut quantizers {
                *q = b.optional_signed(7);
            }
            for _ in 0..4 {
                b.optional_signed(6); // loop filter level per segment
            }
            segments = Some((quantizers, absolute));
        }
        if update_map {
            for _ in 0..3 {
                if b.flag() {
                    b.literal(8); // segment tree probability
                }
            }
        }
    }
    let _filter_type = b.flag();
    let _filter_level = b.literal(6);
    let _sharpness = b.literal(3);
    if b.flag() && b.flag() {
        for _ in 0..8 {
            b.optional_signed(6); // reference frame and mode loop filter deltas
        }
    }
    let _partitions = b.literal(2);
    let y_ac_qi = b.literal(7) as i32;

    let quantizers: Vec<i32> = match segments {
        None => vec![y_ac_qi],
        Some((q, true)) => q.to_vec(),
        Some((q, false)) => q.iter().map(|delta| y_ac_qi + delta).collect(),
    };
    Some(
        quantizers
            .into_iter()
            .map(|q| q.clamp(0, 127) as u8)
            .collect(),
    )
}

/// libwebp's default spatial noise shaping amplitude per unit of segment "alpha":
/// `SNS_TO_DQ (0.9) * sns_strength (50) / 100 / 128`.
const SNS_AMPLITUDE: f64 = 0.9 * 50.0 / 100.0 / 128.0;
/// libwebp clips segment alphas to [-127, 127].
const ALPHA_CLIP: f64 = 127.0;

/// Best estimate of the libwebp `quality` (0-100) that produced these segment quantisers.
///
/// libwebp maps quality to a base compression `c` (`QualityToCompression`), then gives segment
/// `i` the quantiser `127 * (1 - c^(1 - SNS_AMPLITUDE * alpha_i))`, where the segment alphas are
/// linear in the k-means centres and clipped to +-127. At least one extreme segment is therefore
/// clipped, which leaves two candidate solutions for `c`; we keep the consistent one and break
/// ties by which end of the quantiser range is compressed (clipping compresses it).
///
/// Assumes libwebp's default `sns_strength` (50), which browsers and image libraries use.
/// Measured on 468 libwebp 1.6 encodes of natural images (quality 20-95): max error 2.0 points.
/// Synthetic images with very few texture levels (pure gradients) can be off by 15 or more.
fn webp_lossy_quality(quantizers: &[u8]) -> f32 {
    let mut q = quantizers.to_vec();
    q.sort_unstable();
    let n = q.len();
    // ln(1 - q/127), taking the middle of the integer bucket the encoder truncated into.
    let log_c = |qi: u8| {
        if qi == 0 {
            0.0
        } else {
            (1.0 - (f64::from(qi) + 0.5).min(127.0) / 127.0)
                .max(1e-6)
                .ln()
        }
    };
    let (top, bottom) = (log_c(q[0]), log_c(q[n - 1])); // lowest and highest quantiser
    let log_base = if n == 1 || q[0] == q[n - 1] {
        top
    } else if top == 0.0 {
        bottom / (1.0 + ALPHA_CLIP * SNS_AMPLITUDE)
    } else {
        // Highest alpha (lowest quantiser) clipped at +127.
        let top_clipped = top / (1.0 - ALPHA_CLIP * SNS_AMPLITUDE);
        let alpha_bottom = (1.0 - bottom / top_clipped) / SNS_AMPLITUDE;
        let top_ok = (-(ALPHA_CLIP + 1.5)..=0.5).contains(&alpha_bottom);
        // Lowest alpha (highest quantiser) clipped at -127.
        let bottom_clipped = bottom / (1.0 + ALPHA_CLIP * SNS_AMPLITUDE);
        let alpha_top = (1.0 - top / bottom_clipped) / SNS_AMPLITUDE;
        let bottom_ok = (-0.5..=ALPHA_CLIP + 1.5).contains(&alpha_top);
        let average = (top_clipped + bottom_clipped) / 2.0;
        match (top_ok, bottom_ok) {
            (true, false) => top_clipped,
            (false, true) => bottom_clipped,
            (false, false) => average,
            (true, true) => {
                let gap_top = log_c(q[0]) - log_c(q[1]);
                let gap_bottom = log_c(q[n - 2]) - log_c(q[n - 1]);
                if gap_top < gap_bottom {
                    top_clipped
                } else if gap_bottom < gap_top {
                    bottom_clipped
                } else {
                    average
                }
            }
        }
    };
    compression_to_quality(log_base.exp())
}

/// Inverse of libwebp's `QualityToCompression`, rounded to one decimal.
fn compression_to_quality(c: f64) -> f32 {
    let linear = c.clamp(0.0, 1.0).powi(3);
    let quality = if linear < 0.5 {
        linear * 1.5
    } else {
        (linear + 1.0) / 2.0
    };
    ((quality * 1000.0).round() / 10.0) as f32
}

// ------------------------------------------------------------------------------------ encode

/// First quality tried by [`encode_jpeg_within`].
pub const JPEG_START_QUALITY: u8 = 92;
/// Quality decrement between attempts in [`encode_jpeg_within`].
pub const JPEG_QUALITY_STEP: u8 = 5;

/// An encoded image.
#[derive(Clone, PartialEq, Eq)]
pub struct Encoded {
    pub bytes: Vec<u8>,
    /// The JPEG quality used.
    pub quality: u8,
}

impl fmt::Debug for Encoded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Encoded")
            .field("len", &self.bytes.len())
            .field("quality", &self.quality)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EncodeError {
    #[error(
        "the image does not fit in {max_bytes} bytes at JPEG quality {min_quality} or above (smallest attempt: {smallest} bytes)"
    )]
    OverBudget {
        max_bytes: usize,
        min_quality: u8,
        smallest: usize,
    },
    #[error("invalid encode parameters: {0}")]
    InvalidParameters(String),
    #[error("encoding failed: {0}")]
    Failed(String),
}

/// Encode as baseline JPEG, starting at quality [`JPEG_START_QUALITY`] and stepping down by
/// [`JPEG_QUALITY_STEP`] until the file fits in `max_bytes`. Never goes below `min_quality`; the
/// last attempt is exactly `min_quality`. Writes no EXIF, ICC or XMP (only the JFIF header).
pub fn encode_jpeg_within(
    image: &RgbImage,
    max_bytes: usize,
    min_quality: u8,
) -> Result<Encoded, EncodeError> {
    if !(1..=100).contains(&min_quality) {
        return Err(EncodeError::InvalidParameters(format!(
            "min_quality {min_quality} is outside 1-100"
        )));
    }
    let mut quality = JPEG_START_QUALITY.max(min_quality);
    loop {
        let bytes = encode_jpeg(image, quality)?;
        if bytes.len() <= max_bytes {
            return Ok(Encoded { bytes, quality });
        }
        if quality == min_quality {
            return Err(EncodeError::OverBudget {
                max_bytes,
                min_quality,
                smallest: bytes.len(),
            });
        }
        quality = quality.saturating_sub(JPEG_QUALITY_STEP).max(min_quality);
    }
}

/// Encode as baseline JPEG at a fixed quality (1-100). No metadata besides the JFIF header.
pub fn encode_jpeg(image: &RgbImage, quality: u8) -> Result<Vec<u8>, EncodeError> {
    check_encodable(image)?;
    if !(1..=100).contains(&quality) {
        return Err(EncodeError::InvalidParameters(format!(
            "quality {quality} is outside 1-100"
        )));
    }
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, quality)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgb8,
        )
        .map_err(|e| EncodeError::Failed(e.to_string()))?;
    Ok(out)
}

/// Encode as PNG. No metadata chunks.
pub fn encode_png(image: &RgbImage) -> Result<Vec<u8>, EncodeError> {
    check_encodable(image)?;
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgb8,
        )
        .map_err(|e| EncodeError::Failed(e.to_string()))?;
    Ok(out)
}

fn check_encodable(image: &RgbImage) -> Result<(), EncodeError> {
    if image.width() == 0 || image.height() == 0 {
        return Err(EncodeError::InvalidParameters(format!(
            "cannot encode a {}x{} image",
            image.width(),
            image.height()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::webp::WebPEncoder;
    use image::{Rgb, RgbaImage};

    // Synthetic lossy WebPs made with libwebp 1.6.0 (Pillow 12.3, method 4, default settings)
    // from noisy colour gradients. No faces, no third-party content.
    // 48x32, quality 50: small enough that libwebp uses a single segment.
    const WEBP_LOSSY_Q50_ONE_SEGMENT: &str = concat!(
        "52494646e40100005745425056503820d8010000b008009d012a3000200002c04c25b00274cb090f5f14ec21",
        "90c2b77182edc7e81e00d5080a3da0569775e6604a9aed73f8edf6261bf56dd76cd7228cbf8ad7558a154d1a",
        "b7c4c121d7b6f268400000fefa7ea049827b44f33c36162a63dcfb294db8119cae90faa72d7ca8e946d20d7f",
        "377fa1abddd9a532e6dcf37f76b2beddfd167990c476ecd706c68cc7ecc9189524493c09c955487f33e70b80",
        "de8d8b8957d452c8fb39dc1dd40936578814d131d85ff09187c5a5a198311b130819613a1caad4d2044a4692",
        "589d613d1580e1f9bc8a8395261b757f91b36a01829c7ee59305b90ada343b4e742d73c534877ed362847252",
        "d65b5d374cebcbb61470c356f42e590fa9e60a0efa2ed57a5615743c4ac4593d20f19bff4a41200af9525ceb",
        "34d791cc34cb30afebe2de4daca14f90df9dc280a01c0bea8539781cbc2cc9fe5a42113e00e31bb171aa7981",
        "602edf71830c8c7ef83960dc8f4a3cb5e283ceaf2109a4049a52b1c762424a723f11188a999d9d5682d797d6",
        "0517f6b2e897ad77c01ffa2cd3c8435ea408739b2b15664311aca63abef18147e44d554143e84bcb5b10b59c",
        "fdb651fd9a959059283e815f71503efe3fd0ba57da8f0f05ddfd54c30941523bccb2763ef38f1a91a522505d",
        "7f26b6489a43e000",
    );
    // 64x48, quality 60, half smooth and half noisy: four segments (quantisers 45, 36, 29, 19).
    const WEBP_LOSSY_Q60_FOUR_SEGMENTS: &str = concat!(
        "524946464a02000057454250565038203e020000300c009d012a400030003eb5489d49a72422b3b008027016",
        "896c00c10f1305d073f065b7fde6acb9375ddd0768d9decb03fd1e763bc16f49f8adf50ad5e604fee1ae01d2",
        "dbbe60edfc5d751492b4dfa6ba5c521bbf5e2de45b102e41afb6958e33d761430da2130dda8000feff0d9a51",
        "ece782a0697a6da992770fe1fdf1a5ebc0f274fe70b4bd0b45a925943133ceba220b1edcda1b663b40f6e40e",
        "4ee8d9007c6d91dcbbc58131dd3ff8eb9078fcce90b2514f9b4d528be6e13284a1c87793a8d56c785a868d25",
        "c53e9c9d7192b272e02a2dbd2a9e9a1d2a90a5c1a7639ad7345665566c0be69d72ff882fd831b9714ea72318",
        "fc4e0ecd0e2f1166faf1c39b102131aabb75b149373421aa8be234fce70fb57117d7eb82dabb9c54199c531d",
        "5955db13b3b67c7278de4d75eaa5631c3f16b9423f5eb98c8b9c6e38179b7f32b6a75a39b15da337ac8ef218",
        "6a522f7ac06a3256522d888db1d7c96c47809c54fc53a198ecd908355878fb4fab68c58e93e055b5ad9fa197",
        "8ae66f7b98e202066603e612d1d5ded50373d1bbc453907090a467ada2c164168efafa5095b8e45a9d911e6a",
        "3d930c60d7ca3177528dc0b6e4aec71eb82d947ad28005afc898449392206aebf34127b0cd867338a521fe04",
        "83cdbdff750b02849bec0cb848735ed71f85f5a031b7a223f87a751e990ec92693c1edf503edb2bba4d312a0",
        "e8287dfd7bde48fed0552e15217490dd09efd675d8ab9c956311dff418e5139e296046a7c3309ca7830bff9d",
        "b1444179a068c8c2cbd273f14c665f1f77ba71600000",
    );

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn limits() -> DecodeLimits {
        DecodeLimits::new(10 << 20, 40_000_000, &Format::ALL)
    }

    /// Deterministic texture (gradient plus LCG noise) so JPEG size depends on quality.
    fn textured(width: u32, height: u32) -> RgbImage {
        let mut state: u32 = 12345;
        RgbImage::from_fn(width, height, |x, y| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let noise = ((state >> 16) % 64) as u8;
            Rgb([
                ((x * 255 / width) as u8).saturating_add(noise / 2),
                ((y * 255 / height) as u8).saturating_add(noise / 3),
                noise.saturating_mul(3),
            ])
        })
    }

    /// Left half red, right half blue.
    fn halves(width: u32, height: u32) -> RgbImage {
        RgbImage::from_fn(width, height, |x, _| {
            if x < width / 2 {
                Rgb([230, 20, 20])
            } else {
                Rgb([20, 20, 230])
            }
        })
    }

    fn png(image: &RgbImage) -> Vec<u8> {
        encode_png(image).unwrap()
    }

    fn is_red(p: Rgb<u8>) -> bool {
        p.0[0] > 150 && p.0[2] < 100
    }

    fn is_blue(p: Rgb<u8>) -> bool {
        p.0[2] > 150 && p.0[0] < 100
    }

    /// Insert an EXIF APP1 segment carrying only an orientation tag right after SOI.
    fn with_exif_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II\x2A\x00"); // little-endian TIFF header
        tiff.extend_from_slice(&8u32.to_le_bytes()); // IFD0 offset
        tiff.extend_from_slice(&1u16.to_le_bytes()); // one entry
        tiff.extend_from_slice(&0x0112u16.to_le_bytes()); // Orientation
        tiff.extend_from_slice(&3u16.to_le_bytes()); // SHORT
        tiff.extend_from_slice(&1u32.to_le_bytes()); // count
        tiff.extend_from_slice(&orientation.to_le_bytes());
        tiff.extend_from_slice(&[0, 0]); // value padding
        tiff.extend_from_slice(&0u32.to_le_bytes()); // no next IFD
        let mut payload = b"Exif\0\0".to_vec();
        payload.extend_from_slice(&tiff);
        let mut out = jpeg[..2].to_vec();
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
        out.extend_from_slice(&payload);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    fn jpeg_markers(jpeg: &[u8]) -> Vec<u8> {
        let mut markers = Vec::new();
        let mut i = 2;
        while i + 4 <= jpeg.len() && jpeg[i] == 0xFF {
            let marker = jpeg[i + 1];
            markers.push(marker);
            if marker == 0xDA {
                break;
            }
            i += 2 + usize::from(u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]));
        }
        markers
    }

    fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        // CRC-32 (IEEE), bitwise: test-only, a few dozen bytes.
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in kind.iter().chain(data) {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        let mut out = u32::try_from(data.len()).unwrap().to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&(!crc).to_be_bytes());
        out
    }

    // ------------------------------------------------------------------------------ sniffing

    #[test]
    fn sniffs_formats_by_magic_bytes_only() {
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]), Some(Format::Jpeg));
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR"), Some(Format::Png));
        assert_eq!(sniff(b"RIFF\x10\0\0\0WEBPVP8 "), Some(Format::Webp));
        assert_eq!(
            sniff(&encode_jpeg(&textured(8, 8), 80).unwrap()),
            Some(Format::Jpeg)
        );
        assert_eq!(sniff(&png(&textured(8, 8))), Some(Format::Png));

        assert_eq!(sniff(b""), None);
        assert_eq!(sniff(&[0xFF, 0xD8]), None);
        assert_eq!(sniff(b"\x89PNG\r\n"), None);
        assert_eq!(sniff(b"GIF89a\x01\0\x01\0"), None);
        assert_eq!(sniff(b"RIFF\x10\0\0\0WAVEfmt "), None);
        assert_eq!(sniff(b"BM\0\0\0\0\0\0\0\0\0\0"), None);
        assert_eq!(sniff(b"<svg xmlns='http://www.w3.org/2000/svg'/>"), None);
    }

    // ------------------------------------------------------------------------------- limits

    #[test]
    fn byte_limit_is_checked_first() {
        let bytes = png(&textured(32, 32));
        let mut tight = limits();
        tight.max_bytes = bytes.len() - 1;
        assert_eq!(
            decode(&bytes, &tight).unwrap_err(),
            DecodeError::TooManyBytes {
                size: bytes.len(),
                limit: bytes.len() - 1
            }
        );
        // Garbage over the size limit is still a size problem, not a format problem.
        let garbage = vec![0u8; 100];
        tight.max_bytes = 99;
        assert!(matches!(
            decode(&garbage, &tight),
            Err(DecodeError::TooManyBytes { .. })
        ));
        tight.max_bytes = bytes.len();
        assert!(decode(&bytes, &tight).is_ok(), "the limit is inclusive");
    }

    #[test]
    fn unknown_and_disallowed_formats_are_unsupported() {
        assert!(matches!(
            decode(b"GIF89a\x01\0\x01\0\0\0\0", &limits()),
            Err(DecodeError::UnsupportedFormat { detected: None, .. })
        ));
        let jpeg_only = DecodeLimits::new(10 << 20, 40_000_000, &[Format::Jpeg]);
        assert!(matches!(
            decode(&png(&textured(16, 16)), &jpeg_only),
            Err(DecodeError::UnsupportedFormat {
                detected: Some(Format::Png),
                ..
            })
        ));
    }

    #[test]
    fn pixel_limit_uses_the_header_before_any_pixel_data() {
        // Headers that declare huge images and carry no pixel data at all: if the decoder ran
        // first, these would fail as corrupt rather than as too large.
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&50_000u32.to_be_bytes());
        ihdr.extend_from_slice(&40_000u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
        let mut fake_png = b"\x89PNG\r\n\x1a\n".to_vec();
        fake_png.extend_from_slice(&png_chunk(b"IHDR", &ihdr));
        fake_png.extend_from_slice(&png_chunk(b"IEND", &[]));
        assert_eq!(
            decode(&fake_png, &limits()).unwrap_err(),
            DecodeError::TooManyPixels {
                width: 50_000,
                height: 40_000,
                limit: 40_000_000
            }
        );

        // JPEG: SOI, then a baseline frame header declaring 60000x60000, then EOI.
        let mut fake_jpeg = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 8];
        fake_jpeg.extend_from_slice(&60_000u16.to_be_bytes());
        fake_jpeg.extend_from_slice(&60_000u16.to_be_bytes());
        fake_jpeg.extend_from_slice(&[3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1, 0xFF, 0xD9]);
        assert!(matches!(
            decode(&fake_jpeg, &limits()),
            Err(DecodeError::TooManyPixels {
                width: 60_000,
                height: 60_000,
                ..
            })
        ));

        // WebP extended header declaring a 16384x16384 canvas.
        let mut vp8x = vec![0u8; 10];
        vp8x[4..7].copy_from_slice(&16_383u32.to_le_bytes()[..3]);
        vp8x[7..10].copy_from_slice(&16_383u32.to_le_bytes()[..3]);
        let mut fake_webp = b"RIFF\x16\0\0\0WEBPVP8X\x0a\0\0\0".to_vec();
        fake_webp.extend_from_slice(&vp8x);
        assert!(matches!(
            decode(&fake_webp, &limits()),
            Err(DecodeError::TooManyPixels {
                width: 16_384,
                height: 16_384,
                ..
            })
        ));

        // A real image just over the limit.
        let mut small = limits();
        small.max_pixels = 32 * 32 - 1;
        assert!(matches!(
            decode(&png(&textured(32, 32)), &small),
            Err(DecodeError::TooManyPixels { .. })
        ));
        small.max_pixels = 32 * 32;
        assert!(decode(&png(&textured(32, 32)), &small).is_ok());
    }

    #[test]
    fn allocation_limit_reports_too_many_pixels() {
        // A 16-bit RGBA PNG needs 8 bytes per pixel; give the decoder less than that.
        let image = image::ImageBuffer::<image::Rgba<u16>, Vec<u16>>::from_pixel(
            64,
            64,
            image::Rgba([1000, 2000, 3000, 65535]),
        );
        // `write_image` takes 16-bit samples as native-endian bytes.
        let raw: Vec<u8> = image
            .as_raw()
            .iter()
            .flat_map(|s| s.to_ne_bytes())
            .collect();
        let mut bytes = Vec::new();
        PngEncoder::new(&mut bytes)
            .write_image(&raw, 64, 64, ExtendedColorType::Rgba16)
            .unwrap();
        let mut tight = limits();
        tight.max_alloc = 1024;
        assert!(matches!(
            decode(&bytes, &tight),
            Err(DecodeError::TooManyPixels {
                width: 64,
                height: 64,
                ..
            })
        ));
        assert!(decode(&bytes, &limits()).is_ok());
    }

    #[test]
    fn corrupt_images_fail_to_decode() {
        let mut truncated = png(&textured(64, 64));
        truncated.truncate(truncated.len() / 2);
        assert!(matches!(
            decode(&truncated, &limits()),
            Err(DecodeError::Corrupt { .. })
        ));

        let mut garbage_jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0];
        garbage_jpeg.extend_from_slice(&[0x5A; 64]);
        assert!(matches!(
            decode(&garbage_jpeg, &limits()),
            Err(DecodeError::Corrupt { .. })
        ));

        let garbage_webp = b"RIFF\x0c\0\0\0WEBPJUNK\0\0\0\0".to_vec();
        assert!(matches!(
            decode(&garbage_webp, &limits()),
            Err(DecodeError::Corrupt { .. })
        ));

        // A zero-sized header is corrupt, not "too large".
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&0u32.to_be_bytes());
        ihdr.extend_from_slice(&10u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        let mut zero = b"\x89PNG\r\n\x1a\n".to_vec();
        zero.extend_from_slice(&png_chunk(b"IHDR", &ihdr));
        assert!(matches!(
            decode(&zero, &limits()),
            Err(DecodeError::Corrupt { .. })
        ));
    }

    // ------------------------------------------------------------------- orientation, colour

    #[test]
    fn applies_exif_orientation() {
        let jpeg = encode_jpeg(&halves(64, 32), 95).unwrap();

        let plain = decode(&jpeg, &limits()).unwrap().image;
        assert_eq!(plain.dimensions(), (64, 32));
        assert!(is_red(*plain.get_pixel(8, 16)) && is_blue(*plain.get_pixel(56, 16)));

        // 6 = rotate 90 degrees clockwise to display: the left (red) half ends up on top.
        let rotated = decode(&with_exif_orientation(&jpeg, 6), &limits())
            .unwrap()
            .image;
        assert_eq!(rotated.dimensions(), (32, 64));
        assert!(
            is_red(*rotated.get_pixel(16, 8)),
            "{:?}",
            rotated.get_pixel(16, 8)
        );
        assert!(is_blue(*rotated.get_pixel(16, 56)));

        // 8 = rotate 270 degrees clockwise: the left (red) half ends up at the bottom.
        let rotated = decode(&with_exif_orientation(&jpeg, 8), &limits())
            .unwrap()
            .image;
        assert_eq!(rotated.dimensions(), (32, 64));
        assert!(is_blue(*rotated.get_pixel(16, 8)) && is_red(*rotated.get_pixel(16, 56)));

        // 3 = rotate 180 degrees: left and right swap.
        let flipped = decode(&with_exif_orientation(&jpeg, 3), &limits())
            .unwrap()
            .image;
        assert_eq!(flipped.dimensions(), (64, 32));
        assert!(is_blue(*flipped.get_pixel(8, 16)) && is_red(*flipped.get_pixel(56, 16)));

        // An invalid orientation value is ignored, not an error.
        let odd = decode(&with_exif_orientation(&jpeg, 42), &limits())
            .unwrap()
            .image;
        assert_eq!(odd.dimensions(), (64, 32));
    }

    #[test]
    fn composites_alpha_over_white() {
        let mut rgba = RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 0]));
        rgba.put_pixel(1, 1, image::Rgba([0, 0, 0, 255]));
        rgba.put_pixel(2, 2, image::Rgba([0, 0, 0, 128]));
        let mut bytes = Vec::new();
        PngEncoder::new(&mut bytes)
            .write_image(rgba.as_raw(), 4, 4, ExtendedColorType::Rgba8)
            .unwrap();
        let decoded = decode(&bytes, &limits()).unwrap();
        assert_eq!(decoded.image.get_pixel(0, 0).0, [255, 255, 255]);
        assert_eq!(decoded.image.get_pixel(1, 1).0, [0, 0, 0]);
        assert_eq!(decoded.image.get_pixel(2, 2).0, [127, 127, 127]);
    }

    #[test]
    fn debug_output_never_contains_pixels() {
        let decoded = decode(&png(&textured(8, 8)), &limits()).unwrap();
        let text = format!("{decoded:?}");
        assert!(text.contains("width: 8") && text.len() < 120, "{text}");
    }

    // -------------------------------------------------------------------- quality estimates

    #[test]
    fn jpeg_quality_estimate_is_within_three_of_the_encoder_setting() {
        let image = textured(96, 64);
        for quality in [10u8, 20, 35, 50, 60, 70, 75, 85, 90, 92, 95, 98] {
            let jpeg = encode_jpeg(&image, quality).unwrap();
            let estimate = decode(&jpeg, &limits()).unwrap().jpeg_quality.unwrap();
            assert!(
                (estimate - f32::from(quality)).abs() <= 3.0,
                "quality {quality}: estimated {estimate}"
            );
        }
    }

    #[test]
    fn jpeg_quality_uses_sixteen_bit_and_grayscale_tables() {
        // A grayscale frame whose only (16-bit) table is the IJG luminance table at quality 40.
        let scale = ijg_scale(40);
        let mut dqt = vec![0x10u8]; // 16-bit precision, table 0
        for &natural in &UNZIGZAG {
            let v = ((u32::from(STD_LUMINANCE[natural]) * scale + 50) / 100).clamp(1, 32767);
            dqt.extend_from_slice(&u16::try_from(v).unwrap().to_be_bytes());
        }
        let mut header = JpegHeader::default();
        parse_dqt(&dqt, &mut header.tables);
        parse_sof(&[8, 0, 16, 0, 16, 1, 1, 0x11, 0], &mut header);
        assert_eq!(header.dimensions, Some((16, 16)));
        assert_eq!(header.estimate_quality(), Some(40.0));
    }

    #[test]
    fn lossless_formats_report_quality_100() {
        let decoded = decode(&png(&textured(16, 16)), &limits()).unwrap();
        assert_eq!(
            (decoded.format, decoded.jpeg_quality),
            (Format::Png, Some(100.0))
        );

        let image = textured(16, 16);
        let mut webp = Vec::new();
        WebPEncoder::new_lossless(&mut webp)
            .write_image(image.as_raw(), 16, 16, ExtendedColorType::Rgb8)
            .unwrap();
        let decoded = decode(&webp, &limits()).unwrap();
        assert_eq!(
            (decoded.format, decoded.jpeg_quality),
            (Format::Webp, Some(100.0))
        );
        assert_eq!(decoded.image.dimensions(), (16, 16));
    }

    #[test]
    fn lossy_webp_quality_is_estimated_from_real_libwebp_files() {
        for (fixture, quality, size) in [
            (WEBP_LOSSY_Q50_ONE_SEGMENT, 50.0, (48, 32)),
            (WEBP_LOSSY_Q60_FOUR_SEGMENTS, 60.0, (64, 48)),
        ] {
            let bytes = hex(fixture);
            let decoded = decode(&bytes, &limits()).unwrap();
            assert_eq!(decoded.image.dimensions(), size);
            let estimate = decoded.jpeg_quality.unwrap();
            assert!(
                (estimate - quality).abs() <= 3.0,
                "quality {quality}: estimated {estimate}"
            );
        }
        let four = hex(WEBP_LOSSY_Q60_FOUR_SEGMENTS);
        let Some(WebpBitstream::Lossy(vp8)) = first_webp_bitstream(&four) else {
            panic!("expected a lossy bitstream");
        };
        assert_eq!(vp8_segment_quantizers(vp8), Some(vec![45, 36, 29, 19]));
    }

    /// RFC 6386 section 7.3 boolean encoder, to build VP8 frame headers in tests.
    struct BoolEncoder {
        out: Vec<u8>,
        range: u32,
        bottom: u32,
        bit_count: i32,
    }

    impl BoolEncoder {
        fn new() -> Self {
            BoolEncoder {
                out: Vec::new(),
                range: 255,
                bottom: 0,
                bit_count: 24,
            }
        }

        fn carry(&mut self) {
            for byte in self.out.iter_mut().rev() {
                if *byte == 255 {
                    *byte = 0;
                } else {
                    *byte += 1;
                    break;
                }
            }
        }

        fn put(&mut self, bit: bool) {
            let split = 1 + (((self.range - 1) * 128) >> 8);
            if bit {
                self.bottom = self.bottom.wrapping_add(split);
                self.range -= split;
            } else {
                self.range = split;
            }
            while self.range < 128 {
                self.range <<= 1;
                if self.bottom & (1 << 31) != 0 {
                    self.carry();
                }
                self.bottom <<= 1;
                self.bit_count -= 1;
                if self.bit_count == 0 {
                    self.out.push((self.bottom >> 24) as u8);
                    self.bottom &= (1 << 24) - 1;
                    self.bit_count = 8;
                }
            }
        }

        fn literal(&mut self, value: u32, bits: u32) {
            for i in (0..bits).rev() {
                self.put((value >> i) & 1 == 1);
            }
        }

        fn signed(&mut self, value: i32, bits: u32) {
            self.put(value != 0);
            if value != 0 {
                self.literal(value.unsigned_abs(), bits);
                self.put(value < 0);
            }
        }

        fn finish(mut self) -> Vec<u8> {
            let mut c = self.bit_count;
            let mut v = self.bottom;
            if c < 32 && v & (1u32 << (32 - c)) != 0 {
                self.carry();
            }
            v <<= c & 7;
            c >>= 3;
            while c > 0 {
                v <<= 8;
                c -= 1;
            }
            for _ in 0..4 {
                self.out.push((v >> 24) as u8);
                v <<= 8;
            }
            self.out
        }
    }

    /// A VP8 key-frame chunk payload whose header carries the given quantisers.
    fn vp8_with_quantizers(y_ac_qi: u32, segments: Option<([i32; 4], bool)>) -> Vec<u8> {
        let mut e = BoolEncoder::new();
        e.literal(0, 1); // colour space
        e.literal(0, 1); // clamping
        e.put(segments.is_some());
        if let Some((quantizers, absolute)) = segments {
            e.put(true); // update map
            e.put(true); // update data
            e.put(absolute);
            for q in quantizers {
                e.signed(q, 7);
            }
            for _ in 0..4 {
                e.signed(0, 6);
            }
            for _ in 0..3 {
                e.put(true);
                e.literal(200, 8);
            }
        }
        e.put(false); // filter type
        e.literal(20, 6);
        e.literal(0, 3);
        e.put(true); // loop filter adjustments enabled
        e.put(true); // ... and updated
        for delta in [2, 0, -2, -2, 4, -2, 2, 4] {
            e.signed(delta, 6);
        }
        e.literal(0, 2); // one token partition
        e.literal(y_ac_qi, 7);
        for _ in 0..5 {
            e.signed(0, 4);
        }
        let partition = e.finish();
        let size = u32::try_from(partition.len()).unwrap();
        let tag = (size << 5) | (1 << 4); // key frame, version 0, shown
        let mut out = tag.to_le_bytes()[..3].to_vec();
        out.extend_from_slice(&[0x9D, 0x01, 0x2A, 64, 0, 48, 0]);
        out.extend_from_slice(&partition);
        out
    }

    /// libwebp's quantiser for a segment: `VP8SetSegmentParams` with default settings.
    fn libwebp_quantizer(quality: f64, alpha: f64) -> i32 {
        let q = quality / 100.0;
        let linear = if q < 0.75 {
            q * 2.0 / 3.0
        } else {
            2.0 * q - 1.0
        };
        let c_base = linear.powf(1.0 / 3.0);
        let c = c_base.powf(1.0 - SNS_AMPLITUDE * alpha);
        ((127.0 * (1.0 - c)) as i32).clamp(0, 127)
    }

    #[test]
    fn vp8_header_parser_reads_every_segment_mode() {
        let none = vp8_with_quantizers(57, None);
        assert_eq!(vp8_segment_quantizers(&none), Some(vec![57]));
        let absolute = vp8_with_quantizers(40, Some(([45, 36, 29, 19], true)));
        assert_eq!(
            vp8_segment_quantizers(&absolute),
            Some(vec![45, 36, 29, 19])
        );
        let delta = vp8_with_quantizers(40, Some(([5, -4, 0, -100], false)));
        assert_eq!(vp8_segment_quantizers(&delta), Some(vec![45, 36, 40, 0]));
        // Not a key frame, or no start code: no estimate.
        let mut inter = absolute.clone();
        inter[0] |= 1;
        assert_eq!(vp8_segment_quantizers(&inter), None);
        assert_eq!(vp8_segment_quantizers(&absolute[..4]), None);
    }

    #[test]
    fn single_segment_quality_inverts_libwebp_exactly() {
        for quality in [10.0, 25.0, 50.0, 60.0, 75.0, 80.0, 90.0, 95.0] {
            let q = libwebp_quantizer(quality, 0.0);
            let vp8 = vp8_with_quantizers(u32::try_from(q).unwrap(), None);
            let estimate = webp_lossy_quality(&vp8_segment_quantizers(&vp8).unwrap());
            // Only the integer truncation of the quantiser is lost.
            assert!(
                (f64::from(estimate) - quality).abs() <= 1.5,
                "quality {quality}: q {q}, estimated {estimate}"
            );
        }
    }

    #[test]
    fn segmented_quality_recovers_libwebp_base_compression() {
        // Segment alphas as libwebp derives them: 255 * (centre - mean) / (max - min), clipped.
        for alphas in [
            [-99.0, 3.0, 73.0, 127.0],    // textured content: top end clipped
            [-127.0, -99.0, -22.0, 38.0], // smooth content: bottom end clipped
            [-127.0, -40.0, 40.0, 127.0], // symmetric
        ] {
            for quality in [30.0, 50.0, 60.0, 75.0, 85.0, 90.0] {
                let quantizers: Vec<u8> = alphas
                    .iter()
                    .map(|&a| u8::try_from(libwebp_quantizer(quality, a)).unwrap())
                    .collect();
                let estimate = webp_lossy_quality(&quantizers);
                assert!(
                    (f64::from(estimate) - quality).abs() <= 3.0,
                    "quality {quality}, alphas {alphas:?}, q {quantizers:?}: estimated {estimate}"
                );
            }
        }
        assert_eq!(webp_lossy_quality(&[0, 0, 0, 0]), 100.0);
    }

    // ------------------------------------------------------------------------------- encode

    #[test]
    fn encodes_at_92_when_the_budget_allows() {
        let image = textured(64, 48);
        let encoded = encode_jpeg_within(&image, 1 << 20, 60).unwrap();
        assert_eq!(encoded.quality, JPEG_START_QUALITY);
        assert_eq!(encoded.bytes, encode_jpeg(&image, 92).unwrap());
        let decoded = decode(&encoded.bytes, &limits()).unwrap();
        assert_eq!(decoded.image.dimensions(), (64, 48));
        assert_eq!(decoded.jpeg_quality, Some(92.0));
    }

    #[test]
    fn steps_down_by_five_until_the_file_fits() {
        let image = textured(160, 120);
        let size_at = |q| encode_jpeg(&image, q).unwrap().len();
        // A budget that 72 fits but 77 does not.
        let budget = size_at(72);
        assert!(size_at(77) > budget, "texture too simple for this test");
        let encoded = encode_jpeg_within(&image, budget, 30).unwrap();
        assert_eq!(encoded.quality, 72);
        assert!(encoded.bytes.len() <= budget);
        // No metadata: only the JFIF header, tables, frame and scan.
        let markers = jpeg_markers(&encoded.bytes);
        assert!(
            !markers.iter().any(|m| (0xE1..=0xEF).contains(m)),
            "{markers:x?}"
        );
        assert!(!markers.contains(&0xFE), "no comment segment");
    }

    #[test]
    fn last_attempt_is_exactly_the_floor() {
        let image = textured(160, 120);
        // 92, 87, ..., 62, then 60 (clamped), never 57.
        let budget = encode_jpeg(&image, 60).unwrap().len();
        assert!(encode_jpeg(&image, 62).unwrap().len() > budget);
        let encoded = encode_jpeg_within(&image, budget, 60).unwrap();
        assert_eq!(encoded.quality, 60);
    }

    #[test]
    fn fails_when_even_the_floor_is_too_big() {
        let image = textured(160, 120);
        let smallest = encode_jpeg(&image, 70).unwrap().len();
        assert_eq!(
            encode_jpeg_within(&image, 100, 70),
            Err(EncodeError::OverBudget {
                max_bytes: 100,
                min_quality: 70,
                smallest
            })
        );
        // A floor above 92 is the only quality tried.
        let encoded = encode_jpeg_within(&image, 1 << 20, 95).unwrap();
        assert_eq!(encoded.quality, 95);
        assert!(matches!(
            encode_jpeg_within(&image, 1 << 20, 0),
            Err(EncodeError::InvalidParameters(_))
        ));
        assert!(matches!(
            encode_jpeg(&RgbImage::new(0, 0), 80),
            Err(EncodeError::InvalidParameters(_))
        ));
    }

    #[test]
    fn png_round_trips() {
        let image = textured(20, 10);
        let decoded = decode(&encode_png(&image).unwrap(), &limits()).unwrap();
        assert_eq!(decoded.image, image);
    }
}
