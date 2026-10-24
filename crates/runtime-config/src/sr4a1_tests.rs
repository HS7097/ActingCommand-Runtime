// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #386 SR-4a1 (SCH-M frozen model v1.0 §3.2 D-1, D-4, D-6; §6 row SR-4a1): the blackout
// lists and the keys of gradual quiet and the day boundary. Every expected value is the model's:
// - lists: the host list `scheduling.blackout.host` and each instance's own list
//   `instance.<id>.scheduling.blackout` (in the file the entry's `scheduling.blackout`), default
//   empty; entries `weekly` (the host shape, `kind` absent or `weekly`) and `once`
//   (`{kind: "once", id, from_unix_ms, until_unix_ms}`, half-open and non-empty); at most 64 per
//   list; ids unique in their list;
// - a broken entry is `blackout_config_invalid` with `field`, `blackout_id` and
//   `blackout_source` (`host` | `instance`); through an instruction `config_value_invalid`
//   (`section_check`);
// - `scheduling.blackout.quiet_horizon` (`deadline` | `expected`, default `deadline`) and
//   `scheduling.day_boundary.quiet` (default `true`, per instance defaulting to the section's),
//   settable;
// - no game word anywhere: the shipped defaults name no blackout entry at all (R5, R6.1).

use super::*;
use crate::scheduling::{
    BLACKOUT_HOST, BLACKOUT_QUIET_HORIZON, BlackoutEntry, BlackoutWindow, DAY_BOUNDARY_QUIET,
    INSTANCE_BLACKOUT, QuietHorizon,
};
use actingcommand_contract::outcome::BlackoutSource;
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

fn context() -> CheckContext {
    CheckContext::new(5_000, DEVICE)
}

fn shipped() -> Value {
    DefaultsFile::shipped()
        .expect("the shipped defaults file parses")
        .1
}

// The rows a start renders from a file of `scheduling` and, for instance A, its `scheduling`
// member.
fn render(scheduling: Value, own: Value) -> Result<Vec<ConfigParameter>, ConfigFailure> {
    let mut entry = json!({"instance_id": INSTANCE_A, "alias": "node.a"});
    if !own.is_null() {
        entry["scheduling"] = own;
    }
    render_parameters(
        &mechanism_rows(),
        &[INSTANCE_A.to_owned()],
        &shipped(),
        &json!({"scheduling": scheduling, "instances": [entry]}),
        &|_| None,
    )
}

fn from_file(scheduling: Value, own: Value) -> EffectiveConfig {
    EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters: render(scheduling, own).expect("every key resolves"),
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

fn weekly_entry(id: &str, weekday: u64) -> Value {
    json!({"id": id, "weekdays": [weekday], "utc_offset_minutes": 540,
           "start_minute_of_day": 280, "end_minute_of_day": 340})
}

fn once_entry(id: &str, from: u64, until: u64) -> Value {
    json!({"kind": "once", "id": id, "from_unix_ms": from, "until_unix_ms": until})
}

// R5, R6.1 and D-4, D-6: the shipped defaults name no blackout entry (no instance and no game is
// blacked out unless its configuration says so), gradual quiet keeps the hard deadline, and the
// day boundary is on for every instance, which takes the section's value.
#[test]
fn sr4a1_c1_no_blackout_entry_by_default_and_the_new_keys_have_their_model_defaults() {
    let parameters = render(json!({}), Value::Null).expect("the defaults resolve");
    let reported = |key: &str| parameters.iter().find(|parameter| parameter.key == key);
    let instance_blackout = instance_key(INSTANCE_A, INSTANCE_BLACKOUT);
    assert_eq!(
        reported(BLACKOUT_HOST).map(|parameter| parameter.value.clone()),
        Some(FactScalar::String("[]".to_owned()))
    );
    assert!(
        reported(&instance_blackout).is_none(),
        "an instance whose entry names no list has none in effect"
    );
    for (key, value) in [
        (
            BLACKOUT_QUIET_HORIZON.to_owned(),
            FactScalar::String("deadline".to_owned()),
        ),
        (DAY_BOUNDARY_QUIET.to_owned(), FactScalar::Boolean(true)),
        (
            instance_key(INSTANCE_A, DAY_BOUNDARY_QUIET),
            FactScalar::Boolean(true),
        ),
    ] {
        let parameter = reported(&key).unwrap_or_else(|| panic!("no parameter {key}"));
        assert_eq!(parameter.value, value, "{key}");
        assert_eq!(parameter.source, ConfigParameterSource::Default, "{key}");
        assert!(parameter.settable, "{key}");
    }

    let config = from_file(json!({}), Value::Null);
    config
        .check(&context())
        .expect("the defaults pass the check");
    let section = config.scheduling().expect("the section in effect");
    assert!(section.blackout.host.is_empty());
    assert_eq!(section.blackout.quiet_horizon, QuietHorizon::Deadline);
    assert!(section.day_boundary_quiet);
    assert_eq!(config.instance_blackouts(INSTANCE_A), Ok(Vec::new()));
    assert_eq!(config.day_boundary_quiet(INSTANCE_A), Ok(true));
}

// One refused file: where the list is, the expected pointer, entry id and list.
struct Refusal {
    case: &'static str,
    scheduling: Value,
    own: Value,
    config_key: String,
    field: String,
    blackout_id: Option<&'static str>,
    source: BlackoutSource,
}

fn refusals() -> Vec<Refusal> {
    let host = |case, list: Value, member: &str, blackout_id| Refusal {
        case,
        scheduling: json!({"blackout": {"host": list}}),
        own: Value::Null,
        config_key: BLACKOUT_HOST.to_owned(),
        field: format!("/scheduling/blackout/host/{member}"),
        blackout_id,
        source: BlackoutSource::Host,
    };
    let instance = |case, list: Value, member: &str, blackout_id| Refusal {
        case,
        scheduling: json!({}),
        own: json!({"blackout": list}),
        config_key: instance_key(INSTANCE_A, INSTANCE_BLACKOUT),
        field: format!("/instances/0/scheduling/blackout/{member}"),
        blackout_id,
        source: BlackoutSource::Instance,
    };
    let sixty_five = (0..65)
        .map(|index| once_entry(&format!("entry-{index}"), 1_000, 2_000))
        .collect::<Vec<_>>();
    vec![
        host(
            "a once entry that ends where it starts",
            json!([once_entry("gap", 5_000, 5_000)]),
            "0/until_unix_ms",
            Some("gap"),
        ),
        host(
            "a once entry that ends before it starts",
            json!([once_entry("gap", 5_000, 4_000)]),
            "0/until_unix_ms",
            Some("gap"),
        ),
        host(
            "a once entry without its end",
            json!([{"kind": "once", "id": "open", "from_unix_ms": 1_000}]),
            "0",
            Some("open"),
        ),
        host(
            "a once entry with a negative start",
            json!([{"kind": "once", "id": "early", "from_unix_ms": -1, "until_unix_ms": 2_000}]),
            "0/from_unix_ms",
            Some("early"),
        ),
        host(
            "an entry of an unknown kind",
            json!([{"kind": "monthly", "id": "odd", "weekdays": [1], "utc_offset_minutes": 0,
                    "start_minute_of_day": 0, "end_minute_of_day": 60}]),
            "0/kind",
            Some("odd"),
        ),
        host(
            "a weekly entry with a member of the once kind",
            json!([{"kind": "weekly", "id": "mixed", "weekdays": [1], "utc_offset_minutes": 0,
                    "start_minute_of_day": 0, "end_minute_of_day": 60, "until_unix_ms": 5}]),
            "0",
            Some("mixed"),
        ),
        host(
            "a list of 65 entries",
            Value::Array(sixty_five),
            "64",
            Some("entry-64"),
        ),
        instance(
            "an instance's weekly entry with a weekday outside 1 to 7",
            json!([weekly_entry("night", 8)]),
            "0/weekdays",
            Some("night"),
        ),
        instance(
            "two entries of one instance with one id",
            json!([weekly_entry("night", 1), once_entry("night", 1_000, 2_000)]),
            "1/id",
            Some("night"),
        ),
        instance(
            "an instance's once entry that ends where it starts",
            json!([once_entry("gap", 7_000, 7_000)]),
            "0/until_unix_ms",
            Some("gap"),
        ),
    ]
}

// D-1: lists of weekly entries (with and without `kind`) and once entries are read as written,
// the host's and an instance's own; an entry that breaks a rule refuses a start and check-config
// with `blackout_config_invalid`, naming the member, the entry and the list it is in
// (`blackout_source`); through an instruction the same entries are `config_value_invalid`
// (`section_check`) and the configuration in effect stays.
#[test]
fn sr4a1_c2_bad_blackout_entries_are_refused_with_their_source() {
    // The entries the model allows, in both lists.
    let config = from_file(
        json!({"blackout": {"host": [
            weekly_entry("night", 1),
            {"kind": "weekly", "id": "early", "weekdays": [4], "utc_offset_minutes": 0,
             "start_minute_of_day": 1_380, "end_minute_of_day": 1_380},
            once_entry("deploy", 1_000, 2_000),
        ]}}),
        json!({"blackout": [once_entry("absence", 3_000, 4_000)]}),
    );
    config.check(&context()).expect("allowed entries pass");
    assert_eq!(
        config.scheduling().map(|section| section.blackout.host),
        Ok(vec![
            BlackoutEntry {
                id: "night".to_owned(),
                window: BlackoutWindow::Weekly {
                    weekdays: vec![1],
                    utc_offset_minutes: 540,
                    start_minute_of_day: 280,
                    end_minute_of_day: 340,
                },
            },
            BlackoutEntry {
                id: "early".to_owned(),
                window: BlackoutWindow::Weekly {
                    weekdays: vec![4],
                    utc_offset_minutes: 0,
                    start_minute_of_day: 1_380,
                    end_minute_of_day: 1_380,
                },
            },
            BlackoutEntry {
                id: "deploy".to_owned(),
                window: BlackoutWindow::Once {
                    from_unix_ms: 1_000,
                    until_unix_ms: 2_000,
                },
            },
        ])
    );
    assert_eq!(
        config.instance_blackouts(INSTANCE_A),
        Ok(vec![BlackoutEntry {
            id: "absence".to_owned(),
            window: BlackoutWindow::Once {
                from_unix_ms: 3_000,
                until_unix_ms: 4_000,
            },
        }])
    );
    // An id is unique within its list only: the host and the instance may share one.
    from_file(
        json!({"blackout": {"host": [once_entry("same", 1_000, 2_000)]}}),
        json!({"blackout": [once_entry("same", 1_000, 2_000)]}),
    )
    .check(&context())
    .expect("one id in two lists");

    for row in refusals() {
        let refused = match render(row.scheduling.clone(), row.own.clone()) {
            Err(refused) => refused,
            Ok(parameters) => EffectiveConfig::from_manifest(&RuntimeConfigManifest {
                subsystems: Vec::new(),
                parameters,
            })
            .expect("every key is in the key table")
            .check(&context())
            .expect_err(row.case),
        };
        assert_eq!(
            refused.failure_code(),
            ConfigCode::BlackoutConfigInvalid,
            "{}",
            row.case
        );
        assert_eq!(refused.config_key(), row.config_key, "{}", row.case);
        assert_eq!(refused.field(), Some(row.field.as_str()), "{}", row.case);
        assert_eq!(refused.blackout_id(), row.blackout_id, "{}", row.case);
        assert_eq!(refused.blackout_source(), Some(row.source), "{}", row.case);
        let outcome = refused.outcome();
        assert_eq!(
            outcome.value("blackout_source"),
            Some(&json!(row.source.as_str())),
            "{}",
            row.case
        );
        assert_eq!(
            outcome.value("field"),
            Some(&json!(row.field)),
            "{}",
            row.case
        );
    }

    // Through an instruction: a broken host entry and a broken instance entry.
    let running = from_file(json!({}), Value::Null);
    let instance_list = instance_key(INSTANCE_A, INSTANCE_BLACKOUT);
    let named = from_file(json!({}), json!({"blackout": []}));
    for (config, key) in [(&running, BLACKOUT_HOST), (&named, instance_list.as_str())] {
        let refused = config
            .apply(
                &[ConfigSettingChange {
                    key: key.to_owned(),
                    value: Some(json!([once_entry("gap", 9_000, 9_000)])),
                }],
                &stamp(),
                &context(),
            )
            .expect_err(key);
        assert_eq!(
            refused.failure_code(),
            ConfigCode::ConfigValueInvalid,
            "{key}"
        );
        assert_eq!(refused.config_key(), key, "{key}");
        assert_eq!(
            refused.problem(),
            Some(ConfigValueProblem::SectionCheck),
            "{key}"
        );
        assert_eq!(refused.field(), None, "{key}");
        assert_eq!(refused.blackout_source(), None, "{key}");
    }
    assert_eq!(named.instance_blackouts(INSTANCE_A), Ok(Vec::new()));
}
