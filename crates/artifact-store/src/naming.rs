// SPDX-License-Identifier: AGPL-3.0-only

use crate::{ArtifactStoreError, ArtifactStoreResult};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use time::format_description::FormatItem;
use time::macros::format_description;

const SCREENSHOT_TIMESTAMP: &[FormatItem<'static>] =
    format_description!("[year][month][day][hour][minute][second][subsecond digits:3]");
const MAX_COLLISION_SUFFIX: u16 = 9_999;

#[derive(Debug)]
pub struct ScreenshotNameAllocator {
    directory: PathBuf,
    reserved: BTreeSet<String>,
}

impl ScreenshotNameAllocator {
    pub fn new(directory: impl AsRef<Path>) -> ArtifactStoreResult<Self> {
        let directory = directory.as_ref().to_path_buf();
        std::fs::create_dir_all(&directory).map_err(|error| {
            ArtifactStoreError::fatal(
                "screenshot_directory_failed",
                "create_screenshot_directory",
                error.to_string(),
            )
        })?;
        Ok(Self {
            directory,
            reserved: BTreeSet::new(),
        })
    }

    pub fn allocate(&mut self, timestamp_unix_ms: u64) -> ArtifactStoreResult<String> {
        let base = screenshot_timestamp(timestamp_unix_ms)?;
        for suffix in 0..=MAX_COLLISION_SUFFIX {
            let candidate = if suffix == 0 {
                format!("{base}.png")
            } else {
                format!("{base}-{suffix:02}.png")
            };
            if self.reserved.contains(&candidate) || self.directory.join(&candidate).exists() {
                continue;
            }
            self.reserved.insert(candidate.clone());
            return Ok(candidate);
        }
        Err(ArtifactStoreError::fatal(
            "screenshot_name_exhausted",
            "allocate_screenshot_name",
            "all same-millisecond screenshot suffixes are occupied",
        ))
    }
}

fn screenshot_timestamp(timestamp_unix_ms: u64) -> ArtifactStoreResult<String> {
    if timestamp_unix_ms == 0 {
        return Err(ArtifactStoreError::fatal(
            "invalid_artifact_timestamp",
            "format_screenshot_name",
            "timestamp_unix_ms must be positive",
        ));
    }
    let nanos = i128::from(timestamp_unix_ms)
        .checked_mul(1_000_000)
        .ok_or_else(|| {
            ArtifactStoreError::fatal(
                "invalid_artifact_timestamp",
                "format_screenshot_name",
                "timestamp_unix_ms exceeds supported range",
            )
        })?;
    let timestamp = OffsetDateTime::from_unix_timestamp_nanos(nanos).map_err(|error| {
        ArtifactStoreError::fatal(
            "invalid_artifact_timestamp",
            "format_screenshot_name",
            error.to_string(),
        )
    })?;
    timestamp.format(SCREENSHOT_TIMESTAMP).map_err(|error| {
        ArtifactStoreError::fatal(
            "invalid_artifact_timestamp",
            "format_screenshot_name",
            error.to_string(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_millisecond_names_use_collision_suffixes_without_overwrite() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut names = ScreenshotNameAllocator::new(temp.path()).expect("allocator");
        let first = names.allocate(1_752_147_200_123).expect("first name");
        std::fs::write(temp.path().join(&first), b"occupied").expect("occupy first");
        let second = names.allocate(1_752_147_200_123).expect("second name");
        let third = names.allocate(1_752_147_200_123).expect("third name");

        assert_eq!(first, "20250710113320123.png");
        assert_eq!(second, "20250710113320123-01.png");
        assert_eq!(third, "20250710113320123-02.png");
        assert_eq!(
            std::fs::read(temp.path().join(first)).expect("original bytes"),
            b"occupied"
        );
    }

    #[test]
    fn zero_timestamp_fails_loudly() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut names = ScreenshotNameAllocator::new(temp.path()).expect("allocator");
        let error = names.allocate(0).expect_err("zero timestamp rejected");
        assert_eq!(error.code(), "invalid_artifact_timestamp");
        assert!(error.is_fatal());
    }
}
