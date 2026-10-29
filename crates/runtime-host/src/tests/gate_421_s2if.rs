// SPDX-License-Identifier: AGPL-3.0-only

//! #421 S2-IF (v0.12.21; work order rd-1028/brief-s2-instr-fix.md). Production, 10-28 10:11: an
//! update changed the default of a key three instructions overrode, the start kept them over the
//! new default (`config_override_base_changed`), and `StartReport` listed them as dropped, so the
//! installer's `instructions` check rolled a good deploy back. The required state (the work
//! order's Fix 1; model v1.2 §5.7 and `contracts/runtime-config.md`, the restore table): a value
//! kept over a changed default is restored, counted in `instructions_restored` and named in
//! `instructions_base_changed`, never in `instructions_dropped`; a key gone (`dropped`) or
//! superseded by the file is not restored and is named in `instructions_dropped`.
//!
//! - T-1: the default changed: base-changed, restored, nothing dropped;
//! - T-2: the key no longer exists: dropped, not restored;
//! - T-3: the file changed the key (superseded): dropped, not restored;
//! - T-4: the rollback, the default changed back: base-changed, restored, nothing dropped.

use super::*;
use actingcommand_contract::{
    CONFIG_OVERRIDES_FACT_KEY, ConfigParameter, ConfigParameterSource, ConfigSettingChange,
    FactScalar, RuntimeConfigManifest, RuntimeStartReport,
};
use actingcommand_runtime_config::{
    OVERRIDE_CONFIG_KEY, OVERRIDE_VALUE_JSON, STUCK_RECOVERY, STUCK_RECOVERY_COOLDOWN_SECS,
    instance_key,
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

/// A manifest of the per-instance keys given and `bind_port` (`settable` comes from the key
/// table), as D1b's restore tests build it.
fn manifest(mut parameters: Vec<ConfigParameter>) -> RuntimeConfigManifest {
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

/// Sets each `(key, value)` as a run-time instruction, all in one request.
fn instruct(host: &RuntimeHost, values: &[(&str, Value)]) {
    let mut client = TestClient::connect(host);
    let request = client.request(RuntimeOperation::ConfigureSettings {
        expected_version: None,
        changes: values
            .iter()
            .map(|(key, value)| ConfigSettingChange {
                key: (*key).to_owned(),
                value: Some(value.clone()),
            })
            .collect(),
    });
    let receipt = client.send(&request);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
}

/// The Runtime's report of its own start.
fn start_report(host: &RuntimeHost) -> RuntimeStartReport {
    let mut client = TestClient::connect(host);
    let request = client.request(RuntimeOperation::StartReport);
    let receipt = client.send(&request);
    let Some(RuntimeResult::StartReport { report }) = receipt.result() else {
        panic!("expected the start report: {receipt:?}");
    };
    (**report).clone()
}

/// The instructions in effect, `(config_key, value_json)` as `host.config_overrides` holds them
/// (what the installer's check compares with S0).
fn instructions_in_effect(host: &RuntimeHost) -> Vec<(String, String)> {
    let mut client = TestClient::connect(host);
    let request = client.request(RuntimeOperation::RuntimeFactSnapshot);
    let receipt = client.send(&request);
    let Some(RuntimeResult::RuntimeFactSnapshot { snapshot, .. }) = receipt.result() else {
        panic!("expected the runtime fact snapshot: {receipt:?}");
    };
    let text = |row: &BTreeMap<String, FactScalar>, member: &str| match row.get(member) {
        Some(FactScalar::String(text)) => text.clone(),
        other => panic!("{member} is a string: {other:?}"),
    };
    let mut rows = snapshot
        .records
        .iter()
        .find(|record| record.key == CONFIG_OVERRIDES_FACT_KEY)
        .map(|record| match &record.value {
            ContractFactValue::RecordList(rows) => rows
                .iter()
                .map(|row| {
                    (
                        text(row, OVERRIDE_CONFIG_KEY),
                        text(row, OVERRIDE_VALUE_JSON),
                    )
                })
                .collect::<Vec<_>>(),
            other => panic!("{CONFIG_OVERRIDES_FACT_KEY} is a record list: {other:?}"),
        })
        .unwrap_or_default();
    rows.sort();
    rows
}

fn keys(names: &[&str]) -> Vec<String> {
    let mut keys = names
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    keys.sort();
    keys
}

/// An instance's stuck-recovery flag (a default, instructed `false`, never changing) and its
/// cool-down over the default `cooldown_default` (instructed 60): the instructions of T-1 and
/// T-4, started once with the default 600 and set, then started again with `cooldown_default`.
struct Kept {
    root: TempDir,
    instance: InstanceId,
    enabled: String,
    cooldown: String,
}

impl Kept {
    fn new() -> Self {
        let root = TempDir::new().expect("tempdir");
        let instance = instance_id();
        let id = instance_text(instance);
        let kept = Self {
            root,
            instance,
            enabled: instance_key(&id, STUCK_RECOVERY),
            cooldown: instance_key(&id, STUCK_RECOVERY_COOLDOWN_SECS),
        };
        let host = kept.start_with(600);
        instruct(
            &host,
            &[
                (kept.enabled.as_str(), json!(false)),
                (kept.cooldown.as_str(), json!(60)),
            ],
        );
        host.close().expect("close host");
        kept
    }

    /// A start whose file still names neither key, the cool-down's default `cooldown_default`.
    fn start_with(&self, cooldown_default: i64) -> RuntimeHost {
        start(
            &self.root,
            self.instance,
            manifest(vec![
                parameter(
                    &self.enabled,
                    FactScalar::Boolean(true),
                    ConfigParameterSource::Default,
                ),
                parameter(
                    &self.cooldown,
                    FactScalar::Integer(cooldown_default),
                    ConfigParameterSource::Default,
                ),
            ]),
        )
    }

    fn instructions(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            (self.enabled.clone(), "false".to_owned()),
            (self.cooldown.clone(), "60".to_owned()),
        ];
        rows.sort();
        rows
    }
}

// T-1: an instruction whose default changed is restored and named base-changed, never dropped.
#[test]
fn s2if_t1_an_instruction_whose_default_changed_is_base_changed_not_dropped() {
    let kept = Kept::new();
    let host = kept.start_with(900);
    let report = start_report(&host);
    assert_eq!(report.instructions_restored, 2, "{report:?}");
    assert_eq!(
        report.instructions_dropped,
        Vec::<String>::new(),
        "{report:?}"
    );
    assert_eq!(
        report.instructions_base_changed,
        keys(&[kept.cooldown.as_str()]),
        "{report:?}"
    );
    assert_eq!(instructions_in_effect(&host), kept.instructions());
    host.close().expect("close host");
}

// T-2: an instruction whose key no longer exists is dropped and not restored.
#[test]
fn s2if_t2_an_instruction_whose_key_is_gone_is_dropped() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let a = instance_key(&instance_text(instance), STUCK_RECOVERY_COOLDOWN_SECS);
    let b = instance_key(&instance_text(instance_id()), STUCK_RECOVERY_COOLDOWN_SECS);
    let cooldown = |key: &str| {
        parameter(
            key,
            FactScalar::Integer(600),
            ConfigParameterSource::Default,
        )
    };
    let host = start(
        &root,
        instance,
        manifest(vec![cooldown(a.as_str()), cooldown(b.as_str())]),
    );
    instruct(&host, &[(a.as_str(), json!(60)), (b.as_str(), json!(30))]);
    host.close().expect("close host");

    // The file layer no longer has b's key.
    let host = start(&root, instance, manifest(vec![cooldown(a.as_str())]));
    let report = start_report(&host);
    assert_eq!(report.instructions_restored, 1, "{report:?}");
    assert_eq!(
        report.instructions_dropped,
        keys(&[b.as_str()]),
        "{report:?}"
    );
    assert_eq!(
        report.instructions_base_changed,
        Vec::<String>::new(),
        "{report:?}"
    );
    assert_eq!(
        instructions_in_effect(&host),
        vec![(a.clone(), "60".to_owned())]
    );
    host.close().expect("close host");
}

// T-3: an instruction the file superseded is dropped and not restored: one over an explicit
// value the file changed, one over a default the file names now.
#[test]
fn s2if_t3_a_superseded_instruction_is_dropped() {
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
    instruct(
        &host,
        &[
            (enabled.as_str(), json!(false)),
            (cooldown.as_str(), json!(60)),
        ],
    );
    host.close().expect("close host");

    // The file changed the explicit cool-down and names the flag now.
    let host = start(
        &root,
        instance,
        manifest(vec![
            parameter(
                &enabled,
                FactScalar::Boolean(true),
                ConfigParameterSource::Explicit,
            ),
            parameter(
                &cooldown,
                FactScalar::Integer(450),
                ConfigParameterSource::Explicit,
            ),
        ]),
    );
    let report = start_report(&host);
    assert_eq!(report.instructions_restored, 0, "{report:?}");
    assert_eq!(
        report.instructions_dropped,
        keys(&[enabled.as_str(), cooldown.as_str()]),
        "{report:?}"
    );
    assert_eq!(
        report.instructions_base_changed,
        Vec::<String>::new(),
        "{report:?}"
    );
    assert_eq!(instructions_in_effect(&host), Vec::new());
    host.close().expect("close host");
}

// T-4: the rollback's start, where the default changed back, restores the instruction and names
// it base-changed again, never dropped.
#[test]
fn s2if_t4_the_rollback_start_with_the_default_changed_back_is_base_changed() {
    let kept = Kept::new();
    // The update's start: the default changed.
    let host = kept.start_with(900);
    assert_eq!(
        start_report(&host).instructions_base_changed,
        keys(&[kept.cooldown.as_str()])
    );
    host.close().expect("close host");

    // The rollback's start: the default changed back.
    let host = kept.start_with(600);
    let report = start_report(&host);
    assert_eq!(report.instructions_restored, 2, "{report:?}");
    assert_eq!(
        report.instructions_dropped,
        Vec::<String>::new(),
        "{report:?}"
    );
    assert_eq!(
        report.instructions_base_changed,
        keys(&[kept.cooldown.as_str()]),
        "{report:?}"
    );
    assert_eq!(instructions_in_effect(&host), kept.instructions());
    host.close().expect("close host");
}
