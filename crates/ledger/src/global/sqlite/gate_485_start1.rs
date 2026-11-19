// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #485 START-1, fix round 1 (register §1.489; `contracts/ledger-store.md`, "The runs of a
// range"; work order rd-1119/brief-485-start1-fix1.md, after the review of pull request #1846):
// the runs of a range, which the start's settlement of the earlier epochs reads, on the SQLite
// writer a production start uses, its statements spanning two sequences
// (`with_history_scan_rows(2)`; 65 536 unless set), so that a range of more than two sequences is
// read in several spans, as a production range of more than 65 536 records is. The fixture: twelve
// records of three runs and of none, appended in this order (the letter of each one's run, `-` for
// a record without a run link): `AB-A--CB-AC-`.
// - L1: the whole history is read in at least one writer command per span and answers each run
//   once, with its first record, in the order of those records; the range after the third record
//   answers A and B, whose first records lie before it, with their first records in it, and orders
//   the runs by those (A, C, B).
// - L2: the range's two ends are the old read's. Until 0.12.74 the settlement read the range with
//   the query `from_sequence` = previous + 1, `to_sequence` = current - 1 (both inclusive), paged
//   after `previous` through `current - 1`; since then it asks `run_starts_between(previous,
//   current - 1)`. The lower bound is outside the range and the upper bound inside: the record at
//   the lower bound is not answered, the one at the upper bound is. For every range of the history,
//   and past its head, the answer is the runs of the records that query returns, each with its
//   first one, in their order.
// - L3: records without a run link answer no run; an empty range (`after` equal to `through`)
//   answers nothing; a range whose upper bound lies past the ledger's head answers the runs through
//   the head and does not fail.
// - L4: the statement's plan, by the crate's plan check (`plan_check::check_plan`: an empty
//   database built as a new root's, no statistics): `RUN_STARTS` breaks no rule of it and reads
//   `ledger_links` by a range of its row id, never by a scan of the table and never through an
//   index (the run link's `ledger_links_run` among them).
// No clock is read; every record has one fixed time.

use super::RUN_STARTS;
use super::plan_check::check_plan;
use crate::global::tests::sqlite_contract;
use crate::global::{GlobalLedger, GlobalLedgerConfig, LedgerRunStart, Sha256SecretFingerprinter};
use actingcommand_contract::{
    AuditInput, CommandPayloadDraft, EventAction, EventActor, EventDraft, EventLinksDraft,
    EventOrigin, EventQuery, EventSeverity, EventSource, IdentifierIssuer, OriginModule, RunId,
};
use std::collections::BTreeSet;
use std::path::Path;
use tempfile::TempDir;

// The writer's owner id and the drafts' fingerprint salt: neutral fixture words.
const OWNER: &str = "ledger-start1-fixture";
const SALT: &[u8] = b"ledger-start1-fixture-salt";
// Every record's time.
const AT_MS: u64 = 1_752_147_200_000;
// The sequences one statement of the runs of a range spans.
const SPAN_ROWS: usize = 2;
// The fixture's records in their order: the letter of each one's run, `-` for a record without a
// run link.
const RECORDS: &str = "AB-A--CB-AC-";
const LETTERS: [char; 3] = ['A', 'B', 'C'];
// The plan step that reads the link rows by a range of their row id.
const BY_ROW_ID: &str = "SEARCH ledger_links USING INTEGER PRIMARY KEY (rowid>? AND rowid<?)";

/// The fixture's writer, its three runs (A, B, C) and each record's sequence, in ledger order.
struct Fixture {
    ledger: GlobalLedger,
    runs: [RunId; 3],
    sequences: Vec<u64>,
}

impl Fixture {
    /// The fixture's records appended in their order to a new SQLite root at `root`, its writer's
    /// statements spanning `SPAN_ROWS` sequences. The records of a run share its request.
    fn written(root: &Path) -> Self {
        let ledger = sqlite_contract::open(
            GlobalLedgerConfig::new(root, OWNER).with_history_scan_rows(SPAN_ROWS),
        )
        .expect("the writer opens");
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let salt = Sha256SecretFingerprinter::new(SALT).expect("fingerprinter");
        let runs: [_; 3] = std::array::from_fn(|_| {
            (
                ids.mint_run_id().expect("a run id"),
                ids.mint_request_id().expect("a request id"),
            )
        });
        let sequences = RECORDS
            .chars()
            .map(|record| {
                let links = match LETTERS.iter().position(|letter| *letter == record) {
                    Some(index) => EventLinksDraft::default()
                        .with_run_id(runs[index].0)
                        .with_request_id(runs[index].1),
                    None => EventLinksDraft::default()
                        .with_request_id(ids.mint_request_id().expect("a request id")),
                };
                let draft = EventDraft::new(
                    ids.mint_event_id().expect("an event id"),
                    AT_MS,
                    EventSeverity::Info,
                    EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::User),
                    links,
                    CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new())
                        .into(),
                )
                .sanitize(&salt)
                .expect("a fixture draft");
                ledger.append(draft).expect("a fixture record").sequence()
            })
            .collect();
        Self {
            ledger,
            runs: runs.map(|(run, _)| *run.transport()),
            sequences,
        }
    }

    /// The sequence of the fixture's record `index` (the first is 0).
    fn at(&self, index: usize) -> u64 {
        self.sequences[index]
    }

    fn head(&self) -> u64 {
        self.ledger.latest_sequence().expect("the head")
    }

    /// `GlobalLedger::run_starts_between(after, through)`.
    fn runs_between(&self, after: u64, through: u64) -> Vec<LedgerRunStart> {
        self.ledger
            .run_starts_between(after, through)
            .unwrap_or_else(|error| panic!("the runs after {after} through {through}: {error:?}"))
    }

    /// The runs of the records the settlement's read of a range returned until 0.12.74 (its query:
    /// `from_sequence` after `after`, `to_sequence` `through`, both inclusive), each with its first
    /// record there, in ledger order.
    fn old_read_runs(&self, after: u64, through: u64) -> Vec<LedgerRunStart> {
        let mut known = BTreeSet::new();
        self.ledger
            .query(EventQuery {
                from_sequence: Some(after + 1),
                to_sequence: Some(through),
                ..EventQuery::default()
            })
            .unwrap_or_else(|error| {
                panic!("the old read after {after} through {through}: {error:?}")
            })
            .iter()
            .filter_map(|event| {
                let run_id = *event.links().run_id()?;
                known.insert(run_id).then_some(LedgerRunStart {
                    run_id,
                    first_sequence: event.sequence(),
                })
            })
            .collect()
    }

    /// `starts` as the fixture's run letters (`?` for a run that is not the fixture's) with their
    /// first sequences, so that a failed comparison says which run.
    fn named(&self, starts: &[LedgerRunStart]) -> Vec<(char, u64)> {
        starts
            .iter()
            .map(|start| {
                let letter = self
                    .runs
                    .iter()
                    .position(|run| *run == start.run_id)
                    .map_or('?', |index| LETTERS[index]);
                (letter, start.first_sequence)
            })
            .collect()
    }
}

/// L1: the whole history in at least one writer command per span, each run once with its first
/// record, in their order; after the third record, A and B with their first records in the range.
#[test]
fn start1_l1_a_run_across_several_spans_is_answered_once_with_its_first_record_in_the_range() {
    let root = TempDir::new().expect("state root");
    let fixture = Fixture::written(root.path());
    let head = fixture.head();
    let spans = head.div_ceil(SPAN_ROWS as u64);
    fixture.ledger.take_writer_peak();
    let whole = fixture.runs_between(0, head);
    // The writer counts a command once it has answered it, before it takes the next one.
    assert_eq!(fixture.head(), head, "nothing was appended");
    let commands = fixture.ledger.take_writer_peak().commands;
    assert!(
        commands >= spans,
        "the {head} records were read in {commands} writer commands, not one a span of \
         {SPAN_ROWS} sequences ({spans})"
    );
    assert_eq!(
        fixture.named(&whole),
        vec![
            ('A', fixture.at(0)),
            ('B', fixture.at(1)),
            ('C', fixture.at(6))
        ],
        "each run once, with its first record, in the order of those records"
    );
    assert_eq!(
        fixture.named(&fixture.runs_between(fixture.at(2), head)),
        vec![
            ('A', fixture.at(3)),
            ('C', fixture.at(6)),
            ('B', fixture.at(7))
        ],
        "A and B, whose first records lie before the range, with their first records in it"
    );
    fixture.ledger.close().expect("close");
}

/// L2: the record at the lower bound is outside the range, the one at the upper bound inside; for
/// every range, also past the head, the runs of the records the old read's query returns.
#[test]
fn start1_l2_the_lower_bound_is_outside_and_the_upper_bound_inside_as_the_old_read_took_them() {
    let root = TempDir::new().expect("state root");
    let fixture = Fixture::written(root.path());
    // B's record at the lower bound is not answered (B's next record lies past the upper bound);
    // C's record at the upper bound is.
    assert_eq!(
        fixture.named(&fixture.runs_between(fixture.at(1), fixture.at(6))),
        vec![('A', fixture.at(3)), ('C', fixture.at(6))]
    );
    // A range of one sequence: C's record at the lower bound outside, B's at the upper bound
    // inside.
    assert_eq!(
        fixture.named(&fixture.runs_between(fixture.at(6), fixture.at(7))),
        vec![('B', fixture.at(7))]
    );
    let past = fixture.head() + 2;
    for after in 0..=past {
        for through in after..=past {
            assert_eq!(
                fixture.named(&fixture.runs_between(after, through)),
                fixture.named(&fixture.old_read_runs(after, through)),
                "the runs after {after} through {through} are those of the old read's records"
            );
        }
    }
    fixture.ledger.close().expect("close");
}

/// L3: records without a run link answer no run, an empty range nothing, and a range past the
/// head the runs through the head, without failing.
#[test]
fn start1_l3_records_without_a_run_and_an_empty_range_answer_nothing_past_the_head_no_failure() {
    let root = TempDir::new().expect("state root");
    let fixture = Fixture::written(root.path());
    let head = fixture.head();
    let unlinked = fixture.named(&fixture.runs_between(fixture.at(3), fixture.at(5)));
    assert!(
        unlinked.is_empty(),
        "two records without a run link name no run: {unlinked:?}"
    );
    for empty in [0, fixture.at(6), head] {
        let none = fixture.named(&fixture.runs_between(empty, empty));
        assert!(
            none.is_empty(),
            "the range after {empty} through {empty} holds no sequence: {none:?}"
        );
    }
    assert_eq!(
        fixture.named(&fixture.runs_between(fixture.at(9), head + 5)),
        vec![('C', fixture.at(10))],
        "a range past the head answers the runs through the head"
    );
    for after in [head, head + 1] {
        let none = fixture.named(&fixture.runs_between(after, head + 5));
        assert!(
            none.is_empty(),
            "a range after {after}, at or past the head {head}, holds no record: {none:?}"
        );
    }
    fixture.ledger.close().expect("close");
}

/// L4: by the crate's plan check, the statement breaks no rule and reads the link rows by a range
/// of their row id: no scan of the table, no index.
#[test]
fn start1_l4_the_statement_reads_the_link_rows_by_their_row_id_and_breaks_no_plan_rule() {
    let check = check_plan(RUN_STARTS);
    assert!(check.violations.is_empty(), "{check:?}");
    let steps = check
        .plan
        .iter()
        .map(|step| step.trim_start())
        .collect::<Vec<_>>();
    assert!(
        steps.contains(&BY_ROW_ID),
        "the link rows are read by a range of their row id: {check:?}"
    );
    assert!(
        !steps.iter().any(|step| step.starts_with("SCAN ")),
        "no step scans the table: {check:?}"
    );
    assert!(
        !steps
            .iter()
            .any(|step| step.contains(" INDEX ") || step.contains("ledger_links_run")),
        "no step reads an index, the run link's among them: {check:?}"
    );
}
