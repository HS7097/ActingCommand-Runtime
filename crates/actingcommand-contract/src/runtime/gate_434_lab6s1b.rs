// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-6S1b (register §1.287, "Slice LAB-6S1b"; contracts/lab-two-entries.md §2.4):
// `RuntimeRequest::validate()` judges no request by its origin. For every family of origin gate
// the function held before this slice (shutdown, install transition, authoring, signatures,
// debugging, governance, emulator control and its discovery, self-check and drill, the
// scheduling pause, the agent wake test, fact publication by family, the agent dispatcher's
// kinds, the player hold), a request from each envelope of the client set, the ones each gate
// refused among them, is built, encoded, decoded and validated. An envelope outside the client
// set is still `invalid_client_origin`, and an observation that mixes two fact key families is
// still `fact_origin_mixed`, from every envelope: the shape rule of the envelope and the content
// rule stay.

use super::*;
use serde_json::Value;

const ALIAS: &str = "node.a";

/// Every envelope the client set holds that a deleted gate refused for some kind, and the
/// envelopes it took: the person, the operator, the Lab, the agent dispatcher, the console's
/// read envelope and a mixed one.
const CLIENT_ENVELOPES: [(EventActor, EventSource); 6] = [
    (EventActor::User, EventSource::Ui),
    (EventActor::Cli, EventSource::Cli),
    (EventActor::Lab, EventSource::Lab),
    (EventActor::Agent, EventSource::Adapter),
    (EventActor::Ui, EventSource::Ui),
    (EventActor::Cli, EventSource::Lab),
];

/// Envelopes outside the client set (`valid_client_origin`).
const OUTSIDE_ENVELOPES: [(EventActor, EventSource); 5] = [
    (EventActor::System, EventSource::System),
    (EventActor::Runtime, EventSource::Runtime),
    (EventActor::Scheduler, EventSource::Scheduler),
    (EventActor::Cli, EventSource::Device),
    (EventActor::Runtime, EventSource::Cli),
];

/// An instance-information fact (no origin exception) of the server `fixture-server-a`.
fn information_record() -> crate::FactRecord {
    crate::FactRecord {
        scope: crate::FactScope::Server {
            server_id: "fixture-server-a".to_owned(),
        },
        key: "env.ui_theme".to_owned(),
        content: crate::FactContent::Inline {
            value: crate::FactValue::String("Neutral".to_owned()),
        },
        observed_at_unix_ms: 1,
        expires_at_unix_ms: None,
        ttl_policy: None,
        confidence_milli: 1_000,
        source_detector: "detector.fixture".to_owned(),
        source_snapshot_id: "snapshot:lab6s1b".to_owned(),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "a".repeat(64),
        invalidate_on: Vec::new(),
    }
}

/// A manual priority offset (a family with the origin exception) of the same server.
fn offset_record() -> crate::FactRecord {
    crate::FactRecord::priority_offset(
        crate::FactScope::Server {
            server_id: "fixture-server-a".to_owned(),
        },
        "task.a",
        1_000,
        1,
        "detector.fixture",
    )
    .expect("priority offset record")
}

/// One request kind of each family of deleted origin gate, named by the gate that refused it.
fn operations(ids: &IdentifierIssuer) -> Vec<(&'static str, RuntimeOperation)> {
    let target = RuntimeShutdownTarget {
        owner_epoch: *ids.mint_owner_epoch().expect("owner epoch").transport(),
        pid: 1,
        started_at_unix_ms: 1,
    };
    let approval = crate::ApprovalDecisionRecord::new(
        "approval:fixture-a",
        crate::ApprovalDisposition::Approved,
        crate::ApprovalTarget::Catalog {
            catalog_hash: format!("sha256:{}", "a".repeat(64)),
            catalog_version: 1,
        },
        crate::ApprovalReasonCode::new("user_confirmed").expect("approval reason"),
    )
    .expect("approval decision");
    vec![
        (
            "invalid_shutdown_origin",
            RuntimeOperation::RequestShutdown { target },
        ),
        (
            "invalid_install_transition_origin",
            RuntimeOperation::InstallTransition {
                target,
                action: crate::InstallTransitionAction::Query {
                    transition_id: "lab6s1b-check".to_owned(),
                },
            },
        ),
        (
            "invalid_resource_authoring_origin",
            RuntimeOperation::RecordAuthoringEvent {
                event: crate::ResourceAuthoringEvent::new(
                    crate::ResourceAuthoringPhase::PromoteIntent,
                    "draft-a",
                    "resource-root",
                    "b".repeat(64),
                    vec!["operations/task-a/task.json".to_owned()],
                    None,
                )
                .expect("authoring event"),
            },
        ),
        (
            "invalid_signature_origin",
            RuntimeOperation::RetireDiagnosticSignature {
                registration: crate::SignatureRegistrationRef {
                    signature_id: "close_unconfirmed".to_owned(),
                    version: 1,
                    event_id: *ids.mint_event_id().expect("event id").transport(),
                    sequence: 1,
                },
            },
        ),
        (
            "invalid_runtime_debug_origin",
            RuntimeOperation::RecordDebugEvent {
                event: RuntimeDebugEvent::requested(RuntimeDebugOperation::Observe),
            },
        ),
        (
            "invalid_runtime_debug_origin",
            RuntimeOperation::ReleaseLabPin {
                target: crate::LabPinReleaseTarget {
                    artifact_id: *ids.mint_artifact_id().expect("artifact id").transport(),
                    pin: TerminalEvent {
                        event_id: *ids.mint_event_id().expect("pin").transport(),
                        sequence: 1,
                    },
                },
            },
        ),
        (
            "invalid_runtime_debug_origin",
            RuntimeOperation::DebugPackage {
                request: PackageDebugRequest::new("C:/sealed/neutral-task.zip")
                    .expect("debug request"),
            },
        ),
        (
            "invalid_runtime_debug_origin",
            RuntimeOperation::ExportEvidence {
                request: RuntimeEvidenceExportRequest::new(
                    "C:/sealed/neutral-evidence.zip",
                    TaskOutcome::Success,
                )
                .expect("evidence request"),
            },
        ),
        (
            "invalid_governance_origin",
            RuntimeOperation::DeclareGovernanceIdentity {
                card: GovernanceIdentityCard {
                    client: "neutral.client".to_owned(),
                    client_version: None,
                    instance: None,
                },
            },
        ),
        (
            "invalid_governance_origin",
            RuntimeOperation::RecordApprovalDecision { decision: approval },
        ),
        (
            "invalid_emulator_control_origin",
            RuntimeOperation::ControlEmulatorInstance {
                instance_alias: ALIAS.to_owned(),
                action: EmulatorInstanceAction::Restart,
            },
        ),
        (
            "invalid_emulator_control_origin",
            RuntimeOperation::DiscoverInstances,
        ),
        (
            "invalid_emulator_control_origin",
            RuntimeOperation::SelfCheckInstance {
                instance_alias: ALIAS.to_owned(),
            },
        ),
        (
            "invalid_emulator_control_origin",
            RuntimeOperation::DrillRecoveryLadder {
                instance_alias: ALIAS.to_owned(),
                mode: crate::RecoveryDrillMode::Plan,
            },
        ),
        (
            "invalid_scheduling_pause_origin",
            RuntimeOperation::PauseScheduling {
                scope: SchedulingPauseScope::Instance {
                    instance_alias: ALIAS.to_owned(),
                },
                pause_reason: SchedulingPauseReason::new("lab_check").expect("pause reason"),
                drain_timeout_ms: 1_000,
            },
        ),
        (
            "invalid_scheduling_pause_origin",
            RuntimeOperation::ResumeScheduling {
                scope: SchedulingPauseScope::Global,
                expected: None,
            },
        ),
        (
            "invalid_agent_wake_test_origin",
            RuntimeOperation::AgentWakeTest {
                to: "neutral".to_owned(),
                instance_alias: None,
            },
        ),
        (
            "invalid_agent_dispatcher_origin (the origin exception)",
            RuntimeOperation::PublishFacts {
                observation: crate::FactObservation {
                    records: vec![offset_record()],
                },
            },
        ),
        (
            "invalid_agent_dispatcher_origin (instance information)",
            RuntimeOperation::PublishFact {
                record: information_record(),
            },
        ),
        (
            "invalid_agent_dispatcher_origin",
            RuntimeOperation::StartAgentSession {
                wake_id: *ids.mint_agent_wake_id().expect("wake id").transport(),
            },
        ),
        (
            "invalid_agent_dispatcher_origin",
            RuntimeOperation::ProjectPolicyInputIdentity {
                as_of_ledger_position: 17,
            },
        ),
        (
            "invalid_agent_dispatcher_origin",
            RuntimeOperation::ApplyResourceTargets {
                document_json: "{}".to_owned(),
                expected_version: None,
            },
        ),
        (
            "invalid_player_hold_origin",
            RuntimeOperation::AcquirePlayerHold {
                instance_alias: ALIAS.to_owned(),
                drain_timeout_ms: 60_000,
            },
        ),
        (
            "invalid_player_hold_origin",
            RuntimeOperation::ReleasePlayerHold {
                instance_alias: ALIAS.to_owned(),
                expected: PlayerHoldExpectation {
                    owner_epoch: target.owner_epoch,
                    hold_id: *ids.mint_request_id().expect("hold id").transport(),
                    revision: 1,
                },
                startup_package: None,
            },
        ),
    ]
}

/// `operation` from `(actor, source)`, as `RuntimeRequest::new` builds it.
fn request(
    ids: &IdentifierIssuer,
    actor: EventActor,
    source: EventSource,
    operation: RuntimeOperation,
) -> RuntimeContractResult<RuntimeRequest> {
    RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        actor,
        source,
        1,
        operation,
    )
}

// Every family of deleted origin gate takes every envelope of the client set, the ones its gate
// refused included: the request is built (which validates it), and what a peer reads back off the
// wire validates as well.
#[test]
fn lab6s1b_every_family_of_a_deleted_origin_gate_takes_every_client_envelope() {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    for (gate, operation) in operations(&ids) {
        for (actor, source) in CLIENT_ENVELOPES {
            let sent = request(&ids, actor, source, operation.clone()).unwrap_or_else(|error| {
                panic!("{gate}: ({actor:?}, {source:?}) is refused: {error}")
            });
            let wire: Value = serde_json::to_value(&sent).expect("request JSON");
            let received: RuntimeRequest =
                serde_json::from_value(wire).expect("the request decodes");
            assert_eq!(received, sent, "{gate}: ({actor:?}, {source:?})");
            if let Err(error) = received.validate() {
                panic!("{gate}: ({actor:?}, {source:?}) read back is refused: {error}");
            }
        }
    }
}

// An envelope outside the client set is still refused `invalid_client_origin`, whatever the kind.
#[test]
fn lab6s1b_an_envelope_outside_the_client_set_is_still_refused() {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    for (gate, operation) in operations(&ids) {
        for (actor, source) in OUTSIDE_ENVELOPES {
            match request(&ids, actor, source, operation.clone()) {
                Ok(_) => panic!("{gate}: ({actor:?}, {source:?}) is taken"),
                Err(error) => assert_eq!(
                    error.code().as_str(),
                    "invalid_client_origin",
                    "{gate}: ({actor:?}, {source:?})"
                ),
            }
        }
    }
}

// An observation whose keys belong to two families (a priority offset and an instance
// information key) is still refused `fact_origin_mixed`, from every envelope of the client set;
// each of its records alone is taken (the first test).
#[test]
fn lab6s1b_a_fact_observation_that_mixes_two_families_is_still_refused() {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let mixed = RuntimeOperation::PublishFacts {
        observation: crate::FactObservation {
            records: vec![offset_record(), information_record()],
        },
    };
    for (actor, source) in CLIENT_ENVELOPES {
        match request(&ids, actor, source, mixed.clone()) {
            Ok(_) => panic!("({actor:?}, {source:?}): a mixed observation is taken"),
            Err(error) => assert_eq!(
                error.code().as_str(),
                "fact_origin_mixed",
                "({actor:?}, {source:?})"
            ),
        }
    }
}
