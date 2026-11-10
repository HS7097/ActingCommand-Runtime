// SPDX-License-Identifier: AGPL-3.0-only

// #422 QP-1 (coordinator work order rd-1110/brief-qp1-page-plan-bounds.md; investigation
// rd-1110/ledger-window-plan-findings.md): a ledger page stays inside its sequence bounds whatever
// the planner chooses. A page's statements span segments of the sequence (LN-WS R2), but for a
// page without a per-run link nothing in a statement's text made the walk follow the segment:
// SQLite chose the index, and one index it may choose for a page with a time bound, the time
// index, takes no sequence bound (`timestamp_unix_ms` is its first column and the bound a range
// on it). Since QP-1 every time term is written with a unary `+`, and a statement without a
// per-run link writes its window's sequence terms so too, so the segment's pair is the only
// sequence range an index can take (`history_statement`).
//
// The fixture is a formal root whose Runtime writer appended mixed records: Lab anchors and the
// records that name their requests (directly and through runs), commands, captures, lease grants,
// runtime failures of each category, runtime facts and monitor records, on three instances and
// on none, two records to a time, one record stamped before the whole history and one moved back
// within it.
// - `page_statements_stay_inside_sequence_bounds`: for every view and none, and each filter a
//   page carries without a per-run link (severity bounds, module, excluded types, instance ids,
//   one instance, an upper, a lower and both time bounds, an event type, event types, a source)
//   and the severity bounds, module, excluded types, instance ids and both time bounds together,
//   with and without a window, the statement of a page's first, second and widest segment, at
//   the start and in the middle, takes its plan (`EXPLAIN QUERY PLAN` with its values bound) on
//   an empty database and on the fixture's, each without statistics and after `ANALYZE` (STAT4's:
//   the build has `ENABLE_STAT4` and `sqlite_stat4` has rows): the plan's first search of the
//   events or links table is a range that carries the segment's two bounds (by the row id, or as
//   an index's second term), any later one such a range or a point read by the sequence the
//   first gave, and no other step names either table; the text's only indexable sequence terms
//   are the segment's, and no time term is indexable. LN-8d's link-set statement with
//   `to_sequence` (the resident retention's object load) keeps the link-set page's form.
// - `time_bound_never_drives_a_page`: with a lower, an upper and both time bounds, without a
//   per-run link and with each per-run link (run, request and task, correlation, lease, frame,
//   one instance's run, causation, action, recognition), for every view and none, with and
//   without a window, no plan on those four databases uses the time index, and every time term
//   is written with a `+`.
// - `page_results_unchanged_by_plan_bounds`: paging each filter of the first test, in every view
//   and none, through windows at the start, in the middle and at the end of the fixture, on the
//   Runtime writer's reader pool (segments of 128 sequences), gives page by page the rows,
//   continuation, cursor and read scope of the same pages read in memory from every record
//   (`EventIndexes`), which reads no SQLite statement: the slice changed plans, not answers.

use super::plan_check::empty_database;
use super::*;
use crate::global::history::DEFAULT_HISTORY_SCAN_ROWS;
use crate::global::projection::PageSelection;
use crate::global::{
    GlobalLedger, LedgerFailurePlan, LedgerMaintenance, LedgerWritePolicy,
    Sha256SecretFingerprinter,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    AuditInput, CapturePayloadDraft, ClientPayloadDraft, CommandPayloadDraft, CorrelationId,
    DiagnosticCode, DiagnosticDetailDraft, EffectDisposition, EventAction, EventActor, EventDraft,
    EventLinksDraft, EventOrigin, EventPayloadDraft, EventSeverity, EventSource, FactValue,
    FrameId, IdentifierIssuer, InstanceId, IssuedCorrelationId, IssuedInstanceId, IssuedRequestId,
    LeaseId, LeasePayloadDraft, OriginModule, OwnerEpoch, PerformanceMetric,
    PerformanceMonitorHealth, PerformanceMonitorStateEventData, PerformancePayloadDraft, RequestId,
    RuntimeFactRecord, RuntimeFactScope, RuntimePayloadDraft, TaskId,
};
use actingcommand_runtime_database::{DatabaseLocation, MaintenanceLimits};
use actingcommand_runtime_state::RuntimeStateStore;
use std::collections::BTreeMap;
use std::path::Path;
use tempfile::TempDir;

// The State key (a bootstrap seed of at least 16 bytes), the writer's owner id and the drafts'
// fingerprint salt: neutral fixture words.
const STATE_KEY: &[u8] = b"ledger-qp1-fixture-key";
const OWNER: &str = "ledger-qp1-fixture";
const SALT: &[u8] = b"ledger-qp1-fixture-salt";
// The fixture's records: their number, the first one's time and the time between two pairs of
// them, a run every 24 records and the turn of their kinds.
const RECORDS: u64 = 1_200;
const FIRST_MS: u64 = 1_752_147_200_000;
const RECORD_INTERVAL_MS: u64 = 1_000;
const RUN_RECORDS: u64 = 24;
const KINDS: u64 = 12;
// The record moved back within the history, and how far.
const MOVED_BACK_RECORDS: u64 = 300;
// The frame size every capture names.
const FRAME_WIDTH: u32 = 1_280;
const FRAME_HEIGHT: u32 = 720;
// The write policy of the writer and the open's deadline (as LN-WS's), the wait of every caller
// for one writer reply and of the deferred appends' confirmation: no real wait of these tests
// reaches them.
const STALL_MS: u64 = 120_000;
const PROBE_MS: u64 = 1_000;
const DUMP_MAX_BYTES: u64 = 1_073_741_824;
const OPEN_TIMEOUT_SECONDS: u64 = 600;
const REPLY_TIMEOUT: Duration = Duration::from_secs(600);
const CONFIRM_WAIT: Duration = Duration::from_secs(600);
// The plans: a statement's rows (a page's default limit and its lookahead), and the second
// segment of a page (the first spans one sequence, the second two).
const STATEMENT_ROWS: usize = 257;
const SECOND_SEGMENT: u64 = 2;
// The pages: the sequences one statement spans at most, the page's limit, the windows' width,
// and the pages after which a broken paging loop ends.
const SCAN_ROWS: usize = 128;
const PAGE_LIMIT: u16 = 16;
const WINDOW: u64 = 200;
const MAX_PAGES: usize = 1_000;
const PROFILE: ProjectionProfile = ProjectionProfile::Forensic;
// The tables a page statement reads, by their names and the aliases it gives them; the time
// index.
const PAGE_TABLES: [&str; 4] = ["e", "l", "ledger_events", "ledger_links"];
const TIME_INDEX: &str = "ledger_view_index_time_v1";

fn epoch() -> OwnerEpoch {
    *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_owner_epoch()
        .expect("owner epoch")
        .transport()
}

/// The time of the fixture's record `index` (the first is 0): two records to a time; the record
/// halfway through is stamped before every other and the one at three quarters takes the time
/// of the record `MOVED_BACK_RECORDS` before it.
fn time(index: u64) -> u64 {
    if index == RECORDS / 2 {
        return FIRST_MS;
    }
    let stamped = if index == RECORDS * 3 / 4 {
        index - MOVED_BACK_RECORDS
    } else {
        index
    };
    FIRST_MS + RECORD_INTERVAL_MS * (stamped / 2 + 1)
}

/// One of the three instances, in turn.
fn turn<T: Copy>(values: &[T; 3], index: u64) -> T {
    values[usize::try_from(index % 3).expect("a turn")]
}

/// The views a page may name, and none.
fn views() -> impl Iterator<Item = Option<LedgerView>> {
    std::iter::once(None).chain(LedgerView::ALL.into_iter().map(Some))
}

/// A run's identifiers and the links its records carry.
struct Run {
    links: EventLinksDraft,
    request_links: EventLinksDraft,
    run: RunId,
    request: RequestId,
    correlation: CorrelationId,
    task: TaskId,
    lease: LeaseId,
}

impl Run {
    /// A run of `instance`; it names `named` (a Lab anchor's request and correlation) when given.
    fn mint(
        ids: &IdentifierIssuer,
        instance: IssuedInstanceId,
        named: Option<(IssuedRequestId, IssuedCorrelationId)>,
    ) -> Self {
        let (request, correlation) = named.unwrap_or_else(|| {
            (
                ids.mint_request_id().expect("a request id"),
                ids.mint_correlation_id().expect("a correlation id"),
            )
        });
        let task = ids.mint_task_id().expect("a task id");
        let run = ids.mint_run_id().expect("a run id");
        let lease = ids.mint_lease_id().expect("a lease id");
        let request_links = EventLinksDraft::default()
            .with_instance_id(instance)
            .with_request_id(request)
            .with_correlation_id(correlation);
        Self {
            links: request_links
                .clone()
                .with_task_id(task)
                .with_run_id(run)
                .with_lease_id(lease),
            request_links,
            run: *run.transport(),
            request: *request.transport(),
            correlation: *correlation.transport(),
            task: *task.transport(),
            lease: *lease.transport(),
        }
    }
}

/// The fixture's records, appended through the Runtime's writer as deferred appends.
struct Records<'a> {
    ledger: &'a GlobalLedger,
    ids: &'a IdentifierIssuer,
    fingerprinter: Sha256SecretFingerprinter,
}

impl Records<'_> {
    fn append(
        &self,
        at: u64,
        severity: EventSeverity,
        origin: (EventSource, OriginModule, EventActor),
        links: EventLinksDraft,
        payload: EventPayloadDraft,
    ) {
        let draft = EventDraft::new(
            self.ids.mint_event_id().expect("an event id"),
            at,
            severity,
            EventOrigin::new(origin.0, origin.1, origin.2),
            links,
            payload,
        )
        .sanitize(&self.fingerprinter)
        .expect("a fixture draft");
        self.ledger
            .append_deferred(draft)
            .expect("the record is accepted");
    }

    /// A `runtime.failed` whose outcome has the category `severity` (ledger revision 3: a
    /// record's severity is its outcome's category), through a registered code of it.
    fn failed(&self, at: u64, severity: EventSeverity, links: EventLinksDraft) {
        let code = match severity {
            EventSeverity::Warning => "step_budget_exhausted",
            EventSeverity::Error => "capture_failed",
            EventSeverity::Fatal => "watchdog_fatal_hold",
            _ => "max_steps_reached",
        };
        self.append(
            at,
            severity,
            (
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
            ),
            links,
            RuntimePayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::NotPerformed,
                Outcome::relayed(code),
                DiagnosticDetailDraft::new(
                    "fixture_category",
                    LedgerLocation::AppendEvent.location(),
                    "fixture_backend",
                    "fixture_operation",
                ),
                AuditInput::new(),
            )
            .into(),
        );
    }
}

/// What the queries name and read: the fixture's records through its head, its instances, its
/// last run and that run's last frame.
struct Values {
    events: Vec<PersistedEvent>,
    first: u64,
    head: u64,
    instances: [InstanceId; 3],
    run: Run,
    frame: FrameId,
}

impl Values {
    /// The time of the record at `sequence`.
    fn time_at(&self, sequence: u64) -> u64 {
        let at = self
            .events
            .binary_search_by_key(&sequence, PersistedEvent::sequence)
            .unwrap_or_else(|_| panic!("no record at {sequence}"));
        self.events[at].timestamp_unix_ms()
    }

    /// The filters a page carries without a per-run link, their time bounds inside `range`.
    fn page_filters(&self, (low, high): (u64, u64)) -> Vec<(&'static str, EventQuery)> {
        let third = (high - low) / 3;
        let (one, two) = (self.time_at(low + third), self.time_at(high - third));
        let (since, until) = (one.min(two), one.max(two).max(one.min(two) + 1));
        let [a, b, c] = self.instances;
        vec![
            ("no filter", EventQuery::default()),
            (
                "a minimum severity",
                EventQuery {
                    minimum_severity: Some(EventSeverity::Warning),
                    ..EventQuery::default()
                },
            ),
            (
                "a maximum severity",
                EventQuery {
                    maximum_severity: Some(EventSeverity::Info),
                    ..EventQuery::default()
                },
            ),
            (
                "severity bounds",
                EventQuery {
                    minimum_severity: Some(EventSeverity::Info),
                    maximum_severity: Some(EventSeverity::Error),
                    ..EventQuery::default()
                },
            ),
            (
                "a module",
                EventQuery {
                    origin_module: Some(OriginModule::Capture),
                    ..EventQuery::default()
                },
            ),
            (
                "excluded types",
                EventQuery {
                    exclude_event_types: vec![
                        EventType::RuntimeFactRecorded,
                        EventType::PerformanceMonitorDegraded,
                    ],
                    ..EventQuery::default()
                },
            ),
            (
                "instance ids",
                EventQuery {
                    instance_ids: vec![a, b],
                    ..EventQuery::default()
                },
            ),
            (
                "one instance",
                EventQuery {
                    instance_id: Some(c),
                    ..EventQuery::default()
                },
            ),
            (
                "a lower time bound",
                EventQuery {
                    from_timestamp_unix_ms: Some(since),
                    ..EventQuery::default()
                },
            ),
            (
                "an upper time bound",
                EventQuery {
                    to_timestamp_unix_ms: Some(until),
                    ..EventQuery::default()
                },
            ),
            (
                "time bounds",
                EventQuery {
                    from_timestamp_unix_ms: Some(since),
                    to_timestamp_unix_ms: Some(until),
                    ..EventQuery::default()
                },
            ),
            (
                "an event type",
                EventQuery {
                    event_type: Some(EventType::CaptureCompleted),
                    ..EventQuery::default()
                },
            ),
            (
                "event types",
                EventQuery {
                    event_types: vec![EventType::LabRequest, EventType::LeaseGranted],
                    ..EventQuery::default()
                },
            ),
            (
                "a source",
                EventQuery {
                    source: Some(EventSource::Lab),
                    ..EventQuery::default()
                },
            ),
            (
                "every filter",
                EventQuery {
                    minimum_severity: Some(EventSeverity::Debug),
                    maximum_severity: Some(EventSeverity::Fatal),
                    origin_module: Some(OriginModule::Capture),
                    exclude_event_types: vec![EventType::RuntimeFactRecorded],
                    instance_ids: vec![a, b, c],
                    from_timestamp_unix_ms: Some(since),
                    to_timestamp_unix_ms: Some(until),
                    ..EventQuery::default()
                },
            ),
        ]
    }

    /// Each per-run link as a query of its own: the last run's, and a causation, an action and a
    /// recognition that no fixture record carries (the test reads plans, not rows).
    fn per_run_links(&self) -> Vec<(&'static str, EventQuery)> {
        let run = &self.run;
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        vec![
            (
                "a run",
                EventQuery {
                    run_id: Some(run.run),
                    ..EventQuery::default()
                },
            ),
            (
                "a request and a task",
                EventQuery {
                    request_id: Some(run.request),
                    task_id: Some(run.task),
                    ..EventQuery::default()
                },
            ),
            (
                "a correlation",
                EventQuery {
                    correlation_id: Some(run.correlation),
                    ..EventQuery::default()
                },
            ),
            (
                "a lease",
                EventQuery {
                    lease_id: Some(run.lease),
                    ..EventQuery::default()
                },
            ),
            (
                "a frame",
                EventQuery {
                    frame_id: Some(self.frame),
                    ..EventQuery::default()
                },
            ),
            (
                "one instance's run",
                EventQuery {
                    instance_id: Some(self.instances[0]),
                    run_id: Some(run.run),
                    ..EventQuery::default()
                },
            ),
            (
                "a causation",
                EventQuery {
                    causation_id: Some(
                        *ids.mint_causation_id().expect("a causation id").transport(),
                    ),
                    ..EventQuery::default()
                },
            ),
            (
                "an action",
                EventQuery {
                    action_id: Some(*ids.mint_action_id().expect("an action id").transport()),
                    ..EventQuery::default()
                },
            ),
            (
                "a recognition",
                EventQuery {
                    recognition_id: Some(
                        *ids.mint_recognition_id()
                            .expect("a recognition id")
                            .transport(),
                    ),
                    ..EventQuery::default()
                },
            ),
        ]
    }
}

/// The segments of the pages over `(after, through)` the plans are taken for, each as
/// `(after, through)`: a page's first (one sequence), second (two, after the first) and widest
/// (`DEFAULT_HISTORY_SCAN_ROWS`), from the range's start and from its middle.
fn segments((after, through): (u64, u64)) -> Vec<(String, (u64, u64))> {
    let widest = u64::try_from(DEFAULT_HISTORY_SCAN_ROWS).expect("the widest segment");
    let middle = after + (through - after) / 2;
    let mut segments = Vec::new();
    for (at, start) in [("at the start", after), ("in the middle", middle)] {
        for (segment, from, span) in [
            ("first", start, 1),
            ("second", start + 1, SECOND_SEGMENT),
            ("widest", start, widest),
        ] {
            segments.push((
                format!("the {segment} segment {at}"),
                (from, (from + span).min(through)),
            ));
        }
    }
    segments
}

/// A formal root whose Runtime writer (with its reader pool) appended the fixture.
struct Fixture {
    _dir: TempDir,
    database: Arc<RuntimeDatabase>,
    ledger: GlobalLedger,
    values: Values,
}

impl Fixture {
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
        let ledger = open(dir.path(), &database);
        let first = ledger
            .latest_sequence()
            .expect("the head before the fixture")
            + 1;
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let issued = [
            ids.mint_instance_id().expect("an instance id"),
            ids.mint_instance_id().expect("an instance id"),
            ids.mint_instance_id().expect("an instance id"),
        ];
        let records = Records {
            ledger: &ledger,
            ids: &ids,
            fingerprinter: Sha256SecretFingerprinter::new(SALT).expect("fingerprinter"),
        };
        let lab = (EventSource::Lab, OriginModule::Actinglab, EventActor::Lab);
        let cli = (EventSource::Cli, OriginModule::Actingctl, EventActor::User);
        let runtime = (
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        );
        let mut anchor: Option<(IssuedRequestId, IssuedCorrelationId)> = None;
        let mut run = Run::mint(&ids, issued[0], None);
        let mut frame = None;
        for index in 0..RECORDS {
            let at = time(index);
            if index.is_multiple_of(RUN_RECORDS) {
                // Every third run names the latest Lab anchor's request and correlation, so its
                // records are Lab-related through their run.
                let named = anchor.filter(|_| (index / RUN_RECORDS).is_multiple_of(3));
                run = Run::mint(&ids, turn(&issued, index / RUN_RECORDS), named);
            }
            match index % KINDS {
                0 => {
                    let request = ids.mint_request_id().expect("a request id");
                    let correlation = ids.mint_correlation_id().expect("a correlation id");
                    anchor = Some((request, correlation));
                    records.append(
                        at,
                        EventSeverity::Info,
                        lab,
                        EventLinksDraft::default()
                            .with_instance_id(issued[0])
                            .with_request_id(request)
                            .with_correlation_id(correlation),
                        ClientPayloadDraft::lab_request(
                            EventAction::RuntimeTaskRun,
                            AuditInput::new(),
                        )
                        .into(),
                    );
                }
                // A debug record that names the anchor's request: Lab-related by it.
                1 => {
                    let (request, _) = anchor.expect("an anchor before");
                    records.append(
                        at,
                        EventSeverity::Debug,
                        cli,
                        EventLinksDraft::default()
                            .with_instance_id(issued[1])
                            .with_request_id(request),
                        CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new())
                            .into(),
                    );
                }
                2 => records.failed(at, EventSeverity::Warning, EventLinksDraft::default()),
                3 | 7 => {
                    let issued_frame = ids.mint_frame_id().expect("a frame id");
                    frame = Some(*issued_frame.transport());
                    records.append(
                        at,
                        EventSeverity::Info,
                        (
                            EventSource::Device,
                            OriginModule::Capture,
                            EventActor::Runtime,
                        ),
                        run.links.clone().with_frame_id(issued_frame),
                        CapturePayloadDraft::completed_with_capture_acquire(
                            EventAction::CaptureObserve,
                            EffectDisposition::NotPerformed,
                            FRAME_WIDTH,
                            FRAME_HEIGHT,
                            Some(18_000),
                            AuditInput::new(),
                        )
                        .into(),
                    );
                }
                4 => records.append(
                    at,
                    EventSeverity::Info,
                    (
                        EventSource::Scheduler,
                        OriginModule::Scheduler,
                        EventActor::Scheduler,
                    ),
                    run.links.clone(),
                    LeasePayloadDraft::granted(
                        EventAction::LeaseAcquire,
                        EffectDisposition::Performed,
                        AuditInput::new(),
                    )
                    .into(),
                ),
                5 => records.failed(
                    at,
                    EventSeverity::Error,
                    EventLinksDraft::default().with_instance_id(issued[1]),
                ),
                6 => records.append(
                    at,
                    EventSeverity::Info,
                    runtime,
                    EventLinksDraft::default(),
                    RuntimePayloadDraft::fact_recorded(
                        RuntimeFactRecord {
                            scope: RuntimeFactScope::Runtime,
                            key: "backend.fixture_key_qp1".to_owned(),
                            value: FactValue::Integer(
                                i64::try_from(index).expect("a fixture value"),
                            ),
                            observed_at_unix_ms: at,
                            source: OriginModule::Runtime,
                            ttl_ms: None,
                        },
                        AuditInput::new(),
                    )
                    .into(),
                ),
                8 => records.append(
                    at,
                    EventSeverity::Warning,
                    (
                        EventSource::Runtime,
                        OriginModule::PerformanceMonitor,
                        EventActor::Runtime,
                    ),
                    EventLinksDraft::default().with_instance_id(issued[2]),
                    PerformancePayloadDraft::monitor_degraded(
                        PerformanceMonitorStateEventData {
                            observed_at_unix_ms: at,
                            health: PerformanceMonitorHealth::Degraded,
                            outcome: Some(Outcome::relayed("step_budget_exhausted")),
                            consecutive_failures: 1,
                            terminal: false,
                            unavailable_metrics: vec![PerformanceMetric::DiskCapacity],
                        },
                        AuditInput::new(),
                    )
                    .into(),
                ),
                9 => records.append(
                    at,
                    EventSeverity::Info,
                    cli,
                    run.request_links.clone(),
                    CommandPayloadDraft::received(EventAction::RuntimeTaskRun, AuditInput::new())
                        .into(),
                ),
                10 => records.failed(at, EventSeverity::Fatal, EventLinksDraft::default()),
                _ => records.failed(
                    at,
                    EventSeverity::Info,
                    EventLinksDraft::default().with_instance_id(issued[2]),
                ),
            }
        }
        drop(records);
        let summary = ledger
            .confirm_deferred(CONFIRM_WAIT)
            .expect("the confirmation");
        assert!(
            summary.failed.is_empty() && summary.pending == 0,
            "every fixture record commits: {:?}, {} pending",
            summary.failed,
            summary.pending
        );
        let head = ledger.latest_sequence().expect("the head");
        assert!(
            head >= first + RECORDS - 1,
            "the fixture's records are committed: {first}..={head}"
        );
        let events = ledger
            .query(EventQuery::default())
            .expect("every record")
            .into_iter()
            .filter(|event| event.sequence() <= head)
            .collect::<Vec<_>>();
        assert_eq!(
            events.last().map(PersistedEvent::sequence),
            Some(head),
            "every record through the head"
        );
        Self {
            _dir: dir,
            database,
            ledger,
            values: Values {
                events,
                first,
                head,
                instances: issued.map(|instance| *instance.transport()),
                run,
                frame: frame.expect("a frame"),
            },
        }
    }
}

/// The Runtime's writer of the root at `dir`, with its reader pool; a view page's statements span
/// at most `SCAN_ROWS` sequences.
fn open(dir: &Path, database: &Arc<RuntimeDatabase>) -> GlobalLedger {
    let state_root = dir.canonicalize().expect("state root");
    let plan = LedgerFailurePlan::new(
        LedgerWritePolicy::new(1, vec![0], STALL_MS, PROBE_MS, DUMP_MAX_BYTES)
            .expect("write policy"),
        &state_root,
        None,
        epoch(),
        FIRST_MS,
    )
    .with_reply_timeout(REPLY_TIMEOUT)
    .with_history_scan_rows(SCAN_ROWS);
    let deadline = MaintenanceLimits {
        timeout_seconds: OPEN_TIMEOUT_SECONDS,
        ..MaintenanceLimits::default()
    }
    .deadline()
    .expect("deadline");
    LedgerMaintenance::acquire(dir, false, MaintenanceLimits::default(), deadline)
        .expect("maintenance owner")
        .open_writer_timed(
            Arc::clone(database),
            OWNER.into(),
            Some(plan),
            &mut LedgerOpenTiming::default(),
        )
        .expect("the writer opens")
}

/// The plan of `sql` with `values` bound, one line per step indented two spaces per level. With
/// its values bound, SQLite plans the statement as it plans the page's (with statistics, it
/// prepares it again for the values).
fn plan(connection: &Connection, sql: &str, values: &[SqlValue]) -> Vec<String> {
    let mut statement = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .expect("prepare the statement's plan");
    let steps = statement
        .query_map(params_from_iter(values.iter()), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(3)?,
            ))
        })
        .expect("the statement's plan")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("read the statement's plan");
    let mut depths = BTreeMap::new();
    steps
        .into_iter()
        .map(|(id, parent, detail)| {
            let depth = depths.get(&parent).map_or(0, |depth| depth + 1);
            depths.insert(id, depth);
            format!("{}{detail}", "  ".repeat(depth))
        })
        .collect()
}

/// The steps of `plan` that search or scan the events or links table outside a page statement's
/// segment. The first such step (the outer loop over them) must be a range that carries both of
/// the segment's bounds: by the row id (`rowid>? AND rowid<?`), or as an index's second term after
/// its column's (`col=? AND sequence>? AND sequence<?`, or a skip-scan's `ANY(col)` first); a
/// later one may also read the one row of a sequence the outer loop gave (`rowid=?`, or an index's
/// `col=? AND sequence=?`). A scan, an automatic index or any other step that names the events or
/// links table (a Bloom filter built over it, say) is outside; a step that names neither (a
/// subquery, a sort, `json_each`) is not judged. SQLite's plan writes `>` for `>=` and `<` for
/// `<=`.
fn outside_segment(plan: &[String]) -> Vec<String> {
    let mut outside = Vec::new();
    let mut searched = false;
    for step in plan {
        let detail = step.trim_start();
        let words = detail.split(' ').collect::<Vec<_>>();
        if !matches!(words.first(), Some(&("SEARCH" | "SCAN"))) {
            // Any other step that names the events or links table reads it otherwise than by a
            // search that carries the segment's bounds.
            if words.iter().any(|word| PAGE_TABLES.contains(word)) {
                outside.push(format!("other: {detail}"));
            }
            continue;
        }
        let table = match (words.get(1), words.get(2), words.get(3)) {
            (_, Some(&"AS"), Some(&alias)) => alias,
            (Some(&name), _, _) => name,
            (None, _, _) => "",
        };
        if !PAGE_TABLES.contains(&table) {
            continue;
        }
        let outer = !searched;
        searched = true;
        if !inside_segment(detail, outer) {
            let loop_kind = if outer { "outer" } else { "inner" };
            outside.push(format!("{loop_kind}: {detail}"));
        }
    }
    if !searched {
        outside.push("no search of the events or links table".to_owned());
    }
    outside
}

/// Whether one step of the events or links table stays inside the segment (`outside_segment`).
fn inside_segment(detail: &str, outer: bool) -> bool {
    let by_key = detail.contains(" USING INTEGER PRIMARY KEY (");
    let by_index = detail.contains(" USING INDEX ") || detail.contains(" USING COVERING INDEX ");
    if !detail.starts_with("SEARCH ") || !(by_key || by_index) {
        return false;
    }
    let Some((_, constraints)) = detail.split_once(" (") else {
        return false;
    };
    let terms = constraints
        .trim_end_matches(')')
        .split(" AND ")
        .collect::<Vec<_>>();
    // An index's first term is on its own column; the sequence follows.
    let sequence_terms = if by_key {
        &terms[..]
    } else {
        terms.get(1..).unwrap_or_default()
    };
    let is = |term: Option<&&str>, operator: &str| {
        term.is_some_and(|term| {
            ["sequence", "rowid"].iter().any(|column| {
                term.strip_prefix(column)
                    .and_then(|rest| rest.strip_suffix('?'))
                    == Some(operator)
            })
        })
    };
    let range = sequence_terms.len() == 2
        && is(sequence_terms.first(), ">")
        && is(sequence_terms.get(1), "<");
    let point = is(sequence_terms.first(), "=");
    range || (point && !outer)
}

/// How many times `term` stands in `sql` without a unary `+` before it.
fn indexable(sql: &str, term: &str) -> usize {
    sql.matches(term)
        .count()
        .saturating_sub(sql.matches(&format!("+{term}")).count())
}

/// Calls `check` with an empty database and with the fixture's (its writer closed), each without
/// statistics and after `ANALYZE` (the fixture's STAT4 statistics, asserted), and the Lab
/// relations of its head.
fn on_each_database(
    database: &RuntimeDatabase,
    head: u64,
    mut check: impl FnMut(&str, &Connection, &LabRelations),
) {
    let empty = empty_database();
    let no_relations = LabRelations::default();
    check("an empty database", &empty, &no_relations);
    empty
        .execute_batch("ANALYZE")
        .expect("the empty database's statistics");
    check("an empty database after ANALYZE", &empty, &no_relations);
    let filled = database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("the fixture's connection");
    let lab = read_lab_relations(&filled, head, &LabRelations::default())
        .expect("the fixture's Lab relations");
    assert!(
        !lab.requests.is_empty() && !lab.correlations.is_empty() && !lab.runs.is_empty(),
        "the fixture relates records to its Lab anchors: {lab:?}"
    );
    check("the fixture", &filled, &lab);
    filled
        .execute_batch("ANALYZE")
        .expect("the fixture's statistics");
    let statistics: i64 = filled
        .query_row("SELECT COUNT(*) FROM sqlite_stat1", [], |row| row.get(0))
        .expect("the fixture's statistics rows");
    assert!(statistics > 0, "ANALYZE wrote the fixture's statistics");
    // The statistics are STAT4's, as on the SQLite the Runtime builds: this build has
    // `ENABLE_STAT4`, and `ANALYZE` wrote the fixture's samples.
    let mut pragma = filled
        .prepare("PRAGMA compile_options")
        .expect("the compile options' statement");
    let options = pragma
        .query_map([], |row| row.get::<_, String>(0))
        .expect("the compile options")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("read the compile options");
    assert!(
        options.iter().any(|option| option == "ENABLE_STAT4"),
        "this SQLite build has no ENABLE_STAT4, so the plans after ANALYZE are not taken with \
         STAT4 statistics: {options:?}"
    );
    let samples: i64 = filled
        .query_row("SELECT COUNT(*) FROM sqlite_stat4", [], |row| row.get(0))
        .expect("the fixture's STAT4 samples");
    assert!(samples > 0, "ANALYZE wrote the fixture's STAT4 samples");
    check("the fixture after ANALYZE", &filled, &lab);
}

/// `query` with the window `range` (`from_sequence`, `to_sequence`) when it is given.
fn windowed(query: &EventQuery, view: Option<LedgerView>, range: Option<(u64, u64)>) -> EventQuery {
    EventQuery {
        view,
        from_sequence: range.map(|(from, _)| from),
        to_sequence: range.map(|(_, to)| to),
        ..query.clone()
    }
}

/// The statements a page of `query` over `range` builds for each of its `segments`, on `lab`'s
/// relations: each segment's name, the statement and its values.
fn statements(
    query: &EventQuery,
    lab: &LabRelations,
    range: (u64, u64),
) -> Vec<(String, String, Vec<SqlValue>)> {
    let condition = query
        .view
        .map(|view| ViewCondition::of(view, lab))
        .transpose()
        .expect("the view's condition");
    segments(range)
        .into_iter()
        .map(|(segment, span)| {
            let (sql, values) = history_statement(
                &HistorySelection::of(query),
                condition.as_ref(),
                span,
                STATEMENT_ROWS,
            )
            .expect("the page's statement");
            (segment, sql, values)
        })
        .collect()
}

#[test]
fn page_statements_stay_inside_sequence_bounds() {
    let Fixture {
        _dir,
        database,
        ledger,
        values,
    } = Fixture::new();
    ledger.close().expect("close the writer");
    // LN-8d's link-set page with a window: its window's sequence term is written with a `+`, and
    // the statement keeps the form of the link-set page (each dimension's sequences picked in its
    // own index, `linked_page_sequences`), not the join's. The resident retention's object load
    // (`load_objects`) builds it so, with `to_sequence`, for its artifacts and then its frames.
    let windowed_load = EventQuery {
        to_sequence: Some(values.head),
        ..EventQuery::default()
    };
    let artifact = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_artifact_id()
        .expect("an artifact id")
        .transport();
    for (dimension, set) in [
        (
            "artifacts",
            LinkSet {
                artifacts: [artifact].into_iter().collect(),
                ..LinkSet::default()
            },
        ),
        (
            "frames",
            LinkSet {
                frames: [values.frame].into_iter().collect(),
                ..LinkSet::default()
            },
        ),
    ] {
        let (sql, _) = history_statement(
            &HistorySelection::linked(&windowed_load, &set),
            None,
            (0, values.head),
            STATEMENT_ROWS,
        )
        .expect("the link set's statement");
        assert!(
            sql.contains("+e.sequence<=?") && sql.contains("e.sequence IN (SELECT sequence FROM ("),
            "{dimension}: a link-set statement with `to_sequence` has the link-set page's \
             form\n{sql}"
        );
    }
    let whole = (values.first, values.head);
    let quarter = (values.head - values.first) / 4;
    let window = (values.first + quarter, values.head - quarter);
    let mut outside = Vec::new();
    let mut taken = 0_usize;
    on_each_database(&database, values.head, |phase, connection, lab| {
        for (filter, query) in values.page_filters(whole) {
            for view in views() {
                for (windowing, range) in [("no window", None), ("a window", Some(window))] {
                    let query = windowed(&query, view, range);
                    let span = range.map_or((0, values.head), |(from, to)| (from - 1, to));
                    for (segment, sql, bound) in statements(&query, lab, span) {
                        let case = format!("{phase}, {filter}, {view:?}, {windowing}, {segment}");
                        // The segment's pair is the statement's only indexable sequence range,
                        // and no time term is indexable.
                        if (
                            indexable(&sql, "e.sequence>?"),
                            indexable(&sql, "e.sequence>=?"),
                            indexable(&sql, "e.sequence<=?"),
                            indexable(&sql, "e.timestamp_unix_ms>=?"),
                            indexable(&sql, "e.timestamp_unix_ms<?"),
                        ) != (1, 0, 1, 0, 0)
                        {
                            outside.push(format!(
                                "{case}: an indexable term besides the segment's\n{sql}"
                            ));
                        }
                        let steps = plan(connection, &sql, &bound);
                        taken += 1;
                        let found = outside_segment(&steps);
                        if !found.is_empty() {
                            outside.push(format!(
                                "{case}: {}\n  plan: {}\n  {sql}",
                                found.join("; "),
                                steps.join(" | ")
                            ));
                        }
                    }
                }
            }
        }
    });
    // Four databases, 15 filters, seven views, two windows and six segments.
    assert_eq!(taken, 4 * 15 * 7 * 2 * 6, "the plans taken");
    assert!(
        outside.is_empty(),
        "{} statements leave their segment:\n{}",
        outside.len(),
        outside
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn time_bound_never_drives_a_page() {
    let Fixture {
        _dir,
        database,
        ledger,
        values,
    } = Fixture::new();
    ledger.close().expect("close the writer");
    // Time bounds that select few records: past the head's time, before the second record's,
    // and one interval in the middle; and a window of the middle half.
    let late = values.time_at(values.head - 1);
    let early = values.time_at(values.first + 1) + 1;
    let middle = values.time_at(values.first + (values.head - values.first) / 2 + 1);
    let bounds = [
        ("a lower time bound", Some(late), None),
        ("an upper time bound", None, Some(early)),
        (
            "time bounds",
            Some(middle),
            Some(middle + RECORD_INTERVAL_MS),
        ),
    ];
    let mut links = vec![("no per-run link", EventQuery::default())];
    links.extend(values.per_run_links());
    let quarter = (values.head - values.first) / 4;
    let window = (values.first + quarter, values.head - quarter);
    let mut driven = Vec::new();
    let mut taken = 0_usize;
    on_each_database(&database, values.head, |phase, connection, lab| {
        for (link, linked) in &links {
            for (bound, since, until) in bounds {
                let query = EventQuery {
                    from_timestamp_unix_ms: since,
                    to_timestamp_unix_ms: until,
                    ..linked.clone()
                };
                for view in views() {
                    for (windowing, range) in [("no window", None), ("a window", Some(window))] {
                        let query = windowed(&query, view, range);
                        let span = range.map_or((0, values.head), |(from, to)| (from - 1, to));
                        for (segment, sql, bound_values) in statements(&query, lab, span) {
                            let case = format!(
                                "{phase}, {link}, {bound}, {view:?}, {windowing}, {segment}"
                            );
                            if indexable(&sql, "e.timestamp_unix_ms>=?") != 0
                                || indexable(&sql, "e.timestamp_unix_ms<?") != 0
                            {
                                driven.push(format!("{case}: a time term without `+`\n{sql}"));
                            }
                            let steps = plan(connection, &sql, &bound_values);
                            taken += 1;
                            if steps.iter().any(|step| step.contains(TIME_INDEX)) {
                                driven.push(format!("{case}: {}\n  {sql}", steps.join(" | ")));
                            }
                        }
                    }
                }
            }
        }
    });
    // Four databases, no per-run link and nine, three bounds, seven views, two windows and six
    // segments.
    assert_eq!(taken, 4 * 10 * 3 * 7 * 2 * 6, "the plans taken");
    assert!(
        driven.is_empty(),
        "{} statements a time bound may drive:\n{}",
        driven.len(),
        driven
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// What a page answers that the plans must not change: its rows, whether it goes on, its cursor
/// and its read scope.
#[derive(Debug, PartialEq)]
struct Answer {
    rows: Vec<(u64, EventId)>,
    has_more: bool,
    cursor: Option<(u64, u64)>,
    scope: Option<LedgerReadScope>,
}

impl Answer {
    fn of(page: &RuntimeEventQueryPage) -> Self {
        Self {
            rows: page
                .events()
                .iter()
                .map(|row| (row.sequence, row.event_id))
                .collect(),
            has_more: page.has_more(),
            cursor: page
                .next_cursor()
                .map(|cursor| (cursor.snapshot_ledger_position(), cursor.after_sequence())),
            scope: page.read_scope().cloned(),
        }
    }
}

#[test]
fn page_results_unchanged_by_plan_bounds() {
    let Fixture {
        _dir,
        database: _,
        ledger,
        values,
    } = Fixture::new();
    let indexes = EventIndexes::from_events(&values.events);
    let head = values.head;
    let middle = values.first + (head - values.first) / 2 - WINDOW / 2;
    let windows = [
        ("the start", (values.first, values.first + WINDOW - 1)),
        ("the middle", (middle, middle + WINDOW - 1)),
        ("the end", (head - WINDOW + 1, head)),
    ];
    let mut differences = Vec::new();
    let mut pages = 0_usize;
    for (window, range) in windows {
        for (filter, query) in values.page_filters(range) {
            for view in views() {
                let query = windowed(&query, view, Some(range));
                let case = format!("{window}, {filter}, {view:?}");
                let mut cursor = None;
                let mut rows = 0;
                let mut ended = false;
                for _ in 0..MAX_PAGES {
                    let request = match cursor.take() {
                        None => RuntimeEventQueryPageRequest::new(PAGE_LIMIT, None)
                            .and_then(|request| request.at_snapshot(head)),
                        Some(cursor) => RuntimeEventQueryPageRequest::new(PAGE_LIMIT, Some(cursor)),
                    }
                    .expect("a page request");
                    let page = ledger
                        .project_view_page(query.clone(), PROFILE, request.clone())
                        .unwrap_or_else(|error| panic!("{case}: the page: {error:?}"));
                    let unbounded = indexes
                        .project_view_page(
                            &values.events,
                            &query,
                            PROFILE,
                            &request,
                            LedgerReadScope {
                                source: page
                                    .read_scope()
                                    .map_or(LedgerReadSource::Runtime, |scope| scope.source),
                                material_read: LedgerMaterialReadState::NotRequested,
                                scanned_through_position: head,
                                read_complete: true,
                                limits: Vec::new(),
                            },
                            PageSelection::from(head),
                        )
                        .unwrap_or_else(|error| panic!("{case}: the in-memory page: {error:?}"));
                    pages += 1;
                    let (answer, expected) = (Answer::of(&page), Answer::of(&unbounded));
                    rows += answer.rows.len();
                    if answer != expected {
                        differences.push(format!(
                            "{case}, page {pages}: {answer:?}\n  in memory: {expected:?}"
                        ));
                        ended = true;
                        break;
                    }
                    if !page.has_more() {
                        ended = true;
                        break;
                    }
                    cursor = page.next_cursor().cloned();
                }
                assert!(ended, "{case}: the pages did not end within {MAX_PAGES}");
                // The cases are what they claim to be: each filter selects records in each
                // window.
                assert!(
                    view.is_some() || rows > 0,
                    "{case}: the window holds records the filter selects"
                );
            }
        }
    }
    ledger.close().expect("close the writer");
    assert!(pages > 0, "pages were read");
    assert!(
        differences.is_empty(),
        "{} pages differ from the in-memory pages:\n{}",
        differences.len(),
        differences
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
