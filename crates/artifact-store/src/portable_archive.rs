// SPDX-License-Identifier: AGPL-3.0-only

//! Portable compatibility projection used while legacy Lab output consumers migrate to the
//! Runtime-owned evidence exporter.

use crate::{ArtifactStoreError, ArtifactStoreResult};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipWriter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableProjectionArchive {
    pub path: PathBuf,
    pub sha256: String,
}

pub fn write_portable_projection_archive(
    source_root: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
) -> ArtifactStoreResult<PortableProjectionArchive> {
    let source_root = source_root.as_ref();
    let output_path = output_path.as_ref();
    if let Err(error) = write_archive(source_root, output_path) {
        return Err(cleanup_partial_output(output_path, error));
    }
    let bytes = match fs::read(output_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Err(cleanup_partial_output(
                output_path,
                ArtifactStoreError::fatal(
                    "portable_projection_archive_failed",
                    "hash_portable_projection_archive",
                    error.to_string(),
                ),
            ));
        }
    };
    Ok(PortableProjectionArchive {
        path: output_path.to_path_buf(),
        sha256: hex_sha256(&bytes),
    })
}

fn write_archive(source_root: &Path, output_path: &Path) -> ArtifactStoreResult<()> {
    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .map_err(|error| archive_error("create_portable_projection_directory", error))?;
    }
    let file = File::create(output_path)
        .map_err(|error| archive_error("create_portable_projection_archive", error))?;
    let mut zip = ZipWriter::new(file);
    let options = FileOptions::default().compression_method(CompressionMethod::Deflated);
    for directory in ["logs", "screenshots"] {
        zip.add_directory(format!("{directory}/"), options)
            .map_err(|error| archive_error("add_portable_projection_directory", error))?;
        add_directory(&mut zip, source_root, &source_root.join(directory), options)?;
    }
    let file = zip
        .finish()
        .map_err(|error| archive_error("finish_portable_projection_archive", error))?;
    file.sync_all()
        .map_err(|error| archive_error("sync_portable_projection_archive", error))
}

fn add_directory(
    zip: &mut ZipWriter<File>,
    root: &Path,
    directory: &Path,
    options: FileOptions,
) -> ArtifactStoreResult<()> {
    if !directory.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(directory)
        .map_err(|error| archive_error("list_portable_projection_directory", error))?
    {
        let entry =
            entry.map_err(|error| archive_error("read_portable_projection_entry", error))?;
        let path = entry.path();
        if path.is_dir() {
            add_directory(zip, root, &path, options)?;
            continue;
        }
        let relative = path.strip_prefix(root).map_err(|error| {
            ArtifactStoreError::fatal(
                "portable_projection_path_invalid",
                "relativize_portable_projection_entry",
                error.to_string(),
            )
        })?;
        let name = archive_path(relative)?;
        zip.start_file(name, options)
            .map_err(|error| archive_error("start_portable_projection_entry", error))?;
        let bytes = fs::read(&path)
            .map_err(|error| archive_error("read_portable_projection_entry", error))?;
        zip.write_all(&bytes)
            .map_err(|error| archive_error("write_portable_projection_entry", error))?;
    }
    Ok(())
}

fn archive_path(path: &Path) -> ArtifactStoreResult<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => parts.push(value.to_string_lossy().to_string()),
            _ => {
                return Err(ArtifactStoreError::fatal(
                    "portable_projection_path_invalid",
                    "normalize_portable_projection_entry",
                    format!("invalid archive entry path {}", path.display()),
                ));
            }
        }
    }
    Ok(parts.join("/"))
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut value = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(value, "{byte:02x}").expect("writing to a String cannot fail");
    }
    value
}
fn cleanup_partial_output(path: &Path, mut error: ArtifactStoreError) -> ArtifactStoreError {
    match fs::remove_file(path) {
        Ok(()) => error,
        Err(cleanup) if cleanup.kind() == std::io::ErrorKind::NotFound => error,
        Err(cleanup) => {
            error = error.with_secondary(&ArtifactStoreError::fatal(
                "portable_projection_cleanup_failed",
                "cleanup_portable_projection_archive",
                cleanup.to_string(),
            ));
            error
        }
    }
}

fn archive_error(operation: &'static str, error: impl std::fmt::Display) -> ArtifactStoreError {
    ArtifactStoreError::fatal(
        "portable_projection_archive_failed",
        operation,
        error.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use tempfile::tempdir;
    use zip::ZipArchive;

    #[test]
    fn archive_contains_portable_logs_and_screenshots() {
        let temp = tempdir().expect("tempdir");
        let source = temp.path().join("projection");
        fs::create_dir_all(source.join("logs")).expect("logs");
        fs::create_dir_all(source.join("screenshots")).expect("screenshots");
        fs::write(source.join("logs/events.jsonl"), b"event\n").expect("event log");
        fs::write(source.join("screenshots/frame.png"), b"png").expect("frame");
        let output = temp.path().join("portable.zip");

        let receipt = write_portable_projection_archive(&source, &output).expect("archive");
        assert_eq!(receipt.path, output);
        assert_eq!(receipt.sha256.len(), 64);

        let file = File::open(&receipt.path).expect("open archive");
        let mut archive = ZipArchive::new(file).expect("read archive");
        let mut log = String::new();
        archive
            .by_name("logs/events.jsonl")
            .expect("events entry")
            .read_to_string(&mut log)
            .expect("read events");
        assert_eq!(log, "event\n");
        assert!(archive.by_name("screenshots/frame.png").is_ok());
    }

    #[test]
    fn archive_creation_failure_is_fatal_without_output() {
        let temp = tempdir().expect("tempdir");
        let blocked = temp.path().join("blocked");
        fs::write(&blocked, b"not a directory").expect("block parent");
        let output = blocked.join("portable.zip");

        let error = write_portable_projection_archive(temp.path(), &output)
            .expect_err("blocked parent must fail");
        assert!(error.is_fatal());
        assert_eq!(error.code(), "portable_projection_archive_failed");
        assert!(!output.exists());
    }
}
