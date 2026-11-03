// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #429 R-3 (register §1.272; board #429 row R-3, 「`start_emulator` 在线修改」; Alice's
//! ruling 117 A; coordinator work order `rd-1031/brief-r3.md`): each instance's `start_emulator` is
//! set while the Runtime runs, as `actingctl config set`, MCP `ac_config_set` and the console send
//! it, on a discovery-bound instance only (a fixture, a handset and an instance bound by its port
//! refuse it, named `discovery_binding_required`, as the host names its instances). Every automatic
//! emulator start reads the value in effect at its next decision: the configured start at daemon
//! start (here after a restart that restored the instruction values), the emulator watch's
//! restart, IDLE-1's wake, and each of their claims once more on its key. A paused instance is
//! never started automatically, the pause asked again just before the Start; and setting the key
//! starts and stops nothing by itself.
//!
//! On the scripted emulator (HOST-I1) and a scripted connectivity status, with the S6c and IDLE-1P
//! helpers; the watch on the Runtime's manual clock. The instance ids are minted; the names are
//! neutral. Every wait returns as soon as what it waits for is reported or recorded; `GATE_WAIT`
//! only bounds a broken run.

use super::emulator_controller::{EmulatorCall, FakeEmulatorController, stopped_state};
use super::gate_369_s6c::{
    GATE_WAIT, claims_after, connectivity, head, instance_scope, of_request, pause, stop,
};
use super::gate_398_rd1b::{bound, resolved_handset};
use super::gate_idle1p::{
    NOW_UNIX_MS, WAKE_UNIX_MS, X_ALIAS, emulator_starts, idle_records, kept_rows, no_plan,
    start as idle1p_start, stops, wake_records, wake_skips, x_plan_at, x_row, x_state,
};
use super::instance_workers::phases;
use super::*;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    CONFIG_OVERRIDES_FACT_KEY, ConfigParameterSource, ConfigSettingChange, FactScalar, FactValue,
    RuntimeConfigManifest, RuntimeFactScope, RuntimeLifecyclePhase, config_parameters_fact_key,
};
use actingcommand_host_metrics::NetworkConnectivity;
use actingcommand_runtime_config::{
    DefaultsFile, START_EMULATOR, UiTier, annotate_settable, idle, instance_key, mechanism_rows,
    render_parameters, ui_tier,
};
use serde_json::{Value, json};

const ON: &str = "r3.instance.on";
const OFF: &str = "r3.instance.off";
const HANDSET: &str = "r3.instance.handset";
const BY_PORT: &str = "r3.instance.by_port";
const FIXTURE: &str = "r3.instance.fixture";
/// R2: the default polling interval of the emulator watch (`emulator_control.watch_interval_ms`).
const WATCH_INTERVAL_MS: u64 = 60_000;
/// The port of the instance bound by its port.
const BY_PORT_ADB_PORT: u16 = 15_031;

fn id_text(instance: InstanceId) -> String {
    match serde_json::to_value(instance).expect("instance id encodes") {
        Value::String(text) => text,
        other => panic!("an instance id is a string: {other}"),
    }
}

/// `instance.<instance id>.start_emulator` of `instance`.
fn start_key(instance: InstanceId) -> String {
    instance_key(&id_text(instance), START_EMULATOR)
}

/// The `instances[]` entry of `instance` as a file names it: no `start_emulator`, and stuck
/// recovery off, so no ladder acts on the instance.
fn entry(instance: InstanceId) -> Value {
    json!({"instance_id": id_text(instance), "stuck_recovery": false})
}

/// [`entry`] naming `start_emulator: true`.
fn started_entry(instance: InstanceId) -> Value {
    let mut entry = entry(instance);
    entry["start_emulator"] = json!(true);
    entry
}

/// The configuration actingd assembles over the defaults file from these entries.
fn manifest(entries: Vec<Value>) -> RuntimeConfigManifest {
    let ids = entries
        .iter()
        .map(|entry| {
            entry["instance_id"]
                .as_str()
                .expect("an entry names its instance id")
                .to_owned()
        })
        .collect::<Vec<_>>();
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let mut parameters = render_parameters(
        &mechanism_rows(),
        &ids,
        &defaults,
        &json!({ "instances": entries }),
        &|_| None,
    )
    .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// The change of `instance`'s `start_emulator` to `on`.
fn set_start(instance: InstanceId, on: bool) -> ConfigSettingChange {
    ConfigSettingChange {
        key: start_key(instance),
        value: Some(json!(on)),
    }
}

/// One `ConfigureSettings` of `changes`, as `actingctl config set` sends it: the request and its
/// receipt.
fn configure(
    host: &RuntimeHost,
    changes: Vec<ConfigSettingChange>,
    connection: u64,
) -> (RuntimeRequest, RuntimeReceipt) {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::ConfigureSettings {
            changes,
            expected_version: None,
        },
    );
    let receipt = host
        .process_request_for_test(&request, ConnectionId::new(connection).expect("connection"))
        .expect("the request's receipt");
    (request, receipt)
}

fn completed(receipt: &RuntimeReceipt) {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
}

/// The settings a Completed `ConfigureSettings` receipt reports.
fn settings(receipt: &RuntimeReceipt) -> Vec<actingcommand_contract::ConfiguredSetting> {
    completed(receipt);
    let Some(RuntimeResult::SettingsConfigured { settings, .. }) = receipt.result() else {
        panic!("a settings receipt: {receipt:?}");
    };
    settings.clone()
}

/// The rows of the runtime fact `key` the host holds; none when it holds no such fact.
fn fact_rows(host: &RuntimeHost, key: &str) -> Option<Vec<BTreeMap<String, FactScalar>>> {
    host.runtime_fact_snapshot()
        .expect("the runtime facts")
        .records
        .into_iter()
        .find(|record| record.scope == RuntimeFactScope::Runtime && record.key == key)
        .map(|record| match record.value {
            FactValue::RecordList(rows) => rows,
            other => panic!("{key} is a record list: {other:?}"),
        })
}

/// The `config.parameters.instance` row of `key`: its value, source and whether it is settable;
/// none when the section has no row of the key.
fn instance_row(host: &RuntimeHost, key: &str) -> Option<(FactScalar, String, bool)> {
    let rows = fact_rows(host, &config_parameters_fact_key("instance"))
        .expect("config.parameters.instance");
    rows.into_iter()
        .find(|row| row.get("key") == Some(&FactScalar::String(key.to_owned())))
        .map(|row| {
            let value = row.get("value").cloned().expect("the row's value");
            let Some(FactScalar::String(source)) = row.get("source").cloned() else {
                panic!("the row's source is a string: {row:?}");
            };
            let Some(FactScalar::Boolean(settable)) = row.get("settable").cloned() else {
                panic!("the row's settable is a boolean: {row:?}");
            };
            (value, source, settable)
        })
}

/// The `host.config_overrides` row of `key`.
fn override_row(host: &RuntimeHost, key: &str) -> Option<BTreeMap<String, FactScalar>> {
    fact_rows(host, CONFIG_OVERRIDES_FACT_KEY)?
        .into_iter()
        .find(|row| row.get("config_key") == Some(&FactScalar::String(key.to_owned())))
}

fn text(value: &str) -> FactScalar {
    FactScalar::String(value.to_owned())
}

fn reported(lines: &[Outcome], code: &str) -> usize {
    lines
        .iter()
        .filter(|line| line.code().as_str() == code)
        .count()
}

// The report lines once `done` holds for them (none is taken).
fn await_reports(host: &RuntimeHost, done: impl Fn(&[Outcome]) -> bool) -> Vec<Outcome> {
    host.emulator_autostart_reports()
        .await_lines_for_test(done, GATE_WAIT)
        .expect("the awaited report lines")
}

/// Whether `line` is a skip for the value off (`skip_reason` `start_emulator_off`).
fn off_skip(line: &Outcome) -> bool {
    line.code().as_str() == "emulator_autostart_skipped"
        && line.value("skip_reason") == Some(&json!("start_emulator_off"))
}

/// Whether `line` is a skip for the instance's own pause (`skip_reason` `instance_paused`).
fn paused_skip(line: &Outcome) -> bool {
    line.code().as_str() == "emulator_autostart_skipped"
        && line.value("skip_reason") == Some(&json!("instance_paused"))
}

// The outcomes of the `device_state_observed` records under `instance` whose code is `code`, in
// order.
fn observed(host: &RuntimeHost, instance: InstanceId, code: &str) -> Vec<Outcome> {
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    phases(&events)
        .into_iter()
        .filter(|(event, _)| event.links().instance_id() == Some(&instance))
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::DeviceStateObserved { outcome }
                if outcome.code().as_str() == code =>
            {
                Some(outcome)
            }
            _ => None,
        })
        .collect()
}

fn count(events: &[PersistedEvent], event_type: EventType) -> usize {
    events
        .iter()
        .filter(|event| event.event_type() == event_type)
        .count()
}

/// The emulator control actions the scripted emulator was asked for (start, stop, restart).
fn control_calls(state: &FakeState) -> Vec<EmulatorCall> {
    state
        .emulator
        .calls()
        .into_iter()
        .filter(|call| matches!(call, EmulatorCall::Control(_)))
        .collect()
}

/// A discovery-bound fake whose emulator is stopped when the host starts: the Runtime holds it
/// pending, and its opens are refused until a port is bound (#411 A48).
fn pending_state() -> Arc<FakeState> {
    let state = Arc::new(FakeState {
        emulator: FakeEmulatorController::pending(),
        ..FakeState::default()
    });
    state.emulator_control.store(true, Ordering::Release);
    state.refuse_pending_opens.store(true, Ordering::Release);
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    state.observed_open.store(true, Ordering::Release);
    state
}

/// A host of `ON` (its file names no `start_emulator`), discovery-bound to `state`'s running
/// emulator, on `clock` and the connectivity status `read`.
fn on_host(
    root: &TempDir,
    on: InstanceId,
    state: &Arc<FakeState>,
    clock: &Arc<ManualRuntimeClock>,
    read: crate::host::NetworkConnectivityForTest,
) -> RuntimeHost {
    RuntimeHost::start(
        config(root)
            .with_runtime_clock(clock.clone())
            .with_network_connectivity_for_test(read)
            .with_config_manifest(manifest(vec![entry(on)])),
        Arc::new(FakeProvider::one(ON, on, Arc::clone(state))),
    )
    .expect("runtime host")
}

// Set online: on an instance bound by discovery whose file does not name it, `start_emulator` is
// set by an instruction (`instruction`, over no base: `base_source` `default`, `base_json`
// `null`) and cleared again (no value in effect: the receipt's `null` from `default`, no row); on
// one whose file names it, an instruction overrides the file's value. The row is settable and
// the console shows it in the daily tier.
#[test]
fn r3_start_emulator_settable_online_on_a_discovery_bound_instance() {
    let root = TempDir::new().expect("tempdir");
    let (on, off) = (instance_id(), instance_id());
    let host = RuntimeHost::start(
        config(&root)
            .with_emulator_autostart(vec![OFF.to_owned()])
            .with_config_manifest(manifest(vec![entry(on), started_entry(off)])),
        Arc::new(FakeProvider::from_entries([
            (ON.to_owned(), on, x_state()),
            (OFF.to_owned(), off, x_state()),
        ])),
    )
    .expect("runtime host");
    let (on_key, off_key) = (start_key(on), start_key(off));
    assert_eq!(ui_tier(&on_key), UiTier::Daily);
    assert_eq!(
        instance_row(&host, &on_key),
        None,
        "the file does not name it"
    );
    assert_eq!(
        instance_row(&host, &off_key),
        Some((FactScalar::Boolean(true), "explicit".to_owned(), true)),
        "the file's value, settable"
    );

    // On, over no base.
    let (_, receipt) = configure(&host, vec![set_start(on, true)], 1_301);
    let configured = settings(&receipt);
    let [setting] = &configured[..] else {
        panic!("one setting: {receipt:?}");
    };
    assert_eq!(setting.key, on_key);
    assert_eq!(setting.value, FactScalar::Boolean(true));
    assert_eq!(setting.source, ConfigParameterSource::Instruction);
    assert_eq!(
        instance_row(&host, &on_key),
        Some((FactScalar::Boolean(true), "instruction".to_owned(), true))
    );
    let row = override_row(&host, &on_key).expect("the instruction's row");
    assert_eq!(row.get("value_json"), Some(&text("true")));
    assert_eq!(row.get("base_source"), Some(&text("default")));
    assert_eq!(row.get("base_json"), Some(&text("null")));

    // Off, over the file's value.
    let (_, receipt) = configure(&host, vec![set_start(off, false)], 1_302);
    let configured = settings(&receipt);
    let [setting] = &configured[..] else {
        panic!("one setting: {receipt:?}");
    };
    assert_eq!(setting.value, FactScalar::Boolean(false));
    assert_eq!(setting.source, ConfigParameterSource::Instruction);
    assert_eq!(
        instance_row(&host, &off_key),
        Some((FactScalar::Boolean(false), "instruction".to_owned(), true))
    );
    let row = override_row(&host, &off_key).expect("the instruction's row");
    assert_eq!(row.get("base_source"), Some(&text("explicit")));
    assert_eq!(row.get("base_json"), Some(&text("true")));

    // Cleared: the key the file does not write has no value in effect again.
    let (_, receipt) = configure(
        &host,
        vec![ConfigSettingChange {
            key: on_key.clone(),
            value: None,
        }],
        1_303,
    );
    let configured = settings(&receipt);
    let [setting] = &configured[..] else {
        panic!("one setting: {receipt:?}");
    };
    assert_eq!(setting.value, text("null"));
    assert_eq!(setting.source, ConfigParameterSource::Default);
    assert_eq!(instance_row(&host, &on_key), None);
    assert_eq!(override_row(&host, &on_key), None);
    assert!(
        override_row(&host, &off_key).is_some(),
        "the other row stays"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// A fixture, a handset and an instance bound by its port have no emulator the Runtime can start:
// `start_emulator` `true` on each is Denied, `config_value_invalid` with `config_value_problem`
// `discovery_binding_required` naming the instance's key, and nothing changes; the instance
// bound by discovery beside them takes it.
#[test]
fn r3_fixture_and_handset_refuse_start_emulator_named() {
    let refused = |host: &RuntimeHost, instance: InstanceId, connection: u64| {
        let key = start_key(instance);
        let (request, receipt) = configure(host, vec![set_start(instance, true)], connection);
        assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
        assert_eq!(receipt.outcome().code().as_str(), "config_value_invalid");
        let outcome = host
            .failure_outcome_for_test(request.request_id())
            .expect("refusal outcomes")
            .expect("the request was refused");
        assert_eq!(outcome.code().as_str(), "config_value_invalid");
        assert_eq!(outcome.value("config_key"), Some(&json!(key)));
        assert_eq!(
            outcome.value("config_value_problem"),
            Some(&json!("discovery_binding_required"))
        );
        assert_eq!(instance_row(host, &key), None, "nothing changed");
        assert_eq!(override_row(host, &key), None);
    };

    // The physical host: an emulator bound by discovery, a handset and an instance bound by its
    // port.
    let root = TempDir::new().expect("tempdir");
    let (on, handset, by_port) = (instance_id(), instance_id(), instance_id());
    let mut handset_entry = entry(handset);
    handset_entry["device"] = json!({"kind": "handset", "transport": "usb"});
    let port_bound =
        ResolvedExecutionInstance::new(by_port, format!("127.0.0.1:{BY_PORT_ADB_PORT}"))
            .with_adb_endpoint(actingcommand_execution_kernel::ResolvedAdbEndpoint::new(
                "127.0.0.1",
                BY_PORT_ADB_PORT,
                false,
            ));
    let host = RuntimeHost::start(
        config(&root).with_config_manifest(manifest(vec![
            entry(on),
            handset_entry,
            entry(by_port),
        ])),
        Arc::new(
            FakeProvider::from_entries([
                (ON.to_owned(), on, x_state()),
                (HANDSET.to_owned(), handset, Arc::new(FakeState::default())),
                (BY_PORT.to_owned(), by_port, Arc::new(FakeState::default())),
            ])
            .with_resolved_for(
                HANDSET,
                resolved_handset(HANDSET, handset, bound(HANDSET, "TESTSERIAL0031")),
            )
            .with_resolved_for(BY_PORT, port_bound),
        ),
    )
    .expect("runtime host");
    refused(&host, handset, 1_311);
    refused(&host, by_port, 1_312);
    let (_, receipt) = configure(&host, vec![set_start(on, true)], 1_313);
    completed(&receipt);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");

    // The fixture host.
    let root = TempDir::new().expect("tempdir");
    let fixture = instance_id();
    let host = RuntimeHost::start(
        config(&root).with_config_manifest(manifest(vec![entry(fixture)])),
        Arc::new(
            FakeProvider::one(FIXTURE, fixture, Arc::new(FakeState::default()))
                .fixture_simulation(),
        ),
    )
    .expect("runtime host with a fixture");
    refused(&host, fixture, 1_314);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// The configured start at daemon start reads the value in effect at that start, the file's value
// with the instruction values restored over it, not the start-time list: a person turned ON's
// start on (its file names none) and OFF's off (its file names `true`, and actingd lists it), and
// the Runtime restarted. Of the two stopped emulators, ON's alone is claimed and started.
#[test]
fn r3_auto_launch_reads_the_value_in_effect() {
    let root = TempDir::new().expect("tempdir");
    let (on, off) = (instance_id(), instance_id());
    let (on_state, off_state) = (x_state(), x_state());
    let start = || {
        let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
        RuntimeHost::start(
            config(&root)
                .with_emulator_autostart(vec![OFF.to_owned()])
                .with_network_connectivity_for_test(read)
                .with_config_manifest(manifest(vec![entry(on), started_entry(off)])),
            Arc::new(FakeProvider::from_entries([
                (ON.to_owned(), on, Arc::clone(&on_state)),
                (OFF.to_owned(), off, Arc::clone(&off_state)),
            ])),
        )
        .expect("runtime host")
    };

    // The first run: the person's two changes; the Runtime stops.
    let host = start();
    let (_, receipt) = configure(
        &host,
        vec![set_start(on, true), set_start(off, false)],
        1_321,
    );
    completed(&receipt);
    host.close().expect("close host");

    // The next start restores both; both emulators are stopped at its daemon start.
    let host = start();
    stop(&host, ON, 1_322);
    stop(&host, OFF, 1_323);
    let before = head(&host);
    assert_eq!(
        host.enqueue_emulator_autostart().expect("enqueue"),
        1,
        "the instance the value in effect turns on, alone"
    );
    let lines = await_reports(&host, |lines| {
        reported(lines, "emulator_autostart_started") == 1
    });
    let started = lines
        .iter()
        .find(|line| line.code().as_str() == "emulator_autostart_started")
        .expect("the start");
    assert_eq!(started.value("alias"), Some(&json!(ON)));
    let queued = lines
        .iter()
        .find(|line| line.code().as_str() == "emulator_autostart_queued")
        .expect("the count");
    assert_eq!(queued.value("count"), Some(&json!(1)));
    assert_eq!(emulator_starts(&on_state), 1, "ON's emulator is started");
    assert_eq!(emulator_starts(&off_state), 0, "OFF's is not");
    assert_eq!(claims_after(&host, on, before).len(), 1);
    assert!(
        claims_after(&host, off, before).is_empty(),
        "no claim for OFF"
    );
    assert_eq!(reported(&lines, "emulator_autostart_skipped"), 0);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// The emulator watch restarts a stopped emulator only while the instance's `start_emulator` is on
// in effect, read at each poll and by the restart's claim once more on its key: off, a poll
// requests nothing; turned on, the next poll requests the restart; turned off while that claim
// waits for the network, the claim releases its key with no command record and no failure pair,
// starts nothing, says so once (`start_emulator_off`, `watch_kind` `restart`) and gives its place
// in the watch's bound back; turned on again, the next poll restarts the emulator.
#[test]
fn r3_monitor_relaunch_follows_the_value_in_effect() {
    let root = TempDir::new().expect("tempdir");
    let on = instance_id();
    let state = x_state();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    // The first restart's claim waits without internet access; the later one has it.
    let (read, reads) = connectivity(
        vec![NetworkConnectivity::NoInternet],
        NetworkConnectivity::Internet,
    );
    let host = on_host(&root, on, &state, &clock, read);
    host.start_emulator_watch().expect("the watch starts");

    // Off: the first poll finds the emulator stopped and requests nothing.
    let before = head(&host);
    state.emulator.script_read(stopped_state());
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(1, GATE_WAIT)
        .expect("the first poll");
    assert!(
        claims_after(&host, on, before).is_empty(),
        "off: no restart"
    );

    // On: the next poll requests the restart, whose claim waits for the network.
    completed(&configure(&host, vec![set_start(on, true)], 1_331).1);
    let before = head(&host);
    state.emulator.script_read(stopped_state());
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(2, GATE_WAIT)
        .expect("the second poll");
    assert_eq!(
        reads
            .recv_timeout(GATE_WAIT)
            .expect("the claim's first read"),
        NetworkConnectivity::NoInternet
    );
    let [claim] = claims_after(&host, on, before)[..] else {
        panic!("one restart claim");
    };

    // Off while the claim waits: its next check on its key gives the key up.
    completed(&configure(&host, vec![set_start(on, false)], 1_332).1);
    let lines = await_reports(&host, |lines| lines.iter().any(off_skip));
    assert_eq!(reported(&lines, "emulator_autostart_failed"), 0);
    assert_eq!(reported(&lines, "emulator_autostart_started"), 0);
    let claim = of_request(&host, claim);
    assert_eq!(count(&claim, EventType::LeaseGranted), 1, "{claim:#?}");
    assert_eq!(count(&claim, EventType::LeaseReleased), 1, "{claim:#?}");
    for event_type in [
        EventType::CommandReceived,
        EventType::CommandRejected,
        EventType::RuntimeFailed,
    ] {
        assert_eq!(count(&claim, event_type), 0, "{event_type:?}: {claim:#?}");
    }
    let skips = observed(&host, on, "emulator_autostart_skipped");
    let [skip] = &skips[..] else {
        panic!("one skip on the claim's key: {skips:?}");
    };
    assert!(off_skip(skip), "{skip:?}");
    assert_eq!(skip.value("watch_kind"), Some(&json!("restart")));
    assert_eq!(emulator_starts(&state), 0, "no Start");

    // Off: a poll requests nothing again.
    let before = head(&host);
    state.emulator.script_read(stopped_state());
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(3, GATE_WAIT)
        .expect("the third poll");
    assert!(
        claims_after(&host, on, before).is_empty(),
        "off: no restart"
    );

    // On again: the next poll restarts the emulator, its claim reading the status once more.
    completed(&configure(&host, vec![set_start(on, true)], 1_333).1);
    state.emulator.script_read(stopped_state());
    state.emulator.script_read(stopped_state());
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(4, GATE_WAIT)
        .expect("the fourth poll");
    await_reports(&host, |lines| {
        reported(lines, "emulator_autostart_started") == 1
    });
    assert_eq!(emulator_starts(&state), 1, "one Start");
    let queued = observed(&host, on, "emulator_autostart_queued");
    assert_eq!(queued.len(), 2, "{queued:?}");
    for request in &queued {
        assert_eq!(
            request.value("attempts"),
            Some(&json!(1)),
            "the skipped restart gave its place in the bound back: {queued:?}"
        );
    }
    assert_eq!(
        observed(&host, on, "emulator_autostart_skipped").len(),
        1,
        "one skip"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// IDLE-1's wake after `stop_emulator` starts the emulator only while `start_emulator` is on in
// effect: X (its file names `true` and `stop_emulator`) is closed by its idle action, then a
// person turns its action to `close_app` and its emulator start off in one request. The wake comes
// at once (the action switched) and records itself, but requests no claim and starts nothing:
// one skip says why (`start_emulator_off`, `idle_action` `stop_emulator`), and X is no longer kept
// closed once it is free, its emulator left stopped.
#[test]
fn r3_idle_wake_follows_the_value_in_effect() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let x = instance_id();
    let state = x_state();
    let host = idle1p_start(&root, &clock, x, &state, false).expect("the start");
    host.check_idle_instances_for_test(&x_plan_at(NOW_UNIX_MS))
        .expect("the idle check");
    assert_eq!(
        host.await_idle_action_ends_for_test(1, GATE_WAIT),
        Some(1),
        "X's idle action ended"
    );
    assert_eq!(stops(&state), 1, "X's action asked for its Stop");
    assert_eq!(
        kept_rows(&host),
        Some(vec![x_row(Some(WAKE_UNIX_MS))]),
        "X is kept closed with its wake"
    );

    let changed = head(&host);
    let (_, receipt) = configure(
        &host,
        vec![
            ConfigSettingChange {
                key: instance_key(&id_text(x), idle::ACTION),
                value: Some(json!("close_app")),
            },
            set_start(x, false),
        ],
        1_341,
    );
    completed(&receipt);
    // The wake (the lease sweep's check may take it first), then a check that finds X free.
    host.check_idle_instances_for_test(&no_plan())
        .expect("the wake's check");
    host.check_idle_instances_for_test(&no_plan())
        .expect("the check after the wake");

    assert_eq!(
        idle_records(&host, x, changed),
        wake_records().into_iter().take(1).collect::<Vec<_>>(),
        "the wake alone: no configured emulator start queued"
    );
    let skips = wake_skips(&host, x, changed);
    let [skip] = &skips[..] else {
        panic!("one skip of the wake: {skips:?}");
    };
    assert!(off_skip(skip), "{skip:?}");
    assert_eq!(skip.value("alias"), Some(&json!(X_ALIAS)));
    assert_eq!(skip.value("idle_action"), Some(&json!("stop_emulator")));
    assert!(claims_after(&host, x, changed).is_empty(), "no claim");
    assert_eq!(emulator_starts(&state), 0, "nothing starts X's emulator");
    assert!(
        kept_rows(&host).unwrap_or_default().is_empty(),
        "X is no longer kept closed"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// Alice's ruling 117 A with the value set online: ON's start is turned on by an instruction, and
// its own scheduling pause holds it. Neither the configured start at daemon start nor the
// emulator watch starts its stopped emulator: no claim, no Start, and each records its skip
// (`instance_paused`, the watch's with `watch_kind` `restart`), which a start the value turned
// off never records.
#[test]
fn r3_paused_instance_is_never_auto_launched() {
    let root = TempDir::new().expect("tempdir");
    let on = instance_id();
    let state = x_state();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = on_host(&root, on, &state, &clock, read);
    host.start_emulator_watch().expect("the watch starts");
    completed(&configure(&host, vec![set_start(on, true)], 1_351).1);
    completed(&pause(&host, instance_scope(ON), 1_352));
    // A person stops the emulator under the pause (an explicit control runs).
    stop(&host, ON, 1_353);
    let before = head(&host);

    assert_eq!(
        host.enqueue_emulator_autostart().expect("enqueue"),
        0,
        "the daemon start leaves the paused instance"
    );
    state.emulator.script_read(stopped_state());
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(1, GATE_WAIT)
        .expect("a poll under the pause");
    let lines = await_reports(&host, |lines| {
        lines.iter().filter(|line| paused_skip(line)).count() == 2
    });
    assert_eq!(reported(&lines, "emulator_autostart_started"), 0);
    assert!(
        claims_after(&host, on, before).is_empty(),
        "no claim for the paused instance"
    );
    assert_eq!(emulator_starts(&state), 0, "no Start");
    let skips = observed(&host, on, "emulator_autostart_skipped");
    let [daemon_start, watch] = &skips[..] else {
        panic!("the daemon start's skip and the watch's: {skips:?}");
    };
    assert!(paused_skip(daemon_start), "{daemon_start:?}");
    assert_eq!(daemon_start.value("watch_kind"), None);
    assert!(paused_skip(watch), "{watch:?}");
    assert_eq!(watch.value("watch_kind"), Some(&json!("restart")));
    assert_eq!(
        host.instance_claims_for_test(ON).expect("claims"),
        (None, vec![]),
        "nothing holds or waits for the paused instance"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// The pause asked again just before the Start, with the value set online: the configured start
// of ON (turned on by an instruction), claimed before the pause and holding its key while it
// waits for the network, meets the pause on its key: it releases the key with no command record
// and no failure pair, starts nothing and records the skip, and the pause's hand-back completes.
#[test]
fn r3_pause_rechecked_just_before_launch() {
    let root = TempDir::new().expect("tempdir");
    let on = instance_id();
    let state = x_state();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let (read, reads) = connectivity(Vec::new(), NetworkConnectivity::NoInternet);
    let host = on_host(&root, on, &state, &clock, read);
    completed(&configure(&host, vec![set_start(on, true)], 1_361).1);
    stop(&host, ON, 1_362);
    let before = head(&host);
    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    assert_eq!(
        reads
            .recv_timeout(GATE_WAIT)
            .expect("the claim's first read"),
        NetworkConnectivity::NoInternet
    );
    let [claim] = claims_after(&host, on, before)[..] else {
        panic!("one autostart claim");
    };

    // The pause's hand-back waits for the claim's key, which the claim's next check gives up.
    completed(&pause(&host, instance_scope(ON), 1_363));
    let lines = await_reports(&host, |lines| lines.iter().any(paused_skip));
    assert_eq!(reported(&lines, "emulator_autostart_failed"), 0);
    assert_eq!(reported(&lines, "emulator_autostart_started"), 0);
    let claim = of_request(&host, claim);
    assert_eq!(count(&claim, EventType::LeaseGranted), 1, "{claim:#?}");
    assert_eq!(count(&claim, EventType::LeaseReleased), 1, "{claim:#?}");
    for event_type in [
        EventType::CommandReceived,
        EventType::CommandRejected,
        EventType::RuntimeFailed,
    ] {
        assert_eq!(count(&claim, event_type), 0, "{event_type:?}: {claim:#?}");
    }
    assert_eq!(emulator_starts(&state), 0, "no Start");
    let skips = observed(&host, on, "emulator_autostart_skipped");
    let [skip] = &skips[..] else {
        panic!("one skip on the claim's key: {skips:?}");
    };
    assert!(paused_skip(skip), "{skip:?}");
    assert_eq!(skip.value("watch_kind"), None);
    assert_eq!(
        host.instance_claims_for_test(ON).expect("claims"),
        (None, vec![])
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// The change itself: turning `start_emulator` on for an instance whose emulator is stopped, off
// for one whose emulator runs, and clearing either, starts, stops, claims and binds nothing. The
// consumers act at their next decision (above).
#[test]
fn r3_change_itself_launches_and_stops_nothing() {
    let root = TempDir::new().expect("tempdir");
    let (on, off) = (instance_id(), instance_id());
    let (on_state, off_state) = (pending_state(), x_state());
    let host = RuntimeHost::start(
        config(&root)
            .with_emulator_autostart(vec![OFF.to_owned()])
            .with_config_manifest(manifest(vec![entry(on), started_entry(off)])),
        Arc::new(FakeProvider::from_entries([
            (ON.to_owned(), on, Arc::clone(&on_state)),
            (OFF.to_owned(), off, Arc::clone(&off_state)),
        ])),
    )
    .expect("runtime host with ON stopped and OFF running");
    let before = head(&host);

    completed(&configure(&host, vec![set_start(on, true)], 1_371).1);
    completed(&configure(&host, vec![set_start(off, false)], 1_372).1);
    for key in [start_key(on), start_key(off)] {
        completed(&configure(&host, vec![ConfigSettingChange { key, value: None }], 1_373).1);
    }
    completed(
        &configure(
            &host,
            vec![set_start(on, true), set_start(off, false)],
            1_374,
        )
        .1,
    );

    for (alias, instance, state) in [(ON, on, &on_state), (OFF, off, &off_state)] {
        assert!(
            control_calls(state).is_empty(),
            "{alias}: nothing started or stopped"
        );
        assert!(
            claims_after(&host, instance, before).is_empty(),
            "{alias}: nothing claimed"
        );
        assert_eq!(
            host.instance_claims_for_test(alias).expect("claims"),
            (None, vec![]),
            "{alias}"
        );
    }
    assert_eq!(on_state.emulator.bound_port(), None, "ON stays stopped");
    assert!(
        off_state.emulator.bound_port().is_some(),
        "OFF keeps running"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
