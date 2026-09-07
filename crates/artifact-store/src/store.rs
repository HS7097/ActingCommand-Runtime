// SPDX-License-Identifier: AGPL-3.0-only

use crate::{ArtifactStoreError, ArtifactStoreResult};
use actingcommand_contract::{
    ArtifactIssuePolicy, ArtifactKind, ArtifactLinksDraft, ArtifactMaterial,
    ArtifactMaterialAccumulator, ArtifactPayloadDraft, ArtifactReference, ArtifactStoreIssuer,
    AuditInput, DiagnosticCode, EventActor, EventDraft, EventLinksDraft, EventOrigin,
    EventSeverity, EventSource, IdentifierIssuer, OriginModule, ProjectedArtifactReference,
    StoreIssuedArtifact, VerifiedArtifactReference,
};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(1);

pub trait ArtifactEventSink {
    fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()>;
}

#[derive(Debug, Clone)]
pub struct ArtifactWriteContext {
    artifact_links: ArtifactLinksDraft,
    event_links: EventLinksDraft,
    created_at_unix_ms: u64,
}

impl ArtifactWriteContext {
    pub fn new(
        artifact_links: ArtifactLinksDraft,
        event_links: EventLinksDraft,
        created_at_unix_ms: u64,
    ) -> Self {
        Self {
            artifact_links,
            event_links,
            created_at_unix_ms,
        }
    }

    pub fn event_links(&self) -> &EventLinksDraft {
        &self.event_links
    }

    pub const fn created_at_unix_ms(&self) -> u64 {
        self.created_at_unix_ms
    }
}

pub struct ArtifactWriteRequest<'a> {
    kind: actingcommand_contract::ArtifactKind,
    bytes: &'a [u8],
    context: ArtifactWriteContext,
    policy: ArtifactIssuePolicy,
}

impl<'a> ArtifactWriteRequest<'a> {
    pub fn new(
        kind: actingcommand_contract::ArtifactKind,
        bytes: &'a [u8],
        context: ArtifactWriteContext,
        policy: ArtifactIssuePolicy,
    ) -> Self {
        Self {
            kind,
            bytes,
            context,
            policy,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StoredArtifact {
    pub(crate) issued: StoreIssuedArtifact,
    path: PathBuf,
}

impl StoredArtifact {
    pub const fn reference(&self) -> &ArtifactReference {
        self.issued.reference()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Store-issued identity held before an artifact transaction is published.
pub struct PreparedArtifact {
    issued: StoreIssuedArtifact,
    path: PathBuf,
    context: ArtifactWriteContext,
}

impl PreparedArtifact {
    /// Returns the reserved reference used to preflight dependent records.
    pub const fn reference(&self) -> &ArtifactReference {
        self.issued.reference()
    }
}

/// An unpublished stream owned by one task. The owner must seal it or explicitly abort it.
/// Fatal process termination can leave a partial file, which is never a verified artifact.
#[must_use = "seal the artifact stream or abort it before normal task termination"]
pub struct ArtifactStream {
    root: PathBuf,
    temp_path: PathBuf,
    file: Option<File>,
    kind: ArtifactKind,
    context: ArtifactWriteContext,
    policy: ArtifactIssuePolicy,
    material: ArtifactMaterialAccumulator,
    failure: Option<ArtifactStoreError>,
}

impl ArtifactStream {
    pub fn append(&mut self, bytes: &[u8]) -> ArtifactStoreResult<()> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        self.write_all(bytes).map_err(|error| {
            self.failure.clone().unwrap_or_else(|| {
                ArtifactStoreError::fatal(
                    "artifact_write_failed",
                    "write_artifact_stream",
                    error.to_string(),
                )
            })
        })
    }

    /// Abandons unpublished bytes. Cleanup errors propagate to the task's existing fatal path.
    pub fn abort(mut self) -> ArtifactStoreResult<()> {
        self.file.take();
        let removal = fs::remove_file(&self.temp_path);
        match removal {
            Ok(()) => self.failure.map_or(Ok(()), Err),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.failure.map_or(Ok(()), Err)
            }
            Err(error) => {
                let cleanup = ArtifactStoreError::fatal(
                    "artifact_cleanup_failed",
                    "cleanup_artifact_temp",
                    error.to_string(),
                );
                Err(self
                    .failure
                    .map_or_else(|| cleanup.clone(), |error| error.with_secondary(&cleanup)))
            }
        }
    }

    fn fail(&mut self, error: ArtifactStoreError) -> ArtifactStoreError {
        self.file.take();
        let error = cleanup_temp(&self.temp_path, error);
        self.failure = Some(error.clone());
        error
    }
}

impl Write for ArtifactStream {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if let Some(error) = &self.failure {
            return Err(std::io::Error::other(error.clone()));
        }
        let result = self
            .file
            .as_mut()
            .ok_or_else(|| std::io::Error::other("artifact stream is closed"))
            .and_then(|file| file.write(bytes))
            .and_then(|count| {
                if count == 0 && !bytes.is_empty() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "artifact stream write returned zero",
                    ));
                }
                self.material.update(&bytes[..count])?;
                Ok(count)
            });
        result.map_err(|error| {
            std::io::Error::other(self.fail(ArtifactStoreError::fatal(
                "artifact_write_failed",
                "write_artifact_stream",
                error.to_string(),
            )))
        })
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(error) = &self.failure {
            return Err(std::io::Error::other(error.clone()));
        }
        // File writes have no userspace buffer; durability is required by seal_stream.
        Ok(())
    }
}

pub struct ArtifactStore {
    root: PathBuf,
    artifacts: ArtifactStoreIssuer,
    events: IdentifierIssuer,
    writer: Mutex<()>,
}

impl ArtifactStore {
    pub fn open(root: impl AsRef<Path>) -> ArtifactStoreResult<Self> {
        fs::create_dir_all(root.as_ref()).map_err(|error| {
            ArtifactStoreError::fatal(
                "artifact_root_failed",
                "open_artifact_store",
                error.to_string(),
            )
        })?;
        let root = root.as_ref().canonicalize().map_err(|error| {
            ArtifactStoreError::fatal(
                "artifact_root_failed",
                "canonicalize_artifact_root",
                error.to_string(),
            )
        })?;
        Ok(Self {
            root,
            artifacts: ArtifactStoreIssuer::new().map_err(|error| {
                ArtifactStoreError::fatal(
                    "artifact_issuer_failed",
                    "open_artifact_store",
                    error.to_string(),
                )
            })?,
            events: IdentifierIssuer::new().map_err(|error| {
                ArtifactStoreError::fatal(
                    "event_issuer_failed",
                    "open_artifact_store",
                    error.to_string(),
                )
            })?,
            writer: Mutex::new(()),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Opens a task-owned staging stream without publishing an identity or ledger event.
    pub fn begin_stream(
        &self,
        kind: ArtifactKind,
        context: ArtifactWriteContext,
        policy: ArtifactIssuePolicy,
    ) -> ArtifactStoreResult<ArtifactStream> {
        if context.created_at_unix_ms == 0 {
            return Err(ArtifactStoreError::fatal(
                "artifact_issue_failed",
                "begin_artifact_stream",
                "artifact timestamp must be positive",
            ));
        }
        let temp_path = temporary_path(&self.root.join("artifact-stream"))?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|error| {
                ArtifactStoreError::fatal(
                    "artifact_write_failed",
                    "create_artifact_temp",
                    error.to_string(),
                )
            })?;
        Ok(ArtifactStream {
            root: self.root.clone(),
            temp_path,
            file: Some(file),
            kind,
            context,
            policy,
            material: ArtifactMaterialAccumulator::default(),
            failure: None,
        })
    }

    /// Syncs and re-reads actual staged bytes, then publishes once through created → verified.
    pub fn seal_stream(
        &self,
        mut stream: ArtifactStream,
        sink: &mut dyn ArtifactEventSink,
    ) -> ArtifactStoreResult<StoredArtifact> {
        if let Some(error) = stream.failure.clone() {
            return Err(stream.fail(error));
        }
        if stream.root != self.root {
            return Err(stream.fail(ArtifactStoreError::fatal(
                "artifact_path_invalid",
                "seal_artifact_stream",
                "stream belongs to another artifact root",
            )));
        }
        let _writer = self.writer.lock().map_err(|_| {
            stream.fail(ArtifactStoreError::fatal(
                "artifact_writer_poisoned",
                "store_artifact",
                "artifact writer lock is poisoned",
            ))
        })?;
        let material = (|| {
            let file = stream.file.as_mut().ok_or_else(|| {
                ArtifactStoreError::fatal(
                    "artifact_write_failed",
                    "seal_artifact_stream",
                    "artifact stream is closed",
                )
            })?;
            file.sync_all().map_err(|error| {
                ArtifactStoreError::fatal(
                    "artifact_sync_failed",
                    "sync_artifact_temp",
                    error.to_string(),
                )
            })?;
            file.rewind().map_err(|error| {
                ArtifactStoreError::fatal(
                    "artifact_verify_failed",
                    "rewind_artifact_temp",
                    error.to_string(),
                )
            })?;
            ArtifactMaterial::read_from(file).map_err(|error| {
                ArtifactStoreError::fatal(
                    "artifact_verify_failed",
                    "read_artifact_for_verification",
                    error.to_string(),
                )
            })
        })()
        .map_err(|error| stream.fail(error))?;
        let written = std::mem::take(&mut stream.material).finish();
        if material.byte_count() != written.byte_count() || material.sha256() != written.sha256() {
            return Err(stream.fail(ArtifactStoreError::fatal(
                "artifact_hash_mismatch",
                "seal_artifact_stream",
                "staged bytes do not match the written stream",
            )));
        }
        let issued = self
            .artifacts
            .issue(
                stream.kind,
                stream.context.artifact_links.clone(),
                material,
                stream.context.created_at_unix_ms,
                stream.policy,
            )
            .map_err(|error| {
                stream.fail(ArtifactStoreError::fatal(
                    "artifact_issue_failed",
                    "seal_artifact_stream",
                    error.to_string(),
                ))
            })?;
        let path = safe_object_path(&self.root, issued.reference().object_key())
            .map_err(|error| stream.fail(error))?;
        stream.file.take();
        let publication = (|| {
            let parent = path.parent().ok_or_else(|| {
                ArtifactStoreError::fatal(
                    "artifact_path_invalid",
                    "store_artifact",
                    "artifact object has no parent directory",
                )
            })?;
            fs::create_dir_all(parent).map_err(|error| {
                ArtifactStoreError::fatal(
                    "artifact_directory_failed",
                    "store_artifact",
                    error.to_string(),
                )
            })?;
            publish_temp(&stream.temp_path, &path)
        })();
        if let Err(error) = publication {
            let error = stream.fail(error);
            return Err(self.report_failure(
                error,
                sink,
                &stream.context,
                &issued,
                ArtifactPayloadDraft::store_failed(
                    DiagnosticCode::ArtifactWriteFailed,
                    AuditInput::new(),
                ),
            ));
        }
        self.finish_publication(
            PreparedArtifact {
                issued,
                path,
                context: stream.context,
            },
            sink,
        )
    }

    pub fn put(
        &self,
        request: ArtifactWriteRequest<'_>,
        sink: &mut dyn ArtifactEventSink,
    ) -> ArtifactStoreResult<StoredArtifact> {
        let bytes = request.bytes;
        let prepared = self.prepare(request)?;
        self.commit_prepared(prepared, bytes, sink)
    }

    /// Reserves store-issued artifact identity without publishing bytes or ledger events.
    ///
    /// Callers may use the projected reference to validate a larger transaction, then either
    /// abandon the preparation or commit it exactly once through `commit_prepared`.
    pub fn prepare(
        &self,
        request: ArtifactWriteRequest<'_>,
    ) -> ArtifactStoreResult<PreparedArtifact> {
        let issued = self
            .artifacts
            .issue(
                request.kind,
                request.context.artifact_links.clone(),
                request.bytes,
                request.context.created_at_unix_ms,
                request.policy,
            )
            .map_err(|error| {
                ArtifactStoreError::fatal(
                    "artifact_issue_failed",
                    "store_artifact",
                    error.to_string(),
                )
            })?;
        let path = safe_object_path(&self.root, issued.reference().object_key())?;
        Ok(PreparedArtifact {
            issued,
            path,
            context: request.context,
        })
    }

    /// Publishes bytes and ledger events for one previously prepared artifact.
    pub fn commit_prepared(
        &self,
        prepared: PreparedArtifact,
        bytes: &[u8],
        sink: &mut dyn ArtifactEventSink,
    ) -> ArtifactStoreResult<StoredArtifact> {
        let _writer = self.writer.lock().map_err(|_| {
            ArtifactStoreError::fatal(
                "artifact_writer_poisoned",
                "store_artifact",
                "artifact writer lock is poisoned",
            )
        })?;
        verify_bytes(bytes, prepared.issued.reference())?;

        let result = self.write_and_verify(bytes, &prepared.path, prepared.issued.reference());
        if let Err(error) = result {
            return Err(self.report_failure(
                error,
                sink,
                &prepared.context,
                &prepared.issued,
                ArtifactPayloadDraft::store_failed(
                    DiagnosticCode::ArtifactWriteFailed,
                    AuditInput::new(),
                ),
            ));
        }

        self.finish_publication(prepared, sink)
    }

    fn finish_publication(
        &self,
        prepared: PreparedArtifact,
        sink: &mut dyn ArtifactEventSink,
    ) -> ArtifactStoreResult<StoredArtifact> {
        if let Err(error) = self.append_event(
            sink,
            &prepared.context,
            EventSeverity::Info,
            ArtifactPayloadDraft::created(AuditInput::new()),
            prepared.issued.clone(),
        ) {
            return Err(cleanup_published(&prepared.path, error));
        }

        if let Err(error) = verify_file(&prepared.path, prepared.issued.reference()) {
            let error = cleanup_published(&prepared.path, error);
            return Err(self.report_failure(
                error,
                sink,
                &prepared.context,
                &prepared.issued,
                ArtifactPayloadDraft::verification_failed(
                    DiagnosticCode::ArtifactVerifyFailed,
                    AuditInput::new(),
                ),
            ));
        }

        if let Err(error) = self.append_event(
            sink,
            &prepared.context,
            EventSeverity::Info,
            ArtifactPayloadDraft::verified(AuditInput::new()),
            prepared.issued.clone(),
        ) {
            return Err(cleanup_published(&prepared.path, error));
        }

        Ok(StoredArtifact {
            issued: prepared.issued,
            path: prepared.path,
        })
    }

    pub fn read_verified(&self, reference: &ArtifactReference) -> ArtifactStoreResult<Vec<u8>> {
        reference.validate().map_err(|error| {
            ArtifactStoreError::fatal(
                "artifact_reference_invalid",
                "read_artifact",
                error.to_string(),
            )
        })?;
        let path = safe_object_path(&self.root, reference.object_key())?;
        verify_file(&path, reference)?;
        fs::read(path).map_err(|error| {
            ArtifactStoreError::fatal("artifact_read_failed", "read_artifact", error.to_string())
        })
    }

    pub fn verify_recovery_reference(
        &self,
        projected: &ProjectedArtifactReference,
    ) -> ArtifactStoreResult<VerifiedArtifactReference> {
        projected.validate().map_err(|error| {
            ArtifactStoreError::fatal(
                "artifact_reference_invalid",
                "verify_recovery_artifact",
                error.to_string(),
            )
        })?;
        let object_key = projected.object_key().ok_or_else(|| {
            ArtifactStoreError::fatal(
                "artifact_object_key_missing",
                "verify_recovery_artifact",
                "persisted artifact reference has no object key",
            )
        })?;
        let path = safe_object_path(&self.root, object_key)?;
        let mut file = File::open(path).map_err(|error| {
            ArtifactStoreError::fatal(
                "artifact_read_failed",
                "verify_recovery_artifact",
                error.to_string(),
            )
        })?;
        let material = ArtifactMaterial::read_from(&mut file).map_err(|error| {
            ArtifactStoreError::fatal(
                "artifact_read_failed",
                "verify_recovery_artifact",
                error.to_string(),
            )
        })?;
        self.artifacts
            .verify_existing_material(projected.clone(), material)
            .map_err(|error| {
                ArtifactStoreError::fatal(
                    "artifact_verify_failed",
                    "verify_recovery_artifact",
                    error.to_string(),
                )
            })
    }

    #[cfg(feature = "capture")]
    pub(crate) fn rollback_stored(
        &self,
        stored: &StoredArtifact,
        error: ArtifactStoreError,
    ) -> ArtifactStoreError {
        cleanup_published(stored.path(), error)
    }

    fn write_and_verify(
        &self,
        bytes: &[u8],
        final_path: &Path,
        reference: &ArtifactReference,
    ) -> ArtifactStoreResult<()> {
        let parent = final_path.parent().ok_or_else(|| {
            ArtifactStoreError::fatal(
                "artifact_path_invalid",
                "store_artifact",
                "artifact object has no parent directory",
            )
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            ArtifactStoreError::fatal(
                "artifact_directory_failed",
                "store_artifact",
                error.to_string(),
            )
        })?;
        if final_path.exists() {
            return Err(ArtifactStoreError::fatal(
                "artifact_collision",
                "store_artifact",
                "artifact object key already exists",
            ));
        }

        let temp_path = temporary_path(final_path)?;
        let write_result = write_synced(&temp_path, bytes)
            .and_then(|()| verify_file(&temp_path, reference))
            .and_then(|()| publish_temp(&temp_path, final_path));
        if let Err(error) = write_result {
            return Err(cleanup_temp(&temp_path, error));
        }
        Ok(())
    }

    fn append_event(
        &self,
        sink: &mut dyn ArtifactEventSink,
        context: &ArtifactWriteContext,
        severity: EventSeverity,
        payload: ArtifactPayloadDraft,
        artifact: StoreIssuedArtifact,
    ) -> ArtifactStoreResult<()> {
        let draft = EventDraft::new(
            self.events.mint_event_id().map_err(|error| {
                ArtifactStoreError::fatal(
                    "event_issuer_failed",
                    "append_artifact_event",
                    error.to_string(),
                )
            })?,
            context.created_at_unix_ms,
            severity,
            EventOrigin::new(
                EventSource::System,
                OriginModule::ArtifactStore,
                EventActor::System,
            ),
            context.event_links.clone(),
            payload.into(),
        )
        .with_artifacts(vec![artifact]);
        sink.append(draft)
    }

    fn report_failure(
        &self,
        error: ArtifactStoreError,
        sink: &mut dyn ArtifactEventSink,
        context: &ArtifactWriteContext,
        issued: &StoreIssuedArtifact,
        payload: ArtifactPayloadDraft,
    ) -> ArtifactStoreError {
        match self.append_event(sink, context, EventSeverity::Error, payload, issued.clone()) {
            Ok(()) => error,
            Err(event_error) => error.with_secondary(&event_error),
        }
    }
}

/// A bounded reader whose bytes remain provisional until `finish` verifies the entire object.
/// Consumers must obtain their reference from the ledger and enforce its privacy policy.
#[must_use = "finish the reader before treating any streamed bytes as verified"]
pub struct ArtifactReader {
    file: File,
    reference: ProjectedArtifactReference,
    material: ArtifactMaterialAccumulator,
    verified: Option<VerifiedArtifactReference>,
    failure: Option<ArtifactStoreError>,
}

impl ArtifactReader {
    pub fn read_chunk(&mut self, buffer: &mut [u8]) -> ArtifactStoreResult<usize> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        if buffer.is_empty() || self.verified.is_some() {
            return Ok(0);
        }
        let result = self.read_next(buffer);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }

    fn read_next(&mut self, buffer: &mut [u8]) -> ArtifactStoreResult<usize> {
        let remaining = self
            .reference
            .byte_count
            .saturating_sub(self.material.byte_count());
        let limit = usize::try_from(remaining.saturating_add(1))
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        let count = self.file.read(&mut buffer[..limit]).map_err(|error| {
            ArtifactStoreError::fatal(
                "artifact_read_failed",
                "read_projected_artifact",
                error.to_string(),
            )
        })?;
        self.material.update(&buffer[..count]).map_err(|error| {
            ArtifactStoreError::fatal(
                "artifact_verify_failed",
                "read_projected_artifact",
                error.to_string(),
            )
        })?;
        if self.material.byte_count() > self.reference.byte_count {
            return Err(ArtifactStoreError::fatal(
                "artifact_hash_mismatch",
                "verify_projected_artifact",
                "artifact exceeds its published byte count",
            ));
        }
        if count == 0 {
            let material = std::mem::take(&mut self.material).finish();
            verify_projected_material(&material, &self.reference)?;
            self.verified = Some(
                ArtifactStoreIssuer::new()
                    .map_err(|error| {
                        ArtifactStoreError::fatal(
                            "artifact_issuer_failed",
                            "verify_projected_read_only",
                            error.to_string(),
                        )
                    })?
                    .verify_existing_material(self.reference.clone(), material)
                    .map_err(|error| {
                        ArtifactStoreError::fatal(
                            "artifact_verify_failed",
                            "verify_projected_read_only",
                            error.to_string(),
                        )
                    })?,
            );
        }
        Ok(count)
    }

    /// Drains the remaining bytes in bounded chunks and returns authority only after full validation.
    pub fn finish(mut self) -> ArtifactStoreResult<VerifiedArtifactReference> {
        let mut buffer = [0_u8; 65_536];
        while self.read_chunk(&mut buffer)? != 0 {}
        self.verified.ok_or_else(|| {
            ArtifactStoreError::fatal(
                "artifact_verify_failed",
                "verify_projected_read_only",
                "artifact did not reach a verified EOF",
            )
        })
    }
}

impl Read for ArtifactReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.read_chunk(buffer).map_err(std::io::Error::other)
    }
}

/// Opens an immutable ledger-referenced object without taking a writer lock or repairing files.
/// Opening or reading a prefix does not verify it; the caller must complete `finish`.
pub fn open_projected_stream(
    root: impl AsRef<Path>,
    reference: &ProjectedArtifactReference,
) -> ArtifactStoreResult<ArtifactReader> {
    reference.validate().map_err(|error| {
        ArtifactStoreError::fatal(
            "artifact_reference_invalid",
            "read_projected_artifact",
            error.to_string(),
        )
    })?;
    let object_key = reference.object_key().ok_or_else(|| {
        ArtifactStoreError::fatal(
            "artifact_object_key_missing",
            "read_projected_artifact",
            "projected artifact reference does not include an object key",
        )
    })?;
    let root = root.as_ref().canonicalize().map_err(|error| {
        ArtifactStoreError::fatal(
            "artifact_root_failed",
            "read_projected_artifact",
            error.to_string(),
        )
    })?;
    let path = safe_object_path(&root, object_key)?;
    let file = File::open(path).map_err(|error| {
        ArtifactStoreError::fatal(
            "artifact_read_failed",
            "read_projected_artifact",
            error.to_string(),
        )
    })?;
    Ok(ArtifactReader {
        file,
        reference: reference.clone(),
        material: ArtifactMaterialAccumulator::default(),
        verified: None,
        failure: None,
    })
}

pub fn read_projected_verified(
    root: impl AsRef<Path>,
    reference: &ProjectedArtifactReference,
) -> ArtifactStoreResult<Vec<u8>> {
    reference.validate().map_err(|error| {
        ArtifactStoreError::fatal(
            "artifact_reference_invalid",
            "read_projected_artifact",
            error.to_string(),
        )
    })?;
    let object_key = reference.object_key().ok_or_else(|| {
        ArtifactStoreError::fatal(
            "artifact_object_key_missing",
            "read_projected_artifact",
            "projected artifact reference does not include an object key",
        )
    })?;
    let root = root.as_ref().canonicalize().map_err(|error| {
        ArtifactStoreError::fatal(
            "artifact_root_failed",
            "read_projected_artifact",
            error.to_string(),
        )
    })?;
    let path = safe_object_path(&root, object_key)?;
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| {
            file.take(reference.byte_count.saturating_add(1))
                .read_to_end(&mut bytes)
        })
        .map_err(|error| {
            ArtifactStoreError::fatal(
                "artifact_read_failed",
                "read_projected_artifact",
                error.to_string(),
            )
        })?;
    verify_projected_bytes(&bytes, reference)?;
    Ok(bytes)
}

pub fn verify_projected_read_only(
    root: impl AsRef<Path>,
    reference: &ProjectedArtifactReference,
) -> ArtifactStoreResult<VerifiedArtifactReference> {
    open_projected_stream(root, reference)?.finish()
}

fn safe_object_path(root: &Path, object_key: &str) -> ArtifactStoreResult<PathBuf> {
    let relative = Path::new(object_key);
    if relative.is_absolute()
        || relative.components().any(|component| {
            !matches!(component, Component::Normal(_)) && !matches!(component, Component::CurDir)
        })
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(ArtifactStoreError::fatal(
            "artifact_path_invalid",
            "resolve_artifact_path",
            "artifact object key is not a safe relative path",
        ));
    }
    let path = root.join(relative);
    if !path.starts_with(root) || path == root {
        return Err(ArtifactStoreError::fatal(
            "artifact_path_invalid",
            "resolve_artifact_path",
            "artifact object key escapes the store root",
        ));
    }
    Ok(path)
}

fn temporary_path(final_path: &Path) -> ArtifactStoreResult<PathBuf> {
    let file_name = final_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            ArtifactStoreError::fatal(
                "artifact_path_invalid",
                "create_artifact_temp_path",
                "artifact filename is not valid UTF-8",
            )
        })?;
    let nonce = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
    Ok(final_path.with_file_name(format!(
        ".{file_name}.partial-{}-{nonce}",
        std::process::id()
    )))
}

fn write_synced(path: &Path, bytes: &[u8]) -> ArtifactStoreResult<()> {
    write_synced_with(path, bytes, File::sync_all)
}

fn write_synced_with(
    path: &Path,
    bytes: &[u8],
    sync: impl FnOnce(&File) -> std::io::Result<()>,
) -> ArtifactStoreResult<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            ArtifactStoreError::fatal(
                "artifact_write_failed",
                "create_artifact_temp",
                error.to_string(),
            )
        })?;
    file.write_all(bytes).map_err(|error| {
        ArtifactStoreError::fatal(
            "artifact_write_failed",
            "write_artifact_temp",
            error.to_string(),
        )
    })?;
    sync(&file).map_err(|error| {
        ArtifactStoreError::fatal(
            "artifact_sync_failed",
            "sync_artifact_temp",
            error.to_string(),
        )
    })
}

fn publish_temp(temp_path: &Path, final_path: &Path) -> ArtifactStoreResult<()> {
    // Runtime owns one artifact-store writer per state root; the store mutex makes this
    // no-overwrite check and same-filesystem rename indivisible relative to its writers.
    if final_path.exists() {
        return Err(ArtifactStoreError::fatal(
            "artifact_collision",
            "publish_artifact",
            "artifact object key already exists",
        ));
    }
    publish_temp_with(temp_path, final_path, |from, to| fs::rename(from, to))
}

fn publish_temp_with(
    temp_path: &Path,
    final_path: &Path,
    rename: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> ArtifactStoreResult<()> {
    rename(temp_path, final_path).map_err(|error| {
        ArtifactStoreError::fatal(
            "artifact_publish_failed",
            "publish_artifact",
            error.to_string(),
        )
    })
}

fn verify_file(path: &Path, reference: &ArtifactReference) -> ArtifactStoreResult<()> {
    let mut file = File::open(path).map_err(|error| {
        ArtifactStoreError::fatal(
            "artifact_verify_failed",
            "open_artifact_for_verification",
            error.to_string(),
        )
    })?;
    let material = ArtifactMaterial::read_from(&mut file).map_err(|error| {
        ArtifactStoreError::fatal(
            "artifact_verify_failed",
            "read_artifact_for_verification",
            error.to_string(),
        )
    })?;
    if material.byte_count() != reference.byte_count() || material.sha256() != reference.sha256() {
        return Err(ArtifactStoreError::fatal(
            "artifact_hash_mismatch",
            "verify_artifact",
            "artifact byte count or SHA-256 does not match issued metadata",
        ));
    }
    Ok(())
}

fn verify_projected_material(
    material: &ArtifactMaterial,
    reference: &ProjectedArtifactReference,
) -> ArtifactStoreResult<()> {
    if material.byte_count() != reference.byte_count() || material.sha256() != reference.sha256() {
        return Err(ArtifactStoreError::fatal(
            "artifact_hash_mismatch",
            "verify_projected_artifact",
            "artifact byte count or SHA-256 does not match projected metadata",
        ));
    }
    Ok(())
}

fn verify_bytes(bytes: &[u8], reference: &ArtifactReference) -> ArtifactStoreResult<()> {
    let byte_count = u64::try_from(bytes.len()).map_err(|_| {
        ArtifactStoreError::fatal(
            "artifact_verify_failed",
            "verify_artifact",
            "artifact byte count exceeds u64",
        )
    })?;
    if byte_count != reference.byte_count() || canonical_sha256(bytes) != reference.sha256() {
        return Err(ArtifactStoreError::fatal(
            "artifact_hash_mismatch",
            "verify_artifact",
            "artifact byte count or SHA-256 does not match issued metadata",
        ));
    }
    Ok(())
}

fn verify_projected_bytes(
    bytes: &[u8],
    reference: &ProjectedArtifactReference,
) -> ArtifactStoreResult<()> {
    let byte_count = u64::try_from(bytes.len()).map_err(|_| {
        ArtifactStoreError::fatal(
            "artifact_verify_failed",
            "verify_projected_artifact",
            "artifact byte count exceeds u64",
        )
    })?;
    if byte_count != reference.byte_count() || canonical_sha256(bytes) != reference.sha256() {
        return Err(ArtifactStoreError::fatal(
            "artifact_hash_mismatch",
            "verify_projected_artifact",
            "artifact byte count or SHA-256 does not match projected metadata",
        ));
    }
    Ok(())
}

pub(crate) fn canonical_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut value = String::with_capacity(71);
    value.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write as _;
        write!(value, "{byte:02x}").expect("writing to a String cannot fail");
    }
    value
}

fn cleanup_temp(path: &Path, error: ArtifactStoreError) -> ArtifactStoreError {
    cleanup_path(path, "cleanup_artifact_temp", error)
}

fn cleanup_published(path: &Path, error: ArtifactStoreError) -> ArtifactStoreError {
    cleanup_path(path, "cleanup_published_artifact", error)
}

fn cleanup_path(
    path: &Path,
    operation: &'static str,
    error: ArtifactStoreError,
) -> ArtifactStoreError {
    match fs::remove_file(path) {
        Ok(()) => error,
        Err(remove_error) if remove_error.kind() == std::io::ErrorKind::NotFound => error,
        Err(remove_error) => error.with_secondary(&ArtifactStoreError::fatal(
            "artifact_cleanup_failed",
            operation,
            remove_error.to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::{
        ArtifactKind, ArtifactProducer, ArtifactRedactionState, EventType, IssuedCorrelationId,
        IssuedFrameId, IssuedRunId, RetentionClass, SanitizationError, SecretField,
        SecretFingerprinter, Sha256Fingerprint,
    };

    #[derive(Default)]
    struct RecordingSink {
        event_types: Vec<EventType>,
        references: Vec<ArtifactReference>,
        fail_at: Option<usize>,
    }

    impl ArtifactEventSink for RecordingSink {
        fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()> {
            if self.fail_at == Some(self.event_types.len()) {
                return Err(ArtifactStoreError::fatal(
                    "injected_event_failure",
                    "append_event",
                    "injected event sink failure",
                ));
            }
            let sanitized = draft.sanitize(&TestFingerprinter).map_err(|error| {
                ArtifactStoreError::fatal(
                    "event_sanitize_failed",
                    "append_event",
                    error.to_string(),
                )
            })?;
            self.event_types.push(sanitized.event_type());
            self.references.extend_from_slice(sanitized.artifacts());
            Ok(())
        }
    }

    struct TestFingerprinter;

    impl SecretFingerprinter for TestFingerprinter {
        fn fingerprint(
            &self,
            _field: SecretField,
            original: &str,
        ) -> Result<Sha256Fingerprint, SanitizationError> {
            Sha256Fingerprint::new(format!("sha256:{}", "a".repeat(64)), original)
        }
    }

    #[test]
    fn prepared_artifact_has_no_side_effect_until_committed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = ArtifactStore::open(temp.path()).expect("store");
        let mut sink = RecordingSink::default();
        let prepared = store
            .prepare(request(b"prepared artifact bytes"))
            .expect("prepared artifact");
        let reference = prepared.reference().clone();

        assert!(sink.event_types.is_empty());
        assert!(all_files(temp.path()).is_empty());

        let stored = store
            .commit_prepared(prepared, b"prepared artifact bytes", &mut sink)
            .expect("committed artifact");
        assert_eq!(stored.reference(), &reference);
        assert_eq!(
            sink.event_types,
            [EventType::ArtifactCreated, EventType::ArtifactVerified]
        );
    }

    #[test]
    fn prepared_artifact_rejects_different_bytes_before_publication() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = ArtifactStore::open(temp.path()).expect("store");
        let mut sink = RecordingSink::default();
        let prepared = store
            .prepare(request(b"expected artifact bytes"))
            .expect("prepared artifact");

        let error = store
            .commit_prepared(prepared, b"different artifact bytes", &mut sink)
            .expect_err("mismatched prepared bytes");
        assert_eq!(error.code(), "artifact_hash_mismatch");
        assert!(sink.event_types.is_empty());
        assert!(all_files(temp.path()).is_empty());
    }

    #[test]
    fn put_atomically_writes_verifies_and_emits_created_then_verified() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = ArtifactStore::open(temp.path()).expect("store");
        let mut sink = RecordingSink::default();
        let stored = store
            .put(request(b"trusted artifact bytes"), &mut sink)
            .expect("stored artifact");

        assert_eq!(
            fs::read(stored.path()).expect("stored bytes"),
            b"trusted artifact bytes"
        );
        assert!(stored.path().starts_with(store.root()));
        assert_eq!(
            sink.event_types,
            [EventType::ArtifactCreated, EventType::ArtifactVerified]
        );
        assert_eq!(stored.reference().kind(), ArtifactKind::CaptureFrame);
        assert_eq!(
            stored.reference().retention_class(),
            RetentionClass::DebugFull
        );

        let request = request(b"stream context");
        let expected_links = request.context.artifact_links.clone();
        let mut stream = store
            .begin_stream(
                ArtifactKind::DiagnosticJson,
                request.context,
                request.policy,
            )
            .expect("begin stream");
        let staging = stream.temp_path.clone();
        let mut stream_sink = RecordingSink::default();
        let mut expected_hash = Sha256::new();
        stream.append(b"\"").expect("open JSON string");
        expected_hash.update(b"\"");
        for index in 0..257 {
            let chunk = [b'a' + (index % 26) as u8; 4096];
            stream.append(&chunk[..17]).expect("partial chunk");
            stream.append(&chunk[17..]).expect("remaining chunk");
            expected_hash.update(chunk);
        }
        stream.append(b"\"").expect("close JSON string");
        expected_hash.update(b"\"");
        let expected_bytes = 257 * 4096 + 2;
        assert!(expected_bytes > 1_048_576);
        assert_eq!(
            fs::metadata(&staging).expect("staging metadata").len(),
            expected_bytes
        );
        assert!(stream_sink.event_types.is_empty());
        assert_eq!(all_files(temp.path()).len(), 2);

        let streamed = store
            .seal_stream(stream, &mut stream_sink)
            .expect("sealed stream");
        assert!(!staging.exists());
        assert_eq!(all_files(temp.path()).len(), 2);
        assert!(streamed.path().starts_with(store.root()));
        assert_eq!(streamed.reference().kind(), ArtifactKind::DiagnosticJson);
        assert_eq!(streamed.reference().byte_count(), expected_bytes);
        assert_eq!(
            streamed.reference().sha256(),
            format!("sha256:{:x}", expected_hash.finalize())
        );
        assert_eq!(
            stream_sink.event_types,
            [EventType::ArtifactCreated, EventType::ArtifactVerified]
        );
        assert_eq!(
            stream_sink.references,
            [streamed.reference().clone(), streamed.reference().clone()]
        );
        let expected = store
            .artifacts
            .issue(
                ArtifactKind::DiagnosticJson,
                expected_links,
                b"link comparison",
                1_752_147_200_000,
                request.policy,
            )
            .expect("comparison reference");
        assert_eq!(streamed.reference().run_id(), expected.reference().run_id());
        assert_eq!(
            streamed.reference().frame_id(),
            expected.reference().frame_id()
        );
        assert_eq!(
            streamed.reference().correlation_id(),
            expected.reference().correlation_id()
        );
        assert_ne!(
            streamed.reference().artifact_id(),
            stored.reference().artifact_id()
        );
        assert_eq!(
            store
                .verify_recovery_reference(&streamed.reference().project(true))
                .expect("stream recovery")
                .reference(),
            streamed.reference()
        );
    }

    #[test]
    fn projected_reference_reads_only_verified_artifact_bytes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = ArtifactStore::open(temp.path()).expect("store");
        let mut sink = RecordingSink::default();
        let stored = store
            .put(request(b"trusted projected bytes"), &mut sink)
            .expect("stored artifact");
        let projected = stored.reference().project(true);

        assert_eq!(
            read_projected_verified(temp.path(), &projected).expect("read projected artifact"),
            b"trusted projected bytes"
        );

        let request = request(b"stream context");
        let mut stream = store
            .begin_stream(ArtifactKind::TextReport, request.context, request.policy)
            .expect("begin stream");
        let chunk = [b'v'; 4093];
        for _ in 0..257 {
            stream.append(&chunk).expect("stream chunk");
        }
        let streamed = store.seal_stream(stream, &mut sink).expect("sealed stream");
        let stream_reference = streamed.reference().project(true);
        let mut reader =
            open_projected_stream(temp.path(), &stream_reference).expect("open stream");
        let mut buffer = [0_u8; 997];
        let first = reader.read_chunk(&mut buffer).expect("first chunk");
        assert!(first > 0);
        assert!(buffer[..first].iter().all(|value| *value == b'v'));
        assert!(reader.verified.is_none());
        let mut total = first;
        loop {
            let count = reader.read_chunk(&mut buffer).expect("read chunk");
            if count == 0 {
                break;
            }
            assert!(buffer[..count].iter().all(|value| *value == b'v'));
            total += count;
        }
        assert_eq!(total, 257 * chunk.len());
        assert!(total > 1_048_576);
        assert_eq!(
            reader.finish().expect("complete verification").reference(),
            streamed.reference()
        );
        assert_eq!(
            verify_projected_read_only(temp.path(), &stream_reference)
                .expect("read-only verification")
                .reference(),
            streamed.reference()
        );

        let mut reader =
            open_projected_stream(temp.path(), &stream_reference).expect("open before corruption");
        reader.read_chunk(&mut buffer).expect("provisional prefix");
        assert!(reader.verified.is_none());
        let mut corrupt = OpenOptions::new()
            .write(true)
            .open(streamed.path())
            .expect("open tail");
        corrupt.seek(std::io::SeekFrom::End(-1)).expect("seek tail");
        corrupt.write_all(b"x").expect("corrupt same-length tail");
        drop(corrupt);
        assert_eq!(
            reader
                .finish()
                .expect_err("corrupt tail cannot verify a prefix")
                .code(),
            "artifact_hash_mismatch"
        );

        fs::write(stored.path(), b"tampered projected bytes").expect("tamper artifact");
        assert_eq!(
            read_projected_verified(temp.path(), &projected)
                .expect_err("tampered artifact")
                .code(),
            "artifact_hash_mismatch"
        );
    }

    #[test]
    fn recovery_verifier_promotes_only_matching_persisted_artifacts() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = ArtifactStore::open(temp.path()).expect("store");
        let mut sink = RecordingSink::default();
        let stored = store
            .put(request(b"durable recovery bytes"), &mut sink)
            .expect("stored artifact");
        let projected = stored.reference().project(true);

        let verified = store
            .verify_recovery_reference(&projected)
            .expect("verified recovery artifact");
        assert_eq!(verified.reference(), stored.reference());

        fs::write(stored.path(), b"tampered recovery bytes").expect("tamper artifact");
        assert_eq!(
            store
                .verify_recovery_reference(&projected)
                .expect_err("tampered recovery artifact")
                .code(),
            "artifact_verify_failed"
        );
    }

    #[test]
    fn projected_reference_without_safe_object_key_is_rejected() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = ArtifactStore::open(temp.path()).expect("store");
        let mut sink = RecordingSink::default();
        let stored = store
            .put(request(b"trusted projected bytes"), &mut sink)
            .expect("stored artifact");
        let mut missing = stored.reference().project(true);
        missing.object_key = None;
        assert_eq!(
            read_projected_verified(temp.path(), &missing)
                .expect_err("missing object key")
                .code(),
            "artifact_object_key_missing"
        );

        let mut escaped = stored.reference().project(true);
        escaped.object_key = Some("../escape".to_string());
        assert_eq!(
            read_projected_verified(temp.path(), &escaped)
                .expect_err("escaped object key")
                .code(),
            "artifact_reference_invalid"
        );
    }

    #[test]
    fn empty_artifact_fails_before_any_event_or_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = ArtifactStore::open(temp.path()).expect("store");
        let mut sink = RecordingSink::default();
        let error = store
            .put(request(b""), &mut sink)
            .expect_err("empty rejected");

        assert_eq!(error.code(), "artifact_issue_failed");
        assert!(error.is_fatal());
        assert!(sink.event_types.is_empty());
        assert!(!temp.path().join("artifacts").exists());
    }

    #[test]
    fn required_created_event_failure_removes_published_file_and_returns_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = ArtifactStore::open(temp.path()).expect("store");
        let mut sink = RecordingSink {
            fail_at: Some(0),
            ..RecordingSink::default()
        };
        let error = store
            .put(request(b"must not become success"), &mut sink)
            .expect_err("event failure");

        assert_eq!(error.code(), "injected_event_failure");
        assert!(all_files(temp.path()).is_empty());

        let request = request(b"stream context");
        let mut stream = store
            .begin_stream(
                ArtifactKind::DiagnosticJson,
                request.context,
                request.policy,
            )
            .expect("begin stream");
        stream.append(b"{\"result\":").expect("first chunk");
        stream.append(b"\"complete\"}").expect("second chunk");
        let error = store
            .seal_stream(stream, &mut sink)
            .expect_err("stream created failure");
        assert_eq!(error.code(), "injected_event_failure");
        assert!(error.is_fatal());
        assert!(sink.event_types.is_empty());
        assert!(all_files(temp.path()).is_empty());
    }

    #[test]
    fn required_verified_event_failure_removes_published_file_and_returns_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = ArtifactStore::open(temp.path()).expect("store");
        let mut sink = RecordingSink {
            fail_at: Some(1),
            ..RecordingSink::default()
        };
        let error = store
            .put(request(b"verified bytes"), &mut sink)
            .expect_err("verified event failure");

        assert_eq!(error.code(), "injected_event_failure");
        assert_eq!(sink.event_types, [EventType::ArtifactCreated]);
        assert!(all_files(temp.path()).is_empty());

        let request = request(b"stream context");
        let mut stream = store
            .begin_stream(
                ArtifactKind::DiagnosticJson,
                request.context,
                request.policy,
            )
            .expect("begin stream");
        stream.append(b"{\"result\":").expect("first chunk");
        stream.append(b"\"complete\"}").expect("second chunk");
        let mut stream_sink = RecordingSink {
            fail_at: Some(1),
            ..RecordingSink::default()
        };
        let error = store
            .seal_stream(stream, &mut stream_sink)
            .expect_err("stream verified failure");
        assert_eq!(error.code(), "injected_event_failure");
        assert!(error.is_fatal());
        assert_eq!(stream_sink.event_types, [EventType::ArtifactCreated]);
        assert_eq!(stream_sink.references.len(), 1);
        assert!(all_files(temp.path()).is_empty());
    }

    #[test]
    fn hash_mismatch_is_fatal() {
        let temp = tempfile::tempdir().expect("tempdir");
        let issuer = ArtifactStoreIssuer::new().expect("artifact issuer");
        let issued = issuer
            .issue(
                ArtifactKind::CaptureFrame,
                ArtifactLinksDraft::default(),
                b"expected",
                1_752_147_200_000,
                ArtifactIssuePolicy::new(
                    ArtifactProducer::ArtifactStore,
                    RetentionClass::Adaptive,
                    ArtifactRedactionState::NotRequired,
                ),
            )
            .expect("issued");
        let path = temp.path().join("corrupt.bin");
        fs::write(&path, b"different").expect("corrupt file");
        let error = verify_file(&path, issued.reference()).expect_err("mismatch");
        assert_eq!(error.code(), "artifact_hash_mismatch");
    }

    #[test]
    fn publish_collision_preserves_existing_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let final_path = temp.path().join("final.bin");
        let temp_path = temp.path().join("pending.bin");
        fs::write(&final_path, b"old").expect("old file");
        fs::write(&temp_path, b"new").expect("temp file");
        let error = publish_temp(&temp_path, &final_path).expect_err("collision");
        assert_eq!(error.code(), "artifact_collision");
        assert_eq!(fs::read(&final_path).expect("old bytes"), b"old");
        assert_eq!(fs::read(&temp_path).expect("new bytes"), b"new");
    }

    #[test]
    fn sync_failure_is_fatal_and_partial_file_is_removed_by_cleanup() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("pending.bin");
        let error = write_synced_with(&path, b"partial", |_| {
            Err(std::io::Error::other("injected sync failure"))
        })
        .expect_err("sync failure");
        assert_eq!(error.code(), "artifact_sync_failed");
        assert!(path.exists());

        let error = cleanup_temp(&path, error);
        assert_eq!(error.code(), "artifact_sync_failed");
        assert!(!path.exists());
    }

    #[test]
    fn rename_failure_is_fatal_and_does_not_publish() {
        let temp = tempfile::tempdir().expect("tempdir");
        let pending = temp.path().join("pending.bin");
        let final_path = temp.path().join("final.bin");
        fs::write(&pending, b"pending").expect("pending file");

        let error = publish_temp_with(&pending, &final_path, |_, _| {
            Err(std::io::Error::other("injected rename failure"))
        })
        .expect_err("rename failure");
        assert_eq!(error.code(), "artifact_publish_failed");
        assert!(pending.exists());
        assert!(!final_path.exists());
    }

    #[test]
    fn write_failure_is_fatal() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("missing").join("pending.bin");
        let error = write_synced(&path, b"bytes").expect_err("write failure");
        assert_eq!(error.code(), "artifact_write_failed");
        assert!(!path.exists());
    }

    #[test]
    fn unsafe_object_keys_are_rejected() {
        let temp = tempfile::tempdir().expect("tempdir");
        for key in ["../escape", "/absolute", "artifacts/../../escape"] {
            let error = safe_object_path(temp.path(), key).expect_err("unsafe path");
            assert_eq!(error.code(), "artifact_path_invalid");
        }
    }

    fn request(bytes: &[u8]) -> ArtifactWriteRequest<'_> {
        let identifiers = IdentifierIssuer::new().expect("identifiers");
        let run: IssuedRunId = identifiers.mint_run_id().expect("run");
        let frame: IssuedFrameId = identifiers.mint_frame_id().expect("frame");
        let correlation: IssuedCorrelationId =
            identifiers.mint_correlation_id().expect("correlation");
        let context = ArtifactWriteContext::new(
            ArtifactLinksDraft::default()
                .with_run_id(run)
                .with_frame_id(frame)
                .with_correlation_id(correlation),
            EventLinksDraft::default()
                .with_run_id(run)
                .with_frame_id(frame)
                .with_correlation_id(correlation),
            1_752_147_200_000,
        );
        ArtifactWriteRequest::new(
            ArtifactKind::CaptureFrame,
            bytes,
            context,
            ArtifactIssuePolicy::new(
                ArtifactProducer::ArtifactStore,
                RetentionClass::DebugFull,
                ArtifactRedactionState::NotRequired,
            ),
        )
    }

    fn all_files(root: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        if !root.exists() {
            return files;
        }
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).expect("read directory") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    files.push(path);
                }
            }
        }
        files
    }
}
