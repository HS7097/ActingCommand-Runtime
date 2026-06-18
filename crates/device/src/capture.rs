// SPDX-License-Identifier: AGPL-3.0-only

use crate::adb::{Adb, AdbConfig};
use crate::{DeviceError, DeviceResult, DeviceTarget};
use std::time::Duration;

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const IHDR_LENGTH: [u8; 4] = [0, 0, 0, 13];
const DEFAULT_CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);

/// Single-shot screenshot boundary for device capture backends.
pub trait CaptureBackend {
    fn capture(&mut self) -> DeviceResult<Frame>;
}

/// Raw PNG frame returned by the device layer without pixel decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub png: Vec<u8>,
}

/// ADB `exec-out screencap -p` capture backend with no persistent session.
#[derive(Debug, Clone)]
pub struct ScreencapBackend {
    adb_config: AdbConfig,
    target: DeviceTarget,
    capture_timeout: Duration,
}

impl ScreencapBackend {
    pub fn new(adb_config: AdbConfig, target: DeviceTarget) -> Self {
        Self {
            adb_config,
            target,
            capture_timeout: DEFAULT_CAPTURE_TIMEOUT,
        }
    }

    pub fn with_capture_timeout(mut self, capture_timeout: Duration) -> Self {
        self.capture_timeout = capture_timeout;
        self
    }
}

impl CaptureBackend for ScreencapBackend {
    fn capture(&mut self) -> DeviceResult<Frame> {
        let serial = self.target.resolved_serial();
        let adb = Adb::new(self.adb_config.clone());
        if self.target.connect {
            adb.connect(&serial)?;
        }

        let state = adb.get_state(&serial)?;
        if state != "device" {
            return Err(DeviceError::fatal(format!(
                "target device {serial} is not in device state: {state:?}"
            )));
        }

        // `adb exec-out screencap -p` returns one binary PNG and has no long-lived session.
        let output = adb.screencap(&serial, self.capture_timeout)?;
        if output.stdout.is_empty() {
            return Err(DeviceError::fatal(
                "adb exec-out screencap -p returned empty stdout",
            ));
        }

        let (width, height) = parse_png_dimensions(&output.stdout)?;
        Ok(Frame {
            width,
            height,
            png: output.stdout,
        })
    }
}

pub fn parse_png_dimensions(png: &[u8]) -> DeviceResult<(u32, u32)> {
    if png.len() < 24 {
        return Err(DeviceError::fatal(format!(
            "screencap output is too short to be a PNG header: {} bytes",
            png.len()
        )));
    }
    if &png[0..8] != PNG_SIGNATURE {
        return Err(DeviceError::fatal(
            "screencap output does not start with a PNG signature",
        ));
    }
    if png[8..12] != IHDR_LENGTH {
        return Err(DeviceError::fatal(
            "screencap PNG has invalid IHDR chunk length",
        ));
    }
    if &png[12..16] != b"IHDR" {
        return Err(DeviceError::fatal("screencap PNG is missing IHDR"));
    }

    let width = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
    let height = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
    if width == 0 || height == 0 {
        return Err(DeviceError::fatal(format!(
            "screencap PNG has invalid dimensions: {width}x{height}"
        )));
    }

    Ok((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_png_dimensions_from_valid_header() {
        let png = png_header(1280, 720);
        assert_eq!(parse_png_dimensions(&png).expect("valid png"), (1280, 720));
    }

    #[test]
    fn rejects_empty_bytes() {
        assert_fatal(parse_png_dimensions(&[]));
    }

    #[test]
    fn rejects_non_png_signature() {
        let mut png = png_header(1280, 720);
        png[0] = 0;
        assert_fatal(parse_png_dimensions(&png));
    }

    #[test]
    fn rejects_missing_ihdr() {
        let mut png = png_header(1280, 720);
        png[12..16].copy_from_slice(b"TEXT");
        assert_fatal(parse_png_dimensions(&png));
    }

    #[test]
    fn rejects_invalid_ihdr_length() {
        let mut png = png_header(1280, 720);
        png[11] = 12;
        assert_fatal(parse_png_dimensions(&png));
    }

    #[test]
    fn rejects_zero_width() {
        let png = png_header(0, 720);
        assert_fatal(parse_png_dimensions(&png));
    }

    #[test]
    fn rejects_zero_height() {
        let png = png_header(1280, 0);
        assert_fatal(parse_png_dimensions(&png));
    }

    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut png = Vec::new();
        png.extend_from_slice(PNG_SIGNATURE);
        png.extend_from_slice(&IHDR_LENGTH);
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&width.to_be_bytes());
        png.extend_from_slice(&height.to_be_bytes());
        png
    }

    fn assert_fatal(result: DeviceResult<(u32, u32)>) {
        let err = result.expect_err("expected fatal device error");
        assert_eq!(err.severity(), crate::DeviceErrorSeverity::Fatal);
    }
}
