// SPDX-License-Identifier: AGPL-3.0-only

//! #411 P51 (board #411 6437869880; production 10-27: the start of v0.12.18 wrote 1 651
//! `prior_epoch_scope_closed` records, 1 559 of them for the request scopes of the
//! `client.action` records MCP's `ac_lab_observe` had left on two idle instances, one about every
//! 14 seconds): a client action records only itself, so the scope it opens holds no resource and
//! no capture, and the next start closes none of them.
//!
//! The test records N client actions of an instance, as MCP records one after each Lab
//! observation, closes the Runtime and starts it again on the same state root. For N = 1 and
//! N = 4 alike, none of the closes that start writes for the earlier epoch is a client action's:
//! the closes do not grow with N. The start's other closes come from records other than the
//! client actions, which these do not change.

use super::gate_epoch_close::{ALIAS, epoch_config, start_host_with};
use super::*;
use actingcommand_contract::{PriorEpochCloseFact, RuntimePayload};

/// The closes the start after an epoch of `count` client actions of the instance wrote for the
/// scopes of those client actions.
fn client_action_closes(count: usize) -> usize {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let host = start_host_with(epoch_config(root.path()), instance);
    let epoch = host.runtime_info().owner_epoch();
    let mut client = TestClient::connect(&host);
    let mut requests = BTreeSet::new();
    for _ in 0..count {
        let request = client.request(RuntimeOperation::RecordClientAction {
            action: ClientActionRecord::new(
                "mcp",
                "ac_lab_observe",
                ClientActionKind::Command,
                Some(ALIAS.to_owned()),
                None,
            )
            .expect("client action"),
        });
        requests.insert(request.request_id());
        assert!(
            matches!(
                client.send(&request).result(),
                Some(RuntimeResult::ClientActionRecorded)
            ),
            "the client action is recorded"
        );
    }
    drop(client);
    host.close().expect("close the Runtime");

    let host = start_host_with(epoch_config(root.path()), instance);
    assert!(host.fatal_error().expect("runtime health").is_none());
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    host.close().expect("close the restarted Runtime");

    // Each client action is in the earlier epoch's ledger with its instance and its own request,
    // the scope a start would close.
    let recorded = events
        .iter()
        .filter(|event| {
            event.event_type() == EventType::ClientAction
                && event.links().instance_id() == Some(&instance)
                && event.links().run_id().is_none()
                && event
                    .links()
                    .request_id()
                    .is_some_and(|request| requests.contains(request))
        })
        .count();
    assert_eq!(recorded, count, "the client actions of the instance");
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::LifecycleObserved(payload)) => {
                match payload.prior_epoch_close() {
                    Some(PriorEpochCloseFact::ScopeClosed(close))
                        if close.subject.owner == epoch =>
                    {
                        Some(close.subject)
                    }
                    _ => None,
                }
            }
            _ => None,
        })
        .filter(|subject| {
            subject
                .request
                .is_some_and(|request| requests.contains(&request))
        })
        .count()
}

/// #411 P51: N client actions of an instance in one epoch, then a restart: the next start writes
/// no close for their scopes, for N = 1 and N = 4 alike.
#[test]
fn p51_the_next_start_closes_no_scope_of_an_epochs_client_actions() {
    for count in [1, 4] {
        assert_eq!(
            client_action_closes(count),
            0,
            "the next start closed client action scopes after {count} client actions"
        );
    }
}
