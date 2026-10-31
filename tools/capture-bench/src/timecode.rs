// SPDX-License-Identifier: AGPL-3.0-only

//! #402 C2b: the test-pattern timecode of C3's protocol (`tools/capture-bench-apk/PROTOCOL.md`
//! on `exp/402-test-apk`, §4.2 layout and §4.3 timecode v1), decoded from a captured frame, and
//! drawn into a frame for the fake backend and the tests.
//!
//! The band is 24 columns × 8 rows of C × C cells at `[C, C, 25C, 9C)` in view pixels (frame
//! pixels = view pixels + the view offset INFO reports). Row 0 is the sync row `0xB38F0F`; rows
//! 1..7 carry a 21-byte codeword, most significant bit first: the 17-byte payload, then its
//! CRC-32 (IEEE, as `zlib.crc32`), big-endian. Each cell is judged by the mean BT.601 luma of its
//! sample square (the cell inset by `floor(C / 4)` on every side).

use serde::Serialize;

/// The sync row's 24 bits, most significant first at column 0.
pub const SYNC_BITS: u32 = 0x00B3_8F0F;
/// The protocol's default cell edge in view pixels.
pub const DEFAULT_CELL: u32 = 24;
pub const COLUMNS: u32 = 24;
pub const ROWS: u32 = 8;
const PAYLOAD_BYTES: usize = 17;
const CODEWORD_BYTES: usize = 21;
/// The smallest white-minus-black spread of the sync row that counts as a code (§4.3 step 2).
const MIN_SPREAD: f64 = 64.0;

/// Where the band sits in a frame: the cell edge and the view's offset in the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BandGeometry {
    pub cell: u32,
    pub offset_x: u32,
    pub offset_y: u32,
}

impl Default for BandGeometry {
    fn default() -> Self {
        Self {
            cell: DEFAULT_CELL,
            offset_x: 0,
            offset_y: 0,
        }
    }
}

impl BandGeometry {
    /// The frame rectangle `[left, top, right, bottom)` of code cell (row, column).
    pub fn cell_rect(&self, row: u32, column: u32) -> (u32, u32, u32, u32) {
        let c = self.cell;
        let left = self.offset_x + c + column * c;
        let top = self.offset_y + c + row * c;
        (left, top, left + c, top + c)
    }

    /// The sample square of code cell (row, column).
    pub fn sample_rect(&self, row: u32, column: u32) -> (u32, u32, u32, u32) {
        let (left, top, right, bottom) = self.cell_rect(row, column);
        let inset = self.cell / 4;
        (left + inset, top + inset, right - inset, bottom - inset)
    }

    /// Whether the band fits in a `width` × `height` frame.
    pub fn fits(&self, width: u32, height: u32) -> bool {
        let (_, _, right, bottom) = self.cell_rect(ROWS - 1, COLUMNS - 1);
        self.cell >= 4 && right <= width && bottom <= height
    }
}

/// How a decode ended (§4.3 step 4, plus a band that does not fit the frame).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecodeStatus {
    Ok,
    NoSync,
    CrcMismatch,
    UnsupportedVersion,
    BadMode,
    /// The band does not fit in the frame (wrong size or pixel layout).
    OutOfFrame,
}

impl DecodeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::NoSync => "no_sync",
            Self::CrcMismatch => "crc_mismatch",
            Self::UnsupportedVersion => "unsupported_version",
            Self::BadMode => "bad_mode",
            Self::OutOfFrame => "out_of_frame",
        }
    }
}

/// The payload's fields (§4.3 table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Payload {
    pub version: u8,
    /// 1 = pattern, 2 = flip, 3 = step.
    pub mode: u8,
    /// The frame counter mod 2^32.
    pub seq: u32,
    /// `T_vsync` in device nanoseconds.
    pub t_vsync_dev_ns: i64,
    /// flip: n mod 2^24; step: k mod 2^24; pattern: 0.
    pub event: u32,
    /// flip: j in bits 7..4, the level in bit 3; step: the level in bit 3.
    pub aux: u8,
}

impl Payload {
    pub fn mode_name(&self) -> &'static str {
        match self.mode {
            1 => "pattern",
            2 => "flip",
            3 => "step",
            _ => "unknown",
        }
    }

    /// The flip grid cell j (flip mode).
    pub fn flip_j(&self) -> u8 {
        self.aux >> 4
    }

    /// The flip or step level (bit 3 of `aux`).
    pub fn level(&self) -> u8 {
        (self.aux >> 3) & 1
    }

    pub fn bytes(&self) -> [u8; PAYLOAD_BYTES] {
        let mut bytes = [0u8; PAYLOAD_BYTES];
        bytes[0] = (self.version << 4) | (self.mode & 0x0f);
        bytes[1..5].copy_from_slice(&self.seq.to_be_bytes());
        bytes[5..13].copy_from_slice(&self.t_vsync_dev_ns.to_be_bytes());
        let event = self.event & 0x00ff_ffff;
        bytes[13] = (event >> 16) as u8;
        bytes[14] = (event >> 8) as u8;
        bytes[15] = event as u8;
        bytes[16] = self.aux;
        bytes
    }

    fn from_bytes(bytes: &[u8; PAYLOAD_BYTES]) -> Self {
        let mut seq = [0u8; 4];
        seq.copy_from_slice(&bytes[1..5]);
        let mut vsync = [0u8; 8];
        vsync.copy_from_slice(&bytes[5..13]);
        Self {
            version: bytes[0] >> 4,
            mode: bytes[0] & 0x0f,
            seq: u32::from_be_bytes(seq),
            t_vsync_dev_ns: i64::from_be_bytes(vsync),
            event: (u32::from(bytes[13]) << 16)
                | (u32::from(bytes[14]) << 8)
                | u32::from(bytes[15]),
            aux: bytes[16],
        }
    }
}

/// What a decode found.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Decoded {
    pub status: DecodeStatus,
    /// The fields; present only when `status` is `ok`.
    pub payload: Option<Payload>,
    /// The smallest `|mean - T|` over all cells (§4.3 step 5); `None` without a threshold.
    pub margin: Option<f64>,
    pub threshold: Option<f64>,
}

impl Decoded {
    fn failed(status: DecodeStatus, threshold: Option<f64>, margin: Option<f64>) -> Self {
        Self {
            status,
            payload: None,
            margin,
            threshold,
        }
    }

    pub fn is_ok(&self) -> bool {
        self.status == DecodeStatus::Ok
    }
}

/// CRC-32 (IEEE 802.3, reflected, as `zlib.crc32`).
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

/// The 21-byte codeword of a payload: the payload, then its CRC-32 big-endian.
pub fn codeword(payload: &Payload) -> [u8; CODEWORD_BYTES] {
    let bytes = payload.bytes();
    let mut word = [0u8; CODEWORD_BYTES];
    word[..PAYLOAD_BYTES].copy_from_slice(&bytes);
    word[PAYLOAD_BYTES..].copy_from_slice(&crc32(&bytes).to_be_bytes());
    word
}

/// The 8 × 24 cell values of a payload (row 0 the sync row), white = true.
pub fn cells(payload: &Payload) -> [[bool; COLUMNS as usize]; ROWS as usize] {
    let mut grid = [[false; COLUMNS as usize]; ROWS as usize];
    for (column, cell) in grid[0].iter_mut().enumerate() {
        *cell = (SYNC_BITS >> (COLUMNS as usize - 1 - column)) & 1 == 1;
    }
    let word = codeword(payload);
    for bit in 0..(CODEWORD_BYTES * 8) {
        let value = (word[bit / 8] >> (7 - bit % 8)) & 1 == 1;
        grid[1 + bit / COLUMNS as usize][bit % COLUMNS as usize] = value;
    }
    grid
}

fn luma(pixels: &[u8], index: usize) -> f64 {
    0.299 * f64::from(pixels[index])
        + 0.587 * f64::from(pixels[index + 1])
        + 0.114 * f64::from(pixels[index + 2])
}

/// The mean luma of `rect` in a frame of `width` pixels a row and `bytes_per_pixel` bytes a
/// pixel (3 for RGB8, 4 for RGBA8); `None` when the rectangle leaves the buffer.
fn mean_luma(
    pixels: &[u8],
    width: u32,
    bytes_per_pixel: usize,
    rect: (u32, u32, u32, u32),
) -> Option<f64> {
    let (left, top, right, bottom) = rect;
    if right <= left || bottom <= top {
        return None;
    }
    let stride = width as usize * bytes_per_pixel;
    let mut sum = 0.0;
    let mut count = 0usize;
    for y in top..bottom {
        let row = y as usize * stride;
        for x in left..right {
            let index = row + x as usize * bytes_per_pixel;
            if index + 2 >= pixels.len() {
                return None;
            }
            sum += luma(pixels, index);
            count += 1;
        }
    }
    if count > 0 {
        Some(sum / count as f64)
    } else {
        None
    }
}

/// Decodes the band of a `width` × `height` frame (§4.3 "Decoding").
pub fn decode(
    pixels: &[u8],
    width: u32,
    height: u32,
    bytes_per_pixel: usize,
    geometry: BandGeometry,
) -> Decoded {
    if !matches!(bytes_per_pixel, 3 | 4)
        || pixels.len() < width as usize * height as usize * bytes_per_pixel
        || !geometry.fits(width, height)
    {
        return Decoded::failed(DecodeStatus::OutOfFrame, None, None);
    }
    let mut means = [[0.0f64; COLUMNS as usize]; ROWS as usize];
    for (row, values) in means.iter_mut().enumerate() {
        for (column, value) in values.iter_mut().enumerate() {
            match mean_luma(
                pixels,
                width,
                bytes_per_pixel,
                geometry.sample_rect(row as u32, column as u32),
            ) {
                Some(mean) => *value = mean,
                None => return Decoded::failed(DecodeStatus::OutOfFrame, None, None),
            }
        }
    }
    // Step 2: the sync row's white and black means and the threshold.
    let (mut white, mut white_n, mut black, mut black_n) = (0.0, 0, 0.0, 0);
    for (column, mean) in means[0].iter().enumerate() {
        if (SYNC_BITS >> (COLUMNS as usize - 1 - column)) & 1 == 1 {
            white += mean;
            white_n += 1;
        } else {
            black += mean;
            black_n += 1;
        }
    }
    let white = white / f64::from(white_n);
    let black = black / f64::from(black_n);
    if white - black < MIN_SPREAD {
        return Decoded::failed(DecodeStatus::NoSync, None, None);
    }
    let threshold = (white + black) / 2.0;
    for (column, mean) in means[0].iter().enumerate() {
        let bit = (SYNC_BITS >> (COLUMNS as usize - 1 - column)) & 1 == 1;
        if bit != (*mean >= threshold) {
            return Decoded::failed(DecodeStatus::NoSync, Some(threshold), None);
        }
    }
    let margin = means
        .iter()
        .flat_map(|row| row.iter())
        .map(|mean| (mean - threshold).abs())
        .fold(f64::INFINITY, f64::min);
    // Step 3: the data cells.
    let mut word = [0u8; CODEWORD_BYTES];
    for bit in 0..(CODEWORD_BYTES * 8) {
        let mean = means[1 + bit / COLUMNS as usize][bit % COLUMNS as usize];
        if mean >= threshold {
            word[bit / 8] |= 1 << (7 - bit % 8);
        }
    }
    // Step 4.
    let mut payload_bytes = [0u8; PAYLOAD_BYTES];
    payload_bytes.copy_from_slice(&word[..PAYLOAD_BYTES]);
    let mut crc = [0u8; 4];
    crc.copy_from_slice(&word[PAYLOAD_BYTES..]);
    if crc32(&payload_bytes) != u32::from_be_bytes(crc) {
        return Decoded::failed(DecodeStatus::CrcMismatch, Some(threshold), Some(margin));
    }
    let payload = Payload::from_bytes(&payload_bytes);
    if payload.version != 1 {
        return Decoded::failed(
            DecodeStatus::UnsupportedVersion,
            Some(threshold),
            Some(margin),
        );
    }
    if !(1..=3).contains(&payload.mode) {
        return Decoded::failed(DecodeStatus::BadMode, Some(threshold), Some(margin));
    }
    Decoded {
        status: DecodeStatus::Ok,
        payload: Some(payload),
        margin: Some(margin),
        threshold: Some(threshold),
    }
}

/// Draws the band of `payload` into a `width` × `height` frame (white 255, black 0, the
/// alpha channel of RGBA8 left at 255). The fake backend's frames and the tests use it.
pub fn render(
    pixels: &mut [u8],
    width: u32,
    bytes_per_pixel: usize,
    geometry: BandGeometry,
    payload: &Payload,
) {
    let grid = cells(payload);
    let stride = width as usize * bytes_per_pixel;
    for (row, values) in grid.iter().enumerate() {
        for (column, white) in values.iter().enumerate() {
            let (left, top, right, bottom) = geometry.cell_rect(row as u32, column as u32);
            let value = if *white { 255 } else { 0 };
            for y in top..bottom {
                for x in left..right {
                    let index = y as usize * stride + x as usize * bytes_per_pixel;
                    if index + bytes_per_pixel <= pixels.len() {
                        pixels[index] = value;
                        pixels[index + 1] = value;
                        pixels[index + 2] = value;
                        if bytes_per_pixel == 4 {
                            pixels[index + 3] = 255;
                        }
                    }
                }
            }
        }
    }
}

/// The `content` block of an opportunity record (analysis schema §3.2): the decode's result and,
/// when it decoded, the frame counter, `T_vsync` and the event identity.
pub fn content_json(decoded: &Decoded) -> serde_json::Value {
    match decoded.payload {
        Some(payload) => serde_json::json!({
            "decoded": true,
            "status": decoded.status.as_str(),
            "code_mode": payload.mode_name(),
            "frame_counter": payload.seq,
            "t_vsync_dev_ns": payload.t_vsync_dev_ns,
            "t_vsync_next_dev_ns": null,
            "event": payload.event,
            "aux": payload.aux,
            "flip_j": (payload.mode == 2).then_some(payload.flip_j()),
            "level": payload.level(),
            "margin": decoded.margin,
        }),
        None => serde_json::json!({
            "decoded": false,
            "status": decoded.status.as_str(),
            "margin": decoded.margin,
        }),
    }
}
