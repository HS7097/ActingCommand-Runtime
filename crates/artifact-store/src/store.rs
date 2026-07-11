// SPDX-License-Identifier: AGPL-3.0-only

use crate::{ArtifactStoreError, ArtifactStoreResult};
use actingcommand_contract::{
    ArtifactIssuePolicy, ArtifactLinksDraft, ArtifactPayloadDraft, ArtifactReference,
    ArtifactStoreIssuer, AuditInput, DiagnosticCode, EventActor, EventDraft, EventLinksDraft,
    EventOrigin, EventSeverity, EventSource, IdentifierIssuer, OriginModule,
    ProjectedArtifactReference, StoreIssuedArtifact,
};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
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

    pub fn put(
        &self,
        request: ArtifactWriteRequest<'_>,
        sink: &mut dyn ArtifactEventSink,
    ) -> ArtifactStoreResult<StoredArtifact> {
        let _writer = self.writer.lock().map_err(|_| {
            ArtifactStoreError::fatal(
                "artifact_writer_poisoned",
                "store_artifact",
                "artifact writer lock is poisoned",
            )
        })?;
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
        let final_path = safe_object_path(&self.root, issued.reference().object_key())?;

        let result = self.write_and_verify(request.bytes, &final_path, issued.reference());
        if let Err(error) = result {
            return Err(self.report_failure(
                error,
                sink,
                &request.context,
                &issued,
                ArtifactPayloadDraft::store_failed(
                    DiagnosticCode::ArtifactWriteFailed,
                    AuditInput::new(),
                ),
            ));
        }

        if let Err(error) = self.append_event(
            sink,
            &request.context,
            EventSeverity::Info,
            ArtifactPayloadDraft::created(AuditInput::new()),
            issued.clone(),
        ) {
            return Err(cleanup_published(&final_path, error));
        }

        if let Err(error) = verify_file(&final_path, issued.reference()) {
            let error = cleanup_published(&final_path, error);
            return Err(self.report_failure(
                error,
                sink,
                &request.context,
                &issued,
                ArtifactPayloadDraft::verification_failed(
                    DiagnosticCode::ArtifactVerifyFailed,
                    AuditInput::new(),
                ),
            ));
        }

        if let Err(error) = self.append_event(
            sink,
            &request.context,
            EventSeverity::Info,
            ArtifactPayloadDraft::verified(AuditInput::new()),
            issued.clone(),
        ) {
            return Err(cleanup_published(&final_path, error));
        }

        Ok(StoredArtifact {
            issued,
            path: final_path,
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
    let bytes = fs::read(path).map_err(|error| {
        ArtifactStoreError::fatal(
            "artifact_read_failed",
            "read_projected_artifact",
            error.to_string(),
        )
    })?;
    verify_projected_bytes(&bytes, reference)?;
    Ok(bytes)
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
    // no-overwrite check and same-directory rename indivisible relative to its writers.
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
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|error| {
        ArtifactStoreError::fatal(
            "artifact_verify_failed",
            "read_artifact_for_verification",
            error.to_string(),
        )
    })?;
    let byte_count = u64::try_from(bytes.len()).map_err(|_| {
        ArtifactStoreError::fatal(
            "artifact_verify_failed",
            "verify_artifact",
            "artifact byte count exceeds u64",
        )
    })?;
    if byte_count != reference.byte_count() || canonical_sha256(&bytes) != reference.sha256() {
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

        fs::write(stored.path(), b"tampered projected bytes").expect("tamper artifact");
        assert_eq!(
            read_projected_verified(temp.path(), &projected)
                .expect_err("tampered artifact")
                .code(),
            "artifact_hash_mismatch"
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
