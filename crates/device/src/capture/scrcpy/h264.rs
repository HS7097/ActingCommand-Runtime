// SPDX-License-Identifier: AGPL-3.0-only

//! #402 RD-3: H.264 decoding for the scrcpy stream. OpenH264 (the `openh264` crate, which builds
//! Cisco's OpenH264 from source) decodes every packet; the planes of a decoded picture are kept
//! and converted to RGB only when a capture takes the picture. The conversion uses the colour
//! matrix and range the stream's sequence parameter set declares. A stream that declares none
//! gets Android's default for its size (the codec's default colour aspects): BT.2020 from 4K,
//! BT.601 up to 720x576, BT.709 in between, limited range.

use super::{StreamDecoder, StreamEnd, StreamPicture};
use openh264::decoder::Decoder;
use openh264::formats::YUVSource;

const NAL_TYPE_SEQUENCE_PARAMETER_SET: u8 = 7;
const HIGH_PROFILES: [u32; 13] = [100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135];

/// The OpenH264 decoder of one stream.
pub(super) struct OpenH264Decoder {
    decoder: Decoder,
    signalled: Option<SignalledColour>,
}

impl OpenH264Decoder {
    pub(super) fn open() -> Result<Self, StreamEnd> {
        Decoder::new()
            .map(|decoder| Self {
                decoder,
                signalled: None,
            })
            .map_err(|_| StreamEnd::DecoderUnavailable)
    }
}

impl StreamDecoder for OpenH264Decoder {
    fn decode(&mut self, packet: &[u8]) -> Result<Option<StreamPicture>, StreamEnd> {
        for unit in nal_units(packet) {
            if unit
                .first()
                .is_some_and(|header| header & 0x1f == NAL_TYPE_SEQUENCE_PARAMETER_SET)
            {
                self.signalled = Some(sequence_colour(&unescape(&unit[1..]))?);
            }
        }
        let decoded = self
            .decoder
            .decode(packet)
            .map_err(|_| StreamEnd::DecodeFailed)?;
        let Some(yuv) = decoded else {
            return Ok(None);
        };
        let (width, height) = yuv.dimensions();
        let (chroma_width, chroma_height) = yuv.dimensions_uv();
        if width == 0
            || height == 0
            || chroma_width < width.div_ceil(2)
            || chroma_height < height.div_ceil(2)
        {
            return Err(StreamEnd::DecodeFailed);
        }
        let (luma_stride, blue_stride, red_stride) = yuv.strides();
        let luma = copy_plane(yuv.y(), luma_stride, width, height)?;
        let blue = copy_plane(yuv.u(), blue_stride, chroma_width, chroma_height)?;
        let red = copy_plane(yuv.v(), red_stride, chroma_width, chroma_height)?;
        let width = u32::try_from(width).map_err(|_| StreamEnd::DecodeFailed)?;
        let height = u32::try_from(height).map_err(|_| StreamEnd::DecodeFailed)?;
        let chroma_width = u32::try_from(chroma_width).map_err(|_| StreamEnd::DecodeFailed)?;
        let colour = resolve_colour(self.signalled, width, height)?;
        Ok(Some(StreamPicture::yuv420(Yuv420Picture {
            width,
            height,
            chroma_width,
            luma,
            blue,
            red,
            colour,
        })))
    }
}

fn copy_plane(
    plane: &[u8],
    stride: usize,
    width: usize,
    height: usize,
) -> Result<Vec<u8>, StreamEnd> {
    let mut copy = Vec::with_capacity(width * height);
    for row in 0..height {
        let start = row * stride;
        let line = plane
            .get(start..start + width)
            .ok_or(StreamEnd::DecodeFailed)?;
        copy.extend_from_slice(line);
    }
    Ok(copy)
}

/// A decoded 4:2:0 picture without stride padding.
pub(super) struct Yuv420Picture {
    width: u32,
    height: u32,
    chroma_width: u32,
    luma: Vec<u8>,
    blue: Vec<u8>,
    red: Vec<u8>,
    colour: StreamColour,
}

impl Yuv420Picture {
    pub(super) fn width(&self) -> u32 {
        self.width
    }

    pub(super) fn height(&self) -> u32 {
        self.height
    }

    /// Integer conversion (8 fractional bits) with the stream's matrix and range.
    pub(super) fn to_rgb8(&self) -> Vec<u8> {
        let coefficients = Coefficients::of(self.colour);
        let width = self.width as usize;
        let height = self.height as usize;
        let chroma_width = self.chroma_width as usize;
        let mut rgb = Vec::with_capacity(width * height * 3);
        for row in 0..height {
            let luma_row = &self.luma[row * width..(row + 1) * width];
            let chroma_row = (row / 2) * chroma_width;
            for (column, luma) in luma_row.iter().enumerate() {
                let chroma = chroma_row + column / 2;
                let luma = (i32::from(*luma) - coefficients.luma_offset) * coefficients.luma;
                let blue = i32::from(self.blue[chroma]) - 128;
                let red = i32::from(self.red[chroma]) - 128;
                rgb.push(channel(luma + coefficients.red_from_red * red));
                rgb.push(channel(
                    luma - coefficients.green_from_blue * blue - coefficients.green_from_red * red,
                ));
                rgb.push(channel(luma + coefficients.blue_from_blue * blue));
            }
        }
        rgb
    }
}

fn channel(scaled: i32) -> u8 {
    ((scaled + 128) >> 8).clamp(0, 255) as u8
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColourMatrix {
    Bt601,
    Bt709,
    Bt2020,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StreamColour {
    matrix: ColourMatrix,
    full_range: bool,
}

/// The YCbCr to RGB coefficients times 256; limited range includes the 255/219 and 255/224
/// scales.
struct Coefficients {
    luma_offset: i32,
    luma: i32,
    red_from_red: i32,
    green_from_blue: i32,
    green_from_red: i32,
    blue_from_blue: i32,
}

impl Coefficients {
    fn of(colour: StreamColour) -> Self {
        let (luma_offset, luma) = if colour.full_range {
            (0, 256)
        } else {
            (16, 298)
        };
        let (red_from_red, green_from_blue, green_from_red, blue_from_blue) =
            match (colour.matrix, colour.full_range) {
                (ColourMatrix::Bt601, false) => (409, 100, 208, 516),
                (ColourMatrix::Bt709, false) => (459, 55, 136, 541),
                (ColourMatrix::Bt2020, false) => (430, 48, 167, 548),
                (ColourMatrix::Bt601, true) => (359, 88, 183, 454),
                (ColourMatrix::Bt709, true) => (403, 48, 120, 475),
                (ColourMatrix::Bt2020, true) => (377, 42, 146, 482),
            };
        Self {
            luma_offset,
            luma,
            red_from_red,
            green_from_blue,
            green_from_red,
            blue_from_blue,
        }
    }
}

/// What a sequence parameter set declares about colour: the range, and the matrix when it
/// carries a colour description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SignalledColour {
    full_range: bool,
    matrix_coefficients: Option<u32>,
}

/// The declared matrix (H.264 Table E-5: 1 BT.709, 5 and 6 BT.601, 9 BT.2020; 2 unspecified), or
/// Android's default for the picture size. A matrix this conversion does not implement fails the
/// stream instead of being guessed.
fn resolve_colour(
    signalled: Option<SignalledColour>,
    width: u32,
    height: u32,
) -> Result<StreamColour, StreamEnd> {
    let full_range = signalled.is_some_and(|signalled| signalled.full_range);
    let matrix = match signalled.and_then(|signalled| signalled.matrix_coefficients) {
        None | Some(2) => default_matrix(width, height),
        Some(1) => ColourMatrix::Bt709,
        Some(5 | 6) => ColourMatrix::Bt601,
        Some(9) => ColourMatrix::Bt2020,
        Some(_) => return Err(StreamEnd::DecodeFailed),
    };
    Ok(StreamColour { matrix, full_range })
}

fn default_matrix(width: u32, height: u32) -> ColourMatrix {
    let (width, height) = (u64::from(width), u64::from(height));
    if width >= 3840 || height >= 3840 || width * height >= 3840 * 1634 {
        ColourMatrix::Bt2020
    } else if (width <= 720 && height <= 576) || (height <= 720 && width <= 576) {
        ColourMatrix::Bt601
    } else {
        ColourMatrix::Bt709
    }
}

/// The NAL units of an Annex B packet, without start codes and trailing zero bytes.
fn nal_units(packet: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut index = 0;
    while index + 3 <= packet.len() {
        if packet[index] == 0 && packet[index + 1] == 0 && packet[index + 2] == 1 {
            starts.push(index + 3);
            index += 3;
        } else {
            index += 1;
        }
    }
    let mut units = Vec::with_capacity(starts.len());
    for (position, start) in starts.iter().copied().enumerate() {
        let mut end = starts
            .get(position + 1)
            .map_or(packet.len(), |next| next - 3);
        while end > start && packet[end - 1] == 0 {
            end -= 1;
        }
        if end > start {
            units.push(&packet[start..end]);
        }
    }
    units
}

/// The raw byte sequence payload: emulation prevention bytes (`00 00 03`) removed.
fn unescape(escaped: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(escaped.len());
    let mut zeros = 0;
    for byte in escaped.iter().copied() {
        if zeros >= 2 && byte == 3 {
            zeros = 0;
            continue;
        }
        payload.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    payload
}

struct Bits<'a> {
    data: &'a [u8],
    position: usize,
}

impl Bits<'_> {
    fn bit(&mut self) -> Result<u32, StreamEnd> {
        let byte = *self
            .data
            .get(self.position / 8)
            .ok_or(StreamEnd::DecodeFailed)?;
        let bit = (byte >> (7 - self.position % 8)) & 1;
        self.position += 1;
        Ok(u32::from(bit))
    }

    fn bits(&mut self, count: u32) -> Result<u32, StreamEnd> {
        let mut value = 0;
        for _ in 0..count {
            value = (value << 1) | self.bit()?;
        }
        Ok(value)
    }

    fn flag(&mut self) -> Result<bool, StreamEnd> {
        Ok(self.bit()? == 1)
    }

    /// Unsigned Exp-Golomb.
    fn unsigned(&mut self) -> Result<u32, StreamEnd> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return Err(StreamEnd::DecodeFailed);
            }
        }
        let rest = u64::from(self.bits(zeros)?);
        u32::try_from((1u64 << zeros) - 1 + rest).map_err(|_| StreamEnd::DecodeFailed)
    }

    /// Signed Exp-Golomb.
    fn signed(&mut self) -> Result<i64, StreamEnd> {
        let code = i64::from(self.unsigned()?);
        Ok(if code % 2 == 1 {
            (code + 1) / 2
        } else {
            -(code / 2)
        })
    }
}

/// Reads a sequence parameter set (after its NAL header) up to the VUI colour description
/// (H.264 7.3.2.1.1 and E.1.1). Only 8-bit 4:2:0 streams are decoded.
fn sequence_colour(payload: &[u8]) -> Result<SignalledColour, StreamEnd> {
    let mut bits = Bits {
        data: payload,
        position: 0,
    };
    let profile = bits.bits(8)?;
    bits.bits(16)?; // constraint flags and level
    bits.unsigned()?; // seq_parameter_set_id
    if HIGH_PROFILES.contains(&profile) {
        if bits.unsigned()? != 1 {
            return Err(StreamEnd::DecodeFailed); // chroma_format_idc other than 4:2:0
        }
        if bits.unsigned()? != 0 || bits.unsigned()? != 0 {
            return Err(StreamEnd::DecodeFailed); // bit depth other than 8
        }
        bits.flag()?; // qpprime_y_zero_transform_bypass_flag
        if bits.flag()? {
            for list in 0..8 {
                if bits.flag()? {
                    skip_scaling_list(&mut bits, if list < 6 { 16 } else { 64 })?;
                }
            }
        }
    }
    bits.unsigned()?; // log2_max_frame_num_minus4
    match bits.unsigned()? {
        0 => {
            bits.unsigned()?; // log2_max_pic_order_cnt_lsb_minus4
        }
        1 => {
            bits.flag()?;
            bits.signed()?;
            bits.signed()?;
            for _ in 0..bits.unsigned()? {
                bits.signed()?;
            }
        }
        2 => {}
        _ => return Err(StreamEnd::DecodeFailed),
    }
    bits.unsigned()?; // max_num_ref_frames
    bits.flag()?; // gaps_in_frame_num_value_allowed_flag
    bits.unsigned()?; // pic_width_in_mbs_minus1
    bits.unsigned()?; // pic_height_in_map_units_minus1
    if !bits.flag()? {
        bits.flag()?; // mb_adaptive_frame_field_flag
    }
    bits.flag()?; // direct_8x8_inference_flag
    if bits.flag()? {
        for _ in 0..4 {
            bits.unsigned()?; // frame_crop_*_offset
        }
    }
    if !bits.flag()? {
        return Ok(SignalledColour {
            full_range: false,
            matrix_coefficients: None,
        });
    }
    if bits.flag()? && bits.bits(8)? == 255 {
        bits.bits(32)?; // sar_width, sar_height
    }
    if bits.flag()? {
        bits.flag()?; // overscan_appropriate_flag
    }
    if !bits.flag()? {
        return Ok(SignalledColour {
            full_range: false,
            matrix_coefficients: None,
        });
    }
    bits.bits(3)?; // video_format
    let full_range = bits.flag()?;
    let matrix_coefficients = if bits.flag()? {
        bits.bits(16)?; // colour_primaries, transfer_characteristics
        Some(bits.bits(8)?)
    } else {
        None
    };
    Ok(SignalledColour {
        full_range,
        matrix_coefficients,
    })
}

fn skip_scaling_list(bits: &mut Bits<'_>, size: usize) -> Result<(), StreamEnd> {
    let mut last = 8i64;
    let mut next = 8i64;
    for _ in 0..size {
        if next != 0 {
            next = (last + bits.signed()? + 256).rem_euclid(256);
        }
        if next != 0 {
            last = next;
        }
    }
    Ok(())
}
