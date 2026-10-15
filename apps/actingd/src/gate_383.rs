// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #383 AO-7 (model-ledger-observer v1.4.1 §3.7; test rows AO7-a and AO7-c of §14.2):
//! the daemon's policy monitor wakes the driver from the host's business notifications. It has
//! no Runtime client, so it opens no ledger subscription, and a paused ledger flush does not
//! hold a wake back.
//!
//! The host is a real one without instances, the monitor is the production loop
//! (`run_policy_monitor`) and the driver is a stand-in that reports each wake it is given. The
//! ledger flush is paused with the writer stall gate (LED-I3): a commit stops inside its write
//! transaction until the test releases it. `GATE_WAIT` only bounds a broken run.

use super::{ActingdError, PolicyDriverControl, PolicyDriverWake, run_policy_monitor};
use actingcommand_contract::{
    EventActor, EventQuery, EventSource, FactContent, FactRecord, FactScope, FactValue,
    ProjectionProfile, RuntimeSubscriptionRequest, SubscriptionCursor,
};
use actingcommand_ledger::WriterStallGate;
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use actingcommand_runtime_host::{
    ExecutionBackendRegistry, PolicyTrigger, PolicyWakes, ProviderAssembly, RuntimeHost,
    RuntimeHostConfig,
};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tempfile::TempDir;

const GATE_WAIT: Duration = Duration::from_secs(30);

type Driver = JoinHandle<Result<(), ActingdError>>;
type Monitor = JoinHandle<(Result<(), ActingdError>, Driver)>;

/// A Runtime without instances and without a performance thread: only the test's own steps
/// record anything after the start.
fn start_host(root: &TempDir) -> Arc<RuntimeHost> {
    let provider = ExecutionBackendRegistry::from_assembly(ProviderAssembly {
        instances: Vec::new(),
        vision: None,
        discovery: None,
    })
    .expect("a registry without instances");
    Arc::new(
        RuntimeHost::start(
            RuntimeHostConfig::new(root.path(), b"gate-383-test-salt")
                .with_frame_retention_enabled(false),
            Arc::new(provider),
        )
        .expect("runtime host"),
    )
}

/// A new server-scoped observation for the instance fact store.
fn fact(key: &str, snapshot: &str) -> FactRecord {
    FactRecord {
        scope: FactScope::Server {
            server_id: "fixture-server".to_owned(),
        },
        key: key.to_owned(),
        content: FactContent::Inline {
            value: FactValue::Boolean(true),
        },
        observed_at_unix_ms: 1_000,
        expires_at_unix_ms: None,
        ttl_policy: None,
        confidence_milli: 1_000,
        source_detector: "detector.fixture".to_owned(),
        source_snapshot_id: snapshot.to_owned(),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "a".repeat(64),
        invalidate_on: Vec::new(),
    }
}

/// The stand-in driver: reports each wake it is given and ends at the driver shutdown.
fn stand_in_driver(control: &Arc<PolicyDriverControl>) -> (Driver, mpsc::Receiver<PolicyTrigger>) {
    let (reports, woken) = mpsc::channel();
    let control = Arc::clone(control);
    let driver = thread::spawn(move || {
        loop {
            match control.wait(GATE_WAIT)? {
                PolicyDriverWake::Trigger(trigger, _) => {
                    reports.send(trigger).expect("report the wake");
                }
                PolicyDriverWake::Shutdown => return Ok(()),
                PolicyDriverWake::TimedOut => {}
            }
        }
    });
    (driver, woken)
}

/// The production monitor loop on its own thread, as `monitor_policy` runs it.
fn spawn_monitor(
    host: &Arc<RuntimeHost>,
    control: &Arc<PolicyDriverControl>,
    wakes: PolicyWakes,
    driver: Driver,
) -> Monitor {
    let host = Arc::clone(host);
    let control = Arc::clone(control);
    thread::spawn(move || {
        let ended = run_policy_monitor(&host, &control, &wakes, &driver);
        (ended, driver)
    })
}

fn connect(root: &TempDir) -> RuntimeClient {
    RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        actingcommand_runtime_client::RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect a control client")
}

/// The run's end: the host accepts a shutdown, the monitor ends with it and the stand-in driver
/// is released.
fn stop(root: &TempDir, control: &PolicyDriverControl, monitor: Monitor) {
    connect(root)
        .request_shutdown()
        .expect("the host accepts the shutdown");
    let (ended, driver) = monitor.join().expect("monitor thread");
    ended.expect("the monitor ends at the accepted shutdown");
    control.shutdown().expect("release the stand-in driver");
    driver
        .join()
        .expect("driver thread")
        .expect("the stand-in driver ends at its shutdown");
}

fn close(host: Arc<RuntimeHost>) {
    Arc::try_unwrap(host)
        .ok()
        .expect("the test holds the only host")
        .close()
        .expect("close the host");
}

/// AO7-a: a change of the instance fact store wakes the driver (`FactsChanged`) while the
/// ledger flush is paused. The monitor and the driver start only after a later commit is held
/// at the gate, and the driver is woken while that commit is still held.
#[test]
fn gate_ao7a_an_instance_fact_change_wakes_the_driver_while_the_ledger_flush_is_paused() {
    let root = TempDir::new().expect("tempdir");
    let host = start_host(&root);
    let wakes = host.open_policy_wakes().expect("open the policy wakes");
    host.publish_fact(fact("env.fixture_flag", "snapshot:gate-383-a-first"))
        .expect("the instance fact store accepts the observation");

    let gate = WriterStallGate::arm(root.path()).expect("arm the writer stall gate");
    let held = thread::spawn({
        let host = Arc::clone(&host);
        move || {
            host.publish_fact(fact("env.fixture_other", "snapshot:gate-383-a-second"))
                .map(drop)
                .map_err(|error| error.code())
        }
    });
    assert!(
        gate.wait_held_commit(GATE_WAIT),
        "the ledger flush is paused with a commit held at the gate"
    );

    let control = Arc::new(PolicyDriverControl::default());
    let (driver, woken) = stand_in_driver(&control);
    let monitor = spawn_monitor(&host, &control, wakes, driver);
    assert_eq!(
        woken.recv_timeout(GATE_WAIT),
        Ok(PolicyTrigger::FactsChanged),
        "the instance fact change wakes the driver"
    );
    assert!(
        !held.is_finished(),
        "the held commit is still paused when the driver wakes"
    );

    gate.release();
    assert_eq!(held.join().expect("held publication"), Ok(()));
    stop(&root, &control, monitor);
    close(host);
}

/// AO7-c: the driver is woken while nothing subscribes to the ledger. The Runtime counts the
/// event subscriptions it serves; the count is taken once from a client's own subscription
/// (the probe counts) and does not move while the monitor runs and wakes the driver.
#[test]
fn gate_ao7c_the_monitor_wakes_the_driver_without_a_ledger_subscription() {
    let root = TempDir::new().expect("tempdir");
    let host = start_host(&root);
    connect(&root)
        .subscribe_events(
            RuntimeSubscriptionRequest::new(
                EventQuery::default(),
                ProjectionProfile::Concise,
                SubscriptionCursor { after_sequence: 0 },
                0,
                1,
            )
            .expect("subscription request"),
        )
        .expect("a client's own subscription");
    assert_eq!(host.event_subscriptions_for_test(), 1);

    let wakes = host.open_policy_wakes().expect("open the policy wakes");
    let control = Arc::new(PolicyDriverControl::default());
    let (driver, woken) = stand_in_driver(&control);
    let monitor = spawn_monitor(&host, &control, wakes, driver);
    host.publish_fact(fact("env.fixture_flag", "snapshot:gate-383-c"))
        .expect("the instance fact store accepts the observation");
    assert_eq!(
        woken.recv_timeout(GATE_WAIT),
        Ok(PolicyTrigger::FactsChanged),
        "the instance fact change wakes the driver"
    );
    stop(&root, &control, monitor);

    assert_eq!(
        host.event_subscriptions_for_test(),
        1,
        "the policy monitor subscribed to the ledger"
    );
    close(host);
}
