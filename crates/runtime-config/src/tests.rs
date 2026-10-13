// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #387 D1b, test rows D1-M1 and D1-M2 (frozen model v1.2 §8.2). Every expected value
//! comes from the model: §5.2 (defaults file), §5.3 (merge), §5.5 (key table columns).

use super::*;
use actingcommand_contract::{ConfigParameter, ConfigParameterSource, FactScalar};
use serde_json::{Value, json};
use std::collections::BTreeSet;

const INSTANCE_A: &str = "instance_0123456789abcdef0123456789abcdef";
const INSTANCE_B: &str = "instance_fedcba9876543210fedcba9876543210";

/// Every member path of `value` that is not a key of `keys` and holds no such key below it.
fn members_outside(
    value: &Value,
    prefix: &str,
    keys: &BTreeSet<String>,
    outside: &mut Vec<String>,
) {
    let Value::Object(members) = value else {
        return;
    };
    for (name, member) in members {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        if keys.contains(&path) {
            continue;
        }
        let below = format!("{path}.");
        if member.is_object() && keys.iter().any(|key| key.starts_with(&below)) {
            members_outside(member, &path, keys, outside);
        } else {
            outside.push(path);
        }
    }
}

#[test]
fn d1_m1_the_defaults_file_parses_and_holds_exactly_the_keys_whose_default_is_the_file() {
    let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults file parses");
    assert!(defaults.is_object(), "the defaults file is one JSON object");
    let file_keys = rows()
        .filter(|row| row.default == DefaultSource::File)
        .map(|row| match row.name {
            KeyName::Exact(name) => name.to_owned(),
            KeyName::PerInstance(member) => {
                panic!("the per-instance member {member} cannot have a defaults-file entry")
            }
        })
        .collect::<BTreeSet<_>>();
    // Each `default = file` key has its entry.
    for key in &file_keys {
        assert!(
            member(&defaults, key).is_some_and(|value| !value.is_null()),
            "{key} has no entry in the defaults file"
        );
    }
    // No `none`, `section:` or `code` key has one.
    for row in rows().filter(|row| row.default != DefaultSource::File) {
        if let KeyName::Exact(name) = row.name {
            assert!(
                member(&defaults, name).is_none(),
                "{name} (default {:?}) has an entry in the defaults file",
                row.default
            );
        }
    }
    // Nothing outside the key table.
    let mut outside = Vec::new();
    members_outside(&defaults, "", &file_keys, &mut outside);
    assert!(
        outside.is_empty(),
        "defaults-file members outside the key table: {outside:?}"
    );
    // A member without parsing code is refused, as the configuration file refuses one (§5.2).
    assert_eq!(
        DefaultsFile::parse(r#"{"undeclared_section": {"value": 1}}"#).err(),
        Some(DefaultsInvalid)
    );
}

/// A synthetic list key with a defaults-file default, for the whole-list replacement (§5.3).
static LIST_ROWS: &[KeyRow] = &[KeyRow {
    name: KeyName::Exact("neutral_section.items"),
    file_path: FilePath::Key,
    value_type: ValueType::List,
    range: KeyRange::Any,
    default: DefaultSource::File,
    settable: Settable::Yes,
    reader: "D1b",
    since: "0.12.0",
}];

fn parameter<'a>(parameters: &'a [ConfigParameter], key: &str) -> &'a ConfigParameter {
    parameters
        .iter()
        .find(|parameter| parameter.key == key)
        .unwrap_or_else(|| panic!("no parameter {key}: {parameters:?}"))
}

#[test]
fn d1_m2_a_file_naming_one_key_makes_it_explicit_and_leaves_the_rest_default() {
    let instances = [INSTANCE_A.to_owned(), INSTANCE_B.to_owned()];
    // The file names one key: instance A's `stuck_recovery`, in its `instances[]` entry.
    let file = json!({
        "instances": [
            {"instance_id": INSTANCE_B, "alias": "node.b"},
            {"instance_id": INSTANCE_A, "alias": "node.a", "stuck_recovery": false},
        ],
    });
    // Workflow #387 D1c: a per-instance key the file does not name takes the `recovery_ladder`
    // section's value in effect (§5.5 `section:`), here the defaults file's.
    let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults file parses");
    let parameters = render_parameters(&mechanism_rows(), &instances, &defaults, &file, &|_| None)
        .expect("the sections resolve");
    let per_instance = parameters
        .iter()
        .filter(|parameter| {
            find_row(&parameter.key).is_some_and(|(_, instance)| instance.is_some())
        })
        .count();
    assert_eq!(per_instance, 4, "{parameters:?}");
    let named = parameter(&parameters, &instance_key(INSTANCE_A, STUCK_RECOVERY));
    assert_eq!(named.value, FactScalar::Boolean(false));
    assert_eq!(named.source, ConfigParameterSource::Explicit);
    assert!(named.settable);
    let section_enabled = parameter(&parameters, recovery_ladder::ENABLED)
        .value
        .clone();
    let section_cooldown = parameter(&parameters, recovery_ladder::COOLDOWN_SECS)
        .value
        .clone();
    for (instance, member, value) in [
        (
            INSTANCE_A,
            STUCK_RECOVERY_COOLDOWN_SECS,
            section_cooldown.clone(),
        ),
        (INSTANCE_B, STUCK_RECOVERY, section_enabled),
        (INSTANCE_B, STUCK_RECOVERY_COOLDOWN_SECS, section_cooldown),
    ] {
        let unnamed = parameter(&parameters, &instance_key(instance, member));
        assert_eq!(unnamed.value, value, "{instance} {member}");
        assert_eq!(
            unnamed.source,
            ConfigParameterSource::Default,
            "{instance} {member}"
        );
    }

    // A list the file names replaces the default list whole; elements are never merged.
    let defaults = json!({"neutral_section": {"items": [1, 2, 3]}});
    let replaced = render_parameters(
        LIST_ROWS,
        &[],
        &defaults,
        &json!({"neutral_section": {"items": [9]}}),
        &|_| None,
    )
    .expect("the list key resolves");
    assert_eq!(
        replaced,
        vec![ConfigParameter {
            key: "neutral_section.items".to_owned(),
            value: FactScalar::String("[9]".to_owned()),
            source: ConfigParameterSource::Explicit,
            settable: true,
        }]
    );
    let defaulted = render_parameters(LIST_ROWS, &[], &defaults, &json!({}), &|_| None)
        .expect("the list key resolves");
    assert_eq!(defaulted[0].value, FactScalar::String("[1,2,3]".to_owned()));
    assert_eq!(defaulted[0].source, ConfigParameterSource::Default);

    // An older key is found at its file path: the manifest's `performance_monitor.*` is the
    // file's `performance.*`.
    let key = "performance_monitor.pressure_start_samples";
    let (row, instance) = find_row(key).expect("the key is in the key table");
    assert_eq!(instance, None);
    let older = json!({"performance": {"pressure_start_samples": 5}});
    assert_eq!(file_value(row, key, None, &older), Some(&json!(5)));
    assert_eq!(
        file_value(
            row,
            key,
            None,
            &json!({"performance_monitor": {"pressure_start_samples": 5}})
        ),
        None
    );
    // A per-instance key is found in the `instances[]` entry with the same id, and only there.
    let key = instance_key(INSTANCE_A, STUCK_RECOVERY_COOLDOWN_SECS);
    let (row, instance) = find_row(&key).expect("the per-instance key is in the key table");
    assert_eq!(instance, Some(INSTANCE_A));
    let file = json!({
        "instances": [
            {"instance_id": INSTANCE_B, "stuck_recovery_cooldown_secs": 30},
            {"instance_id": INSTANCE_A, "stuck_recovery_cooldown_secs": 90},
        ],
    });
    assert_eq!(file_value(row, &key, instance, &file), Some(&json!(90)));
}
