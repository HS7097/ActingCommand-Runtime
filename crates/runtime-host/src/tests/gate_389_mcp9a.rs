// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #389 MCP-9a (register A5; MCP-0 command table v1.1 §4.10): the program fact
// `config.catalog` names the active policy catalog generation: its id, version and hash, the
// event id of the `catalog.activated` or `catalog.rolled_back` record that made it active, the
// latest approval decision whose target is that generation, and the generation that record
// replaced. Before any generation is active there is no such fact. It follows each committed
// activation and each approval decision, and a start records it again for the generation it
// restored (the `config.` facts are re-observed at every start).

use super::*;
use actingcommand_contract::{
    CONFIG_CATALOG_FACT_KEY, CatalogPayload, ConfigCatalog, RuntimeFactScope,
};

// The `config.catalog` fact the host holds, read back; none when it holds none.
fn config_catalog(host: &RuntimeHost) -> Option<ConfigCatalog> {
    host.runtime_fact_snapshot()
        .expect("read the program facts")
        .records
        .iter()
        .find(|record| {
            record.scope == RuntimeFactScope::Runtime && record.key == CONFIG_CATALOG_FACT_KEY
        })
        .map(|record| ConfigCatalog::from_fact_record(record).expect("config.catalog reads back"))
}

// The event id, as text, of the last `event_type` record that names `catalog_hash`.
fn transition_event_id(host: &RuntimeHost, event_type: EventType, catalog_hash: &str) -> String {
    let events = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(event_type),
            ..EventQuery::default()
        })
        .expect("read the catalog records");
    let event = events
        .iter()
        .rev()
        .find(|event| {
            matches!(
                event.payload(),
                EventPayload::Catalog(
                    CatalogPayload::Activated(payload) | CatalogPayload::RolledBack(payload)
                ) if payload.catalog_hash() == catalog_hash
            )
        })
        .expect("the record that made the generation active");
    match serde_json::to_value(event.event_id()).expect("an event id serialises") {
        serde_json::Value::String(text) => text,
        other => panic!("an event id is text: {other}"),
    }
}

// A person's approval decision about `generation`, through a governance connection.
fn decide(host: &RuntimeHost, generation: &CatalogGeneration, disposition: ApprovalDisposition) {
    let mut client = TestClient::connect(host);
    client.declare_governance_identity();
    let request = client.governance_request(RuntimeOperation::RecordApprovalDecision {
        decision: ApprovalDecisionRecord::new(
            "approval:fixture-a",
            disposition,
            ApprovalTarget::Catalog {
                catalog_hash: generation.catalog_hash().to_owned(),
                catalog_version: generation.catalog_version(),
            },
            actingcommand_contract::ApprovalReasonCode::new(
                if disposition == ApprovalDisposition::Revoked {
                    "user_revoked"
                } else {
                    "user_confirmed"
                },
            )
            .expect("approval reason"),
        )
        .expect("approval decision"),
    });
    assert_eq!(
        client.send(&request).state(),
        RuntimeReceiptState::Completed
    );
}

#[test]
fn mcp9a_config_catalog_names_the_active_generation_its_record_approval_and_predecessor() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    assert_eq!(config_catalog(&host), None, "no generation is active yet");

    // The first generation: its own record, no approval, no predecessor.
    let first = host
        .activate_policy_catalog(&budget_policy_sources(1))
        .expect("activate the first generation");
    let first_record =
        transition_event_id(&host, EventType::CatalogActivated, first.catalog_hash());
    let expected_first = ConfigCatalog {
        catalog_id: first.catalog_id().to_owned(),
        catalog_version: first.catalog_version(),
        catalog_hash: first.catalog_hash().to_owned(),
        activated_event_id: first_record,
        approval_id: None,
        approval_disposition: None,
        previous_catalog_id: None,
        previous_catalog_version: None,
    };
    assert_eq!(config_catalog(&host), Some(expected_first.clone()));

    // An approval decision about it, then its revocation: the latest decision is named.
    decide(&host, &first, ApprovalDisposition::Approved);
    let approved = ConfigCatalog {
        approval_id: Some("approval:fixture-a".to_owned()),
        approval_disposition: Some(ApprovalDisposition::Approved),
        ..expected_first.clone()
    };
    assert_eq!(config_catalog(&host), Some(approved));
    decide(&host, &first, ApprovalDisposition::Revoked);
    let revoked = ConfigCatalog {
        approval_id: Some("approval:fixture-a".to_owned()),
        approval_disposition: Some(ApprovalDisposition::Revoked),
        ..expected_first
    };
    assert_eq!(config_catalog(&host), Some(revoked));

    // A later version replaces it: its own record, no approval of its own, the first as its
    // predecessor.
    let second = host
        .activate_policy_catalog(&budget_policy_sources(2))
        .expect("activate the second generation");
    let expected_second = ConfigCatalog {
        catalog_id: second.catalog_id().to_owned(),
        catalog_version: second.catalog_version(),
        catalog_hash: second.catalog_hash().to_owned(),
        activated_event_id: transition_event_id(
            &host,
            EventType::CatalogActivated,
            second.catalog_hash(),
        ),
        approval_id: None,
        approval_disposition: None,
        previous_catalog_id: Some(first.catalog_id().to_owned()),
        previous_catalog_version: Some(first.catalog_version()),
    };
    assert_eq!(config_catalog(&host), Some(expected_second.clone()));
    host.close().expect("close the host");

    // A start records it again for the generation it restored.
    let reopened = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    assert_eq!(config_catalog(&reopened), Some(expected_second));
    reopened.close().expect("close the reopened host");
}
