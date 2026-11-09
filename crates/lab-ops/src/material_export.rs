// SPDX-License-Identifier: AGPL-3.0-only

//! What the CLI and MCP share for a material written out whole (Workflow #389 MCP-5a; MCP-0
//! command table v1.1 §2.2, §4.5): the material ref, the export bound and the export writer.
//! MCP (`ac_material` mode export, `ac_observe`, Lab answers over budget) writes to the server's
//! fixed export directory; `actingctl observe --export <dir>` writes to the directory it is
//! given.

use crate::codes::ActingctlCode;
use actingcommand_contract::outcome::{IoOp, Outcome};
use actingcommand_contract::{ProjectedArtifactReference, ProjectedEvent};
use serde_json::{Value, json};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

/// The largest material an export writes (`material_too_large` above it).
pub(crate) const MAX_EXPORT_BYTES: usize = 32 * 1024 * 1024;

static EXPORT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// `<seq>:<event_id>:<artifact_id>`: enough to find the material again in the ledger, and what
/// `ac_material` takes as `ref`.
pub(crate) fn material_ref(
    event: &ProjectedEvent,
    artifact: &ProjectedArtifactReference,
) -> String {
    format!(
        "{}:{}:{}",
        event.sequence,
        text_of(json!(event.event_id)),
        text_of(json!(artifact.artifact_id))
    )
}

fn text_of(value: Value) -> String {
    match value {
        Value::String(text) => text,
        other => other.to_string(),
    }
}

/// An export that was not written: the file-system step that failed, on which path.
#[derive(Debug)]
pub(crate) struct WriteFailure {
    op: IoOp,
    path: PathBuf,
    /// The partial file left behind when moving it into place failed.
    partial: Option<PathBuf>,
    error: io::Error,
}

impl WriteFailure {
    /// `material_export_failed` with the path the step failed on (`path`; for the move into
    /// place, the export's own path, with the partial file left behind as `from_path`), caused by
    /// the OS failure (Workflow #378 A5).
    pub(crate) fn outcome(&self) -> Outcome {
        let outcome = Outcome::new(ActingctlCode::MaterialExportFailed).path(&self.path);
        let outcome = match &self.partial {
            Some(partial) => outcome.from_path(partial),
            None => outcome,
        };
        outcome.caused_by(Outcome::from_io_error(&self.error, self.op))
    }
}

/// The export writer: `bytes`, whose `sha256` (`sha256:<hex>`) the caller knows, to
/// `<directory>\<hex>.<extension>` through a partial file moved into place;
/// `{path, sha256, size}`.
pub(crate) fn write(
    directory: &Path,
    sha256: &str,
    extension: &str,
    bytes: &[u8],
) -> Result<Value, WriteFailure> {
    let failed = |op: IoOp, path: &Path, error: io::Error| WriteFailure {
        op,
        path: path.to_path_buf(),
        partial: None,
        error,
    };
    let digest = sha256.strip_prefix("sha256:").unwrap_or(sha256);
    fs::create_dir_all(directory).map_err(|error| failed(IoOp::CreateDir, directory, error))?;
    let path = directory.join(format!("{digest}.{extension}"));
    let partial = directory.join(format!(
        "{digest}.{extension}.{}-{}.partial",
        process::id(),
        EXPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&partial, bytes).map_err(|error| failed(IoOp::Write, &partial, error))?;
    if let Err(error) = fs::rename(&partial, &path) {
        return Err(WriteFailure {
            partial: Some(partial),
            ..failed(IoOp::Rename, &path, error)
        });
    }
    Ok(json!({
        "path": path.display().to_string(),
        "sha256": sha256,
        "size": bytes.len(),
    }))
}
