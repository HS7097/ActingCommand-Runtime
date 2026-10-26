// SPDX-License-Identifier: AGPL-3.0-only

//! #419 / #422 LN-WS-F (brief rd-1026/brief-ln-ws-f.md; the audit "Exposure" of #1367): the
//! Runtime's own recurring ledger reads stay bounded and fail only their own work.
//!
//! R2, bounded, on a production-shaped synthetic ledger (LN-1: three instances in turn, runs of 24
//! frames with every second frame stored, so `artifact.verified` is about one record in
//! nineteen), with a dispatch intent every twenty records of the history appended through its
//! writer, each under its own run links, and two stored reports:
//! - T1 growth: the SQLite work of each bounded read (a decision's dispatch intent from the
//!   sequence the dispatch journal holds, as the reconciliation tick and its settlements read
//!   it; the verified records of a proposal's reports or a strategic report's evidence) on
//!   12 000 records is at most 1.5 times its work on 3 000;
//! - T2 equality: each bounded read answers what today's history-wide read answers (every
//!   intent of the type filtered by its decision; every `artifact.verified` record), for one
//!   intent, two intents of one decision (a repeated append under the same links), none (the
//!   journal's sequence holding another decision's intent, or no sequence), and for references
//!   that are verified, one whose digest differs and one whose artifact the ledger does not hold,
//!   alone and together.
//!
//! R1, tolerant, on hosts whose writer reply limit is 3 s (the only real wait; nothing sleeps):
//! - T3: the reconciliation tick's read of a pending settlement's dispatch intent is held by the
//!   writer's read hold past the reply limit: the tick goes on, nothing latches, no stop is
//!   requested, the settlement wrote nothing, and the next tick settles it;
//! - T4: a proposal request's read of its reports held the same way is answered `failed`,
//!   non-fatal, with `writer_response_timeout`; the Runtime keeps running and the same proposal
//!   is then evaluated;
//! - T5: a damaged record on that settlement read (its dispatch intent) still latches and stops
//!   the Runtime.
//!
//! Fix round 1 (brief rd-1026/brief-ln-ws-f-fix1.md, review rd-1026/ln-ws-f/review.md):
//! - T6 (H1): a live settlement's completion read, which comes after its execution record is
//!   committed, held past the reply limit stays fatal: the Runtime latches a `ledger_failure`
//!   whose cause is the ledger's `writer_response_timeout`, and a stop is requested;
//! - T7 (L1): with two pending settlements and the writer held from the tick's first settlement
//!   read on, only that read waits out the reply limit (one skip in the daemon's log), nothing
//!   latches, neither settlement wrote anything, and the next tick settles both.

use super::lean_scale::{OWNER, SALT, formal_root};
use super::*;
use crate::codes::{HostCode, HostLocation};
use crate::host::{
    AROUND_COMPLETION_READ, AROUND_SETTLEMENT_READS, AROUND_VERIFIED_READ, AroundReadForTest,
    SettlementRead, policy_dispatch_intent, policy_dispatch_intents, verified_reference_sequences,
};
use actingcommand_artifact_store::{
    ArtifactCapacityAdmission, ArtifactEventSink, ArtifactStoreResult, ArtifactWriteContext,
    ArtifactWriteRequest,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    ArtifactIssuePolicy, ArtifactLinksDraft, ArtifactRedactionState, AuditInput,
    CapacityAdmissionOutcome, CapacityAdmissionReason, CapacityDecision, EventDraft,
    EventLinksDraft, EventOrigin, OwnerEpoch, PolicyDispatchEventData, PolicyPayloadDraft,
    PolicyReasonRecord, RetentionClass,
};
use actingcommand_ledger::{
    LedgerCode, LedgerMaintenance, Sha256SecretFingerprinter, SyntheticInstance,
    SyntheticLedgerSpec, WriterReadHoldGate, WriterStatementRecorder, write_synthetic_ledger,
};
use actingcommand_runtime_database::{DatabaseLocation, MaintenanceLimits, RuntimeDatabase};
use std::sync::Mutex;
use std::thread::LocalKey;

// Two synthetic histories of the same shape, one four times the other.
const SMALL_RECORDS: u64 = 3_000;
const LARGE_RECORDS: u64 = 12_000;
// The synthetic runs: 24 frames, every second one stored, on three instances in turn.
const FRAMES_PER_RUN: u64 = 24;
const STORED_FRAME_EVERY: u64 = 2;
const INSTANCE_LETTERS: &str = "abc";
// A dispatch intent every this many records of the history.
const INTENT_EVERY: u64 = 20;
// The appended records' times, after the synthetic history's.
const APPENDED_FIRST_MS: u64 = 1_760_000_000_000;
const RECORD_INTERVAL_MS: u64 = 500;
// The decisions of the cases: none has no intent at all.
const SINGLE_DECISION: &str = "decision:ln-ws-f-single";
const REPEATED_DECISION: &str = "decision:ln-ws-f-repeated";
const NONE_DECISION: &str = "decision:ln-ws-f-none";
// The two stored reports.
const REPORTS: [&str; 2] = ["ln-ws-f fixture report one", "ln-ws-f fixture report two"];
// The salt of the appended records' secret fingerprints (they carry no secret field).
const FINGERPRINT_SALT: &[u8] = b"ln-ws-f-fixture";
// T3, T4: how long a held read waits for the writer.
const REPLY_TIMEOUT: Duration = Duration::from_secs(3);

fn fixture_instance(letter: char) -> InstanceId {
    serde_json::from_str(&format!("\"instance_{letter:0>32}\"")).expect("a fixture instance id")
}

/// The dispatch data of one fixture intent of `decision_id`.
fn dispatch_data(decision_id: &str) -> PolicyDispatchEventData {
    PolicyDispatchEventData {
        decision_id: decision_id.to_owned(),
        task_id: "fixture.observe".to_owned(),
        instance_id: "fixture-instance-a".to_owned(),
        operation_id: "operation:ln-ws-f".to_owned(),
        procedure_binding_digest: format!("sha256:{}", "2".repeat(64)),
        reason_chain_id: format!("reason:{decision_id}"),
        reasons: vec![PolicyReasonRecord(Outcome::relayed("placement_selected"))],
        catalog_hash: format!("sha256:{}", "3".repeat(64)),
        catalog_version: 1,
        input_ledger_position: 1,
        fact_snapshot_id: "snapshot:ln-ws-f".to_owned(),
        approval_fact_ids: vec![],
        urgency_milli: 100,
    }
}

/// Whether `event` is a dispatch intent of `decision_id`.
fn intent_of(event: &PersistedEvent, decision_id: &str) -> bool {
    matches!(
        event.payload(),
        EventPayload::Policy(PolicyPayload::DispatchIntent(payload))
            if payload.decision_id() == decision_id
    )
}

/// The records the artifact store gives while it stores one report, in order.
#[derive(Default)]
struct Drafts(Vec<EventDraft>);

impl ArtifactEventSink for Drafts {
    fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()> {
        self.0.push(draft);
        Ok(())
    }
}

/// The capacity owner of the fixture's artifact store: every write is admitted.
struct AdmitEvery {
    owner_epoch: OwnerEpoch,
}

impl ArtifactCapacityAdmission for AdmitEvery {
    fn decide(&self, _path: &Path, bytes: u64) -> ArtifactStoreResult<CapacityDecision> {
        Ok(CapacityDecision {
            owner_epoch: self.owner_epoch,
            decided_at_unix_ms: APPENDED_FIRST_MS,
            decided_at_monotonic_ms: 0,
            requested_bytes: bytes,
            target_volume: None,
            admission_result: CapacityAdmissionOutcome::Allowed,
            reason: CapacityAdmissionReason::FreshSample,
            fact: None,
        })
    }
}

/// The records the fixture appends through the root's writer.
struct Appender<'a> {
    ledger: &'a GlobalLedger,
    store: ArtifactStore,
    issuer: IdentifierIssuer,
    fingerprinter: Sha256SecretFingerprinter,
    at_ms: u64,
}

impl<'a> Appender<'a> {
    fn new(ledger: &'a GlobalLedger, root: &Path) -> Self {
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        let store = ArtifactStore::open(root).expect("the root's artifact store");
        store
            .install_capacity_admission(Arc::new(AdmitEvery {
                owner_epoch: *issuer
                    .mint_owner_epoch()
                    .expect("an owner epoch")
                    .transport(),
            }))
            .expect("the fixture's capacity owner");
        Self {
            ledger,
            store,
            issuer,
            fingerprinter: Sha256SecretFingerprinter::new(FINGERPRINT_SALT).expect("fingerprinter"),
            at_ms: APPENDED_FIRST_MS,
        }
    }

    fn next_ms(&mut self) -> u64 {
        self.at_ms += RECORD_INTERVAL_MS;
        self.at_ms
    }

    /// A dispatch's own links: instance, request, correlation, task, run and action.
    fn run_links(&self) -> EventLinksDraft {
        EventLinksDraft::default()
            .with_instance_id(self.issuer.mint_instance_id().expect("an instance id"))
            .with_request_id(self.issuer.mint_request_id().expect("a request id"))
            .with_correlation_id(self.issuer.mint_correlation_id().expect("a correlation id"))
            .with_task_id(self.issuer.mint_task_id().expect("a task id"))
            .with_run_id(self.issuer.mint_run_id().expect("a run id"))
            .with_action_id(self.issuer.mint_action_id().expect("an action id"))
    }

    fn intent_draft(&mut self, decision_id: &str, links: EventLinksDraft) -> EventDraft {
        let at = self.next_ms();
        EventDraft::new(
            self.issuer.mint_event_id().expect("an event id"),
            at,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Scheduler,
                OriginModule::Policy,
                EventActor::Scheduler,
            ),
            links,
            PolicyPayloadDraft::dispatch_intent(dispatch_data(decision_id), AuditInput::new())
                .into(),
        )
    }

    /// One record captured without awaiting its commit.
    fn capture(&self, draft: EventDraft) {
        self.ledger
            .append_deferred(
                draft
                    .sanitize(&self.fingerprinter)
                    .expect("a fixture record"),
            )
            .expect("the record is captured");
    }

    /// A dispatch intent of the history, under its own links.
    fn history_intent(&mut self, index: u64) {
        let links = self.run_links();
        let draft = self.intent_draft(&format!("decision:ln-ws-f-{index}"), links);
        self.capture(draft);
    }

    /// A dispatch intent of `decision_id` under `links`, committed: its sequence.
    fn committed_intent(&mut self, decision_id: &str, links: EventLinksDraft) -> u64 {
        let draft = self.intent_draft(decision_id, links);
        self.ledger
            .append(
                draft
                    .sanitize(&self.fingerprinter)
                    .expect("a fixture intent"),
            )
            .expect("the intent is appended")
            .sequence()
    }

    /// A text report stored by the root's artifact store (`artifact.created` and
    /// `artifact.verified`): its reference, as a proposal names it.
    fn report(&mut self, text: &str) -> ProjectedArtifactReference {
        let at = self.next_ms();
        let links = EventLinksDraft::default()
            .with_request_id(self.issuer.mint_request_id().expect("a request id"))
            .with_correlation_id(self.issuer.mint_correlation_id().expect("a correlation id"));
        let mut drafts = Drafts::default();
        let stored = self
            .store
            .put(
                ArtifactWriteRequest::new(
                    ArtifactKind::TextReport,
                    text.as_bytes(),
                    ArtifactWriteContext::new(ArtifactLinksDraft::default(), links, at),
                    ArtifactIssuePolicy::new(
                        ArtifactProducer::ArtifactStore,
                        RetentionClass::Adaptive,
                        ArtifactRedactionState::NotRequired,
                    ),
                ),
                &mut drafts,
            )
            .expect("the report is stored");
        for draft in drafts.0 {
            self.capture(draft);
        }
        stored.reference().project(true)
    }

    /// Every captured record committed.
    fn commit(&self) {
        let summary = self
            .ledger
            .confirm_deferred(Duration::from_secs(600))
            .expect("the confirmation");
        assert!(
            summary.failed.is_empty() && summary.pending == 0,
            "every appended record commits: {:?}, {} pending",
            summary.failed,
            summary.pending
        );
    }
}

/// A formal root holding the synthetic history of `records` records, then, through its writer,
/// a dispatch intent every `INTENT_EVERY` records (the cases' intents among them) and the two
/// reports; its writer open.
struct Root {
    dir: TempDir,
    ledger: GlobalLedger,
    head: u64,
    /// The sequence of the one intent of `SINGLE_DECISION`.
    single_at: u64,
    /// The sequences of the two intents of `REPEATED_DECISION`, under the same links.
    repeated_at: (u64, u64),
    reports: Vec<ProjectedArtifactReference>,
}

impl Root {
    fn new(records: u64) -> Self {
        let dir = TempDir::new().expect("the state root");
        formal_root(dir.path());
        let database =
            RuntimeDatabase::open_existing(dir.path(), false).expect("the root's database");
        let instances = INSTANCE_LETTERS
            .chars()
            .map(|letter| SyntheticInstance {
                alias: format!("fixture-instance-{letter}"),
                instance_id: fixture_instance(letter),
            })
            .collect();
        let mut spec = SyntheticLedgerSpec::new(records, instances);
        spec.mix.frames_per_run_min = FRAMES_PER_RUN;
        spec.mix.frames_per_run_max = FRAMES_PER_RUN;
        spec.mix.frame_artifact_every_frames = STORED_FRAME_EVERY;
        let generated = write_synthetic_ledger(&database, &spec).expect("the synthetic ledger");
        assert_eq!(generated.records, records, "{generated:?}");
        drop(database);
        let database = Arc::new(
            RuntimeStateStore::open_database(dir.path(), SALT).expect("the State database"),
        );
        let limits = MaintenanceLimits::default();
        let ledger = LedgerMaintenance::acquire(
            dir.path(),
            false,
            limits,
            limits.deadline().expect("deadline"),
        )
        .expect("the maintenance owner")
        .open_writer(database, OWNER.into())
        .expect("the writer opens");
        let (single_at, repeated_at, reports) = {
            let mut appender = Appender::new(&ledger, dir.path());
            let history_intents = records / INTENT_EVERY;
            for index in 0..history_intents / 2 {
                appender.history_intent(index);
            }
            let links = appender.run_links();
            let single_at = appender.committed_intent(SINGLE_DECISION, links);
            let links = appender.run_links();
            let first_at = appender.committed_intent(REPEATED_DECISION, links.clone());
            let second_at = appender.committed_intent(REPEATED_DECISION, links);
            for index in history_intents / 2..history_intents {
                appender.history_intent(index);
            }
            let reports = REPORTS
                .iter()
                .map(|text| appender.report(text))
                .collect::<Vec<_>>();
            appender.commit();
            (single_at, (first_at, second_at), reports)
        };
        let head = ledger.latest_sequence().expect("the head");
        Self {
            dir,
            ledger,
            head,
            single_at,
            repeated_at,
            reports,
        }
    }

    /// The records of one event type through the head, read by the type's history.
    fn of_type(&self, event_type: EventType) -> Vec<PersistedEvent> {
        self.ledger
            .query(EventQuery {
                to_sequence: Some(self.head),
                event_type: Some(event_type),
                ..EventQuery::default()
            })
            .expect("the type's records")
    }

    /// The SQLite virtual-machine steps of the statements the writer reads with while `read`
    /// runs, with what `read` gives.
    fn vm_steps<T>(&self, read: impl FnOnce(&GlobalLedger) -> T) -> (T, u64) {
        let recorder =
            WriterStatementRecorder::arm(self.dir.path()).expect("the statement recorder");
        let value = read(&self.ledger);
        // A command is recorded once it has replied: one more read makes every earlier one
        // visible.
        self.ledger.latest_sequence().expect("the head");
        let steps: u64 = recorder
            .take()
            .iter()
            .flat_map(|command| command.statements.iter())
            .map(|statement| statement.vm_steps)
            .sum();
        (value, steps)
    }

    fn close(self) {
        self.ledger.close().expect("close the writer");
    }
}

/// What one root measured: its intents and verified records, and the work of each bounded read.
#[derive(Debug)]
struct Measured {
    records: u64,
    intents: usize,
    verified: usize,
    intent_steps: u64,
    verified_steps: u64,
}

#[test]
fn ln_ws_f_t1_a_bounded_read_does_not_grow_with_the_history() {
    let mut measured = Vec::new();
    for records in [SMALL_RECORDS, LARGE_RECORDS] {
        let root = Root::new(records);
        let (intents, intent_steps) = root.vm_steps(|ledger| {
            policy_dispatch_intents(
                ledger,
                SettlementRead::Tolerant,
                SINGLE_DECISION,
                Some(root.single_at),
                root.head,
                HostLocation::SelectPolicySettlements,
            )
            .expect("the decision's intent")
        });
        assert_eq!(
            intents
                .iter()
                .map(PersistedEvent::sequence)
                .collect::<Vec<_>>(),
            vec![root.single_at],
            "{records} records: the decision's one intent"
        );
        let (verified, verified_steps) = root.vm_steps(|ledger| {
            verified_reference_sequences(ledger, &root.reports, HostLocation::VerifyProposalReports)
                .expect("the reports' verified records")
        });
        assert!(
            verified.len() == REPORTS.len() && verified.iter().all(Option::is_some),
            "{records} records: both reports are verified: {verified:?}"
        );
        measured.push(Measured {
            records,
            intents: root.of_type(EventType::PolicyDispatchIntent).len(),
            verified: root.of_type(EventType::ArtifactVerified).len(),
            intent_steps,
            verified_steps,
        });
        root.close();
    }
    let [small, large] = measured.as_slice() else {
        panic!("two roots measured: {measured:?}");
    };
    // The histories are what they claim: the larger holds about four times the dispatch
    // intents and the verified records of the smaller.
    assert!(
        large.intents >= 3 * small.intents && large.verified >= 3 * small.verified,
        "{measured:?}"
    );
    for (read, small_steps, large_steps) in [
        (
            "a decision's dispatch intent",
            small.intent_steps,
            large.intent_steps,
        ),
        (
            "the reports' verified records",
            small.verified_steps,
            large.verified_steps,
        ),
    ] {
        assert!(
            small_steps > 0,
            "{read}: the writer read nothing: {measured:?}"
        );
        assert!(
            2 * large_steps <= 3 * small_steps,
            "{read}: {large_steps} VM steps on {} records, {small_steps} on {}",
            large.records,
            small.records
        );
    }
}

#[test]
fn ln_ws_f_t2_a_bounded_read_answers_as_the_history_wide_read() {
    let root = Root::new(SMALL_RECORDS);
    let issuer = IdentifierIssuer::new().expect("identifier issuer");

    // Today's intent read: every intent of the type, filtered by its decision, at most two.
    let intents = root.of_type(EventType::PolicyDispatchIntent);
    let history_intents = |decision_id: &str| {
        intents
            .iter()
            .filter(|event| intent_of(event, decision_id))
            .take(2)
            .map(PersistedEvent::sequence)
            .collect::<Vec<_>>()
    };
    let (first_at, second_at) = root.repeated_at;
    assert_eq!(history_intents(SINGLE_DECISION), vec![root.single_at]);
    assert_eq!(
        history_intents(REPEATED_DECISION),
        vec![first_at, second_at]
    );
    assert!(history_intents(NONE_DECISION).is_empty());
    for (case, decision_id, intent_sequence) in [
        ("one intent", SINGLE_DECISION, Some(root.single_at)),
        ("one intent, no journal sequence", SINGLE_DECISION, None),
        (
            "two intents, the journal holding the first",
            REPEATED_DECISION,
            Some(first_at),
        ),
        (
            "two intents, the journal holding the second",
            REPEATED_DECISION,
            Some(second_at),
        ),
        (
            "none: the sequence holds another decision's intent",
            NONE_DECISION,
            Some(root.single_at),
        ),
        ("none, no journal sequence", NONE_DECISION, None),
    ] {
        let expected = history_intents(decision_id);
        let bounded = policy_dispatch_intents(
            &root.ledger,
            SettlementRead::Tolerant,
            decision_id,
            intent_sequence,
            root.head,
            HostLocation::SelectPolicySettlements,
        )
        .expect(case)
        .iter()
        .map(PersistedEvent::sequence)
        .collect::<Vec<_>>();
        assert_eq!(bounded, expected, "{case}");
        // None or more than one is `policy_dispatch_intent_missing`, as today.
        let one = policy_dispatch_intent(
            &root.ledger,
            SettlementRead::Tolerant,
            decision_id,
            intent_sequence,
            root.head,
            HostLocation::SelectPolicySettlements,
        );
        match (expected.as_slice(), one) {
            ([sequence], Ok(intent)) => assert_eq!(intent.sequence(), *sequence, "{case}"),
            ([_], Err(error)) => panic!("{case}: {error:?}"),
            (_, Ok(intent)) => panic!("{case}: one intent read: {}", intent.sequence()),
            (_, Err(error)) => assert_eq!(
                error.code(),
                HostCode::PolicyDispatchIntentMissing.as_str(),
                "{case}"
            ),
        }
    }

    // Today's verified read: every `artifact.verified` record, the first equal reference's.
    let mut history = Vec::new();
    for event in root.of_type(EventType::ArtifactVerified) {
        history.extend(
            event
                .artifacts()
                .iter()
                .map(|artifact| (event.sequence(), artifact.project(true))),
        );
    }
    let first_verified = |reference: &ProjectedArtifactReference| {
        history
            .iter()
            .find_map(|(sequence, artifact)| (artifact == reference).then_some(*sequence))
    };
    let [first, second] = root.reports.as_slice() else {
        panic!("two reports: {:?}", root.reports);
    };
    let mut other_digest = first.clone();
    other_digest.sha256 = format!("sha256:{}", "0".repeat(64));
    let mut unknown = second.clone();
    unknown.artifact_id = *issuer
        .mint_artifact_id()
        .expect("an artifact id")
        .transport();
    let references = vec![first.clone(), other_digest, second.clone(), unknown];
    let expected = references.iter().map(&first_verified).collect::<Vec<_>>();
    assert!(
        expected[0].is_some()
            && expected[1].is_none()
            && expected[2].is_some()
            && expected[3].is_none(),
        "the references are what they claim: {expected:?}"
    );
    assert_eq!(
        verified_reference_sequences(
            &root.ledger,
            &references,
            HostLocation::VerifyStrategicEvidence,
        )
        .expect("the references together"),
        expected,
        "the references together"
    );
    for (reference, expected) in references.iter().zip(&expected) {
        assert_eq!(
            verified_reference_sequences(
                &root.ledger,
                std::slice::from_ref(reference),
                HostLocation::VerifyProposalReports,
            )
            .expect("one reference"),
            vec![*expected],
            "{reference:?}"
        );
    }
    root.close();
}

/// The read hold a hook arms right before its reads; once they have returned, whether the hold
/// held a read, and the hold released.
struct HeldRead {
    gate: Option<WriterReadHoldGate>,
    held: Arc<AtomicBool>,
}

impl Drop for HeldRead {
    fn drop(&mut self) {
        if let Some(gate) = self.gate.take() {
            self.held
                .store(gate.wait_held(Duration::ZERO), Ordering::SeqCst);
            gate.release();
        }
    }
}

/// Installs on this thread, in `hook`, the read hold of `root` around the next reads the hook
/// names: whether it held a read, once they have returned.
fn hold_next_read(hook: &'static LocalKey<AroundReadForTest>, root: &Path) -> Arc<AtomicBool> {
    let held = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&held);
    let state_root = root.to_path_buf();
    hook.with(|around| {
        *around.borrow_mut() = Some(Box::new(move || -> Box<dyn std::any::Any> {
            Box::new(HeldRead {
                gate: Some(WriterReadHoldGate::arm(&state_root).expect("arm the read hold")),
                held: flag,
            })
        }));
    });
    held
}

/// The settlement records of the run `run_id`: its execution and its completion.
fn settlement_records(host: &RuntimeHost, run_id: RunId) -> usize {
    host.query_persisted_events_for_test(EventQuery {
        run_id: Some(run_id),
        ..EventQuery::default()
    })
    .expect("the run's records")
    .iter()
    .filter(|event| {
        matches!(
            event.event_type(),
            EventType::PolicyExecutionRecorded | EventType::PolicyDispatchCompleted
        )
    })
    .count()
}

#[test]
fn ln_ws_f_t3_a_settlement_read_not_answered_in_time_skips_only_its_settlement_this_tick() {
    let root = TempDir::new().expect("tempdir");
    let (host, _state, context, request, _policy_time, _clock) =
        admitted_mapped_run_fixture_configured(&root, "settlement-read-held", |config| {
            config
                .with_frame_retention_enabled(false)
                .with_ledger_reply_timeout(REPLY_TIMEOUT)
        });
    host.run_scheduled_contained_task(&context, &request)
        .expect("one mapped contained run");
    assert_eq!(
        settlement_records(&host, context.run_id()),
        0,
        "the run's settlement is pending"
    );

    // The tick's read of the pending settlement's dispatch intent waits out the reply limit.
    let held = hold_next_read(&AROUND_SETTLEMENT_READS, root.path());
    if let Err(error) = host.evaluate_policy_cycle(PolicyTrigger::Reconciliation) {
        panic!("the tick goes on past the skipped settlement: {error:?}");
    }
    assert!(
        held.load(Ordering::SeqCst),
        "the settlement's read was held past the reply limit"
    );
    assert!(
        host.fatal_error().expect("health").is_none(),
        "nothing latched"
    );
    assert!(
        !host.is_shutdown_requested().expect("shutdown flag"),
        "no stop requested"
    );
    assert_eq!(
        settlement_records(&host, context.run_id()),
        0,
        "the skipped settlement wrote nothing"
    );
    assert!(
        host.pinned_policy_catalog(context.decision_id())
            .expect("catalog pin")
            .is_some(),
        "the dispatch is still open"
    );

    // The next tick settles it.
    if let Err(error) = host.evaluate_policy_cycle(PolicyTrigger::Reconciliation) {
        panic!("the next tick settles: {error:?}");
    }
    assert_eq!(
        settlement_records(&host, context.run_id()),
        2,
        "the settlement's execution and completion, once each"
    );
    assert!(
        host.pinned_policy_catalog(context.decision_id())
            .expect("catalog pin")
            .is_none(),
        "the dispatch is closed"
    );
    assert!(
        host.fatal_error().expect("health").is_none(),
        "nothing latched"
    );
    host.close().expect("close host");
}

/// A request of an agent that compiles `proposal`.
fn compile_request(ids: &IdentifierIssuer, proposal: &CatalogProposal) -> RuntimeRequest {
    RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Agent,
        EventSource::Adapter,
        unix_ms_now().expect("wall clock"),
        RuntimeOperation::CompileProposal {
            proposal: Box::new(proposal.clone()),
        },
    )
    .expect("agent runtime request")
}

#[test]
fn ln_ws_f_t4_a_proposal_read_not_answered_in_time_fails_only_that_request() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root)
            .with_frame_retention_enabled(false)
            .with_ledger_reply_timeout(REPLY_TIMEOUT),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    let base = host
        .activate_policy_catalog(&policy_sources(1))
        .expect("base catalog");
    let report = host
        .store_test_report(b"synthetic immutable strategy report")
        .expect("strategy report");
    let proposal = CatalogProposal::new(
        base.catalog_hash(),
        base.catalog_version(),
        2,
        vec![report],
        ProposalKind::ParameterInstantiation {
            instantiation: TaskTemplateInstantiation::new(
                "fixture.observe",
                "fixture.observe-copy",
                POLICY_INSTANCE_ALIAS,
                Some(110),
                Some(1_100),
            )
            .expect("template instantiation"),
        },
    )
    .expect("a proposal");
    let ids = IdentifierIssuer::new().expect("identifier issuer");

    // The request's read of its reports waits out the reply limit.
    let held = hold_next_read(&AROUND_VERIFIED_READ, root.path());
    let failed = host
        .process_request_for_test(
            &compile_request(&ids, &proposal),
            ConnectionId::new(1).expect("connection"),
        )
        .expect("a receipt");
    assert!(
        held.load(Ordering::SeqCst),
        "the reports' read was held past the reply limit"
    );
    assert_eq!(failed.state(), RuntimeReceiptState::Failed, "{failed:?}");
    assert_eq!(
        failed.outcome().code().as_str(),
        LedgerCode::WriterResponseTimeout.as_str(),
        "{failed:?}"
    );
    assert!(
        !failed
            .error_projection()
            .expect("the request's failure")
            .fatal,
        "{failed:?}"
    );
    assert!(
        host.fatal_error().expect("health").is_none(),
        "nothing latched"
    );
    assert!(
        !host.is_shutdown_requested().expect("shutdown flag"),
        "no stop requested"
    );

    // The Runtime keeps running: the same proposal is evaluated.
    let evaluated = host
        .process_request_for_test(
            &compile_request(&ids, &proposal),
            ConnectionId::new(2).expect("connection"),
        )
        .expect("a receipt");
    assert!(
        matches!(
            evaluated.result(),
            Some(RuntimeResult::ProposalEvaluated { .. })
        ),
        "{evaluated:?}"
    );
    assert!(
        host.fatal_error().expect("health").is_none(),
        "nothing latched"
    );
    host.close().expect("close host");
}

/// Changes the last digit of the keyed tag of the dispatch intent of the run `run_id`: the row no
/// longer equals its projection over its stored bytes.
fn damage_dispatch_intent(root: &Path, run_id: RunId) {
    let database = RuntimeDatabase::open_existing(root, false).expect("state database");
    let connection = database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("state database connection");
    let run = serde_json::to_value(run_id).expect("the run id");
    let run = run.as_str().expect("the run id's text").to_owned();
    let stored: i64 = connection
        .query_row(
            "SELECT e.sequence FROM ledger_events e JOIN ledger_links l ON l.sequence = e.sequence WHERE e.event_type = 'policy.dispatch_intent' AND l.run_id = ?1",
            [run],
            |row| row.get(0),
        )
        .expect("the run's dispatch intent");
    connection
        .execute_batch(&format!(
            "UPDATE ledger_events SET integrity_tag = substr(integrity_tag, 1, length(integrity_tag) - 1) || CASE substr(integrity_tag, -1) WHEN '0' THEN '1' ELSE '0' END WHERE sequence = {stored}"
        ))
        .expect("damage the row");
}

#[test]
fn ln_ws_f_t5_a_damaged_record_on_a_settlement_read_still_stops_the_runtime() {
    let root = TempDir::new().expect("tempdir");
    let (host, _state, context, request, _policy_time, _clock) =
        admitted_mapped_run_fixture_configured(&root, "settlement-read-damaged", |config| {
            config.with_frame_retention_enabled(false)
        });
    host.run_scheduled_contained_task(&context, &request)
        .expect("one mapped contained run");
    damage_dispatch_intent(root.path(), context.run_id());

    let error = match host.evaluate_policy_cycle(PolicyTrigger::Reconciliation) {
        Ok(_) => panic!("the damaged dispatch intent fails the tick"),
        Err(error) => error,
    };
    assert!(error.is_fatal(), "{error:?}");
    assert_eq!(
        error.operation(),
        HostLocation::SelectPolicySettlements.as_str(),
        "{error:?}"
    );
    let latched = host
        .fatal_error()
        .expect("health")
        .unwrap_or_else(|| panic!("the Runtime latched the failure: {error:?}"));
    assert!(
        latched.is_fatal() && latched.halts_runtime(),
        "{latched:?} {error:?}"
    );
    assert!(
        host.is_shutdown_requested().expect("shutdown flag"),
        "the latch stops the Runtime"
    );
    host.close()
        .expect_err("the Runtime ends with its fatal failure");
}

/// Whether `outcome` names the ledger's `writer_response_timeout` as its cause.
fn caused_by_writer_response_timeout(outcome: &Outcome) -> bool {
    outcome
        .causes()
        .iter()
        .any(|link| link.outcome().code().as_str() == LedgerCode::WriterResponseTimeout.as_str())
}

#[test]
fn ln_ws_f_t6_a_completion_read_not_answered_after_the_execution_commit_stops_the_runtime() {
    let root = TempDir::new().expect("tempdir");
    let (host, _state, context, request, _policy_time, _clock) =
        admitted_mapped_run_fixture_configured(&root, "completion-read-held", |config| {
            config
                .with_frame_retention_enabled(false)
                .with_ledger_reply_timeout(REPLY_TIMEOUT)
        });
    let receipt = host
        .run_scheduled_contained_task(&context, &request)
        .expect("one mapped contained run");

    // The live settlement appends and commits the run's execution record, then reads its
    // completion; that read waits out the reply limit.
    let held = hold_next_read(&AROUND_COMPLETION_READ, root.path());
    let error = host
        .complete_scheduled_policy_run(&context, &receipt)
        .expect_err("the completion read was not answered in time");
    assert!(
        held.load(Ordering::SeqCst),
        "the completion read was held past the reply limit"
    );
    assert!(error.is_fatal(), "{error:?}");
    assert_eq!(error.code(), "ledger_failure", "{error:?}");
    assert_eq!(
        error.operation(),
        HostLocation::ReadPolicyDispatchCompletion.as_str(),
        "{error:?}"
    );
    assert!(
        caused_by_writer_response_timeout(&error.outcome()),
        "the failure names the ledger's code as its cause: {error:?}"
    );
    let latched = host
        .fatal_error()
        .expect("health")
        .unwrap_or_else(|| panic!("the Runtime latched the failure: {error:?}"));
    assert!(
        latched.is_fatal() && latched.halts_runtime(),
        "{latched:?} {error:?}"
    );
    assert!(
        caused_by_writer_response_timeout(&latched.outcome()),
        "the latched failure names the ledger's code as its cause: {latched:?}"
    );
    assert!(
        host.is_shutdown_requested().expect("shutdown flag"),
        "the latch stops the Runtime"
    );
    host.close()
        .expect_err("the Runtime ends with its fatal failure");
}

/// T7: the daemon's log of the steps the host skipped, as actingd's reporter would write them.
#[derive(Default)]
struct DaemonLog {
    skipped: Mutex<Vec<Outcome>>,
}

impl DaemonLog {
    /// The skipped steps of the tick's settlement selection that the writer did not answer.
    fn unanswered_settlement_reads(&self) -> usize {
        let operation = serde_json::Value::from(HostLocation::SelectPolicySettlements.as_str());
        self.skipped
            .lock()
            .expect("daemon log")
            .iter()
            .filter(|outcome| {
                outcome.code().as_str() == LedgerCode::WriterResponseTimeout.as_str()
                    && outcome.value("operation") == Some(&operation)
            })
            .count()
    }
}

impl RuntimeDaemonReporter for DaemonLog {
    fn fatal_latched(&self, _outcome: &Outcome) {}

    fn step_skipped(&self, outcome: &Outcome) {
        self.skipped
            .lock()
            .expect("daemon log")
            .push(outcome.clone());
    }

    fn warning_recorded(&self, _outcome: &Outcome) {}
}

// T7: the outcome key the producers' package maps its designated effect to.
const TWO_SETTLEMENTS_KEY: &str = "settlement-reads-held";

/// T7: `facts` with instance B on its own host, as `pending_policy_facts` adds it.
fn with_instance_b(mut facts: EvaluationFacts) -> EvaluationFacts {
    facts.instances.push(InstanceSnapshot {
        instance_id: POLICY_INSTANCE_ALIAS_B.to_owned(),
        server_id: "fixture-server-b".to_owned(),
        game_id: "fixture-game-a".to_owned(),
        host_id: "fixture-host-b".to_owned(),
        available: true,
        capability_operation_ids: vec!["operation.observe".to_owned()],
        preferred_task_ids: Vec::new(),
    });
    facts
}

/// T7: the mapped catalog with a copy of its producer on instance B (`fixture.observe-b`), as
/// `pending_policy_sources` adds one.
fn mapped_sources_with_instance_b() -> CatalogSources {
    let mut sources = mapped_policy_sources(1, TWO_SETTLEMENTS_KEY);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("the mapped tasks");
    let mut producer_b = tasks["tasks"][0].clone();
    producer_b["id"] = serde_json::json!("fixture.observe-b");
    producer_b["scope"] =
        serde_json::json!({"kind": "instance", "instance_id": POLICY_INSTANCE_ALIAS_B});
    producer_b["procedure_ref"] = serde_json::json!("procedure.observe-b");
    producer_b["produces"] = serde_json::json!([]);
    producer_b["instance_overrides"] = serde_json::json!([]);
    tasks["tasks"]
        .as_array_mut()
        .expect("the mapped tasks array")
        .push(producer_b);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("the mapped task bytes");
    sources
}

/// T7: a host on a fake Runtime clock with instances A and B, a 3 s writer reply limit and `log`
/// as its daemon log, on which the producer of each instance was admitted and ran once, as the
/// mapped-run fixture runs one: two settlements pending. Gives the host and the two runs'
/// contexts.
fn two_pending_settlements(
    root: &TempDir,
    log: &Arc<DaemonLog>,
) -> (RuntimeHost, Vec<PolicyRunContext>) {
    let clock = Arc::new(ManualRuntimeClock::new(
        POLICY_NOW_UNIX_MS,
        POLICY_NOW_UNIX_MS,
    ));
    let package =
        neutral_mapped_contained_task_package(TWO_SETTLEMENTS_KEY, "designated_effect_completed");
    let package_path = root.path().join("two-settlements-mapped-task.zip");
    fs::write(&package_path, &package).expect("write the mapped package");
    let request = ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
        .expect("the mapped package request");
    let facts = with_instance_b(mapped_policy_facts(TWO_SETTLEMENTS_KEY, false));
    let host = RuntimeHost::start(
        config(root)
            .with_runtime_clock(clock)
            .with_policy_inputs(PolicyInputSnapshot::new(
                facts.clone(),
                pending_policy_resources(),
            ))
            .with_procedure_manifest(procedure_manifest_with_primary(
                &package,
                vec!["after_observation".to_owned()],
            ))
            .with_frame_retention_enabled(false)
            .with_ledger_reply_timeout(REPLY_TIMEOUT)
            .with_daemon_reporter(log.clone()),
        Arc::new(
            FakeProvider::from_entries([POLICY_INSTANCE_ALIAS, POLICY_INSTANCE_ALIAS_B].map(
                |alias| {
                    let state = Arc::new(FakeState::default());
                    state
                        .transition_capture_after_input
                        .store(true, Ordering::Release);
                    (alias.to_owned(), instance_id(), state)
                },
            ))
            .fixture_simulation(),
        ),
    )
    .expect("the two-instance host");
    host.activate_policy_catalog(&mapped_sources_with_instance_b())
        .expect("activate the catalog");
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &facts,
            &pending_policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            70,
            PolicyTrigger::FactsChanged,
        )
        .expect("the evaluation");
    let evaluation = cycle.evaluation.expect("an evaluation");
    let mut contexts = Vec::new();
    for (task_id, alias) in [
        ("fixture.observe", POLICY_INSTANCE_ALIAS),
        ("fixture.observe-b", POLICY_INSTANCE_ALIAS_B),
    ] {
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == task_id && intent.instance_id == alias)
            .unwrap_or_else(|| panic!("{task_id} on {alias} is dispatched: {evaluation:#?}"))
            .clone();
        let reasons = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("the dispatch's reason chain")
            .clone();
        if contexts.is_empty() {
            record_policy_approval(&host, &intent);
        }
        host.capacity_sampler_for_test().expect("capacity owner")()
            .expect("a capacity sample at the admission time");
        let PolicyDispatchAdmission::Granted { context } = host
            .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
            .unwrap_or_else(|error| panic!("{task_id} is admitted: {error:?}"))
        else {
            panic!("expected a granted dispatch of {task_id}")
        };
        host.run_scheduled_contained_task(&context, &request)
            .unwrap_or_else(|error| panic!("{task_id} runs: {error:?}"));
        contexts.push(*context);
    }
    (host, contexts)
}

#[test]
fn ln_ws_f_t7_the_first_unanswered_settlement_read_ends_the_ticks_reads() {
    let root = TempDir::new().expect("tempdir");
    let log = Arc::new(DaemonLog::default());
    let (host, contexts) = two_pending_settlements(&root, &log);
    for context in &contexts {
        assert_eq!(
            settlement_records(&host, context.run_id()),
            0,
            "both settlements are pending"
        );
    }

    // The writer is held from the tick's first settlement read to the end of the selection: the
    // first read waits out the reply limit, and no other settlement read is sent.
    let held = hold_next_read(&AROUND_SETTLEMENT_READS, root.path());
    if let Err(error) = host.evaluate_policy_cycle(PolicyTrigger::Reconciliation) {
        panic!("the tick goes on past the skipped reads: {error:?}");
    }
    assert!(
        held.load(Ordering::SeqCst),
        "the first settlement read was held past the reply limit"
    );
    assert_eq!(
        log.unanswered_settlement_reads(),
        1,
        "one settlement read waited out the reply limit"
    );
    assert!(
        host.fatal_error().expect("health").is_none(),
        "nothing latched"
    );
    assert!(
        !host.is_shutdown_requested().expect("shutdown flag"),
        "no stop requested"
    );
    for context in &contexts {
        assert_eq!(
            settlement_records(&host, context.run_id()),
            0,
            "neither settlement wrote anything"
        );
        assert!(
            host.pinned_policy_catalog(context.decision_id())
                .expect("catalog pin")
                .is_some(),
            "both dispatches are still open"
        );
    }

    // The next tick settles both.
    if let Err(error) = host.evaluate_policy_cycle(PolicyTrigger::Reconciliation) {
        panic!("the next tick settles: {error:?}");
    }
    for context in &contexts {
        assert_eq!(
            settlement_records(&host, context.run_id()),
            2,
            "each settlement's execution and completion, once each"
        );
        assert!(
            host.pinned_policy_catalog(context.decision_id())
                .expect("catalog pin")
                .is_none(),
            "both dispatches are closed"
        );
    }
    assert_eq!(
        log.unanswered_settlement_reads(),
        1,
        "the next tick waited for nothing"
    );
    assert!(
        host.fatal_error().expect("health").is_none(),
        "nothing latched"
    );
    host.close().expect("close host");
}
