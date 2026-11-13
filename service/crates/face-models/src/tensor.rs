//! Owned `f32` tensors and image-to-tensor conversion following a model's manifest `io` spec.

use face_imaging::RgbImage;

use crate::error::ModelError;
use crate::manifest::{ColorOrder, Dim, TensorSpec};

/// An owned, row-major (C order) `f32` tensor.
#[derive(Clone, Debug, PartialEq)]
pub struct TensorF32 {
    shape: Vec<usize>,
    data: Vec<f32>,
}

impl TensorF32 {
    /// Wrap `data`; its length must equal the product of `shape`.
    pub fn new(shape: Vec<usize>, data: Vec<f32>) -> Result<TensorF32, ModelError> {
        let n: usize = shape.iter().product();
        if n != data.len() {
            return Err(ModelError::InvalidInput(format!(
                "tensor shape {shape:?} needs {n} values, got {}",
                data.len()
            )));
        }
        Ok(TensorF32 { shape, data })
    }

    pub fn zeros(shape: Vec<usize>) -> TensorF32 {
        let n = shape.iter().product();
        TensorF32 {
            shape,
            data: vec![0.0; n],
        }
    }

    pub fn shape(&self) -> &[usize] {
        &self.shape
    }

    pub fn data(&self) -> &[f32] {
        &self.data
    }

    pub fn into_data(self) -> Vec<f32> {
        self.data
    }

    /// Number of elements.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

/// Pixel normalisation: `value = pixel * scale + offset`, with `pixel` in 0..=255.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Normalization {
    pub scale: f32,
    pub offset: f32,
}

impl Normalization {
    /// `x / 255`, i.e. `[0, 1]`.
    pub const UNIT: Normalization = Normalization {
        scale: 1.0 / 255.0,
        offset: 0.0,
    };
    /// `(x - 127.5) / 127.5`, i.e. `[-1, 1]`.
    pub const SYMMETRIC: Normalization = Normalization {
        scale: 1.0 / 127.5,
        offset: -1.0,
    };

    /// Parse a manifest `valueRange`. Accepted forms (whitespace ignored; anything after the
    /// expression in parentheses, such as `x/255 ([0,1])`, is a comment):
    /// `(x-M)/S`, `x/S`, `x`, `[A,B]` (0..255 mapped linearly onto A..B).
    pub fn parse(text: &str) -> Option<Normalization> {
        let s: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        let number = |t: &str| -> Option<(f64, usize)> {
            let len = t
                .char_indices()
                .take_while(|(i, c)| {
                    c.is_ascii_digit() || *c == '.' || ((*c == '-' || *c == '+') && *i == 0)
                })
                .count();
            t[..len].parse::<f64>().ok().map(|v| (v, len))
        };
        if let Some(rest) = s.strip_prefix("(x-") {
            let (m, n) = number(rest)?;
            let rest = rest[n..].strip_prefix(")/")?;
            let (sd, _) = number(rest)?;
            (sd != 0.0).then(|| Normalization {
                scale: (1.0 / sd) as f32,
                offset: (-m / sd) as f32,
            })
        } else if let Some(rest) = s.strip_prefix("x/") {
            let (sd, _) = number(rest)?;
            (sd != 0.0).then(|| Normalization {
                scale: (1.0 / sd) as f32,
                offset: 0.0,
            })
        } else if let Some(rest) = s.strip_prefix('[') {
            let (a, n) = number(rest)?;
            let rest = rest[n..].strip_prefix(',')?;
            let (b, n) = number(rest)?;
            rest[n..].strip_prefix(']')?;
            (b > a).then(|| Normalization {
                scale: ((b - a) / 255.0) as f32,
                offset: a as f32,
            })
        } else if s == "x" || s.starts_with("x(") {
            Some(Normalization {
                scale: 1.0,
                offset: 0.0,
            })
        } else {
            None
        }
    }

    #[inline]
    pub fn apply(&self, pixel: u8) -> f32 {
        f32::from(pixel) * self.scale + self.offset
    }
}

/// Memory layout of an image tensor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Nhwc,
    Nchw,
}

/// How to turn an image into a model's input tensor: size, layout, channel order and
/// normalisation, taken from the manifest `io` spec.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageInputSpec {
    pub width: u32,
    pub height: u32,
    pub layout: Layout,
    pub color: ColorOrder,
    pub normalization: Normalization,
}

impl ImageInputSpec {
    /// Read an image input spec from a manifest tensor spec of model `id`. The shape must be
    /// `[N, H, W, 3]` (NHWC) or `[N, 3, H, W]` (NCHW) with fixed `H`, `W`; `N` may be symbolic.
    /// `default_normalization` is used only when the spec has no `valueRange`.
    pub fn from_tensor_spec(
        id: &str,
        spec: &TensorSpec,
        default_normalization: Option<Normalization>,
    ) -> Result<ImageInputSpec, ModelError> {
        let bad = |reason: String| ModelError::Spec {
            id: id.to_owned(),
            reason,
        };
        if spec.shape.len() != 4 {
            return Err(bad(format!(
                "input `{}` must be rank 4, got {:?}",
                spec.name, spec.shape
            )));
        }
        let dims: Vec<Option<i64>> = spec.shape.iter().map(Dim::fixed).collect();
        let layout = match spec
            .layout
            .as_deref()
            .map(str::to_ascii_uppercase)
            .as_deref()
        {
            Some("NHWC") => Layout::Nhwc,
            Some("NCHW") => Layout::Nchw,
            Some(other) => return Err(bad(format!("unsupported layout `{other}`"))),
            None if dims[3] == Some(3) => Layout::Nhwc,
            None if dims[1] == Some(3) => Layout::Nchw,
            None => return Err(bad("cannot infer the input layout".to_owned())),
        };
        let (c, h, w) = match layout {
            Layout::Nhwc => (dims[3], dims[1], dims[2]),
            Layout::Nchw => (dims[1], dims[2], dims[3]),
        };
        if c != Some(3) {
            return Err(bad(format!("input `{}` must have 3 channels", spec.name)));
        }
        let (Some(h), Some(w)) = (h, w) else {
            return Err(bad(format!(
                "input `{}` needs a fixed height and width",
                spec.name
            )));
        };
        let color = spec.color_order.unwrap_or(ColorOrder::Rgb);
        if color == ColorOrder::Gray {
            return Err(bad("grayscale image inputs are not supported".to_owned()));
        }
        let normalization = match spec.value_range.as_deref() {
            Some(v) => Normalization::parse(v)
                .ok_or_else(|| bad(format!("unsupported valueRange `{v}`")))?,
            None => default_normalization
                .ok_or_else(|| bad(format!("input `{}` has no valueRange", spec.name)))?,
        };
        Ok(ImageInputSpec {
            width: u32::try_from(w).map_err(|_| bad("bad width".to_owned()))?,
            height: u32::try_from(h).map_err(|_| bad("bad height".to_owned()))?,
            layout,
            color,
            normalization,
        })
    }

    /// One image (exactly `width` x `height`) as a `[1, ...]` tensor.
    pub fn to_tensor(&self, image: &RgbImage) -> Result<TensorF32, ModelError> {
        self.to_batch(&[image])
    }

    /// A batch of images (each exactly `width` x `height`) as an `[N, ...]` tensor.
    pub fn to_batch(&self, images: &[&RgbImage]) -> Result<TensorF32, ModelError> {
        let (w, h) = (self.width as usize, self.height as usize);
        let plane = w * h;
        let mut data = vec![0.0f32; images.len() * plane * 3];
        let order: [usize; 3] = match self.color {
            ColorOrder::Bgr => [2, 1, 0],
            _ => [0, 1, 2],
        };
        let norm = self.normalization;
        for (n, img) in images.iter().enumerate() {
            if img.width() != self.width || img.height() != self.height {
                return Err(ModelError::InvalidInput(format!(
                    "image is {}x{}, the model needs {}x{}",
                    img.width(),
                    img.height(),
                    self.width,
                    self.height
                )));
            }
            let out = &mut data[n * plane * 3..(n + 1) * plane * 3];
            for (i, px) in img.as_raw().as_chunks::<3>().0.iter().enumerate() {
                for (k, &src_c) in order.iter().enumerate() {
                    let v = norm.apply(px[src_c]);
                    match self.layout {
                        Layout::Nhwc => out[i * 3 + k] = v,
                        Layout::Nchw => out[k * plane + i] = v,
                    }
                }
            }
        }
        let shape = match self.layout {
            Layout::Nhwc => vec![images.len(), h, w, 3],
            Layout::Nchw => vec![images.len(), 3, h, w],
        };
        TensorF32::new(shape, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn spec(shape: Vec<Dim>, layout: Option<&str>, value_range: Option<&str>) -> TensorSpec {
        TensorSpec {
            name: "input".into(),
            dtype: crate::manifest::DType::Float32,
            shape,
            layout: layout.map(str::to_owned),
            color_order: Some(ColorOrder::Rgb),
            value_range: value_range.map(str::to_owned),
            description: None,
        }
    }

    #[test]
    fn parses_the_manifest_value_ranges() {
        let close = |a: Normalization, b: Normalization| {
            (a.scale - b.scale).abs() < 1e-9 && (a.offset - b.offset).abs() < 1e-6
        };
        assert!(close(
            Normalization::parse("(x-127.5)/127.5").unwrap(),
            Normalization::SYMMETRIC
        ));
        assert!(close(
            Normalization::parse("x/255  ([0,1])").unwrap(),
            Normalization::UNIT
        ));
        assert!(close(
            Normalization::parse("[-1, 1]").unwrap(),
            Normalization {
                scale: 2.0 / 255.0,
                offset: -1.0
            }
        ));
        assert!(close(
            Normalization::parse("[0,1]").unwrap(),
            Normalization::UNIT
        ));
        let n = Normalization::parse("(x - 127.5) / 128").unwrap();
        assert!((n.apply(255) - 127.5 / 128.0).abs() < 1e-6);
        assert!(Normalization::parse("mean=0.5").is_none());
        assert!(Normalization::parse("x/0").is_none());
        assert!((Normalization::SYMMETRIC.apply(0) + 1.0).abs() < 1e-7);
        assert!((Normalization::SYMMETRIC.apply(255) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn builds_nhwc_and_nchw_tensors_with_channel_order() {
        let img = RgbImage::from_fn(2, 1, |x, _| Rgb([10 * x as u8, 100, 255]));
        let nhwc = ImageInputSpec::from_tensor_spec(
            "m",
            &spec(
                vec![Dim::Fixed(1), Dim::Fixed(1), Dim::Fixed(2), Dim::Fixed(3)],
                Some("NHWC"),
                Some("x"),
            ),
            None,
        )
        .unwrap();
        let t = nhwc.to_tensor(&img).unwrap();
        assert_eq!(t.shape(), &[1, 1, 2, 3]);
        assert_eq!(t.data(), &[0.0, 100.0, 255.0, 10.0, 100.0, 255.0]);

        let mut nchw = ImageInputSpec::from_tensor_spec(
            "m",
            &spec(
                vec![
                    Dim::Symbolic("N".into()),
                    Dim::Fixed(3),
                    Dim::Fixed(1),
                    Dim::Fixed(2),
                ],
                None,
                Some("x"),
            ),
            None,
        )
        .unwrap();
        assert_eq!(nchw.layout, Layout::Nchw);
        nchw.color = ColorOrder::Bgr;
        let t = nchw.to_batch(&[&img, &img]).unwrap();
        assert_eq!(t.shape(), &[2, 3, 1, 2]);
        // Channel 0 is blue in BGR order.
        assert_eq!(&t.data()[..6], &[255.0, 255.0, 100.0, 100.0, 0.0, 10.0]);
        assert!(nchw.to_tensor(&RgbImage::new(3, 1)).is_err());
    }

    #[test]
    fn rejects_unusable_specs() {
        let s = spec(
            vec![Dim::Fixed(1), Dim::Fixed(146), Dim::Fixed(2)],
            None,
            None,
        );
        assert!(ImageInputSpec::from_tensor_spec("m", &s, None).is_err());
        let s = spec(
            vec![Dim::Fixed(1), Dim::Fixed(8), Dim::Fixed(8), Dim::Fixed(3)],
            None,
            Some("??"),
        );
        assert!(ImageInputSpec::from_tensor_spec("m", &s, None).is_err());
        let s = spec(
            vec![Dim::Fixed(1), Dim::Fixed(8), Dim::Fixed(8), Dim::Fixed(3)],
            None,
            None,
        );
        assert_eq!(
            ImageInputSpec::from_tensor_spec("m", &s, Some(Normalization::UNIT))
                .unwrap()
                .normalization,
            Normalization::UNIT
        );
        assert!(TensorF32::new(vec![2, 2], vec![0.0; 3]).is_err());
    }
}
