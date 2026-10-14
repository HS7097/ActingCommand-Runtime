// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #382 L-3 G5 with its fault hook LED-I1 (ledger key 4: a joint transaction with State
// work is atomic; a COMMIT error stays an error with the readback `committed`, `not_committed` or
// `unknown`, and never becomes a success). The requirements:
// - `contracts/ledger-store.md` "Catalog SQL transactions": an uncertain COMMIT stops the
//   affected writer, never claims NotPerformed and never resends; the commit readback compares
//   the record's rows, the meta row and the State work; unavailable, expired or conflicting
//   evidence remains Unknown.
// - `contracts/runtime-database.md`: a COMMIT error remains an error with its original detail and
//   observed disposition; it never produces a success response.
// - `contracts/ledger-maintenance.md`: an uncertain COMMIT reports the original error and a
//   readback classification.
// - `contracts/ledger-observer.md` §9.1: a writer without a failure plan makes one attempt, and
//   its first failure ends it.
//
// The writer is the SQLite candidate writer opened without a failure plan, so the readback
// classification is the only evidence; the running Runtime's writer reads a failed write back
// and tries it again, which `gate_383_ao1.rs` covers. The joint record is an approval decision
// whose work writes one row into a table of its own. The write fault gate makes the COMMIT report
// a failure after it took effect, or with its transaction rolled back. Nothing waits on a clock:
// `GATE_WAIT` only bounds a writer reply in a broken run.

use super::sqlite_contract;
use crate::global::{
    GlobalLedger, GlobalLedgerConfig, GlobalLedgerError, GlobalLedgerReadOnlyConfig,
    LedgerTransactionWork, Sha256SecretFingerprinter, TransactionStateObservation,
    TransactionWorkError,
};
use crate::{LedgerCode, LedgerLocation, LedgerWriteFaultGate, PersistedEvent};
use actingcommand_contract::{
    ApprovalDecisionRecord, ApprovalDisposition, ApprovalPayloadDraft, ApprovalReasonCode,
    ApprovalTarget, AuditInput, CommandPayloadDraft, EventAction, EventActor, EventDraft, EventId,
    EventLinksDraft, EventOrigin, EventSeverity, EventSource, EventType, IdentifierIssuer,
    OriginModule, SanitizedEventDraft,
};
use actingcommand_runtime_database::{DatabaseLocation, RuntimeDatabase, RuntimeTransaction};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

const GATE_WAIT: Duration = Duration::from_secs(30);
// The writer's owner id and the fingerprint salt: neutral fixture words.
const OWNER: &str = "ledger-l3-g5";
const SALT: &[u8] = b"ledger-l3-g5-fixture";
// The records' time.
const AT_MS: u64 = 1_752_147_200_000;
// The text after which the error's detail carries the readback classification.
const READBACK: &str = "joint_commit_readback=";

fn salt() -> Sha256SecretFingerprinter {
    Sha256SecretFingerprinter::new(SALT).expect("fingerprint salt")
}

// An ordinary record (`command.received`) the writer commits before the joint one.
fn ordinary(ids: &IdentifierIssuer) -> SanitizedEventDraft {
    EventDraft::new(
        ids.mint_event_id().expect("event id"),
        AT_MS,
        EventSeverity::Info,
        EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::User),
        EventLinksDraft::default(),
        CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into(),
    )
    .sanitize(&salt())
    .expect("an ordinary draft")
}

// The joint record: an approval decision of a catalog, one of the joint record types.
fn approval(ids: &IdentifierIssuer) -> SanitizedEventDraft {
    EventDraft::new(
        ids.mint_event_id().expect("event id"),
        AT_MS + 1,
        EventSeverity::Info,
        EventOrigin::new(EventSource::Ui, OriginModule::Governance, EventActor::User),
        EventLinksDraft::default()
            .with_request_id(ids.mint_request_id().expect("request id"))
            .with_correlation_id(ids.mint_correlation_id().expect("correlation id"))
            .with_action_id(ids.mint_action_id().expect("action id")),
        ApprovalPayloadDraft::decision(
            ApprovalDecisionRecord::new(
                "approval:fixture-g5",
                ApprovalDisposition::Approved,
                ApprovalTarget::Catalog {
                    catalog_hash: format!("sha256:{}", "d".repeat(64)),
                    catalog_version: 1,
                },
                ApprovalReasonCode::new("user_confirmed").expect("approval reason"),
            )
            .expect("approval record"),
            AuditInput::new(),
        )
        .into(),
    )
    .sanitize(&salt())
    .expect("an approval decision draft")
}

// The work's own table (`joint_work_marker`, one row per applied record holding the record's
// ledger sequence): whether it exists.
const MARKER_TABLE_EXISTS: &str =
    "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name='joint_work_marker'";

fn work_failed() -> TransactionWorkError {
    TransactionWorkError {
        code: LedgerCode::LedgerSqliteFailed.as_str(),
        operation: LedgerLocation::AppendTransaction.as_str(),
        fatal: true,
        detail: "the marker work's SQL failed".to_owned(),
    }
}

// The joint record's State work. It writes its row in the writer's transaction. Its `observe`
// reads the row back (a missing table reads as Unchanged); when `observable` is false it cannot
// tell and answers Unknown.
struct MarkerWork {
    observable: bool,
}

impl LedgerTransactionWork for MarkerWork {
    fn apply(
        &self,
        transaction: &RuntimeTransaction<'_, '_>,
        event: &PersistedEvent,
    ) -> Result<(), TransactionWorkError> {
        let sequence = i64::try_from(event.sequence()).map_err(|_| work_failed())?;
        let sql = transaction.sql();
        sql.execute_batch(
            "CREATE TABLE IF NOT EXISTS joint_work_marker (sequence INTEGER NOT NULL)",
        )
        .map_err(|_| work_failed())?;
        sql.execute(
            "INSERT INTO joint_work_marker (sequence) VALUES (?1)",
            [sequence],
        )
        .map_err(|_| work_failed())?;
        Ok(())
    }

    fn observe(
        &self,
        transaction: &RuntimeTransaction<'_, '_>,
    ) -> Result<TransactionStateObservation, TransactionWorkError> {
        if !self.observable {
            return Ok(TransactionStateObservation::Unknown);
        }
        let sql = transaction.sql();
        let tables: i64 = sql
            .query_row(MARKER_TABLE_EXISTS, [], |row| row.get(0))
            .map_err(|_| work_failed())?;
        if tables == 0 {
            return Ok(TransactionStateObservation::Unchanged);
        }
        let rows: i64 = sql
            .query_row("SELECT count(*) FROM joint_work_marker", [], |row| {
                row.get(0)
            })
            .map_err(|_| work_failed())?;
        Ok(match rows {
            0 => TransactionStateObservation::Unchanged,
            1 => TransactionStateObservation::Applied,
            _ => TransactionStateObservation::Unknown,
        })
    }
}

// The fault the joint record's COMMIT meets.
#[derive(Clone, Copy)]
enum Fault {
    // The COMMIT takes effect, then reports a failure.
    TookEffect,
    // The COMMIT reports a failure and the transaction did not take effect.
    DidNotTakeEffect,
}

// What one uncertain COMMIT of a joint record left: the closed root, the head before the record,
// the record's event id and the error `append_transaction` returned.
struct Uncertain {
    root: TempDir,
    head: u64,
    event_id: EventId,
    error: GlobalLedgerError,
}

// A candidate writer without a failure plan commits one ordinary record, then the joint record
// meets `fault` at its COMMIT. The call returns the original COMMIT error, never Ok, and does not
// claim NotPerformed; the attempt is the only one and nothing is committed after it; the writer
// stops: a later read finds no writer, a later record is refused and the close reports the
// failure.
fn uncertain_commit(fault: Fault, observable: bool) -> Uncertain {
    let root = TempDir::new().expect("candidate root");
    let database = sqlite_contract::database(root.path());
    let ledger = GlobalLedger::open_sqlite_candidate(
        GlobalLedgerConfig::new(root.path(), OWNER).with_reply_timeout(GATE_WAIT),
        Arc::clone(&database),
    )
    .expect("the candidate writer opens without a failure plan");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    ledger
        .append(ordinary(&ids))
        .expect("an ordinary record commits first");
    let head = ledger
        .latest_sequence()
        .expect("the head before the joint record");
    let faults = LedgerWriteFaultGate::arm(root.path()).expect("write fault gate");
    match fault {
        Fault::TookEffect => faults.commit_then_fail(1),
        Fault::DidNotTakeEffect => faults.commit_fails_uncommitted(1),
    }
    let draft = approval(&ids);
    let event_id = *draft.event_id();

    let error = match ledger.append_transaction(draft, Box::new(MarkerWork { observable })) {
        Ok(event) => panic!(
            "an uncertain COMMIT never returns Ok (returned sequence {})",
            event.sequence()
        ),
        Err(error) => error,
    };

    assert_eq!(faults.met(), 1, "the joint record's COMMIT met the fault");
    assert_eq!(
        (error.code(), error.operation()),
        (
            LedgerCode::LedgerSqliteFailed.as_str(),
            LedgerLocation::CommitSqliteEvent.as_str()
        ),
        "the original COMMIT error is reported: {error:?}"
    );
    assert!(
        error.rolled_back_work().is_none(),
        "an uncertain COMMIT never claims NotPerformed (confirmed rolled-back work)"
    );
    let read = ledger
        .latest_sequence()
        .expect_err("the writer stopped: a later read finds no writer");
    assert_eq!(read.code(), LedgerCode::WriterUnavailable.as_str());
    ledger
        .append(ordinary(&ids))
        .expect_err("the writer stopped: a later record is refused");
    assert!(ledger.close().is_err(), "the writer ended with its failure");
    assert!(
        faults.commits().is_empty(),
        "nothing committed after the uncertain attempt: the record is not sent again"
    );
    drop(faults);
    drop(database);
    Uncertain {
        root,
        head,
        event_id,
        error,
    }
}

// The readback classification the error's detail carries: the quote-delimited word after
// `joint_commit_readback=`, compared whole (`committed` is part of `not_committed`).
fn classification(error: &GlobalLedgerError) -> Option<&str> {
    let (_, after) = error.detail()?.split_once(READBACK)?;
    let (_, quoted) = after.split_once('"')?;
    quoted.split_once('"').map(|(word, _)| word)
}

fn assert_classified(uncertain: &Uncertain, expected: &str) {
    assert_eq!(
        classification(&uncertain.error),
        Some(expected),
        "the error carries the readback classification; detail: {:?}",
        uncertain.error.detail()
    );
}

// What a reopen of the root sees, on a new physical owner of its database: every ledger record,
// and the sequences in the work's table (none when the table is absent).
struct Reopened {
    events: Vec<PersistedEvent>,
    latest: u64,
    marker_rows: Vec<i64>,
}

fn reopen(root: &Path) -> Reopened {
    let database = sqlite_contract::database(root);
    let ledger = GlobalLedger::open_sqlite_candidate_read_only(
        GlobalLedgerReadOnlyConfig::new(root),
        Arc::clone(&database),
        |_| None,
    )
    .expect("the root reopens");
    Reopened {
        events: ledger.events().to_vec(),
        latest: ledger.latest_sequence(),
        marker_rows: marker_rows(&database),
    }
}

fn marker_rows(database: &RuntimeDatabase) -> Vec<i64> {
    let connection = database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("connection");
    let tables: i64 = connection
        .query_row(MARKER_TABLE_EXISTS, [], |row| row.get(0))
        .expect("the schema");
    if tables == 0 {
        return Vec::new();
    }
    let mut statement = connection
        .prepare("SELECT sequence FROM joint_work_marker ORDER BY rowid")
        .expect("the work's rows");
    let rows = statement
        .query_map([], |row| row.get::<_, i64>(0))
        .expect("the work's rows");
    rows.collect::<Result<Vec<_>, _>>()
        .expect("the work's rows")
}

// The reopen sees the joint record at the sequence after the head, the last record, and the
// work's one row for it: the record and its work committed together.
fn assert_reopen_sees_the_commit(uncertain: &Uncertain) {
    let reopened = reopen(uncertain.root.path());
    let expected = uncertain.head + 1;
    let after = reopened
        .events
        .iter()
        .filter(|event| event.sequence() > uncertain.head)
        .collect::<Vec<_>>();
    assert_eq!(after.len(), 1, "one record after the head: the joint one");
    assert_eq!(*after[0].event_id(), uncertain.event_id);
    assert_eq!(after[0].sequence(), expected);
    assert_eq!(after[0].event_type(), EventType::ApprovalDecision);
    assert_eq!(reopened.latest, expected);
    assert_eq!(
        reopened.marker_rows,
        vec![i64::try_from(expected).expect("a small sequence")],
        "the work's row is there, for the joint record"
    );
}

// Row 1: the COMMIT took effect and the work's `observe` sees its row: the readback is
// `committed`, and a reopen sees the record at the sequence after the head and the work's row.
#[test]
fn gate_l3_g5_a_commit_that_took_effect_reads_back_committed() {
    let uncertain = uncertain_commit(Fault::TookEffect, true);

    assert_classified(&uncertain, "committed");
    assert_reopen_sees_the_commit(&uncertain);
}

// Row 2: the COMMIT reported its failure and the transaction did not take effect: the readback
// is `not_committed`, and a reopen sees neither the record nor the work's row (the head is
// unchanged).
#[test]
fn gate_l3_g5_a_commit_that_did_not_take_effect_reads_back_not_committed() {
    let uncertain = uncertain_commit(Fault::DidNotTakeEffect, true);

    assert_classified(&uncertain, "not_committed");
    let reopened = reopen(uncertain.root.path());
    assert_eq!(reopened.latest, uncertain.head, "the head is unchanged");
    assert!(
        reopened
            .events
            .iter()
            .all(|event| *event.event_id() != uncertain.event_id),
        "the joint record is absent"
    );
    assert!(reopened.marker_rows.is_empty(), "the work's row is absent");
}

// Row 3: the COMMIT took effect but the work's `observe` cannot tell: the evidence is
// incomplete, so the readback is `unknown`, never an invented disposition. The record and its
// work still committed together.
#[test]
fn gate_l3_g5_a_readback_whose_work_cannot_tell_is_unknown() {
    let uncertain = uncertain_commit(Fault::TookEffect, false);

    assert_classified(&uncertain, "unknown");
    assert_reopen_sees_the_commit(&uncertain);
}
