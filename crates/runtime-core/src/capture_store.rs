// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{CaptureRef, Resolution};
use actingcommand_device::Frame;
use sha2::{Digest, Sha256};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::PathBuf;
use time::OffsetDateTime;
use time::error::Format as TimeFormatError;
use time::format_description::well_known::Rfc3339;
use time::macros::format_description;

const VALID_REASONS: [&str; 4] = ["manual", "task_result", "acquisition", "error"];

pub type CaptureStoreResult<T> = Result<T, CaptureStoreError>;

#[derive(Debug)]
pub enum CaptureStoreError {
    Io(std::io::Error),
    TimeFormat(TimeFormatError),
    InvalidReason(String),
    InvalidInput(String),
}

impl fmt::Display for CaptureStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "capture store I/O error: {err}"),
            Self::TimeFormat(err) => write!(f, "capture timestamp format error: {err}"),
            Self::InvalidReason(reason) => write!(f, "invalid capture reason: {reason}"),
            Self::InvalidInput(message) => write!(f, "invalid capture input: {message}"),
        }
    }
}

impl Error for CaptureStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::TimeFormat(err) => Some(err),
            Self::InvalidReason(_) | Self::InvalidInput(_) => None,
        }
    }
}

impl From<std::io::Error> for CaptureStoreError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<TimeFormatError> for CaptureStoreError {
    fn from(err: TimeFormatError) -> Self {
        Self::TimeFormat(err)
    }
}

/// Persists device frames and returns contract-level capture references.
pub struct CaptureStore {
    pub root_dir: PathBuf,
}

impl CaptureStore {
    pub fn new(root_dir: PathBuf) -> Self {
        Self { root_dir }
    }

    pub fn save_frame(
        &self,
        device_or_profile_id: &str,
        reason: &str,
        frame: &Frame,
    ) -> CaptureStoreResult<CaptureRef> {
        validate_reason(reason)?;
        validate_frame(frame)?;

        let now = OffsetDateTime::now_utc();
        let captured_at = now.format(&Rfc3339)?;
        let date = now.format(format_description!("[year]-[month]-[day]"))?;
        let stamp = now.format(format_description!(
            "[year][month][day]T[hour][minute][second][subsecond digits:9]Z"
        ))?;
        let hex = sha256_hex(&frame.png);
        let hash8 = &hex[..8];
        let image_hash = Some(format!("sha256:{hex}"));
        let safe_id = sanitize(device_or_profile_id);
        let safe_reason = sanitize(reason);
        let capture_id = format!("{stamp}-{safe_reason}-{hash8}");
        let rel = format!("captures/{safe_id}/{date}/{capture_id}.png");
        validate_relative_ref(&rel, &capture_id)?;

        let abs = self.root_dir.join(&rel);
        let parent = abs.parent().ok_or_else(|| {
            CaptureStoreError::InvalidInput(format!(
                "capture path has no parent: {}",
                abs.display()
            ))
        })?;
        fs::create_dir_all(parent)?;
        fs::write(&abs, &frame.png)?;

        Ok(CaptureRef {
            id: capture_id,
            image_ref: rel,
            image_hash,
            resolution: Resolution {
                width: i32::try_from(frame.width).map_err(|_| {
                    CaptureStoreError::InvalidInput(format!(
                        "frame width {} exceeds i32 range",
                        frame.width
                    ))
                })?,
                height: i32::try_from(frame.height).map_err(|_| {
                    CaptureStoreError::InvalidInput(format!(
                        "frame height {} exceeds i32 range",
                        frame.height
                    ))
                })?,
                scale: None,
                dpi: None,
            },
            captured_at,
        })
    }
}

fn validate_reason(reason: &str) -> CaptureStoreResult<()> {
    if VALID_REASONS.contains(&reason) {
        Ok(())
    } else {
        Err(CaptureStoreError::InvalidReason(reason.to_string()))
    }
}

fn validate_frame(frame: &Frame) -> CaptureStoreResult<()> {
    if frame.png.is_empty() {
        return Err(CaptureStoreError::InvalidInput(
            "frame PNG bytes are empty".to_string(),
        ));
    }
    if frame.width == 0 || frame.height == 0 {
        return Err(CaptureStoreError::InvalidInput(format!(
            "frame dimensions must be non-zero: {}x{}",
            frame.width, frame.height
        )));
    }
    Ok(())
}

fn validate_relative_ref(rel: &str, capture_id: &str) -> CaptureStoreResult<()> {
    if rel.contains('\\') || rel.contains(':') || rel.contains(' ') {
        return Err(CaptureStoreError::InvalidInput(format!(
            "capture image_ref is not filesystem-safe: {rel}"
        )));
    }
    if rel
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        return Err(CaptureStoreError::InvalidInput(format!(
            "capture image_ref contains a path-traversal segment: {rel}"
        )));
    }
    if capture_id.contains(':') {
        return Err(CaptureStoreError::InvalidInput(format!(
            "capture id is not filesystem-safe: {capture_id}"
        )));
    }
    Ok(())
}

fn sanitize(value: &str) -> String {
    let mut out = String::new();
    let mut previous_underscore = false;
    for byte in value.bytes() {
        let keep = byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-');
        if keep {
            out.push(byte as char);
            previous_underscore = false;
        } else if !previous_underscore {
            out.push('_');
            previous_underscore = true;
        }
    }

    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_device::{CaptureBackendName, PixelFormat};
    use std::fs;

    #[test]
    fn save_frame_writes_png_and_returns_capture_ref() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = CaptureStore::new(temp.path().to_path_buf());
        let frame = test_frame(b"png bytes".to_vec());

        let capture = store
            .save_frame("127.0.0.1:16384", "manual", &frame)
            .expect("capture saved");

        let saved = temp.path().join(&capture.image_ref);
        assert!(saved.exists());
        assert_eq!(fs::read(&saved).expect("saved bytes"), frame.png);
        assert!(capture.image_ref.contains("127.0.0.1_16384"));
        assert_ref_is_safe(&capture);
        assert_hash_shape(capture.image_hash.as_deref());
        assert_eq!(capture.resolution.width, frame.width as i32);
        assert_eq!(capture.resolution.height, frame.height as i32);
        assert_eq!(capture.resolution.scale, None);
        assert_eq!(capture.resolution.dpi, None);
        assert!(capture.captured_at.ends_with('Z'));
    }

    #[test]
    fn invalid_reason_returns_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = CaptureStore::new(temp.path().to_path_buf());
        let err = store
            .save_frame("profile", "manual now", &test_frame(b"png bytes".to_vec()))
            .expect_err("invalid reason");

        assert!(matches!(err, CaptureStoreError::InvalidReason(_)));
    }

    #[test]
    fn consecutive_saves_create_different_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = CaptureStore::new(temp.path().to_path_buf());

        let first = store
            .save_frame("profile", "manual", &test_frame(b"first".to_vec()))
            .expect("first save");
        let second = store
            .save_frame("profile", "manual", &test_frame(b"second".to_vec()))
            .expect("second save");

        assert_ne!(first.id, second.id);
        assert_ne!(first.image_ref, second.image_ref);
        assert!(temp.path().join(&first.image_ref).exists());
        assert!(temp.path().join(&second.image_ref).exists());
    }

    #[test]
    fn sanitize_empty_or_all_invalid_values_returns_unknown() {
        assert_eq!(sanitize(""), "unknown");
        assert_eq!(sanitize(":::// "), "unknown");
        assert_eq!(sanitize("."), "unknown");
        assert_eq!(sanitize(".."), "unknown");
    }

    #[test]
    fn sanitize_compresses_underscores_and_keeps_safe_ascii() {
        assert_eq!(sanitize("abc DEF:::ghi"), "abc_DEF_ghi");
        assert_eq!(sanitize("../profile"), ".._profile");
        assert_eq!(sanitize("Azur.JP_profile-01"), "Azur.JP_profile-01");
    }

    #[test]
    fn save_frame_with_path_traversal_profile_id_stays_under_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = CaptureStore::new(temp.path().to_path_buf());
        let frame = test_frame(b"png bytes".to_vec());

        let capture = store
            .save_frame("..", "manual", &frame)
            .expect("capture saved");

        assert!(!capture.image_ref.contains("captures/../"));
        assert!(!capture.image_ref.split('/').any(|segment| segment == ".."));
        assert!(capture.image_ref.starts_with("captures/unknown/"));

        let root = temp.path().canonicalize().expect("root canonicalized");
        let saved = temp.path().join(&capture.image_ref);
        let saved = saved.canonicalize().expect("saved capture canonicalized");
        assert!(saved.starts_with(&root));
    }

    #[test]
    fn validate_relative_ref_rejects_path_traversal_segments() {
        let err =
            validate_relative_ref("captures/../x/y.png", "y").expect_err("path traversal rejected");

        assert!(matches!(err, CaptureStoreError::InvalidInput(_)));
    }

    fn test_frame(png: Vec<u8>) -> Frame {
        let pixels = vec![0; 1280 * 720 * 3];
        let mut frame = Frame::from_pixels(
            1280,
            720,
            pixels,
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
        .expect("test frame");
        frame.png = png;
        frame
    }

    fn assert_ref_is_safe(capture: &CaptureRef) {
        assert!(!capture.image_ref.contains('\\'));
        assert!(!capture.image_ref.contains(':'));
        assert!(!capture.image_ref.contains(' '));
        assert!(!capture.id.contains(':'));
    }

    fn assert_hash_shape(hash: Option<&str>) {
        let hash = hash.expect("hash");
        let hex = hash.strip_prefix("sha256:").expect("sha256 prefix");
        assert_eq!(hex.len(), 64);
        assert!(hex.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert!(hex.chars().all(|ch| !ch.is_ascii_uppercase()));
    }
}
