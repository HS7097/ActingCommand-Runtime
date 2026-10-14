// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #336 PKG-S2 (option 丙; ledger gate 45), model M-PKG v1.1 §4.3 S2-T1 and S2-T2: a
// run's package facts name each package by its `package_id` (`package_label`) and carry no
// package digest. The records a formal root holds read back exactly when the root is opened
// again; the run's recovery view pairs an entry-recovery failure with a later completion of the
// same package, by label; and a stored admission that still carries `package_sha256` is not
// read, so the root does not open (`corrupt_ledger_record` at `decode_sqlite_record`).

use super::missing_material::formal_root;
use crate::PersistedEvent;
use crate::global::{
    GlobalLedger, GlobalLedgerResult, LedgerMaintenance, LedgerOpenTiming,
    Sha256SecretFingerprinter,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    AuditInput, EventActor, EventDraft, EventLinksDraft, EventOrigin, EventQuery, EventSeverity,
    EventSource, IdentifierIssuer, IssuedRunId, LedgerRecoveryState, OriginModule,
    ProjectionProfile, RuntimeEventQueryPageRequest, TaskPayloadDraft, TaskSemanticFact,
};
use actingcommand_runtime_database::{DatabaseLocation, MaintenanceLimits, RuntimeDatabase};
use serde_json::{Value, json};
use std::sync::Arc;

/// TCLOCK: every timestamp is a fixed constant.
const AT_MS: u64 = 1_752_147_200_000;
/// The stored admission's event type, as the `ledger_events` table indexes it.
const ADMISSION_ROW: &str = "event_type='task.requested'";

/// The writer of the existing formal root, opened as a start opens it.
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
        "pkg-s2-reopen".into(),
        None,
        &mut LedgerOpenTiming::default(),
    )
}

/// One contained run's identities, and the facts it appends as the host appends them.
struct Run {
    ids: IdentifierIssuer,
    run_id: IssuedRunId,
    links: EventLinksDraft,
    fingerprinter: Sha256SecretFingerprinter,
}

impl Run {
    fn new() -> Self {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let run_id = ids.mint_run_id().expect("run id");
        let links = EventLinksDraft::default()
            .with_instance_id(ids.mint_instance_id().expect("instance id"))
            .with_request_id(ids.mint_request_id().expect("request id"))
            .with_correlation_id(ids.mint_correlation_id().expect("correlation id"))
            .with_task_id(ids.mint_task_id().expect("task id"))
            .with_run_id(run_id);
        Self {
            ids,
            run_id,
            links,
            fingerprinter: Sha256SecretFingerprinter::new(b"gate-pkg-s2").expect("fingerprinter"),
        }
    }

    fn append(
        &self,
        ledger: &GlobalLedger,
        offset_ms: u64,
        fact: TaskSemanticFact,
    ) -> PersistedEvent {
        // A failed layer is a warning, every other fact information, as the host records them.
        let severity = if matches!(fact, TaskSemanticFact::EntryRecoveryFailed { .. }) {
            EventSeverity::Warning
        } else {
            EventSeverity::Info
        };
        let draft = EventDraft::new(
            self.ids.mint_event_id().expect("event id"),
            AT_MS + offset_ms,
            severity,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
            ),
            self.links.clone(),
            TaskPayloadDraft::semantic(fact, AuditInput::new()).into(),
        )
        .sanitize(&self.fingerprinter)
        .expect("the fact sanitizes");
        ledger.append(draft).expect("the fact is appended")
    }

    fn query(&self) -> EventQuery {
        EventQuery {
            run_id: Some(*self.run_id.transport()),
            ..EventQuery::default()
        }
    }
}

// S2-T1: the run's admission names the package by `package_label` and `task_label`; every
// record the run wrote reads back exactly from the reopened root, and the same root with the
// admission carrying `package_sha256` does not open.
#[test]
fn s2_t1_a_run_s_package_facts_read_back_by_label_and_a_digest_member_is_not_read() {
    let root = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(root.path());
    let run = Run::new();
    let written = vec![
        run.append(
            &ledger,
            1,
            TaskSemanticFact::PackageAdmitted {
                package_label: "neutral.main".to_owned(),
                task_label: "task".to_owned(),
                response_deadline_monotonic_ms: Some(60_000),
            },
        ),
        run.append(
            &ledger,
            2,
            TaskSemanticFact::EntryRecoveryPackageAdmitted {
                package_label: "neutral.layer".to_owned(),
                role: None,
            },
        ),
        run.append(
            &ledger,
            3,
            TaskSemanticFact::EntryRecoveryFailed {
                package_label: Some("neutral.layer".to_owned()),
                outcome: Outcome::relayed("contained_task_page_unknown"),
                role: None,
            },
        ),
    ];
    ledger.close().expect("close the writer");

    let reopened = reopen(&database).expect("the root this build wrote opens");
    assert_eq!(
        reopened.query(run.query()).expect("the run's records"),
        written
    );
    reopened.close().expect("close the reopened writer");

    let connection = database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("fixture connection");
    let record: Vec<u8> = connection
        .query_row(
            &format!("SELECT canonical_record FROM ledger_events WHERE {ADMISSION_ROW}"),
            [],
            |row| row.get(0),
        )
        .expect("the stored admission");
    let mut stored: Value = serde_json::from_slice(&record).expect("stored record JSON");
    let fact = stored
        .pointer_mut("/payload/payload/data/fact")
        .and_then(Value::as_object_mut)
        .expect("the stored admission's fact");
    let mut members = fact.keys().cloned().collect::<Vec<_>>();
    members.sort();
    assert_eq!(
        members,
        [
            "kind",
            "package_label",
            "response_deadline_monotonic_ms",
            "task_label"
        ]
    );
    fact.insert("package_sha256".to_owned(), json!("c".repeat(64)));
    let rewritten = serde_json::to_vec(&stored).expect("record JSON");
    assert_eq!(
        connection
            .execute(
                &format!("UPDATE ledger_events SET canonical_record=?1 WHERE {ADMISSION_ROW}"),
                rusqlite::params![rewritten],
            )
            .expect("rewrite the stored admission"),
        1
    );
    drop(connection);

    let Err(refused) = reopen(&database) else {
        panic!("a root whose admission carries a package digest is refused");
    };
    assert_eq!(
        (refused.code(), refused.operation()),
        ("corrupt_ledger_record", "decode_sqlite_record")
    );
}

// S2-T2: a two-layer prerequisite chain records each layer's own `package_label`, and the run's
// recovery view pairs the inner layer's failure with the inner layer's later completion, not
// with the outer layer's.
#[test]
fn s2_t2_each_layer_names_its_package_and_recovery_pairs_by_label() {
    let root = tempfile::tempdir().expect("root");
    let (_database, ledger) = formal_root(root.path());
    let run = Run::new();
    let outer = "neutral.outer";
    let inner = "neutral.inner";
    run.append(
        &ledger,
        1,
        TaskSemanticFact::EntryRecoveryPackageAdmitted {
            package_label: outer.to_owned(),
            role: None,
        },
    );
    run.append(
        &ledger,
        2,
        TaskSemanticFact::EntryRecoveryPackageAdmitted {
            package_label: inner.to_owned(),
            role: None,
        },
    );
    let failure = run.append(
        &ledger,
        3,
        TaskSemanticFact::EntryRecoveryFailed {
            package_label: Some(inner.to_owned()),
            outcome: Outcome::relayed("contained_task_page_unknown"),
            role: None,
        },
    );
    run.append(
        &ledger,
        4,
        TaskSemanticFact::EntryRecoveryCompleted {
            package_label: outer.to_owned(),
            final_page: "home".to_owned(),
            executed_steps: 1,
            role: None,
        },
    );
    let recovered = run.append(
        &ledger,
        5,
        TaskSemanticFact::EntryRecoveryCompleted {
            package_label: inner.to_owned(),
            final_page: "home".to_owned(),
            executed_steps: 1,
            role: None,
        },
    );

    let page = ledger
        .project_view_page(
            run.query(),
            ProjectionProfile::Ui,
            RuntimeEventQueryPageRequest::default(),
        )
        .expect("the run's page");
    let [group] = page.run_recovery() else {
        panic!("one recovery group: {:?}", page.run_recovery());
    };
    assert_eq!(group.state, LedgerRecoveryState::Recovered);
    let [resolution] = group.evidence.as_slice() else {
        panic!("one failure: {:?}", group.evidence);
    };
    assert_eq!(resolution.failure.event_id, *failure.event_id());
    assert_eq!(
        resolution.success.map(|success| success.event_id),
        Some(*recovered.event_id())
    );
    ledger.close().expect("close the writer");
}
