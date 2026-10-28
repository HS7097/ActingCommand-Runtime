// SPDX-License-Identifier: AGPL-3.0-only

//! #422 QR-2b (brief rd-1028/brief-qr2b-diagnose.md, investigation
//! rd-1028/qr2-investigation.md §3). On production v0.12.21 `ac_diagnose` took 27 s and was not
//! answered (`runtime_receipt_header_failed` at the client's 5 s socket timeout): it listed the
//! window's runs twice, read the status of every listed run, asked for the run recovery of its
//! errors and failure pages, read the runs of one instance under a 20 s budget of their own, and
//! any page that failed failed the whole diagnosis. The required state:
//! - T1: one diagnosis lists the window's runs once, with and without an instance: one first
//!   page per listing type;
//! - T2: a run the listing shows succeeded or cancelled is never status-read; every other run a
//!   section needs is read once;
//! - T3: every QueryEvents page a diagnosis asks for asks for no run recovery (no part of it
//!   reads the groups);
//! - T4: a section the call's deadline passes in answers `<section>_incomplete` with
//!   `call_budget_exhausted` naming it, and the other sections answer as without the deadline;
//! - T5: a diagnosis with no slow section answers as before: `runs` are the failed and open runs
//!   among the 10 most recent the earlier reads found (`recent_runs` on one instance, the first
//!   page of `list_contained_runs` on all), `runs_grouped` and `failures_grouped` the groups of
//!   the window.
//!
//! The Runtime is A42's fake (`gate_411_a42`), which logs every QueryEvents page asked for. The
//! clock of T4 stands still until the fake has served a chosen page and is then past the
//! deadline: no sleep, and no real time decides where the deadline falls.

use super::gate_411_a42::{AskedPage, FakeRuntime, Recorder};
use super::gate_mcpt1::unreached_deadline;
use crate::codes::ActingctlCode;
use crate::diagnose::{self, Clock, DiagnoseRequest, Diagnosis, Section, SuspensionReport};
use crate::overview::Warning;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    AuditInput, ClientPayloadDraft, CommandPayloadDraft, DiagnosticCode, EffectDisposition,
    EventAction, EventActor, EventLinksDraft, EventOrigin, EventSource, EventType, InstanceId,
    IssuedInstanceId, OriginModule, PolicyDispatchEventData, PolicyPayloadDraft,
    PolicyReasonRecord, ProjectedEvent, RequestId, RuntimeErrorCode, RuntimeInstanceStatus,
    TaskEntryRecognitionPhase, TaskOutcome, TaskPayloadDraft, TaskSemanticFact,
};
use actingcommand_runtime_client::{
    ContainedRunState, ContainedRunStatus, RunListFilter, RuntimeChannel, RuntimeClient,
    RuntimeClientConfig,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// The window (since_unix_ms), after one run of the history before it.
const WINDOW_START_UNIX_MS: u64 = 4_102_444_800_000 + 3_000_000_000;
const ALIAS_A: &str = "neutral.diag.a";
const ALIAS_B: &str = "neutral.diag.b";
/// The policy task the scheduler dispatched A's failing runs for: an opaque id.
const POLICY_TASK: &str = "neutral.routine.collect";
const FINAL_PAGE: &str = "neutral/terminal";
const PAGE_UNKNOWN: SharedCode = SharedCode::ContainedTaskPageUnknown;
/// A spelling no catalog registers: its terminal keeps it as `raw_code` of
/// `outcome_relay_unregistered`, and `runs_grouped` reads it back.
const RELAYED_RUN_CODE: &str = "neutral_unlisted_step_failure";
/// The succeeded runs of each instance between the old and the recent failures: more than the 10
/// most recent runs hold, with and without an instance.
const SUCCEEDED_A: usize = 10;
const SUCCEEDED_B: usize = 6;
/// The record types a run is listed by (runtime-client `RUN_LISTING_EVENT_TYPES`).
const LISTING_TYPES: [EventType; 5] = [
    EventType::TaskRequested,
    EventType::TaskEntryPreflight,
    EventType::TaskCompleted,
    EventType::TaskFailed,
    EventType::TaskCancelled,
];

/// One recorded run: its request and the sequence of its first fact, its entry check (its first
/// listing fact in the window).
#[derive(Clone, Copy)]
struct Recorded {
    request: RequestId,
    first: u64,
}

impl Recorded {
    fn request_id(&self) -> Value {
        json!(self.request)
    }
}

/// How a run was dispatched, as the events before its facts record it.
enum Dispatched {
    /// By hand: its request opens with the CLI's command.
    Cli,
    /// By the scheduler for one policy task on one alias: the dispatch intent, under the
    /// admission request and linked to the run, then the scheduler's command.
    Scheduler(&'static str, &'static str),
}

/// How a run ended in the ledger.
enum Ending {
    Succeeded,
    Cancelled,
    Failed(Outcome),
    /// Its first step started; no terminal.
    Open,
}

/// The ledger: one run before the window; in it, an old failed run of each instance (A's
/// scheduled, B's with a relayed code), then A's and B's succeeded runs, one cancelled run of
/// each, a refused request on each, and last a recent failed run of each (A's scheduled) and
/// B's open run.
struct DiagLedger {
    events: Vec<ProjectedEvent>,
    instances: Vec<RuntimeInstanceStatus>,
    instance_a: InstanceId,
    old_a: Recorded,
    old_b: Recorded,
    recent_a: Recorded,
    recent_b: Recorded,
    open_b: Recorded,
    /// The succeeded and cancelled runs in the window.
    settled: Vec<Recorded>,
    rejected_a: u64,
    rejected_b: u64,
}

impl DiagLedger {
    fn build() -> Self {
        let mut ledger = Recorder::new(WINDOW_START_UNIX_MS - 60 * 60 * 1000);
        let instance_a = ledger.issuer.mint_instance_id().expect("instance id");
        let instance_b = ledger.issuer.mint_instance_id().expect("instance id");
        record_run(&mut ledger, instance_a, Dispatched::Cli, Ending::Succeeded);
        ledger.at(WINDOW_START_UNIX_MS);
        let old_a = record_run(
            &mut ledger,
            instance_a,
            Dispatched::Scheduler(ALIAS_A, POLICY_TASK),
            Ending::Failed(Outcome::new(PAGE_UNKNOWN)),
        );
        let old_b = record_run(
            &mut ledger,
            instance_b,
            Dispatched::Cli,
            Ending::Failed(Outcome::relayed(RELAYED_RUN_CODE)),
        );
        let mut settled = Vec::new();
        for index in 0..SUCCEEDED_A {
            settled.push(record_run(
                &mut ledger,
                instance_a,
                Dispatched::Cli,
                Ending::Succeeded,
            ));
            if index < SUCCEEDED_B {
                settled.push(record_run(
                    &mut ledger,
                    instance_b,
                    Dispatched::Cli,
                    Ending::Succeeded,
                ));
            }
        }
        for instance in [instance_a, instance_b] {
            settled.push(record_run(
                &mut ledger,
                instance,
                Dispatched::Cli,
                Ending::Cancelled,
            ));
        }
        let rejected_a = push_rejected(&mut ledger, instance_a);
        let rejected_b = push_rejected(&mut ledger, instance_b);
        let recent_a = record_run(
            &mut ledger,
            instance_a,
            Dispatched::Scheduler(ALIAS_A, POLICY_TASK),
            Ending::Failed(Outcome::new(PAGE_UNKNOWN)),
        );
        let recent_b = record_run(
            &mut ledger,
            instance_b,
            Dispatched::Cli,
            Ending::Failed(Outcome::new(PAGE_UNKNOWN)),
        );
        let open_b = record_run(&mut ledger, instance_b, Dispatched::Cli, Ending::Open);
        let instances = [(ALIAS_A, instance_a), (ALIAS_B, instance_b)]
            .into_iter()
            .map(|(alias, instance)| {
                RuntimeInstanceStatus::new(
                    alias,
                    *instance.transport(),
                    false,
                    0,
                    false,
                    false,
                    false,
                )
                .expect("instance status")
            })
            .collect();
        Self {
            events: ledger.events,
            instances,
            instance_a: *instance_a.transport(),
            old_a,
            old_b,
            recent_a,
            recent_b,
            open_b,
            settled,
            rejected_a,
            rejected_b,
        }
    }

    /// The `runs_grouped` of every instance: A's two scheduled failures on one page, then the
    /// single runs, the most recent first.
    fn every_runs_grouped(&self) -> Value {
        json!([
            self.page_unknown_scheduled_group(),
            {
                "count": 1,
                "first_seq": self.open_b.first,
                "last_seq": self.open_b.first,
                "latest_request_id": self.open_b.request_id(),
                "instances": [ALIAS_B],
            },
            {
                "failure_code": PAGE_UNKNOWN.as_str(),
                "final_page": FINAL_PAGE,
                "count": 1,
                "first_seq": self.recent_b.first,
                "last_seq": self.recent_b.first,
                "latest_request_id": self.recent_b.request_id(),
                "instances": [ALIAS_B],
            },
            {
                "failure_code": RELAYED_RUN_CODE,
                "final_page": FINAL_PAGE,
                "count": 1,
                "first_seq": self.old_b.first,
                "last_seq": self.old_b.first,
                "latest_request_id": self.old_b.request_id(),
                "instances": [ALIAS_B],
            },
        ])
    }

    fn page_unknown_scheduled_group(&self) -> Value {
        json!({
            "catalog_task_id": POLICY_TASK,
            "failure_code": PAGE_UNKNOWN.as_str(),
            "final_page": FINAL_PAGE,
            "count": 2,
            "first_seq": self.old_a.first,
            "last_seq": self.recent_a.first,
            "latest_request_id": self.recent_a.request_id(),
            "instances": [ALIAS_A],
        })
    }

    /// The `failures_grouped` of every instance, or of A: the refused requests.
    fn failures_grouped(&self, only_a: bool) -> Value {
        let last = if only_a {
            self.rejected_a
        } else {
            self.rejected_b
        };
        json!([{
            "type": "command.rejected",
            "code": DiagnosticCode::CommandRejected.code().as_str(),
            "count": if only_a { 1 } else { 2 },
            "first_seq": self.rejected_a,
            "last_seq": last,
        }])
    }
}

/// Appends one run on `instance`: the events of its dispatch, then its entry check, its main
/// admission, its first step and its terminal by `ending`.
fn record_run(
    ledger: &mut Recorder,
    instance: IssuedInstanceId,
    dispatched: Dispatched,
    ending: Ending,
) -> Recorded {
    let issuer = &ledger.issuer;
    let request = issuer.mint_request_id().expect("request id");
    let correlation_id = issuer.mint_correlation_id().expect("correlation id");
    let task_id = issuer.mint_task_id().expect("task id");
    let run_id = issuer.mint_run_id().expect("run id");
    let admission_request = issuer.mint_request_id().expect("admission request id");
    let action_id = issuer.mint_action_id().expect("intent action id");
    let links = EventLinksDraft::default()
        .with_instance_id(instance)
        .with_request_id(request)
        .with_correlation_id(correlation_id)
        .with_task_id(task_id)
        .with_run_id(run_id);
    match dispatched {
        Dispatched::Cli => ledger.push(
            EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::Cli),
            links.clone(),
            ClientPayloadDraft::cli_command(EventAction::RuntimeTaskRun, AuditInput::new()).into(),
        ),
        Dispatched::Scheduler(alias, catalog_task_id) => {
            let admission = EventLinksDraft::default()
                .with_instance_id(instance)
                .with_request_id(admission_request)
                .with_correlation_id(correlation_id)
                .with_task_id(task_id)
                .with_run_id(run_id)
                .with_action_id(action_id);
            ledger.push(
                EventOrigin::new(
                    EventSource::Scheduler,
                    OriginModule::Policy,
                    EventActor::Scheduler,
                ),
                admission,
                PolicyPayloadDraft::dispatch_intent(
                    dispatch_intent(alias, catalog_task_id),
                    AuditInput::new(),
                )
                .into(),
            );
            ledger.push(
                EventOrigin::new(
                    EventSource::Scheduler,
                    OriginModule::Scheduler,
                    EventActor::Scheduler,
                ),
                links.clone(),
                CommandPayloadDraft::received(EventAction::RuntimeTaskRun, AuditInput::new())
                    .into(),
            );
        }
    }
    let first = u64::try_from(ledger.events.len()).expect("ledger length") + 1;
    let mut facts = vec![
        TaskSemanticFact::EntryRecognition {
            phase: TaskEntryRecognitionPhase::Initial,
            required_pages: vec!["neutral/home".to_owned()],
            matched_page: Some("neutral/home".to_owned()),
            matched: true,
        },
        TaskSemanticFact::PackageAdmitted {
            package_label: "neutral.package".to_owned(),
            task_label: "neutral.task".to_owned(),
            response_deadline_monotonic_ms: None,
        },
        TaskSemanticFact::StepStarted {
            step_index: 0,
            operation_label: "open_terminal".to_owned(),
            from_page: "neutral/home".to_owned(),
            phase: None,
        },
    ];
    let terminal = |task_result, final_page: Option<&str>, executed_steps, outcome| {
        TaskSemanticFact::TerminalCommitted {
            task_result,
            final_page: final_page.map(str::to_owned),
            executed_steps: Some(executed_steps),
            outcome,
            failure_severe: false,
            scheduling_disposition: None,
            task_timing: None,
        }
    };
    match ending {
        Ending::Succeeded => facts.push(terminal(TaskOutcome::Success, Some(FINAL_PAGE), 1, None)),
        Ending::Cancelled => facts.push(terminal(
            TaskOutcome::Cancelled,
            None,
            0,
            Some(Outcome::new(RuntimeErrorCode::ContainedTaskCancelled)),
        )),
        Ending::Failed(outcome) => facts.push(terminal(
            TaskOutcome::Failure,
            Some(FINAL_PAGE),
            1,
            Some(outcome),
        )),
        Ending::Open => {}
    }
    for fact in facts {
        ledger.push(
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
            ),
            links.clone(),
            TaskPayloadDraft::semantic(fact, AuditInput::new()).into(),
        );
    }
    Recorded {
        request: *request.transport(),
        first,
    }
}

/// A refused request on `instance`; its sequence.
fn push_rejected(ledger: &mut Recorder, instance: IssuedInstanceId) -> u64 {
    let issuer = &ledger.issuer;
    let links = EventLinksDraft::default()
        .with_instance_id(instance)
        .with_request_id(issuer.mint_request_id().expect("request id"))
        .with_correlation_id(issuer.mint_correlation_id().expect("correlation id"));
    ledger.push(
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        links,
        CommandPayloadDraft::rejected(
            EventAction::RuntimeTaskRun,
            DiagnosticCode::CommandRejected,
            EffectDisposition::NotPerformed,
            AuditInput::new(),
        )
        .into(),
    );
    u64::try_from(ledger.events.len()).expect("ledger length")
}

/// The scheduler's dispatch intent for `catalog_task_id` on `alias`. Only its task_id is read;
/// the other members hold neutral values the contract accepts.
fn dispatch_intent(alias: &str, catalog_task_id: &str) -> PolicyDispatchEventData {
    let placeholder = |digit: char| format!("sha256:{}", digit.to_string().repeat(64));
    PolicyDispatchEventData {
        decision_id: "decision.diag.scheduled".to_owned(),
        task_id: catalog_task_id.to_owned(),
        instance_id: alias.to_owned(),
        operation_id: "operation.diag.scheduled".to_owned(),
        procedure_binding_digest: placeholder('b'),
        reason_chain_id: "reasons.diag.scheduled".to_owned(),
        reasons: vec![PolicyReasonRecord(Outcome::relayed("placement_selected"))],
        catalog_hash: placeholder('c'),
        catalog_version: 1,
        input_ledger_position: 1,
        fact_snapshot_id: "facts.diag.scheduled".to_owned(),
        approval_fact_ids: Vec::new(),
        urgency_milli: 0,
    }
}

/// A CLI client of the fake Runtime under `root`.
fn connect_client(root: &Path) -> RuntimeClient {
    RuntimeClient::connect(RuntimeClientConfig::new(
        root,
        RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect to the fake Runtime")
}

/// The fake Runtime of `ledger` and a client of it.
fn start(ledger: &DiagLedger) -> (TempDir, FakeRuntime, RuntimeClient) {
    let root = TempDir::new().expect("tempdir");
    let runtime = FakeRuntime::start_with_instances(
        root.path(),
        ledger.events.clone(),
        ledger.instances.clone(),
    );
    let client = connect_client(root.path());
    (root, runtime, client)
}

/// One diagnosis of the window, of A or of every instance, read until `deadline` on `clock`,
/// without its `actingd suspended` child (not part of this gate); and the pages it asked for.
fn diagnose_with(
    client: &RuntimeClient,
    runtime: &FakeRuntime,
    instance: Option<&str>,
    deadline: Instant,
    clock: &dyn Clock,
) -> (Diagnosis, Vec<AskedPage>) {
    let before = runtime.asked_count();
    let request = DiagnoseRequest {
        instance: instance.map(str::to_owned),
        task_id: None,
        since_unix_ms: Some(WINDOW_START_UNIX_MS),
    };
    let diagnosis = match diagnose::read(
        client,
        &request,
        WINDOW_START_UNIX_MS,
        SuspensionReport::not_started(Outcome::new(ActingctlCode::InstallRootUnresolved)),
        None,
        deadline,
        clock,
    ) {
        Ok((diagnosis, _)) => diagnosis,
        Err(failure) => panic!(
            "the diagnosis failed: {}",
            failure.outcome().code().as_str()
        ),
    };
    let asked = runtime.asked().split_off(before);
    (diagnosis, asked)
}

/// A diagnosis with the whole budget ahead, on the system's clock.
fn unhurried(
    client: &RuntimeClient,
    runtime: &FakeRuntime,
    instance: Option<&str>,
) -> (Diagnosis, Vec<AskedPage>) {
    diagnose_with(
        client,
        runtime,
        instance,
        unreached_deadline(),
        &diagnose::SystemClock,
    )
}

/// The first pages of the run listing, by listing type.
fn listing_first_pages(asked: &[AskedPage]) -> BTreeMap<String, usize> {
    let mut pages = BTreeMap::new();
    for page in asked {
        let query = &page.query;
        if let Some(event_type) = query.event_type
            && LISTING_TYPES.contains(&event_type)
            && page.first
            && query.from_timestamp_unix_ms == Some(WINDOW_START_UNIX_MS)
            && query.request_id.is_none()
            && query.view.is_none()
        {
            *pages.entry(json!(event_type).to_string()).or_insert(0) += 1;
        }
    }
    pages
}

/// The runs whose status was read (a first page by request without a sequence bound), each with
/// how many times.
fn status_reads(asked: &[AskedPage]) -> BTreeMap<RequestId, usize> {
    let mut reads = BTreeMap::new();
    for page in asked {
        if page.first
            && page.query.from_sequence.is_none()
            && let Some(request) = page.query.request_id
        {
            *reads.entry(request).or_insert(0) += 1;
        }
    }
    reads
}

/// The failed and open runs among `runs`, as `runs` shows them.
fn failed_or_open(runs: &[ContainedRunStatus]) -> Vec<Value> {
    runs.iter()
        .filter(|run| {
            matches!(
                run.state,
                ContainedRunState::Failed
                    | ContainedRunState::Admitted
                    | ContainedRunState::Running
                    | ContainedRunState::InterruptedUnterminated
            )
        })
        .map(|run| json!(run))
        .collect()
}

/// Whether `warnings` hold `call_budget_exhausted` naming `member`.
fn budget_warning(warnings: &[Warning], member: &str) -> bool {
    warnings.iter().any(|warning| {
        matches!(
            warning,
            Warning::Outcome(outcome)
                if outcome.is(ActingctlCode::CallBudgetExhausted)
                    && outcome.value("member") == Some(&json!(member))
        )
    })
}

/// The diagnosis rendered whole: the `<section>_incomplete` members are the rendering's.
fn rendered(diagnosis: &Diagnosis) -> Value {
    diagnosis.render(
        &diagnosis.kept_all(),
        json!({}),
        &super::super::tools::envelope,
    )
}

#[test]
fn qr2b_t1_one_call_lists_once_with_and_without_an_instance() {
    let ledger = DiagLedger::build();
    let (_root, runtime, client) = start(&ledger);
    for instance in [None, Some(ALIAS_A)] {
        let (_, asked) = unhurried(&client, &runtime, instance);
        let pages = listing_first_pages(&asked);
        // One query per listing type, each one first page: the window's runs are listed once
        // for both `runs` and `runs_grouped` (before: twice, once for each).
        assert_eq!(
            pages.values().copied().collect::<Vec<_>>(),
            vec![1; LISTING_TYPES.len()],
            "instance {instance:?}: the listing's first pages by type {pages:?}"
        );
        let listing_pages = asked
            .iter()
            .filter(|page| {
                page.query
                    .event_type
                    .is_some_and(|event_type| LISTING_TYPES.contains(&event_type))
                    && page.query.request_id.is_none()
            })
            .count();
        assert_eq!(
            listing_pages,
            LISTING_TYPES.len(),
            "instance {instance:?}: the listing's pages"
        );
    }
    drop(client);
    drop(runtime);
}

#[test]
fn qr2b_t2_settled_runs_are_not_status_read() {
    let ledger = DiagLedger::build();
    let (_root, runtime, client) = start(&ledger);
    let cases = [
        (
            None,
            vec![
                ledger.old_a,
                ledger.old_b,
                ledger.recent_a,
                ledger.recent_b,
                ledger.open_b,
            ],
        ),
        (Some(ALIAS_A), vec![ledger.old_a, ledger.recent_a]),
    ];
    for (instance, needed) in cases {
        let (_, asked) = unhurried(&client, &runtime, instance);
        let reads = status_reads(&asked);
        for settled in &ledger.settled {
            assert!(
                !reads.contains_key(&settled.request),
                "instance {instance:?}: a succeeded or cancelled run was read: {reads:?}"
            );
        }
        // Each failed or open run a section needs is read once (before: twice, and every
        // settled run too).
        let expected = needed
            .iter()
            .map(|run| (run.request, 1))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(reads, expected, "instance {instance:?}: the runs read");
    }
    drop(client);
    drop(runtime);
}

#[test]
fn qr2b_t3_every_page_asks_for_no_run_recovery() {
    let ledger = DiagLedger::build();
    let (_root, runtime, client) = start(&ledger);
    for instance in [None, Some(ALIAS_A)] {
        let (_, asked) = unhurried(&client, &runtime, instance);
        // Every kind of page a diagnosis sends was sent: the errors view, the failure records,
        // the listing, a run's status, a dispatch intent and a relayed terminal (without an
        // instance: B's old run).
        let kinds = [
            (
                "errors view",
                asked.iter().any(|page| page.query.view.is_some()),
            ),
            (
                "failure records",
                asked
                    .iter()
                    .any(|page| page.query.event_type == Some(EventType::CommandRejected)),
            ),
            ("listing", !listing_first_pages(&asked).is_empty()),
            ("status", !status_reads(&asked).is_empty()),
            (
                "dispatch intent",
                asked
                    .iter()
                    .any(|page| page.query.event_type == Some(EventType::PolicyDispatchIntent)),
            ),
        ];
        for (kind, sent) in kinds {
            assert!(sent, "instance {instance:?}: no {kind} page was asked for");
        }
        if instance.is_none() {
            assert!(
                asked
                    .iter()
                    .any(|page| page.query.request_id == Some(ledger.old_b.request)
                        && page.query.from_sequence.is_some()),
                "no relayed terminal page was asked for"
            );
        }
        let with_recovery = asked
            .iter()
            .filter(|page| page.run_recovery)
            .map(|page| &page.query)
            .collect::<Vec<_>>();
        assert!(
            with_recovery.is_empty(),
            "instance {instance:?}: pages asked for with their run recovery: {with_recovery:?}"
        );
    }
    drop(client);
    drop(runtime);
}

/// #422 QR-2b T4: a clock that stands at `base` until the fake Runtime has served, in this
/// call, a page `passed` matches, and is then past the deadline (`base` plus an hour).
struct PageClock<'a> {
    runtime: &'a FakeRuntime,
    from: usize,
    base: Instant,
    passed: fn(&AskedPage, &DiagLedger) -> bool,
    ledger: &'a DiagLedger,
}

impl Clock for PageClock<'_> {
    fn now(&self) -> Instant {
        let asked = self.runtime.asked();
        if asked[self.from..]
            .iter()
            .any(|page| (self.passed)(page, self.ledger))
        {
            self.base + Duration::from_secs(2 * 60 * 60)
        } else {
            self.base
        }
    }
}

#[test]
fn qr2b_t4_a_section_past_the_deadline_answers_incomplete_and_the_rest_answers() {
    let ledger = DiagLedger::build();
    let (_root, runtime, client) = start(&ledger);
    let (whole, _) = unhurried(&client, &runtime, None);
    assert!(
        whole.incomplete_sections.is_empty(),
        "setup: a section was cut"
    );

    // The deadline passes while `runs_grouped` reads: once B's old failed run, beyond the 10
    // most recent, has been read.
    let base = Instant::now();
    let clock = PageClock {
        runtime: &runtime,
        from: runtime.asked_count(),
        base,
        passed: |page, ledger| {
            page.first
                && page.query.from_sequence.is_none()
                && page.query.request_id == Some(ledger.old_b.request)
        },
        ledger: &ledger,
    };
    let deadline = base + Duration::from_secs(60 * 60);
    let (grouped_cut, _) = diagnose_with(&client, &runtime, None, deadline, &clock);
    assert_eq!(
        grouped_cut.incomplete_sections,
        vec![Section::RunsGrouped],
        "the sections cut"
    );
    assert!(
        budget_warning(&grouped_cut.warnings, "runs_grouped"),
        "call_budget_exhausted naming runs_grouped"
    );
    let result = rendered(&grouped_cut);
    assert_eq!(result["runs_grouped_incomplete"], json!(true), "{result}");
    assert_eq!(result["incomplete"], json!(true), "{result}");
    assert_eq!(result.get("runs_incomplete"), None, "{result}");
    // The rest answers as without the deadline.
    assert_eq!(grouped_cut.runs, whole.runs, "runs");
    assert_eq!(grouped_cut.errors, whole.errors, "errors_page");
    assert_eq!(
        grouped_cut.failures_grouped, whole.failures_grouped,
        "failures_grouped"
    );
    assert_eq!(
        grouped_cut.not_run.as_ref().map(|rows| rows.total),
        whole.not_run.as_ref().map(|rows| rows.total),
        "not_run"
    );
    assert!(grouped_cut.not_run.is_some(), "not_run was read");

    // The deadline passes while the window's runs are listed: neither run section is read on,
    // and both say so; the errors page, failures_grouped and not_run answer.
    let base = Instant::now();
    let clock = PageClock {
        runtime: &runtime,
        from: runtime.asked_count(),
        base,
        passed: |page, _| {
            page.first
                && page.query.event_type == Some(EventType::TaskRequested)
                && page.query.request_id.is_none()
                && page.query.from_timestamp_unix_ms.is_some()
        },
        ledger: &ledger,
    };
    let deadline = base + Duration::from_secs(60 * 60);
    let (listing_cut, _) = diagnose_with(&client, &runtime, None, deadline, &clock);
    assert_eq!(
        listing_cut.incomplete_sections,
        vec![Section::Runs, Section::RunsGrouped],
        "the sections cut"
    );
    for member in ["runs", "runs_grouped"] {
        assert!(
            budget_warning(&listing_cut.warnings, member),
            "call_budget_exhausted naming {member}"
        );
    }
    let result = rendered(&listing_cut);
    assert_eq!(result["runs_incomplete"], json!(true), "{result}");
    assert_eq!(result["runs_grouped_incomplete"], json!(true), "{result}");
    assert_eq!(listing_cut.errors, whole.errors, "errors_page");
    assert_eq!(
        listing_cut.failures_grouped, whole.failures_grouped,
        "failures_grouped"
    );
    assert!(listing_cut.not_run.is_some(), "not_run was read");
    drop(client);
    drop(runtime);
}

#[test]
fn qr2b_t5_an_answer_without_a_slow_section_is_as_before() {
    let ledger = DiagLedger::build();
    let (_root, runtime, client) = start(&ledger);

    // Every instance: `runs` as the first page of `list_contained_runs` gave them, the failed
    // and open ones of the 10 most recent.
    let (every, _) = unhurried(&client, &runtime, None);
    let before = client
        .list_contained_runs(
            WINDOW_START_UNIX_MS,
            None,
            &RunListFilter::default(),
            None,
            10,
            unreached_deadline(),
        )
        .expect("the run list of before");
    assert!(!before.incomplete, "setup: {before:?}");
    let before_runs = before
        .runs
        .into_iter()
        .map(|listed| listed.status)
        .collect::<Vec<_>>();
    assert_eq!(every.runs, failed_or_open(&before_runs), "runs");
    let recent_requests = every
        .runs
        .iter()
        .map(|run| run["request_id"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        recent_requests,
        vec![
            ledger.open_b.request_id(),
            ledger.recent_b.request_id(),
            ledger.recent_a.request_id(),
        ],
        "runs, newest first"
    );
    assert_eq!(
        Value::Array(every.runs_grouped.clone()),
        ledger.every_runs_grouped(),
        "runs_grouped"
    );
    assert_eq!(
        Value::Array(every.failures_grouped.clone()),
        ledger.failures_grouped(false),
        "failures_grouped"
    );
    assert!(
        !every.errors.is_empty(),
        "the errors page holds the failures"
    );
    assert!(
        every.incomplete_sections.is_empty() && every.warnings.is_empty(),
        "sections cut: {:?}",
        every.incomplete_sections
    );

    // One instance: `runs` as `recent_runs` gave them.
    let (one, _) = unhurried(&client, &runtime, Some(ALIAS_A));
    let before = client
        .recent_runs(ledger.instance_a, WINDOW_START_UNIX_MS, 10)
        .expect("the recent runs of before");
    assert!(!before.incomplete, "setup: {before:?}");
    assert_eq!(one.runs, failed_or_open(&before.runs), "runs of {ALIAS_A}");
    assert_eq!(
        one.runs
            .iter()
            .map(|run| run["request_id"].clone())
            .collect::<Vec<_>>(),
        vec![ledger.recent_a.request_id()],
        "runs of {ALIAS_A}: the old failure is not among its 10 most recent"
    );
    assert_eq!(
        Value::Array(one.runs_grouped.clone()),
        json!([ledger.page_unknown_scheduled_group()]),
        "runs_grouped of {ALIAS_A}"
    );
    assert_eq!(
        Value::Array(one.failures_grouped.clone()),
        ledger.failures_grouped(true),
        "failures_grouped of {ALIAS_A}"
    );
    assert!(
        one.incomplete_sections.is_empty() && one.warnings.is_empty(),
        "sections cut: {:?}",
        one.incomplete_sections
    );
    // The settled runs are among the window's runs: the 10 most recent counted them.
    let settled = ledger
        .settled
        .iter()
        .map(|run| run.request)
        .collect::<BTreeSet<_>>();
    assert!(
        before.runs.iter().any(|run| run
            .request_id
            .is_some_and(|request| settled.contains(&request))),
        "setup: no settled run among {ALIAS_A}'s 10 most recent"
    );
    drop(client);
    drop(runtime);
}
