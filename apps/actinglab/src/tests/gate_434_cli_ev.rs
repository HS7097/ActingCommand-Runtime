// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 CLI-EV (register §1.306; coordinator work order
// rd-1111/brief-cli-events-newest.md, rulings 1 to 5): `events` without a position or a window
// answers the newest `limit` records its other filters match, oldest first within the page, with
// `first_sequence`; it reads the ledger's head once, then windows of 256, 4 096 and 65 536
// sequences that end at the head, every read a sequence range pinned to the head's snapshot and
// without run recovery; when the largest window holds fewer than `limit` matches, the page says
// where the search stopped (`searched_from_sequence`). With a position or a window it reads as
// before. Through the people's command (`actinglab events`, the CLI module's row, which hands
// lab-ops' `events` its words) on the runtime client's scripted Runtime, whose ledger is made-up
// records at chosen sequences (the scripted ledger answers at its last record's sequence and needs
// no record between the ones it holds):
// - `cli_ev_no_position_answers_the_newest_limit_oldest_first`: 600 records that all match; the
//   default limit and `--limit 5` answer the newest 20 and 5, oldest first, with their first and
//   last sequences and no bound, the first window read being the 256 sequences at the head.
// - `cli_ev_fewer_matches_than_the_limit_name_the_bound_and_no_read_leaves_the_window`: a filter
//   that two records inside the largest window match and two below it: the page is those two with
//   `searched_from_sequence` the window's lower bound, the reads are the three windows' parts and
//   the rows' read, all inside the window; `--through-sequence <first_sequence - 1>` then reads
//   the two below it, as before.
// - `cli_ev_a_sparse_filter_is_found_by_widening_within_the_fixed_steps`: a filter whose records
//   lie in the second and third windows only: the newest two are found by the two widenings, the
//   third window's part halved until its newest record, within the bound on reads.
// - `cli_ev_a_position_or_a_window_reads_as_before`: with `after`, `through_sequence`,
//   `since_unix_ms`, `until_unix_ms`, `run`, `request` and `correlation_id` the command sends one
//   page request, unpinned and with run recovery, of the query it sent before, and prints the
//   page's members of before only; a cursor this process did not issue is `cursor_invalid` before
//   any read.
// Fakes only: no device, no clock, no sleep.

use super::*;
use crate::cli_module;
use actingcommand_contract::{
    AuditInput, EventAction, EventActor, EventDraft, EventLinksDraft, EventOrigin,
    EventPayloadDraft, EventQuery, EventSeverity, EventSource, IdentifierIssuer, OriginModule,
    ProjectedEvent, ProjectionPayload, ProjectionProfile, RuntimeEventQueryPageRequest,
    RuntimeOperation, RuntimePayloadDraft, SanitizationError, SecretField, SecretFingerprinter,
    Sha256Fingerprint,
};
use actingcommand_lab_ops::cli_line::Exit;
use actingcommand_lab_ops::events::{self, NEWEST_LARGEST_WINDOW, NEWEST_WINDOWS};
use actingcommand_lab_ops::front::Connection;
use actingcommand_lab_ops::initiator::ENVIRONMENT_VARIABLE;
use actingcommand_runtime_client::test_support::{ScriptedConnection, ScriptedRuntime};
use serde_json::{Value, json};
use std::ffi::OsString;
use std::path::Path;

/// The time of the made-up record at sequence 0; a record's time is this plus its sequence.
const RECORDED_FROM_UNIX_MS: u64 = 1_700_000_000_000;
/// The type of the records the filtered reads ask for; the others are `runtime.takeover`.
const WANTED: &str = "runtime.started";
/// Typed identifiers, as the Runtime spells them; no made-up record carries them.
const RUN: &str = "run_0123456789abcdef0123456789abcdef";
const REQUEST: &str = "request_0123456789abcdef0123456789abcdef";
const CORRELATION: &str = "correlation_0123456789abcdef0123456789abcdef";
/// The most reads one search may send: the head's, one per window, the narrowing of one window's
/// part (its top, then at most one halving per bit of the largest width, then the last sequence)
/// and the rows' read: 23.
const MOST_READS: usize =
    1 + NEWEST_WINDOWS.len() + (1 + NEWEST_LARGEST_WINDOW.ilog2() as usize + 1) + 1;
/// A cursor of the right form that no process of this test issued.
const FOREIGN_CURSOR: &str =
    "v1.0000000000000001.1.30.5.0000000000000000000000000000000000000000000000000000000000000000";
/// The members a page of a read from a position or a window had before the slice, with the
/// line's outcome.
const MEMBERS_BEFORE: [&str; 5] = [
    "events",
    "next_cursor",
    "snapshot_ledger_position",
    "last_sequence",
    "outcome",
];

struct NoSecrets;

impl SecretFingerprinter for NoSecrets {
    fn fingerprint(
        &self,
        _field: SecretField,
        _original: &str,
    ) -> Result<Sha256Fingerprint, SanitizationError> {
        panic!("the made-up records hold no secret")
    }
}

/// One made-up record at `sequence`, as a forensic read projects it: `runtime.started` when
/// `wanted`, `runtime.takeover` otherwise.
fn record(issuer: &IdentifierIssuer, sequence: u64, wanted: bool) -> ProjectedEvent {
    let payload: EventPayloadDraft = if wanted {
        RuntimePayloadDraft::started(EventAction::RuntimeStart, AuditInput::new()).into()
    } else {
        RuntimePayloadDraft::takeover(EventAction::RuntimeTakeover, AuditInput::new()).into()
    };
    let sanitized = EventDraft::new(
        issuer.mint_event_id().expect("event id"),
        RECORDED_FROM_UNIX_MS + sequence,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        EventLinksDraft::default(),
        payload,
    )
    .sanitize(&NoSecrets)
    .expect("sanitize a made-up record");
    ProjectedEvent {
        views: Vec::new(),
        artifact_evictions: Vec::new(),
        schema_version: sanitized.schema_version().to_owned(),
        sequence,
        event_id: *sanitized.event_id(),
        timestamp_unix_ms: sanitized.timestamp_unix_ms(),
        event_type: sanitized.event_type(),
        severity: sanitized.severity(),
        sensitivity: sanitized.sensitivity(),
        origin: sanitized.origin().clone(),
        links: sanitized.links().clone(),
        payload_schema: sanitized.payload_schema().to_owned(),
        payload: ProjectionPayload::Full(Box::new(sanitized.payload().clone())),
        artifacts: Vec::new(),
    }
}

/// A ledger of made-up records at `sequences` (ascending), each wanted as `wanted` says.
fn ledger(sequences: &[(u64, bool)]) -> Vec<ProjectedEvent> {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    sequences
        .iter()
        .map(|(sequence, wanted)| record(&issuer, *sequence, *wanted))
        .collect()
}

/// One QueryEvents request the scripted Runtime received.
struct Read {
    query: EventQuery,
    page: RuntimeEventQueryPageRequest,
    profile: ProjectionProfile,
}

impl Read {
    /// The sequence range the read asks for.
    fn range(&self) -> (Option<u64>, Option<u64>) {
        (self.query.from_sequence, self.query.to_sequence)
    }
}

/// One command's answer: its exit code, its line and every QueryEvents request it sent.
struct Served {
    code: u8,
    line: Value,
    reads: Vec<Read>,
}

/// `run` (given the state root) on a scripted Runtime whose one connection answers its event
/// queries from `records`; the command makes that one connection.
fn served(records: Vec<ProjectedEvent>, run: impl FnOnce(&Path) -> Exit) -> Served {
    let temp = TempDir::new().unwrap();
    let state = temp.path().join("state");
    fs::create_dir_all(&state).unwrap();
    let runtime =
        ScriptedRuntime::start(&state, |_| vec![ScriptedConnection::new().ledger(records)]);
    let exit = {
        let _state_root = use_runtime_state_root(&state);
        run(&state)
    };
    let record = runtime.finish();
    assert!(
        record.unscripted.is_empty(),
        "a connection beyond the command's one"
    );
    let reads = record
        .connections
        .iter()
        .flat_map(|connection| connection.requests.iter())
        .filter_map(|request| match request.operation() {
            RuntimeOperation::QueryEvents {
                query,
                profile,
                page,
            } => Some(Read {
                query: query.clone(),
                page: page.clone(),
                profile: *profile,
            }),
            _ => None,
        })
        .collect();
    Served {
        code: exit.code,
        line: exit.line.expect("a JSON line"),
        reads,
    }
}

/// `actinglab <line>` through the CLI module, on a scripted Runtime holding `records`.
fn people(line: &[&str], records: Vec<ProjectedEvent>) -> Served {
    let words = line
        .iter()
        .map(|word| (*word).to_owned())
        .collect::<Vec<_>>();
    served(records, |_| cli_module::run_line(&words))
}

/// The seq of each row the line printed.
fn rows(line: &Value) -> Vec<u64> {
    line["events"]
        .as_array()
        .unwrap_or_else(|| panic!("a line without events: {line}"))
        .iter()
        .map(|row| {
            row["seq"]
                .as_u64()
                .unwrap_or_else(|| panic!("a row without seq: {row}"))
        })
        .collect()
}

/// The line is `runtime_events_reported`.
fn assert_reported(served: &Served, what: &str) {
    assert_eq!(
        (served.code, &served.line["outcome"]["code"]),
        (0, &json!("runtime_events_reported")),
        "{what}: {}",
        served.line
    );
}

/// The reads of a search below `head`: first the head's, a page of one row that selects nothing
/// (its range starts past every sequence, no other condition, no snapshot, no run recovery); then
/// each a sequence range inside the largest window, `[lowest, head]`, pinned to the head's
/// snapshot, without a cursor and without run recovery; at most [`MOST_READS`] of them.
fn assert_bounded(reads: &[Read], head: u64, lowest: u64, what: &str) {
    assert!(
        reads.len() <= MOST_READS,
        "{what}: {} reads, more than {MOST_READS}",
        reads.len()
    );
    let (probe, rest) = reads
        .split_first()
        .unwrap_or_else(|| panic!("{what}: no read"));
    assert!(
        probe.query.from_sequence.is_some_and(|from| from > head)
            && probe.query.to_sequence.is_none()
            && probe.page.limit() == 1
            && probe.page.snapshot_position().is_none()
            && probe.page.cursor().is_none()
            && !probe.page.derives_run_recovery(),
        "{what}: the first read is not the head's: {:?} {:?}",
        probe.query,
        probe.page
    );
    assert_eq!(
        EventQuery {
            from_sequence: None,
            ..probe.query.clone()
        },
        EventQuery::default(),
        "{what}: the head's read carries a condition"
    );
    for read in rest {
        let (Some(from), Some(through)) = read.range() else {
            panic!("{what}: a read without its range: {:?}", read.query);
        };
        assert!(
            lowest <= from && from <= through && through <= head,
            "{what}: the read [{from}, {through}] leaves the window [{lowest}, {head}]"
        );
        assert_eq!(
            (read.page.snapshot_position(), read.page.cursor()),
            (Some(head), None),
            "{what}: the read [{from}, {through}] is not pinned to the head's snapshot"
        );
        assert!(
            !read.page.derives_run_recovery(),
            "{what}: the read [{from}, {through}] derives run recovery"
        );
    }
}

/// The lower bound of the largest window that ends at `head`.
fn window_floor(head: u64) -> u64 {
    head.saturating_sub(NEWEST_LARGEST_WINDOW - 1).max(1)
}

#[test]
fn cli_ev_no_position_answers_the_newest_limit_oldest_first() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let every = (1..=600)
        .map(|sequence| (sequence, true))
        .collect::<Vec<_>>();
    for (line, limit) in [
        (&["events"][..], 20_u64),
        (&["events", "--limit", "5"][..], 5),
    ] {
        let served = people(line, ledger(&every));
        assert_reported(&served, &format!("{line:?}"));
        let newest = (601 - limit..=600).collect::<Vec<_>>();
        assert_eq!(
            rows(&served.line),
            newest,
            "{line:?}: not the newest {limit} records, oldest first: {}",
            served.line
        );
        assert_eq!(
            (
                &served.line["first_sequence"],
                &served.line["last_sequence"],
                &served.line["snapshot_ledger_position"]
            ),
            (&json!(601 - limit), &json!(600), &json!(600)),
            "{line:?}: {}",
            served.line
        );
        assert!(
            served.line.get("searched_from_sequence").is_none()
                && served.line.get("next_cursor").is_none(),
            "{line:?}: a page of the newest {limit} names a bound or a next page: {}",
            served.line
        );
        assert_bounded(&served.reads, 600, 1, &format!("{line:?}"));
        // The first window is the 256 sequences at the head.
        assert_eq!(
            served.reads[1].range(),
            (Some(600 - NEWEST_WINDOWS[0] + 1), Some(600)),
            "{line:?}: the first window"
        );
        // The rows are read once more, with their payloads, from the first to the last.
        let last = served.reads.last().expect("the rows' read");
        assert_eq!(
            (last.range(), last.profile),
            ((Some(601 - limit), Some(600)), ProjectionProfile::Forensic),
            "{line:?}: the rows' read"
        );
    }
}

#[test]
fn cli_ev_fewer_matches_than_the_limit_name_the_bound_and_no_read_leaves_the_window() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let head = 70_010;
    let lowest = window_floor(head);
    let sequences = [
        (10, true),
        (20, true),
        (69_990, true),
        (69_995, false),
        (70_000, true),
        (head, false),
    ];
    let line = ["events", "--event-type", WANTED];
    let served = people(&line, ledger(&sequences));
    assert_reported(&served, "the filtered page");
    assert_eq!(
        rows(&served.line),
        [69_990, 70_000],
        "not the two records of the window: {}",
        served.line
    );
    assert_eq!(
        (
            &served.line["first_sequence"],
            &served.line["searched_from_sequence"],
            &served.line["snapshot_ledger_position"]
        ),
        (&json!(69_990), &json!(lowest), &json!(head)),
        "the page does not name where the search stopped: {}",
        served.line
    );
    assert_bounded(&served.reads, head, lowest, "the filtered page");
    // The head's read, the three windows' parts, each below the one before, and the rows' read.
    let first = head - NEWEST_WINDOWS[0] + 1;
    let second = head - NEWEST_WINDOWS[1] + 1;
    assert_eq!(
        served
            .reads
            .iter()
            .skip(1)
            .map(Read::range)
            .collect::<Vec<_>>(),
        [
            (Some(first), Some(head)),
            (Some(second), Some(first - 1)),
            (Some(lowest), Some(second - 1)),
            (Some(69_990), Some(70_000)),
        ],
        "the reads of the search"
    );
    assert!(
        served
            .reads
            .iter()
            .skip(1)
            .all(|read| read.query.event_type.is_some()),
        "a read of the search without its filter"
    );

    // The records before the page, read back from its first sequence: the two below the window,
    // oldest first, as a read with a window reads.
    let before = (69_990 - 1).to_string();
    let line = [
        "events",
        "--event-type",
        WANTED,
        "--through-sequence",
        before.as_str(),
    ];
    let served = people(&line, ledger(&sequences));
    assert_reported(&served, "the page before");
    assert_eq!(rows(&served.line), [10, 20], "{}", served.line);
    assert_eq!(served.reads.len(), 1, "the page before is one read");
    assert_eq!(
        (
            served.reads[0].range(),
            served.reads[0].page.snapshot_position()
        ),
        ((None, Some(69_989)), None),
        "the page before"
    );
}

#[test]
fn cli_ev_a_sparse_filter_is_found_by_widening_within_the_fixed_steps() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let head = 10_000;
    // Nothing in the first window; one record in the second; four in the third.
    let sequences = [
        (1_000, true),
        (3_000, true),
        (4_000, true),
        (5_000, true),
        (9_000, true),
        (head, false),
    ];
    let line = ["events", "--event-type", WANTED, "--limit", "2"];
    let served = people(&line, ledger(&sequences));
    assert_reported(&served, "the sparse page");
    assert_eq!(
        rows(&served.line),
        [5_000, 9_000],
        "not the newest two records: {}",
        served.line
    );
    assert_eq!(served.line["first_sequence"], json!(5_000));
    assert!(
        served.line.get("searched_from_sequence").is_none(),
        "a page that holds its limit names a bound: {}",
        served.line
    );
    assert_bounded(&served.reads, head, 1, "the sparse page");
    // The first window, widened twice: the part of each the one before did not hold.
    let first = head - NEWEST_WINDOWS[0] + 1;
    let second = head - NEWEST_WINDOWS[1] + 1;
    assert_eq!(
        served
            .reads
            .iter()
            .skip(1)
            .take(3)
            .map(Read::range)
            .collect::<Vec<_>>(),
        [
            (Some(first), Some(head)),
            (Some(second), Some(first - 1)),
            (Some(1), Some(second - 1)),
        ],
        "the windows of the search"
    );
    // The third window's part held more than the one record still wanted: the later reads
    // halve it, each inside it, and the last reads the two rows.
    let halving = &served.reads[4..served.reads.len() - 1];
    assert!(
        !halving.is_empty()
            && halving.iter().all(|read| {
                let (from, through) = read.range();
                from >= Some(1) && through <= Some(second - 1)
            }),
        "the third window's part was not halved inside itself"
    );
    assert_eq!(
        served.reads.last().map(Read::range),
        Some((Some(5_000), Some(9_000))),
        "the rows' read"
    );
}

#[test]
fn cli_ev_a_position_or_a_window_reads_as_before() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let sequences = (1..=30)
        .map(|sequence| (sequence, sequence % 2 == 1))
        .collect::<Vec<_>>();
    let since = (RECORDED_FROM_UNIX_MS + 28).to_string();
    let until = (RECORDED_FROM_UNIX_MS + 3).to_string();
    let cases: [(&[&str], Value, Vec<u64>); 7] = [
        (
            &["events", "--after", "25"],
            json!({"from_sequence": 26}),
            (26..=30).collect(),
        ),
        (
            &["events", "--through-sequence", "5"],
            json!({"to_sequence": 5}),
            (1..=5).collect(),
        ),
        (
            &["events", "--since-unix-ms", since.as_str()],
            json!({"from_timestamp_unix_ms": RECORDED_FROM_UNIX_MS + 28}),
            (28..=30).collect(),
        ),
        (
            &["events", "--until-unix-ms", until.as_str()],
            json!({"to_timestamp_unix_ms": RECORDED_FROM_UNIX_MS + 3}),
            (1..=2).collect(),
        ),
        (
            &["events", "--run", RUN],
            json!({"run_id": RUN}),
            Vec::new(),
        ),
        (
            &["events", "--request", REQUEST],
            json!({"request_id": REQUEST}),
            Vec::new(),
        ),
        (
            &["events", "--correlation-id", CORRELATION],
            json!({"correlation_id": CORRELATION}),
            Vec::new(),
        ),
    ];
    for (line, query, expected) in cases {
        let query = serde_json::from_value::<EventQuery>(query).expect("the query sent before");
        let served = people(line, ledger(&sequences));
        assert_reported(&served, &format!("{line:?}"));
        let [read] = served.reads.as_slice() else {
            panic!("{line:?}: {} reads, not one", served.reads.len());
        };
        assert_eq!(
            (&read.query, &read.page, read.profile),
            (
                &query,
                &RuntimeEventQueryPageRequest::new(20, None).expect("a page request"),
                ProjectionProfile::Forensic
            ),
            "{line:?}: not the read sent before"
        );
        assert_eq!(rows(&served.line), expected, "{line:?}: {}", served.line);
        let members = served
            .line
            .as_object()
            .expect("a line is an object")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        assert!(
            members.iter().all(|member| MEMBERS_BEFORE.contains(member)),
            "{line:?}: a member the page did not have before: {}",
            served.line
        );
        assert_eq!(served.line["snapshot_ledger_position"], json!(30));
    }

    // A cursor (actingctl's word; lab-ops' `events` serves both) that this process did not
    // issue: `cursor_invalid`, before any read, as before.
    let refused = served(ledger(&sequences), |state| {
        let words = [
            OsString::from("events"),
            OsString::from("--state-root"),
            state.as_os_str().to_owned(),
            OsString::from("--cursor"),
            OsString::from(FOREIGN_CURSOR),
        ];
        events::dispatch_on(&words, Connection::per_call()).expect("the words of events")
    });
    assert_eq!(
        (refused.code, &refused.line["outcome"]["code"]),
        (1, &json!("cursor_invalid")),
        "a foreign cursor: {}",
        refused.line
    );
    assert!(refused.reads.is_empty(), "a foreign cursor was read");
}
