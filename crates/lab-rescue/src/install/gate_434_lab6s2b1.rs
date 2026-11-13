// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-6S2-B1 (register §1.287, "Slice LAB-6S2-B1"; contracts/lab-two-entries.md
// §2.4, §4.3; the coordinator's rulings of 2026-11-13 on board #434, not the owner's words): the
// rescue module's own client (`SystemHost`'s Runtime, `SystemRuntime`) binds `lab` with the
// module's word and no `via` and keeps the envelope (Cli, Cli) it sent on 0.12.58, for every kind
// it sends: the configuration facts (`RuntimeFactSnapshot`), the plan and the catalog
// (`ScheduleExplanation`), an instance's and the global pause (`PauseScheduling`) and resume
// (`ResumeScheduling`), the formal close (`RequestShutdown`) and the start report
// (`StartReport`). Without a word, `actingctl install`'s host until LAB-6S2-B2 (which gives it
// `Lab-CLI`), it binds the CLI channel as on 0.12.58. Each kind on a connection of its own, which
// the scripted Runtime (the runtime client's, on loopback in a temporary state root) refuses, so
// that nothing waits; its status read needs a held owner lock and connects to none here.

use super::host::{Host, SystemHost};
use actingcommand_contract::{
    DeclaredInitiator, EventActor, EventSource, RuntimeChannel, RuntimeErrorCode,
    RuntimeErrorProjection, RuntimeOperation, RuntimeReceiptState, RuntimeRequest,
    SchedulingPauseReason,
};
use actingcommand_lab_ops::initiator::Initiator;
use actingcommand_runtime_client::test_support::{
    ScriptedAnswer, ScriptedConnection, ScriptedRuntime,
};
use serde_json::{Value, json};
use tempfile::TempDir;

/// The instance the pause and the resume name.
const ALIAS: &str = "neutral.a";
/// The kinds the module's client sends, in the order [`send_every_kind`] sends them.
const KINDS: [&str; 9] = [
    "runtime_fact_snapshot",
    "schedule_explanation",
    "schedule_explanation",
    "pause_scheduling",
    "resume_scheduling",
    "pause_scheduling",
    "resume_scheduling",
    "request_shutdown",
    "start_report",
];

/// A refusal of the Runtime: denied, `lease_busy`.
fn denied(request: &RuntimeRequest) -> ScriptedAnswer {
    ScriptedAnswer::failed(
        request,
        RuntimeReceiptState::Denied,
        RuntimeErrorProjection::new(RuntimeErrorCode::LeaseBusy, false).receipt_outcome(None, None),
    )
}

/// Every kind of [`KINDS`] through the Runtime of `host`, each refused, on the Runtime whose state
/// root is `state` and whose owner epoch is `owner_epoch` (the text a status read shows).
fn send_every_kind(host: &SystemHost, state: &std::path::Path, owner_epoch: &str) {
    let runtime = host.runtime();
    let reason = SchedulingPauseReason::new("lab-deploy-1").expect("a pause reason");
    assert!(runtime.config(state).is_err(), "configuration facts");
    assert!(runtime.plan(state).is_err(), "plan");
    assert!(runtime.catalog_hash(state).is_err(), "catalog");
    assert!(
        runtime.pause(state, ALIAS, &reason, 60_000).is_err(),
        "instance pause"
    );
    assert!(
        runtime.resume(state, ALIAS, owner_epoch, 1).is_err(),
        "instance resume"
    );
    assert!(
        runtime.pause_global(state, &reason, 60_000).is_err(),
        "global pause"
    );
    assert!(
        runtime.resume_global(state, owner_epoch, 1).is_err(),
        "global resume"
    );
    assert!(runtime.shutdown(state, state, 1_000).is_err(), "close");
    assert!(runtime.start_report(state).is_err(), "start report");
}

#[test]
fn lab6s2b1_rescue_module_client_binds_lab_with_its_word_and_the_cli_envelope() {
    // The module's word (a command line's, and the MCP server's), and `actingctl install`'s none
    // (until LAB-6S2-B2; no product caller declares none since).
    for (initiator, binding) in [
        (
            Some(Initiator::LabCli),
            (
                RuntimeChannel::Lab,
                None,
                Some(actingcommand_contract::Initiator::LabCli),
            ),
        ),
        (
            Some(Initiator::LabMcp),
            (
                RuntimeChannel::Lab,
                None,
                Some(actingcommand_contract::Initiator::LabMcp),
            ),
        ),
        (None, (RuntimeChannel::Cli, None, None)),
    ] {
        let temp = TempDir::new().expect("tempdir");
        let runtime = ScriptedRuntime::start(temp.path(), |_| {
            KINDS
                .iter()
                .map(|_| ScriptedConnection::new().request(denied))
                .collect()
        });
        let Value::String(owner_epoch) =
            serde_json::to_value(runtime.owner_epoch()).expect("the owner epoch encodes")
        else {
            panic!("an owner epoch is text");
        };
        send_every_kind(&SystemHost::of(initiator), temp.path(), &owner_epoch);
        let record = runtime.finish();
        assert!(
            record.unscripted.is_empty(),
            "{initiator:?}: a connection beyond the script"
        );
        assert_eq!(record.connections.len(), KINDS.len(), "{initiator:?}");
        for (connection, kind) in record.connections.iter().zip(KINDS) {
            let operations = connection
                .requests
                .iter()
                .map(|request| json!(request.operation())["operation"].clone())
                .collect::<Vec<_>>();
            assert_eq!(
                operations,
                [json!("health"), json!("bind_channel"), json!(kind)],
                "{initiator:?}"
            );
            let RuntimeOperation::BindChannel {
                channel,
                via,
                initiator: declared,
            } = connection.requests[1].operation()
            else {
                panic!("the second request is no BindChannel");
            };
            assert_eq!(
                (
                    *channel,
                    *via,
                    declared.as_ref().and_then(DeclaredInitiator::initiator)
                ),
                binding,
                "{initiator:?} {kind}"
            );
            // The envelope the module sent on 0.12.58, on every request.
            for request in &connection.requests {
                assert_eq!(
                    (request.actor(), request.source()),
                    (EventActor::Cli, EventSource::Cli),
                    "{initiator:?} {kind}"
                );
            }
        }
    }
}
