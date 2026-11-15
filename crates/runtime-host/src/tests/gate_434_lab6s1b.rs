// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-6S1b (register §1.287, "Slice LAB-6S1b"; contracts/lab-two-entries.md §2.4):
// `RuntimeRequest::validate()` judges no request by its origin. A (Lab, Lab) connection's global
// and instance pause are taken as a (Cli, Cli) connection's are (completed, revision 1, the
// connection's channel on the receipt), held with their reason, recorded so that a restart
// restores both, and resumed from the same envelope. Workflow #434 LAB-6S4a (0.12.63): the host's
// own checks of the governance and installation kinds went with those kinds, and with them this
// gate's test of those checks.

use super::*;
use actingcommand_contract::{
    InstancePauseStage, RuntimeChannel, SchedulingPauseReason, SchedulingPauseScope,
};

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
