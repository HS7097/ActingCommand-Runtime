// SPDX-License-Identifier: AGPL-3.0-only

//! The header of a picture of the screen as a device gave it (board #479, ACIB-1): its format
//! and its size, read from its first bytes by these few lines of this program's own. No image
//! library is used and nothing of the picture's content is read: only the signature and the
//! numbers the header gives its width and height in.
//!
//! The formats read are the two the screen capture service is documented to give, PNG and JPEG.
//! Bytes that end before their header gives the size are `picture_header_truncated`; bytes that
//! start with no header of the two are `picture_header_unknown`.

use crate::answer::{PictureFormat, Problem};

/// The eight bytes every PNG file starts with.
const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
/// The type of the chunk a PNG file's signature is followed by, which gives the size.
const PNG_HEADER_CHUNK: &[u8; 4] = b"IHDR";
/// Where the header chunk's type, its width and its height start.
const PNG_CHUNK_TYPE_AT: usize = 12;
const PNG_WIDTH_AT: usize = 16;
const PNG_HEIGHT_AT: usize = 20;
/// The marker every JPEG file starts with (start of image).
const JPEG_START: [u8; 2] = [0xFF, 0xD8];
/// The byte every JPEG marker starts with (one or more of them).
const JPEG_MARKER: u8 = 0xFF;
/// Within a frame header segment, after its two length bytes: the sample precision (one byte),
/// then the height and the width (two bytes each).
const JPEG_HEIGHT_IN_SEGMENT: usize = 3;
const JPEG_WIDTH_IN_SEGMENT: usize = 5;

/// What a picture's header says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub format: PictureFormat,
    pub width: u32,
    pub height: u32,
}

/// The header of `bytes`, or the word for why it cannot be read.
pub fn read_header(bytes: &[u8]) -> Result<Header, Problem> {
    if bytes.starts_with(&PNG_SIGNATURE) {
        png(bytes)
    } else if bytes.starts_with(&JPEG_START) {
        jpeg(bytes)
    } else if PNG_SIGNATURE.starts_with(bytes) || JPEG_START.starts_with(bytes) {
        Err(Problem::PictureHeaderTruncated)
    } else {
        Err(Problem::PictureHeaderUnknown)
    }
}

/// A PNG file: the signature, then the header chunk (its length, `IHDR`, the width, the height,
/// each number four bytes, most significant first).
fn png(bytes: &[u8]) -> Result<Header, Problem> {
    let chunk_type = bytes.get(PNG_CHUNK_TYPE_AT..PNG_CHUNK_TYPE_AT + PNG_HEADER_CHUNK.len());
    if chunk_type.is_some_and(|chunk_type| chunk_type != PNG_HEADER_CHUNK) {
        return Err(Problem::PictureHeaderUnknown);
    }
    let width = be_u32(bytes, PNG_WIDTH_AT).ok_or(Problem::PictureHeaderTruncated)?;
    let height = be_u32(bytes, PNG_HEIGHT_AT).ok_or(Problem::PictureHeaderTruncated)?;
    sized(PictureFormat::Png, width, height)
}

/// A JPEG file: after the start of image, segments, each a marker (one or more `0xFF`, then its
/// code) and, but for the few markers that stand alone, a length of two bytes that counts
/// itself; the first frame header segment gives the height and the width.
fn jpeg(bytes: &[u8]) -> Result<Header, Problem> {
    let mut at = JPEG_START.len();
    loop {
        match bytes.get(at) {
            Some(&JPEG_MARKER) => {}
            Some(_) => return Err(Problem::PictureHeaderUnknown),
            None => return Err(Problem::PictureHeaderTruncated),
        }
        while bytes.get(at) == Some(&JPEG_MARKER) {
            at += 1;
        }
        let code = *bytes.get(at).ok_or(Problem::PictureHeaderTruncated)?;
        at += 1;
        match code {
            // Markers that stand alone: TEM and the restart markers.
            0x01 | 0xD0..=0xD7 => continue,
            // A stuffed zero, a second start of image, the end of the image or the start of the
            // scan before any frame header: no header this program reads.
            0x00 | 0xD8 | 0xD9 | 0xDA => return Err(Problem::PictureHeaderUnknown),
            _ => {}
        }
        let length = usize::from(be_u16(bytes, at).ok_or(Problem::PictureHeaderTruncated)?);
        if length < 2 {
            return Err(Problem::PictureHeaderUnknown);
        }
        if is_frame_header(code) {
            let height = be_u16(bytes, at + JPEG_HEIGHT_IN_SEGMENT)
                .ok_or(Problem::PictureHeaderTruncated)?;
            let width =
                be_u16(bytes, at + JPEG_WIDTH_IN_SEGMENT).ok_or(Problem::PictureHeaderTruncated)?;
            return sized(PictureFormat::Jpeg, u32::from(width), u32::from(height));
        }
        at += length;
    }
}

/// The start of frame markers `0xC0` to `0xCF`, but for the three codes of that range that are
/// no frame header (`0xC4` Huffman tables, `0xC8` reserved, `0xCC` arithmetic coding).
fn is_frame_header(code: u8) -> bool {
    (0xC0..=0xCF).contains(&code) && code != 0xC4 && code != 0xC8 && code != 0xCC
}

/// A header with both numbers above zero; a zero is no size this program reads.
fn sized(format: PictureFormat, width: u32, height: u32) -> Result<Header, Problem> {
    if width == 0 || height == 0 {
        Err(Problem::PictureHeaderUnknown)
    } else {
        Ok(Header {
            format,
            width,
            height,
        })
    }
}

fn be_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let four: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    Some(u32::from_be_bytes(four))
}

fn be_u16(bytes: &[u8], at: usize) -> Option<u16> {
    let two: [u8; 2] = bytes.get(at..at + 2)?.try_into().ok()?;
    Some(u16::from_be_bytes(two))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A PNG file's first bytes: the signature and the header chunk of `width` by `height`,
    /// then `rest`.
    pub(crate) fn png_file(width: u32, height: u32, rest: &[u8]) -> Vec<u8> {
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        // Bit depth, colour type, compression, filter, interlace; then the chunk's CRC.
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.extend_from_slice(rest);
        bytes
    }

    /// A JPEG file's first bytes: the start of image, an APP0 segment, a fill byte, a
    /// Huffman table segment (`0xC4`, no frame header), a restart marker, then a frame header
    /// segment of `code` for `width` by `height`.
    fn jpeg_file(code: u8, width: u16, height: u16) -> Vec<u8> {
        let mut bytes = JPEG_START.to_vec();
        bytes.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]);
        bytes.extend_from_slice(b"JFIF\0");
        bytes.extend_from_slice(&[1, 1, 0, 0, 1, 0, 1, 0, 0]);
        bytes.extend_from_slice(&[0xFF, 0xFF, 0xC4, 0x00, 0x04, 0x00, 0x00]);
        bytes.extend_from_slice(&[0xFF, 0xD0]);
        bytes.extend_from_slice(&[0xFF, code, 0x00, 0x11, 8]);
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&[3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
        bytes
    }

    #[test]
    fn a_png_header_gives_its_format_and_its_size() {
        assert_eq!(
            read_header(&png_file(640, 1136, b"\0\0\0\0IDAT")),
            Ok(Header {
                format: PictureFormat::Png,
                width: 640,
                height: 1136,
            })
        );
        assert_eq!(
            read_header(&png_file(70_000, 1, &[])),
            Ok(Header {
                format: PictureFormat::Png,
                width: 70_000,
                height: 1,
            })
        );
    }

    #[test]
    fn a_jpeg_header_gives_its_format_and_its_size_after_other_segments() {
        for code in [0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC9, 0xCF] {
            assert_eq!(
                read_header(&jpeg_file(code, 1136, 640)),
                Ok(Header {
                    format: PictureFormat::Jpeg,
                    width: 1136,
                    height: 640,
                }),
                "frame header {code:#x}"
            );
        }
    }

    /// Bytes that end before the header gives the size, at every length up to it.
    #[test]
    fn a_header_that_ends_too_early_is_truncated() {
        let png = png_file(640, 1136, &[]);
        for length in 0..24 {
            assert_eq!(
                read_header(&png[..length]),
                Err(Problem::PictureHeaderTruncated),
                "PNG cut at {length}"
            );
        }
        let jpeg = jpeg_file(0xC0, 1136, 640);
        // The width is followed by the ten bytes of the three components: the header is read
        // whole once the bytes reach the width's last byte.
        let readable = jpeg.len() - 10;
        for length in 1..readable {
            assert_eq!(
                read_header(&jpeg[..length]),
                Err(Problem::PictureHeaderTruncated),
                "JPEG cut at {length}"
            );
        }
        assert_eq!(
            read_header(&jpeg[..readable]).map(|header| (header.width, header.height)),
            Ok((1136, 640))
        );
    }

    #[test]
    fn a_header_of_no_format_read_here_is_unknown() {
        let mut other_chunk = png_file(640, 1136, &[]);
        other_chunk[PNG_CHUNK_TYPE_AT..PNG_CHUNK_TYPE_AT + 4].copy_from_slice(b"tEXt");
        let mut no_marker = JPEG_START.to_vec();
        no_marker.extend_from_slice(&[0x12, 0x34, 0x56]);
        let mut scan_first = JPEG_START.to_vec();
        scan_first.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 1, 1, 0, 0, 0x3F, 0]);
        let mut end_first = JPEG_START.to_vec();
        end_first.extend_from_slice(&[0xFF, 0xD9]);
        let mut short_length = JPEG_START.to_vec();
        short_length.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x01, 0, 0]);
        let cases: [(&str, Vec<u8>); 9] = [
            ("GIF", b"GIF89a\x01\x00\x01\x00".to_vec()),
            ("TIFF", b"II*\0\x08\0\0\0".to_vec()),
            ("text", b"not a picture".to_vec()),
            ("PNG without its header chunk first", other_chunk),
            ("PNG of width 0", png_file(0, 1136, &[])),
            ("JPEG with no marker after the start", no_marker),
            ("JPEG whose scan starts before a frame header", scan_first),
            ("JPEG that ends before a frame header", end_first),
            ("JPEG segment shorter than its length", short_length),
        ];
        for (case, bytes) in cases {
            assert_eq!(
                read_header(&bytes),
                Err(Problem::PictureHeaderUnknown),
                "{case}"
            );
        }
        assert_eq!(
            read_header(&jpeg_file(0xC0, 1136, 0)),
            Err(Problem::PictureHeaderUnknown),
            "JPEG of height 0"
        );
    }
}
