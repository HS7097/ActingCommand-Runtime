// SPDX-License-Identifier: AGPL-3.0-only

//! #422 IDLE-OV (v0.12.19; brief rd-1027/brief-idle-overview.md, investigation
//! rd-1027/q-idle-overview-budget.md). On production v0.12.18, 10-27 18:55,
//! `ac_overview(instance <an instance paused for days>, sections instances)` was refused with
//! `ledger_read_budget_exceeded`, and the CLI's same read took 19.9 s where an instance that runs
//! every 30 min took 5.3 s: the run lookup walked the whole last 24 h of the ledger back from its
//! tail, window by window, dozens of pages, each a trial against the read budget. The required
//! states, the client's part:
//! - I-1: an instance with no run in a long, busy window answers `run_none_in_window`, complete,
//!   within one page per listing fact type, however many records the window holds;
//! - I-2: an instance with a recent run answers it;
//! - I-6: a read that cannot finish keeps `run_incomplete` with its warning, never "no run": a
//!   listing page the Runtime did not read (its budget stopped it; the host answers `failed`,
//!   `ledger_read_budget_exceeded`, `runtime_unavailable`), and a read the call budget left no
//!   time to start;
//! - I-7 (review H1, fix round 2): a busy instance, whose window holds many entry facts (a busy
//!   production day passed 16 384 on one instance), answers its newest run, one still in its
//!   entry phase, within a bound of pages that does not grow with the window's entry facts.
//!
//! The Runtime is A42's fake (`gate_411_a42`): one in-memory ledger whose first page with a lower
//! time bound starts at the window's first record (LN-WS R3), which counts the QueryEvents pages
//! it is asked for.

use super::gate_411_a42::{FakeRuntime, Recorder};
use super::gate_mcpt1::unreached_deadline;
use crate::overview::{self, RunWindow, Warning};
use actingcommand_contract::{
    AuditInput, ClientPayloadDraft, EventAction, EventActor, EventLinksDraft, EventOrigin,
    EventSource, EventType, IssuedInstanceId, MAX_RUNTIME_EVENT_QUERY_EVENTS, OriginModule,
    ProjectedEvent, RuntimeErrorCode, RuntimeInstanceStatus,
};
use actingcommand_runtime_client::{RuntimeChannel, RuntimeClient, RuntimeClientConfig};
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// The run window (24 h), after a history of the instance's own runs.
const WINDOW_START_UNIX_MS: u64 = 4_102_444_800_000 + 1_000_000_000;
const WINDOW_END_UNIX_MS: u64 = WINDOW_START_UNIX_MS + 24 * 60 * 60 * 1000;
const HISTORY_RUNS: usize = 500;
/// I-1: the window's records, the instance's own Lab activity: a short window and one four
/// times as long. The tail walk read the long one in three windows of ledger sequences (4 096,
/// 8 192 and 16 384) and the short one in two, each window five listings and a probe.
const SHORT_WINDOW_RECORDS: usize = 6_000;
const LONG_WINDOW_RECORDS: usize = 24_000;
/// The listing fact types a run is listed by (runtime-client `RUN_LISTING_EVENT_TYPES`: main
/// admission, entry fact, and the three terminals): an instance without a run in the window
/// costs one page of each.
const LISTING_TYPES: u64 = 5;
/// In the window: another instance's runs, one every this many of the instance's records.
const OTHER_RUN_EVERY: usize = 100;
/// I-7: the busy instance's finished runs before the window and in it, each with this many
/// entry facts: a few, and twenty times as many, when the window's entry facts fill fifteen
/// pages of `MAX_RUNTIME_EVENT_QUERY_EVENTS` (256).
const BUSY_RUNS: usize = 48;
const FEW_ENTRY_FACTS: usize = 4;
const MANY_ENTRY_FACTS: usize = 80;
/// I-7: the busy instance's newest run, still in its entry phase, and its entry facts so far.
const NEWEST_ENTRY_FACTS: usize = 3;
/// I-7: the newest run's own status read (runtime-client `read_contained_run_status`): its facts
/// by request, one page, and the two Runtime restart checks, one page each.
const RUN_STATUS_PAGES: u64 = 3;

/// A ledger of `instance`'s `HISTORY_RUNS` runs before the window, then `window_records` Lab
/// requests of `instance` in the window with another instance's runs among them, and, when
/// `recent` is set, `instance`'s own runs in the window, the newest near its end: the instance's
/// id, the ledger and the request_id of the instance's newest run in the window.
fn ledger(
    window_records: usize,
    recent: bool,
) -> (IssuedInstanceId, Vec<ProjectedEvent>, Option<Value>) {
    let mut ledger = Recorder::new(WINDOW_START_UNIX_MS - 2 * (HISTORY_RUNS as u64) * 1_000);
    let instance = ledger.issuer.mint_instance_id().expect("instance id");
    let other = ledger.issuer.mint_instance_id().expect("instance id");
    for _ in 0..HISTORY_RUNS {
        ledger.listed_run(instance);
    }
    ledger.at(WINDOW_START_UNIX_MS);
    let mut newest = None;
    for index in 0..window_records {
        if index % OTHER_RUN_EVERY == 0 {
            ledger.listed_run(other);
        }
        // The instance's own runs: one early in the window and one near its end.
        if recent && (index == window_records / 10 || index == window_records - 10) {
            newest = Some(ledger.listed_run(instance));
        }
        let links = EventLinksDraft::default()
            .with_instance_id(instance)
            .with_request_id(ledger.issuer.mint_request_id().expect("request id"))
            .with_correlation_id(ledger.issuer.mint_correlation_id().expect("correlation id"));
        ledger.push(
            EventOrigin::new(EventSource::Lab, OriginModule::Actinglab, EventActor::Lab),
            links,
            ClientPayloadDraft::lab_request(EventAction::RuntimeTaskRun, AuditInput::new()).into(),
        );
    }
    (instance, ledger.events, newest)
}

/// I-7: a ledger of the busy instance's `BUSY_RUNS` runs before the window and `BUSY_RUNS` in
/// it, each with `entry_facts` entry facts, its main admission and its terminal, another
/// instance's runs among them, and then its newest run, still in its entry phase: the instance's
/// id, the ledger and the newest run's request_id.
fn busy_ledger(entry_facts: usize) -> (IssuedInstanceId, Vec<ProjectedEvent>, Value) {
    let mut ledger = Recorder::new(WINDOW_START_UNIX_MS - 24 * 60 * 60 * 1000);
    let instance = ledger.issuer.mint_instance_id().expect("instance id");
    let other = ledger.issuer.mint_instance_id().expect("instance id");
    for _ in 0..BUSY_RUNS {
        ledger.entry_run(instance, entry_facts, true);
    }
    ledger.at(WINDOW_START_UNIX_MS);
    for index in 0..BUSY_RUNS {
        if index % 4 == 0 {
            ledger.listed_run(other);
        }
        ledger.entry_run(instance, entry_facts, true);
    }
    let newest = ledger.entry_run(instance, NEWEST_ENTRY_FACTS, false);
    (instance, ledger.events, newest)
}

/// A CLI client of the fake Runtime under `root`, connected as `gate_411_a42` connects its own.
fn connect_client(root: &Path) -> RuntimeClient {
    RuntimeClient::connect(RuntimeClientConfig::new(
        root,
        RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect to the fake Runtime")
}

/// A call budget already spent: the hour `unreached_deadline` grants, taken back, so the
/// deadline is the instant the budget was granted and no read can start before it. It measures
/// nothing; whatever time passes after it, the budget stays spent.
fn spent_deadline() -> Instant {
    unreached_deadline()
        .checked_sub(Duration::from_secs(60 * 60))
        .expect("the deadline unreached_deadline grants, less its hour")
}

/// One instance row's run members as the overview reads them until `deadline`: the row, its
/// warnings and whether the overview is incomplete.
fn run_row(
    client: &RuntimeClient,
    instance: IssuedInstanceId,
    deadline: Instant,
) -> (Value, Vec<Warning>, bool) {
    let status = RuntimeInstanceStatus::new(
        "neutral.instance",
        *instance.transport(),
        false,
        0,
        false,
        false,
        false,
    )
    .expect("instance status");
    let mut row = json!({});
    let (mut warnings, mut incomplete) = (Vec::new(), false);
    overview::run_block(
        client,
        &status,
        RunWindow {
            since_unix_ms: WINDOW_START_UNIX_MS,
            until_unix_ms: WINDOW_END_UNIX_MS,
        },
        deadline,
        &mut row,
        &mut warnings,
        &mut incomplete,
    );
    (row, warnings, incomplete)
}

fn warning_codes(warnings: &[Warning]) -> Vec<String> {
    warnings
        .iter()
        .map(|warning| match warning {
            Warning::Outcome(outcome) => outcome.code().as_str().to_owned(),
            Warning::Runtime(error) => error.outcome().code().as_str().to_owned(),
        })
        .collect()
}

#[test]
fn idle_ov_i1_an_instance_without_a_run_in_a_long_window_answers_none_within_one_page_per_type() {
    let mut pages = Vec::new();
    for window_records in [SHORT_WINDOW_RECORDS, LONG_WINDOW_RECORDS] {
        let (instance, events, _) = ledger(window_records, false);
        let root = TempDir::new().expect("tempdir");
        let runtime = FakeRuntime::start(root.path(), events);
        let client = connect_client(root.path());
        let before = runtime.stats().pages;
        let (row, warnings, incomplete) = run_row(&client, instance, unreached_deadline());
        let read = runtime.stats().pages - before;
        // "None in window", typed, with the window's bounds; the read finished.
        assert_eq!(
            row,
            json!({"run_none_in_window": {
                "since_unix_ms": WINDOW_START_UNIX_MS,
                "until_unix_ms": WINDOW_END_UNIX_MS,
            }}),
            "a window of {window_records} records"
        );
        assert!(
            !incomplete && warnings.is_empty(),
            "a window of {window_records} records: incomplete {incomplete}, warnings {:?}",
            warning_codes(&warnings)
        );
        // The bound: one page per listing fact type.
        assert!(
            read <= LISTING_TYPES,
            "a window of {window_records} records read {read} pages, more than {LISTING_TYPES}"
        );
        drop(client);
        pages.push(read);
        drop(runtime);
    }
    // However busy the window was: four times its records cost not one page more.
    assert_eq!(
        pages[1], pages[0],
        "pages read for a window of {LONG_WINDOW_RECORDS} records and of {SHORT_WINDOW_RECORDS}"
    );
}

#[test]
fn idle_ov_i2_an_instance_with_a_recent_run_answers_it() {
    let (instance, events, newest) = ledger(SHORT_WINDOW_RECORDS, true);
    let newest = newest.expect("the instance's newest run in the window");
    let root = TempDir::new().expect("tempdir");
    let runtime = FakeRuntime::start(root.path(), events);
    let client = connect_client(root.path());
    let (row, warnings, incomplete) = run_row(&client, instance, unreached_deadline());
    assert_eq!(row["run"]["request_id"], newest, "{row}");
    assert!(
        row.get("run_none_in_window").is_none() && row.get("run_incomplete").is_none(),
        "{row}"
    );
    assert!(
        !incomplete && warnings.is_empty(),
        "incomplete {incomplete}, warnings {:?}",
        warning_codes(&warnings)
    );
    drop(client);
    drop(runtime);
}

#[test]
fn idle_ov_i6_a_read_that_cannot_finish_keeps_run_incomplete_and_its_warning() {
    // A listing page the Runtime did not read: the row is incomplete, with the Runtime's
    // failure as its warning, and never says "no run".
    let (instance, events, _) = ledger(SHORT_WINDOW_RECORDS, false);
    let root = TempDir::new().expect("tempdir");
    let runtime = FakeRuntime::start_refusing(root.path(), events, Some(EventType::TaskRequested));
    let client = connect_client(root.path());
    let (row, warnings, incomplete) = run_row(&client, instance, unreached_deadline());
    assert_eq!(row, json!({"run_incomplete": true}), "{row}");
    assert!(incomplete, "the overview is incomplete");
    assert_eq!(
        warning_codes(&warnings),
        vec!["ledger_read_budget_exceeded".to_owned()],
        "the read's warning"
    );
    match warnings.first() {
        Some(Warning::Runtime(error)) => assert_eq!(
            error.projection().map(|projection| projection.code),
            Some(RuntimeErrorCode::RuntimeUnavailable),
            "{error:?}"
        ),
        _ => panic!("a Runtime warning"),
    }
    assert!(runtime.stats().pages >= 1, "the listing was asked for");
    drop(client);
    drop(runtime);

    // A read the call budget left no time to start: incomplete, `call_budget_exhausted`.
    let (instance, events, _) = ledger(SHORT_WINDOW_RECORDS, false);
    let root = TempDir::new().expect("tempdir");
    let runtime = FakeRuntime::start(root.path(), events);
    let client = connect_client(root.path());
    let (row, warnings, incomplete) = run_row(&client, instance, spent_deadline());
    assert_eq!(row, json!({"run_incomplete": true}), "{row}");
    assert!(incomplete, "the overview is incomplete");
    assert_eq!(
        warning_codes(&warnings),
        vec!["call_budget_exhausted".to_owned()],
        "the read's warning"
    );
    drop(client);
    drop(runtime);
}

#[test]
fn idle_ov_i7_a_busy_instance_answers_its_newest_run_within_a_bound_its_entry_facts_do_not_grow() {
    // The many-entry window: a listing of all its entry facts would take this many pages.
    let many_entry_pages =
        (BUSY_RUNS * MANY_ENTRY_FACTS).div_ceil(usize::from(MAX_RUNTIME_EVENT_QUERY_EVENTS));
    let bound = LISTING_TYPES + RUN_STATUS_PAGES;
    assert!(
        u64::try_from(many_entry_pages).expect("pages") > bound,
        "the window's entry facts fill {many_entry_pages} pages, more than the bound {bound}"
    );
    let mut pages = Vec::new();
    for entry_facts in [FEW_ENTRY_FACTS, MANY_ENTRY_FACTS] {
        let (instance, events, newest) = busy_ledger(entry_facts);
        let root = TempDir::new().expect("tempdir");
        let runtime = FakeRuntime::start(root.path(), events);
        let client = connect_client(root.path());
        let before = runtime.stats().pages;
        let (row, warnings, incomplete) = run_row(&client, instance, unreached_deadline());
        let read = runtime.stats().pages - before;
        // The newest run, the one still in its entry phase, read whole.
        assert_eq!(
            row["run"]["request_id"], newest,
            "runs with {entry_facts} entry facts: {row}"
        );
        assert!(
            row.get("run_none_in_window").is_none() && row.get("run_incomplete").is_none(),
            "runs with {entry_facts} entry facts: {row}"
        );
        assert!(
            !incomplete && warnings.is_empty(),
            "runs with {entry_facts} entry facts: incomplete {incomplete}, warnings {:?}",
            warning_codes(&warnings)
        );
        // The bound: one page per listing type and the newest run's own status read.
        assert!(
            read <= bound,
            "runs with {entry_facts} entry facts read {read} pages, more than {bound}"
        );
        drop(client);
        pages.push(read);
        drop(runtime);
    }
    // Twenty times the entry facts cost not one page more.
    assert_eq!(
        pages[1], pages[0],
        "pages read with {MANY_ENTRY_FACTS} entry facts per run and with {FEW_ENTRY_FACTS}"
    );
}
