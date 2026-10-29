// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-1 (A.6 rule 4; register §1.219; since 0.12.31): a key whose default is none
// that the configuration file does not write can be set by an instruction once its instance or
// section is in the configuration, over the base `base_source` `default`, `base_json` `null`;
// only the barrier "the file does not write it" goes, each key's own kind stays. Tests 4 to 6 of
// the work order; tests 1 to 3 run through the host (`runtime-host` `gate_387_d1b`, `r1_t*`).

use super::*;
use crate::effective::RecordedOverride;
use crate::performance::pacing::MAX_CONCURRENT_RUNS;
use crate::scheduling::{FACT_REFRESH_OVERDUE_MS, INSTANCE_BLACKOUT};
use actingcommand_contract::{ConfigSettingChange, FactScalar, FactValue, RuntimeConfigManifest};
use serde_json::{Value, json};

const INSTANCE_A: &str = "instance_0123456789abcdef0123456789abcdef";
const DEVICE: DeviceBounds = DeviceBounds {
    application_stop_ms: 60_000,
    emulator_stop_ms: 200_000,
    emulator_start_ms: 190_000,
};

/// The `host.config_overrides` value of the two instructions `unwritten_changes` sets over a
/// file that writes neither key, recorded as the ledger's fact value holds it (`FactValue`, rows
/// in manifest order, fields in key order): the fixed sample the rollback test reads.
const UNWRITTEN_KEY_ROWS: &str = concat!(
    r#"{"type":"record_list","value":["#,
    r#"{"base_json":{"type":"string","value":"null"},"#,
    r#""base_source":{"type":"string","value":"default"},"#,
    r#""config_key":{"type":"string","value":"scheduling.fact_refresh.overdue_ms"},"#,
    r#""request_id":{"type":"string","value":"request_r1"},"#,
    r#""set_in_owner_epoch":{"type":"string","value":"epoch_r1"},"#,
    r#""set_unix_ms":{"type":"timestamp_ms","value":1800000000000},"#,
    r#""value_json":{"type":"string","value":"600000"}},"#,
    r#"{"base_json":{"type":"string","value":"null"},"#,
    r#""base_source":{"type":"string","value":"default"},"#,
    r#""config_key":{"type":"string","value":"#,
    r#""instance.instance_0123456789abcdef0123456789abcdef.scheduling.blackout"},"#,
    r#""request_id":{"type":"string","value":"request_r1"},"#,
    r#""set_in_owner_epoch":{"type":"string","value":"epoch_r1"},"#,
    r#""set_unix_ms":{"type":"timestamp_ms","value":1800000000000},"#,
    r#""value_json":{"type":"string","value":"#,
    r#""[{\"from_unix_ms\":1800000000000,\"id\":\"away\",\"kind\":\"once\","#,
    r#"\"until_unix_ms\":1800003600000}]"}}"#,
    r#"]}"#,
);

fn context() -> CheckContext {
    CheckContext::new(5_000, DEVICE)
}

fn stamp() -> InstructionStamp {
    InstructionStamp {
        set_unix_ms: 1_800_000_000_000,
        set_in_owner_epoch: "epoch_r1".to_owned(),
        request_id: "request_r1".to_owned(),
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

fn blackout_key() -> String {
    instance_key(INSTANCE_A, INSTANCE_BLACKOUT)
}

/// Instance A's own blackout list of one `once` entry, and `fact_refresh.overdue_ms`: the two
/// settable keys without a default the work order names.
fn unwritten_changes() -> Vec<ConfigSettingChange> {
    vec![
        set(
            &blackout_key(),
            json!([{"kind": "once", "id": "away", "from_unix_ms": 1_800_000_000_000_u64,
                    "until_unix_ms": 1_800_003_600_000_u64}]),
        ),
        set(FACT_REFRESH_OVERDUE_MS, json!(600_000)),
    ]
}

// R-1 test 4: `performance.pacing.max_concurrent_runs` has no default and is read once at
// assembly, so its kind is not settable; R-1 lifts only the barrier of a key the file does not
// write, so an instruction naming it is refused `config_key_not_settable` whether the file is
// silent or writes it (before R-1 a silent file answered `config_key_unknown`).
#[test]
fn r1_t4_max_concurrent_runs_stays_not_settable_whether_or_not_the_file_writes_it() {
    let (row, _) = find_row(MAX_CONCURRENT_RUNS).expect("the key is in the key table");
    assert_eq!(row.default, DefaultSource::None);
    assert!(!row.settable.is_settable());
    for (case, file) in [
        ("the file is silent", json!({})),
        (
            "the file writes it",
            json!({"performance": {"pacing": {"max_concurrent_runs": 2}}}),
        ),
    ] {
        let config = from_file(file);
        let refused = config
            .apply(&[set(MAX_CONCURRENT_RUNS, json!(1))], &stamp(), &context())
            .expect_err(case);
        assert_eq!(
            refused.failure_code(),
            ConfigCode::ConfigKeyNotSettable,
            "{case}"
        );
        assert_eq!(refused.config_key(), MAX_CONCURRENT_RUNS, "{case}");
    }
}

// R-1 test 5: over a file that writes neither instance A's blackout list nor
// `fact_refresh.overdue_ms`, one instruction sets both; each takes effect and its
// `host.config_overrides` row records the base as absent, `base_source` `default` and
// `base_json` `null`.
#[test]
fn r1_t5_the_row_of_a_key_the_file_does_not_write_has_base_source_default_and_base_json_null() {
    let config = from_file(json!({}));
    assert_eq!(config.value(&blackout_key()), None);
    assert_eq!(config.value(FACT_REFRESH_OVERDUE_MS), None);

    let applied = config
        .apply(&unwritten_changes(), &stamp(), &context())
        .expect("both keys are set");
    let rows = applied.config.override_rows();
    assert_eq!(
        rows.iter()
            .map(|row| row.get(OVERRIDE_CONFIG_KEY).cloned())
            .collect::<Vec<_>>(),
        vec![
            Some(FactScalar::String(FACT_REFRESH_OVERDUE_MS.to_owned())),
            Some(FactScalar::String(blackout_key())),
        ]
    );
    for row in &rows {
        assert_eq!(
            row.get(OVERRIDE_BASE_SOURCE),
            Some(&FactScalar::String("default".to_owned())),
            "{row:?}"
        );
        assert_eq!(
            row.get(OVERRIDE_BASE_JSON),
            Some(&FactScalar::String("null".to_owned())),
            "{row:?}"
        );
    }
    assert_eq!(
        applied
            .config
            .fact_refresh()
            .map(|refresh| refresh.overdue_ms),
        Ok(Some(600_000))
    );
    assert_eq!(
        applied
            .config
            .instance_blackouts(INSTANCE_A)
            .map(|entries| entries.len()),
        Ok(1)
    );
}

// R-1 test 6 (rollback safety): the fixed sample is what this build records for test 5's
// instructions. Runtime 0.12.30's row parser, unchanged since 0.12.30, reads every row of it;
// a start whose configuration has no setting of those keys, as 0.12.30 has none for a key the
// file does not write, drops each (`config_override_dropped`) and never refuses the record
// (which would fail the start, `host_config_overrides_fact_invalid`); this build keeps both over
// the same file.
#[test]
fn r1_t6_the_0_12_30_row_parser_reads_the_rows_of_keys_the_file_does_not_write() {
    let FactValue::RecordList(rows) =
        serde_json::from_str::<FactValue>(UNWRITTEN_KEY_ROWS).expect("the sample decodes")
    else {
        panic!("the sample is a record list");
    };
    let written = from_file(json!({}))
        .apply(&unwritten_changes(), &stamp(), &context())
        .expect("both keys are set")
        .config
        .override_rows();
    assert_eq!(written, rows, "the sample is what this build records");

    for row in &rows {
        assert!(RecordedOverride::parse(row).is_ok(), "{row:?}");
    }

    let mut without_the_keys = EffectiveConfig::default();
    let notes = without_the_keys
        .restore(&rows, &context())
        .expect("the record has the row shape");
    assert_eq!(
        notes
            .iter()
            .map(|note| (note.note, note.config_key.clone()))
            .collect::<Vec<_>>(),
        vec![
            (
                ConfigCode::ConfigOverrideDropped,
                FACT_REFRESH_OVERDUE_MS.to_owned()
            ),
            (ConfigCode::ConfigOverrideDropped, blackout_key()),
        ]
    );
    assert!(without_the_keys.override_rows().is_empty());

    let mut restored = from_file(json!({}));
    assert_eq!(restored.restore(&rows, &context()), Ok(Vec::new()));
    assert_eq!(restored.override_rows(), rows);
}
