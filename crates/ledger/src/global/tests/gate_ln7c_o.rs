// SPDX-License-Identifier: AGPL-3.0-only

// #422 LN-7c O (rd-1026/ln7c/final-query.md §7 row O and §2; brief rd-1026/brief-ln7c-f6ro.md): the
// writer open reads the owner records of the two latest owner epochs, and of the older owners a
// start may still close, instead of every owner record of the history.
// - O1: the owner state the open keeps (the retained owners, the owners a next start closes, and of
//   each retained owner its epochs, end, reopening, import, the closes of its scopes, their writer,
//   its quiescences, scope sources and releases) equals the state of the read of every owner record
//   (the open's read before O) on synthetic roots with many starts, a takeover, a dump import of
//   the previous owner, a reopened owner and a prior-epoch close; a root whose latest owner wrote
//   no start record is read whole, and the open says why.
// - O2: the owner rows the open reads lie in the two latest owner epochs (with the short read back
//   to the first record of the epoch before the latest) and are the same on histories of 3 000,
//   12 000 and 30 000 records (`LedgerReadCounts`), while the read of every owner record grows
//   with the history.
// - O3 (fix round 1, from the independent review `rd-1026/ln7c/review-o.md` §5): on a
//   production-shaped root (live frames in four epochs, lease releases and run terminals, a
//   takeover, a dump import of the previous owner), the bounded open's frame view (LN-7b's view
//   and its kept moves) and the records a real startup reconcile then writes are those of the read
//   of every owner record; a dump present at the open, the previous owner in no start's epoch and
//   an epoch with no owner record after its start make the open read every owner record and say
//   why.
//
// An owner epoch here is laid out as a start writes it: its first owner record (as the start's
// capacity sample), its prior-epoch imports and closes, its start record, its records, and its last
// owner record (as a shutdown's) unless it stopped. The roots are real formal SQLite roots whose
// records are written straight into their tables (`SyntheticAppender`, LN-1), each a valid
// sanitized record at its sequence.

use super::missing_material::material_store;
use crate::codes::{LedgerCode, LedgerLocation};
use crate::global::retention::{
    OwnerState, read_every_owner_record_for_test, take_opened_owner_state_for_test,
};
use crate::global::{
    FrameRetentionSwitches, FrameRetentionView, GlobalLedger, KeptFrameFolder, LedgerFailurePlan,
    LedgerMaintenance, LedgerOpenTiming, LedgerWritePolicy, OwnerReadFull,
    Sha256SecretFingerprinter, SyntheticAppender,
};
use crate::owner_journal::RuntimeOwnerJournal;
use crate::{LedgerReadCounter, PersistedEvent};
use actingcommand_artifact_store::{
    ArtifactEventSink, ArtifactStore, ArtifactStoreResult, ArtifactWriteContext,
    ArtifactWriteRequest,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    ArtifactId, ArtifactIssuePolicy, ArtifactKind, ArtifactLinksDraft, ArtifactPayloadDraft,
    ArtifactPinReason, ArtifactPinRecord, ArtifactProducer, ArtifactRedactionState,
    ArtifactRetentionFact, ArtifactRetentionIdentity, AuditInput, CapturePayloadDraft,
    CapturePersistedEvidence, CaptureSummaryRecord, CommandPayloadDraft, DiagnosticCode,
    DiagnosticDetailDraft, EffectDisposition, EventAction, EventActor, EventDraft, EventLinksDraft,
    EventOrigin, EventPayloadDraft, EventQuery, EventSeverity, EventSource, EventType,
    EvidenceCompleteness, FRAME_RETENTION_POLICY_VERSION, FactContent, FactPayloadDraft,
    FactRecord, FactScope, FactValue, IdentifierIssuer, IssuedCorrelationId, IssuedInstanceId,
    IssuedRequestId, IssuedRunId, LeasePayloadDraft, LedgerPayloadDraft, OWNER_JOURNAL_SCHEMA,
    OriginModule, OwnerEpoch, OwnerEpochCloseEvidence, OwnerResourceDisposition,
    PriorEpochCloseBasis, PriorEpochCloseFact, PriorEpochOwnerImport, PriorEpochScope,
    PriorEpochScopeClose, ProjectedArtifactReference, RESOURCE_READING_DETECTOR_PREFIX,
    ResourceQuiescence, RetentionClass, RuntimeLifecyclePhase, RuntimePayloadDraft,
    SanitizedEventDraft, TaskOutcome, TaskPayloadDraft, TaskSemanticFact, TerminalEvent,
    UnprovenReason, resource_reading_snapshot_id,
};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use actingcommand_runtime_state::RuntimeStateStore;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

// The State key (a bootstrap seed of at least 16 bytes), the writer's owner id and the fingerprint
// salt: neutral fixture words.
const STATE_KEY: &[u8] = b"ledger-ln7c-o-fixture";
const OWNER: &str = "ledger-ln7c-o-fixture";
const SALT: &[u8] = b"ledger-ln7c-o-fixture-salt";
// The records' time: one millisecond per sequence after it.
const AT_MS: u64 = 1_752_147_200_000;
// Records per appended transaction.
const BATCH_RECORDS: usize = 2_000;
// O1: the records of an epoch's middle.
const BODY_RECORDS: u64 = 24;
// O2: the records of one epoch, and the lengths of the three histories (the last ten times the
// first).
const EPOCH_RECORDS: u64 = 150;
const HISTORIES: [u64; 3] = [3_000, 12_000, 30_000];
// O2's opens authenticate a short tail and read pages of 16 rows, so the read back from the start
// before the latest one begins with 16 sequences.
const TAIL: usize = 16;
const PAGE_ROWS: usize = 16;
const STALL_MS: u64 = 120_000;
const PROBE_MS: u64 = 1_000;
const DUMP_MAX_BYTES: u64 = 1_073_741_824;

fn runtime() -> EventOrigin {
    EventOrigin::new(
        EventSource::Runtime,
        OriginModule::Runtime,
        EventActor::Runtime,
    )
}

// The types whose records can name an owner epoch (the open's `OWNER_TYPES`).
fn owner_type(event_type: EventType) -> bool {
    matches!(
        event_type,
        EventType::RuntimeLifecycleObserved
            | EventType::PerformanceSummary
            | EventType::CommandValidated
            | EventType::MonitorProbeCompleted
            | EventType::MonitorProbeFailed
    )
}

// The records of a synthetic history from sequence `first` on.
struct History {
    ids: IdentifierIssuer,
    fingerprinter: Sha256SecretFingerprinter,
    instance: IssuedInstanceId,
    first: u64,
    events: Vec<PersistedEvent>,
    // The sequences of the start records.
    starts: Vec<u64>,
}

impl History {
    fn new(first: u64) -> Self {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let instance = ids.mint_instance_id().expect("instance");
        Self {
            ids,
            fingerprinter: Sha256SecretFingerprinter::new(SALT).expect("fingerprinter"),
            instance,
            first,
            events: Vec::new(),
            starts: Vec::new(),
        }
    }

    fn owner_epoch(&self) -> OwnerEpoch {
        *self
            .ids
            .mint_owner_epoch()
            .expect("owner epoch")
            .transport()
    }

    fn next(&self) -> u64 {
        self.first + self.events.len() as u64
    }

    fn push(
        &mut self,
        origin: EventOrigin,
        links: EventLinksDraft,
        payload: EventPayloadDraft,
    ) -> TerminalEvent {
        let sequence = self.next();
        let draft = EventDraft::new(
            self.ids.mint_event_id().expect("event id"),
            AT_MS + sequence,
            EventSeverity::Info,
            origin,
            links,
            payload,
        )
        .sanitize(&self.fingerprinter)
        .expect("a fixture record");
        let event = PersistedEvent::from_sanitized(sequence, draft).expect("a record at its place");
        let terminal = TerminalEvent {
            sequence,
            event_id: *event.event_id(),
        };
        self.events.push(event);
        terminal
    }

    // A lifecycle observation of `owner` (as a capacity sample or a shutdown names its owner).
    fn owner(&mut self, owner: OwnerEpoch) -> u64 {
        self.push(
            runtime(),
            EventLinksDraft::default(),
            RuntimePayloadDraft::lifecycle_observed(
                owner,
                RuntimeLifecyclePhase::ShutdownRequested,
                AuditInput::new(),
            )
            .into(),
        )
        .sequence
    }

    // A confirmed resource quiescence of the fixture instance in `owner`'s epoch, in a request
    // and correlation of its own.
    fn quiescence(&mut self, owner: OwnerEpoch) {
        let instance = self.instance;
        let links = EventLinksDraft::default()
            .with_instance_id(instance)
            .with_request_id(self.ids.mint_request_id().expect("request"))
            .with_correlation_id(self.ids.mint_correlation_id().expect("correlation"));
        let payload = RuntimePayloadDraft::lifecycle_observed(
            owner,
            RuntimeLifecyclePhase::ResourceQuiescence {
                instance_id: *instance.transport(),
                resource_count: 1,
                quiescence: ResourceQuiescence::Confirmed,
                owner_disposition: OwnerResourceDisposition::ConfirmedClosed,
                resource_dispositions: None,
            },
            AuditInput::new(),
        );
        self.push(runtime(), links, payload.into());
    }

    // A record that names no owner (`command.received`).
    fn plain(&mut self) {
        self.push(
            EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::User),
            EventLinksDraft::default(),
            CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into(),
        );
    }

    // A record of an owner type that names no owner (`command.validated` without a state).
    fn unowned(&mut self) {
        self.push(
            runtime(),
            EventLinksDraft::default(),
            CommandPayloadDraft::validated(
                EventAction::RuntimeTaskRun,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            )
            .into(),
        );
    }

    // A start record (`runtime.takeover` when `takeover`).
    fn start(&mut self, takeover: bool) {
        let payload = if takeover {
            RuntimePayloadDraft::takeover(EventAction::RuntimeTakeover, AuditInput::new())
        } else {
            RuntimePayloadDraft::started(EventAction::RuntimeStart, AuditInput::new())
        };
        let start = self.push(runtime(), EventLinksDraft::default(), payload.into());
        self.starts.push(start.sequence);
    }

    // `records` records of `owner`'s epoch: plain records, owner-type records without an owner,
    // quiescences and owner records.
    fn body(&mut self, owner: OwnerEpoch, records: u64) {
        for index in 0..records {
            match index % 5 {
                0 | 3 => self.plain(),
                1 => self.unowned(),
                2 => self.quiescence(owner),
                _ => {
                    self.owner(owner);
                }
            }
        }
    }

    // A whole epoch of `owner` of `records` records: its first owner record, its start, its body
    // and its last owner record. Returns its first sequence.
    fn epoch(&mut self, owner: OwnerEpoch, records: u64) -> u64 {
        let first = self.owner(owner);
        self.start(false);
        self.body(owner, records - 3);
        self.owner(owner);
        first
    }

    // A run's first record in `owner`'s epoch, the source of its scope; nothing closes it.
    fn open_run(&mut self, owner: OwnerEpoch) -> (PriorEpochScope, TerminalEvent) {
        let instance = self.instance;
        let run = self.ids.mint_run_id().expect("run");
        let links = EventLinksDraft::default()
            .with_instance_id(instance)
            .with_request_id(self.ids.mint_request_id().expect("request"))
            .with_correlation_id(self.ids.mint_correlation_id().expect("correlation"))
            .with_task_id(self.ids.mint_task_id().expect("task"))
            .with_run_id(run);
        let source = self.push(
            runtime(),
            links,
            TaskPayloadDraft::started(EventAction::RuntimeTaskRun, AuditInput::new()).into(),
        );
        let scope = PriorEpochScope {
            owner,
            instance: *instance.transport(),
            run: Some(*run.transport()),
            lease: None,
            request: None,
            correlation: None,
        };
        (scope, source)
    }

    // A prior-epoch fact `writer` records, as the ledger's own start record does.
    fn prior_epoch(&mut self, writer: OwnerEpoch, fact: PriorEpochCloseFact) -> TerminalEvent {
        let links = EventLinksDraft::default()
            .with_request_id(self.ids.mint_request_id().expect("request"))
            .with_correlation_id(self.ids.mint_correlation_id().expect("correlation"))
            .with_action_id(self.ids.mint_action_id().expect("action"));
        self.push(
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::GlobalLedger,
                EventActor::Runtime,
            ),
            links,
            RuntimePayloadDraft::prior_epoch_close(writer, fact).into(),
        )
    }

    // `writer`'s unproven import of `subject`, whose epoch ended at `upper`: the import and the
    // prefix it saw.
    fn import(
        &mut self,
        writer: OwnerEpoch,
        subject: OwnerEpoch,
        upper: u64,
    ) -> (TerminalEvent, u64) {
        let through = self.next() - 1;
        let import = PriorEpochOwnerImport {
            evidence: OwnerEpochCloseEvidence {
                schema_version: OWNER_JOURNAL_SCHEMA.to_owned(),
                subject,
                pid: 0,
                started_at_unix_ms: 0,
                first_revision: 0,
                confirmed_revision: 0,
                final_revision: 0,
                journal_through_revision: 0,
                journal_bytes: 0,
                journal_sha256: "0".repeat(64),
                suffix: Vec::new(),
            },
            basis: unproven(),
            through_sequence: through,
            scope_upper_sequence: upper,
        };
        let recorded = self.prior_epoch(writer, PriorEpochCloseFact::OwnerImported(import));
        (recorded, through)
    }

    // `writer`'s close of `scope`, whose source is `source`, by `import`.
    fn close(
        &mut self,
        writer: OwnerEpoch,
        (scope, source): (PriorEpochScope, TerminalEvent),
        import: (TerminalEvent, u64),
        upper: u64,
    ) {
        let close = PriorEpochScopeClose {
            subject: scope,
            proof: import.0,
            scope_source: source,
            through_sequence: import.1,
            scope_upper_sequence: upper,
            basis: unproven(),
        };
        self.prior_epoch(writer, PriorEpochCloseFact::ScopeClosed(close));
    }

    // The ledger's record of a dump it imported, after the dump's `count` records.
    fn dump_imported(&mut self, count: u64) {
        let outcome = Outcome::new(LedgerCode::LedgerDumpImported)
            .count(count)
            .duplicate_count(0)
            .dump_sealed(true)
            .dump_sha256(format!("sha256:{}", "0".repeat(64)));
        self.push(
            EventOrigin::new(
                EventSource::System,
                OriginModule::GlobalLedger,
                EventActor::System,
            ),
            EventLinksDraft::default(),
            LedgerPayloadDraft::dump_imported(outcome, AuditInput::new()).into(),
        );
    }
}

fn unproven() -> PriorEpochCloseBasis {
    PriorEpochCloseBasis::Unproven {
        reason: UnprovenReason::ProofMissing,
    }
}

// A closed formal root holding a synthetic history.
struct Root {
    dir: TempDir,
    database: Arc<RuntimeDatabase>,
    starts: Vec<u64>,
    // The sequences of the records of an owner type.
    owner_rows: BTreeSet<u64>,
    records: u64,
}

impl Root {
    // A root with the history `build` writes, and what `build` returns.
    fn write<T>(build: impl FnOnce(&mut History) -> T) -> (Self, T) {
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
        let maintenance = LedgerMaintenance::acquire(
            dir.path(),
            false,
            limits,
            limits.deadline().expect("deadline"),
        )
        .expect("maintenance owner");
        let mut appender = SyntheticAppender::open(&database).expect("the root's appender");
        let mut history = History::new(appender.next_sequence().expect("the next sequence"));
        let built = build(&mut history);
        for batch in history.events.chunks(BATCH_RECORDS) {
            appender.append(batch).expect("a batch of fixture records");
        }
        drop(appender);
        maintenance.close().expect("release the root");
        let owner_rows = history
            .events
            .iter()
            .filter(|event| owner_type(event.event_type()))
            .map(PersistedEvent::sequence)
            .collect();
        let root = Self {
            dir,
            database,
            starts: history.starts,
            owner_rows,
            records: history.events.len() as u64,
        };
        (root, built)
    }
}

// The owner state of a start's writer open of `root`, and whether it read every owner record
// because it could not bound its read; with `every`, the open reads every owner record (the
// reference).
fn opened(root: &Root, every: bool) -> (OwnerState, bool) {
    read_every_owner_record_for_test(every);
    let limits = MaintenanceLimits::default();
    let opened = LedgerMaintenance::acquire(
        root.dir.path(),
        false,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("maintenance owner")
    .open_writer(Arc::clone(&root.database), OWNER.into());
    read_every_owner_record_for_test(false);
    let ledger = opened.expect("the writer opens");
    let state = take_opened_owner_state_for_test().expect("the open kept its owner state");
    ledger.close().expect("close the writer");
    state
}

// The bounded open's owner state of `root`, after checking that it equals the full read's.
fn compared(name: &str, root: &Root) -> (OwnerState, bool) {
    let (reference, fell_back) = opened(root, true);
    assert!(
        !fell_back,
        "{name}: the reference reads every owner record by itself"
    );
    let bounded = opened(root, false);
    assert_eq!(
        bounded.0, reference,
        "{name}: the bounded read keeps the owner state of the read of every owner record"
    );
    bounded
}

#[test]
fn ln7c_o1_the_open_keeps_the_owner_state_of_the_read_of_every_owner_record() {
    // Many starts: eight epochs, each closed.
    let (root, (owners, firsts)) = Root::write(|history| {
        let mut owners = Vec::new();
        let mut firsts = Vec::new();
        for _ in 0..8 {
            let owner = history.owner_epoch();
            firsts.push(history.epoch(owner, BODY_RECORDS + 3));
            owners.push(owner);
        }
        (owners, firsts)
    });
    let (state, full) = compared("many starts", &root);
    assert!(!full, "many starts: the owner read is bounded");
    let (latest, previous) = (owners[7], owners[6]);
    assert_eq!(state.retained, BTreeSet::from([latest, previous]));
    assert_eq!(state.next_closes, BTreeSet::from([latest]));
    let facts = &state.owners[&previous];
    assert_eq!(facts.epochs, vec![(firsts[6], firsts[7] - 1)]);
    assert_eq!(facts.end, Some(firsts[7] - 1));
    assert!(!facts.reopened && facts.import.is_none() && facts.closes.is_empty());
    assert!(facts.writer.is_none() && facts.released_leases.is_empty());
    assert!(
        !facts.quiescences.is_empty() && !facts.scope_sources.is_empty(),
        "many starts: the previous epoch's quiescences and scopes are kept"
    );
    assert_eq!(state.owners[&latest].end, None);

    // A takeover: the epoch before the latest stopped without its last record, and the latest
    // start took over.
    let (root, (stopped, latest, firsts)) = Root::write(|history| {
        for _ in 0..5 {
            let owner = history.owner_epoch();
            history.epoch(owner, BODY_RECORDS + 3);
        }
        let stopped = history.owner_epoch();
        let stopped_first = history.owner(stopped);
        history.start(false);
        history.body(stopped, BODY_RECORDS);
        let latest = history.owner_epoch();
        let latest_first = history.owner(latest);
        history.start(true);
        history.body(latest, BODY_RECORDS);
        history.owner(latest);
        (stopped, latest, [stopped_first, latest_first])
    });
    let (state, full) = compared("a takeover", &root);
    assert!(!full, "a takeover: the owner read is bounded");
    assert_eq!(state.retained, BTreeSet::from([stopped, latest]));
    assert_eq!(
        state.owners[&stopped].epochs,
        vec![(firsts[0], firsts[1] - 1)]
    );

    // A dump import of the previous owner: the start of the latest owner imported the dump the
    // previous one left, its records then `ledger.dump_imported`, before its own first record.
    let (root, (previous, dumped, firsts)) = Root::write(|history| {
        for _ in 0..5 {
            let owner = history.owner_epoch();
            history.epoch(owner, BODY_RECORDS + 3);
        }
        let previous = history.owner_epoch();
        let previous_first = history.owner(previous);
        history.start(false);
        history.body(previous, BODY_RECORDS);
        let dumped = history.owner(previous);
        history.quiescence(previous);
        history.owner(previous);
        history.dump_imported(3);
        let latest = history.owner_epoch();
        let latest_first = history.epoch(latest, BODY_RECORDS + 3);
        (previous, dumped, [previous_first, latest_first])
    });
    let (state, full) = compared("a dump import of the previous owner", &root);
    assert!(!full, "a dump import: the owner read is bounded");
    let facts = &state.owners[&previous];
    assert_eq!(facts.epochs, vec![(firsts[0], firsts[1] - 1)]);
    assert!(
        facts.end > Some(dumped),
        "the previous epoch ends after its imported records"
    );

    // A reopened owner: an owner's dump is imported only by the start after the next one, so its
    // records follow another epoch; it is the owner before the latest, and reopened.
    let (root, (reopened, firsts)) = Root::write(|history| {
        for _ in 0..4 {
            let owner = history.owner_epoch();
            history.epoch(owner, BODY_RECORDS + 3);
        }
        let owner = history.owner_epoch();
        let first = history.owner(owner);
        history.start(false);
        history.body(owner, BODY_RECORDS);
        let next = history.owner_epoch();
        let next_first = history.epoch(next, BODY_RECORDS + 3);
        let dumped = history.owner(owner);
        history.quiescence(owner);
        history.owner(owner);
        history.dump_imported(3);
        let latest = history.owner_epoch();
        let latest_first = history.epoch(latest, BODY_RECORDS + 3);
        (owner, [first, next_first, dumped, latest_first])
    });
    let (state, full) = compared("a reopened owner", &root);
    assert!(!full, "a reopened owner: the owner read is bounded");
    assert!(state.retained.contains(&reopened));
    let facts = &state.owners[&reopened];
    assert!(facts.reopened, "the owner is reopened");
    assert_eq!(
        facts.epochs,
        vec![(firsts[0], firsts[1] - 1), (firsts[2], firsts[3] - 1)]
    );

    // A prior-epoch close: two epochs in turn stop with a run open; each next start imports the
    // owner before it and closes that run's scope before its start record. The epoch before the
    // previous one stays retained (the previous one wrote its import), so it is read back too.
    let (root, [older, previous, latest]) = Root::write(|history| {
        for _ in 0..4 {
            let owner = history.owner_epoch();
            history.epoch(owner, BODY_RECORDS + 3);
        }
        let older = history.owner_epoch();
        history.owner(older);
        history.start(false);
        let older_run = history.open_run(older);
        history.body(older, BODY_RECORDS);
        let previous = history.owner_epoch();
        let first = history.owner(previous);
        let import = history.import(previous, older, first - 1);
        history.close(previous, older_run, import, first - 1);
        history.start(false);
        let previous_run = history.open_run(previous);
        history.body(previous, BODY_RECORDS);
        let latest = history.owner_epoch();
        let first = history.owner(latest);
        let import = history.import(latest, previous, first - 1);
        history.close(latest, previous_run, import, first - 1);
        history.start(false);
        history.body(latest, BODY_RECORDS);
        history.owner(latest);
        [older, previous, latest]
    });
    let (state, full) = compared("a prior-epoch close", &root);
    assert!(!full, "a prior-epoch close: the owner read is bounded");
    assert_eq!(state.retained, BTreeSet::from([older, previous, latest]));
    assert_eq!(state.next_closes, BTreeSet::from([previous, latest]));
    for (subject, writer) in [(older, previous), (previous, latest)] {
        let facts = &state.owners[&subject];
        assert!(facts.import.is_some(), "the import of each closed owner");
        assert_eq!(facts.closes.len(), 1, "the close of each open run");
        assert_eq!(facts.writer, Some(writer));
    }

    // An owner in no start's epoch: the latest owner's start stopped before its start record.
    let (root, ()) = Root::write(|history| {
        for _ in 0..5 {
            let owner = history.owner_epoch();
            history.epoch(owner, BODY_RECORDS + 3);
        }
        let stopped = history.owner_epoch();
        history.owner(stopped);
        history.body(stopped, 4);
    });
    let (_, full) = compared("an owner in no start's epoch", &root);
    assert!(full, "the open reads every owner record and says so");
}

// A start's writer open of `root` with a tail of `TAIL` records and pages of `PAGE_ROWS` rows,
// counted on this thread: the sequences of the owner-type rows it read before its tail, and the
// first sequence of that tail. With `every`, the open reads every owner record (the reference).
fn owner_rows_read(root: &Root, every: bool) -> (BTreeSet<u64>, u64) {
    let state_root = root.dir.path().canonicalize().expect("state root");
    let epoch = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_owner_epoch()
        .expect("owner epoch")
        .transport();
    let plan = LedgerFailurePlan::new(
        LedgerWritePolicy::new(1, vec![0], STALL_MS, PROBE_MS, DUMP_MAX_BYTES)
            .expect("write policy"),
        &state_root,
        None,
        epoch,
        AT_MS,
    )
    .with_open_verify_tail_records(TAIL)
    .with_history_page_rows(PAGE_ROWS);
    read_every_owner_record_for_test(every);
    let counter = LedgerReadCounter::start();
    let opened: Result<GlobalLedger, _> = LedgerMaintenance::acquire(
        root.dir.path(),
        false,
        MaintenanceLimits::default(),
        MaintenanceLimits::default().deadline().expect("deadline"),
    )
    .and_then(|maintenance| {
        maintenance.open_writer_timed(
            Arc::clone(&root.database),
            OWNER.into(),
            Some(plan),
            &mut LedgerOpenTiming::default(),
        )
    });
    let sequences = counter.sequences_read();
    drop(counter);
    read_every_owner_record_for_test(false);
    let ledger = opened.expect("the writer opens");
    let _ = take_opened_owner_state_for_test();
    let head = ledger.latest_sequence().expect("head");
    ledger.close().expect("close the writer");
    let tail = head + 1 - TAIL as u64;
    let read = sequences
        .events
        .into_iter()
        .filter(|sequence| *sequence < tail && root.owner_rows.contains(sequence))
        .collect();
    (read, tail)
}

#[test]
fn ln7c_o2_the_owner_rows_read_at_open_stay_in_the_two_latest_epochs_whatever_the_history() {
    let mut bounded = Vec::new();
    for records in HISTORIES {
        let (root, firsts) = Root::write(|history| {
            (0..records / EPOCH_RECORDS)
                .map(|_| {
                    let owner = history.owner_epoch();
                    history.epoch(owner, EPOCH_RECORDS)
                })
                .collect::<Vec<_>>()
        });
        assert_eq!(root.records, records, "the history holds {records} records");
        let starts = root.starts.len();
        assert_eq!(starts, firsts.len(), "one start per epoch");
        let (read, tail) = owner_rows_read(&root, false);
        let (every, every_tail) = owner_rows_read(&root, true);
        // Both opens are compared before the tail of either.
        let tail = tail.min(every_tail);
        let read = read.range(..tail).copied().collect::<BTreeSet<_>>();
        let every = every.range(..tail).copied().collect::<BTreeSet<_>>();
        // The two latest epochs begin at the previous epoch's first record; the read back to it
        // starts at most one page of sequences before it.
        let previous = firsts[firsts.len() - 2];
        let below = previous - PAGE_ROWS as u64;
        assert!(
            read.iter().all(|sequence| *sequence >= below),
            "{records} records: every owner row read is in the two latest epochs or the read back \
             to them (from {below}): {read:?}"
        );
        assert!(
            root.owner_rows
                .range(previous..tail)
                .all(|sequence| read.contains(sequence)),
            "{records} records: every owner row of the two latest epochs is read"
        );
        assert_eq!(
            every,
            root.owner_rows
                .range(..tail)
                .copied()
                .collect::<BTreeSet<_>>(),
            "{records} records: the reference reads every owner row"
        );
        assert!(
            read.len() < every.len(),
            "{records} records: {} owner rows read, {} by the reference",
            read.len(),
            every.len()
        );
        bounded.push((records, read.len(), every.len()));
    }
    assert!(
        bounded.windows(2).all(|pair| pair[0].1 == pair[1].1),
        "the same owner rows read at every length of the history: {bounded:?}"
    );
    assert!(
        bounded.windows(2).all(|pair| pair[0].2 < pair[1].2),
        "the reference's owner rows grow with the history: {bounded:?}"
    );
}

// O3's root is production-shaped: its records carry their own times, one second apart within a
// scene and a scene apart (more than the 30 s error window) between scenes, and its live frames
// are published through an ArtifactStore whose files are copied into the root.
//
// 2026-01-01 00:00:00 UTC, the first epoch's first record.
const T0: u64 = 1_767_225_600_000;
const SECOND: u64 = 1_000;
// Each epoch begins six hours after the one before.
const EPOCH_MS: u64 = 6 * 3_600_000;
const SCENE_MS: u64 = 120 * SECOND;
const DAY_MS: u64 = 86_400_000;
// The views are read two days after `T0`: every frame is settled, and the frames of the older
// epochs are past their day.
const NOW: u64 = T0 + 2 * DAY_MS;
const SWITCHES: FrameRetentionSwitches = FrameRetentionSwitches {
    dedup_error: true,
    dedup_lab: false,
};
// How long a start's reconcile may take here.
const RECONCILE_MS: u64 = 600_000;

fn terminal(event: &PersistedEvent) -> TerminalEvent {
    TerminalEvent {
        sequence: event.sequence(),
        event_id: *event.event_id(),
    }
}

// An identifier's canonical text, as a snapshot id names it.
fn text<T: serde::Serialize>(id: &T) -> String {
    match serde_json::to_value(id) {
        Ok(serde_json::Value::String(text)) => text,
        other => panic!("identifier text: {other:?}"),
    }
}

fn scheduler() -> EventOrigin {
    EventOrigin::new(
        EventSource::Scheduler,
        OriginModule::Scheduler,
        EventActor::Scheduler,
    )
}

fn ledger_origin() -> EventOrigin {
    EventOrigin::new(
        EventSource::Runtime,
        OriginModule::GlobalLedger,
        EventActor::Runtime,
    )
}

// The native owner journal a start reads, here empty: every prior owner is unproven, and an
// import sealed from it is supported by it.
fn empty_journal() -> RuntimeOwnerJournal {
    let mut file = tempfile::tempfile().expect("an empty owner journal");
    RuntimeOwnerJournal::read_locked(&mut file)
        .unwrap_or_else(|error| panic!("the empty owner journal reads: {}", error.code.as_str()))
}

// A run of the fixture instance: the links of its task records and their request and
// correlation, the links of its lease records, and its two scopes (the run's and its lease's),
// each with its first record, as a start's prior-epoch close names them.
struct Run {
    id: IssuedRunId,
    request: IssuedRequestId,
    correlation: IssuedCorrelationId,
    links: EventLinksDraft,
    lease_links: EventLinksDraft,
    scopes: Vec<(PriorEpochScope, TerminalEvent)>,
}

// Collects the ArtifactStore's records into the history, each at the next sequence, and keeps the
// frame's `artifact.verified`.
struct Collect<'a> {
    fingerprinter: &'a Sha256SecretFingerprinter,
    events: &'a mut Vec<PersistedEvent>,
    first: u64,
    verified: Option<PersistedEvent>,
}

impl ArtifactEventSink for Collect<'_> {
    fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()> {
        let draft = draft
            .sanitize(self.fingerprinter)
            .expect("a frame store record");
        let sequence = self.first + self.events.len() as u64;
        let event = PersistedEvent::from_sanitized(sequence, draft).expect("a record at its place");
        if event.event_type() == EventType::ArtifactVerified {
            self.verified = Some(event.clone());
        }
        self.events.push(event);
        Ok(())
    }
}

// O3's history from sequence `first` on, and its live frames.
struct Shaped {
    ids: IdentifierIssuer,
    fingerprinter: Sha256SecretFingerprinter,
    instance: IssuedInstanceId,
    first: u64,
    events: Vec<PersistedEvent>,
    starts: Vec<u64>,
    // The next record's time, and the epoch it is in.
    at: u64,
    epoch: u64,
    // The frame store the frames are published through, then its folder.
    store: ArtifactStore,
    _scratch: TempDir,
    // The live frames: the file of each, relative to the store's root, and its epoch.
    files: Vec<PathBuf>,
    live: BTreeMap<ArtifactId, u64>,
}

impl Shaped {
    fn new(first: u64) -> Self {
        let scratch = tempfile::tempdir().expect("the frame store's folder");
        let store = material_store(scratch.path());
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let instance = ids.mint_instance_id().expect("instance");
        Self {
            ids,
            fingerprinter: Sha256SecretFingerprinter::new(SALT).expect("fingerprinter"),
            instance,
            first,
            events: Vec::new(),
            starts: Vec::new(),
            at: T0,
            epoch: 0,
            store,
            _scratch: scratch,
            files: Vec::new(),
            live: BTreeMap::new(),
        }
    }

    fn owner_epoch(&self) -> OwnerEpoch {
        *self
            .ids
            .mint_owner_epoch()
            .expect("owner epoch")
            .transport()
    }

    // The next epoch begins `EPOCH_MS` after the one before.
    fn begin_epoch(&mut self, epoch: u64) {
        self.epoch = epoch;
        self.at = self.at.max(T0 + epoch * EPOCH_MS);
    }

    fn scene(&mut self) {
        self.at += SCENE_MS;
    }

    fn tick(&mut self) -> u64 {
        let at = self.at;
        self.at += SECOND;
        at
    }

    fn record(&mut self, draft: SanitizedEventDraft) -> PersistedEvent {
        let sequence = self.first + self.events.len() as u64;
        let event = PersistedEvent::from_sanitized(sequence, draft).expect("a record at its place");
        self.events.push(event.clone());
        event
    }

    fn push_as(
        &mut self,
        severity: EventSeverity,
        origin: EventOrigin,
        links: EventLinksDraft,
        payload: EventPayloadDraft,
    ) -> PersistedEvent {
        let at = self.tick();
        let draft = EventDraft::new(
            self.ids.mint_event_id().expect("event id"),
            at,
            severity,
            origin,
            links,
            payload,
        )
        .sanitize(&self.fingerprinter)
        .expect("a fixture record");
        self.record(draft)
    }

    fn push(
        &mut self,
        origin: EventOrigin,
        links: EventLinksDraft,
        payload: EventPayloadDraft,
    ) -> PersistedEvent {
        self.push_as(EventSeverity::Info, origin, links, payload)
    }

    // A lifecycle observation of `owner`, standing for a start's capacity sample or a shutdown.
    fn owner(&mut self, owner: OwnerEpoch) -> u64 {
        self.push(
            runtime(),
            EventLinksDraft::default(),
            RuntimePayloadDraft::lifecycle_observed(
                owner,
                RuntimeLifecyclePhase::ShutdownRequested,
                AuditInput::new(),
            )
            .into(),
        )
        .sequence()
    }

    fn start(&mut self, takeover: bool) {
        let payload = if takeover {
            RuntimePayloadDraft::takeover(EventAction::RuntimeTakeover, AuditInput::new())
        } else {
            RuntimePayloadDraft::started(EventAction::RuntimeStart, AuditInput::new())
        };
        let start = self.push(runtime(), EventLinksDraft::default(), payload.into());
        self.starts.push(start.sequence());
    }

    fn scope_ids(&self) -> (IssuedRequestId, IssuedCorrelationId) {
        (
            self.ids.mint_request_id().expect("request"),
            self.ids.mint_correlation_id().expect("correlation"),
        )
    }

    fn scoped(
        &self,
        request: IssuedRequestId,
        correlation: IssuedCorrelationId,
    ) -> EventLinksDraft {
        EventLinksDraft::default()
            .with_instance_id(self.instance)
            .with_request_id(request)
            .with_correlation_id(correlation)
    }

    // The instance's confirmed resource quiescence in `owner`'s epoch, in `request` and
    // `correlation`: it closes the epoch's run-less scopes of the instance.
    fn quiescence(
        &mut self,
        owner: OwnerEpoch,
        request: IssuedRequestId,
        correlation: IssuedCorrelationId,
    ) {
        let links = self.scoped(request, correlation);
        let payload = RuntimePayloadDraft::lifecycle_observed(
            owner,
            RuntimeLifecyclePhase::ResourceQuiescence {
                instance_id: *self.instance.transport(),
                resource_count: 1,
                quiescence: ResourceQuiescence::Confirmed,
                owner_disposition: OwnerResourceDisposition::ConfirmedClosed,
                resource_dispositions: None,
            },
            AuditInput::new(),
        );
        self.push(runtime(), links, payload.into());
    }

    // A run of `owner`'s epoch: its lease granted, then its `task.started`.
    fn run(&mut self, owner: OwnerEpoch) -> Run {
        let id = self.ids.mint_run_id().expect("run");
        let (request, correlation) = self.scope_ids();
        let lease = self.ids.mint_lease_id().expect("lease");
        let links = self
            .scoped(request, correlation)
            .with_task_id(self.ids.mint_task_id().expect("task"))
            .with_run_id(id);
        let lease_links = links.clone().with_lease_id(lease);
        let granted = self.push(
            scheduler(),
            lease_links
                .clone()
                .with_action_id(self.ids.mint_action_id().expect("action")),
            LeasePayloadDraft::granted(
                EventAction::LeaseAcquire,
                EffectDisposition::Performed,
                AuditInput::new(),
            )
            .into(),
        );
        let started = self.push(
            runtime(),
            links.clone(),
            TaskPayloadDraft::started(EventAction::RuntimeTaskRun, AuditInput::new()).into(),
        );
        let instance = *self.instance.transport();
        let scope = |lease| PriorEpochScope {
            owner,
            instance,
            run: Some(*id.transport()),
            lease,
            request: None,
            correlation: None,
        };
        let scopes = vec![
            (scope(None), terminal(&started)),
            (scope(Some(*lease.transport())), terminal(&granted)),
        ];
        Run {
            id,
            request,
            correlation,
            links,
            lease_links,
            scopes,
        }
    }

    // A live capture frame of `owner`'s epoch in the scope of `links`, published through the
    // store, with its Explicit pin; its reference and its links.
    fn frame(
        &mut self,
        owner: OwnerEpoch,
        links: EventLinksDraft,
        correlation: IssuedCorrelationId,
        run: Option<IssuedRunId>,
    ) -> (ProjectedArtifactReference, EventLinksDraft) {
        let at = self.tick();
        let frame = self.ids.mint_frame_id().expect("frame");
        let links = links.with_frame_id(frame);
        let mut artifact_links = ArtifactLinksDraft::default()
            .with_frame_id(frame)
            .with_correlation_id(correlation);
        if let Some(run) = run {
            artifact_links = artifact_links.with_run_id(run);
        }
        let bytes = format!("ln7c-o frame {} captured at {at}", self.events.len()).into_bytes();
        let mut sink = Collect {
            fingerprinter: &self.fingerprinter,
            events: &mut self.events,
            first: self.first,
            verified: None,
        };
        let stored = self
            .store
            .put(
                ArtifactWriteRequest::new(
                    ArtifactKind::CaptureFrame,
                    &bytes,
                    ArtifactWriteContext::new(artifact_links, links.clone(), at),
                    ArtifactIssuePolicy::new(
                        ArtifactProducer::CaptureStore,
                        RetentionClass::DebugFull,
                        ArtifactRedactionState::Pending,
                    ),
                ),
                &mut sink,
            )
            .expect("frame publication");
        let verified = sink.verified.take().expect("verified receipt");
        let reference = verified
            .artifacts()
            .iter()
            .find(|artifact| artifact.kind() == ArtifactKind::CaptureFrame)
            .expect("capture frame reference")
            .project(true);
        let source = verified.links();
        let identity = ArtifactRetentionIdentity {
            artifact: reference.clone(),
            owner_epoch: owner,
            instance_id: *source.instance_id().expect("instance link"),
            request_id: *source.request_id().expect("request link"),
            correlation_id: *source.correlation_id().expect("correlation link"),
            run_id: source.run_id().copied(),
            lease_id: source.lease_id().copied(),
            policy_version: FRAME_RETENTION_POLICY_VERSION,
        };
        let pin = EventDraft::new(
            self.ids.mint_event_id().expect("event id"),
            at,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::ArtifactStore,
                EventActor::Runtime,
            ),
            EventLinksDraft::default(),
            ArtifactPayloadDraft::retention(
                ArtifactRetentionFact::PinRecorded(ArtifactPinRecord {
                    identity,
                    reason: ArtifactPinReason::Explicit,
                    trigger: terminal(&verified),
                }),
                AuditInput::new(),
            )
            .into(),
        )
        .sanitize(&self.fingerprinter)
        .expect("pin draft");
        let pin = source
            .artifact_retention_source()
            .apply_to(pin)
            .expect("pin source links");
        self.record(pin);
        self.files.push(
            stored
                .path()
                .strip_prefix(self.store.root())
                .expect("a frame file in the store")
                .to_path_buf(),
        );
        self.live.insert(reference.artifact_id, self.epoch);
        (reference, links)
    }

    fn run_frame(&mut self, run: &Run, owner: OwnerEpoch) -> ProjectedArtifactReference {
        self.frame(owner, run.links.clone(), run.correlation, Some(run.id))
            .0
    }

    // A run-less observe frame of the instance in its own correlation and in its own request (or
    // `request`), and its `capture.completed`.
    fn observe(&mut self, owner: OwnerEpoch, request: Option<IssuedRequestId>) {
        let (fresh, correlation) = self.scope_ids();
        let links = self.scoped(request.unwrap_or(fresh), correlation);
        let (_, links) = self.frame(owner, links, correlation, None);
        self.push(
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
    }

    // The run's capture summary naming `frames`, then its successful terminal.
    fn close_run(&mut self, run: &Run, frames: &[&ProjectedArtifactReference]) {
        let evidence = frames
            .iter()
            .enumerate()
            .map(|(index, frame)| {
                CapturePersistedEvidence::new(index as u64, (*frame).clone())
                    .expect("summary frame")
            })
            .collect::<Vec<_>>();
        let count = frames.len() as u64;
        let summary = CaptureSummaryRecord::new(
            count,
            0,
            0,
            count,
            EvidenceCompleteness::Complete,
            evidence,
            Vec::new(),
        )
        .expect("summary");
        self.push(
            runtime(),
            run.links.clone(),
            CapturePayloadDraft::summary_committed(summary, AuditInput::new()).into(),
        );
        self.push(
            runtime(),
            run.links.clone(),
            TaskPayloadDraft::semantic(
                TaskSemanticFact::TerminalCommitted {
                    task_result: TaskOutcome::Success,
                    final_page: None,
                    executed_steps: Some(0),
                    outcome: None,
                    failure_severe: false,
                    scheduling_disposition: None,
                    task_timing: None,
                },
                AuditInput::new(),
            )
            .into(),
        );
    }

    // The run's lease released (`Performed`): it closes the lease's scope.
    fn release(&mut self, run: &Run) {
        let links = run
            .lease_links
            .clone()
            .with_action_id(self.ids.mint_action_id().expect("action"));
        self.push(
            scheduler(),
            links,
            LeasePayloadDraft::released(
                EventAction::LeaseRelease,
                EffectDisposition::Performed,
                AuditInput::new(),
            )
            .into(),
        );
    }

    // An Error `runtime.failed` of the instance without a run: an error point.
    fn error_point(&mut self) {
        let (request, correlation) = self.scope_ids();
        let links = self.scoped(request, correlation);
        self.push_as(
            EventSeverity::Error,
            runtime(),
            links,
            RuntimePayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::NotPerformed,
                Outcome::relayed("capture_failed"),
                DiagnosticDetailDraft::new(
                    "ln7_fixture",
                    LedgerLocation::AppendEvent.location(),
                    "ln7_backend",
                    "ln7_operation",
                ),
                AuditInput::new(),
            )
            .into(),
        );
    }

    // A resource reading taken on `frame` of `run`, published as the Runtime publishes one: with
    // system links (no run, instance, lease or frame link) and inline content.
    fn reading(&mut self, run: &Run, frame: &ProjectedArtifactReference) {
        let record = FactRecord {
            scope: FactScope::Instance {
                instance_id: "fixture-instance-a".to_owned(),
            },
            key: "resource.ln7_fixture_counter".to_owned(),
            content: FactContent::Inline {
                value: FactValue::Integer(1),
            },
            observed_at_unix_ms: self.at,
            expires_at_unix_ms: None,
            ttl_policy: None,
            confidence_milli: 1_000,
            source_detector: format!("{RESOURCE_READING_DETECTOR_PREFIX}ln7_task/ln7_reading"),
            source_snapshot_id: resource_reading_snapshot_id(
                &text(run.id.transport()),
                &text(&frame.frame_id.expect("a capture frame has a frame id")),
                "ln7_reading",
            ),
            schema_version: "fact.v1".to_owned(),
            resource_bundle_hash: "5".repeat(64),
            invalidate_on: Vec::new(),
        };
        let (request, correlation) = self.scope_ids();
        let links = EventLinksDraft::default()
            .with_request_id(request)
            .with_correlation_id(correlation)
            .with_action_id(self.ids.mint_action_id().expect("action"));
        self.push(
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::FactStore,
                EventActor::Runtime,
            ),
            links,
            FactPayloadDraft::published(record, AuditInput::new()).into(),
        );
    }

    // A Lab anchor (a record of the Lab) on `request`.
    fn lab_anchor(&mut self, request: IssuedRequestId) {
        let links = EventLinksDraft::default()
            .with_request_id(request)
            .with_correlation_id(self.ids.mint_correlation_id().expect("correlation"));
        self.push(
            EventOrigin::new(EventSource::Lab, OriginModule::Actinglab, EventActor::Lab),
            links,
            CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into(),
        );
    }

    fn prior_epoch(&mut self, writer: OwnerEpoch, fact: PriorEpochCloseFact) -> TerminalEvent {
        let (request, correlation) = (
            self.ids.mint_request_id().expect("request"),
            self.ids.mint_correlation_id().expect("correlation"),
        );
        let links = EventLinksDraft::default()
            .with_request_id(request)
            .with_correlation_id(correlation)
            .with_action_id(self.ids.mint_action_id().expect("action"));
        let event = self.push(
            ledger_origin(),
            links,
            RuntimePayloadDraft::prior_epoch_close(writer, fact).into(),
        );
        terminal(&event)
    }

    // `writer`'s import of `subject`, whose epoch ended at `upper`, as a start's reconcile seals
    // it from the empty native journal: the import and the prefix it saw.
    fn import(
        &mut self,
        writer: OwnerEpoch,
        subject: OwnerEpoch,
        upper: u64,
    ) -> (TerminalEvent, u64) {
        let through = self.first + self.events.len() as u64 - 1;
        let import = PriorEpochOwnerImport {
            evidence: empty_journal().observation(subject),
            basis: unproven(),
            through_sequence: through,
            scope_upper_sequence: upper,
        };
        let recorded = self.prior_epoch(writer, PriorEpochCloseFact::OwnerImported(import));
        (recorded, through)
    }

    // `writer`'s close of `scope`, whose first record is `source`, by `import`.
    fn close(
        &mut self,
        writer: OwnerEpoch,
        (scope, source): (PriorEpochScope, TerminalEvent),
        import: (TerminalEvent, u64),
        upper: u64,
    ) {
        let close = PriorEpochScopeClose {
            subject: scope,
            proof: import.0,
            scope_source: source,
            through_sequence: import.1,
            scope_upper_sequence: upper,
            basis: unproven(),
        };
        self.prior_epoch(writer, PriorEpochCloseFact::ScopeClosed(close));
    }

    // The ledger's record of a dump it imported, after the dump's `count` records.
    fn dump_imported(&mut self, count: u64) {
        let outcome = Outcome::new(LedgerCode::LedgerDumpImported)
            .count(count)
            .duplicate_count(0)
            .dump_sealed(true)
            .dump_sha256(format!("sha256:{}", "0".repeat(64)));
        self.push(
            EventOrigin::new(
                EventSource::System,
                OriginModule::GlobalLedger,
                EventActor::System,
            ),
            EventLinksDraft::default(),
            LedgerPayloadDraft::dump_imported(outcome, AuditInput::new()).into(),
        );
    }

    // A closed epoch of `owner`: its first owner record and its start; a run (its lease granted,
    // two frames, its summary and terminal, its lease released) and then the instance's
    // quiescence in the run's request and correlation, which the open's frame read reads with the
    // run (an owner record older than the bounded read, review F1); a resource reading frame; a
    // run-less frame an error point follows; a run-less frame a Lab anchor names; its last owner
    // record.
    fn closed_epoch(&mut self, owner: OwnerEpoch) {
        self.owner(owner);
        self.start(false);
        let run = self.run(owner);
        let frames = [self.run_frame(&run, owner), self.run_frame(&run, owner)];
        self.close_run(&run, &[&frames[0], &frames[1]]);
        self.release(&run);
        self.quiescence(owner, run.request, run.correlation);
        self.scene();
        let reading = self.run(owner);
        let frame = self.run_frame(&reading, owner);
        self.close_run(&reading, &[&frame]);
        self.release(&reading);
        self.reading(&reading, &frame);
        self.scene();
        self.observe(owner, None);
        self.error_point();
        self.scene();
        let (request, _) = self.scope_ids();
        self.observe(owner, Some(request));
        self.lab_anchor(request);
        self.scene();
        self.owner(owner);
    }

    // O3's history: two closed epochs, then the two latest:
    // - `previous`: a run whose lease is never released and that has no terminal, and a run-less
    //   frame; its writer failed, and the next start's open imported its dump (its quiescence and
    //   its last owner record, then `ledger.dump_imported`) before that start's first record;
    // - `latest`, a takeover: its start imports `previous` and closes both scopes of its open run
    //   before `runtime.takeover`; then a closed run (summary, terminal, lease released), a
    //   run-less frame an error point follows, the instance's quiescence and an open run (lease
    //   granted, no terminal). It stops without its last owner record.
    // Returns the owners in order: the two old ones, `previous`, `latest`.
    fn production(first: u64) -> (Self, [OwnerEpoch; 4]) {
        let mut shaped = Self::new(first);
        let owners = [(); 4].map(|()| shaped.owner_epoch());
        let [old1, old2, previous, latest] = owners;
        for (epoch, owner) in [(0, old1), (1, old2)] {
            shaped.begin_epoch(epoch);
            shaped.closed_epoch(owner);
        }
        shaped.begin_epoch(2);
        shaped.owner(previous);
        shaped.start(false);
        let open = shaped.run(previous);
        for _ in 0..3 {
            shaped.run_frame(&open, previous);
        }
        shaped.scene();
        shaped.observe(previous, None);
        shaped.scene();
        let (request, correlation) = shaped.scope_ids();
        shaped.quiescence(previous, request, correlation);
        shaped.owner(previous);
        shaped.dump_imported(2);
        shaped.begin_epoch(3);
        let upper = shaped.owner(latest) - 1;
        let import = shaped.import(latest, previous, upper);
        for scope in open.scopes.iter().copied() {
            shaped.close(latest, scope, import, upper);
        }
        shaped.start(true);
        let closed = shaped.run(latest);
        let frames = [
            shaped.run_frame(&closed, latest),
            shaped.run_frame(&closed, latest),
        ];
        shaped.close_run(&closed, &[&frames[0], &frames[1]]);
        shaped.release(&closed);
        shaped.scene();
        shaped.observe(latest, None);
        shaped.error_point();
        shaped.scene();
        let (request, correlation) = shaped.scope_ids();
        shaped.quiescence(latest, request, correlation);
        shaped.scene();
        let running = shaped.run(latest);
        for _ in 0..2 {
            shaped.run_frame(&running, latest);
        }
        (shaped, owners)
    }
}

// A formal root whose ledger is ready and holds no history yet, prepared as `Root::write` does.
struct EmptyRoot {
    dir: TempDir,
    database: Arc<RuntimeDatabase>,
}

impl EmptyRoot {
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

    fn next_sequence(&self) -> u64 {
        SyntheticAppender::open(&self.database)
            .expect("the root's appender")
            .next_sequence()
            .expect("the next sequence")
    }

    // The root holding `shaped`'s history and its live frame files.
    fn fill(self, shaped: &Shaped) -> Root {
        let Self { dir, database } = self;
        let limits = MaintenanceLimits::default();
        let maintenance = LedgerMaintenance::acquire(
            dir.path(),
            false,
            limits,
            limits.deadline().expect("deadline"),
        )
        .expect("maintenance owner");
        let mut appender = SyntheticAppender::open(&database).expect("the root's appender");
        assert_eq!(
            appender.next_sequence().expect("the next sequence"),
            shaped.first,
            "the history begins at the root's next sequence"
        );
        for batch in shaped.events.chunks(BATCH_RECORDS) {
            appender.append(batch).expect("a batch of fixture records");
        }
        drop(appender);
        maintenance.close().expect("release the root");
        for file in &shaped.files {
            let target = dir.path().join(file);
            std::fs::create_dir_all(target.parent().expect("a frame file's folder"))
                .expect("the live frame store's folder");
            std::fs::copy(shaped.store.root().join(file), target).expect("a live frame file");
        }
        let owner_rows = shaped
            .events
            .iter()
            .filter(|event| owner_type(event.event_type()))
            .map(PersistedEvent::sequence)
            .collect();
        Root {
            dir,
            database,
            starts: shaped.starts.clone(),
            owner_rows,
            records: shaped.events.len() as u64,
        }
    }
}

// The drafts of the new start's own records, the same for both roots: its first owner record (the
// instance's quiescence, standing for its capacity sample) and its start record.
struct NewStart {
    writer: OwnerEpoch,
    first: SanitizedEventDraft,
    start: SanitizedEventDraft,
}

impl NewStart {
    fn new(shaped: &Shaped) -> Self {
        let writer = shaped.owner_epoch();
        let at = T0 + 5 * EPOCH_MS;
        let (request, correlation) = shaped.scope_ids();
        let first = EventDraft::new(
            shaped.ids.mint_event_id().expect("event id"),
            at,
            EventSeverity::Info,
            runtime(),
            shaped.scoped(request, correlation),
            RuntimePayloadDraft::lifecycle_observed(
                writer,
                RuntimeLifecyclePhase::ResourceQuiescence {
                    instance_id: *shaped.instance.transport(),
                    resource_count: 1,
                    quiescence: ResourceQuiescence::Confirmed,
                    owner_disposition: OwnerResourceDisposition::ConfirmedClosed,
                    resource_dispositions: None,
                },
                AuditInput::new(),
            )
            .into(),
        )
        .sanitize(&shaped.fingerprinter)
        .expect("the start's first owner record");
        let (request, correlation) = shaped.scope_ids();
        let start = EventDraft::new(
            shaped.ids.mint_event_id().expect("event id"),
            at + SECOND,
            EventSeverity::Info,
            runtime(),
            EventLinksDraft::default()
                .with_request_id(request)
                .with_correlation_id(correlation),
            RuntimePayloadDraft::started(EventAction::RuntimeStart, AuditInput::new()).into(),
        )
        .sanitize(&shaped.fingerprinter)
        .expect("the start record");
        Self {
            writer,
            first,
            start,
        }
    }
}

// What a start's open of an O3 root keeps and decides, and what its startup reconcile writes.
struct Started {
    // The owner state the open kept, and whether it read every owner record.
    state: (OwnerState, bool),
    // The frame view at the open, and at the frame cleaner's first sweep start after the start's
    // own records (LN-7b).
    opened: FrameRetentionView,
    swept: FrameRetentionView,
    // The records the startup reconcile wrote (`decided`).
    reconciled: Vec<String>,
}

// The records after `head`.
fn after(ledger: &GlobalLedger, head: u64) -> Vec<PersistedEvent> {
    ledger
        .query(EventQuery {
            from_sequence: Some(head + 1),
            ..EventQuery::default()
        })
        .expect("ledger records")
}

// Every string in `value`.
fn strings(value: &serde_json::Value, found: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) => found.push(text.clone()),
        serde_json::Value::Array(items) => items.iter().for_each(|item| strings(item, found)),
        serde_json::Value::Object(fields) => fields.values().for_each(|item| strings(item, found)),
        _ => {}
    }
}

fn rename(value: &mut serde_json::Value, minted: &BTreeMap<String, String>) {
    match value {
        serde_json::Value::String(text) => {
            if let Some(name) = minted.get(text.as_str()) {
                *text = name.clone();
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(|item| rename(item, minted)),
        serde_json::Value::Object(fields) => {
            fields.values_mut().for_each(|item| rename(item, minted));
        }
        _ => {}
    }
}

// The records a start's reconcile wrote, each as its public JSON, compared byte for byte once the
// identifiers the writer mints for each record it appends (its event id and its links) are named
// by that record's sequence and its time is set aside: what is left is what the reconcile decided
// from the ledger.
fn decided(records: &[PersistedEvent]) -> Vec<String> {
    let values = records
        .iter()
        .map(|event| serde_json::to_value(event).expect("a record's JSON"))
        .collect::<Vec<_>>();
    let mut minted = BTreeMap::new();
    for (event, value) in records.iter().zip(&values) {
        let mut own = Vec::new();
        strings(&value["event_id"], &mut own);
        strings(&value["links"], &mut own);
        for (index, id) in own.into_iter().enumerate() {
            minted.insert(id, format!("minted:{}:{index}", event.sequence()));
        }
    }
    values
        .into_iter()
        .map(|mut value| {
            value["timestamp_unix_ms"] = serde_json::Value::from(0_u64);
            rename(&mut value, &minted);
            serde_json::to_string(&value).expect("a record's JSON")
        })
        .collect()
}

// A start on an O3 root: the writer open (with `every`, the reference's read of every owner
// record), its owner state and frame view; then its first owner record, its real startup
// reconcile (`reconcile_prior_epoch_closes` with the empty native journal) and its start record;
// then the frame cleaner's sweep start.
fn started(root: &Root, every: bool, new: &NewStart) -> Started {
    read_every_owner_record_for_test(every);
    let limits = MaintenanceLimits::default();
    let opened = LedgerMaintenance::acquire(
        root.dir.path(),
        false,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("maintenance owner")
    .open_writer(Arc::clone(&root.database), OWNER.into());
    read_every_owner_record_for_test(false);
    let ledger = opened.expect("the writer opens");
    let state = take_opened_owner_state_for_test().expect("the open kept its owner state");
    let opened = ledger
        .frame_retention_view(NOW, SWITCHES)
        .expect("the frame view at the open");
    let first = ledger
        .append(new.first.clone())
        .expect("the start's first owner record");
    ledger
        .reconcile_prior_epoch_closes(
            new.writer,
            empty_journal(),
            MaintenanceLimits {
                timeout_seconds: RECONCILE_MS / 1000,
                ..MaintenanceLimits::default()
            }
            .deadline()
            .expect("the reconcile's deadline"),
        )
        .expect("the startup reconcile");
    let reconciled = decided(&after(&ledger, first.sequence()));
    ledger.append(new.start.clone()).expect("the start record");
    let swept = ledger
        .frame_retention_view_live(NOW, SWITCHES, root.dir.path())
        .expect("the frame view at a sweep start");
    ledger.close().expect("close the writer");
    Started {
        state,
        opened,
        swept,
        reconciled,
    }
}

// The kept moves of a view: the folder of each settled Lab and error frame.
fn kept_moves(view: &FrameRetentionView) -> BTreeMap<ArtifactId, KeptFrameFolder> {
    view.frames
        .iter()
        .filter_map(|frame| Some((frame.reference.artifact_id, frame.folder.clone()?)))
        .collect()
}

// Why a start's writer open of `root` (with a failure plan, so that it imports a dump) read every
// owner record, `None` when it did not. With `dump`, an earlier run left a dump at the plan's dump
// path, here one cut short before its header line.
fn owner_read_reason(root: &Root, dump: bool) -> Option<OwnerReadFull> {
    let state_root = root.dir.path().canonicalize().expect("state root");
    let epoch = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_owner_epoch()
        .expect("owner epoch")
        .transport();
    let plan = LedgerFailurePlan::new(
        LedgerWritePolicy::new(1, vec![0], STALL_MS, PROBE_MS, DUMP_MAX_BYTES)
            .expect("write policy"),
        &state_root,
        None,
        epoch,
        T0 + 5 * EPOCH_MS,
    );
    if dump {
        std::fs::write(plan.dump_path(), b"").expect("a dump an earlier run left");
    }
    let mut timing = LedgerOpenTiming::default();
    let ledger = LedgerMaintenance::acquire(
        root.dir.path(),
        false,
        MaintenanceLimits::default(),
        MaintenanceLimits::default().deadline().expect("deadline"),
    )
    .and_then(|maintenance| {
        maintenance.open_writer_timed(
            Arc::clone(&root.database),
            OWNER.into(),
            Some(plan),
            &mut timing,
        )
    })
    .expect("the writer opens");
    let _ = take_opened_owner_state_for_test();
    ledger.close().expect("close the writer");
    timing.owner_read_full
}

#[test]
fn ln7c_o3_the_bounded_open_keeps_the_frame_view_and_the_startup_closes_of_the_full_read() {
    // The production-shaped root, written twice from the same records and frame files: the
    // reference opens one with the read of every owner record, the bounded open the other.
    let empty = EmptyRoot::new();
    let (shaped, [old1, old2, previous, latest]) = Shaped::production(empty.next_sequence());
    let reference_root = empty.fill(&shaped);
    let bounded_root = EmptyRoot::new().fill(&shaped);
    assert_eq!(
        shaped.live.values().copied().collect::<BTreeSet<_>>(),
        BTreeSet::from([0, 1, 2, 3]),
        "live frames in each of the four epochs"
    );
    let new = NewStart::new(&shaped);
    let reference = started(&reference_root, true, &new);
    let bounded = started(&bounded_root, false, &new);

    // The owner state.
    assert!(
        !reference.state.1,
        "the reference reads every owner record by itself"
    );
    assert!(!bounded.state.1, "the owner read is bounded");
    assert_eq!(
        bounded.state.0, reference.state.0,
        "the bounded read keeps the owner state of the read of every owner record"
    );
    assert_eq!(
        bounded.state.0.retained,
        BTreeSet::from([previous, latest]),
        "the two latest owners are retained, not {old1:?} or {old2:?}"
    );

    // LN-7b's frame view and its kept moves, at the open and at a sweep start.
    let live = shaped.live.keys().copied().collect::<BTreeSet<_>>();
    for view in [&reference.opened, &reference.swept] {
        assert_eq!(
            view.frames
                .iter()
                .map(|frame| frame.reference.artifact_id)
                .collect::<BTreeSet<_>>(),
            live,
            "the view lists every live frame"
        );
    }
    assert_eq!(
        bounded.opened, reference.opened,
        "the frame view at the open is the full read's"
    );
    assert_eq!(
        bounded.swept, reference.swept,
        "the frame view at a sweep start is the full read's"
    );
    let moves = kept_moves(&reference.opened);
    assert!(!moves.is_empty(), "the root has frames to move into kept");
    assert_eq!(
        kept_moves(&bounded.opened),
        moves,
        "the kept moves are the full read's"
    );

    // The records the start's real reconcile writes: the import of the latest owner and the
    // closes of its open scopes, after the check of the previous owner's sealed import.
    assert!(
        !reference.reconciled.is_empty(),
        "the start imports the latest owner and closes its open scopes"
    );
    assert_eq!(
        bounded.reconciled, reference.reconciled,
        "the startup reconcile writes the full read's records"
    );

    // Review F2: a dump an earlier run left makes the open read every owner record, and say why;
    // without one the same root's open is bounded.
    assert_eq!(owner_read_reason(&bounded_root, false), None);
    assert_eq!(
        owner_read_reason(&bounded_root, true),
        Some(OwnerReadFull::DumpPresent),
        "a dump present at the open"
    );

    // The previous owner in no start's epoch: its start stopped after its first owner record,
    // before its start record, and the latest start followed.
    let (root, ()) = Root::write(|history| {
        for _ in 0..4 {
            let owner = history.owner_epoch();
            history.epoch(owner, BODY_RECORDS + 3);
        }
        let stopped = history.owner_epoch();
        history.owner(stopped);
        history.plain();
        let owner = history.owner_epoch();
        history.epoch(owner, BODY_RECORDS + 3);
    });
    let (_, full) = compared("the previous owner in no start's epoch", &root);
    assert!(
        full,
        "the previous owner in no start's epoch: the open reads every owner record"
    );
    assert_eq!(
        owner_read_reason(&root, false),
        Some(OwnerReadFull::OwnerInNoStartEpoch)
    );

    // An epoch with no owner record after its start record (a start whose process died at
    // once): reading the next owner back reaches the start before its own.
    let (root, ()) = Root::write(|history| {
        for _ in 0..3 {
            let owner = history.owner_epoch();
            history.epoch(owner, BODY_RECORDS + 3);
        }
        let died = history.owner_epoch();
        history.owner(died);
        history.start(false);
        history.plain();
        history.unowned();
        for _ in 0..2 {
            let owner = history.owner_epoch();
            history.epoch(owner, BODY_RECORDS + 3);
        }
    });
    let (_, full) = compared("an epoch with no owner record after its start", &root);
    assert!(
        full,
        "an epoch with no owner record after its start: the open reads every owner record"
    );
    assert_eq!(
        owner_read_reason(&root, false),
        Some(OwnerReadFull::OwnerInNoStartEpoch)
    );
}
