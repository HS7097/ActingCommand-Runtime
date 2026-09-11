// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::FactPayload;

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
    let invalidating_catalog = host
        .activate_policy_catalog(&policy_sources(2))
        .expect("activate invalidating catalog");
    let mut client = TestClient::connect(&host);
    let before = projected_events(&mut client, EventQuery::default());
    drop(client);

    let baseline = before
        .iter()
        .map(|event| event.sequence)
        .max()
        .expect("committed catalog baseline");
    let catalog_event = before
        .iter()
        .rev()
        .find(|event| event.event_type == EventType::CatalogActivated)
        .expect("baseline contains catalog activation");
    let ProjectionPayload::Full(payload) = &catalog_event.payload else {
        panic!(
            "catalog payload unavailable: event={:?} sequence={}",
            catalog_event.event_id, catalog_event.sequence
        );
    };
    let EventPayload::Catalog(CatalogPayload::Activated(catalog)) = payload.as_ref() else {
        panic!(
            "catalog payload mismatch: event={:?} sequence={}",
            catalog_event.event_id, catalog_event.sequence
        );
    };
    assert!(
        catalog.catalog_id() == invalidating_catalog.catalog_id()
            && catalog.catalog_version() == invalidating_catalog.catalog_version()
            && catalog.catalog_hash() == invalidating_catalog.catalog_hash(),
        "baseline activation mismatch: event={:?} sequence={} id={:.96} version={} hash={:.80}",
        catalog_event.event_id,
        catalog_event.sequence,
        catalog.catalog_id(),
        catalog.catalog_version(),
        catalog.catalog_hash()
    );
    let invalidations: Vec<_> = before
        .iter()
        .filter(|event| event.event_type == EventType::FactInvalidated)
        .take(2)
        .collect();
    assert_eq!(
        invalidations.len(),
        1,
        "one published fact invalidation required; first two (sequence, id)={:?}; catalog={:?}",
        invalidations
            .iter()
            .map(|event| (event.sequence, event.event_id))
            .collect::<Vec<_>>(),
        catalog_event.event_id
    );
    let invalidation_event = invalidations[0];
    let ProjectionPayload::Full(payload) = &invalidation_event.payload else {
        panic!(
            "invalidation payload unavailable: event={:?} sequence={}",
            invalidation_event.event_id, invalidation_event.sequence
        );
    };
    let EventPayload::Fact(FactPayload::Invalidated(payload)) = payload.as_ref() else {
        panic!(
            "invalidation payload mismatch: event={:?} sequence={}",
            invalidation_event.event_id, invalidation_event.sequence
        );
    };
    let invalidation = payload.invalidation();
    let (scope_kind, scope_id) = match &invalidation.scope {
        FactScope::Instance { instance_id } => ("instance", instance_id),
        FactScope::Server { server_id } => ("server", server_id),
        FactScope::Game { game_id } => ("game", game_id),
    };
    assert!(
        invalidation.scope
            == (FactScope::Instance {
                instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            })
            && invalidation.key == "resource.projection_only"
            && invalidation.source_snapshot_id == "snapshot:forward-read-only"
            && invalidation.invalidated_by_event_id == catalog_event.event_id
            && invalidation.invalidated_by_event_type == EventType::CatalogActivated
            && invalidation_event.sequence > catalog_event.sequence,
        "baseline invalidation mismatch: event={:?} sequence={} scope={scope_kind}/{scope_id:.96} key={:.96} snapshot={:.96} cause={:?}/{:?} catalog={:?}/{} (strings limited to 96 chars)",
        invalidation_event.event_id,
        invalidation_event.sequence,
        invalidation.key,
        invalidation.source_snapshot_id,
        invalidation.invalidated_by_event_id,
        invalidation.invalidated_by_event_type,
        catalog_event.event_id,
        catalog_event.sequence
    );

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
    assert!(
        before == after,
        "projection changed complete event snapshot: baseline={baseline} before_count={} after_count={} last eight after (sequence, id, type, invalidation cause)={:?}",
        before.len(),
        after.len(),
        after
            .iter()
            .rev()
            .take(8)
            .map(|event| {
                let cause = match &event.payload {
                    ProjectionPayload::Full(payload) => match payload.as_ref() {
                        EventPayload::Fact(FactPayload::Invalidated(payload)) => Some((
                            payload.invalidation().invalidated_by_event_id,
                            payload.invalidation().invalidated_by_event_type,
                        )),
                        _ => None,
                    },
                    _ => None,
                };
                (event.sequence, event.event_id, event.event_type, cause)
            })
            .collect::<Vec<_>>()
    );
    assert!(
        after
            .iter()
            .filter(|event| event.sequence > baseline)
            .all(|event| event.event_type != EventType::FactInvalidated),
        "projection appended an invalidation after baseline={baseline}"
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
