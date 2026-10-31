// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #387 D1b, test rows D1-M3 to D1-M7 (frozen model v1.2 §8.2): `ConfigureSettings`,
//! its records, its refusals, `clear`, and the restore of instruction values at start by the
//! later-change-wins rule (§12 question 3). Every expected value comes from the model: §5.5
//! (`settable`), §5.6 (checks, receipt, two layers), §5.7 (records, `host.config_overrides`),
//! §5.9 (`config.parameters.<section>`), §6.4 (codes).
//!
//! Workflow #411 B15 R3 (model-b15 §5.2; register §1.113): the remedy keys are settable and
//! recorded as rows (host key 112).
//!
//! Workflow #429 R-1 (A.6 rule 4; register §1.219; since 0.12.31), tests 1 to 3: an instance
//! whose file writes no blackout list of its own gets one by instruction and keeps it across a
//! restart; clearing it leaves the instance with none; a file that writes the key later voids the
//! instruction (`config_override_superseded`). Tests 4 to 6 are `runtime-config`'s `r1_tests`.
//!
//! Workflow #429 R-2 (A.1 契约 2; register §1.220; since 0.12.31), test 4: applying a preset is one
//! `ConfigureSettings` of its members, every key at once and one `host.config_overrides` record,
//! or none of them.
//!
//! Workflow #429 R-4 (A.6 rules 2, 3 and 5; register §1.239; since 0.12.36), tests 1 to 3, 6 and
//! 7: `ConfigureSettings.expected_version` is a compare-and-swap of the settings' version, the
//! receipt and the snapshot answer it, and a key the start does not know is dropped into
//! `StartReport.instructions_dropped`. The receipts of D1-M3, D1-M7, R-1 test 1 and R-1 test 2
//! carry that version from R-4 on (before, none).

use super::*;
use actingcommand_contract::outcome::Detail;
use actingcommand_contract::{
    CONFIG_OVERRIDES_FACT_KEY, ConfigParameter, ConfigParameterSource, ConfigSettingChange,
    ConfiguredSetting, FactScalar, RuntimeConfigManifest, RuntimeOperationKind, RuntimePayload,
    config_parameters_fact_key,
};
use actingcommand_runtime_config::frame_retention::EVIDENCE_MAX_AGE_DAYS;
use actingcommand_runtime_config::performance::pacing::{
    HEAVY_DISPATCH_LIMIT_PAUSED, MAX_CONCURRENT_RUNS,
};
use actingcommand_runtime_config::performance::presets;
use actingcommand_runtime_config::scheduling::{
    INSTANCE_BLACKOUT, REMEDY_CHOICES, REMEDY_ENABLED, REMEDY_MAX_PER_ROUND,
};
use actingcommand_runtime_config::{
    ConfigCode, DefaultsFile, OVERRIDE_BASE_JSON, OVERRIDE_BASE_SOURCE, OVERRIDE_CONFIG_KEY,
    STUCK_RECOVERY, STUCK_RECOVERY_COOLDOWN_SECS, canonical_json, instance_key, mechanism_rows,
    render_parameters,
};
use serde_json::{Value, json};

const ALIAS: &str = "node.a";

fn instance_text(instance: InstanceId) -> String {
    match serde_json::to_value(instance).expect("instance id encodes") {
        Value::String(text) => text,
        other => panic!("an instance id is a string: {other}"),
    }
}

fn parameter(key: &str, value: FactScalar, source: ConfigParameterSource) -> ConfigParameter {
    ConfigParameter {
        key: key.to_owned(),
        value,
        source,
        settable: false,
    }
}

/// A manifest of the per-instance keys given, and `bind_port`, a key the Runtime reads only at
/// start (`settable` comes from the key table).
fn manifest(parameters: Vec<ConfigParameter>) -> RuntimeConfigManifest {
    let mut parameters = parameters;
    parameters.push(parameter(
        "bind_port",
        FactScalar::Integer(0),
        ConfigParameterSource::Default,
    ));
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

fn start(root: &TempDir, instance: InstanceId, manifest: RuntimeConfigManifest) -> RuntimeHost {
    RuntimeHost::start(
        config(root).with_config_manifest(manifest),
        Arc::new(FakeProvider::one(
            ALIAS,
            instance,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host")
}

fn set(key: &str, value: Value) -> ConfigSettingChange {
    ConfigSettingChange {
        key: key.to_owned(),
        value: Some(value),
    }
}

fn clear(key: &str) -> ConfigSettingChange {
    ConfigSettingChange {
        key: key.to_owned(),
        value: None,
    }
}

fn configure(
    client: &mut TestClient,
    changes: Vec<ConfigSettingChange>,
) -> (RuntimeRequest, RuntimeReceipt) {
    let request = client.request(RuntimeOperation::ConfigureSettings {
        changes,
        expected_version: None,
    });
    let receipt = client.send(&request);
    (request, receipt)
}

/// The rows of one runtime fact, from the Runtime's fact snapshot.
fn fact_rows(client: &mut TestClient, key: &str) -> Option<Vec<BTreeMap<String, FactScalar>>> {
    let request = client.request(RuntimeOperation::RuntimeFactSnapshot);
    let receipt = client.send(&request);
    let Some(RuntimeResult::RuntimeFactSnapshot { snapshot, .. }) = receipt.result() else {
        panic!("expected the runtime fact snapshot: {receipt:?}");
    };
    snapshot
        .records
        .iter()
        .find(|record| record.key == key)
        .map(|record| match &record.value {
            ContractFactValue::RecordList(rows) => rows.clone(),
            other => panic!("{key} is a record list: {other:?}"),
        })
}

/// Workflow #429 R-4: the settings' version the Runtime's fact snapshot answers now (its
/// `config_version`, beside the snapshot).
fn snapshot_version(client: &mut TestClient) -> u64 {
    let request = client.request(RuntimeOperation::RuntimeFactSnapshot);
    let receipt = client.send(&request);
    let Some(RuntimeResult::RuntimeFactSnapshot {
        config_version: Some(version),
        ..
    }) = receipt.result()
    else {
        panic!("expected the runtime fact snapshot with its config_version: {receipt:?}");
    };
    *version
}

/// One key's `config.parameters.<section>` row: `(value, source, settable)`.
fn reported(client: &mut TestClient, key: &str) -> (FactScalar, String, bool) {
    let section = actingcommand_contract::config_parameter_section(key);
    let rows = fact_rows(client, &config_parameters_fact_key(section))
        .unwrap_or_else(|| panic!("no config.parameters.{section}"));
    let row = rows
        .iter()
        .find(|row| row.get("key") == Some(&FactScalar::String(key.to_owned())))
        .unwrap_or_else(|| panic!("no row {key}: {rows:?}"));
    let FactScalar::String(source) = row.get("source").expect("source").clone() else {
        panic!("source is a string");
    };
    let FactScalar::Boolean(settable) = row.get("settable").expect("settable").clone() else {
        panic!("settable is a boolean");
    };
    (row.get("value").expect("value").clone(), source, settable)
}

/// Every `runtime.fact_recorded` of `key` with its sequence.
fn recorded_facts(client: &mut TestClient, key: &str) -> Vec<u64> {
    projected_events(
        client,
        EventQuery {
            event_type: Some(EventType::RuntimeFactRecorded),
            ..EventQuery::default()
        },
    )
    .into_iter()
    .filter(|event| match &event.payload {
        ProjectionPayload::Full(payload) => matches!(
            payload.as_ref(),
            EventPayload::Runtime(RuntimePayload::FactRecorded(recorded))
                if recorded.record().key == key
        ),
        _ => false,
    })
    .map(|event| event.sequence)
    .collect()
}

/// The command records (`command.received`, `command.validated`, `command.rejected`) among
/// `events`; the counter's own record is not the Runtime's (§5.7).
fn command_records(
    events: Vec<actingcommand_contract::ProjectedEvent>,
) -> Vec<actingcommand_contract::ProjectedEvent> {
    events
        .into_iter()
        .filter(|event| {
            matches!(
                event.event_type,
                EventType::CommandReceived
                    | EventType::CommandValidated
                    | EventType::CommandRejected
            )
        })
        .collect()
}

/// The `(code, config_key)` of every `runtime.failed` whose outcome is a configuration code.
fn configuration_notes(client: &mut TestClient) -> Vec<(String, String)> {
    projected_events(
        client,
        EventQuery {
            event_type: Some(EventType::RuntimeFailed),
            ..EventQuery::default()
        },
    )
    .into_iter()
    .filter_map(|event| {
        let ProjectionPayload::Full(payload) = &event.payload else {
            return None;
        };
        let outcome = payload.outcome()?;
        let config_key = outcome.value("config_key")?.as_str()?.to_owned();
        Some((outcome.code().as_str().to_owned(), config_key))
    })
    .collect()
}

#[test]
fn d1_m3_a_settable_key_changes_at_once_with_its_command_and_two_value_records() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let id = instance_text(instance);
    let cooldown = instance_key(&id, STUCK_RECOVERY_COOLDOWN_SECS);
    let enabled = instance_key(&id, STUCK_RECOVERY);
    let host = start(
        &root,
        instance,
        manifest(vec![
            parameter(
                &enabled,
                FactScalar::Boolean(true),
                ConfigParameterSource::Default,
            ),
            parameter(
                &cooldown,
                FactScalar::Integer(600),
                ConfigParameterSource::Default,
            ),
        ]),
    );
    let mut client = TestClient::connect(&host);
    assert_eq!(
        reported(&mut client, &cooldown),
        (FactScalar::Integer(600), "default".to_owned(), true)
    );

    let (request, receipt) = configure(&mut client, vec![set(&cooldown, json!(120))]);
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    // Receipt v2 (Workflow #378 A4, train 5): a completed receipt carries its kind's success code.
    assert_eq!(
        Some(receipt.outcome().code().as_str()),
        RuntimeOperationKind::ConfigureSettings
            .success_code()
            .map(|code| code.as_str())
    );
    assert_eq!(
        receipt.result(),
        Some(&RuntimeResult::SettingsConfigured {
            settings: vec![ConfiguredSetting {
                key: cooldown.clone(),
                value: FactScalar::Integer(120),
                source: ConfigParameterSource::Instruction,
            }],
            // Workflow #429 R-4: the settings' version after the change, the snapshot's now.
            config_version: Some(snapshot_version(&mut client)),
        })
    );
    // The value in effect: what the ladder reads now, and what `config.parameters` reports.
    assert_eq!(
        host.stuck_recovery_for_test(instance)
            .expect("stuck recovery")
            .cooldown_secs,
        120
    );
    assert_eq!(
        reported(&mut client, &cooldown),
        (FactScalar::Integer(120), "instruction".to_owned(), true)
    );
    // The command record, and between its receipt and its validation the two value records:
    // `host.config_overrides` and the changed section.
    let commands = command_records(projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(request.correlation_id()),
            ..EventQuery::default()
        },
    ));
    let types = commands
        .iter()
        .map(|event| event.event_type)
        .collect::<Vec<_>>();
    assert_eq!(
        types,
        vec![EventType::CommandReceived, EventType::CommandValidated]
    );
    let (received, validated) = (commands[0].sequence, commands[1].sequence);
    let between = |sequences: Vec<u64>| {
        sequences
            .into_iter()
            .filter(|sequence| (received..validated).contains(sequence))
            .count()
    };
    assert_eq!(
        between(recorded_facts(&mut client, CONFIG_OVERRIDES_FACT_KEY)),
        1
    );
    assert_eq!(
        between(recorded_facts(
            &mut client,
            &config_parameters_fact_key("instance")
        )),
        1
    );
    let overrides = fact_rows(&mut client, CONFIG_OVERRIDES_FACT_KEY).expect("overrides");
    assert_eq!(overrides.len(), 1);
    assert_eq!(
        overrides[0].get(OVERRIDE_CONFIG_KEY),
        Some(&FactScalar::String(cooldown.clone()))
    );
    assert_eq!(
        overrides[0].get(OVERRIDE_BASE_JSON),
        Some(&FactScalar::String("600".to_owned()))
    );
    assert_eq!(
        overrides[0].get(OVERRIDE_BASE_SOURCE),
        Some(&FactScalar::String("default".to_owned()))
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn d1_m4_one_invalid_change_refuses_the_whole_request_with_one_rejected_command() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let id = instance_text(instance);
    let cooldown = instance_key(&id, STUCK_RECOVERY_COOLDOWN_SECS);
    let enabled = instance_key(&id, STUCK_RECOVERY);
    let host = start(
        &root,
        instance,
        manifest(vec![
            parameter(
                &enabled,
                FactScalar::Boolean(true),
                ConfigParameterSource::Default,
            ),
            parameter(
                &cooldown,
                FactScalar::Integer(300),
                ConfigParameterSource::Explicit,
            ),
        ]),
    );
    let mut client = TestClient::connect(&host);
    let overrides_before = recorded_facts(&mut client, CONFIG_OVERRIDES_FACT_KEY).len();
    let sections_before =
        recorded_facts(&mut client, &config_parameters_fact_key("instance")).len();

    // A valid change and one of the wrong type; then a valid one and one out of range.
    for invalid in [set(&enabled, json!("off")), set(&enabled, json!(1))] {
        let (request, receipt) = configure(&mut client, vec![set(&cooldown, json!(120)), invalid]);
        assert_eq!(receipt.state(), RuntimeReceiptState::Denied);
        assert_eq!(
            Some(receipt.outcome().code().as_str()),
            Some(ConfigCode::ConfigValueInvalid.as_str())
        );
        // Receipt v2 (Workflow #378 A4, train 5): the refusal tops the receipt with its own
        // values, its Runtime error code, its operation and the halt.
        let outcome = receipt.outcome();
        assert_eq!(outcome.value("config_key"), Some(&json!(enabled)));
        assert_eq!(
            outcome.value("runtime_code"),
            Some(&json!("invalid_request"))
        );
        assert_eq!(
            outcome.value("operation"),
            Some(&json!("configure_settings"))
        );
        assert_eq!(outcome.value("host_halted"), Some(&json!(false)));
        let commands = command_records(projected_events(
            &mut client,
            EventQuery {
                correlation_id: Some(request.correlation_id()),
                ..EventQuery::default()
            },
        ))
        .into_iter()
        .map(|event| event.event_type)
        .collect::<Vec<_>>();
        assert_eq!(
            commands,
            vec![EventType::CommandReceived, EventType::CommandRejected]
        );
    }
    let (_, receipt) = configure(
        &mut client,
        vec![set(&enabled, json!(false)), set(&cooldown, json!(0))],
    );
    assert_eq!(
        Some(receipt.outcome().code().as_str()),
        Some(ConfigCode::ConfigValueInvalid.as_str())
    );
    // Neither key changed, and no value record was written.
    assert_eq!(
        reported(&mut client, &cooldown),
        (FactScalar::Integer(300), "explicit".to_owned(), true)
    );
    assert_eq!(
        reported(&mut client, &enabled),
        (FactScalar::Boolean(true), "default".to_owned(), true)
    );
    assert_eq!(
        host.stuck_recovery_for_test(instance)
            .expect("stuck recovery")
            .cooldown_secs,
        300
    );
    assert_eq!(
        recorded_facts(&mut client, CONFIG_OVERRIDES_FACT_KEY).len(),
        overrides_before
    );
    assert_eq!(
        recorded_facts(&mut client, &config_parameters_fact_key("instance")).len(),
        sections_before
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn d1_m5_a_key_read_only_at_start_and_an_unknown_key_are_refused() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let id = instance_text(instance);
    let cooldown = instance_key(&id, STUCK_RECOVERY_COOLDOWN_SECS);
    let host = start(
        &root,
        instance,
        manifest(vec![parameter(
            &cooldown,
            FactScalar::Integer(600),
            ConfigParameterSource::Default,
        )]),
    );
    let mut client = TestClient::connect(&host);
    assert_eq!(
        reported(&mut client, "bind_port"),
        (FactScalar::Integer(0), "default".to_owned(), false)
    );
    for (changes, code) in [
        (
            vec![set("bind_port", json!(9_000))],
            ConfigCode::ConfigKeyNotSettable,
        ),
        (
            vec![set(&cooldown, json!(60)), set("bind_port", json!(9_000))],
            ConfigCode::ConfigKeyNotSettable,
        ),
        (
            vec![set("neutral_section.unknown_member", json!(1))],
            ConfigCode::ConfigKeyUnknown,
        ),
        (
            vec![set(&instance_key(&id, "unknown_member"), json!(true))],
            ConfigCode::ConfigKeyUnknown,
        ),
    ] {
        let (_, receipt) = configure(&mut client, changes);
        assert_eq!(receipt.state(), RuntimeReceiptState::Denied);
        assert_eq!(Some(receipt.outcome().code().as_str()), Some(code.as_str()));
    }
    assert_eq!(
        reported(&mut client, &cooldown),
        (FactScalar::Integer(600), "default".to_owned(), true)
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn d1_m6_a_restart_restores_instruction_values_and_the_later_change_wins() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let a = instance_text(instance);
    let b = instance_text(instance_id());
    let c = instance_text(instance_id());
    let (cooldown_a, enabled_a) = (
        instance_key(&a, STUCK_RECOVERY_COOLDOWN_SECS),
        instance_key(&a, STUCK_RECOVERY),
    );
    let (cooldown_b, enabled_b) = (
        instance_key(&b, STUCK_RECOVERY_COOLDOWN_SECS),
        instance_key(&b, STUCK_RECOVERY),
    );
    let cooldown_c = instance_key(&c, STUCK_RECOVERY_COOLDOWN_SECS);
    let host = start(
        &root,
        instance,
        manifest(vec![
            parameter(
                &enabled_a,
                FactScalar::Boolean(true),
                ConfigParameterSource::Default,
            ),
            parameter(
                &cooldown_a,
                FactScalar::Integer(300),
                ConfigParameterSource::Explicit,
            ),
            parameter(
                &enabled_b,
                FactScalar::Boolean(true),
                ConfigParameterSource::Default,
            ),
            parameter(
                &cooldown_b,
                FactScalar::Integer(600),
                ConfigParameterSource::Default,
            ),
            parameter(
                &cooldown_c,
                FactScalar::Integer(600),
                ConfigParameterSource::Default,
            ),
        ]),
    );
    let mut client = TestClient::connect(&host);
    let (_, receipt) = configure(
        &mut client,
        vec![
            set(&cooldown_a, json!(120)),
            set(&enabled_a, json!(false)),
            set(&cooldown_b, json!(60)),
            set(&enabled_b, json!(false)),
            set(&cooldown_c, json!(30)),
        ],
    );
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    drop(client);
    host.close().expect("close host");

    // The file changed a's explicit cool-down and now names a's flag; b's cool-down default
    // changed while the file still does not name it; b's flag is as it was; c is gone.
    let host = start(
        &root,
        instance,
        manifest(vec![
            parameter(
                &enabled_a,
                FactScalar::Boolean(true),
                ConfigParameterSource::Explicit,
            ),
            parameter(
                &cooldown_a,
                FactScalar::Integer(450),
                ConfigParameterSource::Explicit,
            ),
            parameter(
                &enabled_b,
                FactScalar::Boolean(true),
                ConfigParameterSource::Default,
            ),
            parameter(
                &cooldown_b,
                FactScalar::Integer(900),
                ConfigParameterSource::Default,
            ),
        ]),
    );
    let mut client = TestClient::connect(&host);
    assert_eq!(
        configuration_notes(&mut client),
        vec![
            (
                ConfigCode::ConfigOverrideSuperseded.as_str().to_owned(),
                enabled_a.clone()
            ),
            (
                ConfigCode::ConfigOverrideSuperseded.as_str().to_owned(),
                cooldown_a.clone()
            ),
            (
                ConfigCode::ConfigOverrideBaseChanged.as_str().to_owned(),
                cooldown_b.clone()
            ),
            (
                ConfigCode::ConfigOverrideDropped.as_str().to_owned(),
                cooldown_c.clone()
            ),
        ]
    );
    assert_eq!(
        reported(&mut client, &cooldown_a),
        (FactScalar::Integer(450), "explicit".to_owned(), true)
    );
    assert_eq!(
        reported(&mut client, &enabled_a),
        (FactScalar::Boolean(true), "explicit".to_owned(), true)
    );
    assert_eq!(
        reported(&mut client, &cooldown_b),
        (FactScalar::Integer(60), "instruction".to_owned(), true)
    );
    assert_eq!(
        reported(&mut client, &enabled_b),
        (FactScalar::Boolean(false), "instruction".to_owned(), true)
    );
    assert_eq!(
        host.stuck_recovery_for_test(instance)
            .expect("stuck recovery")
            .cooldown_secs,
        450
    );
    // The restored values are recorded again: b's two, b's cool-down now over the new default.
    let overrides = fact_rows(&mut client, CONFIG_OVERRIDES_FACT_KEY).expect("overrides");
    assert_eq!(
        overrides
            .iter()
            .map(|row| row.get(OVERRIDE_CONFIG_KEY).cloned())
            .collect::<Vec<_>>(),
        vec![
            Some(FactScalar::String(enabled_b.clone())),
            Some(FactScalar::String(cooldown_b.clone())),
        ]
    );
    assert_eq!(
        overrides[1].get(OVERRIDE_BASE_JSON),
        Some(&FactScalar::String("900".to_owned()))
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn d1_m7_clear_returns_a_key_to_its_file_value_and_source() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let id = instance_text(instance);
    let cooldown = instance_key(&id, STUCK_RECOVERY_COOLDOWN_SECS);
    let enabled = instance_key(&id, STUCK_RECOVERY);
    let host = start(
        &root,
        instance,
        manifest(vec![
            parameter(
                &enabled,
                FactScalar::Boolean(true),
                ConfigParameterSource::Default,
            ),
            parameter(
                &cooldown,
                FactScalar::Integer(300),
                ConfigParameterSource::Explicit,
            ),
        ]),
    );
    let mut client = TestClient::connect(&host);
    let (_, receipt) = configure(
        &mut client,
        vec![set(&cooldown, json!(120)), set(&enabled, json!(false))],
    );
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    let (_, receipt) = configure(&mut client, vec![clear(&cooldown), clear(&enabled)]);
    assert_eq!(
        receipt.result(),
        Some(&RuntimeResult::SettingsConfigured {
            settings: vec![
                ConfiguredSetting {
                    key: cooldown.clone(),
                    value: FactScalar::Integer(300),
                    source: ConfigParameterSource::Explicit,
                },
                ConfiguredSetting {
                    key: enabled.clone(),
                    value: FactScalar::Boolean(true),
                    source: ConfigParameterSource::Default,
                },
            ],
            // Workflow #429 R-4: the settings' version after the change, the snapshot's now.
            config_version: Some(snapshot_version(&mut client)),
        })
    );
    assert_eq!(
        reported(&mut client, &cooldown),
        (FactScalar::Integer(300), "explicit".to_owned(), true)
    );
    assert_eq!(
        reported(&mut client, &enabled),
        (FactScalar::Boolean(true), "default".to_owned(), true)
    );
    assert_eq!(
        fact_rows(&mut client, CONFIG_OVERRIDES_FACT_KEY),
        Some(Vec::new())
    );
    assert_eq!(
        host.stuck_recovery_for_test(instance)
            .expect("stuck recovery")
            .cooldown_secs,
        300
    );
    drop(client);
    host.close().expect("close host");
}

// Workflow #411 B15 R3 (model-b15 §5.2; register §1.113; host key 112), requirements 1 and 4:
// the four remedy keys, as actingd assembles them from the shipped defaults file, are recorded in
// `config.parameters.scheduling` and, for the instance, `config.parameters.instance`, each with
// its value, source `default` and `settable`; one `ConfigureSettings` changes all four at once
// and the rows then report the instruction's values (a list as its canonical JSON text).
#[test]
fn b15c_remedy_keys_are_settable_and_recorded_as_rows() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let id = instance_text(instance);
    let own = instance_key(&id, REMEDY_CHOICES);
    let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults file parses");
    let remedy_keys = [
        REMEDY_ENABLED,
        REMEDY_MAX_PER_ROUND,
        REMEDY_CHOICES,
        own.as_str(),
    ];
    let parameters = render_parameters(
        &mechanism_rows(),
        std::slice::from_ref(&id),
        &defaults,
        &json!({"instances": [{"instance_id": id, "alias": ALIAS}]}),
        &|_| None,
    )
    .expect("the defaults resolve")
    .into_iter()
    .filter(|parameter| remedy_keys.contains(&parameter.key.as_str()))
    .collect::<Vec<_>>();
    assert_eq!(parameters.len(), remedy_keys.len(), "{parameters:?}");
    let host = start(&root, instance, manifest(parameters));
    let mut client = TestClient::connect(&host);
    for (key, value) in [
        (REMEDY_ENABLED, FactScalar::Boolean(true)),
        (REMEDY_MAX_PER_ROUND, FactScalar::Integer(2)),
        (REMEDY_CHOICES, FactScalar::String("[]".to_owned())),
        (own.as_str(), FactScalar::String("[]".to_owned())),
    ] {
        assert_eq!(
            reported(&mut client, key),
            (value, "default".to_owned(), true),
            "{key}"
        );
    }

    let (_, receipt) = configure(
        &mut client,
        vec![
            set(REMEDY_ENABLED, json!(false)),
            set(REMEDY_MAX_PER_ROUND, json!(3)),
            set(REMEDY_CHOICES, json!([{"row": "none", "need": "need_b"}])),
            set(
                &own,
                json!([{"row": "allow", "need": "need_a", "capability": "cap_x"}]),
            ),
        ],
    );
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    for (key, value) in [
        (REMEDY_ENABLED, FactScalar::Boolean(false)),
        (REMEDY_MAX_PER_ROUND, FactScalar::Integer(3)),
        (
            REMEDY_CHOICES,
            FactScalar::String(r#"[{"need":"need_b","row":"none"}]"#.to_owned()),
        ),
        (
            own.as_str(),
            FactScalar::String(
                r#"[{"capability":"cap_x","need":"need_a","row":"allow"}]"#.to_owned(),
            ),
        ),
    ] {
        assert_eq!(
            reported(&mut client, key),
            (value, "instruction".to_owned(), true),
            "{key}"
        );
    }
    drop(client);
    host.close().expect("close host");
}

// ---- Workflow #429 R-1 (A.6 rule 4; register §1.219; since 0.12.31): a key whose default is
// none that the file does not write, the instance's own blackout list.

/// The R-1 tests' first start, before the entry of `away` begins; each later start is a minute
/// on. The clock is fixed: nothing here reads the system clock.
const R1_START_MS: u64 = 1_700_000_000_000;

/// A host of the fixture instance on `manifest`, its clock fixed at `at_unix_ms`.
fn start_at(
    root: &TempDir,
    instance: InstanceId,
    manifest: RuntimeConfigManifest,
    at_unix_ms: u64,
) -> RuntimeHost {
    RuntimeHost::start(
        config(root)
            .with_config_manifest(manifest)
            .with_runtime_clock(Arc::new(ManualRuntimeClock::new(at_unix_ms, 0))),
        Arc::new(FakeProvider::one(
            ALIAS,
            instance,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host")
}

/// The file layer of the instance `id`: its stuck-recovery cool-down, so the file configures
/// the instance, and its own blackout list only when the file writes one.
fn r1_file(id: &str, blackout: Option<&Value>) -> RuntimeConfigManifest {
    let mut parameters = vec![parameter(
        &instance_key(id, STUCK_RECOVERY_COOLDOWN_SECS),
        FactScalar::Integer(600),
        ConfigParameterSource::Default,
    )];
    if let Some(list) = blackout {
        parameters.push(parameter(
            &instance_key(id, INSTANCE_BLACKOUT),
            FactScalar::String(canonical_json(list)),
            ConfigParameterSource::Explicit,
        ));
    }
    manifest(parameters)
}

/// An own blackout list of one `once` entry (`scheduling-blackout.md`).
fn away() -> Value {
    json!([{"kind": "once", "id": "away", "from_unix_ms": 1_800_000_000_000_u64,
            "until_unix_ms": 1_800_003_600_000_u64}])
}

/// Whether `config.parameters.<section>` has a row of `key`.
fn has_row(client: &mut TestClient, key: &str) -> bool {
    let section = actingcommand_contract::config_parameter_section(key);
    fact_rows(client, &config_parameters_fact_key(section))
        .unwrap_or_default()
        .iter()
        .any(|row| row.get("key") == Some(&FactScalar::String(key.to_owned())))
}

// R-1 test 1 (with test 5's row shape as the host records it): an instance whose file writes
// no blackout list of its own gets one by instruction. The receipt, `config.parameters.instance`
// and the blackout input of the policy cycle carry it, and `host.config_overrides` holds one row
// over the base `default` / `null`; the next start over the same file restores it with no note.
#[test]
fn r1_t1_an_instance_without_a_blackout_list_in_the_file_gets_one_online_that_survives_a_restart() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let id = instance_text(instance);
    let blackout = instance_key(&id, INSTANCE_BLACKOUT);
    let list = away();
    let host = start_at(&root, instance, r1_file(&id, None), R1_START_MS);
    let mut client = TestClient::connect(&host);
    assert!(!has_row(&mut client, &blackout));
    assert_eq!(
        host.own_blackout_entries_for_test(ALIAS)
            .expect("the blackout input"),
        0
    );

    let (_, receipt) = configure(&mut client, vec![set(&blackout, list.clone())]);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(
        receipt.result(),
        Some(&RuntimeResult::SettingsConfigured {
            settings: vec![ConfiguredSetting {
                key: blackout.clone(),
                value: FactScalar::String(canonical_json(&list)),
                source: ConfigParameterSource::Instruction,
            }],
            // Workflow #429 R-4: the settings' version after the change, the snapshot's now.
            config_version: Some(snapshot_version(&mut client)),
        })
    );
    let in_effect = (
        FactScalar::String(canonical_json(&list)),
        "instruction".to_owned(),
        true,
    );
    assert_eq!(reported(&mut client, &blackout), in_effect);
    assert_eq!(
        host.own_blackout_entries_for_test(ALIAS)
            .expect("the blackout input"),
        1
    );
    let overrides = fact_rows(&mut client, CONFIG_OVERRIDES_FACT_KEY).expect("overrides");
    assert_eq!(overrides.len(), 1);
    assert_eq!(
        overrides[0].get(OVERRIDE_CONFIG_KEY),
        Some(&FactScalar::String(blackout.clone()))
    );
    assert_eq!(
        overrides[0].get(OVERRIDE_BASE_SOURCE),
        Some(&FactScalar::String("default".to_owned()))
    );
    assert_eq!(
        overrides[0].get(OVERRIDE_BASE_JSON),
        Some(&FactScalar::String("null".to_owned()))
    );
    drop(client);
    host.close().expect("close host");

    let host = start_at(&root, instance, r1_file(&id, None), R1_START_MS + 60_000);
    let mut client = TestClient::connect(&host);
    assert_eq!(
        configuration_notes(&mut client),
        Vec::<(String, String)>::new()
    );
    assert_eq!(reported(&mut client, &blackout), in_effect);
    assert_eq!(
        host.own_blackout_entries_for_test(ALIAS)
            .expect("the blackout input"),
        1
    );
    assert_eq!(
        fact_rows(&mut client, CONFIG_OVERRIDES_FACT_KEY),
        Some(overrides)
    );
    drop(client);
    host.close().expect("close host");
}

// R-1 test 2: clearing that instruction returns the instance to no blackout list of its own. The
// receipt names the key as not in effect (`null` text from `default`, the base it was set over),
// `config.parameters` has no row of it, `host.config_overrides` is empty and the policy cycle's
// input has no entry of the instance; the next start keeps it so.
#[test]
fn r1_t2_clearing_it_returns_the_instance_to_no_blackout_list() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let id = instance_text(instance);
    let blackout = instance_key(&id, INSTANCE_BLACKOUT);
    let host = start_at(&root, instance, r1_file(&id, None), R1_START_MS);
    let mut client = TestClient::connect(&host);
    let (_, receipt) = configure(&mut client, vec![set(&blackout, away())]);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(
        host.own_blackout_entries_for_test(ALIAS)
            .expect("the blackout input"),
        1
    );

    let (_, receipt) = configure(&mut client, vec![clear(&blackout)]);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(
        receipt.result(),
        Some(&RuntimeResult::SettingsConfigured {
            settings: vec![ConfiguredSetting {
                key: blackout.clone(),
                value: FactScalar::String("null".to_owned()),
                source: ConfigParameterSource::Default,
            }],
            // Workflow #429 R-4: the settings' version after the change, the snapshot's now.
            config_version: Some(snapshot_version(&mut client)),
        })
    );
    assert!(!has_row(&mut client, &blackout));
    assert_eq!(
        fact_rows(&mut client, CONFIG_OVERRIDES_FACT_KEY),
        Some(Vec::new())
    );
    assert_eq!(
        host.own_blackout_entries_for_test(ALIAS)
            .expect("the blackout input"),
        0
    );
    drop(client);
    host.close().expect("close host");

    let host = start_at(&root, instance, r1_file(&id, None), R1_START_MS + 60_000);
    let mut client = TestClient::connect(&host);
    assert_eq!(
        configuration_notes(&mut client),
        Vec::<(String, String)>::new()
    );
    assert!(!has_row(&mut client, &blackout));
    assert_eq!(
        host.own_blackout_entries_for_test(ALIAS)
            .expect("the blackout input"),
        0
    );
    drop(client);
    host.close().expect("close host");
}

// R-1 test 3: once the file writes the key, the next start voids the instruction set over its
// absence, `config_override_superseded` naming the key: the file's list is in effect, as the
// file wrote it, and `host.config_overrides` keeps no row.
#[test]
fn r1_t3_a_file_that_later_writes_the_key_voids_the_instruction_as_superseded() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let id = instance_text(instance);
    let blackout = instance_key(&id, INSTANCE_BLACKOUT);
    let host = start_at(&root, instance, r1_file(&id, None), R1_START_MS);
    let mut client = TestClient::connect(&host);
    let (_, receipt) = configure(&mut client, vec![set(&blackout, away())]);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    drop(client);
    host.close().expect("close host");

    let host = start_at(
        &root,
        instance,
        r1_file(&id, Some(&json!([]))),
        R1_START_MS + 60_000,
    );
    let mut client = TestClient::connect(&host);
    assert_eq!(
        configuration_notes(&mut client),
        vec![(
            ConfigCode::ConfigOverrideSuperseded.as_str().to_owned(),
            blackout.clone()
        )]
    );
    assert_eq!(
        reported(&mut client, &blackout),
        (
            FactScalar::String("[]".to_owned()),
            "explicit".to_owned(),
            true
        )
    );
    assert_eq!(
        host.own_blackout_entries_for_test(ALIAS)
            .expect("the blackout input"),
        0
    );
    assert_eq!(
        fact_rows(&mut client, CONFIG_OVERRIDES_FACT_KEY),
        Some(Vec::new())
    );
    drop(client);
    host.close().expect("close host");
}

// ---- Workflow #429 R-2 (A.1 契约 2; register §1.220; since 0.12.31): applying a preset.

/// The file layer of every section a start assembles from the shipped defaults and `file`.
fn r2_file(file: Value) -> RuntimeConfigManifest {
    let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults");
    let parameters = render_parameters(&mechanism_rows(), &[], &defaults, &file, &|_| None)
        .expect("every key resolves");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// The parameters in effect the current preset reads: the rows of `config.parameters.performance`
/// and `config.parameters.scheduling`, as the three counters read them.
fn r2_in_effect(client: &mut TestClient) -> Vec<ConfigParameter> {
    ["performance", "scheduling"]
        .into_iter()
        .flat_map(|section| {
            fact_rows(client, &config_parameters_fact_key(section))
                .unwrap_or_else(|| panic!("config.parameters.{section}"))
        })
        .map(|row| presets::parameter_of_row(&row).unwrap_or_else(|| panic!("a row: {row:?}")))
        .collect()
}

/// The current preset of the parameters in effect, by the shared function.
fn r2_current(client: &mut TestClient) -> String {
    let parameters = r2_in_effect(client);
    let table = presets::table_of(&parameters)
        .expect("the preset table's row")
        .expect("the table reads back");
    presets::current_preset(&table, &parameters).preset
}

/// The changes of the preset `name` as the three counters build them, from the table in effect.
fn r2_preset_changes(client: &mut TestClient, name: &str) -> Vec<ConfigSettingChange> {
    presets::table_of(&r2_in_effect(client))
        .expect("the preset table's row")
        .expect("the table reads back")
        .get(name)
        .unwrap_or_else(|| panic!("the preset {name}"))
        .changes()
}

// R-2 test 4: applying `light`, then `standard`, over a file that writes none of the members. Each
// is one `ConfigureSettings`: every member's key changes at once (the receipt names each, in the
// mapping table's order) and `host.config_overrides` is recorded exactly once, its rows the
// members the preset writes (`standard` leaves `max_concurrent_runs` unwritten, so its change
// clears `light`'s value: receipt `null` from `default`, no row). The current preset follows.
#[test]
fn r2_t4_applying_a_preset_sets_every_member_at_once_with_one_overrides_record() {
    let root = TempDir::new().expect("tempdir");
    let host = start_at(&root, instance_id(), r2_file(json!({})), R1_START_MS);
    let mut client = TestClient::connect(&host);
    assert_eq!(r2_current(&mut client), "adaptive");
    assert!(recorded_facts(&mut client, CONFIG_OVERRIDES_FACT_KEY).is_empty());

    for (name, records, rows, max) in [
        ("light", 1, 4, FactScalar::Integer(1)),
        ("standard", 2, 3, FactScalar::String("null".to_owned())),
    ] {
        let changes = r2_preset_changes(&mut client, name);
        let keys = changes
            .iter()
            .map(|change| change.key.clone())
            .collect::<Vec<_>>();
        let (_, receipt) = configure(&mut client, changes);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{name}: {receipt:?}"
        );
        let Some(RuntimeResult::SettingsConfigured { settings, .. }) = receipt.result() else {
            panic!("{name}: the settings configured: {receipt:?}");
        };
        assert_eq!(
            settings
                .iter()
                .map(|setting| setting.key.clone())
                .collect::<Vec<_>>(),
            keys,
            "{name}"
        );
        assert_eq!(
            settings
                .iter()
                .find(|setting| setting.key == MAX_CONCURRENT_RUNS)
                .map(|setting| setting.value.clone()),
            Some(max),
            "{name}"
        );
        assert_eq!(
            recorded_facts(&mut client, CONFIG_OVERRIDES_FACT_KEY).len(),
            records,
            "{name}: one record per preset"
        );
        let overrides = fact_rows(&mut client, CONFIG_OVERRIDES_FACT_KEY).expect("overrides");
        assert_eq!(overrides.len(), rows, "{name}: {overrides:?}");
        assert_eq!(r2_current(&mut client), name);
    }
    drop(client);
    host.close().expect("close host");
}

// R-2 test 4: one member that cannot be applied refuses the whole preset. The file writes
// `.paused` 2 beside `.normal` 2; `light`'s `.normal` 1 breaks the section (`config_value_invalid`,
// `section_check`, naming `.paused`), so none of its four changes applies: no record of
// `host.config_overrides`, every member as before, the current preset still `adaptive`.
#[test]
fn r2_t4_one_member_that_cannot_be_applied_applies_none() {
    let root = TempDir::new().expect("tempdir");
    let file =
        json!({"performance": {"pacing": {"heavy_dispatch_limit": {"normal": 2, "paused": 2}}}});
    let host = start_at(&root, instance_id(), r2_file(file), R1_START_MS);
    let mut client = TestClient::connect(&host);
    let before = r2_in_effect(&mut client);
    assert_eq!(r2_current(&mut client), "adaptive");

    let changes = r2_preset_changes(&mut client, "light");
    let (_, receipt) = configure(&mut client, changes);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    let outcome = receipt.outcome();
    assert_eq!(
        outcome.code().as_str(),
        ConfigCode::ConfigValueInvalid.as_str()
    );
    assert_eq!(
        outcome.value("config_key").and_then(Value::as_str),
        Some(HEAVY_DISPATCH_LIMIT_PAUSED)
    );
    assert_eq!(
        outcome
            .value("config_value_problem")
            .and_then(Value::as_str),
        Some("section_check")
    );
    assert!(recorded_facts(&mut client, CONFIG_OVERRIDES_FACT_KEY).is_empty());
    assert_eq!(r2_in_effect(&mut client), before);
    assert_eq!(r2_current(&mut client), "adaptive");
    drop(client);
    host.close().expect("close host");
}

// ---- Workflow #429 R-4 (A.6 rules 2, 3 and 5; register §1.239; since 0.12.36): the settings'
// version, `ConfigureSettings` as a compare-and-swap of it, and the keys a rollback drops. Tests 4
// and 5 are `runtime-config`'s `r4_tests`, the CLI and MCP read points `actingctl`'s
// `gate_429_r4`. Every host here runs on the fixed clock of `start_at`.

/// `ConfigureSettings` of `changes`, at `expected_version` when one is given.
fn configure_at(
    client: &mut TestClient,
    changes: Vec<ConfigSettingChange>,
    expected_version: Option<u64>,
) -> (RuntimeRequest, RuntimeReceipt) {
    let request = client.request(RuntimeOperation::ConfigureSettings {
        changes,
        expected_version,
    });
    let receipt = client.send(&request);
    (request, receipt)
}

/// The settings' version a completed `ConfigureSettings` receipt carries.
fn receipt_version(receipt: &RuntimeReceipt) -> u64 {
    let Some(RuntimeResult::SettingsConfigured {
        config_version: Some(version),
        ..
    }) = receipt.result()
    else {
        panic!("expected the settings configured with their config_version: {receipt:?}");
    };
    *version
}

// R-4 test 1: a change at the settings' current version applies, as one without the member does:
// the value is in effect from `instruction`, and the receipt carries the version after the change,
// another one, which is the next `expected_version`: a change at it applies too.
#[test]
fn r4_t1_the_current_expected_version_applies_and_the_receipt_carries_the_new_version() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let id = instance_text(instance);
    let cooldown = instance_key(&id, STUCK_RECOVERY_COOLDOWN_SECS);
    let host = start_at(&root, instance, r1_file(&id, None), R1_START_MS);
    let mut client = TestClient::connect(&host);
    let before = snapshot_version(&mut client);

    let (_, receipt) = configure_at(&mut client, vec![set(&cooldown, json!(120))], Some(before));
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let Some(RuntimeResult::SettingsConfigured { settings, .. }) = receipt.result() else {
        panic!("the settings configured: {receipt:?}");
    };
    assert_eq!(
        *settings,
        vec![ConfiguredSetting {
            key: cooldown.clone(),
            value: FactScalar::Integer(120),
            source: ConfigParameterSource::Instruction,
        }]
    );
    let after = receipt_version(&receipt);
    assert_ne!(after, before, "a committed change has another version");
    assert_eq!(
        reported(&mut client, &cooldown),
        (FactScalar::Integer(120), "instruction".to_owned(), true)
    );
    assert_eq!(
        host.stuck_recovery_for_test(instance)
            .expect("stuck recovery")
            .cooldown_secs,
        120
    );

    let (_, receipt) = configure_at(&mut client, vec![set(&cooldown, json!(90))], Some(after));
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(
        reported(&mut client, &cooldown),
        (FactScalar::Integer(90), "instruction".to_owned(), true)
    );
    drop(client);
    host.close().expect("close host");
}

// R-4 test 2: a change at another version is refused, Denied
// `fact_observation_version_conflict` with `fact_key` `host.config_overrides` and the
// `current_version` now, which a codes-mode receipt (the console's default) keeps too, as it keeps
// `host_halted`. The refusal is the command's `command.rejected`, and nothing changes: the value
// in effect, `config.parameters`, `host.config_overrides` (no record) and the version.
#[test]
fn r4_t2_another_expected_version_is_refused_as_a_version_conflict_and_changes_nothing() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let id = instance_text(instance);
    let cooldown = instance_key(&id, STUCK_RECOVERY_COOLDOWN_SECS);
    let host = start_at(&root, instance, r1_file(&id, None), R1_START_MS);
    let mut client = TestClient::connect(&host);
    let current = snapshot_version(&mut client);
    // Any other token: the version is compared for equality only.
    let other = current ^ 1;
    let reported_before = reported(&mut client, &cooldown);
    let rows_before = fact_rows(&mut client, CONFIG_OVERRIDES_FACT_KEY);
    let records_before = recorded_facts(&mut client, CONFIG_OVERRIDES_FACT_KEY).len();

    for detail in [Detail::Full, Detail::Codes] {
        let request = client
            .request(RuntimeOperation::ConfigureSettings {
                changes: vec![set(&cooldown, json!(120))],
                expected_version: Some(other),
            })
            .with_detail(detail);
        let receipt = client.send(&request);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Denied,
            "{detail:?} {receipt:?}"
        );
        assert_eq!(receipt.outcome_envelope().detail(), detail);
        let outcome = receipt.outcome();
        assert_eq!(
            outcome.code().as_str(),
            "fact_observation_version_conflict",
            "{detail:?}"
        );
        assert_eq!(
            outcome.value("current_version"),
            Some(&json!(current)),
            "{detail:?}: the refusal names the version now"
        );
        assert_eq!(
            outcome.value("operation"),
            Some(&json!("configure_settings")),
            "{detail:?}"
        );
        assert_eq!(
            outcome.value("runtime_code"),
            Some(&json!("invalid_request")),
            "{detail:?}"
        );
        if detail == Detail::Full {
            assert_eq!(
                outcome.value("fact_key"),
                Some(&json!(CONFIG_OVERRIDES_FACT_KEY))
            );
        }
        assert!(receipt.result().is_none(), "{detail:?}");
        let commands = command_records(projected_events(
            &mut client,
            EventQuery {
                correlation_id: Some(request.correlation_id()),
                ..EventQuery::default()
            },
        ))
        .into_iter()
        .map(|event| event.event_type)
        .collect::<Vec<_>>();
        assert_eq!(
            commands,
            vec![EventType::CommandReceived, EventType::CommandRejected],
            "{detail:?}"
        );
    }

    assert_eq!(snapshot_version(&mut client), current);
    assert_eq!(reported(&mut client, &cooldown), reported_before);
    assert_eq!(
        fact_rows(&mut client, CONFIG_OVERRIDES_FACT_KEY),
        rows_before
    );
    assert_eq!(
        recorded_facts(&mut client, CONFIG_OVERRIDES_FACT_KEY).len(),
        records_before
    );
    assert_eq!(
        host.stuck_recovery_for_test(instance)
            .expect("stuck recovery")
            .cooldown_secs,
        600
    );
    drop(client);
    host.close().expect("close host");
}

// R-4 test 3: without `expected_version` nothing is compared, as before R-4: after another client
// changed the key at the version both read, this client's change still applies, the last writer
// wins, and its receipt carries the version after it.
#[test]
fn r4_t3_a_change_without_expected_version_applies_as_before_the_last_writer_wins() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let id = instance_text(instance);
    let cooldown = instance_key(&id, STUCK_RECOVERY_COOLDOWN_SECS);
    let host = start_at(&root, instance, r1_file(&id, None), R1_START_MS);
    let mut client = TestClient::connect(&host);
    let mut other = TestClient::connect(&host);
    let read = snapshot_version(&mut client);

    let (_, receipt) = configure_at(&mut other, vec![set(&cooldown, json!(120))], Some(read));
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let (_, receipt) = configure(&mut client, vec![set(&cooldown, json!(90))]);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(
        reported(&mut client, &cooldown),
        (FactScalar::Integer(90), "instruction".to_owned(), true)
    );
    assert_eq!(receipt_version(&receipt), snapshot_version(&mut client));
    drop(other);
    drop(client);
    host.close().expect("close host");
}

/// The fixed sample of R-4 test 6: a `host.config_overrides` value as a later Runtime records it
/// (`FactValue`, rows in manifest order, fields in key order), with one row of a key this build
/// knows (`frame_retention_evidence_max_age_days`, 30 over its default 15) and one of a key no
/// row of this build's key table has (`neutral_later_release.switch`).
const LATER_RELEASE_ROWS: &str = concat!(
    r#"{"type":"record_list","value":["#,
    r#"{"base_json":{"type":"string","value":"15"},"#,
    r#""base_source":{"type":"string","value":"default"},"#,
    r#""config_key":{"type":"string","value":"frame_retention_evidence_max_age_days"},"#,
    r#""request_id":{"type":"string","value":"request_later"},"#,
    r#""set_in_owner_epoch":{"type":"string","value":"epoch_later"},"#,
    r#""set_unix_ms":{"type":"timestamp_ms","value":1700000000000},"#,
    r#""value_json":{"type":"string","value":"30"}},"#,
    r#"{"base_json":{"type":"string","value":"false"},"#,
    r#""base_source":{"type":"string","value":"default"},"#,
    r#""config_key":{"type":"string","value":"neutral_later_release.switch"},"#,
    r#""request_id":{"type":"string","value":"request_later"},"#,
    r#""set_in_owner_epoch":{"type":"string","value":"epoch_later"},"#,
    r#""set_unix_ms":{"type":"timestamp_ms","value":1700000000000},"#,
    r#""value_json":{"type":"string","value":"true"}}"#,
    r#"]}"#,
);

/// The Runtime's report of its own start.
fn start_report(client: &mut TestClient) -> actingcommand_contract::RuntimeStartReport {
    let request = client.request(RuntimeOperation::StartReport);
    let receipt = client.send(&request);
    let Some(RuntimeResult::StartReport { report }) = receipt.result() else {
        panic!("expected the start report: {receipt:?}");
    };
    (**report).clone()
}

// R-4 test 6 (rollback, A.6 rule 5): the start of a Runtime that does not know a key a later one
// set restores the record by its own key table. The unknown key is dropped
// (`config_override_dropped`), named in `StartReport.instructions_dropped` and gone from the
// recorded `host.config_overrides`; the known key is restored and counted; the record is never
// refused.
#[test]
fn r4_t6_a_key_this_runtime_does_not_know_is_dropped_at_start_into_instructions_dropped() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let id = instance_text(instance);
    // The exact key first, then the instance's, as the start-up manifest orders them.
    let file = manifest(vec![
        parameter(
            EVIDENCE_MAX_AGE_DAYS,
            FactScalar::Integer(15),
            ConfigParameterSource::Default,
        ),
        parameter(
            &instance_key(&id, STUCK_RECOVERY_COOLDOWN_SECS),
            FactScalar::Integer(600),
            ConfigParameterSource::Default,
        ),
    ]);
    let ContractFactValue::RecordList(rows) =
        serde_json::from_str::<ContractFactValue>(LATER_RELEASE_ROWS).expect("the sample decodes")
    else {
        panic!("the sample is a record list");
    };
    let host = start_at(&root, instance, file.clone(), R1_START_MS);
    host.record_config_overrides_for_test(rows)
        .expect("record the later Runtime's rows");
    host.close().expect("close host");

    let host = start_at(&root, instance, file, R1_START_MS + 60_000);
    let mut client = TestClient::connect(&host);
    let report = start_report(&mut client);
    assert_eq!(
        report.instructions_dropped,
        vec!["neutral_later_release.switch".to_owned()],
        "{report:?}"
    );
    assert_eq!(report.instructions_restored, 1, "{report:?}");
    assert_eq!(
        report.instructions_base_changed,
        Vec::<String>::new(),
        "{report:?}"
    );
    assert_eq!(
        configuration_notes(&mut client),
        vec![(
            ConfigCode::ConfigOverrideDropped.as_str().to_owned(),
            "neutral_later_release.switch".to_owned()
        )]
    );
    assert_eq!(
        reported(&mut client, EVIDENCE_MAX_AGE_DAYS),
        (FactScalar::Integer(30), "instruction".to_owned(), true)
    );
    assert_eq!(
        fact_rows(&mut client, CONFIG_OVERRIDES_FACT_KEY)
            .expect("overrides")
            .iter()
            .map(|row| row.get(OVERRIDE_CONFIG_KEY).cloned())
            .collect::<Vec<_>>(),
        vec![Some(FactScalar::String(EVIDENCE_MAX_AGE_DAYS.to_owned()))]
    );
    drop(client);
    host.close().expect("close host");
}

// R-4 test 7, the host's read point: the runtime fact snapshot answers the version the last
// receipt carried, beside the snapshot, never in it; after a restart over the same file it still
// does, the start having restored the same rows.
#[test]
fn r4_t7_the_snapshot_answers_the_last_receipt_s_version_also_after_a_restart() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let id = instance_text(instance);
    let cooldown = instance_key(&id, STUCK_RECOVERY_COOLDOWN_SECS);
    let host = start_at(&root, instance, r1_file(&id, None), R1_START_MS);
    let mut client = TestClient::connect(&host);
    let (_, receipt) = configure(&mut client, vec![set(&cooldown, json!(120))]);
    let last = receipt_version(&receipt);
    assert_eq!(snapshot_version(&mut client), last);
    drop(client);
    host.close().expect("close host");

    let host = start_at(&root, instance, r1_file(&id, None), R1_START_MS + 60_000);
    let mut client = TestClient::connect(&host);
    assert_eq!(snapshot_version(&mut client), last);
    drop(client);
    host.close().expect("close host");
}
