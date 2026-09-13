// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #109 S2: the existing bounded store corpus, using the candidate owner.
use super::*;
use actingcommand_runtime_database::RuntimeDatabase;

// The original shared helpers also serve Host's other specifications.
#[allow(dead_code)]
mod host {
    use actingcommand_contract::{
        ApplicationLifecycleAction, IdentifierIssuer, InputAction, InstanceId, MonitorDiagnosis,
        MonitorObservation,
    };
    use actingcommand_device::{
        CaptureBackend, CaptureBackendName, DeviceError, DeviceErrorCategory,
        DeviceErrorSensitivity, DeviceResult, Frame, InputBackend, PixelFormat,
        PreparedSegmentedSwipePlan,
    };
    use actingcommand_execution_kernel::ExecutionBackendProvenance;
    use actingcommand_policy::{
        EvaluationFacts, EvaluationResources, FactValue, HostResourceSnapshot, InstanceSnapshot,
        ObservedOutcome, PoolValueSnapshot,
    };
    use actingcommand_recognition_pack::VisionProvider;
    use actingcommand_runtime_host::{
        ExecutionBackendProvider, PolicyInputSnapshot, ProcedureBinding, ProcedureManifest,
        ResolvedExecutionInstance, RuntimeHost, RuntimeHostConfig,
    };
    use actingcommand_scheduler::SchedulerConfig;
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::thread;
    use std::time::Duration;
    use tempfile::TempDir;

    include!("../../../../runtime-host/src/tests/support/backend.rs");
    include!("../../../../runtime-host/src/tests/support/startup.rs");
}

pub(in crate::global) fn database(root: &Path) -> Arc<RuntimeDatabase> {
    Arc::new(
        RuntimeDatabase::open_with_initializer::<GlobalLedgerError>(
            root,
            b"sqlite-ledger-contract-seed",
            |_| Ok(()),
            |_| Ok(()),
        )
        .expect("candidate physical owner"),
    )
}

pub(in crate::global) fn open(config: GlobalLedgerConfig) -> GlobalLedgerResult<GlobalLedger> {
    let database = database(&config.root);
    GlobalLedger::open_sqlite_candidate(config, database)
}

#[test]
fn sqlite_runs_the_existing_seven_store_contracts() {
    let held = std::cell::RefCell::new(None);
    store_contract::query_filters_by_sequence_and_all_typed_correlation_ids(
        |config| {
            let database = match held.borrow().as_ref() {
                Some(database) => Arc::clone(database),
                None => database(&config.root),
            };
            *held.borrow_mut() = Some(Arc::clone(&database));
            GlobalLedger::open_sqlite_candidate(config, database)
        },
        |config| {
            GlobalLedger::open_sqlite_candidate_read_only(
                config,
                Arc::clone(held.borrow().as_ref().expect("same database owner")),
                |_| None,
            )
            .map(Box::new)
            .map(store_contract::ContractReadOnly::Sqlite)
        },
    );
    store_contract::query_pages_are_bounded_ordered_and_pinned_to_the_requested_snapshot(open);
    store_contract::indexes_rebuild_after_reopen(open);
    store_contract::scheduled_settlement_continuation_is_one_shot_and_idempotent(open);
    store_contract::subscription_reports_terminal_writer_failure(open);
    store_contract::scheduling_projection_rejects_a_second_same_run_terminal_with_another_request(
        open,
    );
    store_contract::signature_catalog_rebuilds_versions_and_binds_paginated_multiple_matches(open);
}

#[test]
fn sqlite_owner_and_read_only_snapshot_preserve_live_writer_and_bounds() {
    let root = TempDir::new().expect("root");
    let database = database(root.path());
    let ledger = GlobalLedger::open_sqlite_candidate(config(&root, "owner"), Arc::clone(&database))
        .expect("writer");
    let first = ledger.append(event("first")).expect("first");
    let before = database
        .connection("test snapshot")
        .expect("connection")
        .total_changes();
    let snapshot = GlobalLedger::open_sqlite_candidate_read_only(
        GlobalLedgerReadOnlyConfig::new(root.path()),
        Arc::clone(&database),
        |_| None,
    )
    .expect("snapshot");
    assert_eq!(snapshot.events(), std::slice::from_ref(&first));
    assert_eq!(snapshot.latest_sequence(), 1);
    assert_eq!(
        database
            .connection("test snapshot")
            .expect("connection")
            .total_changes(),
        before
    );
    let conflict =
        GlobalLedger::open_sqlite_candidate(config(&root, "other"), Arc::clone(&database))
            .expect_err("same owner cannot have two ledgers");
    assert_eq!(conflict.code(), "writer_conflict");
    assert!(conflict.is_fatal());
    let bounded = GlobalLedger::open_sqlite_candidate_read_only(
        GlobalLedgerReadOnlyConfig::new(root.path()).with_budget(
            1,
            1,
            Instant::now() + Duration::from_secs(2),
        ),
        Arc::clone(&database),
        |_| None,
    );
    assert_eq!(
        bounded.err().expect("bounded read").code(),
        "ledger_read_budget_exceeded"
    );
    ledger
        .append(event("second"))
        .expect("writer remains usable");
    assert_eq!(snapshot.events(), &[first]);
    ledger.close().expect("close");
    let reopened = GlobalLedger::open_sqlite_candidate(config(&root, "next-owner"), database)
        .expect("ownership released");
    assert_eq!(reopened.latest_sequence().expect("position"), 2);
    reopened.close().expect("close reopened");

    // S3 reuses this owner/read-only specification and the same event factory.
    let imported_root = TempDir::new().expect("import root");
    let imported_database = self::database(imported_root.path());
    let legacy = GlobalLedger::open(GlobalLedgerConfig::new(
        imported_root.path().join("ledger"),
        "source",
    ))
    .expect("segment source");
    let expected = [
        legacy.append(event("first")).expect("source first"),
        legacy.append(event("second")).expect("source second"),
    ];
    legacy.close().expect("source closed");
    let source_owner_path = imported_root.path().join("ledger/writer.lock");
    let original_metadata = std::fs::read(&source_owner_path).expect("source ownership metadata");
    let limits = actingcommand_runtime_database::MaintenanceLimits::default();
    let deadline = limits.deadline().expect("deadline");
    let maintenance = LedgerMaintenance::acquire(imported_root.path(), false, limits, deadline)
        .expect("lock without journal writes");
    assert_eq!(
        maintenance.locked_material().expect("locked metadata")[0].1,
        original_metadata
    );
    let source = maintenance.source(|_| None).expect("complete source");
    let backup_parent = TempDir::new().expect("backup parent");
    let backup_path = backup_parent.path().join("frozen");
    let files = actingcommand_runtime_database::list_material(
        imported_root.path(),
        "ledger",
        &["ledger/writer.lock"],
        limits,
        deadline,
    )
    .expect("frozen source material");
    let backup = imported_database
        .backup(
            &backup_path,
            &files,
            &maintenance
                .locked_material()
                .expect("locked source metadata"),
            serde_json::json!({ "source": source.identity() }),
            limits,
            deadline,
        )
        .expect("consistent backup");
    assert_eq!(
        imported_database
            .verify_backup(&backup_path, limits, deadline)
            .expect("verified backup"),
        backup
    );
    let record = source
        .migration_record(
            &backup.backup_id,
            &actingcommand_runtime_database::digest(b"state fixture"),
        )
        .expect("migration identity");
    let completion = EventDraft::new(
        event_id(),
        1_752_147_200_001,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::System,
            OriginModule::GlobalLedger,
            EventActor::System,
        ),
        EventLinksDraft::default(),
        actingcommand_contract::LedgerPayloadDraft::migrated(record.clone(), AuditInput::new())
            .into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"test-private-salt").unwrap())
    .expect("typed completion");
    maintenance
        .import(
            &imported_database,
            &source,
            &record,
            completion.clone(),
            true,
        )
        .expect("dry-run transaction");
    assert_eq!(
        maintenance
            .status(&imported_database, |_| None)
            .expect("no schema committed"),
        LedgerStorageStatus::Missing
    );
    assert_eq!(
        maintenance.locked_material().expect("locked metadata")[0].1,
        original_metadata
    );
    let committed = maintenance
        .import(
            &imported_database,
            &source,
            &record,
            completion.clone(),
            false,
        )
        .expect("atomic import");
    assert_eq!(
        maintenance
            .import(
                &imported_database,
                &source,
                &record,
                completion.clone(),
                false
            )
            .expect("same frozen backup reentry"),
        committed
    );
    assert_eq!(
        maintenance.locked_material().expect("locked metadata")[0].1,
        original_metadata
    );
    let mut conflict = record.clone();
    conflict.backup_sha256 = actingcommand_runtime_database::digest(b"another backup");
    assert!(
        maintenance
            .import(
                &imported_database,
                &source,
                &conflict,
                completion.clone(),
                false
            )
            .is_err()
    );
    let imported = maintenance
        .open_writer(Arc::clone(&imported_database), "formal".into(), |_| None)
        .expect("continuous writer transfer");
    assert_eq!(
        GlobalLedger::open(GlobalLedgerConfig::new(
            imported_root.path().join("ledger"),
            "other-backend"
        ))
        .expect_err("same OS writer lock")
        .code(),
        "writer_conflict"
    );
    assert_eq!(
        GlobalLedger::open_sqlite_candidate(
            GlobalLedgerConfig::new(imported_root.path(), "candidate"),
            Arc::clone(&imported_database)
        )
        .expect_err("candidate cannot own production")
        .code(),
        "candidate_production_conflict"
    );
    assert_eq!(
        imported
            .append(completion)
            .expect_err("normal append cannot forge a migration")
            .code(),
        "migration_requires_import_transaction"
    );
    let facts = imported
        .query(EventQuery::default())
        .expect("imported facts");
    assert_eq!(&facts[..2], &expected);
    assert_eq!(facts.len(), 3);
    {
        let mut connection = imported_database
            .connection("formal original rows")
            .unwrap();
        let before = connection.total_changes();
        let transaction = connection.transaction().unwrap();
        let borrowed = imported_database.borrow_transaction(&transaction);
        for event in &facts {
            verify_transaction_event(&imported_database, &borrowed, event)
                .expect("imported original fact in the same transaction");
        }
        let other_owner = RuntimeDatabase::open_existing(imported_root.path(), true).unwrap();
        assert_eq!(
            verify_transaction_event(&other_owner, &borrowed, &facts[0])
                .expect_err("different Database owner")
                .code(),
            "ledger_transaction_owner_mismatch"
        );
        assert!(!transaction.is_autocommit());
        assert_eq!(transaction.total_changes(), before);
        transaction.rollback().unwrap();
    }
    let frozen_metadata =
        GlobalLedger::open_metadata(GlobalLedgerEvidenceConfig::new(imported_root.path())).unwrap();
    imported
        .append(event("after-cutover"))
        .expect("formal append preserves marker");
    let frozen_page = frozen_metadata
        .project_view_page(
            &EventQuery::default(),
            ProjectionProfile::Ui,
            &actingcommand_contract::RuntimeEventQueryPageRequest::default(),
        )
        .unwrap();
    assert_eq!(frozen_page.snapshot_ledger_position(), 3);
    assert_eq!(
        frozen_page
            .events()
            .iter()
            .map(|event| event.event_id)
            .collect::<Vec<_>>(),
        facts
            .iter()
            .map(|event| *event.event_id())
            .collect::<Vec<_>>()
    );
    let view = GlobalLedger::open_evidence(
        GlobalLedgerEvidenceConfig::new(imported_root.path()),
        |_| None,
    )
    .expect("formal immutable evidence");
    assert!(view.segment().is_none());
    assert!(view.is_complete());
    assert_eq!(&view.events()[..2], &expected);
    assert_eq!(view.latest_sequence(), 4);
    let metadata =
        GlobalLedger::open_metadata(GlobalLedgerEvidenceConfig::new(imported_root.path()))
            .expect("metadata source verifies the imported prefix and marker");
    assert_eq!(metadata.backend(), "sqlite");
    assert!(metadata.read_complete());
    assert_eq!(metadata.latest_sequence(), view.latest_sequence());
    let request = actingcommand_contract::RuntimeEventQueryPageRequest::default();
    let projected = metadata
        .project_view_page(
            &EventQuery::default(),
            ProjectionProfile::Forensic,
            &request,
        )
        .expect("metadata page");
    let online = imported
        .project_view_page(EventQuery::default(), ProjectionProfile::Forensic, request)
        .expect("writer page");
    assert_eq!(projected.events(), online.events());
    assert_eq!(
        projected.snapshot_ledger_position(),
        online.snapshot_ledger_position()
    );
    assert_eq!(
        projected.read_scope().unwrap().material_read,
        actingcommand_contract::LedgerMaterialReadState::NotRequested
    );
    let bounded_metadata = GlobalLedger::open_metadata(
        GlobalLedgerEvidenceConfig::new(imported_root.path()).with_budget(1, 1, deadline),
    )
    .err()
    .expect("bounded metadata read");
    assert_eq!(
        (
            bounded_metadata.code(),
            bounded_metadata.operation(),
            bounded_metadata.is_fatal(),
        ),
        ("ledger_read_budget_exceeded", "read_only_snapshot", false)
    );
    imported.close().expect("formal writer close");
    {
        let connection = imported_database
            .connection("supported pre-view offline root")
            .unwrap();
        let objects = connection
            .prepare("SELECT type,name FROM sqlite_schema WHERE name GLOB 'ledger_view_*'")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(objects.iter().filter(|(kind, _)| kind == "view").count(), 6);
        for (kind, name) in objects {
            connection
                .execute_batch(&format!("DROP {kind} {name}"))
                .unwrap();
        }
    }
    let prior_schema =
        GlobalLedger::open_metadata(GlobalLedgerEvidenceConfig::new(imported_root.path())).unwrap();
    for ledger_view in actingcommand_contract::LedgerView::ALL {
        let query = EventQuery {
            view: Some(ledger_view),
            ..EventQuery::default()
        };
        let request = actingcommand_contract::RuntimeEventQueryPageRequest::default();
        let page = prior_schema
            .project_view_page(&query, ProjectionProfile::Forensic, &request)
            .unwrap();
        let expected = view.query(&query);
        assert_eq!(
            page.events()
                .iter()
                .map(|event| event.event_id)
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|event| *event.event_id())
                .collect::<Vec<_>>()
        );
        assert_eq!(page.snapshot_ledger_position(), 4);
    }
    assert_eq!(
        imported_database
            .connection("read-only view schema unchanged")
            .unwrap()
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name GLOB 'ledger_view_*'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    let reopened = LedgerMaintenance::acquire(imported_root.path(), false, limits, deadline)
        .expect("formal lock released");
    assert!(matches!(
        reopened
            .status(&imported_database, |_| None)
            .expect("marker survives new facts"),
        LedgerStorageStatus::Ready {
            head_sequence: 4,
            migration: Some(_),
            ..
        }
    ));
    imported_database
        .connection("mutate marker in existing integrity specification")
        .unwrap()
        .execute("UPDATE ledger_meta SET migration_record='{}'", [])
        .unwrap();
    assert!(
        reopened
            .status(&imported_database, |_| None)
            .expect_err("malformed marker is fatal")
            .is_fatal()
    );
    assert!(
        GlobalLedger::open_metadata(GlobalLedgerEvidenceConfig::new(imported_root.path()))
            .err()
            .expect("metadata does not skip malformed marker")
            .is_fatal()
    );
    imported_database.connection("remove schema in existing integrity specification").unwrap().execute_batch("DROP TABLE ledger_artifacts; DROP TABLE ledger_links; DROP TABLE ledger_events; DROP TABLE ledger_meta;").unwrap();
    assert!(
        reopened
            .status(&imported_database, |_| None)
            .expect_err("formal format cannot fall back to old segments")
            .is_fatal()
    );
    reopened.close().expect("read owner close");

    {
        use actingcommand_runtime_host::RuntimeHost;
        use actingcommand_runtime_state::RuntimeStateStore;
        use host::{FakeProvider, FakeState, config, host_with_state, instance_id};

        // S3 extends the existing owner/startup specification through the formal offline entry.
        let source_root = TempDir::new().expect("legacy runtime root");
        drop(
            RuntimeStateStore::open(source_root.path(), b"runtime-host-test-salt")
                .expect("existing State material"),
        );
        let segment = GlobalLedger::open(GlobalLedgerConfig::new(
            source_root.path().join("ledger"),
            "legacy-source",
        ))
        .expect("legacy source");
        segment.close().expect("source closed");
        let original_writer = std::fs::read(source_root.path().join("ledger/writer.lock"))
            .expect("source writer bytes");
        let external = TempDir::new().expect("maintenance destinations");
        let backup = external.path().join("backup");
        let maintenance = |operation, target| {
            RuntimeHost::maintain_ledger(
                config(&source_root),
                actingcommand_runtime_host::LedgerMaintenanceRequest {
                    operation,
                    backup: Some(backup.clone()),
                    target,
                    artifact_root: None,
                    limits: Default::default(),
                },
            )
        };
        let refused = RuntimeHost::start(
            config(&source_root),
            Arc::new(FakeProvider::one(
                "node.a",
                instance_id(),
                Arc::new(FakeState::default()),
            )),
        )
        .err()
        .expect("legacy startup requires migration");
        assert_eq!(refused.code(), "ledger_migration_required");
        let frozen = maintenance(
            actingcommand_runtime_host::LedgerMaintenanceOperation::Backup,
            None,
        )
        .expect("formal frozen backup");
        assert_eq!(frozen.status, "backed-up");
        let preview = maintenance(
            actingcommand_runtime_host::LedgerMaintenanceOperation::DryRun,
            None,
        )
        .expect("formal dry-run");
        assert_eq!(preview.status, "dry-run");
        assert_eq!(
            serde_json::to_value(&preview.ledger).expect("preview status"),
            serde_json::to_value(LedgerStorageStatus::Missing).expect("missing status")
        );
        assert!(!preview.activated);
        let delivered = maintenance(
            actingcommand_runtime_host::LedgerMaintenanceOperation::Import,
            None,
        )
        .expect("formal import");
        assert_eq!(delivered.status, "imported");
        assert_eq!(delivered.backup_id, frozen.backup_id);
        let repeated = maintenance(
            actingcommand_runtime_host::LedgerMaintenanceOperation::Import,
            None,
        )
        .expect("formal idempotent import");
        assert_eq!(repeated.status, "already-imported");
        assert_eq!(repeated.ledger, delivered.ledger);
        assert_eq!(
            std::fs::read(source_root.path().join("ledger/writer.lock")).unwrap(),
            original_writer
        );
        let restore_target = external.path().join("restored");
        let restored = maintenance(
            actingcommand_runtime_host::LedgerMaintenanceOperation::Restore,
            Some(restore_target.clone()),
        )
        .expect("exact pre-cutover restore before later events");
        assert_eq!(restored.status, "restored");
        assert!(!restored.activated);
        assert_eq!(
            serde_json::to_value(&restored.ledger).expect("restored status"),
            serde_json::to_value(LedgerStorageStatus::Missing).expect("missing status")
        );
        let migrated = host_with_state(&source_root, "node.a", Arc::new(FakeState::default()));
        migrated
            .close()
            .expect("normal SQLite startup after cutover");
        let refused = maintenance(
            actingcommand_runtime_host::LedgerMaintenanceOperation::Restore,
            Some(external.path().join("discard-forbidden")),
        )
        .expect_err("new Runtime facts cannot be lost");
        assert!(matches!(
            refused.code.as_str(),
            "restore_would_discard_new_events" | "restore_state_has_advanced"
        ));
        assert!(!external.path().join("discard-forbidden").exists());
    }
}
