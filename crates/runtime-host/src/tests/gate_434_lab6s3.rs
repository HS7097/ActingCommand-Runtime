// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-6S3 (register §1.287, "Slice LAB-6S3"; contracts/lab-two-entries.md §2.3
// step 5, §2.6, §3): the `ui` part of `gate_two_entries` and `ui_settings_within_tiers`. On a
// connection bound to `ui`, by its `BindChannel` or by its first request's envelope, the door takes
// a handshake kind and a kind of the UI list, and a `ConfigureSettings` only when every change's
// key has the console tier `daily` or `advanced` and is settable. Every other request there is
// Denied `channel_request_refused` with `channel` `ui`, `operation` the kind and, for a setting,
// `config_key` the first key refused, in the request's order; nothing of it is applied. A refused
// request that G14 copies (a write, a read that records) leaves one `command.rejected` with the
// channel `ui`; a refused pure read leaves no record. `lab`, `cli`, `mcp` and an unbound
// connection are answered as before this slice.

use super::*;
use crate::host::ui_gate::{kind_location, ui_may_set, ui_takes_kind};
use actingcommand_contract::outcome::Location;
use actingcommand_contract::{
    CONFIG_OVERRIDES_FACT_KEY, CommandPayload, ConfigParameter, ConfigParameterSource,
    ConfigSettingChange, DiagnosticOutcomePayload, FactScalar, PackLoadEntry, PackRegistryKey,
    PackageDebugRequest, RuntimeChannel, RuntimeConfigManifest, RuntimeOperationKind as Kind,
    ui_allows,
};
use actingcommand_runtime_config::{STUCK_RECOVERY, UI_TIERS, UiTier, find_row, instance_key};
use serde_json::{Value, json};

const ALIAS: &str = "node.a";
/// A neutral instance id for the per-instance keys of the key table.
const FIXTURE_INSTANCE: &str = "inst-fixture";
/// A key of the console's third tier, settable, that the test host's configuration carries.
const EVIDENCE_AGE: &str = "frame_retention_evidence_max_age_days";

/// The UI list as contracts/lab-two-entries.md §3 writes it, the transitional kind included.
const UI_LIST: [Kind; 30] = [
    Kind::Status,
    Kind::RuntimeFactSnapshot,
    Kind::ScheduleExplanation,
    Kind::StartReport,
    Kind::QueryEvents,
    Kind::ReadMaterial,
    Kind::DiscoverInstances,
    Kind::ObserveReadonly,
    Kind::MonitorStatus,
    Kind::InstanceFactSnapshot,
    Kind::PerformanceStatus,
    Kind::FactRefreshStatus,
    Kind::RoutineSummary,
    Kind::ResourceTargetView,
    Kind::PackScheduleView,
    Kind::CaptureSequence,
    Kind::PauseScheduling,
    Kind::ResumeScheduling,
    Kind::ControlEmulatorInstance,
    Kind::SelfCheckInstance,
    Kind::RequestShutdown,
    Kind::ApplicationLifecycle,
    Kind::ConfigureSettings,
    Kind::StopInstanceRun,
    Kind::EditPackSchedule,
    Kind::RunInstanceTask,
    Kind::EditResourceTarget,
    Kind::AcquirePlayerHold,
    Kind::ReleasePlayerHold,
    Kind::RecordClientAction,
];

/// What the released console (v0.12.16) sends besides the handshake, and the two kinds its next
/// release adds.
const CONSOLE_KINDS: [Kind; 19] = [
    Kind::Status,
    Kind::RuntimeFactSnapshot,
    Kind::QueryEvents,
    Kind::ReadMaterial,
    Kind::ScheduleExplanation,
    Kind::StartReport,
    Kind::DiscoverInstances,
    Kind::ObserveReadonly,
    Kind::PauseScheduling,
    Kind::ResumeScheduling,
    Kind::ConfigureSettings,
    Kind::SelfCheckInstance,
    Kind::ControlEmulatorInstance,
    Kind::ApplicationLifecycle,
    Kind::RequestShutdown,
    Kind::RecordClientAction,
    Kind::StopInstanceRun,
    Kind::PackScheduleView,
    Kind::EditPackSchedule,
];

/// The keys the released console's settings page sends, for the instance `instance`, each with a
/// value it may set.
fn console_keys(instance: &str) -> [(String, Value); 11] {
    [
        ("performance.pacing.mode".to_owned(), json!("normal")),
        (
            "performance.pacing.max_concurrent_runs".to_owned(),
            json!(2),
        ),
        (
            "performance.pacing.heavy_dispatch_limit.normal".to_owned(),
            json!(1),
        ),
        ("scheduling.parallel.enabled".to_owned(), json!(true)),
        ("scheduling.blackout.host".to_owned(), json!([])),
        (EVIDENCE_AGE.to_owned(), json!(30)),
        (instance_key(instance, "scheduling.blackout"), json!([])),
        (instance_key(instance, "idle.action"), json!("none")),
        (instance_key(instance, "start_emulator"), json!(false)),
        (instance_key(instance, "touch_backend"), json!("maatouch")),
        (instance_key(instance, "capture_backend"), json!("adb")),
    ]
}

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

// A connection bound to `channel` by its `BindChannel`.
fn bound_client(host: &RuntimeHost, channel: RuntimeChannel) -> TestClient {
    let mut client = TestClient::connect(host);
    let bind = client.request(RuntimeOperation::BindChannel {
        channel,
        via: None,
        initiator: None,
    });
    let bound = client.send(&bind);
    assert_eq!(bound.state(), RuntimeReceiptState::Completed, "{channel:?}");
    client
}

// A host whose configuration carries the instance's `stuck_recovery` (settable, of no console
// tier) and `frame_retention_evidence_max_age_days` (settable, `advanced`); the first key's name.
fn start_with_settings(root: &TempDir) -> (RuntimeHost, String) {
    let instance = instance_id();
    let Value::String(id) = serde_json::to_value(instance).expect("instance id encodes") else {
        panic!("an instance id is a string");
    };
    let stuck = instance_key(&id, STUCK_RECOVERY);
    let mut parameters = vec![
        ConfigParameter {
            key: stuck.clone(),
            value: FactScalar::Boolean(true),
            source: ConfigParameterSource::Default,
            settable: false,
        },
        ConfigParameter {
            key: EVIDENCE_AGE.to_owned(),
            value: FactScalar::Integer(15),
            source: ConfigParameterSource::Default,
            settable: false,
        },
    ];
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("the keys are in the key table");
    let host = RuntimeHost::start(
        config(root).with_config_manifest(RuntimeConfigManifest {
            subsystems: Vec::new(),
            parameters,
        }),
        Arc::new(FakeProvider::one(
            ALIAS,
            instance,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    (host, stuck)
}

fn configure(changes: Vec<(String, Option<Value>)>) -> RuntimeOperation {
    RuntimeOperation::ConfigureSettings {
        expected_version: None,
        changes: changes
            .into_iter()
            .map(|(key, value)| ConfigSettingChange { key, value })
            .collect(),
    }
}

fn load_pack() -> RuntimeOperation {
    RuntimeOperation::LoadPack {
        entry: PackLoadEntry::Restore {
            key: PackRegistryKey::Procedure {
                package_id: "package.example".to_owned(),
            },
        },
    }
}

fn upgrade_state_status() -> RuntimeOperation {
    RuntimeOperation::UpgradeStateStatus {
        transition_id: "transition-1".to_owned(),
    }
}

// Every record of `request`: its type and, for a `command.rejected`, its payload.
fn records(
    host: &RuntimeHost,
    request: &RuntimeRequest,
) -> Vec<(EventType, Option<DiagnosticOutcomePayload>)> {
    host.query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events")
        .into_iter()
        .filter(|event| event.links().request_id() == Some(&request.request_id()))
        .map(|event| {
            let rejected = match event.payload() {
                EventPayload::Command(CommandPayload::Rejected(rejected)) => Some(rejected.clone()),
                _ => None,
            };
            (event.event_type(), rejected)
        })
        .collect()
}

// The refusal's receipt: Denied `channel_request_refused`, `channel` `ui`, `operation`
// `operation` and `config_key` only for a setting.
fn assert_refused(receipt: &RuntimeReceipt, operation: &str, config_key: Option<&str>) {
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    assert_eq!(
        receipt.outcome().code().as_str(),
        "channel_request_refused",
        "{receipt:?}"
    );
    assert_eq!(receipt.channel(), Some(RuntimeChannel::Ui));
    let outcome = receipt.outcome();
    assert_eq!(outcome.value("channel"), Some(&json!("ui")), "{receipt:?}");
    assert_eq!(
        outcome.value("operation"),
        Some(&json!(operation)),
        "{receipt:?}"
    );
    assert_eq!(
        outcome.value("config_key"),
        config_key.map(|key| json!(key)).as_ref(),
        "{receipt:?}"
    );
}

// The one record of the refused `request`: a `command.rejected` with the channel `ui` whose
// outcome is the refusal.
fn assert_rejected_on_ui(host: &RuntimeHost, request: &RuntimeRequest, operation: &str) {
    let found = records(host, request);
    let [(EventType::CommandRejected, Some(rejected))] = found.as_slice() else {
        panic!("one command.rejected of the request: {found:?}");
    };
    assert_eq!(
        (rejected.channel(), rejected.via()),
        (Some(RuntimeChannel::Ui), None)
    );
    assert_eq!(
        rejected.outcome().code().as_str(),
        "channel_request_refused"
    );
    assert_eq!(rejected.outcome().value("channel"), Some(&json!("ui")));
    assert_eq!(
        rejected.outcome().value("operation"),
        Some(&json!(operation))
    );
}

// The settings' version and whether `host.config_overrides` holds a record, read on `reader`.
fn settings_state(reader: &mut TestClient) -> (Option<u64>, bool) {
    let request = reader.request(RuntimeOperation::RuntimeFactSnapshot);
    let receipt = reader.send(&request);
    let Some(RuntimeResult::RuntimeFactSnapshot {
        snapshot,
        config_version,
    }) = receipt.result()
    else {
        panic!("the runtime fact snapshot: {receipt:?}");
    };
    let overrides = snapshot
        .records
        .iter()
        .any(|record| record.key == CONFIG_OVERRIDES_FACT_KEY);
    (*config_version, overrides)
}

// gate_two_entries, the `ui` part: for every kind of `RuntimeOperationKind::ALL` the `ui` channel
// takes it exactly when it is a handshake kind or `ui_allows` holds it, and `ui_allows` is the
// list of §3 (the operation table's `ui_kinds`: lab-ops' `lab_ops_ui_allows_is_the_tables_ui_kinds`).
// The identity card is no handshake kind there. Every kind the released console sends, and the
// two its next release adds, is taken. A refusal names every kind by a registered location.
#[test]
fn lab6s3_gate_two_entries_ui_takes_the_handshake_and_the_ui_list_only() {
    let mut refused = 0;
    for kind in Kind::ALL {
        let listed = UI_LIST.contains(kind);
        assert_eq!(ui_allows(*kind), listed, "{kind:?}");
        let handshake = matches!(kind, Kind::Health | Kind::BindChannel);
        assert_eq!(ui_takes_kind(*kind), handshake || listed, "{kind:?}");
        if !ui_takes_kind(*kind) {
            refused += 1;
        }
        assert!(
            Location::relayed(kind_location(*kind)).is_some(),
            "{kind:?} names {}, no registered location",
            kind_location(*kind)
        );
    }
    assert_eq!(refused, Kind::ALL.len() - UI_LIST.len() - 2);
    assert!(!ui_takes_kind(Kind::DeclareGovernanceIdentity));
    for kind in CONSOLE_KINDS {
        assert!(ui_takes_kind(kind), "the console sends {kind:?}");
    }
}

// On a connection bound to `ui` by its `BindChannel` and on one bound by its first request's
// (User, Ui) envelope: a kind of the list is taken; a write off the list (LoadPack) is refused
// and leaves one `command.rejected` with the channel `ui`; a pure read off the list
// (UpgradeStateStatus) is refused and leaves no record; a read that records (DebugPackage, sent
// with the Lab's envelope, which its own check asks for) is refused and recorded like a write. The
// receipts are Denied `channel_request_refused` with `channel` `ui`, `operation` the kind and no
// `config_key` (read in full on the connection whose envelope asks for the full outcome).
#[test]
fn lab6s3_a_ui_connection_is_refused_a_kind_off_the_list_however_it_was_bound() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, ALIAS, Arc::new(FakeState::default()));
    let mut bound = bound_client(&host, RuntimeChannel::Ui);
    let mut implied = TestClient::connect(&host);
    let first = request_as(
        &implied,
        EventActor::User,
        EventSource::Ui,
        RuntimeOperation::Status,
    );
    let first = implied.send(&first);
    assert_eq!(
        (first.state(), first.channel()),
        (RuntimeReceiptState::Completed, Some(RuntimeChannel::Ui))
    );

    for (client, actor, source) in [
        (&mut bound, EventActor::Cli, EventSource::Cli),
        (&mut implied, EventActor::User, EventSource::Ui),
    ] {
        let full = source == EventSource::Cli;
        let status = request_as(client, actor, source, RuntimeOperation::Status);
        let taken = client.send(&status);
        assert_eq!(
            (taken.state(), taken.channel()),
            (RuntimeReceiptState::Completed, Some(RuntimeChannel::Ui)),
            "{source:?}"
        );

        let write = request_as(client, actor, source, load_pack());
        let refused = client.send(&write);
        if full {
            assert_refused(&refused, "load_pack", None);
        }
        assert_eq!(
            refused.outcome().code().as_str(),
            "channel_request_refused",
            "{source:?}"
        );
        assert!(refused.terminal().is_some(), "{source:?}: {refused:?}");
        assert_rejected_on_ui(&host, &write, "load_pack");

        let read = request_as(client, actor, source, upgrade_state_status());
        let refused = client.send(&read);
        if full {
            assert_refused(&refused, "upgrade_state_status", None);
        }
        assert_eq!(
            (refused.state(), refused.outcome().code().as_str()),
            (RuntimeReceiptState::Denied, "channel_request_refused"),
            "{source:?}"
        );
        assert!(refused.terminal().is_none(), "{source:?}: {refused:?}");
        assert!(records(&host, &read).is_empty(), "{source:?}");
    }

    let recorded_read = request_as(
        &bound,
        EventActor::Lab,
        EventSource::Lab,
        RuntimeOperation::DebugPackage {
            request: PackageDebugRequest::new("fixture-package").expect("debug package request"),
        },
    );
    let refused = bound.send(&recorded_read);
    assert_eq!(refused.state(), RuntimeReceiptState::Denied, "{refused:?}");
    assert_eq!(refused.outcome().code().as_str(), "channel_request_refused");
    assert_eq!(
        refused.outcome().value("operation"),
        Some(&json!("debug_package"))
    );
    assert_rejected_on_ui(&host, &recorded_read, "debug_package");
    drop(bound);
    drop(implied);
    host.close().expect("close host");
}

// ui_settings_within_tiers: for every key of the key table (a per-instance key for a neutral
// instance) the `ui` channel may change it exactly when its console tier is `daily` or `advanced`
// and its row is settable; the eleven keys the released console sends are among them; a key of
// no tier, a key of the third tier that is not settable and a key the key table does not have are
// not. On a `ui` connection a request naming an allowed key and then a refused one is refused
// naming the refused one, and applies nothing (the settings' version stays and no override is
// recorded), while the same request on a `cli` connection completes; a clear of the refused key is
// refused too; a clear of an allowed key is taken and completes; an unknown key is refused.
#[test]
fn lab6s3_ui_settings_within_tiers() {
    let mut allowed = BTreeSet::new();
    for row in actingcommand_runtime_config::rows() {
        let key = row
            .key_for(None)
            .or_else(|| row.key_for(Some(FIXTURE_INSTANCE)))
            .expect("a row's key");
        let (found, _) = find_row(&key).expect("the key table has the key");
        let tier = UI_TIERS
            .iter()
            .find(|(name, _)| *name == found.name)
            .map(|(_, tier)| *tier);
        let expected =
            matches!(tier, Some(UiTier::Daily | UiTier::Advanced)) && found.settable.is_settable();
        assert_eq!(ui_may_set(&key), expected, "{key}");
        if expected {
            allowed.insert(key);
        }
    }
    for (key, _) in console_keys(FIXTURE_INSTANCE) {
        assert!(allowed.contains(&key), "the console sends {key}");
    }
    for key in [
        instance_key(FIXTURE_INSTANCE, STUCK_RECOVERY),
        "mumu_root".to_owned(),
        "lab_module_debugging".to_owned(),
        "no_such.key".to_owned(),
    ] {
        assert!(!ui_may_set(&key), "{key}");
    }

    let root = TempDir::new().expect("tempdir");
    let (host, stuck) = start_with_settings(&root);
    let mut ui = bound_client(&host, RuntimeChannel::Ui);
    let mut cli = bound_client(&host, RuntimeChannel::Cli);
    let mut reader = TestClient::connect(&host);
    let before = settings_state(&mut reader);
    assert!(!before.1, "no override is recorded before");

    let changes = vec![
        (EVIDENCE_AGE.to_owned(), Some(json!(30))),
        (stuck.clone(), Some(json!(false))),
    ];
    let mixed = ui.request(configure(changes.clone()));
    assert_refused(&ui.send(&mixed), "configure_settings", Some(stuck.as_str()));
    assert_rejected_on_ui(&host, &mixed, "configure_settings");
    let mixed_records = records(&host, &mixed);
    let [(_, Some(rejected))] = mixed_records.as_slice() else {
        panic!("one command.rejected of the request: {mixed_records:?}");
    };
    assert_eq!(rejected.outcome().value("config_key"), Some(&json!(stuck)));
    assert_eq!(settings_state(&mut reader), before, "nothing is applied");

    let clear_refused = ui.request(configure(vec![(stuck.clone(), None)]));
    assert_refused(
        &ui.send(&clear_refused),
        "configure_settings",
        Some(stuck.as_str()),
    );
    let unknown = ui.request(configure(vec![("no_such.key".to_owned(), Some(json!(1)))]));
    assert_refused(
        &ui.send(&unknown),
        "configure_settings",
        Some("no_such.key"),
    );
    assert_eq!(settings_state(&mut reader), before, "nothing is applied");

    let on_cli = cli.request(configure(changes));
    let completed = cli.send(&on_cli);
    assert_eq!(
        completed.state(),
        RuntimeReceiptState::Completed,
        "{completed:?}"
    );
    assert!(settings_state(&mut reader).1, "the cli request is applied");

    let clear = ui.request(configure(vec![(EVIDENCE_AGE.to_owned(), None)]));
    let cleared = ui.send(&clear);
    assert_eq!(
        cleared.state(),
        RuntimeReceiptState::Completed,
        "{cleared:?}"
    );
    assert_eq!(cleared.channel(), Some(RuntimeChannel::Ui));
    drop(ui);
    drop(cli);
    drop(reader);
    host.close().expect("close host");
}

// The eleven keys the released console sends, each set and cleared on a `ui` connection, are
// taken: the gate refuses none, and each is answered as the same request on a `cli` connection is
// (here the checks behind the gate answer, the test host's configuration carrying none of them).
#[test]
fn lab6s3_the_released_consoles_eleven_keys_are_taken_on_ui() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, ALIAS, Arc::new(FakeState::default()));
    let mut ui = bound_client(&host, RuntimeChannel::Ui);
    let mut cli = bound_client(&host, RuntimeChannel::Cli);
    for (key, value) in console_keys(FIXTURE_INSTANCE) {
        assert!(ui_may_set(&key), "{key}");
        for value in [Some(value), None] {
            let change = vec![(key.clone(), value.clone())];
            let on_ui = ui.request(configure(change.clone()));
            let on_ui = ui.send(&on_ui);
            assert_ne!(
                on_ui.outcome().code().as_str(),
                "channel_request_refused",
                "{key} {value:?}"
            );
            assert_eq!(on_ui.channel(), Some(RuntimeChannel::Ui));
            let on_cli = cli.request(configure(change));
            let on_cli = cli.send(&on_cli);
            assert_eq!(
                (on_ui.state(), on_ui.outcome().code().as_str()),
                (on_cli.state(), on_cli.outcome().code().as_str()),
                "{key} {value:?}"
            );
        }
    }
    drop(ui);
    drop(cli);
    host.close().expect("close host");
}

// On `lab`, `cli` and `mcp` (bound by their `BindChannel`) and on an unbound connection (bound
// from its first request's (Cli, Cli) envelope, as before this slice), a kind off the UI list and a
// key of no console tier are answered as before this slice: LoadPack and UpgradeStateStatus are
// `not_implemented` and record nothing, the instance's `stuck_recovery` is set, and an unknown key
// is the checks' own `config_key_unknown`; no receipt is `channel_request_refused`.
#[test]
fn lab6s3_every_other_connection_is_answered_as_before() {
    let root = TempDir::new().expect("tempdir");
    let (host, stuck) = start_with_settings(&root);
    let connections = [
        (
            Some(RuntimeChannel::Lab),
            EventActor::Lab,
            EventSource::Lab,
            RuntimeChannel::Lab,
        ),
        (
            Some(RuntimeChannel::Cli),
            EventActor::Cli,
            EventSource::Cli,
            RuntimeChannel::Cli,
        ),
        (
            Some(RuntimeChannel::Mcp),
            EventActor::Cli,
            EventSource::Cli,
            RuntimeChannel::Mcp,
        ),
        (None, EventActor::Cli, EventSource::Cli, RuntimeChannel::Cli),
    ];
    for (bind, actor, source, channel) in connections {
        let mut client = match bind {
            Some(bind) => bound_client(&host, bind),
            None => TestClient::connect(&host),
        };
        for operation in [load_pack(), upgrade_state_status()] {
            let request = request_as(&client, actor, source, operation);
            let receipt = client.send(&request);
            assert_eq!(
                (receipt.state(), receipt.outcome().code().as_str()),
                (RuntimeReceiptState::Denied, "not_implemented"),
                "{bind:?}: {receipt:?}"
            );
            assert_eq!(receipt.channel(), Some(channel), "{bind:?}");
            assert!(records(&host, &request).is_empty(), "{bind:?}");
        }
        let set = request_as(
            &client,
            actor,
            source,
            configure(vec![(stuck.clone(), Some(json!(false)))]),
        );
        let set = client.send(&set);
        assert_eq!(
            (set.state(), set.channel()),
            (RuntimeReceiptState::Completed, Some(channel)),
            "{bind:?}: {set:?}"
        );
        let unknown = request_as(
            &client,
            actor,
            source,
            configure(vec![("no_such.key".to_owned(), Some(json!(1)))]),
        );
        let unknown = client.send(&unknown);
        assert_eq!(
            (unknown.state(), unknown.outcome().code().as_str()),
            (RuntimeReceiptState::Denied, "config_key_unknown"),
            "{bind:?}: {unknown:?}"
        );
    }
    host.close().expect("close host");
}
