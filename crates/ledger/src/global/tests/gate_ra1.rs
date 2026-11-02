// SPDX-License-Identifier: AGPL-3.0-only

// #422 RA-1 (coordinator work order rd-1031/brief-ra-1.md, register §1.266): the Runtime's artifact
// resolution is answered by the writer's reader pool, off the writer thread, as its view pages are
// (QR-1); the writer resolves only when no reader serves.
// - `an_artifact_resolution_is_answered_by_the_reader_pool`: for every kind of selection (a frame
//   without an eviction, one with a pending and one with a completed eviction, and each refusal a
//   selection can meet) the pool's answer is the writer's at the same head, and the writer did not
//   resolve it.
// - `a_resolution_does_not_wait_for_a_busy_writer`: while the writer is held inside an append's
//   write transaction, a resolution is answered at the committed head, and refused past it.
// - `a_resolution_racing_retention_answers_missing`: a resolution that took its point before
//   retention removed the frame answers the frame without an eviction as of that point, and the
//   material read then finds no file, the answer of a missing artifact; the writer committed the
//   eviction meanwhile, and a later resolution sees it.
// - `without_the_reader_pool_a_resolution_falls_back_to_the_writer`: a ledger without a pool, and
//   one whose readers all ended (each such end answered `writer_panicked`, not fatal, and said),
//   resolve on the writer with the same answer.
//
// The roots are formal roots holding real captured frames (`missing_material::capture`). No test
// reads a clock or sleeps: every deadline is far ahead and every wait returns as soon as its event
// arrives; `GATE_WAIT` only bounds a broken run.

use super::missing_material::{Capture, admit, capture, formal_root, material_store};
use crate::codes::{LedgerCode, LedgerLocation};
use crate::global::{
    GlobalLedger, GlobalLedgerError, GlobalLedgerResult, LedgerArtifactSelection,
    LedgerFailurePlan, LedgerMaintenance, LedgerOpenTiming, LedgerWritePolicy,
    LedgerWriterCommandKind, ResolvedLedgerArtifact, Sha256SecretFingerprinter,
};
use crate::{WriterReadHoldGate, WriterStallGate};
use actingcommand_artifact_store::open_projected_stream;
use actingcommand_contract::{
    ArtifactEvictionDisposition, AuditInput, EventActor, EventDraft, EventLinksDraft, EventOrigin,
    EventSeverity, EventSource, FactValue, IdentifierIssuer, LedgerEventPosition, OriginModule,
    OwnerEpoch, RuntimeFactRecord, RuntimeFactScope, RuntimePayloadDraft, SanitizedEventDraft,
};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use actingcommand_runtime_state::RuntimeStateStore;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

// The State key (a bootstrap seed of at least 16 bytes) and the writer's owner id: neutral
// fixture words.
const STATE_KEY: &[u8] = b"ledger-ra1-fixture-key";
const OWNER: &str = "ledger-ra1-fixture";
// The writer's start time (2030-01-01), its write policy and the open's deadline, as QR-1's.
const FIRST_MS: u64 = 1_893_456_000_000;
const STALL_MS: u64 = 120_000;
const PROBE_MS: u64 = 1_000;
const DUMP_MAX_BYTES: u64 = 1_073_741_824;
const OPEN_TIMEOUT_SECONDS: u64 = 600;
// The reader pool's readers, named so that the fallback test ends each of them.
const READERS: usize = 2;
// Every caller waits this long for one reply; no wait of these tests reaches it.
const REPLY_TIMEOUT: Duration = Duration::from_secs(600);
// The one bound of a wait in this file, and of every resolution's deadline: only a broken run
// reaches it. The busy-writer test's callers also wait at most this long for a reply, so a
// resolution that waited for the held writer ends there, refused.
const GATE_WAIT: Duration = Duration::from_secs(30);

fn epoch() -> OwnerEpoch {
    *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_owner_epoch()
        .expect("owner epoch")
        .transport()
}

/// A formal root, empty and closed.
struct Root {
    dir: TempDir,
    database: Arc<RuntimeDatabase>,
}

impl Root {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("state root");
        let database = Arc::new(
            RuntimeStateStore::open_database(dir.path(), STATE_KEY).expect("State database"),
        );
        let limits = MaintenanceLimits::default();
        let maintenance = LedgerMaintenance::acquire(
            dir.path(),
            true,
            limits,
            limits.deadline().expect("deadline"),
        )
        .expect("maintenance owner");
        maintenance
            .initialize_empty(&database)
            .expect("formal ledger");
        maintenance
            .open_writer(Arc::clone(&database), OWNER.into())
            .expect("formal writer")
            .close()
            .expect("close the formal writer");
        Self { dir, database }
    }

    /// The Runtime's writer of the root (a failure plan: it has a reader pool of `READERS`
    /// readers), its callers waiting `reply_timeout`.
    fn open(&self, reply_timeout: Duration) -> GlobalLedger {
        let state_root = self.dir.path().canonicalize().expect("state root");
        let plan = LedgerFailurePlan::new(
            LedgerWritePolicy::new(1, vec![0], STALL_MS, PROBE_MS, DUMP_MAX_BYTES)
                .expect("write policy"),
            &state_root,
            None,
            epoch(),
            FIRST_MS,
        )
        .with_reply_timeout(reply_timeout)
        .with_reader_threads_for_test(READERS);
        let deadline = MaintenanceLimits {
            timeout_seconds: OPEN_TIMEOUT_SECONDS,
            ..MaintenanceLimits::default()
        }
        .deadline()
        .expect("deadline");
        let ledger = LedgerMaintenance::acquire(
            self.dir.path(),
            false,
            MaintenanceLimits::default(),
            deadline,
        )
        .expect("maintenance owner")
        .open_writer_timed(
            Arc::clone(&self.database),
            OWNER.into(),
            Some(plan),
            &mut LedgerOpenTiming::default(),
        )
        .expect("the writer opens");
        assert!(
            ledger.reads_off_writer(),
            "the Runtime's writer reads on its reader pool: {:?}",
            ledger.reader_pool_finding()
        );
        ledger
    }
}

/// A deadline `GATE_WAIT` ahead. A gate file reads no clock itself (test plan v2.2 §2.1 guard 3);
/// as in `gate_ln6.rs`, the deadline is the one `MaintenanceLimits` computes.
fn deadline() -> Instant {
    MaintenanceLimits {
        timeout_seconds: GATE_WAIT.as_secs(),
        ..MaintenanceLimits::default()
    }
    .deadline()
    .expect("a deadline GATE_WAIT ahead")
}

/// The selection of `frame`'s artifact in its `artifact.verified` record, at `snapshot`.
fn selection(frame: &Capture, snapshot: u64) -> LedgerArtifactSelection {
    LedgerArtifactSelection {
        event: LedgerEventPosition {
            event_id: frame.verified.event_id,
            sequence: frame.verified.sequence,
        },
        artifact_id: frame.reference.artifact_id,
        snapshot_position: snapshot,
        sha256: frame.reference.sha256.clone(),
        byte_count: frame.reference.byte_count,
        run_id: None,
        frame_id: None,
        request_id: None,
        correlation_id: None,
    }
}

/// A resolution's answer as its caller sees it: the resolved reference, links, sensitivity,
/// positions and eviction, or the refusal's code and operation and whether it is fatal.
fn answer(result: &GlobalLedgerResult<ResolvedLedgerArtifact>) -> String {
    match result {
        Ok(resolved) => format!("{resolved:?}"),
        Err(error) => format!(
            "refused {} of {}, fatal {}",
            error.code(),
            error.operation(),
            error.is_fatal()
        ),
    }
}

/// An error's code, operation and whether it is fatal.
fn named(error: &GlobalLedgerError) -> (&'static str, &'static str, bool) {
    (error.code(), error.operation(), error.is_fatal())
}

/// A synchronous append as the policy driver makes one: a runtime fact the writer records.
fn runtime_fact(ids: &IdentifierIssuer, value: i64) -> SanitizedEventDraft {
    EventDraft::new(
        ids.mint_event_id().expect("event id"),
        FIRST_MS,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        EventLinksDraft::default(),
        RuntimePayloadDraft::fact_recorded(
            RuntimeFactRecord {
                scope: RuntimeFactScope::Runtime,
                key: "backend.fixture_key_ra1".to_owned(),
                value: FactValue::Integer(value),
                observed_at_unix_ms: FIRST_MS,
                source: OriginModule::Runtime,
                ttl_ms: None,
            },
            AuditInput::new(),
        )
        .into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"ra1-fixture-salt").expect("salt"))
    .expect("a fixture draft")
}

/// The writer's command just before an append of `value`, as the append observed it.
fn writer_work_before_append(
    ledger: &GlobalLedger,
    ids: &IdentifierIssuer,
    value: i64,
) -> Option<LedgerWriterCommandKind> {
    let (appended, observation) = ledger.append_with_observation(runtime_fact(ids, value));
    appended.expect("the append commits");
    observation
        .expect("the append's observation")
        .previous_writer_work
        .command
}

#[test]
fn an_artifact_resolution_is_answered_by_the_reader_pool() {
    let root = Root::new();
    let ledger = root.open(REPLY_TIMEOUT);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let artifacts = material_store(root.dir.path());
    let kept = capture(&ledger, &artifacts, b"frame kept without an eviction");
    let evicted = capture(&ledger, &artifacts, b"frame evicted with its outcome");
    let outcome = ledger
        .finish_artifact_eviction(admit(&ledger, &artifacts, &evicted))
        .expect("recorded deletion");
    let pending = capture(&ledger, &artifacts, b"frame of a pending eviction");
    drop(admit(&ledger, &artifacts, &pending));
    let head = ledger.latest_sequence().expect("head");

    // Every kind of selection: the three frames, the frame at its own position, and each refusal
    // a selection can meet.
    let mut another_record = selection(&kept, head);
    another_record.event.event_id = *outcome.event_id();
    let mut another_hash = selection(&kept, head);
    another_hash.sha256 = evicted.reference.sha256.clone();
    let mut another_run = selection(&kept, head);
    another_run.run_id = Some(*ids.mint_run_id().expect("run id").transport());
    let mut another_artifact = selection(&kept, head);
    another_artifact.artifact_id = evicted.reference.artifact_id;
    let mut no_bytes = selection(&kept, head);
    no_bytes.byte_count = 0;
    let mut past_the_head = selection(&kept, head + 1);
    past_the_head.event.sequence = head + 1;
    let cases = [
        ("kept", selection(&kept, head), None),
        ("pending", selection(&pending, head), None),
        ("evicted", selection(&evicted, head), None),
        (
            "kept at its own position",
            selection(&kept, kept.verified.sequence),
            None,
        ),
        (
            "another record at the position",
            another_record,
            Some(LedgerCode::ArtifactEventNotFound),
        ),
        (
            "a snapshot past the head",
            selection(&kept, head + 1),
            Some(LedgerCode::ArtifactSnapshotPositionInvalid),
        ),
        (
            "a record past the head",
            past_the_head,
            Some(LedgerCode::ArtifactSnapshotPositionInvalid),
        ),
        (
            "another hash",
            another_hash,
            Some(LedgerCode::ArtifactReferenceMismatch),
        ),
        (
            "another run",
            another_run,
            Some(LedgerCode::ArtifactEventLinksMismatch),
        ),
        (
            "an artifact the record does not carry",
            another_artifact,
            Some(LedgerCode::ArtifactReferenceNotFound),
        ),
        (
            "no bytes",
            no_bytes,
            Some(LedgerCode::InvalidArtifactSelection),
        ),
    ];
    for (label, chosen, refusal) in cases {
        let pooled = ledger.resolve_artifact(chosen.clone(), deadline());
        let written = ledger.resolve_artifact_on_writer_for_test(chosen, deadline());
        assert_eq!(
            answer(&pooled),
            answer(&written),
            "{label}: the pool answers as the writer does"
        );
        match (refusal, &pooled) {
            (None, Ok(resolved)) => assert_eq!(resolved.availability_through(), head, "{label}"),
            (Some(code), Err(error)) => assert_eq!(
                (error.code(), error.is_fatal()),
                (code.as_str(), false),
                "{label}: {error:?}"
            ),
            _ => panic!("{label}: {pooled:?}"),
        }
    }

    // The pool's answers carry each frame's retention as of the head.
    let resolve = |frame: &Capture| {
        ledger
            .resolve_artifact(selection(frame, head), deadline())
            .expect("a resolved frame")
    };
    assert!(resolve(&kept).eviction().is_none(), "no eviction");
    let pending_eviction = resolve(&pending)
        .eviction()
        .cloned()
        .expect("the pending eviction");
    assert!(
        pending_eviction.disposition.is_none(),
        "{pending_eviction:?}"
    );
    let evicted_eviction = resolve(&evicted)
        .eviction()
        .cloned()
        .expect("the completed eviction");
    assert_eq!(
        evicted_eviction
            .terminal_event
            .map(|terminal| terminal.sequence),
        Some(outcome.sequence()),
        "{evicted_eviction:?}"
    );

    // The pool resolved them, not the writer: after an append, a resolution leaves the writer's
    // last work the append's.
    ledger
        .append(runtime_fact(&ids, 1))
        .expect("an append commits");
    resolve(&kept);
    assert_ne!(
        writer_work_before_append(&ledger, &ids, 2),
        Some(LedgerWriterCommandKind::ResolveArtifact),
        "the reader pool resolved the artifact, not the writer"
    );
    ledger.close().expect("close");
}

#[test]
fn a_resolution_does_not_wait_for_a_busy_writer() {
    let root = Root::new();
    let ledger = root.open(GATE_WAIT);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let artifacts = material_store(root.dir.path());
    let frame = capture(
        &ledger,
        &artifacts,
        b"frame resolved while the writer is held",
    );
    let head = ledger.latest_sequence().expect("head");
    let expected =
        answer(&ledger.resolve_artifact_on_writer_for_test(selection(&frame, head), deadline()));
    let stall = WriterStallGate::arm(root.dir.path()).expect("writer stall gate");
    let writer = &ledger;
    let appended = thread::scope(|scope| {
        let draft = runtime_fact(&ids, 1);
        let appending = scope.spawn(move || writer.append(draft));
        assert!(
            stall.wait_held_commit(GATE_WAIT),
            "the writer is held inside the append's write transaction"
        );
        let resolved = writer.resolve_artifact(selection(&frame, head), deadline());
        assert_eq!(
            answer(&resolved),
            expected,
            "the pool answers at the committed head while the writer is held"
        );
        let refused = writer
            .resolve_artifact(selection(&frame, head + 1), deadline())
            .expect_err("a snapshot past the committed head");
        assert_eq!(
            named(&refused),
            (
                LedgerCode::ArtifactSnapshotPositionInvalid.as_str(),
                LedgerLocation::ResolveLedgerArtifact.as_str(),
                false
            ),
            "the record held in the write transaction is not committed"
        );
        stall.release();
        appending.join().expect("the append's thread")
    });
    let appended = appended.expect("the held append commits once released");
    assert_eq!(appended.sequence(), head + 1);
    // Once it committed, a resolution answers at its head.
    let resolved = ledger
        .resolve_artifact(selection(&frame, head + 1), deadline())
        .expect("a resolution at the new head");
    assert_eq!(resolved.availability_through(), head + 1);
    ledger.close().expect("close");
}

#[test]
fn a_resolution_racing_retention_answers_missing() {
    let root = Root::new();
    let ledger = root.open(REPLY_TIMEOUT);
    let artifacts = material_store(root.dir.path());
    let raced = capture(
        &ledger,
        &artifacts,
        b"frame retention removes during a resolution",
    );
    let head = ledger.latest_sequence().expect("head");
    let before =
        answer(&ledger.resolve_artifact_on_writer_for_test(selection(&raced, head), deadline()));
    let hold = WriterReadHoldGate::arm(root.dir.path()).expect("read hold");
    let writer = &ledger;
    let chosen = selection(&raced, head);
    let (resolved, outcome) = thread::scope(|scope| {
        let resolving = scope.spawn(move || writer.resolve_artifact(chosen, deadline()));
        assert!(
            hold.wait_held(GATE_WAIT),
            "the resolution took its point and is held"
        );
        // Retention removes the frame meanwhile: the writer commits its intent, the file goes and
        // the writer commits the outcome, none of it waiting for the held resolution.
        let outcome = writer
            .finish_artifact_eviction(admit(writer, &artifacts, &raced))
            .expect("recorded deletion");
        assert!(!raced.material.exists(), "retention removed the file");
        hold.release();
        (resolving.join().expect("the resolution's thread"), outcome)
    });
    // The resolution answers at the point it took: the frame's record, no eviction yet.
    let resolved = resolved.expect("the resolution answers at its point");
    assert_eq!(
        format!("{resolved:?}"),
        before,
        "the writer's answer at that head"
    );
    assert_eq!(resolved.availability_through(), head);
    assert!(resolved.eviction().is_none(), "no eviction as of its point");
    // The material read then finds no file: what a missing artifact answers today (`missing`,
    // `material_read_missing`), no new failure kind.
    let missing = open_projected_stream(artifacts.root(), resolved.reference())
        .err()
        .expect("no file to open");
    assert!(missing.is_material_missing(), "{missing:?}");
    // A later resolution sees the eviction, as the writer does.
    let now = ledger.latest_sequence().expect("head");
    assert!(now > head, "the eviction committed");
    let later = ledger.resolve_artifact(selection(&raced, now), deadline());
    assert_eq!(
        answer(&later),
        answer(&ledger.resolve_artifact_on_writer_for_test(selection(&raced, now), deadline())),
        "the pool answers as the writer does"
    );
    let eviction = later
        .expect("a later resolution")
        .eviction()
        .cloned()
        .expect("the eviction");
    assert_eq!(
        eviction.terminal_event.map(|terminal| terminal.sequence),
        Some(outcome.sequence())
    );
    assert!(
        matches!(
            eviction.disposition,
            Some(ArtifactEvictionDisposition::Deleted)
        ),
        "{eviction:?}"
    );
    ledger.close().expect("close");
}

#[test]
fn without_the_reader_pool_a_resolution_falls_back_to_the_writer() {
    let ids = IdentifierIssuer::new().expect("identifier issuer");

    // A ledger without a reader pool (no failure plan) resolves on the writer.
    let dir = tempfile::tempdir().expect("state root");
    let (_database, plain) = formal_root(dir.path());
    assert!(!plain.reads_off_writer(), "no reader pool");
    let artifacts = material_store(dir.path());
    let frame = capture(
        &plain,
        &artifacts,
        b"frame resolved by a writer without a pool",
    );
    let head = plain.latest_sequence().expect("head");
    let resolved = plain
        .resolve_artifact(selection(&frame, head), deadline())
        .expect("the writer resolves");
    assert_eq!(resolved.availability_through(), head);
    assert_eq!(
        writer_work_before_append(&plain, &ids, 1),
        Some(LedgerWriterCommandKind::ResolveArtifact),
        "the writer resolved the artifact"
    );
    plain.close().expect("close");

    // A Runtime writer whose readers all ended: each resolution a reader ended in is its
    // `writer_panicked`, not fatal; the ends are said; then the writer resolves, as it did.
    let root = Root::new();
    let ledger = root.open(REPLY_TIMEOUT);
    let artifacts = material_store(root.dir.path());
    let frame = capture(
        &ledger,
        &artifacts,
        b"frame resolved once no reader is left",
    );
    let head = ledger.latest_sequence().expect("head");
    let expected =
        answer(&ledger.resolve_artifact_on_writer_for_test(selection(&frame, head), deadline()));
    ledger
        .reader_pool
        .as_ref()
        .expect("the reader pool")
        .end_readers_for_test(READERS);
    for reader in 0..READERS {
        let error = ledger
            .resolve_artifact(selection(&frame, head), deadline())
            .expect_err("the resolution whose reader ended");
        assert_eq!(
            named(&error),
            (
                LedgerCode::WriterPanicked.as_str(),
                LedgerLocation::ResolveLedgerArtifact.as_str(),
                false
            ),
            "reader {reader}: a reader's end is not fatal"
        );
    }
    let findings = ledger.take_reader_pool_findings();
    assert_eq!(
        findings.iter().map(named).collect::<Vec<_>>(),
        [
            (
                LedgerCode::WriterPanicked.as_str(),
                LedgerLocation::QueryLedgerView.as_str(),
                false
            ),
            (
                LedgerCode::WriterPanicked.as_str(),
                LedgerLocation::ProjectLedgerViewPage.as_str(),
                false
            ),
        ],
        "the first end and the fallback to the writer are said: {findings:?}"
    );
    assert!(!ledger.reads_off_writer(), "no reader is left");
    let resolved = ledger.resolve_artifact(selection(&frame, head), deadline());
    assert_eq!(answer(&resolved), expected, "the writer answers as it did");
    assert_eq!(
        writer_work_before_append(&ledger, &ids, 2),
        Some(LedgerWriterCommandKind::ResolveArtifact),
        "the writer resolved the artifact"
    );
    ledger.close().expect("close");
}
