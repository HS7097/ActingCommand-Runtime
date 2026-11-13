// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-6S2-B1 (register §1.287, "Slice LAB-6S2-B1"; contracts/lab-two-entries.md
// §4.1, §4.2; the coordinator's rulings of 2026-11-13 on board #434, not the owner's words): the
// watchdog's probe (`observe::live_owner`) takes a word and binds `lab` with it and no `via`,
// keeps the envelope (Cli, Cli) it sent on 0.12.58 and still sends only `Health` and
// `BindChannel`; without a word, as actingctl's callers probed until LAB-6S2-B2 (which gives them
// `Lab-CLI`), it binds the CLI channel as on 0.12.58. A Runtime that cannot decode a declared word (one before LAB-6S1a) drops
// the binding: the probe reconnects once, goes on unbound, is answered, and the runtime client's
// one report of the process, `client_channel_unsupported`, is made once. Fakes only: the runtime
// client's scripted Runtime on loopback in a temporary state root; no clock, no sleep.

use super::observe;
use actingcommand_contract::{
    ChannelVia, DeclaredInitiator, EventActor, EventSource, RuntimeChannel, RuntimeOperation,
    RuntimeRequest,
};
use actingcommand_lab_ops::initiator::Initiator;
use actingcommand_runtime_client::test_support::{
    ScriptedConnection, ScriptedConnectionRecord, ScriptedRuntime,
};
use std::sync::Mutex;
use tempfile::TempDir;

/// A binding as a `BindChannel` declares it: the channel, `via` and the word.
type Declared = (
    RuntimeChannel,
    Option<ChannelVia>,
    Option<actingcommand_contract::Initiator>,
);

/// The binding the received `request`, a `BindChannel`, declares.
fn declared(request: &RuntimeRequest) -> Declared {
    let RuntimeOperation::BindChannel {
        channel,
        via,
        initiator,
    } = request.operation()
    else {
        panic!("BindChannel, got {:?}", request.operation());
    };
    (
        *channel,
        *via,
        initiator.as_ref().and_then(DeclaredInitiator::initiator),
    )
}

/// Whether `request` is a `Health`.
fn health(request: &RuntimeRequest) -> bool {
    matches!(request.operation(), RuntimeOperation::Health)
}

/// Asserts that every request of `record` carries the probe's envelope of 0.12.58, (Cli, Cli).
fn assert_cli_envelope(record: &ScriptedConnectionRecord) {
    for request in &record.requests {
        assert_eq!(
            (request.actor(), request.source()),
            (EventActor::Cli, EventSource::Cli),
            "{:?}",
            request.operation()
        );
    }
}

#[test]
fn lab6s2b1_watchdog_probe_binds_lab_with_its_word_and_sends_health_and_bind_only() {
    // actingwatch's word, a Lab program's (the rescue module's `alive` from a command line), and
    // none (actingctl's until LAB-6S2-B2; no product caller declares none since).
    for (initiator, binding) in [
        (
            Some(Initiator::LabWatchdog),
            (
                RuntimeChannel::Lab,
                None,
                Some(actingcommand_contract::Initiator::LabWatchdog),
            ),
        ),
        (
            Some(Initiator::LabCli),
            (
                RuntimeChannel::Lab,
                None,
                Some(actingcommand_contract::Initiator::LabCli),
            ),
        ),
        (None, (RuntimeChannel::Cli, None, None)),
    ] {
        let temp = TempDir::new().expect("tempdir");
        let runtime = ScriptedRuntime::start(temp.path(), |_| vec![ScriptedConnection::new()]);
        observe::live_owner(temp.path(), None, initiator)
            .unwrap_or_else(|failure| panic!("{initiator:?}: the owner answers: {failure}"));
        let record = runtime.finish();
        assert!(record.unscripted.is_empty(), "{initiator:?}: a reconnect");
        let [connection] = record.connections.as_slice() else {
            panic!("{initiator:?}: one connection");
        };
        // `Health` and `BindChannel`, nothing else.
        assert_eq!(connection.requests.len(), 2, "{initiator:?}");
        assert!(health(&connection.requests[0]), "{initiator:?}");
        assert_eq!(declared(&connection.requests[1]), binding, "{initiator:?}");
        assert_cli_envelope(connection);
    }
}

/// The codes this test binary's reporter of the runtime client received.
static REPORTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[test]
fn lab6s2b1_a_runtime_that_drops_the_word_leaves_the_probe_unbound_once() {
    // The test binary's reporter, as actingwatch installs its own (no other test here does).
    assert!(
        actingcommand_runtime_client::set_channel_unsupported_reporter(|outcome| {
            REPORTS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(outcome.code().as_str().to_owned());
        })
    );
    // Two probes, each against a Runtime that drops the word: the first is reported, the second
    // not, as the runtime client reports once per process.
    for probe in 0..2 {
        let temp = TempDir::new().expect("tempdir");
        let runtime = ScriptedRuntime::start(temp.path(), |_| {
            vec![
                ScriptedConnection::dropping_bind(),
                ScriptedConnection::unbound(),
            ]
        });
        observe::live_owner(temp.path(), None, Some(Initiator::LabWatchdog))
            .unwrap_or_else(|failure| panic!("probe {probe}: the owner answers: {failure}"));
        let record = runtime.finish();
        assert!(
            record.unscripted.is_empty(),
            "probe {probe}: a third connection"
        );
        let [dropped, unbound] = record.connections.as_slice() else {
            panic!("probe {probe}: two connections");
        };
        // The binding the old Runtime dropped, then one reconnect without `BindChannel`.
        assert_eq!(dropped.requests.len(), 2, "probe {probe}");
        assert!(health(&dropped.requests[0]), "probe {probe}");
        assert_eq!(
            declared(&dropped.requests[1]),
            (
                RuntimeChannel::Lab,
                None,
                Some(actingcommand_contract::Initiator::LabWatchdog)
            ),
            "probe {probe}"
        );
        assert_eq!(unbound.requests.len(), 1, "probe {probe}");
        assert!(health(&unbound.requests[0]), "probe {probe}");
        assert_cli_envelope(dropped);
        assert_cli_envelope(unbound);
        assert_eq!(
            *REPORTS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec!["client_channel_unsupported".to_owned()],
            "probe {probe}"
        );
    }
}
