// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn release_sets_switch_atomically_rollback_and_recover_without_duplicate_events() {
    let root = TempDir::new().expect("tempdir");
    let runtime_instance_id = instance_id();
    let (first, first_sources) = release_set(root.path(), "1.0.0", 'a');
    let (second, second_sources) = release_set(root.path(), "2.0.0", 'b');
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "neutral-release",
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");

    assert_eq!(
        host.stage_release_set(first.clone(), &first_sources)
            .expect("stage first"),
        first
    );
    host.stage_release_set(second.clone(), &second_sources)
        .expect("stage second");
    assert_eq!(
        host.activate_release_set(first.release_id())
            .expect("activate first"),
        first
    );
    assert_eq!(
        host.activate_release_set(second.release_id())
            .expect("activate second"),
        second
    );
    assert_eq!(
        host.rollback_release_set(first.release_id())
            .expect("rollback first"),
        first
    );
    assert_eq!(
        host.active_release_set().expect("active release"),
        Some(first.clone())
    );
    let mut client = TestClient::connect(&host);
    let events = projected_events(&mut client, EventQuery::default());
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::ReleaseStaged)
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::ReleaseActivated)
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::ReleaseRolledBack)
            .count(),
        1
    );
    drop(client);
    host.close().expect("close host");

    let reopened = RuntimeHost::start(
        RuntimeHostConfig::new(root.path(), b"rotated-fingerprint-salt"),
        Arc::new(FakeProvider::one(
            "neutral-release",
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("reopen runtime host");
    assert_eq!(
        reopened.active_release_set().expect("recovered release"),
        Some(first)
    );
    let mut client = TestClient::connect(&reopened);
    let events = projected_events(&mut client, EventQuery::default());
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::ReleaseStaged)
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                matches!(
                    event.event_type,
                    EventType::ReleaseActivated | EventType::ReleaseRolledBack
                )
            })
            .count(),
        3
    );
    drop(client);
    reopened.close().expect("close reopened host");
}

#[test]
fn committed_release_without_ledger_outcome_is_reconciled_on_restart() {
    let root = TempDir::new().expect("tempdir");
    let runtime_instance_id = instance_id();
    let (release, sources) = release_set(root.path(), "1.0.0", 'c');
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "neutral-release",
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.stage_release_set(release.clone(), &sources)
        .expect("stage release");
    host.close().expect("close host");

    let state =
        RuntimeStateStore::open(root.path(), b"different-bootstrap-seed").expect("runtime state");
    let preview = state
        .preview_release_transition(ReleaseTransitionKind::Activate, release.release_id())
        .expect("transition preview");
    state
        .commit_release_transition(&preview)
        .expect("commit transition without ledger outcome");
    drop(state);

    let reopened = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "neutral-release",
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("reopen runtime host");
    assert_eq!(
        reopened.active_release_set().expect("active release"),
        Some(release)
    );
    let mut client = TestClient::connect(&reopened);
    let events = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::ReleaseActivated),
            ..EventQuery::default()
        },
    );
    assert_eq!(events.len(), 1);
    let ProjectionPayload::Full(payload) = &events[0].payload else {
        panic!("expected forensic release payload")
    };
    let EventPayload::Release(ReleasePayload::Activated(payload)) = payload.as_ref() else {
        panic!("expected release activation")
    };
    assert_eq!(
        payload.transition().validation_result(),
        StateValidationResult::Recovered
    );
    assert_eq!(
        payload.transition().recovery_action(),
        StateRecoveryAction::ReplayedCommitted
    );
    drop(client);
    reopened.close().expect("close reopened host");
}

#[test]
fn legacy_catalog_pointer_migrates_once_into_authoritative_state() {
    let root = TempDir::new().expect("tempdir");
    let runtime_instance_id = instance_id();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "neutral-release",
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    let catalog = host
        .activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    host.close().expect("close host");

    let database =
        actingcommand_runtime_database::RuntimeDatabase::open_existing(root.path(), false)
            .expect("shared database");
    database
        .connection("prepare legacy catalog pointer in existing specification")
        .expect("connection")
        .execute_batch(
            "BEGIN IMMEDIATE;
             DELETE FROM state_documents WHERE state_key='policy.catalog.active';
             DELETE FROM state_document_history WHERE state_key='policy.catalog.active';
             COMMIT;",
        )
        .expect("prepare absent catalog pointer without changing ledger or key");
    drop(database);
    let legacy = root
        .path()
        .join("policy")
        .join("catalogs")
        .join("active.json");
    fs::write(
        &legacy,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": "actingcommand.catalog-state.v1",
            "generation": catalog,
        }))
        .expect("legacy pointer"),
    )
    .expect("write legacy pointer");

    let reopened = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "neutral-release",
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("reopen runtime host");
    assert_eq!(
        reopened
            .active_policy_catalog()
            .expect("active catalog")
            .expect("catalog")
            .catalog_hash(),
        catalog.catalog_hash()
    );
    assert!(!legacy.exists());
    let mut client = TestClient::connect(&reopened);
    let events = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::StateMigrated),
            ..EventQuery::default()
        },
    );
    assert_eq!(events.len(), 1);
    let ProjectionPayload::Full(payload) = &events[0].payload else {
        panic!("expected forensic state payload")
    };
    let EventPayload::State(StatePayload::Migrated(payload)) = payload.as_ref() else {
        panic!("expected state migration")
    };
    assert_eq!(payload.migration().state_key(), "policy.catalog.active");
    drop(client);
    reopened.close().expect("close reopened host");
}
