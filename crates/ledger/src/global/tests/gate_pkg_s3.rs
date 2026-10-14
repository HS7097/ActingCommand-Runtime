// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #336 PKG-S3 (option 丙; ledger gate 47), model M-PKG v1.1 §4.4 S3-T1: a policy
// dispatch names its procedure binding, never its package's content. The dispatch intent this
// build writes carries no `package_digest` member and reads back exactly when the formal root is
// opened again, and a stored intent that still carries `package_digest` is not read, so the root
// does not open (`corrupt_ledger_record` at `decode_sqlite_record`).

use super::missing_material::formal_root;
use crate::global::{
    GlobalLedger, GlobalLedgerResult, LedgerMaintenance, LedgerOpenTiming,
    Sha256SecretFingerprinter,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    AuditInput, EventActor, EventDraft, EventLinksDraft, EventOrigin, EventQuery, EventSeverity,
    EventSource, EventType, IdentifierIssuer, OriginModule, PolicyDispatchEventData,
    PolicyPayloadDraft, PolicyReasonRecord,
};
use actingcommand_runtime_database::{DatabaseLocation, MaintenanceLimits, RuntimeDatabase};
use serde_json::{Value, json};
use std::sync::Arc;

// TCLOCK: every timestamp is a fixed constant.
const AT_MS: u64 = 1_752_147_200_000;
// The stored intent's event type, as the `ledger_events` table indexes it.
const INTENT_ROW: &str = "event_type='policy.dispatch_intent'";

// The writer of the existing formal root, opened as a start opens it.
fn reopen(database: &Arc<RuntimeDatabase>) -> GlobalLedgerResult<GlobalLedger> {
    let limits = MaintenanceLimits::default();
    LedgerMaintenance::acquire(
        database.root(),
        false,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("maintenance owner")
    .open_writer_timed(
        Arc::clone(database),
        "pkg-s3-reopen".into(),
        None,
        &mut LedgerOpenTiming::default(),
    )
}

// The object of `value` that holds `member`, depth first.
fn holder_of<'a>(
    value: &'a mut Value,
    member: &str,
) -> Option<&'a mut serde_json::Map<String, Value>> {
    // A shared look first: returning the guarded mutable borrow from a match arm is refused
    // by the borrow checker (E0499).
    if value
        .as_object()
        .is_some_and(|object| object.contains_key(member))
    {
        return value.as_object_mut();
    }
    match value {
        Value::Object(object) => object
            .values_mut()
            .find_map(|child| holder_of(child, member)),
        Value::Array(items) => items.iter_mut().find_map(|child| holder_of(child, member)),
        _ => None,
    }
}

// S3-T1: the dispatch intent of a binding (its `procedure_binding_digest`) has no package digest
// member; the root this build wrote opens and reads it back exactly; with `package_digest`
// written into the stored intent, the root is refused.
#[test]
fn s3_t1_a_dispatch_intent_names_no_package_digest_and_one_that_does_is_not_read() {
    let root = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(root.path());
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let links = EventLinksDraft::default()
        .with_instance_id(ids.mint_instance_id().expect("instance id"))
        .with_request_id(ids.mint_request_id().expect("request id"))
        .with_correlation_id(ids.mint_correlation_id().expect("correlation id"))
        .with_task_id(ids.mint_task_id().expect("task id"))
        .with_run_id(ids.mint_run_id().expect("run id"))
        .with_action_id(ids.mint_action_id().expect("action id"));
    let dispatch = PolicyDispatchEventData {
        decision_id: "decision:pkg-s3".to_owned(),
        task_id: "task:pkg-s3".to_owned(),
        instance_id: "instance:pkg-s3".to_owned(),
        operation_id: "operation:pkg-s3".to_owned(),
        procedure_binding_digest: format!("sha256:{}", "2".repeat(64)),
        reason_chain_id: "reason:pkg-s3".to_owned(),
        reasons: vec![PolicyReasonRecord(Outcome::relayed("placement_selected"))],
        catalog_hash: format!("sha256:{}", "3".repeat(64)),
        catalog_version: 1,
        input_ledger_position: 1,
        fact_snapshot_id: "snapshot:pkg-s3".to_owned(),
        approval_fact_ids: vec![],
        urgency_milli: 100,
    };
    let draft = EventDraft::new(
        ids.mint_event_id().expect("event id"),
        AT_MS,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Scheduler,
            OriginModule::Policy,
            EventActor::Scheduler,
        ),
        links,
        PolicyPayloadDraft::dispatch_intent(dispatch, AuditInput::new()).into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"gate-pkg-s3").expect("fingerprinter"))
    .expect("the intent sanitizes");
    let written = ledger.append(draft).expect("the intent is appended");
    ledger.close().expect("close the writer");

    let intents = EventQuery {
        event_type: Some(EventType::PolicyDispatchIntent),
        ..EventQuery::default()
    };
    let reopened = reopen(&database).expect("the root this build wrote opens");
    assert_eq!(
        reopened.query(intents).expect("the stored intents"),
        vec![written]
    );
    reopened.close().expect("close the reopened writer");

    let connection = database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("fixture connection");
    let record: Vec<u8> = connection
        .query_row(
            &format!("SELECT canonical_record FROM ledger_events WHERE {INTENT_ROW}"),
            [],
            |row| row.get(0),
        )
        .expect("the stored intent");
    let mut stored: Value = serde_json::from_slice(&record).expect("stored record JSON");
    let intent =
        holder_of(&mut stored, "procedure_binding_digest").expect("the stored intent's data");
    assert!(
        !intent.contains_key("package_digest"),
        "the stored intent names no package digest: {intent:?}"
    );
    intent.insert(
        "package_digest".to_owned(),
        json!(format!("sha256:{}", "c".repeat(64))),
    );
    let rewritten = serde_json::to_vec(&stored).expect("record JSON");
    assert_eq!(
        connection
            .execute(
                &format!("UPDATE ledger_events SET canonical_record=?1 WHERE {INTENT_ROW}"),
                rusqlite::params![rewritten],
            )
            .expect("rewrite the stored intent"),
        1
    );
    drop(connection);

    let Err(refused) = reopen(&database) else {
        panic!("a root whose dispatch intent carries a package digest is refused");
    };
    assert_eq!(
        (refused.code(), refused.operation()),
        ("corrupt_ledger_record", "decode_sqlite_record")
    );
}
