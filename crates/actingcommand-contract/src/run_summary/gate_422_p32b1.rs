// SPDX-License-Identifier: AGPL-3.0-only

//! P32 B-1 (#422 6473873443; design rd-1028/design-p32.md §2 B, §3 B-1; since 0.12.27). The
//! required state:
//! - the run fold is commutative: the same facts in every order of a small run (all 40 320
//!   orders of eight facts) and in 256 fixed-seed shuffles of a larger one, each fact also fed
//!   twice, leave one fold, field by field, and one brief status;
//! - two terminals stay an anomaly in every order, and of several anomalies the fold reports the
//!   one a read in sequence order meets first;
//! - a run's listing facts give one order, `opened` and `settled` in every order;
//! - `QueryRuns` requests and pages round-trip, and invalid ones are refused by the member with
//!   `invalid_query_runs`.
//!
//! Nothing here is random at run time: the orders are enumerated (Heap's algorithm) or drawn
//! from a fixed-seed xorshift.

use super::*;
use crate::codes::RuntimeContractCode;
use crate::outcome::Outcome;
use crate::{
    QueryRunsCoverage, QueryRunsCoverageState, QueryRunsCursor, QueryRunsPage, QueryRunsRequest,
    QueryRunsRow, RuntimeContractError, RuntimeErrorCode,
};
use serde_json::json;

const PACKAGE: &str = "neutral.package";
const OUTER: &str = "neutral.recovery.outer";
const INNER: &str = "neutral.recovery.inner";

fn id<T: serde::de::DeserializeOwned>(prefix: &str, n: u64) -> T {
    serde_json::from_value(json!(format!("{prefix}{n:032x}"))).expect("typed id")
}

fn links(run: u64, lease: bool, request_only: bool) -> EventLinks {
    let mut value = json!({ "request_id": format!("request_{run:032x}") });
    if !request_only {
        value["instance_id"] = json!(format!("instance_{:032x}", 7));
        value["correlation_id"] = json!(format!("correlation_{run:032x}"));
        value["task_id"] = json!(format!("task_{run:032x}"));
        value["run_id"] = json!(format!("run_{run:032x}"));
    }
    if lease {
        value["lease_id"] = json!(format!("lease_{run:032x}"));
    }
    serde_json::from_value(value).expect("event links")
}

/// One recorded fact of a test run, owned.
#[derive(Clone)]
struct Recorded {
    sequence: u64,
    event_type: EventType,
    source: EventSource,
    links: EventLinks,
    semantic: Option<TaskSemanticFact>,
}

impl Recorded {
    fn fact(&self) -> RunFact<'_> {
        RunFact {
            sequence: self.sequence,
            event_id: id("evt_", self.sequence),
            timestamp_unix_ms: 1_800_000_000_000 + self.sequence,
            event_type: self.event_type,
            source: self.source,
            links: &self.links,
            semantic: self.semantic.as_ref(),
        }
    }
}

fn origin(sequence: u64, run: u64, event_type: EventType, source: EventSource) -> Recorded {
    Recorded {
        sequence,
        event_type,
        source,
        links: links(run, false, true),
        semantic: None,
    }
}

fn semantic(sequence: u64, run: u64, event_type: EventType, fact: TaskSemanticFact) -> Recorded {
    Recorded {
        sequence,
        event_type,
        source: EventSource::Runtime,
        links: links(run, true, false),
        semantic: Some(fact),
    }
}

fn admitted(sequence: u64, run: u64) -> Recorded {
    semantic(
        sequence,
        run,
        EventType::TaskRequested,
        TaskSemanticFact::PackageAdmitted {
            package_label: PACKAGE.to_owned(),
            task_label: "neutral.task".to_owned(),
            response_deadline_monotonic_ms: None,
        },
    )
}

fn entry(sequence: u64, run: u64) -> Recorded {
    semantic(
        sequence,
        run,
        EventType::TaskEntryPreflight,
        TaskSemanticFact::EntryRecoveryDecision { required: false },
    )
}

fn recovery(sequence: u64, run: u64, label: &str) -> Recorded {
    semantic(
        sequence,
        run,
        EventType::TaskEntryPreflight,
        TaskSemanticFact::EntryRecoveryPackageAdmitted {
            package_label: label.to_owned(),
            role: None,
            package_version: None,
        },
    )
}

fn step_started(sequence: u64, run: u64, step_index: u32) -> Recorded {
    semantic(
        sequence,
        run,
        EventType::TaskStepStarted,
        TaskSemanticFact::StepStarted {
            step_index,
            operation_label: format!("neutral.step.{step_index}"),
            from_page: "neutral/home".to_owned(),
            phase: None,
        },
    )
}

fn step_finished(sequence: u64, run: u64, step_index: u32) -> Recorded {
    semantic(
        sequence,
        run,
        EventType::TaskStepFinished,
        TaskSemanticFact::StepFinished {
            step_index,
            operation_label: format!("neutral.step.{step_index}"),
            page_label: format!("neutral/page.{step_index}"),
            phase: None,
        },
    )
}

fn terminal(sequence: u64, run: u64, task_result: TaskOutcome) -> Recorded {
    let (event_type, outcome) = match task_result {
        TaskOutcome::Success => (EventType::TaskCompleted, None),
        TaskOutcome::Failure => (
            EventType::TaskFailed,
            Some(Outcome::new(
                RuntimeErrorCode::ContainedTaskDeadlineExceeded,
            )),
        ),
        TaskOutcome::Cancelled => (
            EventType::TaskCancelled,
            Some(Outcome::new(RuntimeErrorCode::ContainedTaskCancelled)),
        ),
    };
    semantic(
        sequence,
        run,
        event_type,
        TaskSemanticFact::TerminalCommitted {
            task_result,
            final_page: Some("neutral/done".to_owned()),
            executed_steps: Some(1),
            outcome,
            failure_severe: false,
            scheduling_disposition: None,
            task_timing: None,
        },
    )
}

fn released(sequence: u64, run: u64) -> Recorded {
    Recorded {
        sequence,
        event_type: EventType::LeaseReleased,
        source: EventSource::Runtime,
        links: links(run, true, true),
        semantic: None,
    }
}

/// A succeeded manual run of eight facts: its CLI command, admission, an entry fact, two
/// recovery packages, a step started and finished, its terminal and its lease's release.
fn small_run() -> Vec<Recorded> {
    vec![
        origin(2, 1, EventType::CliCommand, EventSource::Cli),
        admitted(3, 1),
        recovery(5, 1, OUTER),
        recovery(6, 1, INNER),
        step_started(7, 1, 0),
        step_finished(8, 1, 0),
        terminal(9, 1, TaskOutcome::Success),
        released(10, 1),
    ]
}

/// A scheduled run of 64 facts: its scheduler receipt, admission, 30 entry facts, three
/// recovery packages, 12 steps started and finished, its cancelled terminal and its release.
fn larger_run() -> Vec<Recorded> {
    let mut facts = vec![
        origin(10, 2, EventType::CommandReceived, EventSource::Scheduler),
        admitted(11, 2),
    ];
    let mut sequence = 12;
    for _ in 0..30 {
        facts.push(entry(sequence, 2));
        sequence += 1;
    }
    for label in [OUTER, INNER, "neutral.recovery.last"] {
        facts.push(recovery(sequence, 2, label));
        sequence += 1;
    }
    for step in 0..12 {
        facts.push(step_started(sequence, 2, step));
        facts.push(step_finished(sequence + 1, 2, step));
        sequence += 2;
    }
    facts.push(terminal(sequence, 2, TaskOutcome::Cancelled));
    facts.push(released(sequence + 1, 2));
    facts
}

fn fold_in(order: &[&Recorded]) -> RunFold {
    let mut fold = RunFold::new();
    for recorded in order {
        fold.fold(recorded.fact());
    }
    fold
}

fn reads(fold: &RunFold) -> RunReads {
    RunReads {
        restarted_after_first_fact: false,
        restarted_after_admission: false,
        lease_terminal: fold
            .lease_terminals()
            .terminal()
            .expect("one lease terminal"),
    }
}

/// Every order of `facts` (Heap's algorithm, iterative), each handed to `visit`.
fn every_order<'a>(facts: &'a [Recorded], mut visit: impl FnMut(&[&'a Recorded])) {
    let mut order = facts.iter().collect::<Vec<_>>();
    let mut counters = vec![0_usize; order.len()];
    visit(&order);
    let mut index = 1;
    while index < order.len() {
        if counters[index] < index {
            let swap = if index % 2 == 0 { 0 } else { counters[index] };
            order.swap(swap, index);
            visit(&order);
            counters[index] += 1;
            index = 1;
        } else {
            counters[index] = 0;
            index += 1;
        }
    }
}

/// A fixed-seed shuffle (xorshift64), the same on every run.
fn shuffled<'a>(facts: &'a [Recorded], seed: u64) -> Vec<&'a Recorded> {
    let mut state = seed.max(1);
    let mut order = facts.iter().collect::<Vec<_>>();
    for index in (1..order.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let bound = u64::try_from(index).expect("index") + 1;
        let pick = usize::try_from(state % bound).expect("index");
        order.swap(index, pick);
    }
    order
}

#[test]
fn p32_b1_the_fold_of_a_small_run_is_one_fold_and_one_brief_status_in_every_order() {
    let facts = small_run();
    let in_order = fold_in(&facts.iter().collect::<Vec<_>>());
    let request_id = id::<RequestId>("request_", 1);
    let brief = in_order
        .brief(request_id, 10, reads(&in_order))
        .expect("one run's facts");
    // The required status (#338 R1 as runtime-client's run status read answers it).
    assert_eq!(brief.state, ContainedRunState::Succeeded);
    assert_eq!(
        (brief.dispatch, brief.origin),
        (RunDispatch::Manual, RunOrigin::Cli)
    );
    assert_eq!(brief.package_label.as_deref(), Some(PACKAGE));
    assert_eq!(brief.recovery_package_labels, vec![OUTER, INNER]);
    assert_eq!(brief.terminal.as_ref().map(|found| found.sequence), Some(9));
    assert_eq!(
        brief.lease.and_then(|lease| lease.terminal),
        Some(ContainedTaskLeaseTerminal::Released)
    );
    assert_eq!(brief.evidence.admitted_sequence, Some(3));
    assert_eq!(brief.run_id, Some(id("run_", 1)));
    assert_eq!(in_order.first_fact(), Some((3, 1_800_000_000_003)));
    assert_eq!(
        in_order.progress().map(|found| found.page.as_str()),
        Some("neutral/page.0")
    );
    let listing = in_order.listing().expect("listing facts");
    assert_eq!(
        (
            listing.order_sequence(),
            listing.opened(),
            listing.settled()
        ),
        (3, true, Some(ContainedRunState::Succeeded))
    );

    let mut orders = 0_u32;
    every_order(&facts, |order| {
        orders += 1;
        let fold = fold_in(order);
        assert_eq!(fold, in_order, "order {orders}");
        assert_eq!(fold.brief(request_id, 10, reads(&fold)), Ok(brief.clone()));
        // The same facts again, in the same order, change nothing.
        let mut twice = fold.clone();
        for recorded in order {
            twice.fold(recorded.fact());
        }
        assert_eq!(twice, in_order, "order {orders} twice");
    });
    assert_eq!(orders, 40_320);
}

#[test]
fn p32_b1_the_fold_of_a_larger_run_is_one_fold_in_fixed_seed_shuffled_orders() {
    let facts = larger_run();
    let in_order = fold_in(&facts.iter().collect::<Vec<_>>());
    let request_id = id::<RequestId>("request_", 2);
    let brief = in_order
        .brief(request_id, 200, reads(&in_order))
        .expect("one run's facts");
    assert_eq!(brief.state, ContainedRunState::Cancelled);
    assert_eq!(
        (brief.dispatch, brief.origin),
        (RunDispatch::Scheduled, RunOrigin::Scheduler)
    );
    assert_eq!(
        brief.recovery_package_labels,
        vec![OUTER, INNER, "neutral.recovery.last"]
    );
    assert_eq!(
        brief
            .terminal
            .as_ref()
            .and_then(|found| found.cancellation_reason),
        Some(ContainedTaskCancellationReason::ClientRequested)
    );
    assert_eq!(in_order.progress().map(|found| found.step_index), Some(11));
    for seed in 1..=256_u64 {
        let order = shuffled(&facts, seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let fold = fold_in(&order);
        assert_eq!(fold, in_order, "seed {seed}");
        // Each fact twice, the second time in reverse.
        let mut twice = fold.clone();
        for recorded in order.iter().rev() {
            twice.fold(recorded.fact());
        }
        assert_eq!(twice, in_order, "seed {seed} twice");
        assert_eq!(fold.brief(request_id, 200, reads(&fold)), Ok(brief.clone()));
    }
}

#[test]
fn p32_b1_two_terminals_stay_an_anomaly_in_every_order() {
    let mut facts = small_run();
    facts.pop();
    facts.push(terminal(11, 1, TaskOutcome::Failure));
    every_order(&facts, |order| {
        let fold = fold_in(order);
        assert_eq!(
            fold.anomaly_found(),
            Some(RunFactAnomaly::TerminalStateInconsistent)
        );
        assert_eq!(
            fold.brief(
                id("request_", 1),
                11,
                RunReads {
                    restarted_after_first_fact: false,
                    restarted_after_admission: false,
                    lease_terminal: None,
                },
            ),
            Err(RunFactAnomaly::TerminalStateInconsistent)
        );
        assert_eq!(fold.terminal(), None);
        assert_eq!(fold.listing().and_then(|listing| listing.settled()), None);
    });
}

#[test]
fn p32_b1_of_several_anomalies_the_fold_reports_the_one_a_read_in_sequence_order_meets_first() {
    // A second origin at 4, another run's step at 7 and a second terminal at 11.
    let mut facts = small_run();
    facts.push(origin(4, 1, EventType::UiAction, EventSource::Ui));
    facts.push(terminal(11, 1, TaskOutcome::Success));
    facts.retain(|recorded| recorded.sequence != 7);
    facts.push(step_started(7, 9, 3));
    facts.sort_by_key(|recorded| recorded.sequence);
    // In sequence order, after each fact: nothing until the second origin at 4.
    let mut fold = RunFold::new();
    for recorded in &facts {
        fold.fold(recorded.fact());
        let expected = (recorded.sequence >= 4).then_some(RunFactAnomaly::OriginInconsistent);
        assert_eq!(
            fold.anomaly_found(),
            expected,
            "after {}",
            recorded.sequence
        );
    }
    for seed in 1..=64_u64 {
        let fold = fold_in(&shuffled(&facts, seed));
        assert_eq!(
            fold.anomaly_found(),
            Some(RunFactAnomaly::OriginInconsistent),
            "seed {seed}"
        );
    }
    // Without the second origin, the foreign step at 7 comes first, before the second terminal.
    facts.retain(|recorded| recorded.event_type != EventType::UiAction);
    for seed in 1..=64_u64 {
        let fold = fold_in(&shuffled(&facts, seed));
        assert_eq!(
            fold.anomaly_found(),
            Some(RunFactAnomaly::IdentityMismatch),
            "seed {seed}"
        );
    }
}

#[test]
fn p32_b1_listing_facts_give_one_order_and_settled_state_in_every_order() {
    let cases: [(&[(u64, EventType)], u64, bool, Option<ContainedRunState>); 4] = [
        (
            &[
                (40, EventType::TaskEntryPreflight),
                (41, EventType::TaskRequested),
                (45, EventType::TaskEntryPreflight),
                (50, EventType::TaskCompleted),
            ],
            40,
            true,
            Some(ContainedRunState::Succeeded),
        ),
        (
            &[
                (60, EventType::TaskEntryPreflight),
                (61, EventType::TaskCancelled),
            ],
            60,
            false,
            Some(ContainedRunState::Cancelled),
        ),
        (
            &[(70, EventType::TaskRequested), (71, EventType::TaskFailed)],
            70,
            true,
            None,
        ),
        (
            &[
                (80, EventType::TaskRequested),
                (81, EventType::TaskCompleted),
                (82, EventType::TaskCancelled),
            ],
            80,
            true,
            None,
        ),
    ];
    for (facts, order_sequence, opened, settled) in cases {
        let recorded = facts
            .iter()
            .map(|(sequence, event_type)| Recorded {
                sequence: *sequence,
                event_type: *event_type,
                source: EventSource::Runtime,
                links: links(3, false, false),
                semantic: None,
            })
            .collect::<Vec<_>>();
        every_order(&recorded, |order| {
            let mut listing = RunListingFacts::first(order[0].sequence, order[0].event_type);
            for fact in order.iter().chain(order.iter()) {
                listing.fold(fact.sequence, fact.event_type);
            }
            assert_eq!(
                (
                    listing.order_sequence(),
                    listing.opened(),
                    listing.settled()
                ),
                (order_sequence, opened, settled)
            );
        });
    }
}

fn request() -> QueryRunsRequest {
    QueryRunsRequest {
        instance_id: Some(id("instance_", 7)),
        since_unix_ms: 1_800_000_000_000,
        until_unix_ms: Some(1_800_000_100_000),
        cursor: Some(QueryRunsCursor {
            snapshot_ledger_position: 500,
            before_sequence: 120,
        }),
        limit: 5,
        states: vec![ContainedRunState::Succeeded, ContainedRunState::Failed],
        dispatch: Some(RunDispatch::Scheduled),
        task_prefix: Some("neutral.routine.".to_owned()),
    }
}

fn page() -> QueryRunsPage {
    let facts = larger_run();
    let fold = fold_in(&facts.iter().collect::<Vec<_>>());
    let status = fold
        .brief(id("request_", 2), 500, reads(&fold))
        .expect("one run's facts");
    let listing = fold.listing().expect("listing facts");
    QueryRunsPage {
        rows: vec![QueryRunsRow {
            status,
            catalog_task_id: Some("neutral.routine.collect".to_owned()),
            order_sequence: listing.order_sequence(),
            first_fact_unix_ms: fold.first_fact().expect("first fact").1,
            terminal_unix_ms: fold.terminal().map(|(_, timestamp)| timestamp),
            settled: listing.settled(),
            opened_in_window: listing.opened(),
        }],
        snapshot_ledger_position: 500,
        next: Some(QueryRunsCursor {
            snapshot_ledger_position: 500,
            before_sequence: 11,
        }),
        coverage: QueryRunsCoverage {
            from_unix_ms: 1_799_900_000_000,
            state: QueryRunsCoverageState::Complete,
        },
    }
}

#[test]
fn p32_b1_query_runs_request_and_page_round_trip() {
    let request = request();
    assert_eq!(request.validate(), Ok(()));
    let encoded = serde_json::to_value(&request).expect("request JSON");
    assert_eq!(encoded["cursor"]["before_sequence"], 120);
    assert_eq!(encoded["states"], json!(["succeeded", "failed"]));
    let decoded: QueryRunsRequest = serde_json::from_value(encoded).expect("request back");
    assert_eq!(decoded, request);
    // The least request: a window start and a limit.
    let least: QueryRunsRequest =
        serde_json::from_value(json!({"since_unix_ms": 1, "limit": 1})).expect("least request");
    assert_eq!(least.validate(), Ok(()));
    assert_eq!(
        serde_json::to_value(&least).expect("least JSON"),
        json!({"since_unix_ms": 1, "limit": 1})
    );

    let page = page();
    assert_eq!(page.validate(), Ok(()));
    let encoded = serde_json::to_value(&page).expect("page JSON");
    assert_eq!(encoded["rows"][0]["status"]["state"], "cancelled");
    assert_eq!(encoded["rows"][0]["settled"], "cancelled");
    assert_eq!(
        encoded["coverage"],
        json!({"from_unix_ms": 1_799_900_000_000_u64, "state": "complete"})
    );
    let decoded: QueryRunsPage = serde_json::from_value(encoded).expect("page back");
    assert_eq!(decoded, page);
    // An unknown member is refused on decode.
    let mut unknown = serde_json::to_value(&request).expect("request JSON");
    unknown["profile"] = json!("forensic");
    assert!(serde_json::from_value::<QueryRunsRequest>(unknown).is_err());
}

#[test]
fn p32_b1_invalid_query_runs_requests_and_pages_are_refused_by_the_member() {
    let refused = |member: &'static str, request: QueryRunsRequest| {
        let invalid = request.validate().expect_err(member);
        assert_eq!(invalid.member(), member);
        assert_eq!(invalid.code(), RuntimeContractCode::InvalidQueryRuns.code());
        assert_eq!(
            RuntimeContractError::from(invalid).code(),
            RuntimeContractCode::InvalidQueryRuns.code()
        );
    };
    refused(
        "limit",
        QueryRunsRequest {
            limit: 0,
            ..request()
        },
    );
    refused(
        "limit",
        QueryRunsRequest {
            limit: 101,
            ..request()
        },
    );
    refused(
        "until_unix_ms",
        QueryRunsRequest {
            until_unix_ms: Some(1_799_999_999_999),
            ..request()
        },
    );
    for cursor in [(0, 1), (500, 0), (500, 501)] {
        refused(
            "cursor",
            QueryRunsRequest {
                cursor: Some(QueryRunsCursor {
                    snapshot_ledger_position: cursor.0,
                    before_sequence: cursor.1,
                }),
                ..request()
            },
        );
    }
    for states in [
        vec![ContainedRunState::Failed, ContainedRunState::Failed],
        vec![ContainedRunState::NotFound],
    ] {
        refused(
            "states",
            QueryRunsRequest {
                states,
                ..request()
            },
        );
    }
    for prefix in [String::new(), "p".repeat(257)] {
        refused(
            "task_prefix",
            QueryRunsRequest {
                task_prefix: Some(prefix),
                ..request()
            },
        );
    }

    let refused_page = |member: &'static str, change: &dyn Fn(&mut QueryRunsPage)| {
        let mut page = page();
        change(&mut page);
        let invalid = page.validate().expect_err(member);
        assert_eq!(invalid.member(), member);
        assert_eq!(invalid.code(), RuntimeContractCode::InvalidQueryRuns.code());
    };
    refused_page("snapshot_ledger_position", &|page: &mut QueryRunsPage| {
        page.snapshot_ledger_position = 0;
    });
    refused_page("coverage", &|page: &mut QueryRunsPage| {
        page.coverage.state = QueryRunsCoverageState::Unavailable;
    });
    refused_page("rows.order_sequence", &|page: &mut QueryRunsPage| {
        page.rows[0].order_sequence = 501
    });
    refused_page("rows.order_sequence", &|page: &mut QueryRunsPage| {
        let row = page.rows[0].clone();
        page.rows.push(row);
    });
    refused_page("rows.status.evidence", &|page: &mut QueryRunsPage| {
        page.rows[0].status.evidence.snapshot_ledger_position = 499;
    });
    refused_page("rows.status.state", &|page: &mut QueryRunsPage| {
        page.rows[0].status.state = ContainedRunState::NotFound;
        page.rows[0].settled = None;
    });
    refused_page("rows.terminal_unix_ms", &|page: &mut QueryRunsPage| {
        page.rows[0].terminal_unix_ms = None
    });
    refused_page("rows.settled", &|page: &mut QueryRunsPage| {
        page.rows[0].settled = Some(ContainedRunState::Succeeded);
    });
    refused_page("rows.catalog_task_id", &|page: &mut QueryRunsPage| {
        page.rows[0].status.dispatch = RunDispatch::Manual;
    });
    refused_page("next", &|page: &mut QueryRunsPage| {
        if let Some(next) = &mut page.next {
            next.before_sequence = 12;
        }
    });
    refused_page("next", &|page: &mut QueryRunsPage| {
        if let Some(next) = &mut page.next {
            next.snapshot_ledger_position = 499;
        }
    });
}
