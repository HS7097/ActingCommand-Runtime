// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #407 GPU-1 A: the pinned DirectML runtime of a vision root (design
//! `rd-1031/perf-407/design-gpu1.md` §5).
//!
//! A DirectML runtime is ONNX Runtime 1.24.4's DirectML build with DirectML 1.15.4, three files
//! under the vision root's `ort\`. The Runtime package and the Tools package do not carry them;
//! the Windows installation package does, by the owner's decision (Workflow #441, recorded in
//! the tool registry); whoever places them takes them from the two official NuGet packages
//! that `scripts/windows-tools/windows-tool-sources.v1.json` pins (`onnxruntime-directml-1.24.4`
//! and `directml-1.15.4`), and `Materialize-TaskToolCache.ps1 -Component …` reproduces them
//! from the official URLs. This table is a pin, not a setting: a logic test holds it equal to
//! those two entries, and the remote-build tool test freezes the entries.

use crate::models::lower_hex;
use crate::{DIRECTML_LIBRARY, ONNXRUNTIME_LIBRARY, ONNXRUNTIME_SHARED_PROVIDERS_LIBRARY};
use actingcommand_contract::outcome::{FileState, IoOp, Outcome};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{ErrorKind, Read};
use std::path::Path;

/// One pinned runtime file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinnedRuntimeFile {
    /// The file name under the vision root's `ort\`.
    pub file_name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    /// The `windows-tool-sources.v1.json` component whose archive holds the file.
    pub tool_source: &'static str,
    /// The file's path inside that archive.
    pub archive_path: &'static str,
}

/// The DirectML runtime: ONNX Runtime's DirectML build, its shared providers library, and
/// DirectML.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectmlRuntimePin {
    /// The version `OrtGetApiBase()->GetVersionString()` must report.
    pub onnxruntime_version: &'static str,
    pub directml_version: &'static str,
    pub onnxruntime: PinnedRuntimeFile,
    pub providers_shared: PinnedRuntimeFile,
    pub directml: PinnedRuntimeFile,
}

impl DirectmlRuntimePin {
    /// The three files, in closure order of a DirectML run.
    pub const fn files(&self) -> [&PinnedRuntimeFile; 3] {
        [&self.providers_shared, &self.directml, &self.onnxruntime]
    }
}

/// The component of `windows-tool-sources.v1.json` that holds ONNX Runtime's DirectML build.
pub const ONNXRUNTIME_DIRECTML_TOOL_SOURCE: &str = "onnxruntime-directml-1.24.4";
/// The component of `windows-tool-sources.v1.json` that holds DirectML.
pub const DIRECTML_TOOL_SOURCE: &str = "directml-1.15.4";

/// The pin (sizes and SHA-256 of the NuGet packages' files, read from the GPU-0 downloads the
/// design's §5 table records).
pub const DIRECTML_RUNTIME_PIN: DirectmlRuntimePin = DirectmlRuntimePin {
    onnxruntime_version: "1.24.4",
    directml_version: "1.15.4",
    onnxruntime: PinnedRuntimeFile {
        file_name: ONNXRUNTIME_LIBRARY,
        size: 17_328_152,
        sha256: "e7eedec6a6f26dc39dc948276a75ef6d2bee3fff944d874ceed0bbd3b97bff40",
        tool_source: ONNXRUNTIME_DIRECTML_TOOL_SOURCE,
        archive_path: "runtimes/win-x64/native/onnxruntime.dll",
    },
    providers_shared: PinnedRuntimeFile {
        file_name: ONNXRUNTIME_SHARED_PROVIDERS_LIBRARY,
        size: 22_040,
        sha256: "265c8daf29637cb259cac8be9f08f2cd45f3883f0f0e4949cbfddd5b4cbec3b6",
        tool_source: ONNXRUNTIME_DIRECTML_TOOL_SOURCE,
        archive_path: "runtimes/win-x64/native/onnxruntime_providers_shared.dll",
    },
    directml: PinnedRuntimeFile {
        file_name: DIRECTML_LIBRARY,
        size: 18_527_776,
        sha256: "9c9e6d822561c6c41b90e6994b3e8857cf1d66dbfb1e0c4c799c7c89b4e92da1",
        tool_source: DIRECTML_TOOL_SOURCE,
        archive_path: "bin/x64-win/DirectML.dll",
    },
};

/// How one file compares with its pin, with the OS failure behind an unreadable file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedFileCheck {
    /// `ok`, `missing`, `sha256_mismatch` (a size or digest that differs, or no regular file)
    /// or `unreadable`.
    pub state: FileState,
    pub os_cause: Option<Outcome>,
}

impl PinnedFileCheck {
    const fn of(state: FileState) -> Self {
        Self {
            state,
            os_cause: None,
        }
    }

    fn unreadable(error: &std::io::Error, op: IoOp) -> Self {
        Self {
            state: FileState::Unreadable,
            os_cause: Some(Outcome::from_io_error(error, op)),
        }
    }
}

impl PinnedRuntimeFile {
    /// Compares the file at `path` with this pin: its size first, then its SHA-256, read in
    /// bounded chunks.
    pub fn check(&self, path: &Path) -> PinnedFileCheck {
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return PinnedFileCheck::of(FileState::Missing);
            }
            Err(error) => return PinnedFileCheck::unreadable(&error, IoOp::Metadata),
        };
        if !metadata.is_file() || metadata.len() != self.size {
            return PinnedFileCheck::of(FileState::Sha256Mismatch);
        }
        let mut file = match fs::File::open(path) {
            Ok(file) => file,
            Err(error) => return PinnedFileCheck::unreadable(&error, IoOp::Open),
        };
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => hasher.update(&buffer[..read]),
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return PinnedFileCheck::unreadable(&error, IoOp::Read),
            }
        }
        if lower_hex(&hasher.finalize()) == self.sha256 {
            PinnedFileCheck::of(FileState::Ok)
        } else {
            PinnedFileCheck::of(FileState::Sha256Mismatch)
        }
    }
}
