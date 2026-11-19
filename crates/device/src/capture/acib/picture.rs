// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #479 F-3a (`contracts/device-binding.md`, "An iOS handset", "The frame"): the picture
//! `acib` gives, a PNG of the phone's screen, as the 8-bit sRGB frame the Runtime builds; no
//! original is kept, so the store writes the 8-bit picture and the Lab and a run read the same
//! bytes.
//!
//! **The colour space is read from the picture, never guessed** (the coordinator's ruling of 11-19
//! 08:18): a `cICP` chunk decides when present, and this build reads primaries 1 (BT.709, the sRGB
//! primaries) or 12 (P3 with a D65 white), transfer function 13 (the sRGB curve), matrix
//! coefficients 0 (RGB) and full range 1; without one, an `sRGB` chunk means sRGB. A picture with
//! neither, or with other `cICP` numbers, is [`PictureProblem::ColorSpaceUnsupported`]. An `iCCP`
//! profile is not parsed and decides nothing; an `eXIf` chunk is not read. The chunks are read from
//! the bytes before the picture data, since the PNG decoder does not hand them out.
//!
//! **The samples.** RGB of 8 or 16 bits per sample; anything else (a palette, grey, an alpha
//! channel) is [`PictureProblem::Unreadable`], never a guess.
//! - sRGB, 8 bits: the samples as they are.
//! - sRGB, 16 bits: each sample `v` becomes `(v·255 + 32767) / 65535`, which maps every `257·k`
//!   exactly to `k`.
//! - P3 (primaries 12), 8 or 16 bits: each sample to [0, 1], decoded by the sRGB transfer
//!   function to linear light; the 3×3 matrix from the P3-D65 primaries to the BT.709 primaries
//!   ([`p3_d65_to_bt709`]); each channel clipped to [0, 1]; encoded by the sRGB transfer function;
//!   rounded to 8 bits. A neutral sample (its three channels equal) is the same grey in both
//!   spaces, which share the D65 white (each row of the matrix sums to 1): it takes the 16-to-8
//!   rounding exactly.
//!
//! The cost per frame: two tables made once for the process (the linear light of each of the
//! 65 536 sample values, and the 8-bit code of each of 65 536 linear steps), then per pixel three
//! lookups, nine multiplications, three clips and three lookups; no power is computed per sample.

use std::sync::OnceLock;

/// Why a picture is no frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PictureProblem {
    /// It cannot be decoded, or is of a form the conversion does not cover.
    Unreadable,
    /// It names no colour space this build converts.
    ColorSpaceUnsupported,
}

/// An 8-bit sRGB picture: its size and its RGB pixels, row by row.
pub(super) struct Rgb8Picture {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) pixels: Vec<u8>,
}

/// The two colour spaces this build reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColorSpace {
    /// The sRGB primaries (ITU-T H.273 colour primaries 1) and curve.
    Srgb,
    /// The P3 primaries with a D65 white (colour primaries 12) and the sRGB curve.
    DisplayP3,
}

/// PNG colour type 2: RGB.
const COLOR_TYPE_RGB: u8 = 2;
/// The `cICP` numbers this build reads: colour primaries, transfer function, matrix
/// coefficients, full range.
const CICP_SRGB: [u8; 4] = [1, 13, 0, 1];
const CICP_DISPLAY_P3: [u8; 4] = [12, 13, 0, 1];

/// What the chunks before the picture data say.
struct Header {
    bit_depth: u8,
    color_type: u8,
    srgb: bool,
    cicp: Option<[u8; 4]>,
}

impl Header {
    /// The picture's colour space: its `cICP` chunk's numbers when it has one, else sRGB when it
    /// has an `sRGB` chunk.
    fn color_space(&self) -> Result<ColorSpace, PictureProblem> {
        match self.cicp {
            Some(CICP_SRGB) => Ok(ColorSpace::Srgb),
            Some(CICP_DISPLAY_P3) => Ok(ColorSpace::DisplayP3),
            Some(_) => Err(PictureProblem::ColorSpaceUnsupported),
            None if self.srgb => Ok(ColorSpace::Srgb),
            None => Err(PictureProblem::ColorSpaceUnsupported),
        }
    }
}

/// Reads the chunks of `png` before its first `IDAT`: `IHDR` (first, 13 bytes), `sRGB` and
/// `cICP` (4 bytes). Each chunk's CRC is the decoder's to check.
fn read_header(png: &[u8]) -> Result<Header, PictureProblem> {
    if !png.starts_with(crate::capture::PNG_SIGNATURE) {
        return Err(PictureProblem::Unreadable);
    }
    let mut at = crate::capture::PNG_SIGNATURE.len();
    let mut ihdr = None;
    let mut srgb = false;
    let mut cicp = None;
    loop {
        let length = png
            .get(at..at + 4)
            .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
            .map(u32::from_be_bytes)
            .and_then(|length| usize::try_from(length).ok())
            .ok_or(PictureProblem::Unreadable)?;
        let kind = png.get(at + 4..at + 8).ok_or(PictureProblem::Unreadable)?;
        let data_start = at + 8;
        let data_end = data_start
            .checked_add(length)
            .ok_or(PictureProblem::Unreadable)?;
        let next = data_end.checked_add(4).ok_or(PictureProblem::Unreadable)?;
        let data = png
            .get(data_start..data_end)
            .filter(|_| next <= png.len())
            .ok_or(PictureProblem::Unreadable)?;
        match kind {
            b"IHDR" if ihdr.is_none() && length == 13 => ihdr = Some((data[8], data[9])),
            b"IHDR" => return Err(PictureProblem::Unreadable),
            _ if ihdr.is_none() => return Err(PictureProblem::Unreadable),
            b"IDAT" | b"IEND" => break,
            b"sRGB" => srgb = true,
            b"cICP" => {
                cicp = Some(<[u8; 4]>::try_from(data).map_err(|_| PictureProblem::Unreadable)?);
            }
            _ => {}
        }
        at = next;
    }
    let (bit_depth, color_type) = ihdr.ok_or(PictureProblem::Unreadable)?;
    Ok(Header {
        bit_depth,
        color_type,
        srgb,
        cicp,
    })
}

/// The 8-bit sRGB picture of `png`.
pub(super) fn to_srgb8(png: &[u8]) -> Result<Rgb8Picture, PictureProblem> {
    let header = read_header(png)?;
    let space = header.color_space()?;
    if header.color_type != COLOR_TYPE_RGB || !matches!(header.bit_depth, 8 | 16) {
        return Err(PictureProblem::Unreadable);
    }
    let decoded = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .map_err(|_| PictureProblem::Unreadable)?;
    let (width, height, pixels) = match (decoded, space) {
        (image::DynamicImage::ImageRgb8(buffer), ColorSpace::Srgb) => {
            let (width, height) = buffer.dimensions();
            (width, height, buffer.into_raw())
        }
        (image::DynamicImage::ImageRgb8(buffer), ColorSpace::DisplayP3) => {
            let (width, height) = buffer.dimensions();
            let conversion = conversion();
            let pixels: Vec<u8> = buffer
                .into_raw()
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|pixel| conversion.p3_pixel(pixel.map(widened)))
                .collect();
            (width, height, pixels)
        }
        (image::DynamicImage::ImageRgb16(buffer), ColorSpace::Srgb) => {
            let (width, height) = buffer.dimensions();
            let pixels: Vec<u8> = buffer.into_raw().into_iter().map(rounded).collect();
            (width, height, pixels)
        }
        (image::DynamicImage::ImageRgb16(buffer), ColorSpace::DisplayP3) => {
            let (width, height) = buffer.dimensions();
            let conversion = conversion();
            let pixels: Vec<u8> = buffer
                .into_raw()
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|pixel| conversion.p3_pixel(*pixel))
                .collect();
            (width, height, pixels)
        }
        _ => return Err(PictureProblem::Unreadable),
    };
    Ok(Rgb8Picture {
        width,
        height,
        pixels,
    })
}

/// The 16-bit sample of the 8-bit sample `sample`: `sample · 257`, the same fraction of full.
fn widened(sample: u8) -> u16 {
    u16::from(sample) * 257
}

/// The 8-bit sample of the 16-bit sample `sample`: `(sample · 255 + 32767) / 65535`.
pub(super) fn rounded(sample: u16) -> u8 {
    let scaled = (u32::from(sample) * 255 + 32_767) / 65_535;
    u8::try_from(scaled).unwrap_or(u8::MAX)
}

/// The sRGB transfer function (IEC 61966-2-1; ITU-T H.273 transfer characteristics 13), from the
/// encoded value `encoded` in [0, 1] to linear light.
pub(super) fn srgb_decode(encoded: f64) -> f64 {
    if encoded <= 0.040_45 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

/// The inverse of [`srgb_decode`], from linear light `linear` in [0, 1] to the encoded value.
pub(super) fn srgb_encode(linear: f64) -> f64 {
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

/// Chromaticities (x, y) of a set of primaries and its white.
struct Primaries {
    red: (f64, f64),
    green: (f64, f64),
    blue: (f64, f64),
    white: (f64, f64),
}

/// ITU-T H.273 colour primaries 1 (BT.709, the sRGB primaries): R (0.640, 0.330), G (0.300,
/// 0.600), B (0.150, 0.060), white D65 (0.3127, 0.3290).
const BT709: Primaries = Primaries {
    red: (0.640, 0.330),
    green: (0.300, 0.600),
    blue: (0.150, 0.060),
    white: (0.3127, 0.3290),
};

/// ITU-T H.273 colour primaries 12 (SMPTE EG 432-1, P3 with a D65 white): R (0.680, 0.320), G
/// (0.265, 0.690), B (0.150, 0.060), white D65 (0.3127, 0.3290).
const P3_D65: Primaries = Primaries {
    red: (0.680, 0.320),
    green: (0.265, 0.690),
    blue: (0.150, 0.060),
    white: (0.3127, 0.3290),
};

type Matrix = [[f64; 3]; 3];

/// The XYZ of the chromaticity `(x, y)` at luminance 1: `(x / y, 1, (1 - x - y) / y)`.
fn xyz_of((x, y): (f64, f64)) -> [f64; 3] {
    [x / y, 1.0, (1.0 - x - y) / y]
}

fn multiply(left: &Matrix, right: &Matrix) -> Matrix {
    let mut product = [[0.0; 3]; 3];
    for (row, out) in product.iter_mut().enumerate() {
        for (column, cell) in out.iter_mut().enumerate() {
            *cell = (0..3).map(|k| left[row][k] * right[k][column]).sum();
        }
    }
    product
}

fn apply(matrix: &Matrix, vector: [f64; 3]) -> [f64; 3] {
    matrix.map(|row| row[0] * vector[0] + row[1] * vector[1] + row[2] * vector[2])
}

/// The inverse of `matrix` by its cofactors.
pub(super) fn inverse(matrix: &Matrix) -> Matrix {
    let [[a, b, c], [d, e, f], [g, h, i]] = *matrix;
    let determinant = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    [
        [e * i - f * h, c * h - b * i, b * f - c * e],
        [f * g - d * i, a * i - c * g, c * d - a * f],
        [d * h - e * g, b * g - a * h, a * e - b * d],
    ]
    .map(|row| row.map(|cell| cell / determinant))
}

/// The matrix from linear RGB of `primaries` to CIE XYZ: the XYZ of each primary at luminance 1
/// as a column, each column scaled so that RGB (1, 1, 1) is the white.
fn rgb_to_xyz(primaries: &Primaries) -> Matrix {
    let [red, green, blue] = [primaries.red, primaries.green, primaries.blue].map(xyz_of);
    let columns = [
        [red[0], green[0], blue[0]],
        [red[1], green[1], blue[1]],
        [red[2], green[2], blue[2]],
    ];
    let scale = apply(&inverse(&columns), xyz_of(primaries.white));
    columns.map(|row| [row[0] * scale[0], row[1] * scale[1], row[2] * scale[2]])
}

/// The matrix from linear P3-D65 RGB to linear BT.709 RGB, derived from the chromaticities of
/// ITU-T H.273 colour primaries 12 and 1: `M709⁻¹ · MP3`, each `M` the RGB-to-XYZ matrix of its
/// primaries ([`rgb_to_xyz`]). Worked through:
///
/// ```text
/// MP3  = [0.4865709486 0.2656676932 0.1982172852]   M709 = [0.4123907993 0.3575843394 0.1804807884]
///        [0.2289745641 0.6917385218 0.0792869141]          [0.2126390059 0.7151686788 0.0721923154]
///        [0.0000000000 0.0451133819 1.0439443689]          [0.0193308187 0.1191947798 0.9505321522]
///
/// M709⁻¹ · MP3 = [ 1.2249401763 -0.2249401763  0.0000000000]
///                [-0.0420569547  1.0420569547  0.0000000000]
///                [-0.0196375546 -0.0786360456  1.0982736001]
/// ```
///
/// Each row sums to 1 (the two share the D65 white), and the blue column of the first two rows is
/// 0 (the two share the blue primary).
pub(super) fn p3_d65_to_bt709() -> Matrix {
    multiply(&inverse(&rgb_to_xyz(&BT709)), &rgb_to_xyz(&P3_D65))
}

/// The conversion's tables and matrix, made once for the process.
struct Conversion {
    /// The linear light of each 16-bit sample value.
    decode: Vec<f64>,
    /// The 8-bit sRGB code of each linear step `i / 65535`, rounded half up.
    encode: Vec<u8>,
    matrix: Matrix,
}

fn conversion() -> &'static Conversion {
    static CONVERSION: OnceLock<Conversion> = OnceLock::new();
    CONVERSION.get_or_init(|| Conversion {
        decode: (0..=u16::MAX)
            .map(|sample| srgb_decode(f64::from(sample) / 65_535.0))
            .collect(),
        encode: (0..=u16::MAX)
            .map(|step| {
                let code = (srgb_encode(f64::from(step) / 65_535.0) * 255.0 + 0.5).floor();
                code.clamp(0.0, 255.0) as u8
            })
            .collect(),
        matrix: p3_d65_to_bt709(),
    })
}

impl Conversion {
    /// The 8-bit sRGB pixel of the 16-bit P3 pixel `pixel`.
    fn p3_pixel(&self, pixel: [u16; 3]) -> [u8; 3] {
        if pixel[0] == pixel[1] && pixel[1] == pixel[2] {
            let grey = rounded(pixel[0]);
            return [grey; 3];
        }
        let linear = pixel.map(|sample| self.decode[usize::from(sample)]);
        apply(&self.matrix, linear).map(|value| {
            let step = (value.clamp(0.0, 1.0) * 65_535.0).round();
            self.encode[step as usize]
        })
    }
}
