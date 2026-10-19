// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #387 D1b, test rows D1-M3 to D1-M7 (frozen model v1.2 §8.2): `ConfigureSettings`,
//! its records, its refusals, `clear`, and the restore of instruction values at start by the
//! later-change-wins rule (§12 question 3). Every expected value comes from the model: §5.5
//! (`settable`), §5.6 (checks, receipt, two layers), §5.7 (records, `host.config_overrides`),
//! §5.9 (`config.parameters.<section>`), §6.4 (codes).
//!
//! Workflow #411 B15 R3 (model-b15 §5.2; register §1.113): the remedy keys are settable and
//! recorded as rows (host key 112).

use super::*;
use actingcommand_contract::{
    CONFIG_OVERRIDES_FACT_KEY, ConfigParameter, ConfigParameterSource, ConfigSettingChange,
    ConfiguredSetting, FactScalar, RuntimeConfigManifest, RuntimeOperationKind, RuntimePayload,
    config_parameters_fact_key,
};
use actingcommand_runtime_config::scheduling::{
    REMEDY_CHOICES, REMEDY_ENABLED, REMEDY_MAX_PER_ROUND,
};
use actingcommand_runtime_config::{
    ConfigCode, DefaultsFile, OVERRIDE_BASE_JSON, OVERRIDE_BASE_SOURCE, OVERRIDE_CONFIG_KEY,
    STUCK_RECOVERY, STUCK_RECOVERY_COOLDOWN_SECS, instance_key, mechanism_rows, render_parameters,
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
    let request = client.request(RuntimeOperation::ConfigureSettings { changes });
    let receipt = client.send(&request);
    (request, receipt)
}

/// The rows of one runtime fact, from the Runtime's fact snapshot.
fn fact_rows(client: &mut TestClient, key: &str) -> Option<Vec<BTreeMap<String, FactScalar>>> {
    let request = client.request(RuntimeOperation::RuntimeFactSnapshot);
    let receipt = client.send(&request);
    let Some(RuntimeResult::RuntimeFactSnapshot { snapshot }) = receipt.result() else {
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
