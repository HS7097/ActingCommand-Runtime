// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn forward_projection_reuses_policy_state_without_runtime_side_effects() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::clone(&state));
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate policy catalog");
    host.publish_fact(stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        "resource.projection_only",
        ContractFactValue::Boolean(true),
        "snapshot:forward-read-only",
        vec![EventType::CatalogActivated],
    ))
    .expect("publish projection fact");
    host.activate_policy_catalog(&policy_sources(2))
        .expect("activate invalidating catalog");
    let mut client = TestClient::connect(&host);
    let before = projected_events(&mut client, EventQuery::default());
    drop(client);

    let config = ForwardProjectionConfig::for_hours(2, 64).expect("projection config");
    let first = host
        .project_policy_forward(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            17,
            config,
        )
        .expect("forward projection");
    let second = host
        .project_policy_forward(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            17,
            config,
        )
        .expect("replayed forward projection");
    assert_eq!(first, second);
    assert!(!first.steps.is_empty());

    let mut client = TestClient::connect(&host);
    let after = projected_events(&mut client, EventQuery::default());
    assert_eq!(before, after);
    assert!(
        after
            .iter()
            .all(|event| event.event_type != EventType::FactInvalidated)
    );
    assert_eq!(state.open_count.load(Ordering::SeqCst), 0);
    assert_eq!(state.capture_open_count.load(Ordering::SeqCst), 0);
    assert_eq!(state.input_count.load(Ordering::SeqCst), 0);
    drop(client);
    let snapshot = host
        .instance_fact_snapshot(InstanceFactContext {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
        })
        .expect("synchronize fact snapshot");
    assert!(
        snapshot
            .records
            .iter()
            .all(|record| record.key != "resource.projection_only")
    );
    host.close().expect("close host");
}
