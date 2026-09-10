// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #109 S2: bounded SQLite integrity/transaction differential criteria.
use super::*;
use crate::global::{GlobalLedger, Sha256SecretFingerprinter, tests::sqlite_contract::database};
use actingcommand_contract::*;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use tempfile::TempDir;

fn draft(timestamp: u64) -> SanitizedEventDraft {
    EventDraft::new(
        IdentifierIssuer::new()
            .expect("issuer")
            .mint_event_id()
            .expect("event id"),
        timestamp,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        EventLinksDraft::default(),
        CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"sqlite-ledger-contract").expect("salt"))
    .expect("draft")
}

fn config(root: &Path, owner: &str) -> GlobalLedgerConfig {
    GlobalLedgerConfig::new(root, owner)
}

#[test]
fn ordered_u64_round_trips_extremes_and_preserves_sql_order() {
    let root = TempDir::new().expect("root");
    let database = database(root.path());
    let values = [0, 1, i64::MAX as u64, 1_u64 << 63, u64::MAX - 1, u64::MAX];
    let connection = database
        .connection("encoding specification")
        .expect("connection");
    let stored = connection
        .prepare("SELECT column1 FROM (VALUES (?1),(?2),(?3),(?4),(?5),(?6)) ORDER BY column1")
        .expect("integer query")
        .query_map(params_from_iter(values.map(encode)), |row| {
            row.get::<_, i64>(0)
        })
        .expect("ordered integers")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("read integers");
    assert_eq!(stored.into_iter().map(decode).collect::<Vec<_>>(), values);
    for value in values {
        assert_eq!(decode(encode(value)), value);
    }
    drop(connection);
    let sqlite = GlobalLedger::open_sqlite_candidate(
        config(root.path(), "integer-writer"),
        Arc::clone(&database),
    )
    .expect("ledger");
    let segment_root = TempDir::new().expect("segment root");
    let segment =
        GlobalLedger::open(config(segment_root.path(), "integer-reference")).expect("reference");
    for timestamp in values.into_iter().skip(1) {
        let draft = draft(timestamp);
        assert_eq!(
            sqlite.append(draft.clone()).expect("sqlite fact"),
            segment.append(draft).expect("segment fact")
        );
    }
    let expected = segment
        .query(EventQuery::default())
        .expect("reference facts");
    assert!(views::installed(&database.connection("derived schema").unwrap()).unwrap());
    for view in LedgerView::ALL {
        for from in values {
            let query = EventQuery {
                view: Some(view),
                from_timestamp_unix_ms: Some(from),
                to_timestamp_unix_ms: Some(u64::MAX),
                ..EventQuery::default()
            };
            let request = RuntimeEventQueryPageRequest::new(2, None).unwrap();
            assert_eq!(
                sqlite
                    .project_view_page(query.clone(), ProjectionProfile::Ui, request.clone())
                    .unwrap(),
                segment
                    .project_view_page(query, ProjectionProfile::Ui, request)
                    .unwrap()
            );
        }
    }
    sqlite.close().expect("close sqlite");
    segment.close().expect("close segment");
    {
        let connection = database
            .connection("pre-view schema specification")
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
        for (kind, name) in objects {
            connection
                .execute_batch(&format!("DROP {kind} {name}"))
                .unwrap();
        }
        assert!(!views::installed(&connection).unwrap());
    }
    let reopened = GlobalLedger::open_sqlite_candidate(
        config(root.path(), "integer-reopened"),
        Arc::clone(&database),
    )
    .expect("reopen");
    assert!(views::installed(&database.connection("writer schema upgrade").unwrap()).unwrap());
    assert_eq!(
        reopened.query(EventQuery::default()).expect("facts"),
        expected
    );
    reopened.close().expect("close");
}

#[test]
fn sqlite_integrity_matrix_rejects_changed_and_missing_material() {
    for (label, sql) in [
        (
            "canonical JSON",
            "UPDATE ledger_events SET canonical_record=CAST('token-secret-invalid-json' AS BLOB) WHERE sequence=-9223372036854775807",
        ),
        (
            "record hash",
            "UPDATE ledger_events SET record_sha256='token-secret-invalid-hash' WHERE sequence=-9223372036854775807",
        ),
        (
            "previous hash",
            "UPDATE ledger_events SET previous_record_sha256='token-secret-invalid-chain' WHERE sequence=-9223372036854775806",
        ),
        (
            "record tag",
            "UPDATE ledger_events SET integrity_tag='token-secret-invalid-tag' WHERE sequence=-9223372036854775807",
        ),
        (
            "indexed event",
            "UPDATE ledger_events SET severity='warning' WHERE sequence=-9223372036854775807",
        ),
        (
            "indexed link",
            "UPDATE ledger_links SET run_id='token-secret-invalid-run' WHERE sequence=-9223372036854775807",
        ),
        (
            "missing link",
            "DELETE FROM ledger_links WHERE sequence=-9223372036854775807",
        ),
        (
            "missing event",
            "DELETE FROM ledger_links WHERE sequence=-9223372036854775806; DELETE FROM ledger_events WHERE sequence=-9223372036854775806",
        ),
        (
            "metadata next",
            "UPDATE ledger_meta SET next_sequence=next_sequence+1",
        ),
        (
            "metadata tag",
            "UPDATE ledger_meta SET integrity_tag='token-secret-invalid-meta'",
        ),
        ("missing metadata", "DELETE FROM ledger_meta"),
        ("partial view schema", "DROP VIEW ledger_view_health_v1"),
        (
            "changed view schema",
            "DROP VIEW ledger_view_health_v1; CREATE VIEW ledger_view_health_v1 AS SELECT * FROM ledger_events",
        ),
        (
            "missing schema",
            "DROP TABLE ledger_artifacts; DROP TABLE ledger_links; DROP TABLE ledger_events; DROP TABLE ledger_meta",
        ),
    ] {
        let root = TempDir::new().expect("root");
        let database = database(root.path());
        let ledger = GlobalLedger::open_sqlite_candidate(
            config(root.path(), "matrix"),
            Arc::clone(&database),
        )
        .expect("writer");
        ledger.append(draft(1)).expect("first");
        ledger.append(draft(2)).expect("second");
        let mut subscription = ledger
            .subscribe(SubscriptionCursor { after_sequence: 2 })
            .unwrap();
        database
            .connection("mutate assigned matrix")
            .expect("connection")
            .execute_batch(sql)
            .expect("mutate fixture");
        let query_error = ledger
            .project_view_page(
                EventQuery {
                    view: Some(LedgerView::Health),
                    ..EventQuery::default()
                },
                ProjectionProfile::Ui,
                RuntimeEventQueryPageRequest::default(),
            )
            .expect_err("an empty view must still reject a corrupt full snapshot");
        assert!(query_error.is_fatal(), "{label}: {query_error}");
        assert_eq!(
            subscription
                .recv_timeout(Duration::from_secs(1))
                .expect_err("query fatal reaches subscribers"),
            query_error
        );
        ledger
            .close()
            .expect_err("query failure terminates the writer");
        let error =
            GlobalLedger::open_sqlite_candidate(config(root.path(), "matrix-reopen"), database)
                .expect_err(label);
        assert!(error.is_fatal(), "{label}: {error}");
        assert!(
            !format!("{error:?} {error}").contains("token-secret"),
            "{label}: disclosure"
        );
    }
}

#[test]
fn sqlite_unique_constraints_and_transaction_rollback_fail_closed() {
    let root = TempDir::new().expect("root");
    let database = database(root.path());
    let ledger =
        GlobalLedger::open_sqlite_candidate(config(root.path(), "rollback"), Arc::clone(&database))
            .expect("writer");
    let first = ledger.append(draft(1)).expect("first");
    let second = ledger.append(draft(2)).expect("second");
    {
        let connection = database
            .connection("constraint specification")
            .expect("connection");
        connection.execute_batch("INSERT INTO ledger_events SELECT * FROM ledger_events WHERE sequence=-9223372036854775807").expect_err("sequence unique");
        connection.execute_batch("UPDATE ledger_events SET event_id=(SELECT event_id FROM ledger_events WHERE sequence=-9223372036854775807) WHERE sequence=-9223372036854775806").expect_err("EventId unique");
        connection.execute_batch("CREATE TRIGGER reject_ledger_meta BEFORE UPDATE ON ledger_meta BEGIN SELECT RAISE(ABORT, 'sqlite-rollback-fixture'); END;").expect("late transaction failure");
    }
    let mut subscription = ledger
        .subscribe(SubscriptionCursor { after_sequence: 2 })
        .expect("subscriber");
    let error = ledger
        .append(draft(3))
        .expect_err("metadata failure rolls back inserts");
    assert!(error.is_fatal());
    assert_eq!(error.operation(), "update_sqlite_head");
    assert_eq!(
        subscription
            .recv_timeout(Duration::from_secs(1))
            .expect_err("writer fatal notification"),
        error
    );
    ledger.close().expect_err("fatal writer remains failed");
    database
        .connection("release fixture trigger")
        .expect("connection")
        .execute_batch("DROP TRIGGER reject_ledger_meta")
        .expect("remove assigned trigger");
    let raw = read_snapshot(&database, None).expect("durable snapshot after rollback");
    assert_eq!(raw.events.len(), 2);
    assert_eq!(raw.links.len(), 2);
    let (events, _) = verify_snapshot(
        &database,
        raw,
        &mut None::<fn(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>>,
    )
    .expect("unchanged integrity");
    assert_eq!(events, [first, second]);
}

#[test]
fn sqlite_artifact_order_summary_projection_and_verifier_are_preserved() {
    let fixture = TempDir::new().expect("fixture");
    let artifacts =
        actingcommand_artifact_store::ArtifactStore::open(fixture.path().join("objects"))
            .expect("artifact owner");
    let ids = IdentifierIssuer::new().expect("identity issuer");
    let run = ids.mint_run_id().expect("run");
    let frame = ids.mint_frame_id().expect("frame");
    let correlation = ids.mint_correlation_id().expect("correlation");
    struct ArtifactDrafts(Vec<SanitizedEventDraft>);
    impl actingcommand_artifact_store::ArtifactEventSink for ArtifactDrafts {
        fn append(
            &mut self,
            draft: EventDraft,
        ) -> actingcommand_artifact_store::ArtifactStoreResult<()> {
            self.0.push(
                draft
                    .sanitize(
                        &Sha256SecretFingerprinter::new(b"sqlite-artifact-specification")
                            .expect("salt"),
                    )
                    .expect("artifact owner draft"),
            );
            Ok(())
        }
    }
    let mut inputs = ArtifactDrafts(Vec::new());
    let mut stored = Vec::new();
    for bytes in [b"frame-one".as_slice(), b"frame-two".as_slice()] {
        stored.push(
            artifacts
                .put(
                    actingcommand_artifact_store::ArtifactWriteRequest::new(
                        ArtifactKind::CaptureFrame,
                        bytes,
                        actingcommand_artifact_store::ArtifactWriteContext::new(
                            ArtifactLinksDraft::default()
                                .with_run_id(run)
                                .with_frame_id(frame)
                                .with_correlation_id(correlation),
                            EventLinksDraft::default()
                                .with_run_id(run)
                                .with_frame_id(frame)
                                .with_correlation_id(correlation),
                            u64::MAX,
                        ),
                        ArtifactIssuePolicy::new(
                            ArtifactProducer::CaptureStore,
                            RetentionClass::DebugFull,
                            ArtifactRedactionState::Pending,
                        ),
                    ),
                    &mut inputs,
                )
                .expect("official artifact persistence"),
        );
    }
    let first_reference = stored[0].reference().project(true);
    let summary = CaptureSummaryRecord::new(
        1,
        0,
        0,
        1,
        EvidenceCompleteness::Complete,
        vec![CapturePersistedEvidence::new(0, first_reference.clone()).expect("frame")],
        vec![
            CapturePinnedEvidence::new(Some(0), PinnedFrameReason::Terminal, Some(first_reference))
                .expect("pin"),
        ],
    )
    .expect("existing summary shape");
    let event = EventDraft::new(
        ids.mint_event_id().expect("event"),
        u64::MAX,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        EventLinksDraft::default()
            .with_run_id(run)
            .with_frame_id(frame)
            .with_correlation_id(correlation),
        CapturePayloadDraft::summary_committed(summary, AuditInput::new()).into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"sqlite-artifact-specification").expect("salt"))
    .expect("artifact event");
    let sqlite_root = TempDir::new().expect("sqlite");
    let segment_root = TempDir::new().expect("segment");
    let database = database(sqlite_root.path());
    let sqlite = GlobalLedger::open_sqlite_candidate(
        config(sqlite_root.path(), "artifacts"),
        Arc::clone(&database),
    )
    .expect("sqlite");
    let segment = GlobalLedger::open(config(segment_root.path(), "reference")).expect("segment");
    inputs.0.push(event);
    for input in inputs.0 {
        let expected = segment.append(input.clone()).expect("reference event");
        assert_eq!(sqlite.append(input).expect("candidate event"), expected);
    }
    assert_eq!(
        sqlite
            .project(EventQuery::default(), ProjectionProfile::Lab)
            .expect("projection"),
        segment
            .project(EventQuery::default(), ProjectionProfile::Lab)
            .expect("reference projection")
    );
    let (records, _) = verify_snapshot_records(
        &database,
        read_snapshot(&database, None).expect("same SQLite snapshot"),
    )
    .expect("metadata verifies canonical rows and indexes");
    let metadata = records
        .into_iter()
        .map(StoredEventRecord::into_metadata)
        .collect::<Result<Vec<_>, _>>()
        .expect("typed reference metadata");
    let metadata_indexes = EventIndexes::from_events(&metadata);
    let segment_metadata = super::super::read_only::open_metadata(GlobalLedgerReadOnlyConfig::new(
        segment_root.path(),
    ))
    .expect("Segment metadata with original material kept in the artifact owner root");
    let segment_indexes = EventIndexes::from_events(&segment_metadata.events);
    let through_sequence = sqlite.latest_sequence().expect("verified committed head");
    for profile in [ProjectionProfile::Lab, ProjectionProfile::Ui] {
        let request = RuntimeEventQueryPageRequest::default();
        let online = sqlite
            .project_view_page(EventQuery::default(), profile, request.clone())
            .expect("writer view page");
        let scope = LedgerReadScope {
            source: LedgerReadSource::Offline,
            material_read: LedgerMaterialReadState::NotRequested,
            scanned_through_position: through_sequence,
            read_complete: true,
            limits: Vec::new(),
        };
        let page = metadata_indexes
            .project_view_page(
                &metadata,
                &EventQuery::default(),
                profile,
                &request,
                scope.clone(),
                through_sequence.into(),
            )
            .expect("SQLite metadata projection");
        let reference = segment_indexes
            .project_view_page(
                &segment_metadata.events,
                &EventQuery::default(),
                profile,
                &request,
                scope,
                through_sequence.into(),
            )
            .expect("Segment metadata projection");
        assert_eq!(page, reference);
        assert_eq!(page.events(), online.events());
        let projected_references = page
            .events()
            .iter()
            .flat_map(|event| &event.artifacts)
            .collect::<Vec<_>>();
        assert!(!projected_references.is_empty());
        assert!(
            projected_references
                .iter()
                .all(|reference| reference.object_key.is_some()
                    == (profile == ProjectionProfile::Lab))
        );
        assert_eq!(
            page.read_scope().unwrap().material_read,
            LedgerMaterialReadState::NotRequested
        );
    }
    sqlite.close().expect("close sqlite");
    segment.close().expect("close segment");
    let missing = GlobalLedger::open_sqlite_candidate(
        config(sqlite_root.path(), "without-verifier"),
        Arc::clone(&database),
    )
    .expect_err("verifier required");
    assert_eq!(missing.code(), "artifact_store_verification_unavailable");
    let rejected = GlobalLedger::open_sqlite_candidate_with_artifact_verifier(
        config(sqlite_root.path(), "rejecting-verifier"),
        Arc::clone(&database),
        |_| None,
    )
    .expect_err("verifier rejection");
    assert_eq!(rejected.code(), "artifact_store_verification_failed");
    let sqlite = GlobalLedger::open_sqlite_candidate_with_artifact_verifier(
        config(sqlite_root.path(), "verified"),
        Arc::clone(&database),
        |reference| artifacts.verify_recovery_reference(reference).ok(),
    )
    .expect("artifact recovery");
    let segment = GlobalLedger::open_with_artifact_verifier(
        config(segment_root.path(), "verified-reference"),
        |reference| artifacts.verify_recovery_reference(reference).ok(),
    )
    .expect("reference recovery");
    assert_eq!(
        sqlite
            .query(EventQuery::default())
            .expect("candidate facts"),
        segment
            .query(EventQuery::default())
            .expect("reference facts")
    );
    let references = sqlite
        .query(EventQuery::default())
        .expect("restorable facts")
        .iter()
        .flat_map(|event| {
            event
                .artifacts()
                .iter()
                .map(|reference| reference.project(true))
        })
        .collect::<Vec<_>>();
    let restored_root = TempDir::new().expect("external artifact restore root");
    let restored = actingcommand_artifact_store::ArtifactStore::open(restored_root.path())
        .expect("artifact restore owner");
    for reference in &references {
        let verified = restored
            .restore_recovery_reference(
                artifacts.root(),
                reference,
                1024 * 1024,
                Instant::now() + Duration::from_secs(5),
            )
            .expect("restore original identity");
        assert_eq!(&verified.reference().project(true), reference);
        assert_eq!(
            restored
                .verify_recovery_reference(reference)
                .expect("restored bytes"),
            verified
        );
    }
    sqlite.close().expect("close sqlite");
    segment.close().expect("close segment");
    database
        .connection("artifact order mutation")
        .expect("connection")
        .execute(
            "UPDATE ledger_artifacts SET ordinal=?1 WHERE ordinal=?2",
            [encode(2), encode(0)],
        )
        .expect("mutate ordinal");
    assert_eq!(
        verify_snapshot_records(
            &database,
            read_snapshot(&database, None).expect("mutated snapshot")
        )
        .err()
        .expect("metadata preserves artifact index integrity")
        .code(),
        "ledger_index_mismatch"
    );
    let error = GlobalLedger::open_sqlite_candidate_with_artifact_verifier(
        config(sqlite_root.path(), "order-check"),
        database,
        |reference| artifacts.verify_recovery_reference(reference).ok(),
    )
    .expect_err("artifact order must match canonical");
    assert_eq!(error.code(), "ledger_index_mismatch");
    assert!(error.is_fatal());
}

#[test]
fn sqlite_commit_process_child() {
    let Some(root) = std::env::var_os("ACTINGCOMMAND_TEST_REPAIR_ROOT") else {
        return;
    };
    let root = Path::new(&root);
    let ledger =
        GlobalLedger::open_sqlite_candidate(config(root, "sqlite-commit-child"), database(root))
            .expect("child writer");
    ledger
        .append(draft(42_000))
        .expect("append waits at committed boundary");
    fs::write(root.join("append-ack"), b"acknowledged").expect("ack receipt");
    ledger.close().expect("child close");
}

#[test]
fn sqlite_commit_before_notification_replays_after_process_exit() {
    let root = TempDir::new().expect("root");
    let database = database(root.path());
    let ledger = GlobalLedger::open_sqlite_candidate(
        config(root.path(), "commit-prefix"),
        Arc::clone(&database),
    )
    .expect("writer");
    ledger.append(draft(1)).expect("prefix");
    ledger.close().expect("close prefix");
    let ready = root.path().join("commit-ready");
    let mut child = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "global::sqlite::tests::sqlite_commit_process_child",
            "--nocapture",
        ])
        .env("ACTINGCOMMAND_TEST_REPAIR_ROOT", root.path())
        .env("ACTINGCOMMAND_TEST_REPAIR_FAILPOINT", "after_sqlite_commit")
        .env("ACTINGCOMMAND_TEST_REPAIR_READY", &ready)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("commit child");
    crate::global::recovery_tests::wait_for_barrier(&mut child, &ready, "after_sqlite_commit");
    child.kill().expect("kill at committed boundary");
    child.wait().expect("release child");
    let snapshot = GlobalLedger::open_sqlite_candidate_read_only(
        GlobalLedgerReadOnlyConfig::new(root.path()),
        Arc::clone(&database),
        |_| None,
    )
    .expect("committed snapshot after exit before notification");
    assert_eq!(snapshot.events().len(), 2);
    let committed = snapshot.events()[1].clone();
    assert_eq!(committed.timestamp_unix_ms(), 42_000);
    assert!(!root.path().join("append-ack").exists());
    let reopened =
        GlobalLedger::open_sqlite_candidate(config(root.path(), "commit-replay"), database)
            .expect("recover committed facts");
    let mut subscription = reopened
        .subscribe(SubscriptionCursor { after_sequence: 1 })
        .expect("cursor replay");
    assert_eq!(
        subscription
            .recv_timeout(Duration::from_secs(1))
            .expect("committed replay"),
        committed
    );
    let recovered = reopened.query(EventQuery::default()).expect("facts");
    assert_eq!(
        recovered
            .iter()
            .filter(|event| event.event_id() == committed.event_id())
            .count(),
        1
    );
    assert_eq!(
        recovered
            .iter()
            .filter(|event| event.event_type() == EventType::LedgerRecovered)
            .count(),
        1
    );
    reopened.close().expect("close recovered");
}
