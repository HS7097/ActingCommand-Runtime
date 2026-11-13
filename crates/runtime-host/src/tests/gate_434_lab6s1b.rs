// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-6S1b (register §1.287, "Slice LAB-6S1b"; contracts/lab-two-entries.md §2.4):
// `RuntimeRequest::validate()` judges no request by its origin. A (Lab, Lab) connection's global
// and instance pause are taken as a (Cli, Cli) connection's are (completed, revision 1, the
// connection's channel on the receipt), held with their reason, recorded so that a restart
// restores both, and resumed from the same envelope. The host's own checks of the kinds LAB-6S4a
// deletes still refuse what they refused: a governance identity card from an envelope other than
// (User, Ui) or (Cli, Cli) is `invalid_governance_origin`; an approval decision from any envelope
// on a connection whose card was not accepted is `governance_authority_required`; an install
// transition other than its Query on such a connection is `install_identity_required`.

use super::*;
use actingcommand_contract::{
    GovernanceIdentityCard, InstallTransitionAction, InstancePauseStage, RuntimeChannel,
    SchedulingPauseReason, SchedulingPauseScope,
};

const ALIAS: &str = "node.a";
const REASON: &str = "lab6s1b_check";

// A request of `client`'s connection with the envelope `(actor, source)`.
fn request_as(
    client: &TestClient,
    actor: EventActor,
    source: EventSource,
    operation: RuntimeOperation,
) -> RuntimeRequest {
    RuntimeRequest::new(
        client.ids.mint_request_id().expect("request id"),
        client.ids.mint_correlation_id().expect("correlation id"),
        None,
        actor,
        source,
        1,
        operation,
    )
    .expect("runtime request")
}

fn reason() -> SchedulingPauseReason {
    SchedulingPauseReason::new(REASON).expect("pause reason")
}

// Workflow #434 LAB-6S3: binds the connection to `cli`, as actingctl's is. On a connection bound
// to `ui` the door refuses the governance kinds by the UI list before the host's own check is
// reached, and a first request with the console's envelope would bind an unbound connection to
// `ui`. Bound to `cli`, every envelope reaches the host's own check.
fn bind_cli(client: &mut TestClient) {
    let bind = client.request(RuntimeOperation::BindChannel {
        channel: RuntimeChannel::Cli,
        via: None,
        initiator: None,
    });
    let bound = client.send(&bind);
    assert_eq!(bound.state(), RuntimeReceiptState::Completed, "{bound:?}");
}

// The pause and the resume of a (Lab, Lab) connection are taken and recorded as those of a
// (Cli, Cli) connection: the same receipts, the same held pauses, the same restore after a
// restart, the same resume.
#[test]
fn lab6s1b_a_lab_connections_pause_and_resume_are_taken_and_recorded_as_the_clis() {
    for (actor, source, channel) in [
        (EventActor::Cli, EventSource::Cli, RuntimeChannel::Cli),
        (EventActor::Lab, EventSource::Lab, RuntimeChannel::Lab),
    ] {
        let root = TempDir::new().expect("tempdir");
        let registered_id = instance_id();
        let start = |root: &TempDir| {
            RuntimeHost::start(
                config(root),
                Arc::new(FakeProvider::one(
                    POLICY_INSTANCE_ALIAS,
                    registered_id,
                    Arc::new(FakeState::default()),
                )),
            )
            .expect("runtime host")
        };
        let scopes = || {
            [
                SchedulingPauseScope::Global,
                SchedulingPauseScope::Instance {
                    instance_alias: POLICY_INSTANCE_ALIAS.to_owned(),
                },
            ]
        };

        let host = start(&root);
        let mut client = TestClient::connect(&host);
        for scope in scopes() {
            let pause = request_as(
                &client,
                actor,
                source,
                RuntimeOperation::PauseScheduling {
                    scope,
                    pause_reason: reason(),
                    drain_timeout_ms: 1_000,
                },
            );
            let receipt = client.send(&pause);
            assert_eq!(
                receipt.state(),
                RuntimeReceiptState::Completed,
                "{source:?}: {receipt:?}"
            );
            assert_eq!(receipt.channel(), Some(channel), "{source:?}");
            assert!(
                matches!(
                    receipt.result(),
                    Some(RuntimeResult::SchedulingPaused { revision: 1, .. })
                ),
                "{source:?}: {receipt:?}"
            );
        }
        drop(client);
        let (global, instances) = host.scheduling_pauses_for_test().expect("held pauses");
        let global = global.expect("the global pause");
        assert_eq!(
            (global.revision, global.pause_reason.as_str()),
            (1, REASON),
            "{source:?}"
        );
        let instance = instances
            .get(POLICY_INSTANCE_ALIAS)
            .expect("the instance pause");
        assert_eq!(
            (
                instance.revision,
                instance.pause_reason.as_str(),
                instance.stage
            ),
            (1, REASON, InstancePauseStage::Released),
            "{source:?}"
        );
        host.close().expect("close host");

        // Recorded: a restart restores both pauses with their reason.
        let reopened = start(&root);
        assert_eq!(reopened.scheduling_pause_restore().len(), 2, "{source:?}");
        let (global, instances) = reopened
            .scheduling_pauses_for_test()
            .expect("restored pauses");
        assert_eq!(
            global.map(|pause| pause.pause_reason),
            Some(reason()),
            "{source:?}"
        );
        assert_eq!(
            instances
                .get(POLICY_INSTANCE_ALIAS)
                .map(|pause| pause.pause_reason.clone()),
            Some(reason()),
            "{source:?}"
        );

        let mut client = TestClient::connect(&reopened);
        for scope in scopes() {
            let resume = request_as(
                &client,
                actor,
                source,
                RuntimeOperation::ResumeScheduling {
                    scope,
                    expected: None,
                },
            );
            let receipt = client.send(&resume);
            assert_eq!(
                receipt.state(),
                RuntimeReceiptState::Completed,
                "{source:?}: {receipt:?}"
            );
            assert!(
                matches!(
                    receipt.result(),
                    Some(RuntimeResult::SchedulingResumed { .. })
                ),
                "{source:?}: {receipt:?}"
            );
        }
        drop(client);
        let (global, instances) = reopened.scheduling_pauses_for_test().expect("pauses");
        assert!(global.is_none(), "{source:?}");
        assert!(instances.is_empty(), "{source:?}");
        reopened.close().expect("close reopened host");
    }
}

// The host's own checks of the governance and installation kinds, which `validate()` no longer
// repeats, still refuse: the card from the Lab, an agent or the console's read envelope; an
// approval decision from every envelope on a connection without an accepted card; and an install
// transition's drain from the Lab, whose connection declared no card. The card's and the
// approval's connections are bound to `cli` (Workflow #434 LAB-6S3: on `ui` the door refuses
// both kinds first, `channel_request_refused`, which that slice's own gate holds).
#[test]
fn lab6s1b_the_hosts_own_checks_of_the_governance_and_install_kinds_still_refuse() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, ALIAS, Arc::new(FakeState::default()));

    for (actor, source) in [
        (EventActor::Lab, EventSource::Lab),
        (EventActor::Agent, EventSource::Adapter),
        (EventActor::Ui, EventSource::Ui),
    ] {
        let mut client = TestClient::connect(&host);
        bind_cli(&mut client);
        let card = request_as(
            &client,
            actor,
            source,
            RuntimeOperation::DeclareGovernanceIdentity {
                card: GovernanceIdentityCard {
                    client: TEST_GOVERNANCE_CLIENT.to_owned(),
                    client_version: None,
                    instance: None,
                },
            },
        );
        let refused = client.send(&card);
        assert_eq!(
            refused.state(),
            RuntimeReceiptState::Denied,
            "{source:?}: {refused:?}"
        );
        assert_eq!(
            refused.outcome().code().as_str(),
            "invalid_governance_origin",
            "{source:?}"
        );
    }

    for (actor, source) in [
        (EventActor::Lab, EventSource::Lab),
        (EventActor::Agent, EventSource::Adapter),
        (EventActor::Cli, EventSource::Cli),
        (EventActor::User, EventSource::Ui),
    ] {
        let mut client = TestClient::connect(&host);
        bind_cli(&mut client);
        let decision = ApprovalDecisionRecord::new(
            "approval:fixture-a",
            ApprovalDisposition::Approved,
            ApprovalTarget::Catalog {
                catalog_hash: format!("sha256:{}", "a".repeat(64)),
                catalog_version: 1,
            },
            actingcommand_contract::ApprovalReasonCode::new("user_confirmed")
                .expect("approval reason"),
        )
        .expect("approval decision");
        let approval = request_as(
            &client,
            actor,
            source,
            RuntimeOperation::RecordApprovalDecision { decision },
        );
        let refused = client.send(&approval);
        assert_eq!(
            refused.state(),
            RuntimeReceiptState::Denied,
            "{source:?}: {refused:?}"
        );
        assert_eq!(
            refused.outcome().code().as_str(),
            "governance_authority_required",
            "{source:?}"
        );
    }

    let mut client = TestClient::connect(&host);
    let drain = request_as(
        &client,
        EventActor::Lab,
        EventSource::Lab,
        RuntimeOperation::InstallTransition {
            target: host.runtime_info().shutdown_target(),
            action: InstallTransitionAction::BeginDrain {
                transition_id: "lab6s1b-check".to_owned(),
                timeout_ms: 60_000,
            },
        },
    );
    let refused = client.send(&drain);
    assert_eq!(refused.state(), RuntimeReceiptState::Denied, "{refused:?}");
    assert_eq!(
        refused.outcome().code().as_str(),
        "install_identity_required"
    );
    drop(client);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
