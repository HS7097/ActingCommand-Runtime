// SPDX-License-Identifier: AGPL-3.0-only

// LN-8 (#422, LEAN-L model §3.2 step 2, §6; brief rd-1026/brief-ln8.md "Tests"): the writer open
// reads the keyed meta row, `MAX(sequence)` and the newest `ledger_observer.open_verify_tail_records`
// records only, each authenticated on its own. Damage in that tail is refused at open, with the
// code and operation `ledger-maintenance verify` gives for it. Damage in an older row is not read
// at open; a read of that row refuses it, fatally, and so does `ledger-maintenance verify`, which
// reads every row.
//
// The roots are real formal SQLite roots (the fixture of `missing_material.rs`) holding plain
// records only, so the open's retention reads by type select none of them. Each open is the
// running Runtime's writer with a failure plan naming the tail; it closes cleanly, so no dump is
// imported and no gap or stale-owner record is written.

use super::missing_material::formal_root;
use crate::global::{
    GlobalLedger, GlobalLedgerError, LedgerFailurePlan, LedgerMaintenance, LedgerOpenTiming,
    LedgerWritePolicy, Sha256SecretFingerprinter,
};
use actingcommand_contract::{
    AuditInput, CommandPayloadDraft, EventAction, EventActor, EventDraft, EventLinksDraft,
    EventOrigin, EventQuery, EventSeverity, EventSource, IdentifierIssuer, OriginModule,
    OwnerEpoch, SanitizedEventDraft,
};
use actingcommand_runtime_database::{DatabaseLocation, MaintenanceLimits, RuntimeDatabase};
use actingcommand_runtime_host::{
    LedgerMaintenanceOperation, LedgerMaintenanceRequest, RuntimeHost, RuntimeHostConfig,
};
use std::sync::Arc;
use tempfile::TempDir;

// The fixture's State key (`missing_material.rs`), the writer's owner id and the fingerprint
// salt: neutral fixture words.
const STATE_KEY: &[u8] = b"runtime-host-test-salt";
const OWNER: &str = "ledger-ln8-fixture";
const SALT: &[u8] = b"ledger-ln8-fixture";
// The records' time and the epoch's start.
const AT_MS: u64 = 1_752_147_200_000;
// The records of a root and the open's tail, so most rows are older than the tail.
const RECORDS: u64 = 40;
const TAIL: usize = 8;
// A row well before the tail.
const OLD: u64 = 5;
// The write policy of the plans here: one attempt, no wait.
const STALL_MS: u64 = 120_000;
const PROBE_MS: u64 = 1_000;
const DUMP_MAX_BYTES: u64 = 1_073_741_824;

// A closed formal root of `RECORDS` plain records.
struct Root {
    dir: TempDir,
    database: Arc<RuntimeDatabase>,
    head: u64,
}

// A plain record without artifacts (`command.received`).
fn plain(ids: &IdentifierIssuer) -> SanitizedEventDraft {
    EventDraft::new(
        ids.mint_event_id().expect("event id"),
        AT_MS,
        EventSeverity::Info,
        EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::User),
        EventLinksDraft::default(),
        CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(SALT).expect("fingerprint salt"))
    .expect("a plain draft")
}

fn build() -> Root {
    let dir = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(dir.path());
    let ids = IdentifierIssuer::new().expect("issuer");
    for _ in 0..RECORDS {
        ledger.append(plain(&ids)).expect("plain record");
    }
    let head = ledger.latest_sequence().expect("head");
    ledger.close().expect("close the writer");
    assert!(
        OLD + (TAIL as u64) < head,
        "row {OLD} is older than the tail of {TAIL} before head {head}"
    );
    Root {
        dir,
        database,
        head,
    }
}

fn epoch() -> OwnerEpoch {
    *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_owner_epoch()
        .expect("owner epoch")
        .transport()
}

// A start's writer open of `root` with a tail of `TAIL` records.
fn open(root: &Root, timing: &mut LedgerOpenTiming) -> Result<GlobalLedger, GlobalLedgerError> {
    let state_root = root.dir.path().canonicalize().expect("state root");
    let plan = LedgerFailurePlan::new(
        LedgerWritePolicy::new(1, vec![0], STALL_MS, PROBE_MS, DUMP_MAX_BYTES)
            .expect("write policy"),
        &state_root,
        None,
        epoch(),
        AT_MS,
    )
    .with_open_verify_tail_records(TAIL);
    let limits = MaintenanceLimits::default();
    LedgerMaintenance::acquire(
        root.dir.path(),
        false,
        limits,
        limits.deadline().expect("deadline"),
    )?
    .open_writer_timed(Arc::clone(&root.database), OWNER.into(), Some(plan), timing)
}

// `ledger-maintenance verify` of `root`: its failure's code and operation.
fn verify(root: &Root) -> Result<(), (String, Option<String>)> {
    RuntimeHost::maintain_ledger(
        RuntimeHostConfig::new(root.dir.path(), STATE_KEY),
        LedgerMaintenanceRequest {
            operation: LedgerMaintenanceOperation::Verify,
            backup: None,
            target: None,
            artifact_root: None,
            limits: MaintenanceLimits::default(),
            frame_query: false,
        },
    )
    .map(|_| ())
    .map_err(|failure| {
        let outcome = failure.outcome();
        (
            outcome.code().as_str().to_owned(),
            outcome
                .value("operation")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
        )
    })
}

// A sequence as the ledger tables store it.
fn stored(sequence: u64) -> i64 {
    (sequence ^ (1_u64 << 63)) as i64
}

fn damage(root: &Root, sql: &str) {
    root.database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("fixture connection")
        .execute_batch(sql)
        .expect("damage the fixture");
}

// One damage of the tail: its label, its SQL for a root's head, and the code and operation both
// refusals give.
struct TailDamage {
    label: &'static str,
    sql: fn(u64) -> String,
    code: &'static str,
    operation: &'static str,
}

#[test]
fn ln8_damage_in_the_tail_is_refused_at_open_as_verify_refuses_it() {
    let cases = [
        TailDamage {
            label: "a tail row's tag",
            sql: |head| {
                format!(
                    "UPDATE ledger_events SET integrity_tag='ln8-invalid-tag' WHERE sequence={}",
                    stored(head - 1)
                )
            },
            code: "ledger_record_mismatch",
            operation: "verify_sqlite_record",
        },
        TailDamage {
            label: "the head's record",
            sql: |head| {
                format!(
                    "UPDATE ledger_events SET canonical_record=CAST('ln8-invalid-json' AS BLOB) WHERE sequence={}",
                    stored(head)
                )
            },
            code: "corrupt_ledger_record",
            operation: "parse_sqlite_record",
        },
        TailDamage {
            label: "a tail row's link",
            sql: |head| {
                format!(
                    "UPDATE ledger_links SET run_id='ln8-invalid-run' WHERE sequence={}",
                    stored(head - 2)
                )
            },
            code: "ledger_index_mismatch",
            operation: "verify_sqlite_relations",
        },
        TailDamage {
            label: "the hash the oldest tail row chains to",
            sql: |head| {
                format!(
                    "UPDATE ledger_events SET record_sha256='sha256:{}' WHERE sequence={}",
                    "0".repeat(64),
                    stored(head - TAIL as u64)
                )
            },
            code: "ledger_record_mismatch",
            operation: "verify_sqlite_record",
        },
        TailDamage {
            label: "the head row",
            sql: |head| {
                let head = stored(head);
                format!(
                    "DELETE FROM ledger_links WHERE sequence={head}; DELETE FROM ledger_artifacts WHERE sequence={head}; DELETE FROM ledger_events WHERE sequence={head}"
                )
            },
            code: "ledger_meta_mismatch",
            operation: "verify_sqlite_head",
        },
    ];
    for TailDamage {
        label,
        sql,
        code,
        operation,
    } in cases
    {
        let root = build();
        damage(&root, &sql(root.head));
        // `ledger-maintenance verify` reads every row and refuses the damage.
        let verified = verify(&root).expect_err(label);
        assert_eq!(
            verified,
            (code.to_owned(), Some(operation.to_owned())),
            "{label}: verify"
        );
        // The start's open reads the tail and refuses it with the same code and operation.
        let mut timing = LedgerOpenTiming::default();
        let refused = open(&root, &mut timing)
            .err()
            .unwrap_or_else(|| panic!("{label}: the open refuses damage in its tail"));
        assert_eq!(
            (refused.code(), refused.operation()),
            (code, operation),
            "{label}: open"
        );
        assert!(refused.is_fatal(), "{label}: {refused}");
        assert!(
            timing.events <= TAIL as u64,
            "{label}: the open read at most its tail: {timing:?}"
        );
    }
}

#[test]
fn ln8_damage_in_an_old_row_is_not_read_at_open_and_is_found_by_its_read_and_by_verify() {
    let root = build();
    damage(
        &root,
        &format!(
            "UPDATE ledger_events SET integrity_tag='ln8-invalid-tag' WHERE sequence={}",
            stored(OLD)
        ),
    );
    // `ledger-maintenance verify` reads every row and refuses the old row.
    assert_eq!(
        verify(&root),
        Err((
            "ledger_record_mismatch".to_owned(),
            Some("verify_sqlite_record".to_owned())
        ))
    );

    // The start's open reads the newest `TAIL` rows only, and opens.
    let mut timing = LedgerOpenTiming::default();
    let ledger = open(&root, &mut timing).expect("the open does not read the old row");
    assert_eq!(timing.events, TAIL as u64, "{timing:?}");
    assert_eq!(ledger.latest_sequence().expect("head"), root.head);
    // A read of the tail answers.
    let newest = ledger
        .query(EventQuery {
            from_sequence: Some(root.head - 1),
            ..EventQuery::default()
        })
        .expect("a read of the tail");
    assert_eq!(newest.len(), 2);
    // A read of the old row refuses it, fatally, with the code the complete read gives.
    let refused = ledger
        .query(EventQuery {
            from_sequence: Some(OLD),
            to_sequence: Some(OLD),
            ..EventQuery::default()
        })
        .expect_err("a read of the damaged row refuses it");
    assert_eq!(
        (refused.code(), refused.operation()),
        ("ledger_record_mismatch", "verify_sqlite_record")
    );
    assert!(refused.is_fatal(), "{refused}");
    ledger.close().expect("close the writer");
}
