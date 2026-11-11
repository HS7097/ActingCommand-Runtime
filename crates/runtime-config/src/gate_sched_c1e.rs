// SPDX-License-Identifier: AGPL-3.0-only

// #428 SCHED-C1e (register §1.299 SC1-12; the coordinator's point 8 and answers 3, 5 and 8 of
// 11-11, not Alice's words; coordinator work order `rd-1111/brief-sched-c1e.md`;
// contracts/scheduling/README.md, "Lines for one instance"): the per-instance key
// `instance.<instance id>.scheduling.pool_lines`, in the file the `instances[]` entry's
// `scheduling.pool_lines`: settable, no default, an object of at most 32 pools, each
// `{"stop_line", "hard_line", "trigger_line"?}` in the two forms a catalog line has.
// - C1: the row; the file names it for one instance; an instruction sets and clears it.
// - C2: each refusal of the section check, `config_value_invalid` with the key, its problem and
//   the JSON pointer of the pool (and member) in the value; the file layer is refused the same.
// - C3: an instruction names only pools the check context gives the instance with a declared
//   valuation; a context that knows none asks nothing, and a change of another key does not ask
//   again of a value set before.
// - C4: the restore at start (a context without pools) keeps a value whose pool it cannot check.
// - C5: a configuration without the key has no row of it, and the entry's member is the only new
//   member of an `instances[]` entry.

use super::*;
use crate::scheduling::{INSTANCE_POOL_LINES, MAX_POOL_LINES_POOLS};
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, ConfigSettingChange, FactScalar, RuntimeConfigManifest,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const INSTANCE_A: &str = "instance_0123456789abcdef0123456789abcdef";
const INSTANCE_B: &str = "instance_fedcba9876543210fedcba9876543210";
const POOL: &str = "pool.neutral";
const CAPACITY_KEY: &str = "resource.neutral.capacity";
const DEVICE: DeviceBounds = DeviceBounds {
    application_stop_ms: 60_000,
    emulator_stop_ms: 200_000,
    emulator_start_ms: 190_000,
};

fn context() -> CheckContext {
    CheckContext::new(5_000, DEVICE)
}

/// The context of an instruction whose active generation gives instance A `pool.neutral` and
/// `pool.out` with a declared valuation, and instance B nothing.
fn valued_context() -> CheckContext {
    context().with_valued_pools(BTreeMap::from([(
        INSTANCE_A.to_owned(),
        BTreeSet::from([POOL.to_owned(), "pool.out".to_owned()]),
    )]))
}

fn key(instance: &str) -> String {
    instance_key(instance, INSTANCE_POOL_LINES)
}

// The rows a start renders from a file whose instance A names `own_a` as its `scheduling`
// member; instance B names none.
fn render(own_a: Value) -> Vec<ConfigParameter> {
    let mut a = json!({"instance_id": INSTANCE_A, "alias": "node.a"});
    if !own_a.is_null() {
        a["scheduling"] = own_a;
    }
    let b = json!({"instance_id": INSTANCE_B, "alias": "node.b"});
    render_parameters(
        &mechanism_rows(),
        &[INSTANCE_A.to_owned(), INSTANCE_B.to_owned()],
        &DefaultsFile::shipped()
            .expect("the shipped defaults file parses")
            .1,
        &json!({"instances": [a, b]}),
        &|_| None,
    )
    .expect("every key resolves")
}

fn from_file(own_a: Value) -> EffectiveConfig {
    EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters: render(own_a),
    })
    .expect("every key is in the key table")
}

fn stamp() -> InstructionStamp {
    InstructionStamp {
        set_unix_ms: 1,
        set_in_owner_epoch: "epoch".to_owned(),
        request_id: "request".to_owned(),
        channel: None,
    }
}

fn set(instance: &str, value: Value) -> ConfigSettingChange {
    ConfigSettingChange {
        key: key(instance),
        value: Some(value),
    }
}

fn reference(offset: i64) -> Value {
    json!({"type": "fact", "fact_key": CAPACITY_KEY, "offset": offset})
}

fn fact_line(offset: i64) -> PoolLineValue {
    PoolLineValue::Fact {
        fact_key: CAPACITY_KEY.to_owned(),
        offset,
    }
}

// C1: the row is A's own `scheduling.pool_lines`: an object in the entry's `scheduling` member, no
// default, settable, read by SCHED-C1e since 0.12.54. The file names it for A only: A reports its
// canonical text as `explicit` and reads it back as its pools' lines; B reports no row and has
// none. An instruction sets B's (over no base) and clears it again.
#[test]
fn sched_c1e_c1_the_key_is_read_from_the_file_and_set_and_cleared_by_an_instruction() {
    let name = key(INSTANCE_A);
    let (row, instance) = find_row(&name).expect("the key is in the key table");
    assert_eq!(instance, Some(INSTANCE_A));
    assert_eq!(row.name, KeyName::PerInstance(INSTANCE_POOL_LINES));
    assert_eq!(row.file_path, FilePath::InstanceMember);
    assert_eq!(row.value_type, ValueType::Object);
    assert_eq!(row.range, KeyRange::Any);
    assert_eq!(row.default, DefaultSource::None);
    assert_eq!(row.settable, Settable::Yes);
    assert_eq!((row.reader, row.since), ("SCHED-C1e", "0.12.54"));

    let own = json!({POOL: {"stop_line": 150, "hard_line": reference(-76), "trigger_line": 170}});
    let parameters = render(json!({"pool_lines": own}));
    let reported = parameters
        .iter()
        .find(|parameter| parameter.key == key(INSTANCE_A))
        .expect("A's row");
    assert_eq!(reported.value, FactScalar::String(canonical_json(&own)));
    assert_eq!(reported.source, ConfigParameterSource::Explicit);
    assert!(
        parameters
            .iter()
            .all(|parameter| parameter.key != key(INSTANCE_B)),
        "B names none"
    );
    let config = from_file(json!({"pool_lines": own}));
    config.check(&context()).expect("the file's value passes");
    assert_eq!(
        config.pool_lines(INSTANCE_A),
        Ok(BTreeMap::from([(
            POOL.to_owned(),
            InstancePoolLineSet {
                stop_line: PoolLineValue::Constant(150),
                hard_line: fact_line(-76),
                trigger_line: Some(PoolLineValue::Constant(170)),
            }
        )]))
    );
    assert_eq!(config.pool_lines(INSTANCE_B), Ok(BTreeMap::new()));

    let b_context = context().with_valued_pools(BTreeMap::from([(
        INSTANCE_B.to_owned(),
        BTreeSet::from([POOL.to_owned()]),
    )]));
    let applied = config
        .apply(
            &[set(
                INSTANCE_B,
                json!({POOL: {"stop_line": 140, "hard_line": 104}}),
            )],
            &stamp(),
            &b_context,
        )
        .expect("B's lines are set");
    let [setting] = applied.settings.as_slice() else {
        panic!("one setting: {applied:?}");
    };
    assert_eq!(setting.source, ConfigParameterSource::Instruction);
    assert_eq!(
        applied.config.pool_lines(INSTANCE_B),
        Ok(BTreeMap::from([(
            POOL.to_owned(),
            InstancePoolLineSet {
                stop_line: PoolLineValue::Constant(140),
                hard_line: PoolLineValue::Constant(104),
                trigger_line: None,
            }
        )]))
    );
    let row = applied
        .config
        .override_rows()
        .into_iter()
        .find(|row| row.get(OVERRIDE_CONFIG_KEY) == Some(&FactScalar::String(key(INSTANCE_B))))
        .expect("B's instruction row");
    assert_eq!(
        row.get(OVERRIDE_BASE_JSON),
        Some(&FactScalar::String("null".to_owned()))
    );
    assert_eq!(
        row.get(OVERRIDE_BASE_SOURCE),
        Some(&FactScalar::String("default".to_owned()))
    );
    let cleared = applied
        .config
        .apply(
            &[ConfigSettingChange {
                key: key(INSTANCE_B),
                value: None,
            }],
            &stamp(),
            &b_context,
        )
        .expect("B's lines are cleared");
    assert_eq!(cleared.config.pool_lines(INSTANCE_B), Ok(BTreeMap::new()));
    assert_eq!(
        cleared.config.pool_lines(INSTANCE_A),
        config.pool_lines(INSTANCE_A),
        "A's stay"
    );
}

// C2: each refusal of the section check is `config_value_invalid` for A's key with its problem and
// the JSON pointer, into the value, of what breaks the rule; nothing changes. A value whose lines
// the values cannot order (a constant beside a reference) passes. The file layer is refused the
// same way at start and by check-config.
#[test]
fn sched_c1e_c2_each_refusal_of_the_section_check_names_the_key_and_its_problem() {
    use ConfigValueProblem::{OutOfRange, SectionCheck, WrongType};
    let lines = |members: Value| json!({POOL: members});
    let many = Value::Object(
        (0..=MAX_POOL_LINES_POOLS)
            .map(|index| {
                (
                    format!("pool.{index:02}"),
                    json!({"stop_line": 150, "hard_line": 110}),
                )
            })
            .collect(),
    );
    let mut scoped = reference(-40);
    scoped["scope"] = json!({"kind": "instance", "instance_id": "node.a"});
    let mut other_type = reference(-40);
    other_type["type"] = json!("value");
    let mut text_offset = reference(-40);
    text_offset["offset"] = json!("-40");
    let mut far_offset = reference(-40);
    far_offset["offset"] = json!(9_007_199_254_740_992_i64);
    let mut unnamed = reference(-40);
    unnamed["fact_key"] = json!("Not A Key");
    let cases: Vec<(&str, Value, ConfigValueProblem, Option<&str>)> = vec![
        ("a list", json!([]), WrongType, None),
        (
            "a pool that is no object",
            lines(json!(5)),
            WrongType,
            Some("/pool.neutral"),
        ),
        (
            "a member of no line",
            lines(json!({"stop_line": 150, "hard_line": 110, "scope": 1})),
            WrongType,
            Some("/pool.neutral/scope"),
        ),
        (
            "a line of text",
            lines(json!({"stop_line": "150", "hard_line": 110})),
            WrongType,
            Some("/pool.neutral/stop_line"),
        ),
        (
            "a line of no integer",
            lines(json!({"stop_line": 1.5, "hard_line": 110})),
            WrongType,
            Some("/pool.neutral/stop_line"),
        ),
        (
            "a reference that names a scope",
            lines(json!({"stop_line": scoped, "hard_line": 110})),
            WrongType,
            Some("/pool.neutral/stop_line/scope"),
        ),
        (
            "a reference of another type",
            lines(json!({"stop_line": other_type, "hard_line": 110})),
            WrongType,
            Some("/pool.neutral/stop_line/type"),
        ),
        (
            "an offset of text",
            lines(json!({"stop_line": text_offset, "hard_line": 110})),
            WrongType,
            Some("/pool.neutral/stop_line/offset"),
        ),
        (
            "a member that is no pool identifier",
            json!({"Pool Neutral": {"stop_line": 150, "hard_line": 110}}),
            OutOfRange,
            Some("/Pool Neutral"),
        ),
        ("a 33rd pool", many, OutOfRange, Some("/pool.32")),
        (
            "a constant above 2^53 - 1",
            lines(json!({"stop_line": 9_007_199_254_740_992_u64, "hard_line": 110})),
            OutOfRange,
            Some("/pool.neutral/stop_line"),
        ),
        (
            "a negative constant",
            lines(json!({"stop_line": 150, "hard_line": -1})),
            OutOfRange,
            Some("/pool.neutral/hard_line"),
        ),
        (
            "an offset outside the safe range",
            lines(json!({"stop_line": far_offset, "hard_line": 110})),
            OutOfRange,
            Some("/pool.neutral/stop_line/offset"),
        ),
        (
            "a fact key that is no identifier",
            lines(json!({"stop_line": unnamed, "hard_line": 110})),
            OutOfRange,
            Some("/pool.neutral/stop_line/fact_key"),
        ),
        (
            "no hard line",
            lines(json!({"stop_line": 150})),
            SectionCheck,
            Some("/pool.neutral/hard_line"),
        ),
        (
            "no stop line",
            lines(json!({"hard_line": 110, "trigger_line": 170})),
            SectionCheck,
            Some("/pool.neutral/stop_line"),
        ),
        (
            "a hard line at the stop line",
            lines(json!({"stop_line": 110, "hard_line": 110})),
            SectionCheck,
            Some("/pool.neutral/hard_line"),
        ),
        (
            "two references, the hard line above",
            lines(json!({"stop_line": reference(-80), "hard_line": reference(-40)})),
            SectionCheck,
            Some("/pool.neutral/hard_line"),
        ),
        (
            "a trigger line below the stop line",
            lines(json!({"stop_line": 150, "hard_line": 110, "trigger_line": 140})),
            SectionCheck,
            Some("/pool.neutral/trigger_line"),
        ),
        (
            "a trigger reference below the stop reference",
            lines(json!({
                "stop_line": reference(-40),
                "hard_line": reference(-76),
                "trigger_line": reference(-50)
            })),
            SectionCheck,
            Some("/pool.neutral/trigger_line"),
        ),
    ];
    let config = from_file(Value::Null);
    for (case, value, problem, pointer) in cases {
        let refused = config
            .apply(&[set(INSTANCE_A, value)], &stamp(), &valued_context())
            .expect_err(case);
        assert_eq!(
            refused.failure_code(),
            ConfigCode::ConfigValueInvalid,
            "{case}"
        );
        assert_eq!(refused.config_key(), key(INSTANCE_A), "{case}");
        assert_eq!(refused.problem(), Some(problem), "{case}");
        assert_eq!(refused.pointer(), pointer, "{case}");
        assert_eq!(config.pool_lines(INSTANCE_A), Ok(BTreeMap::new()), "{case}");
    }
    let unordered = lines(json!({"stop_line": 110, "hard_line": reference(-76)}));
    let applied = config
        .apply(&[set(INSTANCE_A, unordered)], &stamp(), &valued_context())
        .expect("a constant beside a reference is checked at the evaluation");
    assert_eq!(
        applied.config.pool_lines(INSTANCE_A),
        Ok(BTreeMap::from([(
            POOL.to_owned(),
            InstancePoolLineSet {
                stop_line: PoolLineValue::Constant(110),
                hard_line: fact_line(-76),
                trigger_line: None,
            }
        )]))
    );

    let refused = from_file(json!({"pool_lines": lines(json!({"stop_line": 150}))}))
        .check(&context())
        .expect_err("the file layer is checked");
    assert_eq!(refused.failure_code(), ConfigCode::ConfigValueInvalid);
    assert_eq!(refused.config_key(), key(INSTANCE_A));
    assert_eq!(refused.problem(), Some(SectionCheck));
}

// C3: with the active generation's valued pools in the context, an instruction naming a pool it
// does not give A (or gives B nothing) is refused, `config_value_invalid` `section_check` for the
// instance's key, its pointer naming the pool; one naming A's valued pool passes. A context without
// pools asks nothing. A change of another key of A does not ask again of A's value set before,
// whose pool the generation no longer gives.
#[test]
fn sched_c1e_c3_an_instruction_names_only_the_valued_pools_the_generation_gives_the_instance() {
    let config = from_file(Value::Null);
    let lines = |pool: &str| json!({pool: {"stop_line": 150, "hard_line": 110}});
    for (instance, pool) in [(INSTANCE_A, "pool.other"), (INSTANCE_B, POOL)] {
        let refused = config
            .apply(&[set(instance, lines(pool))], &stamp(), &valued_context())
            .expect_err(pool);
        assert_eq!(refused.failure_code(), ConfigCode::ConfigValueInvalid);
        assert_eq!(refused.config_key(), key(instance));
        assert_eq!(refused.problem(), Some(ConfigValueProblem::SectionCheck));
        assert_eq!(refused.pointer(), Some(format!("/{pool}").as_str()));
    }
    config
        .apply(&[set(INSTANCE_A, lines(POOL))], &stamp(), &valued_context())
        .expect("A's valued pool");
    config
        .apply(
            &[set(INSTANCE_A, lines("pool.other"))],
            &stamp(),
            &context(),
        )
        .expect("a context without pools asks nothing");

    let earlier = from_file(json!({"pool_lines": lines("pool.gone")}));
    earlier
        .apply(
            &[ConfigSettingChange {
                key: instance_key(INSTANCE_A, STUCK_RECOVERY),
                value: Some(json!(false)),
            }],
            &stamp(),
            &valued_context(),
        )
        .expect("another key does not ask again");
}

// C4: the restore at start checks with a context that holds no pools, so it keeps an instruction
// value naming a pool no generation gives the instance, as it keeps one naming a valued pool: the
// value is in effect again and nothing is noted.
#[test]
fn sched_c1e_c4_the_restore_at_start_keeps_a_value_whose_pool_it_cannot_check() {
    for (pool, at_set) in [(POOL, valued_context()), ("pool.gone", context())] {
        let value = json!({pool: {"stop_line": 150, "hard_line": 110, "trigger_line": 170}});
        let applied = from_file(Value::Null)
            .apply(&[set(INSTANCE_A, value)], &stamp(), &at_set)
            .expect("the value is set");
        let rows = applied.config.override_rows();
        let mut restarted = from_file(Value::Null);
        let notes = restarted
            .restore(&rows, &context())
            .expect("the rows are this build's");
        assert!(notes.is_empty(), "{pool}: {notes:?}");
        assert_eq!(
            restarted.pool_lines(INSTANCE_A),
            applied.config.pool_lines(INSTANCE_A),
            "{pool}"
        );
        assert_eq!(
            restarted.pool_lines(INSTANCE_A).map(|lines| lines.len()),
            Ok(1),
            "{pool}"
        );
    }
}

// C5: a configuration that names no pool lines reports no row of the key and passes its checks;
// the entry's `scheduling` member reads `pool_lines` and writes it back as read, and leaves it out
// when it is absent.
#[test]
fn sched_c1e_c5_a_configuration_without_the_key_has_no_row_of_it() {
    let parameters = render(Value::Null);
    assert!(
        parameters
            .iter()
            .all(|parameter| !parameter.key.ends_with(INSTANCE_POOL_LINES)),
        "no row"
    );
    from_file(Value::Null)
        .check(&context())
        .expect("the configuration passes");
    let none = serde_json::from_value::<InstanceSchedulingFile>(json!({}))
        .expect("an entry's scheduling member without pool lines");
    assert_eq!(serde_json::to_value(&none).expect("encodes"), json!({}));
    let own = json!({"pool_lines": {POOL: {"stop_line": 150, "hard_line": 110}}});
    let named = serde_json::from_value::<InstanceSchedulingFile>(own.clone())
        .expect("an entry's scheduling member with pool lines");
    assert_eq!(serde_json::to_value(&named).expect("encodes"), own);
}
