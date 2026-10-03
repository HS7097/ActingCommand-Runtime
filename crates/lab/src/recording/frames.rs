// SPDX-License-Identifier: AGPL-3.0-only

//! Content-addressed frame and crop storage: `lab/frames/<sha256>.png` and
//! `lab/crops/<sha256>.png`, read back only after the sha256 check.

use super::model::{RecordRect, RecordSize, RecordedFrame};
use super::store::{blocked, hex_sha256, invalid, read_verified, with_details};
use actingcommand_contract::LabResult;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_recognition::Scene;
use serde_json::json;
use std::path::{Path, PathBuf};

/// A decoded frame with its bytes; written to the store only when the command commits.
pub(crate) struct LoadedFrame {
    pub(crate) png: Vec<u8>,
    pub(crate) sha256: String,
    pub(crate) frame: Frame,
    pub(crate) scene: Scene,
}

impl LoadedFrame {
    pub(crate) fn size(&self) -> RecordSize {
        RecordSize {
            width: self.frame.width,
            height: self.frame.height,
        }
    }
}

pub(crate) fn frames_dir(lab_dir: &Path) -> PathBuf {
    lab_dir.join("frames")
}

pub(crate) fn crops_dir(lab_dir: &Path) -> PathBuf {
    lab_dir.join("crops")
}

pub(crate) fn frame_store_path(lab_dir: &Path, sha256: &str) -> PathBuf {
    frames_dir(lab_dir).join(format!("{sha256}.png"))
}

pub(crate) fn crop_store_path(lab_dir: &Path, sha256: &str) -> PathBuf {
    crops_dir(lab_dir).join(format!("{sha256}.png"))
}

/// Decodes PNG bytes as the `record` command does (`Frame::from_png`).
pub(crate) fn decode_frame(png: Vec<u8>, label: &str) -> LabResult<LoadedFrame> {
    let sha256 = hex_sha256(&png);
    let frame =
        Frame::from_png(png.clone(), CaptureBackendName::AdbScreencap).map_err(|error| {
            invalid(
                "record_frame_unreadable",
                format!("failed to decode {label} as PNG: {error}"),
            )
        })?;
    let scene = scene_of(&frame, label)?;
    Ok(LoadedFrame {
        png,
        sha256,
        frame,
        scene,
    })
}

fn scene_of(frame: &Frame, label: &str) -> LabResult<Scene> {
    let scene = match frame.pixel_format {
        PixelFormat::Rgba8 => Scene::from_rgba8(frame.width, frame.height, &frame.pixels),
        PixelFormat::Rgb8 => Scene::from_rgb8(frame.width, frame.height, &frame.pixels),
    };
    scene.map_err(|error| {
        invalid(
            "record_frame_unreadable",
            format!("failed to build a scene from {label}: {error}"),
        )
    })
}

/// Reads an offline frame file (`record mark --frame/--sample`).
pub(crate) fn read_frame_file(path: &str) -> LabResult<LoadedFrame> {
    let bytes = std::fs::read(path).map_err(|error| {
        with_details(
            invalid(
                "record_frame_unreadable",
                format!("failed to read frame {path}: {error}"),
            ),
            json!({"path": path}),
        )
    })?;
    decode_frame(bytes, path)
}

/// Reads a stored frame back after checking its sha256.
pub(crate) fn read_stored_frame(entry: &RecordedFrame) -> LabResult<LoadedFrame> {
    let bytes = read_verified(Path::new(&entry.path), &entry.sha256)?;
    decode_frame(bytes, &entry.path)
}

/// Every frame of a recording has one size: the package coordinate space and resolution.
pub(crate) fn check_frame_size(
    expected: Option<RecordSize>,
    frame: &LoadedFrame,
    label: &str,
) -> LabResult<()> {
    let actual = frame.size();
    match expected {
        Some(expected) if expected != actual => Err(with_details(
            blocked(
                "record_frame_size_mismatch",
                format!(
                    "{label} is {}x{}, the recording is {}x{}",
                    actual.width, actual.height, expected.width, expected.height
                ),
            ),
            json!({"frame": label, "expected": expected, "actual": actual}),
        )),
        _ => Ok(()),
    }
}

/// `region` lies inside the frame and has a positive size.
pub(crate) fn rect_inside(rect: RecordRect, size: RecordSize) -> bool {
    let (width, height) = (i64::from(size.width), i64::from(size.height));
    rect.x >= 0
        && rect.y >= 0
        && rect.width >= 1
        && rect.height >= 1
        && i64::from(rect.x) + i64::from(rect.width) <= width
        && i64::from(rect.y) + i64::from(rect.height) <= height
}

pub(crate) fn rect_contains(outer: RecordRect, inner: RecordRect) -> bool {
    i64::from(inner.x) >= i64::from(outer.x)
        && i64::from(inner.y) >= i64::from(outer.y)
        && i64::from(inner.x) + i64::from(inner.width)
            <= i64::from(outer.x) + i64::from(outer.width)
        && i64::from(inner.y) + i64::from(inner.height)
            <= i64::from(outer.y) + i64::from(outer.height)
}

/// Crops `rect` out of the frame and encodes it as a PNG template.
pub(crate) fn crop_png(frame: &LoadedFrame, rect: RecordRect) -> LabResult<Vec<u8>> {
    if !rect_inside(rect, frame.size()) {
        return Err(invalid(
            "validation_failed",
            format!(
                "crop {}x{} at {},{} is outside the frame",
                rect.width, rect.height, rect.x, rect.y
            ),
        ));
    }
    let source = &frame.frame;
    let stride = match source.pixel_format {
        PixelFormat::Rgb8 => 3usize,
        PixelFormat::Rgba8 => 4usize,
    };
    let frame_width = source.width as usize;
    let (x, y) = (rect.x as usize, rect.y as usize);
    let (width, height) = (rect.width as usize, rect.height as usize);
    let row_bytes = width * stride;
    let mut pixels = Vec::with_capacity(row_bytes * height);
    for row in 0..height {
        let offset = ((y + row) * frame_width + x) * stride;
        let end = offset + row_bytes;
        let slice = source.pixels.get(offset..end).ok_or_else(|| {
            invalid(
                "record_frame_unreadable",
                "frame pixel buffer is shorter than its size",
            )
        })?;
        pixels.extend_from_slice(slice);
    }
    let crop = Frame::from_pixels(
        rect.width as u32,
        rect.height as u32,
        pixels,
        source.pixel_format,
        CaptureBackendName::AdbScreencap,
    )
    .map_err(|error| {
        invalid(
            "record_frame_unreadable",
            format!("failed to crop: {error}"),
        )
    })?;
    crop.encode_png_fast().map_err(|error| {
        invalid(
            "record_frame_unreadable",
            format!("failed to encode crop: {error}"),
        )
    })
}
