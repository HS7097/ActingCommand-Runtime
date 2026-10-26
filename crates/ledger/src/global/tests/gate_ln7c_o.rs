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
//
// An owner epoch here is laid out as a start writes it: its first owner record (as the start's
// capacity sample), its prior-epoch imports and closes, its start record, its records, and its last
// owner record (as a shutdown's) unless it stopped. The roots are real formal SQLite roots whose
// records are written straight into their tables (`SyntheticAppender`, LN-1), each a valid
// sanitized record at its sequence.

use crate::codes::LedgerCode;
use crate::global::retention::{
    OwnerState, read_every_owner_record_for_test, take_opened_owner_state_for_test,
};
use crate::global::{
    GlobalLedger, LedgerFailurePlan, LedgerMaintenance, LedgerOpenTiming, LedgerWritePolicy,
    Sha256SecretFingerprinter, SyntheticAppender,
};
use crate::{LedgerReadCounter, PersistedEvent};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    AuditInput, CommandPayloadDraft, EffectDisposition, EventAction, EventActor, EventDraft,
    EventLinksDraft, EventOrigin, EventPayloadDraft, EventSeverity, EventSource, EventType,
    IdentifierIssuer, IssuedInstanceId, LedgerPayloadDraft, OWNER_JOURNAL_SCHEMA, OriginModule,
    OwnerEpoch, OwnerEpochCloseEvidence, OwnerResourceDisposition, PriorEpochCloseBasis,
    PriorEpochCloseFact, PriorEpochOwnerImport, PriorEpochScope, PriorEpochScopeClose,
    ResourceQuiescence, RuntimeLifecyclePhase, RuntimePayloadDraft, TaskPayloadDraft,
    TerminalEvent, UnprovenReason,
};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use actingcommand_runtime_state::RuntimeStateStore;
use std::collections::BTreeSet;
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
