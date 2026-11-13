// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #453 S2 (register §1.357; contracts/runtime-config.md, "An entry that starts paused";
// contracts/scheduling-pause.md, "Pauses held at a first start"): an `instances[]` entry's optional
// boolean `start_paused`. The assembly takes a boolean, reports the row
// `instance.<id>.start_paused` (explicit, not settable) only for an entry that names it, and hands
// the host the instance ids of the entries that declare it `true`; a non-boolean is refused as the
// entry's other members are; the key table lists the key. check-config reports the row and, for a
// configuration with such an entry whose own `ledger_observer.tag.workbench_patterns` leaves out
// `host.start_pause_applied`, the note `ledger_tag_key_unclassified`. The ids, names and paths are
// made up; the entry is a fixture (no device).

use super::*;
use actingcommand_contract::{ConfigParameterSource, FactScalar, IdentifierIssuer};
use serde_json::{Value, json};
use std::ffi::OsString;
use std::fs;
use tempfile::TempDir;

const ALIAS: &str = "neutral.a";
/// The tagged runtime fact a start loads to know which entries were held already.
const APPLIED: &str = "host.start_pause_applied";

fn instance_id() -> InstanceId {
    *IdentifierIssuer::new()
        .expect("issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport()
}

/// One fixture entry of `id`, naming `start_paused` as `start_paused` gives it (`None`: the entry
/// does not name it).
fn entry(id: InstanceId, start_paused: Option<Value>) -> Value {
    let mut entry = json!({
        "alias": ALIAS,
        "instance_id": id,
        "fixture_backend": {
            "frames": [{"width": 1, "height": 1, "rgb": [1, 2, 3]}],
            "max_inputs": 1
        }
    });
    if let Some(value) = start_paused {
        entry["start_paused"] = value;
    }
    entry
}

/// The configuration file's JSON around `entry`, with `extra` members at the top level.
fn file(root: &TempDir, entry: Value, extra: &[(&str, Value)]) -> Value {
    let mut config = json!({
        "schema_version": CONFIG_SCHEMA_VERSION,
        "state_root": root.path().join("state"),
        "bind_host": "127.0.0.1",
        "bind_port": 0,
        "secret_fingerprint_salt": "0123456789abcdef",
        "instances": [entry],
    });
    for (member, value) in extra {
        config[*member] = value.clone();
    }
    config
}

/// The assembly of the configuration around `entry`, which must pass.
fn assembled(root: &TempDir, entry: Value) -> RuntimeAssembly {
    serde_json::from_value::<ActingdConfigFile>(file(root, entry, &[]))
        .expect("typed config")
        .assemble()
        .unwrap_or_else(|code| panic!("assembles: {code:?}"))
}

/// `instance.<id>.start_paused`.
fn key(id: InstanceId) -> String {
    format!(
        "{}.{}.{}",
        actingcommand_runtime_config::INSTANCE_SECTION,
        manifest::instance_id_text(id).expect("the id's text"),
        actingcommand_runtime_config::START_PAUSED
    )
}

/// check-config of the configuration written to `path`, which must pass: its report and its
/// notes, in order.
fn check(path: &std::path::Path) -> (Value, Vec<Outcome>) {
    let reported = crate::check_config::run(vec![
        OsString::from("check-config"),
        OsString::from("--config"),
        path.as_os_str().to_owned(),
    ])
    .unwrap_or_else(|error| panic!("check-config passes: {}", error.outcome()));
    let report = reported.data.get("report").expect("the report").clone();
    let notes = reported
        .outcome
        .causes()
        .iter()
        .filter(|link| link.relation().as_str() == "note")
        .map(|link| link.outcome().clone())
        .collect();
    (report, notes)
}

/// The notes of `ledger_tag_key_unclassified`, as their values.
fn tag_notes(notes: &[Outcome]) -> Vec<Value> {
    notes
        .iter()
        .filter(|note| note.code().as_str() == "ledger_tag_key_unclassified")
        .map(|note| Value::Object(note.values().clone()))
        .collect()
}

/// Whether check-config's manifest lists `key`.
fn manifest_lists(report: &Value, key: &str) -> bool {
    report["config_manifest"]["parameters"]
        .as_array()
        .expect("the manifest's parameters")
        .iter()
        .any(|parameter| parameter["key"] == key)
}

// A boolean is taken. `true` is reported as the row instance.<id>.start_paused, explicit and not
// settable, and the host gets the entry's instance id; `false` is reported too and hands the host
// nothing; an entry without the member has no row, hands the host nothing, and its manifest is
// the one with the member less that row. The key table lists the key: boolean, no default, read at
// assembly.
#[test]
fn s2_config_a_boolean_start_paused_is_reported_and_only_true_reaches_the_host() {
    let root = TempDir::new().expect("tempdir");
    let id = instance_id();
    let key = key(id);
    let paused = assembled(&root, entry(id, Some(json!(true))));
    let unpaused = assembled(&root, entry(id, Some(json!(false))));
    let without = assembled(&root, entry(id, None));
    for (assembly, value) in [(&paused, true), (&unpaused, false)] {
        let row = assembly
            .manifest
            .parameters
            .iter()
            .find(|parameter| parameter.key == key)
            .expect("the entry's start_paused row");
        assert_eq!(row.value, FactScalar::Boolean(value));
        assert_eq!(row.source, ConfigParameterSource::Explicit);
        assert!(!row.settable);
    }
    assert_eq!(paused.host.start_paused(), &BTreeSet::from([id]));
    assert!(unpaused.host.start_paused().is_empty());
    assert!(without.host.start_paused().is_empty());
    assert!(
        !without
            .manifest
            .parameters
            .iter()
            .any(|parameter| parameter.key == key),
        "an entry without the member has no row"
    );
    let others = paused
        .manifest
        .parameters
        .iter()
        .filter(|parameter| parameter.key != key)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(others, without.manifest.parameters);
    assert_eq!(paused.manifest.subsystems, without.manifest.subsystems);

    let (row, _) = actingcommand_runtime_config::find_row(&key).expect("the key table row");
    assert_eq!(
        row.name,
        actingcommand_runtime_config::KeyName::PerInstance(
            actingcommand_runtime_config::START_PAUSED
        )
    );
    assert_eq!(
        row.value_type,
        actingcommand_runtime_config::ValueType::Boolean
    );
    assert_eq!(
        row.default,
        actingcommand_runtime_config::DefaultSource::None
    );
    assert_eq!(
        row.settable,
        actingcommand_runtime_config::Settable::No(
            actingcommand_runtime_config::NotSettable::ReadAtAssembly
        )
    );
}

// A value that is not a boolean is refused as the entry's other boolean members are: the entry
// does not decode (actingd's `config_decode_failed`), as `start_emulator` with the same value does.
#[test]
fn s2_config_a_non_boolean_start_paused_is_refused_as_the_entrys_other_members_are() {
    let root = TempDir::new().expect("tempdir");
    for value in [json!("true"), json!(1), json!({}), json!([true])] {
        let refused = serde_json::from_value::<ActingdConfigFile>(file(
            &root,
            entry(instance_id(), Some(value.clone())),
            &[],
        ));
        assert!(refused.is_err(), "start_paused {value} is refused");
        let mut other = entry(instance_id(), None);
        other["start_emulator"] = value.clone();
        assert!(
            serde_json::from_value::<ActingdConfigFile>(file(&root, other, &[])).is_err(),
            "start_emulator {value} is refused the same way"
        );
    }
}

// check-config reports the row with the instance's other keys. With the entry declaring the
// member and the configuration's own tag list leaving out host.start_pause_applied, it notes
// ledger_tag_key_unclassified with fact_key host.start_pause_applied, once; with the defaults'
// list (which holds the key) it notes nothing of it; a configuration without the member and the
// same own list notes nothing of it either.
#[test]
fn s2_config_check_config_reports_the_row_and_notes_a_tag_list_that_leaves_the_fact_out() {
    let root = TempDir::new().expect("tempdir");
    let id = instance_id();
    let own_list = json!({"tag": {"workbench_patterns": ["host.scheduling_pause"]}});
    let write = |name: &str, config: Value| {
        let path = root.path().join(name);
        fs::write(
            &path,
            serde_json::to_vec(&config).expect("the configuration JSON"),
        )
        .expect("write the configuration");
        path
    };
    let declared_own = write(
        "declared-own.json",
        file(
            &root,
            entry(id, Some(json!(true))),
            &[("ledger_observer", own_list.clone())],
        ),
    );
    let declared_default = write(
        "declared-default.json",
        file(&root, entry(id, Some(json!(true))), &[]),
    );
    let without_own = write(
        "without-own.json",
        file(&root, entry(id, None), &[("ledger_observer", own_list)]),
    );

    let (report, notes) = check(&declared_own);
    assert!(manifest_lists(&report, &key(id)), "{report}");
    assert_eq!(
        tag_notes(&notes),
        [json!({"fact_key": APPLIED})],
        "{notes:?}"
    );

    let (report, notes) = check(&declared_default);
    assert!(manifest_lists(&report, &key(id)), "{report}");
    assert!(tag_notes(&notes).is_empty(), "{notes:?}");

    let (report, notes) = check(&without_own);
    assert!(!manifest_lists(&report, &key(id)), "{report}");
    assert!(tag_notes(&notes).is_empty(), "{notes:?}");
}
