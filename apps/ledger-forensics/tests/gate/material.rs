// SPDX-License-Identifier: AGPL-3.0-only

// `actingledger --state-root <root> material --request <request-json>`
// (contracts/material-read.md, "Offline consumer and privacy", with "Result and failure"). The
// typed selection goes through the same Ledger resolver and ArtifactStore reader and returns a
// structured `material_read` report within the requested response bound. Only `verified` carries
// a chunk, whose bytes passed the full length and SHA-256 verification; non-verified outcomes
// return nonzero after the bounded report. The read never writes new facts into the source, and
// a `missing` read (a frame deleted by hand: a native NotFound) records nothing.

use crate::fixture::{OpenRoot, at, cli, report};
use actingcommand_artifact_store::{
    ArtifactCapacityAdmission, ArtifactEventSink, ArtifactStore, ArtifactStoreError,
    ArtifactStoreResult, ArtifactWriteContext, ArtifactWriteRequest,
};
use actingcommand_contract::{
    ArtifactIssuePolicy, ArtifactKind, ArtifactLinksDraft, ArtifactProducer,
    ArtifactRedactionState, AuditInput, CapacityAdmissionOutcome, CapacityAdmissionReason,
    CapacityDecision, CapacityFactReference, CapturePayloadDraft, EffectDisposition, EventAction,
    EventActor, EventDraft, EventLinksDraft, EventOrigin, EventSource, EventType, IdentifierIssuer,
    LedgerEventPosition, MAX_RUNTIME_MATERIAL_REPLY_BYTES, OriginModule, RetentionClass,
    RuntimeMaterialReadRequest, RuntimeMaterialReadResult, RuntimeMaterialReadState,
};
use actingcommand_ledger::PersistedEvent;
use actingcommand_runtime_database::DATABASE_FILE;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;

// The captured frame's bytes.
const FRAME: &[u8] = b"neutral frame bytes read offline";

// The fixture's capacity owner: every write into the root is allowed on one committed sample.
struct FixtureCapacity {
    fact: CapacityFactReference,
}

impl FixtureCapacity {
    fn new(ids: &IdentifierIssuer) -> Self {
        Self {
            fact: CapacityFactReference {
                event_id: *ids.mint_event_id().expect("capacity event").transport(),
                sequence: 1,
                owner_epoch: *ids.mint_owner_epoch().expect("capacity owner").transport(),
                observed_at_unix_ms: 1,
                observed_at_monotonic_ms: 0,
            },
        }
    }
}

impl ArtifactCapacityAdmission for FixtureCapacity {
    fn decide(&self, _path: &Path, bytes: u64) -> ArtifactStoreResult<CapacityDecision> {
        Ok(CapacityDecision {
            owner_epoch: self.fact.owner_epoch,
            decided_at_unix_ms: 1,
            decided_at_monotonic_ms: 0,
            requested_bytes: bytes,
            target_volume: Some("fixture-volume".to_owned()),
            admission_result: CapacityAdmissionOutcome::Allowed,
            reason: CapacityAdmissionReason::FreshSample,
            fact: Some(self.fact.clone()),
        })
    }
}

// Appends the ArtifactStore's events through the root's writer and keeps the frame's
// `artifact.verified` event.
struct LedgerSink<'a> {
    writer: &'a OpenRoot,
    verified: Option<PersistedEvent>,
}

impl ArtifactEventSink for LedgerSink<'_> {
    fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()> {
        let draft = draft
            .sanitize(self.writer.fingerprinter())
            .map_err(|error| {
                ArtifactStoreError::fatal(
                    "fixture_sanitize_failed",
                    "fixture_sink",
                    error.to_string(),
                )
            })?;
        let event = self.writer.ledger().append(draft).map_err(|error| {
            ArtifactStoreError::fatal(error.code(), "fixture_sink", error.to_string())
        })?;
        if event.event_type() == EventType::ArtifactVerified {
            self.verified = Some(event);
        }
        Ok(())
    }
}

// A closed formal root holding one captured frame whose bytes are in the ArtifactStore.
struct FrameRoot {
    directory: TempDir,
    // The selection of the whole frame by its `artifact.verified` event.
    request: RuntimeMaterialReadRequest,
    // The frame's file in the ArtifactStore.
    material: PathBuf,
}

// One frame of an instance published into the ArtifactStore with its events, then the capture's
// completion, as an observe records it.
fn frame_root() -> FrameRoot {
    let directory = tempfile::tempdir().expect("root directory");
    let writer = OpenRoot::create(directory.path());
    let artifacts = ArtifactStore::open(directory.path()).expect("artifact owner");
    artifacts
        .install_capacity_admission(Arc::new(FixtureCapacity::new(writer.ids())))
        .expect("fixture capacity owner");
    let ids = writer.ids();
    let correlation = ids.mint_correlation_id().expect("correlation id");
    let frame = ids.mint_frame_id().expect("frame id");
    let links = EventLinksDraft::default()
        .with_instance_id(writer.instance())
        .with_request_id(ids.mint_request_id().expect("request id"))
        .with_correlation_id(correlation)
        .with_frame_id(frame);
    let mut sink = LedgerSink {
        writer: &writer,
        verified: None,
    };
    let stored = artifacts
        .put(
            ArtifactWriteRequest::new(
                ArtifactKind::CaptureFrame,
                FRAME,
                ArtifactWriteContext::new(
                    ArtifactLinksDraft::default()
                        .with_frame_id(frame)
                        .with_correlation_id(correlation),
                    links.clone(),
                    at(1),
                ),
                ArtifactIssuePolicy::new(
                    ArtifactProducer::CaptureStore,
                    RetentionClass::DebugFull,
                    ArtifactRedactionState::Pending,
                ),
            ),
            &mut sink,
        )
        .expect("frame publication");
    let verified = sink
        .verified
        .take()
        .expect("the frame's artifact.verified event");
    let reference = verified
        .artifacts()
        .iter()
        .find(|artifact| artifact.kind() == ArtifactKind::CaptureFrame)
        .expect("the frame's reference");
    let request = RuntimeMaterialReadRequest {
        event: LedgerEventPosition {
            event_id: *verified.event_id(),
            sequence: verified.sequence(),
        },
        artifact_id: *reference.artifact_id(),
        snapshot_position: verified.sequence(),
        byte_count: reference.byte_count(),
        sha256: reference.sha256().to_owned(),
        expected_run_id: None,
        expected_frame_id: None,
        expected_request_id: None,
        expected_correlation_id: None,
        offset: 0,
        requested_length: u32::try_from(FRAME.len()).expect("frame length"),
        max_reply_bytes: MAX_RUNTIME_MATERIAL_REPLY_BYTES,
    };
    writer.append(
        at(2),
        EventOrigin::new(
            EventSource::Device,
            OriginModule::Capture,
            EventActor::Runtime,
        ),
        links,
        CapturePayloadDraft::completed(
            EventAction::CaptureObserve,
            EffectDisposition::Performed,
            1280,
            720,
            AuditInput::new(),
        )
        .into(),
    );
    writer.close();
    FrameRoot {
        directory,
        request,
        material: stored.path().to_path_buf(),
    }
}

// One `material --request <the request's JSON>`: its result and what it printed.
fn read_material(
    root: &Path,
    request: &RuntimeMaterialReadRequest,
) -> (Result<(), actingledger::CliError>, Vec<u8>) {
    let json = serde_json::to_string(request).expect("request JSON");
    cli(root, &["material", "--request", &json])
}

// The `material_read` report printed, which fits the request's reply bound.
fn material_report(
    output: &[u8],
    request: &RuntimeMaterialReadRequest,
) -> RuntimeMaterialReadResult {
    let body = output.strip_suffix(b"\n").unwrap_or(output);
    assert!(
        body.len() <= request.max_reply_bytes,
        "the report fits the reply bound: {} bytes",
        body.len()
    );
    let printed = report(output);
    assert_eq!(printed["command"], "material_read", "{printed}");
    serde_json::from_value(printed["data"].clone()).expect("the material read result")
}

// Every file under `directory` with its bytes, by its path relative to `root`.
fn collect(root: &Path, directory: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
    for entry in fs::read_dir(directory).expect("read a directory of the root") {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            collect(root, &path, files);
        } else {
            let bytes =
                fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            files.insert(
                path.strip_prefix(root)
                    .expect("a path inside the root")
                    .to_path_buf(),
                bytes,
            );
        }
    }
}

// The root's files and their bytes. A reader that opens the State database may leave SQLite's
// WAL index (`-shm`) and an empty write-ahead log (`-wal`) beside it; neither holds a record, so
// neither counts. A record written would change the database file or fill the log.
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut found = BTreeMap::new();
    collect(root, root, &mut found);
    found.remove(Path::new(&format!("{DATABASE_FILE}-shm")));
    let log = PathBuf::from(format!("{DATABASE_FILE}-wal"));
    if found.get(&log).is_some_and(Vec::is_empty) {
        found.remove(&log);
    }
    found
}

// The read of the frame's verified bytes returns the verified report with exactly those bytes and
// their hash, exits 0, and leaves the source as it was.
#[test]
fn gate_material_returns_the_verified_frame_bytes_and_writes_nothing() {
    let frame = frame_root();
    let root = frame.directory.path();
    let before = files(root);

    let (result, output) = read_material(root, &frame.request);
    assert!(
        result.is_ok(),
        "the verified read exits 0: {result:?}\n{}",
        String::from_utf8_lossy(&output)
    );
    let read = material_report(&output, &frame.request);
    assert_eq!(read.state, RuntimeMaterialReadState::Verified, "{read:?}");
    let chunk = read.chunk.as_ref().expect("the verified chunk");
    assert_eq!(chunk.bytes, FRAME, "exactly the frame's bytes");
    assert_eq!(chunk.sha256, frame.request.sha256, "the frame's SHA-256");
    assert_eq!(
        (chunk.offset, chunk.total_length, chunk.is_last),
        (0, frame.request.byte_count, true),
        "the whole frame"
    );
    assert_eq!(
        files(root),
        before,
        "the read writes nothing into the source"
    );
}

// A frame deleted by hand reads `missing` without bytes: the bounded report is printed, the tool
// exits nonzero, and the read records nothing.
#[test]
fn gate_material_of_a_frame_deleted_by_hand_is_missing_and_exits_nonzero() {
    let frame = frame_root();
    let root = frame.directory.path();
    fs::remove_file(&frame.material).expect("delete the frame by hand");
    let before = files(root);

    let (result, output) = read_material(root, &frame.request);
    assert!(
        result.is_err(),
        "a non-verified outcome exits nonzero: {}",
        String::from_utf8_lossy(&output)
    );
    let read = material_report(&output, &frame.request);
    assert_eq!(read.state, RuntimeMaterialReadState::Missing, "{read:?}");
    assert!(read.chunk.is_none(), "only a verified read carries bytes");
    assert_eq!(files(root), before, "a missing read records nothing");
}
