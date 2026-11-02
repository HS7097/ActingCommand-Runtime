// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #432 AGT-1a (`contracts/agent-wake.md`; register §1.245): with no `agent_wake` section a
// start has no key and no subsystem row of it; with one, the start-up assembly reports the keys and
// the row, and refuses what the section check and the assembly's own checks refuse, naming the key.
// A missing registry or an unknown `to` is a note and never a refusal (#419). The names are made up.

use super::*;
use actingcommand_contract::{
    ConfigParameterSource, ConfigSettingChange, FactScalar, IdentifierIssuer,
};
use actingcommand_runtime_config::agent_wake::{ENABLED, RULES, SUMMARY_MAX_CHARS};
use actingcommand_runtime_config::{
    CheckContext, ConfigCode, DeviceBounds, EffectiveConfig, InstructionStamp,
};
use serde_json::{Value, json};
use tempfile::TempDir;

const ALIAS: &str = "neutral.device";

fn configuration(root: &TempDir, agent_wake: Option<Value>) -> ActingdConfigFile {
    let id = IdentifierIssuer::new()
        .expect("issuer")
        .mint_instance_id()
        .expect("instance id");
    let mut config = json!({
        "schema_version": CONFIG_SCHEMA_VERSION,
        "state_root": root.path(),
        "bind_host": "127.0.0.1",
        "bind_port": 0,
        "secret_fingerprint_salt": "0123456789abcdef",
        "instances": [{
            "alias": ALIAS, "instance_id": id.transport(),
            "application_id": "neutral.application", "adb_path": root.path().join("missing-adb.exe"),
            "connect": false, "host": "127.0.0.1", "port": 16384,
            "touch_backend": "adb_shell_input", "capture_backend": "adb"
        }]
    });
    if let Some(section) = agent_wake {
        config["agent_wake"] = section;
    }
    serde_json::from_value::<ActingdConfigFile>(config).expect("typed config")
}

fn rule(on: Value, instances: Value) -> Value {
    json!({
        "id": "neutral-rule", "on": on, "instances": instances, "to": "总览",
        "cooldown_ms": 1800000, "daily_cap": 8,
    })
}

fn whole(rules: Value) -> Value {
    json!({
        "enabled": true,
        // A temporary root is longer than a real one.
        "summary": {"max_chars": 400},
        "body": {"dir": "agent-wakes", "max_bytes": 65536, "retention_days": 14, "max_files": 512},
        "delivery": {
            "queue_capacity": 64, "coalesce_ms": 3000, "send_deadline_ms": 30000,
            "retry_interval_ms": 300000, "give_up_ms": 86400000, "channel_gap_ms": 5000,
            "history_rows": 32, "panic_backoff_ms": 60000, "panic_backoff_max_ms": 900000,
        },
        "quiet": {"windows": [], "hold_until_unix_ms": 0},
        "budget": {"daily_cap": 24},
        "rules": rules,
    })
}

fn refused(root: &TempDir, section: Value) -> (actingcommand_contract::outcome::Code, Outcome) {
    match configuration(root, Some(section.clone())).assemble_or_refusal() {
        Err(refusal) => refusal,
        Ok(_) => panic!("{section} is refused"),
    }
}

fn subsystem_row(assembly: &RuntimeAssembly) -> Option<&actingcommand_contract::ConfigSubsystem> {
    assembly
        .manifest
        .subsystems
        .iter()
        .find(|subsystem| subsystem.name == "agent_wake")
}

fn write_registry(root: &TempDir, channels: Value) {
    let folder = root.path().join("agent-forward");
    std::fs::create_dir_all(&folder).expect("the registry folder");
    std::fs::write(
        folder.join("channels.json"),
        json!({"schema": "actingcommand.agent-channels.v1", "channels": channels}).to_string(),
    )
    .expect("the registry file");
}

#[test]
fn agent_wake_section_absent_keys_are_unknown_and_no_subsystem_row() {
    let root = TempDir::new().expect("tempdir");
    let assembly = configuration(&root, None)
        .assemble()
        .expect("runtime assembly");
    assert!(subsystem_row(&assembly).is_none());
    assert!(
        assembly
            .manifest
            .parameters
            .iter()
            .all(|parameter| !parameter.key.starts_with("agent_wake.")),
        "no key of the section"
    );
    assert!(
        !assembly
            .manifest
            .parameter_sections()
            .contains(&"agent_wake"),
        "no section fact"
    );
    // The keys are in the key table, and an instruction names none of them.
    assert!(actingcommand_runtime_config::find_row(ENABLED).is_some());
    let refused = EffectiveConfig::from_manifest(&assembly.manifest)
        .expect("the configuration in effect")
        .apply(
            &[ConfigSettingChange {
                key: ENABLED.to_owned(),
                value: Some(json!(true)),
            }],
            &InstructionStamp {
                set_unix_ms: 1,
                set_in_owner_epoch: "epoch".to_owned(),
                request_id: "request".to_owned(),
                channel: None,
            },
            &CheckContext::new(
                5_000,
                DeviceBounds {
                    application_stop_ms: 60_000,
                    emulator_stop_ms: 200_000,
                    emulator_start_ms: 190_000,
                },
            ),
        )
        .expect_err("no section, no key");
    assert_eq!(refused.failure_code(), ConfigCode::ConfigKeyUnknown);

    // With the section: the row beside `agent_dispatcher`, and the keys as the file wrote them.
    let assembly = configuration(&root, Some(whole(json!([]))))
        .assemble()
        .expect("runtime assembly");
    let names = assembly
        .manifest
        .subsystems
        .iter()
        .map(|subsystem| subsystem.name.as_str())
        .collect::<Vec<_>>();
    let at = names
        .iter()
        .position(|name| *name == "agent_dispatcher")
        .expect("the dispatcher's row");
    assert_eq!(names.get(at + 1), Some(&"agent_wake"));
    let row = subsystem_row(&assembly).expect("the row");
    assert!(row.enabled);
    assert!(
        row.outcome
            .is(ActingdCode::ConfigSubsystemConfigured.code())
    );
    let enabled = assembly
        .manifest
        .parameters
        .iter()
        .find(|parameter| parameter.key == ENABLED)
        .expect("the key's row");
    assert_eq!(enabled.value, FactScalar::Boolean(true));
    assert_eq!(enabled.source, ConfigParameterSource::Explicit);
    assert!(enabled.settable);
}

#[test]
fn agent_wake_refuses_bad_kinds_codes_and_unknown_aliases() {
    let root = TempDir::new().expect("tempdir");
    let code = "config_value_invalid";
    // What passes: every word of the closed list, a listed code, `*` and a known alias. A
    // `daily_summary` rule names that kind alone, with its time and a day cap of 1 (AGT-2b).
    for on in [
        json!({"kinds": ["ladder_exhausted", "instance_fenced", "delivery_test"]}),
        json!({"kinds": ["daily_summary"], "at_minute_of_day": 1260, "utc_offset_minutes": 540}),
        json!({"kinds": ["run_failed"], "codes": [code]}),
    ] {
        for instances in [json!("*"), json!([ALIAS])] {
            let mut one = rule(on.clone(), instances);
            if on["kinds"] == json!(["daily_summary"]) {
                one["daily_cap"] = json!(1);
            }
            let rules = json!([one]);
            assert!(
                configuration(&root, Some(whole(rules.clone())))
                    .assemble()
                    .is_ok(),
                "{rules} passes"
            );
        }
    }
    let ladder = json!({"kinds": ["ladder_exhausted"]});
    let mut twice = rule(ladder.clone(), json!("*"));
    twice["id"] = json!("same");
    for (rules, problem) in [
        (
            json!([rule(json!({"kinds": ["neutral_kind"]}), json!("*"))]),
            "kind_unknown",
        ),
        (
            json!([rule(json!({"kinds": []}), json!("*"))]),
            "match_missing",
        ),
        (
            json!([rule(json!({"kinds": ["run_failed"]}), json!("*"))]),
            "match_missing",
        ),
        (
            json!([rule(
                json!({"kinds": ["run_failed"], "codes": ["neutral_unlisted_failure"]}),
                json!("*")
            )]),
            "code_unknown",
        ),
        (
            json!([rule(
                json!({"kinds": ["run_failed"], "codes": ["recognition_sample_deadline_or_permission_lost"]}),
                json!("*")
            )]),
            "code_retired",
        ),
        (
            json!([rule(ladder.clone(), json!(["neutral.other"]))]),
            "instance_unknown",
        ),
        (
            json!([rule(ladder.clone(), json!("all"))]),
            "instance_unknown",
        ),
        (json!([rule(ladder.clone(), json!([]))]), "instance_unknown"),
        (json!([twice.clone(), twice.clone()]), "duplicate_id"),
    ] {
        let (refused_code, outcome) = refused(&root, whole(rules.clone()));
        assert_eq!(
            refused_code,
            ConfigCode::AgentWakeConfigInvalid.code(),
            "{rules}"
        );
        assert_eq!(outcome.value("config_key"), Some(&json!(RULES)), "{rules}");
        assert_eq!(
            outcome.value("config_value_problem"),
            Some(&json!(problem)),
            "{rules}"
        );
    }
}

#[test]
fn agent_wake_invalid_names_its_config_key() {
    let root = TempDir::new().expect("tempdir");
    // An empty section is no whole section.
    let (code, outcome) = refused(&root, json!({}));
    assert_eq!(code, ConfigCode::AgentWakeConfigInvalid.code());
    assert_eq!(outcome.value("config_key"), Some(&json!(ENABLED)));
    assert_eq!(
        outcome.value("config_value_problem"),
        Some(&json!("required_missing"))
    );
    // A body folder inside the Runtime's own material.
    let mut section = whole(json!([]));
    section["body"]["dir"] = json!("ledger/wakes");
    let (code, outcome) = refused(&root, section);
    assert_eq!(code, ConfigCode::AgentWakeConfigInvalid.code());
    assert_eq!(
        outcome.value("config_key"),
        Some(&json!("agent_wake.body.dir"))
    );
    assert_eq!(
        outcome.value("config_value_problem"),
        Some(&json!("dir_reserved"))
    );

    // No registry, and a rule whose channel nothing lists: notes, and the start goes on.
    let channel_rule = rule(json!({"kinds": ["ladder_exhausted"]}), json!("*"));
    let assembly = configuration(&root, Some(whole(json!([channel_rule.clone()]))))
        .assemble()
        .expect("a missing registry never refuses");
    let row = subsystem_row(&assembly).expect("the row");
    assert_eq!(
        row.outcome.codes().len(),
        2,
        "the row's code and one note: {:?}",
        row.outcome
    );
    write_registry(
        &root,
        json!([{"name": "项目秘书", "client": "codex", "native_id": "neutral-id"}]),
    );
    configuration(&root, Some(whole(json!([channel_rule]))))
        .assemble()
        .expect("an unknown channel never refuses");

    // A channel whose message cannot fit names the limit.
    write_registry(
        &root,
        json!([{"name": "频".repeat(16), "client": "codex", "native_id": "neutral-id"}]),
    );
    let mut section = whole(json!([]));
    section["summary"]["max_chars"] = json!(60);
    let (code, outcome) = refused(&root, section);
    assert_eq!(code, ConfigCode::AgentWakeConfigInvalid.code());
    assert_eq!(outcome.value("config_key"), Some(&json!(SUMMARY_MAX_CHARS)));
    assert_eq!(
        outcome.value("config_value_problem"),
        Some(&json!("message_floor"))
    );

    // An unreadable registry is a note too: it never stops a start.
    std::fs::write(
        root.path().join("agent-forward").join("channels.json"),
        "not json",
    )
    .expect("the registry file");
    configuration(&root, Some(whole(json!([]))))
        .assemble()
        .expect("an unreadable registry never refuses");

    // The name bound the section reads is the registry's.
    assert_eq!(
        actingcommand_runtime_config::agent_wake::MAX_CHANNEL_CHARS,
        actingcommand_agent_forward::channels::MAX_CHANNEL_CHARS
    );
}
