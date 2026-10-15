// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #410 item 1 (board #410, Alice 2026-10-15): what an `observe` or `do` answer says
//! about its frame.
//!
//! - `frame_coordinates` states the frame's coordinate system: device pixels, its width and
//!   height, its orientation, the origin at the top-left pixel, and the `do` arguments that take
//!   coordinates in this same system (`tap`, `tap_rect`, `swipe`).
//! - `--preview` writes a JPEG preview of the frame, its long edge at most 1024 pixels, into the
//!   Lab frame directory and answers its path, its size and its scale: a preview pixel times the
//!   scale is a frame pixel.
//! - After a performed `do`, the answer attaches its result: the after page's projection when
//!   the page is known, otherwise the after frame's path and its preview's path.
//!
//! No image is ever put into an answer, only paths. A part that could not be written is answered
//! as `{failure}` in its place (`lab_frame_export_failed`), and the answer still succeeds: with
//! `do` the action has been performed.

use super::*;
use actingcommand_artifact_store::read_projected_verified;
use actingcommand_contract::page_projection::PageProjection;
use actingcommand_contract::{LabOperationFrame, LabOperationRecord, PageObservationStatus};
use actingcommand_ledger::{MIN_PROJECTION_HARD_LIMIT_BYTES, MIN_PROJECTION_SOFT_LIMIT_BYTES};
use image::RgbImage;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::{self, FilterType};

/// The preview's longest edge in pixels; a frame no larger is previewed at its own size.
pub(super) const PREVIEW_LONG_EDGE: u32 = 1024;
/// The preview's JPEG quality (1..=100).
const PREVIEW_JPEG_QUALITY: u8 = 75;
/// The `do` arguments whose coordinates are in the frame's coordinate system.
const COORDINATE_INPUTS: [&str; 3] = ["tap", "tap_rect", "swipe"];
/// The Lab frame directory below actinglab's state root.
const FRAME_DIRECTORY: &str = "frames";

/// `frame_coordinates` of a `width` × `height` frame: device pixels, with the origin at the
/// top-left pixel; `landscape` when the frame is wider than high, `portrait` when higher than
/// wide, `square` otherwise; `applies_to` names the `do` arguments given in this system.
pub(super) fn coordinates(width: u32, height: u32) -> Value {
    let orientation = match width.cmp(&height) {
        std::cmp::Ordering::Greater => "landscape",
        std::cmp::Ordering::Less => "portrait",
        std::cmp::Ordering::Equal => "square",
    };
    json!({
        "unit": "device_pixel",
        "width": width,
        "height": height,
        "orientation": orientation,
        "origin": "top_left",
        "applies_to": COORDINATE_INPUTS,
    })
}

/// The preview's width and height for a `width` × `height` frame, and its scale (frame pixels
/// per preview pixel): a long edge over `PREVIEW_LONG_EDGE` becomes `PREVIEW_LONG_EDGE` and the
/// other edge keeps the aspect, rounded to the nearest pixel; a frame no larger keeps its size,
/// scale 1.
pub(super) fn preview_size(width: u32, height: u32) -> (u32, u32, f64) {
    let long = width.max(height);
    if long <= PREVIEW_LONG_EDGE {
        return (width, height, 1.0);
    }
    let edge = |pixels: u32| {
        let scaled = (u64::from(pixels) * u64::from(PREVIEW_LONG_EDGE) + u64::from(long) / 2)
            / u64::from(long);
        u32::try_from(scaled).unwrap_or(PREVIEW_LONG_EDGE).max(1)
    };
    (
        edge(width),
        edge(height),
        f64::from(long) / f64::from(PREVIEW_LONG_EDGE),
    )
}

/// Why a frame part was not written: the path it was being written to, when one was known, and
/// the operating system's or the decoder's text.
struct ExportFailure {
    path: Option<PathBuf>,
    raw_text: Option<String>,
}

impl ExportFailure {
    fn at(path: &Path, error: impl std::fmt::Display) -> Self {
        Self {
            path: Some(path.to_path_buf()),
            raw_text: Some(error.to_string()),
        }
    }

    fn text(error: impl std::fmt::Display) -> Self {
        Self {
            path: None,
            raw_text: Some(error.to_string()),
        }
    }

    /// `{failure: {code, path?, raw_text?}}`, answered in place of the part not written.
    fn answer(&self) -> Value {
        let mut failure = json!({
            "code": crate::codes::ActinglabCode::LabFrameExportFailed.as_str(),
        });
        if let Some(path) = &self.path {
            failure["path"] = json!(path.display().to_string());
        }
        if let Some(text) = &self.raw_text {
            failure["raw_text"] = json!(text);
        }
        json!({ "failure": failure })
    }
}

/// The Lab frame directory, `%LOCALAPPDATA%\ActingCommand\actinglab\frames`.
fn frame_directory() -> Result<PathBuf, ExportFailure> {
    app_state_root()
        .map(|root| root.join(FRAME_DIRECTORY))
        .map_err(|error| ExportFailure::text(error.message))
}

/// `bytes` written whole as `<directory>\<name>`, through a partial file of this process
/// renamed into place; frames and previews are named by the SHA-256 of the frame's PNG, so the
/// same frame is always the same file.
fn write_frame_file(directory: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, ExportFailure> {
    fs::create_dir_all(directory).map_err(|error| ExportFailure::at(directory, error))?;
    let path = directory.join(name);
    let partial = directory.join(format!("{name}.{}.part", std::process::id()));
    fs::write(&partial, bytes).map_err(|error| ExportFailure::at(&partial, error))?;
    fs::rename(&partial, &path).map_err(|error| ExportFailure::at(&path, error))?;
    Ok(path)
}

/// The JPEG preview of a decoded frame, written as `<directory>\<name>.preview.jpg`:
/// `{path, width, height, scale}`.
fn write_preview(directory: &Path, name: &str, scene: &Scene) -> Result<Value, ExportFailure> {
    let (width, height, scale) = preview_size(scene.width(), scene.height());
    let image = RgbImage::from_raw(scene.width(), scene.height(), scene.rgb8_pixels().to_vec())
        .ok_or(ExportFailure {
            path: None,
            raw_text: None,
        })?;
    let image = if image.dimensions() == (width, height) {
        image
    } else {
        imageops::resize(&image, width, height, FilterType::Triangle)
    };
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, PREVIEW_JPEG_QUALITY)
        .encode_image(&image)
        .map_err(ExportFailure::text)?;
    let path = write_frame_file(directory, &format!("{name}.preview.jpg"), &jpeg)?;
    Ok(json!({
        "path": path.display().to_string(),
        "width": width,
        "height": height,
        "scale": scale,
    }))
}

/// `preview` of a decoded frame: `{path, width, height, scale}`, or `{failure}`. The file is
/// named by the SHA-256 of the frame's PNG when the caller has it, else of its RGB8 pixels.
pub(super) fn scene_preview(scene: &Scene, png: Option<&[u8]>) -> Value {
    let name = hex_sha256(png.unwrap_or(scene.rgb8_pixels()));
    frame_directory()
        .and_then(|directory| write_preview(&directory, &name, scene))
        .unwrap_or_else(|failure| failure.answer())
}

/// `preview` of the frame whose PNG is `png`: `{path, width, height, scale}`, or `{failure}`.
pub(super) fn preview(png: &[u8]) -> Value {
    match Scene::from_png(png) {
        Ok(scene) => scene_preview(&scene, Some(png)),
        Err(error) => ExportFailure::text(error).answer(),
    }
}

/// The PNG of a Lab operation frame, read verified from the Runtime's artifact store.
fn operation_frame_png(frame: &LabOperationFrame) -> Result<Vec<u8>, ExportFailure> {
    let root = runtime_state_root().map_err(|error| ExportFailure::text(error.message))?;
    read_projected_verified(&root, frame.observation.artifact()).map_err(ExportFailure::text)
}

/// The after page's projection as the attachment answers it: under Min it is cut as `observe`
/// cuts its own projection, on the projection alone: entries from the end while it is over the
/// soft limit, keeping one actionable element; when that one is still over the hard limit, the
/// identity alone, marked truncated with its omitted counts. Normal and Debug answer it whole.
fn after_projection(mut projection: PageProjection, min: bool) -> CliOutcome<Value> {
    if min {
        loop {
            let bytes = serde_json::to_vec(&projection)
                .map_err(|error| CliError::device(error.to_string()))?
                .len();
            if bytes <= MIN_PROJECTION_SOFT_LIMIT_BYTES {
                break;
            }
            if projection.reduce_for_min()? {
                continue;
            }
            if bytes > MIN_PROJECTION_HARD_LIMIT_BYTES {
                projection.reduce_to_identity()?;
            }
            break;
        }
    }
    serde_json::to_value(&projection).map_err(|error| CliError::device(error.to_string()))
}

/// `after_attachment` of a performed `do`, or `None` when the operation has no after frame (it
/// ended before one was captured).
///
/// - The after page is known (its projection is recognized): `{kind: "page_projection",
///   observation}`, the after page's projection (see [`after_projection`]); with `--preview`
///   also `preview`, of the after frame.
/// - Otherwise (no match, a conflict, a partial evaluation or no projection): `{kind: "frame",
///   frame_coordinates, frame_path, preview}`, the after frame written as `<sha256>.png` and its
///   preview, whether `--preview` was given or not.
///
/// A part that could not be written is `{failure}` in its place.
pub(super) fn after_attachment(
    record: &LabOperationRecord,
    preview_requested: bool,
    min: bool,
) -> CliOutcome<Option<Value>> {
    let Some(frame) = &record.after_frame else {
        return Ok(None);
    };
    let known = record
        .after_projection
        .as_ref()
        .filter(|observed| observed.status == PageObservationStatus::Recognized);
    if let Some(observed) = known {
        let mut attachment = json!({
            "kind": "page_projection",
            "observation": after_projection(observed.projection.clone(), min)?,
        });
        if preview_requested {
            attachment["preview"] = match operation_frame_png(frame) {
                Ok(png) => preview(&png),
                Err(failure) => failure.answer(),
            };
        }
        return Ok(Some(attachment));
    }
    let mut attachment = json!({
        "kind": "frame",
        "frame_coordinates": coordinates(frame.observation.width(), frame.observation.height()),
    });
    match operation_frame_png(frame) {
        Ok(png) => {
            let name = format!("{}.png", hex_sha256(&png));
            attachment["frame_path"] = match frame_directory()
                .and_then(|directory| write_frame_file(&directory, &name, &png))
            {
                Ok(path) => json!(path.display().to_string()),
                Err(failure) => failure.answer(),
            };
            attachment["preview"] = preview(&png);
        }
        Err(failure) => {
            attachment["frame_path"] = failure.answer();
            attachment["preview"] = failure.answer();
        }
    }
    Ok(Some(attachment))
}

#[cfg(test)]
mod tests {
    // Workflow #410 item 1 (board #410, approved by Alice 2026-10-15): the coordinate statement
    // and the preview's size. The required values: the frame's device pixels W x H and its
    // orientation, with tap and tap_rect in the same system; a preview whose long edge is 1024
    // and whose scale maps a preview pixel back to frame pixels.

    use super::*;
    use image::ImageDecoder;
    use image::codecs::jpeg::JpegDecoder;
    use std::io::Cursor;

    #[test]
    fn labobs_the_coordinate_statement_names_device_pixels_the_orientation_and_the_inputs() {
        for (width, height, orientation) in [
            (2340, 1080, "landscape"),
            (1080, 2340, "portrait"),
            (720, 720, "square"),
        ] {
            let statement = coordinates(width, height);
            assert_eq!(statement["unit"], "device_pixel");
            assert_eq!(statement["width"], width);
            assert_eq!(statement["height"], height);
            assert_eq!(statement["orientation"], orientation, "{width}x{height}");
            assert_eq!(statement["origin"], "top_left");
            let inputs = statement["applies_to"]
                .as_array()
                .expect("applies_to is a list");
            for input in ["tap", "tap_rect"] {
                assert!(
                    inputs.iter().any(|named| named == input),
                    "{input} is given in the frame's coordinates: {statement}"
                );
            }
        }
    }

    #[test]
    fn labobs_the_preview_long_edge_is_1024_and_its_scale_maps_back_to_frame_pixels() {
        // A frame larger than 1024 on its long edge: the long edge becomes 1024, the aspect is
        // kept and preview × scale gives the frame's size again.
        for (width, height) in [(2340, 1080), (1080, 2340), (1920, 1080), (1280, 720)] {
            let (preview_width, preview_height, scale) = preview_size(width, height);
            assert_eq!(preview_width.max(preview_height), 1024, "{width}x{height}");
            assert!(
                (f64::from(preview_width) * scale - f64::from(width)).abs() <= scale,
                "{width}x{height}: width {preview_width} x {scale}"
            );
            assert!(
                (f64::from(preview_height) * scale - f64::from(height)).abs() <= scale,
                "{width}x{height}: height {preview_height} x {scale}"
            );
        }
        // A frame no larger is previewed at its own size.
        assert_eq!(preview_size(1024, 576), (1024, 576, 1.0));
        assert_eq!(preview_size(16, 9), (16, 9, 1.0));
    }

    #[test]
    fn labobs_the_preview_is_a_jpeg_of_the_answered_size_and_scale() {
        let directory = tempfile::TempDir::new().expect("a frame directory");
        let scene = Scene::from_rgb8(2340, 1080, &[40, 90, 160].repeat(2340 * 1080))
            .expect("a landscape frame");
        let answer = write_preview(directory.path(), "neutral-frame", &scene)
            .unwrap_or_else(|failure| panic!("{}", failure.answer()));
        let path = PathBuf::from(answer["path"].as_str().expect("the preview's path"));
        assert_eq!(path.parent(), Some(directory.path()));
        let bytes = fs::read(&path).expect("the preview file");
        let decoder = JpegDecoder::new(Cursor::new(bytes)).expect("the preview is a JPEG");
        assert_eq!(decoder.dimensions(), (1024, 473));
        assert_eq!(
            (answer["width"].clone(), answer["height"].clone()),
            (json!(1024), json!(473))
        );
        assert_eq!(answer["scale"], json!(2340.0 / 1024.0));
    }
}
