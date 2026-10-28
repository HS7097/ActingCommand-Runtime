// SPDX-License-Identifier: AGPL-3.0-only

// #422 W1-8 (brief rd-1028/brief-wave1.md W1-8; #422 6460155516 L-3 and L-5, 6461256427 L-4's
// follow-up, 6463703245): the QR-1 leftovers and the published read point, the ledger's part.
// - T1 (P27, L-3): the writer's eviction proofs are kept in a persistent set: a few shared levels
//   (no more than log2 of the proofs plus two), each artifact's newest proof answered, and a copy
//   taken as a publication takes it keeps the proofs as of then while the set moves on.
// - T2 (P28, L-5): a reader-pool page over records whose artifact has a real eviction proof (its
//   intent and its outcome) is the writer's page byte for byte and carries the eviction; the
//   point published after a commit that changed no proof shares the earlier point's proofs.
// - T3 (P28, L-5): a reader-pool page whose statement bound interrupts a statement outside the
//   page's selection (the medium's check, before it) is `ledger_read_budget_exceeded` of
//   `project_ledger_view_page`, not fatal, as the writer's page is; the pool goes on reading.
// - T4 (P28, L-5): a pass start's trim keeps a capture frame object outside the live listing that
//   a record named after the first chunk's head (`as_of`), in every chunk, and drops one named at
//   or before it.
// - T5 (P29, L-4's follow-up): the pages the reader pool read are counted with their times while
//   the writer runs; a ledger without a pool has none.
// - T6 (P30): the published head answers while the writer is held inside an append's write
//   transaction, at the committed head; once the append committed it names it, as the writer's
//   own head does; a ledger that publishes no point reads the writer's head.
//
// Every bounded read runs on a writer clock that moves only when the test sets a step; nothing
// sleeps and no test reads a clock itself.

use super::missing_material::{capture, formal_root, material_store};
use super::*;
use crate::codes::{LedgerCode, LedgerLocation};
use crate::global::retention::{ProofLevels, RetentionIndex};
use crate::global::{
    ArtifactEvictionAdmission, GlobalLedger, GlobalLedgerError, GlobalLedgerResult,
    LedgerFailurePlan, LedgerMaintenance, LedgerMonotonicClock, LedgerOpenTiming,
    LedgerReaderPoolPages, LedgerWritePolicy, Sha256SecretFingerprinter,
};
use crate::{SyntheticInstance, SyntheticLedgerSpec, WriterStallGate, write_synthetic_ledger};
use actingcommand_artifact_store::try_artifact_delete_guard;
use actingcommand_contract::{
    ArtifactEvictionProof, ArtifactId, ArtifactKind, ArtifactRetentionIdentity, AuditInput,
    EventActor, EventDraft, EventLinksDraft, EventOrigin, EventQuery, EventSeverity, EventSource,
    FRAME_RETENTION_POLICY_VERSION, FactValue, IdentifierIssuer, InstanceId, OriginModule,
    OwnerEpoch, ProjectedArtifactReference, ProjectionProfile, RuntimeEventQueryPage,
    RuntimeEventQueryPageRequest, RuntimeFactRecord, RuntimeFactScope, RuntimePayloadDraft,
    SanitizedEventDraft, TerminalEvent,
};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use actingcommand_runtime_state::RuntimeStateStore;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

// The State key (a bootstrap seed of at least 16 bytes) and the writer's owner id: neutral
// fixture words.
const STATE_KEY: &[u8] = b"ledger-w1-8-fixture-key";
const OWNER: &str = "ledger-w1-8-fixture";
// T3 and T6 read a small synthetic history.
const SMALL_RECORDS: u64 = 60;
// The first record's time (2030-01-01) and the time between two records.
const FIRST_MS: u64 = 1_893_456_000_000;
const RECORD_INTERVAL_MS: u64 = 3_000;
// The writer's write policy and the open's deadline, as QR-1's.
const STALL_MS: u64 = 120_000;
const PROBE_MS: u64 = 1_000;
const DUMP_MAX_BYTES: u64 = 1_073_741_824;
const OPEN_TIMEOUT_SECONDS: u64 = 600;
// Every caller waits this long for one reply; no wait of these tests reaches it.
const REPLY_TIMEOUT: Duration = Duration::from_secs(600);
const PROFILE: ProjectionProfile = ProjectionProfile::Forensic;
// T1: the proofs set, every `T1_RESET_EVERY`th change sets an earlier artifact's proof again, and
// the change after which a publication takes its copy.
const T1_PROOFS: u64 = 1_000;
const T1_RESET_EVERY: u64 = 7;
const T1_PUBLISHED_AT: u64 = 400;
// T3: the read budget, a clock step that spends it at the first look, and a bound that looks at
// the clock at every SQLite step.
const T3_BUDGET: Duration = Duration::from_millis(1_000);
const T3_STEP_MS: u64 = 2_000;
const T3_CHECK_VM_STEPS: u32 = 1;
// T4: the head at the trim's first chunk, the records that named the objects before, at and
// after it, and the objects of each.
const T4_AS_OF: u64 = 7;
const T4_BEFORE: u64 = 5;
const T4_AFTER: u64 = 9;
const T4_OBJECTS: usize = 4;
// T5: the pages read one after another.
const T5_PAGES: u64 = 3;

/// A writer clock in milliseconds that moves the step the test sets each time it is read (none
/// until it sets one).
struct Clock {
    now: Arc<AtomicU64>,
    step: Arc<AtomicU64>,
}

impl Clock {
    fn new() -> Self {
        Self {
            now: Arc::new(AtomicU64::new(0)),
            step: Arc::new(AtomicU64::new(0)),
        }
    }

    fn clock(&self) -> LedgerMonotonicClock {
        let now = Arc::clone(&self.now);
        let step = Arc::clone(&self.step);
        Arc::new(move || now.fetch_add(step.load(Ordering::SeqCst), Ordering::SeqCst))
    }

    fn step_ms(&self, step: u64) {
        self.step.store(step, Ordering::SeqCst);
    }
}

fn instance(letter: char) -> InstanceId {
    serde_json::from_str(&format!("\"instance_{letter:0>32}\"")).expect("a fixture instance id")
}

fn epoch() -> OwnerEpoch {
    *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_owner_epoch()
        .expect("owner epoch")
        .transport()
}

/// A formal root, closed, holding `records` synthetic records (none when 0).
struct Root {
    dir: TempDir,
    database: Arc<RuntimeDatabase>,
}

impl Root {
    fn new(records: u64) -> Self {
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
        if records > 0 {
            let instances = "abc"
                .chars()
                .map(|letter| SyntheticInstance {
                    alias: format!("fixture-instance-{letter}"),
                    instance_id: instance(letter),
                })
                .collect();
            let mut spec = SyntheticLedgerSpec::new(records, instances);
            spec.first_timestamp_unix_ms = FIRST_MS;
            spec.record_interval_ms = RECORD_INTERVAL_MS;
            let generated = write_synthetic_ledger(&database, &spec).expect("the synthetic ledger");
            assert_eq!(generated.records, records, "{generated:?}");
        }
        Self { dir, database }
    }

    /// The Runtime's writer of the root (a failure plan: it has a reader pool and publishes its
    /// points), its reads bounded by `budget` on `clock` when they are given.
    fn open(&self, bounded: Option<(Duration, LedgerMonotonicClock, u32)>) -> GlobalLedger {
        let state_root = self.dir.path().canonicalize().expect("state root");
        let mut plan = LedgerFailurePlan::new(
            LedgerWritePolicy::new(1, vec![0], STALL_MS, PROBE_MS, DUMP_MAX_BYTES)
                .expect("write policy"),
            &state_root,
            None,
            epoch(),
            FIRST_MS,
        )
        .with_reply_timeout(REPLY_TIMEOUT);
        if let Some((budget, clock, check_vm_steps)) = bounded {
            plan = plan
                .with_read_budget(budget)
                .with_clock(clock)
                .with_read_check_vm_steps(check_vm_steps);
        }
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
            "the Runtime's writer reads its pages on its reader pool: {:?}",
            ledger.reader_pool_finding()
        );
        ledger
    }
}

fn first_page(limit: u16) -> RuntimeEventQueryPageRequest {
    RuntimeEventQueryPageRequest::new(limit, None).expect("a first page")
}

/// A view page of every record, read by the reader pool.
fn read_page(ledger: &GlobalLedger) -> GlobalLedgerResult<RuntimeEventQueryPage> {
    ledger.project_view_page(EventQuery::default(), PROFILE, first_page(100))
}

/// An error's code, operation and whether it is fatal.
fn named(error: &GlobalLedgerError) -> (&'static str, &'static str, bool) {
    (error.code(), error.operation(), error.is_fatal())
}

/// A page's bytes, as a client receives them.
fn bytes(page: &RuntimeEventQueryPage) -> Vec<u8> {
    serde_json::to_vec(page).expect("a page serializes")
}

/// A synchronous append as the policy driver makes one: a runtime fact the writer records.
fn runtime_fact(ids: &IdentifierIssuer, value: i64) -> SanitizedEventDraft {
    EventDraft::new(
        ids.mint_event_id().expect("event id"),
        FIRST_MS + RECORD_INTERVAL_MS * (SMALL_RECORDS + 1),
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
                key: "backend.fixture_key_w1_8".to_owned(),
                value: FactValue::Integer(value),
                observed_at_unix_ms: FIRST_MS,
                source: OriginModule::Runtime,
                ttl_ms: None,
            },
            AuditInput::new(),
        )
        .into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"w1-8-fixture-salt").expect("salt"))
    .expect("a fixture draft")
}

/// T1: an eviction proof of a fixture frame, its intent at `sequence`.
fn fixture_proof(ids: &IdentifierIssuer, sequence: u64) -> ArtifactEvictionProof {
    let artifact = *ids.mint_artifact_id().expect("artifact id").transport();
    let correlation = *ids.mint_correlation_id().expect("correlation").transport();
    ArtifactEvictionProof {
        identity: ArtifactRetentionIdentity {
            artifact: ProjectedArtifactReference {
                artifact_id: artifact,
                kind: ArtifactKind::CaptureFrame,
                run_id: None,
                frame_id: Some(*ids.mint_frame_id().expect("frame id").transport()),
                correlation_id: Some(correlation),
                object_key: None,
                media_type: ArtifactKind::CaptureFrame.media_type(),
                byte_count: 1,
                sha256: String::new(),
                created_at_unix_ms: 0,
                producer: actingcommand_contract::ArtifactProducer::CaptureStore,
                retention_class: actingcommand_contract::RetentionClass::Adaptive,
                redaction_state: actingcommand_contract::ArtifactRedactionState::NotRequired,
            },
            owner_epoch: epoch(),
            instance_id: instance('a'),
            request_id: *ids.mint_request_id().expect("request").transport(),
            correlation_id: correlation,
            run_id: None,
            lease_id: None,
            policy_version: FRAME_RETENTION_POLICY_VERSION,
        },
        intent: TerminalEvent {
            sequence,
            event_id: *ids.mint_event_id().expect("event id").transport(),
        },
        terminal_event: None,
        disposition: None,
        through_sequence: sequence,
    }
}

#[test]
fn w1_8_t1_the_proof_set_keeps_few_shared_levels_and_a_published_copy_keeps_its_proofs() {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let artifacts = (0..T1_PROOFS)
        .map(|_| *ids.mint_artifact_id().expect("artifact id").transport())
        .collect::<Vec<ArtifactId>>();
    let mut set = ProofLevels::default();
    // What each artifact's proof is, by the intent sequence that tells the proofs apart.
    let mut expected = BTreeMap::<ArtifactId, u64>::new();
    let mut published = None;
    for change in 1..=T1_PROOFS {
        // Most changes set a new artifact's proof; some set an earlier artifact's again (its
        // outcome on its intent), which the newer level answers.
        let index = if change % T1_RESET_EVERY == 0 {
            usize::try_from(change / 2).expect("an index")
        } else {
            usize::try_from(change - 1).expect("an index")
        };
        let artifact = artifacts[index];
        set.set(artifact, fixture_proof(&ids, change));
        expected.insert(artifact, change);
        if change == T1_PUBLISHED_AT {
            published = Some((set.clone(), expected.clone()));
        }
    }
    let bound = usize::try_from(T1_PROOFS.ilog2()).expect("a level bound") + 2;
    assert!(
        set.level_count() <= bound,
        "{} levels for {T1_PROOFS} changes, more than {bound}",
        set.level_count()
    );
    assert_eq!(
        set.artifacts(),
        expected.keys().copied().collect::<BTreeSet<_>>()
    );
    for (artifact, sequence) in &expected {
        assert_eq!(
            set.get(artifact).map(|proof| proof.intent.sequence),
            Some(*sequence),
            "the newest proof of {artifact:?}"
        );
    }
    // The copy a publication took keeps the proofs as of then, whatever the set did since.
    let (published, then) = published.expect("the publication's copy");
    for artifact in &artifacts {
        assert_eq!(
            published.get(artifact).map(|proof| proof.intent.sequence),
            then.get(artifact).copied(),
            "the published proof of {artifact:?}"
        );
    }
}

#[test]
fn w1_8_t2_a_pool_page_with_real_eviction_proofs_is_the_writers_and_shares_unchanged_proofs() {
    let root = Root::new(0);
    let ledger = root.open(None);
    let artifacts = material_store(root.dir.path());
    let kept = capture(&ledger, &artifacts, b"frame kept without a proof");
    let evicted = capture(&ledger, &artifacts, b"frame evicted with its proof");
    let guard = try_artifact_delete_guard(artifacts.root(), &evicted.reference)
        .expect("delete guard")
        .expect("material guard is free");
    let permit = match ledger.admit_artifact_eviction(guard).expect("admission") {
        ArtifactEvictionAdmission::Committed(permit) => permit,
        ArtifactEvictionAdmission::Deferred => panic!("the closed capture is admitted"),
    };
    let outcome = ledger
        .finish_artifact_eviction(permit)
        .expect("recorded deletion");
    let head = ledger.latest_sequence().expect("head");
    let terminal = TerminalEvent {
        sequence: outcome.sequence(),
        event_id: *outcome.event_id(),
    };

    // The pool's page is the writer's, byte for byte, and its rows carry the real eviction.
    let writers = ledger
        .project_view_page_on_writer_for_test(EventQuery::default(), PROFILE, first_page(100))
        .expect("the writer's page");
    let page = read_page(&ledger).expect("the pool's page");
    assert_eq!(page.snapshot_ledger_position(), head);
    assert_eq!(
        bytes(&page),
        bytes(&writers),
        "the pool's page is the writer's"
    );
    let evictions = page
        .events()
        .iter()
        .flat_map(|row| row.artifact_evictions.iter())
        .collect::<Vec<_>>();
    assert!(
        !evictions.is_empty(),
        "a row of the page carries the eviction"
    );
    for eviction in &evictions {
        assert_eq!(eviction.artifact_id, evicted.reference.artifact_id);
        assert_eq!(eviction.terminal_event, Some(terminal), "{eviction:?}");
        assert!(eviction.disposition.is_some(), "{eviction:?}");
    }

    // The published point holds the evicted artifact's proof and none of the kept one.
    let points = ledger.read_points.as_ref().expect("the writer publishes");
    let before = points.current().expect("a published point");
    assert_eq!(before.head, head);
    let published = before
        .proofs
        .proof(&evicted.reference.artifact_id)
        .expect("the evicted artifact's published proof");
    assert_eq!(published.terminal_event, Some(terminal));
    assert!(before.proofs.proof(&kept.reference.artifact_id).is_none());
    // A commit that changes no proof publishes a new head with the same proofs, shared.
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    ledger
        .append(runtime_fact(&ids, 1))
        .expect("an append commits");
    let after = points.current().expect("a published point");
    assert_eq!(after.head, head + 1);
    assert!(
        Arc::ptr_eq(&before.proofs, &after.proofs),
        "the unchanged proofs are the earlier point's"
    );
    ledger.close().expect("close");
}

#[test]
fn w1_8_t3_a_pool_page_its_bound_interrupts_outside_the_selection_is_budget_exceeded() {
    let root = Root::new(SMALL_RECORDS);
    let clock = Clock::new();
    let ledger = root.open(Some((T3_BUDGET, clock.clock(), T3_CHECK_VM_STEPS)));
    let expected = (
        LedgerCode::LedgerReadBudgetExceeded.as_str(),
        LedgerLocation::ProjectLedgerViewPage.as_str(),
        false,
    );
    // From now on the budget is spent at the read's first look at the clock, which is inside the
    // first statement of the page's check of the medium, before its selection.
    clock.step_ms(T3_STEP_MS);
    let error = read_page(&ledger).expect_err("the pool's page is refused");
    assert_eq!(named(&error), expected, "the pool's page: {error:?}");
    let error = ledger
        .project_view_page_on_writer_for_test(EventQuery::default(), PROFILE, first_page(100))
        .expect_err("the writer's page is refused");
    assert_eq!(named(&error), expected, "the writer's page: {error:?}");
    // Nothing ended: the pool still reads, and its next page within its budget answers.
    clock.step_ms(0);
    assert!(ledger.reads_off_writer(), "the pool still reads the pages");
    assert!(ledger.take_reader_pool_findings().is_empty());
    let page = read_page(&ledger).expect("the next page answers");
    assert_eq!(
        page.snapshot_ledger_position(),
        ledger.latest_sequence().expect("head")
    );
    ledger.close().expect("close");
}

#[test]
fn w1_8_t4_a_trim_keeps_an_object_a_record_named_after_its_first_chunks_head() {
    let mut index = RetentionIndex::default();
    let before = index.seed_frame_objects_for_test(T4_OBJECTS, T4_BEFORE);
    let at = index.seed_frame_objects_for_test(T4_OBJECTS, T4_AS_OF);
    let after = index.seed_frame_objects_for_test(T4_OBJECTS, T4_AFTER);
    // No object is in the live listing; the trim's first chunk read the head T4_AS_OF, and each
    // chunk looks at one object, so every object is looked at by a chunk of its own.
    let live = BTreeSet::new();
    let mut position = (None, T4_AS_OF);
    let mut looked = 0;
    let mut dropped = 0;
    loop {
        let chunk = index.retain_live_objects_chunk(&live, position, 1);
        looked += chunk.looked;
        dropped += chunk.dropped;
        let Some(next) = chunk.next else {
            break;
        };
        assert_eq!(
            next.as_of(),
            T4_AS_OF,
            "every chunk keeps the first one's head"
        );
        position = (Some(next.after()), next.as_of());
    }
    assert_eq!(
        looked,
        before.len() + at.len() + after.len(),
        "every object was looked at"
    );
    assert_eq!(
        dropped,
        before.len() + at.len(),
        "the objects named at or before the head were dropped"
    );
    // What is left is exactly the objects named after the head: a trim at a later head whose
    // listing holds them drops nothing, and one whose listing does not drops them all.
    let named_after = after.iter().copied().collect::<BTreeSet<_>>();
    let kept = index.retain_live_objects_chunk(&named_after, (None, T4_AFTER), usize::MAX);
    assert_eq!(
        (kept.looked, kept.dropped, kept.next),
        (after.len(), 0, None)
    );
    let later = index.retain_live_objects_chunk(&live, (None, T4_AFTER), usize::MAX);
    assert_eq!((later.looked, later.dropped), (after.len(), after.len()));
}

#[test]
fn w1_8_t5_the_pool_pages_are_counted_with_their_times_while_the_writer_runs() {
    let root = Root::new(0);
    let ledger = root.open(None);
    assert_eq!(
        ledger.reader_pool_pages(),
        Some(LedgerReaderPoolPages::default()),
        "no page read yet"
    );
    for read in 0..T5_PAGES {
        read_page(&ledger).unwrap_or_else(|error| panic!("page {read}: {error:?}"));
    }
    let pages = ledger.reader_pool_pages().expect("the pool's pages");
    assert_eq!(pages.pages, T5_PAGES, "{pages:?}");
    assert!(
        pages.last <= pages.longest && pages.longest <= pages.total,
        "{pages:?}"
    );
    ledger.close().expect("close");

    // A writer without a failure plan has no pool, and no pages.
    let dir = tempfile::tempdir().expect("root");
    let (_database, ledger) = formal_root(dir.path());
    assert!(ledger.reader_pool_pages().is_none());
    ledger.close().expect("close");
}

#[test]
fn w1_8_t6_the_published_head_answers_while_the_writer_is_held_and_names_each_commit() {
    let root = Root::new(SMALL_RECORDS);
    let ledger = root.open(None);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let head = ledger.latest_sequence().expect("head");
    assert_eq!(ledger.published_head().expect("published head"), head);
    let stall = WriterStallGate::arm(root.dir.path()).expect("writer stall gate");
    let writer = &ledger;
    let appended = thread::scope(|scope| {
        let draft = runtime_fact(&ids, 1);
        let appending = scope.spawn(move || writer.append(draft));
        assert!(
            stall.wait_held_commit(BARRIER_WAIT),
            "the writer is held inside the append's write transaction"
        );
        // The writer takes no command now; the published head answers without one, at the head
        // committed before the held append.
        assert_eq!(
            writer.published_head().expect("published head while held"),
            head
        );
        stall.release();
        appending.join().expect("the append's thread")
    })
    .expect("the held append commits once released");
    assert_eq!(appended.sequence(), head + 1);
    assert_eq!(
        ledger.published_head().expect("published head"),
        head + 1,
        "the answered append is in the published head"
    );
    assert_eq!(ledger.latest_sequence().expect("head"), head + 1);
    ledger.close().expect("close");

    // A writer that publishes no point (no failure plan) reads its own head.
    let dir = tempfile::tempdir().expect("root");
    let (_database, ledger) = formal_root(dir.path());
    assert!(ledger.read_points.is_none());
    assert_eq!(
        ledger.published_head().expect("published head"),
        ledger.latest_sequence().expect("head")
    );
    ledger.close().expect("close");
}
