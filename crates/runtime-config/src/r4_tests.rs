// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-4 (A.6 rules 7 and 8; register §1.239; since 0.12.36), tests 4 and 5 of the
// work order: a commit of one key leaves every other key alone, and the console's three tiers
// come from the key table, key by key. Tests 1 to 3, 6 and 7 run through the host
// (`runtime-host` `gate_387_d1b`, `r4_t*`) and `actingctl` (`gate_429_r4`).

use super::*;
use actingcommand_contract::{
    ConfigParameterSource, ConfigSettingChange, FactScalar, RuntimeConfigManifest,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const INSTANCE_A: &str = "instance_0123456789abcdef0123456789abcdef";
const INSTANCE_B: &str = "instance_fedcba9876543210fedcba9876543210";
const DEVICE: DeviceBounds = DeviceBounds {
    application_stop_ms: 60_000,
    emulator_stop_ms: 200_000,
    emulator_start_ms: 190_000,
};

fn context() -> CheckContext {
    CheckContext::new(5_000, DEVICE)
}

fn stamp(request_id: &str) -> InstructionStamp {
    InstructionStamp {
        set_unix_ms: 1_800_000_000_000,
        set_in_owner_epoch: "epoch_r4".to_owned(),
        request_id: request_id.to_owned(),
        channel: None,
    }
}

/// The configuration a start assembles from the shipped defaults and an empty file, with
/// instance A configured (its entry names nothing else).
fn shipped() -> EffectiveConfig {
    let file = json!({"instances": [{"instance_id": INSTANCE_A, "alias": "node.a"}]});
    let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults file parses");
    let parameters = render_parameters(
        &mechanism_rows(),
        &[INSTANCE_A.to_owned()],
        &defaults,
        &file,
        &|_| None,
    )
    .expect("every key resolves");
    EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    })
    .expect("every key is in the key table")
}

fn set(key: &str, value: Value) -> ConfigSettingChange {
    ConfigSettingChange {
        key: key.to_owned(),
        value: Some(value),
    }
}

/// Each key in effect: its value, source and whether it is settable.
type InEffect = BTreeMap<String, (FactScalar, ConfigParameterSource, bool)>;

/// Every key in effect with its value, source and whether it is settable.
fn in_effect(config: &EffectiveConfig) -> InEffect {
    config
        .manifest()
        .parameters
        .into_iter()
        .map(|parameter| {
            (
                parameter.key,
                (parameter.value, parameter.source, parameter.settable),
            )
        })
        .collect()
}

/// The instruction rows in effect, by key.
fn rows_by_key(config: &EffectiveConfig) -> BTreeMap<String, BTreeMap<String, FactScalar>> {
    config
        .override_rows()
        .into_iter()
        .map(|row| match row.get(OVERRIDE_CONFIG_KEY) {
            Some(FactScalar::String(key)) => (key.clone(), row),
            other => panic!("a row names its key: {other:?}"),
        })
        .collect()
}

fn once(id: &str) -> Value {
    json!([{"kind": "once", "id": id, "from_unix_ms": 1_800_000_000_000_u64,
            "until_unix_ms": 1_800_003_600_000_u64}])
}

// R-4 test 4 (A.6 rule 7): over the shipped configuration with one instruction already in effect,
// one change of one key, of each key the console shows that an instruction may set (and the
// change that clears the earlier instruction), leaves every other key's value in effect, source
// and settable as they were, adds or removes no other key, and leaves every other instruction row
// as it was. The changed key alone is different: from `instruction`, or back at its file value.
#[test]
fn r4_t4_changing_one_key_leaves_every_other_key_s_value_and_source_as_it_was() {
    let base = shipped()
        .apply(
            &[set("frame_retention_evidence_max_age_days", json!(30))],
            &stamp("request_base"),
            &context(),
        )
        .expect("the earlier instruction applies")
        .config;
    let before = in_effect(&base);
    let rows_before = rows_by_key(&base);
    let blackout_a = format!("instance.{INSTANCE_A}.scheduling.blackout");
    let idle_a = format!("instance.{INSTANCE_A}.idle.action");
    let changes = [
        set("performance.pacing.mode", json!("observe")),
        set("performance.pacing.max_concurrent_runs", json!(1)),
        set("performance.pacing.heavy_dispatch_limit.normal", json!(3)),
        set("scheduling.parallel.enabled", json!(false)),
        set("scheduling.blackout.host", once("night")),
        set(&blackout_a, once("away")),
        set(&idle_a, json!("none")),
        set("frame_retention_evidence_max_age_days", json!(60)),
        ConfigSettingChange {
            key: "frame_retention_evidence_max_age_days".to_owned(),
            value: None,
        },
    ];
    for change in changes {
        let key = change.key.clone();
        let after = base
            .apply(
                std::slice::from_ref(&change),
                &stamp("request_one"),
                &context(),
            )
            .unwrap_or_else(|failure| panic!("{key}: {failure:?}"))
            .config;
        let mut others_after = in_effect(&after);
        let changed = others_after.remove(&key);
        let mut others_before = before.clone();
        let was = others_before.remove(&key);
        assert_eq!(
            others_after, others_before,
            "changing {key} touched another key"
        );
        match change.value {
            Some(_) => assert_eq!(
                changed.map(|(_, source, settable)| (source, settable)),
                Some((ConfigParameterSource::Instruction, true)),
                "{key}"
            ),
            None => {
                assert_ne!(changed, was, "{key}: the clear changed it");
                assert_eq!(
                    changed.map(|(_, source, _)| source),
                    Some(ConfigParameterSource::Default),
                    "{key}: back at its file value"
                );
            }
        }
        let mut rows_after = rows_by_key(&after);
        rows_after.remove(&key);
        let mut rows_others = rows_before.clone();
        rows_others.remove(&key);
        assert_eq!(
            rows_after, rows_others,
            "changing {key} touched another row"
        );
    }
}

// R-4 test 5 (A.6 rule 8): `ui_tier` key by key against the contract table: the eight daily keys
// (each per-instance one for any instance) and the six advanced ones; every other key of the
// shipped configuration, a key the key table does not have and a malformed name are hidden. The
// table holds exactly these rows, each a row of the key table, at most eight of them advanced.
#[test]
fn r4_t5_ui_tier_key_by_key_and_an_unlisted_key_is_hidden() {
    let mut table = vec![
        ("performance.pacing.mode".to_owned(), UiTier::Daily),
        (
            "performance.pacing.max_concurrent_runs".to_owned(),
            UiTier::Daily,
        ),
        (
            "performance.pacing.heavy_dispatch_limit.normal".to_owned(),
            UiTier::Daily,
        ),
        ("scheduling.parallel.enabled".to_owned(), UiTier::Daily),
        ("scheduling.blackout.host".to_owned(), UiTier::Daily),
        ("mumu_root".to_owned(), UiTier::Advanced),
        (
            "frame_retention_evidence_max_age_days".to_owned(),
            UiTier::Advanced,
        ),
        ("host.process_memory_priority".to_owned(), UiTier::Advanced),
        ("host.process_io_priority".to_owned(), UiTier::Advanced),
        ("lab_module_authoring".to_owned(), UiTier::Advanced),
        ("lab_module_debugging".to_owned(), UiTier::Advanced),
    ];
    for instance in [INSTANCE_A, INSTANCE_B] {
        for member in ["scheduling.blackout", "start_emulator", "idle.action"] {
            table.push((format!("instance.{instance}.{member}"), UiTier::Daily));
        }
    }
    for (key, tier) in &table {
        assert_eq!(ui_tier(key), *tier, "{key}");
    }
    assert_eq!(UiTier::Daily.as_str(), "daily");
    assert_eq!(UiTier::Advanced.as_str(), "advanced");
    assert_eq!(UiTier::Hidden.as_str(), "hidden");

    let unlisted = [
        "performance.pacing.heavy_dispatch_limit.paused".to_owned(),
        "performance.presets".to_owned(),
        "idle.action".to_owned(),
        "scheduling.blackout.quiet_margin_ms".to_owned(),
        "bind_port".to_owned(),
        "standard_packs.root".to_owned(),
        format!("instance.{INSTANCE_A}.stuck_recovery"),
        format!("instance.{INSTANCE_A}.idle.min_gap_ms"),
        "neutral_later_release.switch".to_owned(),
        "instance..scheduling.blackout".to_owned(),
        String::new(),
    ];
    for key in &unlisted {
        assert_eq!(ui_tier(key), UiTier::Hidden, "{key:?}");
    }
    let listed = table
        .iter()
        .map(|(key, _)| key.as_str())
        .collect::<Vec<_>>();
    for key in in_effect(&shipped()).keys() {
        if !listed.contains(&key.as_str()) {
            assert_eq!(ui_tier(key), UiTier::Hidden, "{key}");
        }
    }

    assert_eq!(UI_TIERS.len(), 14);
    for (name, _) in UI_TIERS {
        assert!(
            rows().any(|row| row.name == *name),
            "{name:?} is not a row of the key table"
        );
    }
    let advanced = UI_TIERS
        .iter()
        .filter(|(_, tier)| *tier == UiTier::Advanced)
        .count();
    assert_eq!(advanced, 6);
    assert!(advanced <= MAX_ADVANCED_KEYS);
    assert!(UI_TIERS.iter().all(|(_, tier)| *tier != UiTier::Hidden));
}
