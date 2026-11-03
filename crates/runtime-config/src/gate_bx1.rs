// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 BX-1 (register §1.273, since 0.12.39): a blackout entry's parser and shape as a
// client surface. The console edits an instance's blackout entries through `ConfigureSettings`
// and keeps no copy of their shape, so what the work order requires is:
// - the client's entries serialize to the JSON the key takes, byte-stable (the same entries are
//   the same bytes, the canonical text the Runtime keeps, and parsing them gives them back);
// - the client's parse refuses as the configuration check does: the same code, key, list and
//   entry id, its `field` pointing into the value the client passed where `check-config` points
//   into the file; a value that is not a list as `ConfigureSettings` refuses it;
// - the Runtime reads what the client writes, through `ConfigureSettings` and from the file.

use super::*;
use crate::scheduling::{BLACKOUT_HOST, INSTANCE_BLACKOUT};
use actingcommand_contract::outcome::BlackoutSource;
use actingcommand_contract::{
    ConfigParameter, ConfigSettingChange, FactScalar, RuntimeConfigManifest,
};
use serde_json::{Value, json};

const INSTANCE_A: &str = "instance_0123456789abcdef0123456789abcdef";
const DEVICE: DeviceBounds = DeviceBounds {
    application_stop_ms: 60_000,
    emulator_stop_ms: 200_000,
    emulator_start_ms: 190_000,
};

/// The text of [`entries`], as the key takes it.
const WEEKLY_TEXT: &str = concat!(
    r#"{"end_minute_of_day":60,"id":"night","kind":"weekly","start_minute_of_day":1380,"#,
    r#""utc_offset_minutes":-300,"weekdays":[5,1]}"#,
);
const ONCE_TEXT: &str =
    r#"{"from_unix_ms":1700000000000,"id":"deploy","kind":"once","until_unix_ms":1700000360000}"#;

fn context() -> CheckContext {
    CheckContext::new(5_000, DEVICE)
}

fn stamp() -> InstructionStamp {
    InstructionStamp {
        set_unix_ms: 1,
        set_in_owner_epoch: "epoch".to_owned(),
        request_id: "request".to_owned(),
        channel: None,
    }
}

// The rows a start renders from a file of `scheduling` and, for instance A, its `scheduling`
// member.
fn render(scheduling: Value, own: Value) -> Result<Vec<ConfigParameter>, ConfigFailure> {
    let mut entry = json!({"instance_id": INSTANCE_A, "alias": "node.a"});
    if !own.is_null() {
        entry["scheduling"] = own;
    }
    let shipped = DefaultsFile::shipped()
        .expect("the shipped defaults file parses")
        .1;
    render_parameters(
        &mechanism_rows(),
        &[INSTANCE_A.to_owned()],
        &shipped,
        &json!({"scheduling": scheduling, "instances": [entry]}),
        &|_| None,
    )
}

fn from_parameters(parameters: Vec<ConfigParameter>) -> EffectiveConfig {
    EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    })
    .expect("every key is in the key table")
}

fn from_file(scheduling: Value, own: Value) -> EffectiveConfig {
    from_parameters(render(scheduling, own).expect("every key resolves"))
}

/// The configuration file with `list` as `list_key`'s list.
fn file_with(list_key: BlackoutListKey<'_>, list: &Value) -> (Value, Value) {
    match list_key {
        BlackoutListKey::Host => (json!({"blackout": {"host": list}}), Value::Null),
        BlackoutListKey::Instance(_) => (json!({}), json!({"blackout": list})),
    }
}

/// The JSON pointer of `list_key`'s list in that file.
fn file_pointer(list_key: BlackoutListKey<'_>) -> &'static str {
    match list_key {
        BlackoutListKey::Host => "/scheduling/blackout/host",
        BlackoutListKey::Instance(_) => "/instances/0/scheduling/blackout",
    }
}

const LISTS: [BlackoutListKey<'static>; 2] =
    [BlackoutListKey::Host, BlackoutListKey::Instance(INSTANCE_A)];

/// A weekly entry that crosses midnight on two weekdays behind UTC, and a once entry.
fn entries() -> Vec<BlackoutEntry> {
    vec![
        BlackoutEntry {
            id: "night".to_owned(),
            window: BlackoutWindow::Weekly {
                weekdays: vec![5, 1],
                utc_offset_minutes: -300,
                start_minute_of_day: 1_380,
                end_minute_of_day: 60,
            },
        },
        BlackoutEntry {
            id: "deploy".to_owned(),
            window: BlackoutWindow::Once {
                from_unix_ms: 1_700_000_000_000,
                until_unix_ms: 1_700_000_360_000,
            },
        },
    ]
}

fn weekly_entry(id: &str, weekdays: Value) -> Value {
    json!({"id": id, "weekdays": weekdays, "utc_offset_minutes": 540,
           "start_minute_of_day": 280, "end_minute_of_day": 340})
}

fn once_entry(id: &str, from: u64, until: u64) -> Value {
    json!({"kind": "once", "id": id, "from_unix_ms": from, "until_unix_ms": until})
}

// The entries serialize to the JSON the key takes, members in name order and `kind` written, so
// the value's text is its canonical JSON; that text parses back to the same entries, for either
// list, and serializes to the same bytes again. An entry written without `kind`, or with its
// members in another order, reads as the same entry and is written the same way.
#[test]
fn bx1_round_trip_is_byte_stable() {
    let entries = entries();
    let list_text = format!("[{WEEKLY_TEXT},{ONCE_TEXT}]");
    let value = BlackoutEntry::list_json(&entries);
    assert_eq!(value.to_string(), list_text);
    assert_eq!(canonical_json(&value), list_text);

    let read: Value = serde_json::from_str(&list_text).expect("the text is JSON");
    for list in LISTS {
        let parsed = BlackoutEntry::parse_list(list, &read).expect("the client's own list");
        assert_eq!(parsed, entries, "{list:?}");
        assert_eq!(
            BlackoutEntry::list_json(&parsed).to_string(),
            list_text,
            "{list:?}"
        );
    }

    for (entry, text) in entries.iter().zip([WEEKLY_TEXT, ONCE_TEXT]) {
        assert_eq!(entry.to_json().to_string(), text);
        let read: Value = serde_json::from_str(text).expect("the text is JSON");
        for list in LISTS {
            assert_eq!(
                BlackoutEntry::parse(list, &read).as_ref(),
                Ok(entry),
                "{list:?}"
            );
        }
    }

    // The host shape (no `kind`) and another member order: the same entry, the same bytes.
    for written in [
        json!({"weekdays": [5, 1], "utc_offset_minutes": -300, "start_minute_of_day": 1_380,
               "end_minute_of_day": 60, "id": "night"}),
        json!({"weekdays": [5, 1], "kind": "weekly", "id": "night", "end_minute_of_day": 60,
               "start_minute_of_day": 1_380, "utc_offset_minutes": -300}),
    ] {
        assert_ne!(written.to_string(), WEEKLY_TEXT, "another text");
        let entry = BlackoutEntry::parse(BlackoutListKey::Host, &written).expect("a weekly entry");
        assert_eq!(entry, entries[0], "{written}");
        assert_eq!(entry.to_json().to_string(), WEEKLY_TEXT, "{written}");
    }
}

// One broken list, refused the same way whichever list it is.
struct Refusal {
    case: &'static str,
    list: Value,
    /// The member's pointer into the value the client passes.
    field: &'static str,
    blackout_id: Option<&'static str>,
    /// Whether the broken rule is one entry's own (not the bound or the unique ids), so
    /// [`BlackoutEntry::parse`] of the first entry alone refuses it too.
    own_rule: bool,
}

fn refusals() -> Vec<Refusal> {
    let refusal = |case, list, field, blackout_id, own_rule| Refusal {
        case,
        list,
        field,
        blackout_id,
        own_rule,
    };
    let sixty_five = (0..65)
        .map(|index| once_entry(&format!("entry-{index}"), 1_000, 2_000))
        .collect::<Vec<_>>();
    vec![
        refusal(
            "a once entry that ends where it starts",
            json!([once_entry("gap", 5_000, 5_000)]),
            "/0/until_unix_ms",
            Some("gap"),
            true,
        ),
        refusal(
            "a once entry without its end",
            json!([{"kind": "once", "id": "open", "from_unix_ms": 1_000}]),
            "/0",
            Some("open"),
            true,
        ),
        refusal(
            "a weekly entry with a weekday outside 1 to 7",
            json!([weekly_entry("night", json!([8]))]),
            "/0/weekdays",
            Some("night"),
            true,
        ),
        refusal(
            "a weekly entry with one weekday twice",
            json!([weekly_entry("night", json!([2, 2]))]),
            "/0/weekdays",
            Some("night"),
            true,
        ),
        refusal(
            "a weekly entry whose clock is more than 14 hours from UTC",
            json!([{"id": "far", "weekdays": [1], "utc_offset_minutes": 841,
                    "start_minute_of_day": 0, "end_minute_of_day": 60}]),
            "/0/utc_offset_minutes",
            Some("far"),
            true,
        ),
        refusal(
            "a weekly entry that starts at minute 1440",
            json!([{"id": "late", "weekdays": [1], "utc_offset_minutes": 0,
                    "start_minute_of_day": 1_440, "end_minute_of_day": 60}]),
            "/0/start_minute_of_day",
            Some("late"),
            true,
        ),
        refusal(
            "an entry of an unknown kind",
            json!([{"kind": "monthly", "id": "odd", "weekdays": [1], "utc_offset_minutes": 0,
                    "start_minute_of_day": 0, "end_minute_of_day": 60}]),
            "/0/kind",
            Some("odd"),
            true,
        ),
        refusal(
            "a weekly entry with a member of the once kind",
            json!([{"kind": "weekly", "id": "mixed", "weekdays": [1], "utc_offset_minutes": 0,
                    "start_minute_of_day": 0, "end_minute_of_day": 60, "until_unix_ms": 5}]),
            "/0",
            Some("mixed"),
            true,
        ),
        refusal(
            "an entry whose id is empty",
            json!([once_entry("", 1_000, 2_000)]),
            "/0/id",
            None,
            true,
        ),
        refusal(
            "two entries with one id",
            json!([
                weekly_entry("night", json!([1])),
                once_entry("night", 1_000, 2_000)
            ]),
            "/1/id",
            Some("night"),
            false,
        ),
        refusal(
            "a list of 65 entries",
            Value::Array(sixty_five),
            "/64",
            Some("entry-64"),
            false,
        ),
    ]
}

// For the host list and an instance's own list, every broken entry the configuration check
// refuses the client's parse refuses with the same code (`blackout_config_invalid`), key, list
// (`blackout_source`) and entry id, its `field` the check's pointer without the list's place in
// the file; one entry alone is refused for its own rules with the pointer into that entry.
// Through `ConfigureSettings` the same list is `config_value_invalid` (`section_check`) for the
// key. A value that is not a list is refused as `ConfigureSettings` refuses it.
#[test]
fn bx1_parse_refuses_as_the_config_check_does() {
    let running = from_file(json!({}), Value::Null);
    for list in LISTS {
        let config_key = list.config_key();
        assert_eq!(
            config_key,
            match list {
                BlackoutListKey::Host => BLACKOUT_HOST.to_owned(),
                BlackoutListKey::Instance(id) => instance_key(id, INSTANCE_BLACKOUT),
            }
        );
        let source = match list {
            BlackoutListKey::Host => BlackoutSource::Host,
            BlackoutListKey::Instance(_) => BlackoutSource::Instance,
        };
        assert_eq!(list.source(), source);

        for row in refusals() {
            let case = format!("{}, {list:?}", row.case);
            let (scheduling, own) = file_with(list, &row.list);
            let checked = match render(scheduling, own) {
                Err(refused) => refused,
                Ok(parameters) => from_parameters(parameters)
                    .check(&context())
                    .expect_err(&case),
            };
            let client = BlackoutEntry::parse_list(list, &row.list).expect_err(&case);

            assert_eq!(
                client.failure_code(),
                ConfigCode::BlackoutConfigInvalid,
                "{case}"
            );
            assert_eq!(client.failure_code(), checked.failure_code(), "{case}");
            assert_eq!(client.config_key(), config_key, "{case}");
            assert_eq!(client.config_key(), checked.config_key(), "{case}");
            assert_eq!(client.blackout_source(), Some(source), "{case}");
            assert_eq!(
                client.blackout_source(),
                checked.blackout_source(),
                "{case}"
            );
            assert_eq!(client.blackout_id(), row.blackout_id, "{case}");
            assert_eq!(client.blackout_id(), checked.blackout_id(), "{case}");
            assert_eq!(client.field(), Some(row.field), "{case}");
            assert_eq!(
                checked.field(),
                Some(format!("{}{}", file_pointer(list), row.field).as_str()),
                "{case}"
            );
            let outcome = client.outcome();
            assert_eq!(outcome.value("field"), Some(&json!(row.field)), "{case}");
            assert_eq!(
                outcome.value("blackout_source"),
                Some(&json!(source.as_str())),
                "{case}"
            );

            if row.own_rule {
                let entry = &row.list[0];
                let alone = BlackoutEntry::parse(list, entry).expect_err(&case);
                assert_eq!(alone.failure_code(), client.failure_code(), "{case}");
                assert_eq!(alone.config_key(), client.config_key(), "{case}");
                assert_eq!(alone.blackout_source(), client.blackout_source(), "{case}");
                assert_eq!(alone.blackout_id(), client.blackout_id(), "{case}");
                let in_entry = row.field.strip_prefix("/0").expect("the first entry");
                assert_eq!(alone.field(), Some(in_entry), "{case}");
            }

            let instructed = running
                .apply(
                    &[ConfigSettingChange {
                        key: config_key.clone(),
                        value: Some(row.list.clone()),
                    }],
                    &stamp(),
                    &context(),
                )
                .expect_err(&case);
            assert_eq!(
                instructed.failure_code(),
                ConfigCode::ConfigValueInvalid,
                "{case}"
            );
            assert_eq!(instructed.config_key(), config_key, "{case}");
            assert_eq!(
                instructed.problem(),
                Some(ConfigValueProblem::SectionCheck),
                "{case}"
            );
        }

        // A value that is not a list: `wrong_type` for the key, from the client and from
        // `ConfigureSettings` alike.
        let not_a_list = once_entry("deploy", 1_000, 2_000);
        let client = BlackoutEntry::parse_list(list, &not_a_list).expect_err("an object");
        let instructed = running
            .apply(
                &[ConfigSettingChange {
                    key: config_key.clone(),
                    value: Some(not_a_list),
                }],
                &stamp(),
                &context(),
            )
            .expect_err("an object");
        for refused in [&client, &instructed] {
            assert_eq!(refused.failure_code(), ConfigCode::ConfigValueInvalid);
            assert_eq!(refused.config_key(), config_key);
            assert_eq!(refused.problem(), Some(ConfigValueProblem::WrongType));
        }
    }
}

// What the client writes the Runtime reads as the same entries: through `ConfigureSettings`, for
// the host list and an instance's own list (one whose file entry names none), the change applies,
// the Runtime keeps the client's text byte for byte, reads the client's entries, and the client
// reads them back from the value in effect; and the same value written in the configuration file
// passes the check at start and reads as the same entries.
#[test]
fn bx1_the_runtime_reads_what_the_client_writes() {
    let entries = entries();
    let value = BlackoutEntry::list_json(&entries);
    let running = from_file(json!({}), Value::Null);
    for list in LISTS {
        let config_key = list.config_key();
        let applied = running
            .apply(
                &[ConfigSettingChange {
                    key: config_key.clone(),
                    value: Some(value.clone()),
                }],
                &stamp(),
                &context(),
            )
            .expect(&config_key);
        let read = match list {
            BlackoutListKey::Host => applied
                .config
                .scheduling()
                .map(|section| section.blackout.host),
            BlackoutListKey::Instance(id) => applied.config.instance_blackouts(id),
        };
        assert_eq!(read.as_ref(), Ok(&entries), "{list:?}");
        let held = applied
            .config
            .value(&config_key)
            .expect("the list in effect");
        assert_eq!(
            held,
            &FactScalar::String(canonical_json(&value)),
            "{list:?}"
        );
        let held = scalar_json(ValueType::List, held).expect("a list");
        assert_eq!(
            BlackoutEntry::parse_list(list, &held).as_ref(),
            Ok(&entries),
            "{list:?}"
        );
    }

    let file = from_file(
        json!({"blackout": {"host": value.clone()}}),
        json!({"blackout": value}),
    );
    file.check(&context())
        .expect("the client's list passes at start");
    assert_eq!(
        file.scheduling()
            .map(|section| section.blackout.host)
            .as_ref(),
        Ok(&entries)
    );
    assert_eq!(file.instance_blackouts(INSTANCE_A), Ok(entries));
}
