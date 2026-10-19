// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #386 SR-1 (frozen scheduling model v1.4.2 §9; `contracts/v012-register.md` §5): the
// `scheduling` keys are parsed, checked and defaulted in v0.12.0, and Workflow #405's random
// task delay (the coordinator's order of 2026-10-14) per instance as well. The defaults are the
// model's §9 table and the order's values; the rules are the model's (§5.1 b host blackout
// entries, §4.1 and §7.2 code lists) and the order's (`0 <= min_ms <= max_ms <= 86400000`,
// checked on the pair in effect).

use super::*;
use crate::scheduling::{
    BLACKOUT_HARD_QUIET_MS, BLACKOUT_HOST, BLACKOUT_QUIET_MARGIN_MS,
    BLACKOUT_STOP_IN_FLIGHT_AT_START, COST_COMMIT_MARGIN_MS, COST_COMMIT_MAX_STAGE,
    COST_COMMIT_PROMOTE, FEATURE_LOCK_CODES, FEATURE_LOCK_LOCKED_VALID_FOR_MS,
    NOT_RUN_DIAGNOSE_ROWS, RANDOM_DELAY_ENABLED, RANDOM_DELAY_MAX_MS, RANDOM_DELAY_MIN_MS,
    REMEDY_CHOICES, REMEDY_ENABLED, REMEDY_MAX_PER_ROUND, RERUN_CONDITION_CODES, RERUN_EXPIRES,
    RERUN_MAX_PER_OCCURRENCE, RERUN_PROMOTE, RERUN_REFUND_COUNTS, RERUN_RESUME_SETTLE_MS,
    RERUN_RESUME_STAGE, SAMPLE_TIMEOUT_RETRIES,
};
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, ConfigSettingChange, FactScalar, RuntimeConfigManifest,
};
use serde_json::{Value, json};

const INSTANCE_A: &str = "instance_0123456789abcdef0123456789abcdef";
const DEVICE: DeviceBounds = DeviceBounds {
    application_stop_ms: 60_000,
    emulator_stop_ms: 200_000,
    emulator_start_ms: 190_000,
};
// SCHED §9 `scheduling.rerun.condition_codes`: the eight codes that end a run by a condition.
const CONDITION_CODES: [&str; 8] = [
    "recognition_sample_budget_insufficient",
    "candidate_sample_budget_insufficient",
    "recognition_sample_deadline_exceeded",
    "candidate_sample_deadline_exceeded",
    "blackout_run_stopped",
    "perf_run_yielded",
    "perf_hold_expired",
    "cost_commit_time_insufficient",
];

fn context() -> CheckContext {
    CheckContext::new(5_000, DEVICE)
}

fn shipped() -> Value {
    DefaultsFile::shipped()
        .expect("the shipped defaults file parses")
        .1
}

// The rows a start renders from a file of `scheduling` and, for instance A, `instance`.
fn render(scheduling: Value, instance: Value) -> Result<Vec<ConfigParameter>, ConfigFailure> {
    let mut entry = json!({"instance_id": INSTANCE_A, "alias": "node.a"});
    if let Value::Object(members) = instance {
        entry
            .as_object_mut()
            .expect("an instance entry")
            .extend(members);
    }
    render_parameters(
        &mechanism_rows(),
        &[INSTANCE_A.to_owned()],
        &shipped(),
        &json!({"scheduling": scheduling, "instances": [entry]}),
        &|_| None,
    )
}

fn from_file(scheduling: Value, instance: Value) -> EffectiveConfig {
    EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters: render(scheduling, instance).expect("every key resolves"),
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

fn set(key: &str, value: Value) -> ConfigSettingChange {
    ConfigSettingChange {
        key: key.to_owned(),
        value: Some(value),
    }
}

fn list(value: &FactScalar) -> Value {
    match value {
        FactScalar::String(text) => serde_json::from_str(text).expect("a list's canonical JSON"),
        other => panic!("a list is reported as JSON text: {other:?}"),
    }
}

// SCHED §9 and the #405 order: every key of the section, defaulted from the defaults file
// (source `default`), settable while the Runtime runs; an instance's random delay defaults to
// the section's. The rerun codes are the model's eight (SH-1 registers `perf_run_yielded` and
// `perf_hold_expired`).
#[test]
fn sr1_c1_every_scheduling_key_has_its_model_default() {
    let parameters = render(json!({}), json!({})).expect("the defaults resolve");
    let reported = |key: &str| {
        parameters
            .iter()
            .find(|parameter| parameter.key == key)
            .unwrap_or_else(|| panic!("no parameter {key}"))
    };
    let expected = [
        (SAMPLE_TIMEOUT_RETRIES, FactScalar::Integer(2)),
        (RERUN_MAX_PER_OCCURRENCE, FactScalar::Integer(2)),
        (RERUN_RESUME_STAGE, FactScalar::String("s2".to_owned())),
        (RERUN_RESUME_SETTLE_MS, FactScalar::DurationMs(60_000)),
        (RERUN_PROMOTE, FactScalar::Boolean(true)),
        (RERUN_REFUND_COUNTS, FactScalar::Boolean(true)),
        (RERUN_EXPIRES, FactScalar::String("window_end".to_owned())),
        (COST_COMMIT_PROMOTE, FactScalar::Boolean(true)),
        (COST_COMMIT_MAX_STAGE, FactScalar::String("s2".to_owned())),
        (COST_COMMIT_MARGIN_MS, FactScalar::DurationMs(60_000)),
        (BLACKOUT_HOST, FactScalar::String("[]".to_owned())),
        (BLACKOUT_QUIET_MARGIN_MS, FactScalar::DurationMs(60_000)),
        (BLACKOUT_HARD_QUIET_MS, FactScalar::DurationMs(0)),
        (BLACKOUT_STOP_IN_FLIGHT_AT_START, FactScalar::Boolean(true)),
        (
            FEATURE_LOCK_LOCKED_VALID_FOR_MS,
            FactScalar::DurationMs(86_400_000),
        ),
        (NOT_RUN_DIAGNOSE_ROWS, FactScalar::Integer(10)),
        (RANDOM_DELAY_ENABLED, FactScalar::Boolean(false)),
        (RANDOM_DELAY_MIN_MS, FactScalar::DurationMs(60_000)),
        (RANDOM_DELAY_MAX_MS, FactScalar::DurationMs(3_600_000)),
    ];
    for (key, value) in &expected {
        let parameter = reported(key);
        assert_eq!(&parameter.value, value, "{key}");
        assert_eq!(parameter.source, ConfigParameterSource::Default, "{key}");
        assert!(parameter.settable, "{key}");
    }
    assert_eq!(
        list(&reported(RERUN_CONDITION_CODES).value),
        json!(CONDITION_CODES)
    );
    assert_eq!(
        list(&reported(FEATURE_LOCK_CODES).value),
        json!(["contained_task_feature_locked"])
    );
    for member in [
        RANDOM_DELAY_ENABLED,
        RANDOM_DELAY_MIN_MS,
        RANDOM_DELAY_MAX_MS,
    ] {
        let parameter = reported(&instance_key(INSTANCE_A, member));
        assert_eq!(parameter.value, reported(member).value, "{member}");
        assert_eq!(parameter.source, ConfigParameterSource::Default, "{member}");
        assert!(parameter.settable, "{member}");
    }

    let config = from_file(json!({}), json!({}));
    config
        .check(&context())
        .expect("the defaults pass the check");
    assert_eq!(config.sample_timeout_retries(), Ok(2));
    let section = config.scheduling().expect("the section in effect");
    assert_eq!(section.sample_timeout_retries, 2);
    assert!(section.blackout.host.is_empty());
    assert_eq!(
        from_file(
            json!({"recognition": {"sample_timeout_retries": 0}}),
            json!({})
        )
        .sample_timeout_retries(),
        Ok(0)
    );
}

// One refused file: the `scheduling` members, the code, the key, why, and for a host blackout
// entry the member pointer and the entry's id.
struct Refusal {
    case: &'static str,
    scheduling: Value,
    code: ConfigCode,
    config_key: &'static str,
    problem: Option<ConfigValueProblem>,
    blackout: Option<(&'static str, Option<&'static str>)>,
}

fn night(weekdays: Value) -> Value {
    json!({"id": "night", "weekdays": weekdays, "utc_offset_minutes": 540,
           "start_minute_of_day": 1380, "end_minute_of_day": 360})
}

fn refusals() -> Vec<Refusal> {
    let value = |case, scheduling, config_key, problem| Refusal {
        case,
        scheduling,
        code: ConfigCode::ConfigValueInvalid,
        config_key,
        problem: Some(problem),
        blackout: None,
    };
    let entry = |case, host, field, id| Refusal {
        case,
        scheduling: json!({"blackout": {"host": host}}),
        code: ConfigCode::BlackoutConfigInvalid,
        config_key: BLACKOUT_HOST,
        problem: None,
        blackout: Some((field, id)),
    };
    vec![
        value(
            "a negative retry count",
            json!({"recognition": {"sample_timeout_retries": -1}}),
            SAMPLE_TIMEOUT_RETRIES,
            ConfigValueProblem::OutOfRange,
        ),
        value(
            "a retry count as text",
            json!({"recognition": {"sample_timeout_retries": "2"}}),
            SAMPLE_TIMEOUT_RETRIES,
            ConfigValueProblem::WrongType,
        ),
        value(
            "a stage outside the vocabulary",
            json!({"rerun": {"resume_stage": "s5"}}),
            RERUN_RESUME_STAGE,
            ConfigValueProblem::OutOfRange,
        ),
        value(
            "an expiry other than the window end",
            json!({"rerun": {"expires": "never"}}),
            RERUN_EXPIRES,
            ConfigValueProblem::OutOfRange,
        ),
        value(
            "a condition code the catalog does not list",
            json!({"rerun": {"condition_codes": ["neutral_unlisted_failure"]}}),
            RERUN_CONDITION_CODES,
            ConfigValueProblem::CodeUnknown,
        ),
        value(
            "a retired condition code",
            json!({"rerun": {"condition_codes": ["recognition_sample_deadline_or_permission_lost"]}}),
            RERUN_CONDITION_CODES,
            ConfigValueProblem::CodeRetired,
        ),
        value(
            "a feature lock code that is no text",
            json!({"feature_lock": {"codes": [1]}}),
            FEATURE_LOCK_CODES,
            ConfigValueProblem::WrongType,
        ),
        value(
            "a random delay longer than a day",
            json!({"random_delay": {"max_ms": 86_400_001}}),
            RANDOM_DELAY_MAX_MS,
            ConfigValueProblem::OutOfRange,
        ),
        value(
            "a shortest random delay above the longest",
            json!({"random_delay": {"min_ms": 7_200_000}}),
            RANDOM_DELAY_MIN_MS,
            ConfigValueProblem::SectionCheck,
        ),
        entry(
            "a weekday outside 1 to 7",
            json!([night(json!([0]))]),
            "/scheduling/blackout/host/0/weekdays",
            Some("night"),
        ),
        entry(
            "a weekday twice",
            json!([night(json!([1, 1]))]),
            "/scheduling/blackout/host/0/weekdays",
            Some("night"),
        ),
        entry(
            "two entries with one id",
            json!([night(json!([1])), night(json!([4]))]),
            "/scheduling/blackout/host/1/id",
            Some("night"),
        ),
        entry(
            "a start minute past the day",
            json!([{"id": "late", "weekdays": [1], "utc_offset_minutes": 0,
                    "start_minute_of_day": 1440, "end_minute_of_day": 0}]),
            "/scheduling/blackout/host/0/start_minute_of_day",
            Some("late"),
        ),
        entry(
            "an offset beyond fourteen hours",
            json!([{"id": "far", "weekdays": [1], "utc_offset_minutes": 900,
                    "start_minute_of_day": 0, "end_minute_of_day": 60}]),
            "/scheduling/blackout/host/0/utc_offset_minutes",
            Some("far"),
        ),
        entry(
            "an entry without its end",
            json!([{"id": "open", "weekdays": [1], "utc_offset_minutes": 0,
                    "start_minute_of_day": 0}]),
            "/scheduling/blackout/host/0",
            Some("open"),
        ),
        entry(
            "an entry without an id",
            json!([{"weekdays": [1], "utc_offset_minutes": 0, "start_minute_of_day": 0,
                    "end_minute_of_day": 60}]),
            "/scheduling/blackout/host/0",
            None,
        ),
    ]
}

// SCHED §5.1 b, §4.1, §7.2 and the #405 order: a value of the wrong type or outside its range is
// refused as the file layer is rendered, a broken section rule by the check that a start and
// check-config make; a host blackout entry's rule names the member and the entry. Through an
// instruction the same values are `config_value_invalid` and the configuration in effect stays.
#[test]
fn sr1_c2_scheduling_values_that_break_a_rule_are_refused() {
    for row in refusals() {
        let refused = match render(row.scheduling.clone(), json!({})) {
            Err(refused) => refused,
            Ok(parameters) => EffectiveConfig::from_manifest(&RuntimeConfigManifest {
                subsystems: Vec::new(),
                parameters,
            })
            .expect("every key is in the key table")
            .check(&context())
            .expect_err(row.case),
        };
        assert_eq!(refused.failure_code(), row.code, "{}", row.case);
        assert_eq!(refused.config_key(), row.config_key, "{}", row.case);
        assert_eq!(refused.problem(), row.problem, "{}", row.case);
        assert_eq!(
            refused.field().zip(Some(refused.blackout_id())),
            row.blackout,
            "{}",
            row.case
        );
    }

    // Through an instruction: a broken host blackout entry and a reversed pair.
    let running = from_file(json!({}), json!({}));
    for (changes, config_key) in [
        (
            vec![set(BLACKOUT_HOST, json!([night(json!([0]))]))],
            BLACKOUT_HOST,
        ),
        (
            vec![set(RANDOM_DELAY_MIN_MS, json!(5_000_000))],
            RANDOM_DELAY_MIN_MS,
        ),
    ] {
        let refused = running
            .apply(&changes, &stamp(), &context())
            .expect_err(config_key);
        assert_eq!(refused.failure_code(), ConfigCode::ConfigValueInvalid);
        assert_eq!(refused.config_key(), config_key);
        assert_eq!(refused.problem(), Some(ConfigValueProblem::SectionCheck));
        assert_eq!(refused.field(), None);
    }
    assert_eq!(
        running.scheduling(),
        from_file(json!({}), json!({})).scheduling()
    );
}

// The #405 order: an instance's random delay is its own `min_ms`, `max_ms` and `enabled`, or the
// section's where it names none; `min_ms` is at most `max_ms` as the pair is in effect, so a
// value that sets one side alone is checked against the other, at start and through an
// instruction; every member is settable while the Runtime runs.
#[test]
fn sr1_c3_an_instance_random_delay_is_checked_as_the_pair_in_effect() {
    let own = |members: Value| json!({"scheduling": {"random_delay": members}});
    let key = |member| instance_key(INSTANCE_A, member);

    // Its own shortest delay above the section's longest.
    let refused = from_file(json!({}), own(json!({"min_ms": 4_000_000})))
        .check(&context())
        .expect_err("a shortest delay above the longest in effect");
    assert_eq!(refused.failure_code(), ConfigCode::ConfigValueInvalid);
    assert_eq!(refused.config_key(), key(RANDOM_DELAY_MIN_MS));
    assert_eq!(refused.problem(), Some(ConfigValueProblem::SectionCheck));

    // Its own shortest delay within the section's longest.
    let running = from_file(json!({}), own(json!({"min_ms": 120_000})));
    running
        .check(&context())
        .expect("a shortest delay below the longest in effect");
    assert_eq!(
        running.value(&key(RANDOM_DELAY_MIN_MS)),
        Some(&FactScalar::DurationMs(120_000))
    );
    assert_eq!(
        running.value(&key(RANDOM_DELAY_MAX_MS)),
        Some(&FactScalar::DurationMs(3_600_000))
    );

    // An instruction on one side is checked against the other side in effect.
    let refused = running
        .apply(
            &[set(&key(RANDOM_DELAY_MAX_MS), json!(60_000))],
            &stamp(),
            &context(),
        )
        .expect_err("a longest delay below the instance's shortest");
    assert_eq!(refused.failure_code(), ConfigCode::ConfigValueInvalid);
    assert_eq!(refused.config_key(), key(RANDOM_DELAY_MIN_MS));
    assert_eq!(refused.problem(), Some(ConfigValueProblem::SectionCheck));
    let refused = running
        .apply(
            &[set(RANDOM_DELAY_MAX_MS, json!(100_000))],
            &stamp(),
            &context(),
        )
        .expect_err("the section's longest below the instance's shortest");
    assert_eq!(refused.config_key(), key(RANDOM_DELAY_MIN_MS));

    let applied = running
        .apply(
            &[set(&key(RANDOM_DELAY_ENABLED), json!(true))],
            &stamp(),
            &context(),
        )
        .expect("enabling the instance's delay");
    assert_eq!(
        applied.config.value(&key(RANDOM_DELAY_ENABLED)),
        Some(&FactScalar::Boolean(true))
    );
    assert_eq!(
        applied.settings[0].source,
        ConfigParameterSource::Instruction
    );
}

// ---- Workflow #411 B15 R3 (model-b15 §5.1, §5.2; register §1.113; host key 112) ----
// The remedy keys: `scheduling.remedy.enabled` (default `true`), `.max_per_round` (`1..=8`,
// default 2), `.choices` (default `[]`) and each instance's `scheduling.remedy.choices`, whose
// default is the section's list and which, when set, replaces it as a whole. Rows keep the shape
// of `session.remedy.choices`. Names are neutral placeholders.

const INSTANCE_B: &str = "instance_fedcba9876543210fedcba9876543210";

fn allow(need: &str, capability: &str) -> Value {
    json!({"row": "allow", "need": need, "capability": capability})
}

fn decline(need: &str) -> Value {
    json!({"row": "none", "need": need})
}

fn parameter_row(capability: &str, parameter: &str, value: &str) -> Value {
    json!({"row": "parameter", "capability": capability, "parameter": parameter, "value": value})
}

fn allowed(need: &str, capability: &str) -> RemedyChoiceRow {
    RemedyChoiceRow::Allow {
        need: need.to_owned(),
        capability: capability.to_owned(),
    }
}

// The configuration of a file whose `scheduling` member is `scheduling` and whose entries of
// instances A and B carry `a` and `b` as their own `scheduling` members (`null`: none).
fn two_instances(scheduling: Value, a: Value, b: Value) -> EffectiveConfig {
    let entry = |instance: &str, alias: &str, own: Value| {
        let mut entry = json!({"instance_id": instance, "alias": alias});
        if !own.is_null() {
            entry["scheduling"] = own;
        }
        entry
    };
    let parameters = render_parameters(
        &mechanism_rows(),
        &[INSTANCE_A.to_owned(), INSTANCE_B.to_owned()],
        &shipped(),
        &json!({"scheduling": scheduling,
                "instances": [entry(INSTANCE_A, "node.a", a), entry(INSTANCE_B, "node.b", b)]}),
        &|_| None,
    )
    .expect("every key resolves");
    EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    })
    .expect("every key is in the key table")
}

// Requirement 1: the three section keys and the instance's list default from the shipped
// defaults file (the instance's through the section's key), source `default`, all settable; the
// accessor gives the same values; a defaults file without them leaves the keys without a value
// (no code constant stands in).
#[test]
fn b15c_remedy_keys_default_from_the_shipped_file() {
    let parameters = render(json!({}), json!({})).expect("the defaults resolve");
    let own_key = instance_key(INSTANCE_A, REMEDY_CHOICES);
    for (key, value, default) in [
        (
            REMEDY_ENABLED,
            FactScalar::Boolean(true),
            DefaultSource::File,
        ),
        (
            REMEDY_MAX_PER_ROUND,
            FactScalar::Integer(2),
            DefaultSource::File,
        ),
        (
            REMEDY_CHOICES,
            FactScalar::String("[]".to_owned()),
            DefaultSource::File,
        ),
        (
            own_key.as_str(),
            FactScalar::String("[]".to_owned()),
            DefaultSource::Section(REMEDY_CHOICES),
        ),
    ] {
        let parameter = parameters
            .iter()
            .find(|parameter| parameter.key == key)
            .unwrap_or_else(|| panic!("no parameter {key}"));
        assert_eq!(parameter.value, value, "{key}");
        assert_eq!(parameter.source, ConfigParameterSource::Default, "{key}");
        assert!(parameter.settable, "{key}");
        let (row, _) = find_row(key).unwrap_or_else(|| panic!("{key} is in the key table"));
        assert_eq!(row.default, default, "{key}");
    }

    let config = from_file(json!({}), json!({}));
    config
        .check(&context())
        .expect("the defaults pass the check");
    let shipped_remedy = RemedyConfig {
        enabled: true,
        max_per_round: 2,
        rows: Vec::new(),
    };
    assert_eq!(config.remedy(INSTANCE_A), Ok(shipped_remedy.clone()));
    assert_eq!(
        config.scheduling().map(|section| section.remedy),
        Ok(shipped_remedy)
    );

    let mut defaults = shipped();
    defaults["scheduling"]
        .as_object_mut()
        .expect("the defaults file's scheduling section")
        .remove("remedy");
    let refused = render_parameters(
        &mechanism_rows(),
        &[INSTANCE_A.to_owned()],
        &defaults,
        &json!({"instances": [{"instance_id": INSTANCE_A, "alias": "node.a"}]}),
        &|_| None,
    )
    .expect_err("no remedy default outside the defaults file");
    assert_eq!(refused.failure_code(), ConfigCode::ConfigValueInvalid);
    assert_eq!(refused.config_key(), REMEDY_ENABLED);
}

// Requirements 1 and 3: an instance that sets its own list has exactly that list in effect, never
// the section's rows added to it; an instance without one has the section's; a new section list
// reaches only the instances without their own; an empty list an instance sets is its list too.
// The switch and the request bound are the section's for every instance.
#[test]
fn b15c_an_instance_list_replaces_the_section_list_as_a_whole() {
    let section = json!([
        allow("need_a", "cap_x"),
        parameter_row("cap_x", "param_p", "opt_1")
    ]);
    let config = two_instances(
        json!({"remedy": {"max_per_round": 3, "choices": section}}),
        json!({"remedy": {"choices": [allow("need_b", "cap_y")]}}),
        Value::Null,
    );
    config.check(&context()).expect("both lists keep the rules");
    let section_rows = vec![
        allowed("need_a", "cap_x"),
        RemedyChoiceRow::Parameter {
            capability: "cap_x".to_owned(),
            parameter: "param_p".to_owned(),
            value: "opt_1".to_owned(),
        },
    ];
    let own_rows = vec![allowed("need_b", "cap_y")];
    let in_effect = |config: &EffectiveConfig, instance: &str| {
        config
            .remedy(instance)
            .unwrap_or_else(|failure| panic!("{instance}: {failure:?}"))
    };
    assert_eq!(
        in_effect(&config, INSTANCE_A),
        RemedyConfig {
            enabled: true,
            max_per_round: 3,
            rows: own_rows.clone(),
        }
    );
    assert_eq!(
        in_effect(&config, INSTANCE_B),
        RemedyConfig {
            enabled: true,
            max_per_round: 3,
            rows: section_rows,
        }
    );
    let manifest = config.manifest();
    let source = |instance: &str| {
        let key = instance_key(instance, REMEDY_CHOICES);
        manifest
            .parameters
            .iter()
            .find(|parameter| parameter.key == key)
            .map(|parameter| parameter.source)
    };
    assert_eq!(source(INSTANCE_A), Some(ConfigParameterSource::Explicit));
    assert_eq!(source(INSTANCE_B), Some(ConfigParameterSource::Default));

    let changed = config
        .apply(
            &[set(REMEDY_CHOICES, json!([decline("need_c")]))],
            &stamp(),
            &context(),
        )
        .expect("a new section list")
        .config;
    assert_eq!(
        in_effect(&changed, INSTANCE_B).rows,
        vec![RemedyChoiceRow::NoRemedy {
            need: "need_c".to_owned()
        }]
    );
    assert_eq!(in_effect(&changed, INSTANCE_A).rows, own_rows);

    let emptied = changed
        .apply(
            &[set(&instance_key(INSTANCE_B, REMEDY_CHOICES), json!([]))],
            &stamp(),
            &context(),
        )
        .expect("an empty instance list")
        .config;
    assert_eq!(in_effect(&emptied, INSTANCE_B).rows, Vec::new());
    assert_eq!(in_effect(&emptied, INSTANCE_A).rows, own_rows);
}

// Requirement 2: one case per rule of the row shape. Each is `config_value_invalid` with the key
// and the pointer of the row (and member) in the key's value: in the section's list and in an
// instance's own list as a start and check-config check them, and through an instruction on
// either key.
#[test]
fn b15c_malformed_rows_are_refused_with_their_pointer() {
    let too_many = (0..257)
        .map(|index| allow("need_a", &format!("cap_{index}")))
        .collect::<Vec<_>>();
    let cases = [
        (
            "two values for one parameter",
            json!([
                parameter_row("cap_x", "param_p", "opt_1"),
                allow("need_a", "cap_x"),
                parameter_row("cap_x", "param_p", "opt_2")
            ]),
            "/2",
            ConfigValueProblem::SectionCheck,
        ),
        (
            "a row that is no object",
            json!(["allow"]),
            "/0",
            ConfigValueProblem::WrongType,
        ),
        (
            "a row kind that does not exist",
            json!([{"row": "deny", "need": "need_a"}]),
            "/0/row",
            ConfigValueProblem::OutOfRange,
        ),
        (
            "a field of the kind missing",
            json!([{"row": "allow", "need": "need_a"}]),
            "/0/capability",
            ConfigValueProblem::WrongType,
        ),
        (
            "a member the kind does not have",
            json!([{"row": "none", "need": "need_a", "capability": "cap_x"}]),
            "/0/capability",
            ConfigValueProblem::WrongType,
        ),
        (
            "a field that is no text",
            json!([{"row": "parameter", "capability": "cap_x", "parameter": "param_p",
                    "value": 1}]),
            "/0/value",
            ConfigValueProblem::WrongType,
        ),
        (
            "a name outside the token charset",
            json!([allow("need_a", "cap_x"), allow("need_b", "Cap-y")]),
            "/1/capability",
            ConfigValueProblem::OutOfRange,
        ),
        (
            "none as a capability",
            json!([allow("need_a", "none")]),
            "/0/capability",
            ConfigValueProblem::OutOfRange,
        ),
        (
            "a row twice",
            json!([
                allow("need_a", "cap_x"),
                parameter_row("cap_x", "param_p", "opt_1"),
                allow("need_a", "cap_x")
            ]),
            "/2",
            ConfigValueProblem::SectionCheck,
        ),
        (
            "none with an allow of the same need",
            json!([allow("need_a", "cap_x"), decline("need_a")]),
            "/1",
            ConfigValueProblem::SectionCheck,
        ),
        (
            "more than 256 rows",
            Value::Array(too_many),
            "/256",
            ConfigValueProblem::OutOfRange,
        ),
    ];
    let own_key = instance_key(INSTANCE_A, REMEDY_CHOICES);
    let running = from_file(json!({}), json!({}));
    for (case, rows, pointer, problem) in cases {
        let at_start = [
            (
                from_file(json!({"remedy": {"choices": rows.clone()}}), json!({})),
                REMEDY_CHOICES,
            ),
            (
                from_file(
                    json!({}),
                    json!({"scheduling": {"remedy": {"choices": rows.clone()}}}),
                ),
                own_key.as_str(),
            ),
        ]
        .into_iter()
        .map(|(config, key)| (config.check(&context()).expect_err(case), key));
        let by_instruction = [REMEDY_CHOICES, own_key.as_str()].into_iter().map(|key| {
            (
                running
                    .apply(&[set(key, rows.clone())], &stamp(), &context())
                    .expect_err(case),
                key,
            )
        });
        for (refused, key) in at_start.chain(by_instruction) {
            assert_eq!(
                refused.failure_code(),
                ConfigCode::ConfigValueInvalid,
                "{case}: {key}"
            );
            assert_eq!(refused.config_key(), key, "{case}");
            assert_eq!(refused.problem(), Some(problem), "{case}: {key}");
            assert_eq!(refused.pointer(), Some(pointer), "{case}: {key}");
        }
    }

    // No conflict: two different parameters of one capability are taken, in either list, at
    // start and through an instruction.
    let rows = json!([
        parameter_row("cap_x", "param_p", "opt_1"),
        parameter_row("cap_x", "param_q", "opt_2")
    ]);
    let taken = ["param_p", "param_q"]
        .into_iter()
        .zip(["opt_1", "opt_2"])
        .map(|(parameter, value)| RemedyChoiceRow::Parameter {
            capability: "cap_x".to_owned(),
            parameter: parameter.to_owned(),
            value: value.to_owned(),
        })
        .collect::<Vec<_>>();
    let at_start = [
        from_file(json!({"remedy": {"choices": rows.clone()}}), json!({})),
        from_file(
            json!({}),
            json!({"scheduling": {"remedy": {"choices": rows.clone()}}}),
        ),
    ];
    let by_instruction = [REMEDY_CHOICES, own_key.as_str()].map(|key| {
        running
            .apply(&[set(key, rows.clone())], &stamp(), &context())
            .unwrap_or_else(|failure| panic!("{key}: {failure:?}"))
            .config
    });
    for config in at_start.into_iter().chain(by_instruction) {
        config
            .check(&context())
            .expect("two parameters of one capability");
        assert_eq!(
            config.remedy(INSTANCE_A).map(|remedy| remedy.rows),
            Ok(taken.clone())
        );
    }
}

// Requirement 1: `max_per_round` is `1..=8`; a value outside is `config_value_invalid`
// (`out_of_range`, with the range) as the file layer is rendered and through an instruction; the
// two bounds themselves are taken.
#[test]
fn b15c_max_per_round_out_of_bounds_is_refused() {
    let running = from_file(json!({}), json!({}));
    for value in [0, 9] {
        let at_start = render(json!({"remedy": {"max_per_round": value}}), json!({}))
            .expect_err("a request bound outside 1..=8 in the file");
        let by_instruction = running
            .apply(
                &[set(REMEDY_MAX_PER_ROUND, json!(value))],
                &stamp(),
                &context(),
            )
            .expect_err("a request bound outside 1..=8 by an instruction");
        for refused in [at_start, by_instruction] {
            assert_eq!(
                refused.failure_code(),
                ConfigCode::ConfigValueInvalid,
                "{value}"
            );
            assert_eq!(refused.config_key(), REMEDY_MAX_PER_ROUND, "{value}");
            assert_eq!(
                refused.problem(),
                Some(ConfigValueProblem::OutOfRange),
                "{value}"
            );
            let outcome = refused.outcome();
            assert_eq!(outcome.value("range_min"), Some(&json!(1)), "{value}");
            assert_eq!(outcome.value("range_max"), Some(&json!(8)), "{value}");
        }
    }
    let highest = from_file(json!({"remedy": {"max_per_round": 8}}), json!({}));
    highest.check(&context()).expect("eight requests a round");
    assert_eq!(
        highest
            .remedy(INSTANCE_A)
            .map(|remedy| remedy.max_per_round),
        Ok(8)
    );
    let lowest = running
        .apply(&[set(REMEDY_MAX_PER_ROUND, json!(1))], &stamp(), &context())
        .expect("one request a round")
        .config;
    assert_eq!(
        lowest.remedy(INSTANCE_A).map(|remedy| remedy.max_per_round),
        Ok(1)
    );
}
