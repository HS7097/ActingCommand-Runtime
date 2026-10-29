// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-2 (design 429-A A.1; register §1.220; since 0.12.31): the performance presets
// and the current preset, on the configuration in effect a start assembles from the shipped
// defaults and a file (tests 5 and 6 of the work order). Applying a preset is the one
// `ConfigureSettings` of its changes; the host's tests 1 to 4 are in `runtime-host`
// (`gate_0122_pk1b`, `gate_427b`, `gate_387_d1b`, `r2_t*`). Every expected value is the work
// order's: A.1's preset table (standard: observe, no max, 2, parallel; adaptive: pace, no max, 2,
// parallel; light: pace, 1, 1, parallel), ruling 98's defaults (pace, parallel on, 2, no max),
// `full` refused by instruction and the current preset by exact match or `custom`.

use super::*;
use crate::performance::pacing::{HEAVY_DISPATCH_LIMIT_NORMAL, MAX_CONCURRENT_RUNS, MODE};
use crate::performance::presets::{
    CUSTOM, MEMBERS, PRESETS, PresetMember, PresetTable, current_preset, table_of,
};
use crate::scheduling::PARALLEL_ENABLED;
use actingcommand_contract::{
    ConfigParameterSource, ConfigSettingChange, FactScalar, RuntimeConfigManifest,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const INSTANCE_A: &str = "instance_0123456789abcdef0123456789abcdef";
const DEVICE: DeviceBounds = DeviceBounds {
    application_stop_ms: 60_000,
    emulator_stop_ms: 200_000,
    emulator_start_ms: 190_000,
};
const PRESET_NAMES: [&str; 3] = ["standard", "adaptive", "light"];

fn context() -> CheckContext {
    CheckContext::new(5_000, DEVICE)
}

fn stamp() -> InstructionStamp {
    InstructionStamp {
        set_unix_ms: 1_800_000_000_000,
        set_in_owner_epoch: "epoch_r2".to_owned(),
        request_id: "request_r2".to_owned(),
        channel: None,
    }
}

/// The configuration a start assembles from the shipped defaults and `file`, with instance A
/// configured (its entry names nothing else).
fn from_file(file: Value) -> EffectiveConfig {
    let mut file = file;
    file["instances"] = json!([{"instance_id": INSTANCE_A, "alias": "node.a"}]);
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

/// The preset table in effect in `config`.
fn table(config: &EffectiveConfig) -> PresetTable {
    table_of(&config.manifest().parameters)
        .expect("the table's row")
        .expect("the table reads back")
}

/// The current preset of `config`.
fn current(config: &EffectiveConfig) -> String {
    current_preset(&table(config), &config.manifest().parameters).preset
}

/// `config` after the one `ConfigureSettings` that applies the preset `name` of its own table.
fn applied(config: &EffectiveConfig, name: &str) -> EffectiveConfig {
    let changes = table(config)
        .get(name)
        .unwrap_or_else(|| panic!("the preset {name}"))
        .changes();
    config
        .apply(&changes, &stamp(), &context())
        .unwrap_or_else(|failure| panic!("{name} applies: {failure:?}"))
        .config
}

// R-2 test 6, the mapping table and the table: each member's key, key by key, in the order the
// changes and the answer list them; the key `performance.presets` is an object read at assembly,
// not settable, its default in the defaults file since 0.12.31; the shipped table is A.1's.
#[test]
fn r2_t6_the_mapping_table_key_by_key_and_the_shipped_presets() {
    assert_eq!(
        MEMBERS,
        [
            PresetMember {
                member: "mode",
                key: "performance.pacing.mode",
                optional: false,
            },
            PresetMember {
                member: "max_concurrent_runs",
                key: "performance.pacing.max_concurrent_runs",
                optional: true,
            },
            PresetMember {
                member: "heavy_dispatch_limit_normal",
                key: "performance.pacing.heavy_dispatch_limit.normal",
                optional: false,
            },
            PresetMember {
                member: "parallel_enabled",
                key: "scheduling.parallel.enabled",
                optional: false,
            },
        ]
    );
    let (row, _) = find_row(PRESETS).expect("the key is in the key table");
    assert_eq!(row.value_type, ValueType::Object);
    assert_eq!(row.default, DefaultSource::File);
    assert!(!row.settable.is_settable());
    assert_eq!(row.since, "0.12.31");

    let table = table(&from_file(json!({})));
    let word = |text: &str| Some(FactScalar::String(text.to_owned()));
    let number = |value: i64| Some(FactScalar::Integer(value));
    let on = Some(FactScalar::Boolean(true));
    for (name, mode, max, heavy) in [
        ("standard", word("observe"), None, number(2)),
        ("adaptive", word("pace"), None, number(2)),
        ("light", word("pace"), number(1), number(1)),
    ] {
        let preset = table
            .get(name)
            .unwrap_or_else(|| panic!("the preset {name}"));
        assert_eq!(
            [
                MODE,
                MAX_CONCURRENT_RUNS,
                HEAVY_DISPATCH_LIMIT_NORMAL,
                PARALLEL_ENABLED
            ]
            .map(|key| preset.value(key).cloned()),
            [mode, max, heavy, on.clone()],
            "{name}"
        );
    }
    assert_eq!(
        table
            .presets()
            .iter()
            .map(|preset| preset.name())
            .collect::<Vec<_>>(),
        ["adaptive", "light", "standard"]
    );
    // A member a preset leaves unwritten is a change without a value; the others carry theirs.
    assert_eq!(
        table.get("adaptive").expect("adaptive").changes(),
        vec![
            set(MODE, json!("pace")),
            ConfigSettingChange {
                key: MAX_CONCURRENT_RUNS.to_owned(),
                value: None,
            },
            set(HEAVY_DISPATCH_LIMIT_NORMAL, json!(2)),
            set(PARALLEL_ENABLED, json!(true)),
        ]
    );
}

// R-2 test 6: a new install with no overrides is `adaptive` (ruling 98: the defaults file is
// adaptive's members: pace, parallel on, 2, no max), each member from `default`.
#[test]
fn r2_t6_a_new_install_without_overrides_is_adaptive() {
    let config = from_file(json!({}));
    let read = current_preset(&table(&config), &config.manifest().parameters);
    assert_eq!(read.preset, "adaptive");
    assert_eq!(
        read.members
            .iter()
            .map(|member| (member.key, member.value.clone(), member.source))
            .collect::<Vec<_>>(),
        vec![
            (
                MODE,
                Some(FactScalar::String("pace".to_owned())),
                ConfigParameterSource::Default
            ),
            (MAX_CONCURRENT_RUNS, None, ConfigParameterSource::Default),
            (
                HEAVY_DISPATCH_LIMIT_NORMAL,
                Some(FactScalar::Integer(2)),
                ConfigParameterSource::Default
            ),
            (
                PARALLEL_ENABLED,
                Some(FactScalar::Boolean(true)),
                ConfigParameterSource::Default
            ),
        ]
    );
    assert_eq!(
        read.answer(),
        json!({"preset": "adaptive", "members": [
            {"key": MODE, "value": "pace", "source": "default"},
            {"key": MAX_CONCURRENT_RUNS, "value": null, "source": "default"},
            {"key": HEAVY_DISPATCH_LIMIT_NORMAL, "value": 2, "source": "default"},
            {"key": PARALLEL_ENABLED, "value": true, "source": "default"},
        ]})
    );
}

// R-2 test 6: after a preset is applied the current preset is its name; any member changed on its
// own afterwards makes it `custom`.
#[test]
fn r2_t6_an_exact_match_names_the_preset_and_any_changed_member_is_custom() {
    let file = from_file(json!({}));
    for name in PRESET_NAMES {
        let config = applied(&file, name);
        assert_eq!(current(&config), name, "{name} applied");
        for change in [
            set(MODE, json!("observe")),
            set(MODE, json!("pace")),
            set(MAX_CONCURRENT_RUNS, json!(2)),
            set(HEAVY_DISPATCH_LIMIT_NORMAL, json!(3)),
            set(PARALLEL_ENABLED, json!(false)),
        ] {
            // A change to the value the preset already gives the key changes no member.
            let same = table(&config)
                .get(name)
                .expect("the preset")
                .changes()
                .contains(&change);
            if same {
                continue;
            }
            let changed = config
                .apply(std::slice::from_ref(&change), &stamp(), &context())
                .unwrap_or_else(|failure| panic!("{name} {change:?}: {failure:?}"))
                .config;
            assert_eq!(current(&changed), CUSTOM, "{name} with {change:?}");
        }
    }
}

// R-2 test 6: an install whose earlier release recorded online overrides keeps them after the
// update and is judged by them. 0.12.30 could set `scheduling.parallel.enabled` only; its row of
// `false`, set over that release's default `false`, is restored over the new default `true`
// (`config_override_base_changed`, kept), so the install is `custom`, not the new defaults'
// `adaptive`. Overrides of this build that match a preset are judged that preset after a start.
#[test]
fn r2_t6_an_install_with_online_overrides_keeps_them_after_the_update_and_is_judged_by_them() {
    let earlier = BTreeMap::from([
        (
            OVERRIDE_CONFIG_KEY.to_owned(),
            FactScalar::String(PARALLEL_ENABLED.to_owned()),
        ),
        (
            OVERRIDE_VALUE_JSON.to_owned(),
            FactScalar::String("false".to_owned()),
        ),
        (
            OVERRIDE_BASE_JSON.to_owned(),
            FactScalar::String("false".to_owned()),
        ),
        (
            OVERRIDE_BASE_SOURCE.to_owned(),
            FactScalar::String("default".to_owned()),
        ),
        (
            OVERRIDE_SET_UNIX_MS.to_owned(),
            FactScalar::TimestampMs(1_790_000_000_000),
        ),
        (
            OVERRIDE_SET_IN_OWNER_EPOCH.to_owned(),
            FactScalar::String("epoch_0_12_30".to_owned()),
        ),
        (
            OVERRIDE_REQUEST_ID.to_owned(),
            FactScalar::String("request_0_12_30".to_owned()),
        ),
    ]);
    let mut updated = from_file(json!({}));
    let notes = updated
        .restore(std::slice::from_ref(&earlier), &context())
        .expect("the row has the row shape");
    assert_eq!(
        notes
            .iter()
            .map(|note| (note.note, note.config_key.clone()))
            .collect::<Vec<_>>(),
        vec![(
            ConfigCode::ConfigOverrideBaseChanged,
            PARALLEL_ENABLED.to_owned()
        )]
    );
    assert_eq!(
        updated.value(PARALLEL_ENABLED),
        Some(&FactScalar::Boolean(false))
    );
    assert_eq!(current(&updated), CUSTOM);

    let standard = applied(&from_file(json!({})), "standard");
    let rows = standard.override_rows();
    let mut restarted = from_file(json!({}));
    assert_eq!(restarted.restore(&rows, &context()), Ok(Vec::new()));
    assert_eq!(current(&restarted), "standard");
}

// R-2 test 6: a file that writes `mode` `full` and no override is `custom` (no preset is `full`).
#[test]
fn r2_t6_a_full_file_without_overrides_is_custom() {
    let config = from_file(json!({"performance": {"pacing": {"mode": "full"}}}));
    config.check(&context()).expect("full is a file's choice");
    assert_eq!(current(&config), CUSTOM);
}

// R-2 test 6: each of the three presets applies over a file that writes `full`, leaving `full`
// through `mode`'s online override; the current preset is then its name.
#[test]
fn r2_t6_all_three_presets_apply_over_a_full_file() {
    let file = from_file(json!({"performance": {"pacing": {"mode": "full"}}}));
    for name in PRESET_NAMES {
        let config = applied(&file, name);
        assert_eq!(current(&config), name, "{name}");
        let mode = config.instruction(MODE).expect("mode's online override");
        assert_eq!(mode.base_json, "\"full\"", "{name}");
        assert_eq!(mode.base_source, ConfigParameterSource::Explicit, "{name}");
    }
}

// R-2 test 6: a file that writes `max_concurrent_runs` (here 4): after `standard` or `adaptive`,
// which leave it unwritten, the file's value is still in effect (clearing returns to it), so the
// current preset is `custom`; after `light`, which writes 1, it is `light`.
#[test]
fn r2_t6_a_file_that_writes_max_concurrent_runs_is_custom_after_standard_or_adaptive_and_light_after_light()
 {
    let file = from_file(json!({"performance": {"pacing": {"max_concurrent_runs": 4}}}));
    for (name, expected, max) in [
        ("standard", CUSTOM, 4),
        ("adaptive", CUSTOM, 4),
        ("light", "light", 1),
    ] {
        let config = applied(&file, name);
        assert_eq!(current(&config), expected, "{name}");
        assert_eq!(
            config.value(MAX_CONCURRENT_RUNS),
            Some(&FactScalar::Integer(max)),
            "{name}"
        );
    }
}

// R-2 test 5: `full` by instruction is refused, `config_value_invalid` with `out_of_range` naming
// `mode`, whether the file writes `full` or not; `full` stays a file-only choice, and a recorded
// value `full` (which no build writes) is not restored. `observe` and `pace` are taken.
#[test]
fn r2_t5_full_by_instruction_is_refused() {
    for file in [
        json!({}),
        json!({"performance": {"pacing": {"mode": "full"}}}),
    ] {
        let config = from_file(file.clone());
        let refused = config
            .apply(&[set(MODE, json!("full"))], &stamp(), &context())
            .expect_err("full by instruction");
        assert_eq!(
            refused.failure_code(),
            ConfigCode::ConfigValueInvalid,
            "{file}"
        );
        assert_eq!(refused.config_key(), MODE, "{file}");
        assert_eq!(
            refused.problem(),
            Some(ConfigValueProblem::OutOfRange),
            "{file}"
        );
        for mode in ["observe", "pace"] {
            assert!(
                config
                    .apply(&[set(MODE, json!(mode))], &stamp(), &context())
                    .is_ok(),
                "{mode} over {file}"
            );
        }
    }
    let mut row = from_file(json!({}))
        .apply(&[set(MODE, json!("observe"))], &stamp(), &context())
        .expect("observe")
        .config
        .override_rows()
        .remove(0);
    row.insert(
        OVERRIDE_VALUE_JSON.to_owned(),
        FactScalar::String("\"full\"".to_owned()),
    );
    let mut restarted = from_file(json!({}));
    let notes = restarted
        .restore(&[row], &context())
        .expect("the row has the row shape");
    assert_eq!(
        notes
            .iter()
            .map(|note| (note.note, note.config_key.clone()))
            .collect::<Vec<_>>(),
        vec![(ConfigCode::ConfigOverrideDropped, MODE.to_owned())]
    );
}
