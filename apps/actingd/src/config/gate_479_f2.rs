// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #479 F-2 (register §1.482; contracts/device-binding.md, "An iOS handset";
// `actingd-config` revision 34; the coordinator's rulings 1 and 3 in work order
// rd-1118/brief-479-f2-ios-handset-config-binding.md, and fix round 1's items 3 and 5 and its
// small items in rd-1119/brief-479-f2-fix1.md): an instance's `device` takes `system`, `android`
// (also what an absent member means) or `ios`, beside `kind: "handset"` alone. An iOS handset
// loads, reports `instance.<id>.device_system`, names no backend and no member only a device
// reached through adb has, and is registered with the private file's iOS row (its phone's key,
// never an identifier) or unbound. A row of the other device system unbinds its handset alone,
// with one warning, and every other instance starts. An Android handset without `system`
// assembles as before, with no such row. The ids, aliases, paths and keys are made up.

use super::device_binding::write_private_file_for_test;
use super::*;
use crate::codes::DeviceConfigProblem;
use actingcommand_contract::outcome::Category;
use actingcommand_contract::{
    ConfigParameterSource, DeviceBindingProblem, DeviceBoundVia, FactScalar, IdentifierIssuer,
};
use actingcommand_runtime_host::{ResolvedAdbEndpoint, ResolvedInstanceEndpoint};
use serde_json::{Value, json};
use tempfile::TempDir;

const IPHONE: &str = "neutral.iphone";
const PHONE: &str = "neutral.phone";
const EMULATOR: &str = "neutral.emulator";
const SERIAL: &str = "TESTSERIAL0479";
/// The phone's key the private file holds, made under the installation's own secret.
const KEY: &str = "sha256:4790f2000000000000000000000000000000000000000000000000000000f2a1";
const BOUND_AT_UNIX_MS: u64 = 1_700_000_000_479;

fn instance_id() -> InstanceId {
    *IdentifierIssuer::new()
        .expect("issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport()
}

/// An iOS handset's entry: its alias, its id, its application, and its device; no backend.
fn ios_entry(id: InstanceId) -> Value {
    json!({
        "alias": IPHONE,
        "instance_id": id,
        "application_id": "neutral.application",
        "device": {"kind": "handset", "system": "ios"}
    })
}

/// An Android handset's entry as RD-1b wrote one: no `system`.
fn android_entry(id: InstanceId) -> Value {
    json!({
        "alias": PHONE,
        "instance_id": id,
        "application_id": "neutral.application",
        "adb_path": "neutral-adb",
        "touch_backend": "adb_shell_input",
        "capture_backend": "adb",
        "device": {"kind": "handset"}
    })
}

/// An emulator's entry, addressed by its port, as RD-1b's T9 writes one.
fn emulator_entry(id: InstanceId) -> Value {
    json!({
        "alias": EMULATOR,
        "instance_id": id,
        "application_id": "neutral.application",
        "adb_path": "neutral-adb",
        "port": 16479,
        "touch_backend": "adb_shell_input",
        "capture_backend": "adb"
    })
}

fn config(state_root: &Path, instances: Vec<Value>) -> ActingdConfigFile {
    serde_json::from_value(json!({
        "schema_version": CONFIG_SCHEMA_VERSION,
        "state_root": state_root,
        "bind_host": "127.0.0.1",
        "bind_port": 0,
        "secret_fingerprint_salt": "0123456789abcdef",
        "instances": instances
    }))
    .expect("typed config")
}

/// The private binding file of `state_root` holding `rows`, in the file's version `schema`.
fn bindings(state_root: &Path, schema: &str, rows: Vec<Value>) {
    write_private_file_for_test(
        state_root,
        &serde_json::to_vec(&json!({"schema": schema, "bindings": rows})).expect("the file"),
    );
}

fn ios_row(alias: &str) -> Value {
    json!({
        "alias": alias,
        "system": "ios",
        "device_key": KEY,
        "bound_at_unix_ms": BOUND_AT_UNIX_MS,
        "bound_via": "cli"
    })
}

fn android_row(alias: &str) -> Value {
    json!({
        "alias": alias,
        "serial": SERIAL,
        "bound_at_unix_ms": BOUND_AT_UNIX_MS,
        "bound_via": "cli"
    })
}

/// `instance.<id>.<member>`.
fn key(id: InstanceId, member: &str) -> String {
    format!(
        "{}.{}.{member}",
        actingcommand_runtime_config::INSTANCE_SECTION,
        manifest::instance_id_text(id).expect("the id's text"),
    )
}

/// The handset the registry registered for `alias`.
fn handset_of(assembly: RuntimeAssembly, alias: &str) -> HandsetEndpoint {
    let registry = assembly
        .provider
        .into_registry()
        .expect("configured registry");
    registry
        .resolve(alias)
        .expect("the registered handset")
        .adb_endpoint()
        .and_then(ResolvedInstanceEndpoint::bound)
        .and_then(ResolvedAdbEndpoint::handset)
        .cloned()
        .expect("a handset endpoint")
}

// `system: "ios"` beside `handset` loads, bound by the private file's iOS row or unbound; the
// manifest reports `instance.<id>.device_system` (explicit, not settable) beside its kind, and the
// key table has the row; an iOS handset uses no adb. An Android handset without `system` loads as
// before, with no such row.
#[test]
fn f2_config_an_ios_handset_loads_and_reports_its_system_and_an_android_one_as_before() {
    let root = TempDir::new().expect("tempdir");
    let (iphone, phone) = (instance_id(), instance_id());
    bindings(
        root.path(),
        "actingcommand.device-bindings.v2",
        vec![ios_row(IPHONE)],
    );
    let assembly = config(root.path(), vec![ios_entry(iphone)])
        .assemble()
        .expect("an iOS handset assembles");
    assert!(assembly.unbound_handsets.is_empty());
    assert!(assembly.mismatched_handsets.is_empty());
    let parameter = |assembly: &RuntimeAssembly, key: &str| {
        assembly
            .manifest
            .parameters
            .iter()
            .find(|parameter| parameter.key == key)
            .cloned()
    };
    let system = parameter(&assembly, &key(iphone, "device_system")).expect("the system's row");
    assert_eq!(system.value, FactScalar::String("ios".to_owned()));
    assert_eq!(system.source, ConfigParameterSource::Explicit);
    assert!(!system.settable);
    assert_eq!(
        parameter(&assembly, &key(iphone, "device_kind")).map(|row| row.value),
        Some(FactScalar::String("handset".to_owned()))
    );
    for member in ["touch_backend", "capture_backend"] {
        assert!(
            parameter(&assembly, &key(iphone, member)).is_none(),
            "an iOS handset names no {member}"
        );
    }
    let HandsetEndpoint::Ios(Some(bound)) = handset_of(assembly, IPHONE) else {
        panic!("an iOS handset bound by its key");
    };
    assert_eq!(bound.device_key().as_str(), KEY);
    assert_eq!(
        (bound.bound_at_unix_ms(), bound.bound_via()),
        (BOUND_AT_UNIX_MS, DeviceBoundVia::Cli)
    );

    // Unbound: registered, reported unbound, as an Android handset without a binding is.
    let unbound_root = TempDir::new().expect("tempdir");
    let assembly = config(unbound_root.path(), vec![ios_entry(iphone)])
        .assemble()
        .expect("an unbound iOS handset assembles");
    assert_eq!(assembly.unbound_handsets, [IPHONE]);
    assert!(assembly.mismatched_handsets.is_empty());
    assert_eq!(handset_of(assembly, IPHONE), HandsetEndpoint::Ios(None));

    // An Android handset without `system`: no system row, bound by its serial as before.
    let android_root = TempDir::new().expect("tempdir");
    bindings(
        android_root.path(),
        "actingcommand.device-bindings.v1",
        vec![android_row(PHONE)],
    );
    let assembly = config(android_root.path(), vec![android_entry(phone)])
        .assemble()
        .expect("an Android handset assembles");
    assert!(
        assembly
            .manifest
            .parameters
            .iter()
            .all(|parameter| !parameter.key.ends_with(".device_system")),
        "an Android handset that names no system has no row"
    );
    assert!(matches!(
        handset_of(assembly, PHONE),
        HandsetEndpoint::Bound(_)
    ));

    let (row, _) = actingcommand_runtime_config::find_row(&key(iphone, "device_system"))
        .expect("the key table row");
    assert_eq!(
        row.name,
        actingcommand_runtime_config::KeyName::PerInstance(
            actingcommand_runtime_config::DEVICE_SYSTEM
        )
    );
    assert_eq!(
        row.range,
        actingcommand_runtime_config::KeyRange::Tokens(&["android", "ios"])
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

// `system` beside a kind other than `handset`, or a word outside the pair, is refused by the
// existing rule for an invalid device member (`device_config_invalid`, `system_unsupported`); an
// iOS handset that names a backend is `backend_kind_unsupported`; fix round 1: one that names a
// member only a device reached through adb has (its adb, its commands' timeout, a touch tool's
// member) is refused the same way, `adb_member_with_ios`, never ignored.
#[test]
fn f2_config_system_is_a_handsets_member_of_its_pair_and_an_ios_handset_names_no_backend() {
    let id = instance_id();
    let emulator_with_system = {
        let mut entry = android_entry(id);
        entry["device"] = json!({"kind": "emulator", "system": "ios"});
        entry
    };
    let unknown_system = {
        let mut entry = ios_entry(id);
        entry["device"]["system"] = json!("neutral-system");
        entry
    };
    let ios_with = |member: &str, value: Value| {
        let mut entry = ios_entry(id);
        entry[member] = value;
        entry
    };
    for (entry, problem) in [
        (emulator_with_system, DeviceConfigProblem::SystemUnsupported),
        (unknown_system, DeviceConfigProblem::SystemUnsupported),
        (
            ios_with("touch_backend", json!("adb_shell_input")),
            DeviceConfigProblem::BackendKindUnsupported,
        ),
        (
            ios_with("capture_backend", json!("adb")),
            DeviceConfigProblem::BackendKindUnsupported,
        ),
        (
            ios_with("adb_path", json!("neutral-adb")),
            DeviceConfigProblem::AdbMemberWithIos,
        ),
        (
            ios_with("command_timeout_ms", json!(5_000)),
            DeviceConfigProblem::AdbMemberWithIos,
        ),
        (
            ios_with("maatouch_local_path", json!("neutral-maatouch")),
            DeviceConfigProblem::AdbMemberWithIos,
        ),
        (
            ios_with("minitouch_local_path", json!("neutral-minitouch")),
            DeviceConfigProblem::AdbMemberWithIos,
        ),
        (
            ios_with("push_touch_tool", json!(false)),
            DeviceConfigProblem::AdbMemberWithIos,
        ),
        (
            ios_with("handshake_timeout_ms", json!(1_000)),
            DeviceConfigProblem::AdbMemberWithIos,
        ),
        (
            ios_with("shutdown_timeout_ms", json!(1_000)),
            DeviceConfigProblem::AdbMemberWithIos,
        ),
        (
            ios_with("tap_hold_ms", json!(50)),
            DeviceConfigProblem::AdbMemberWithIos,
        ),
    ] {
        let root = TempDir::new().expect("tempdir");
        let alias = entry["alias"].clone();
        let config = config(root.path(), vec![entry]);
        let refused = config.device_refusal().expect("a device refusal");
        assert_eq!(
            Value::Object(refused.outcome().values().clone()),
            json!({"alias": alias, "device_config_problem": problem.as_str()}),
            "{problem}"
        );
        assert_eq!(
            config.assemble().err().map(Code::as_str),
            Some(ActingdCode::DeviceConfigInvalid.as_str()),
            "{problem}"
        );
    }
}

// Fix round 1, item 3: a private row of another device system than its handset's configuration
// names (an Android row for an iOS handset, and the reverse) unbinds that handset only: the start
// registers it unbound and says so in one warning, naming its alias and the word and no
// identifier, and every other instance assembles as it would without it (the good Android row
// binds its handset, the emulator is registered). check-config refuses the file as before
// (`device_binding_file_invalid` `system_mismatch`).
#[test]
fn f2_config_a_row_of_the_other_system_unbinds_its_handset_alone_and_the_others_start() {
    let (iphone, phone, emulator) = (instance_id(), instance_id(), instance_id());

    // An iOS handset whose row is an Android row, beside a good Android row and an emulator.
    let root = TempDir::new().expect("tempdir");
    let mut iphone_row = android_row(IPHONE);
    iphone_row["serial"] = json!("TESTSERIAL0480");
    bindings(
        root.path(),
        "actingcommand.device-bindings.v1",
        vec![iphone_row, android_row(PHONE)],
    );
    let file = config(
        root.path(),
        vec![
            ios_entry(iphone),
            android_entry(phone),
            emulator_entry(emulator),
        ],
    );
    let refused = file
        .device_refusal()
        .expect("check-config's refusal of the file");
    assert_eq!(refused.code(), ActingdCode::DeviceBindingFileInvalid.code());
    assert_eq!(
        refused.outcome().values()["device_binding_problem"],
        json!(DeviceBindingProblem::SystemMismatch.as_str())
    );
    let assembly = file.assemble().expect("the start assembles");
    assert_eq!(assembly.unbound_handsets, [IPHONE]);
    assert_eq!(assembly.mismatched_handsets, [IPHONE]);
    let warnings = binding_mismatch_warnings(&assembly.mismatched_handsets);
    let [warning] = &warnings[..] else {
        panic!("one warning: {warnings:?}");
    };
    assert_eq!(warning.code().as_str(), "device_binding_ignored");
    assert_eq!(warning.category(), Category::Warning);
    assert_eq!(
        Value::Object(warning.values().clone()),
        json!({"alias": IPHONE, "device_binding_problem": "system_mismatch"})
    );
    let shown =
        serde_json::to_string(&warning.envelope(actingcommand_contract::outcome::Detail::Full))
            .expect("the warning's envelope");
    assert!(
        !shown.contains("TESTSERIAL0480") && !shown.contains(SERIAL),
        "{shown}"
    );
    let registry = assembly
        .provider
        .into_registry()
        .expect("configured registry");
    let handset = |alias: &str| {
        registry
            .resolve(alias)
            .expect("registered")
            .adb_endpoint()
            .and_then(ResolvedInstanceEndpoint::bound)
            .and_then(ResolvedAdbEndpoint::handset)
            .cloned()
    };
    assert_eq!(handset(IPHONE), Some(HandsetEndpoint::Ios(None)));
    assert!(matches!(handset(PHONE), Some(HandsetEndpoint::Bound(_))));
    assert_eq!(handset(EMULATOR), None, "the emulator is registered as one");

    // An Android handset whose row is an iOS row: the same, the other way round.
    let root = TempDir::new().expect("tempdir");
    bindings(
        root.path(),
        "actingcommand.device-bindings.v2",
        vec![ios_row(PHONE)],
    );
    let assembly = config(
        root.path(),
        vec![android_entry(phone), emulator_entry(emulator)],
    )
    .assemble()
    .expect("the start assembles");
    assert_eq!(assembly.unbound_handsets, [PHONE]);
    assert_eq!(assembly.mismatched_handsets, [PHONE]);
    assert_eq!(handset_of(assembly, PHONE), HandsetEndpoint::Unbound);

    // A file malformed as a whole keeps refusing the start.
    let root = TempDir::new().expect("tempdir");
    write_private_file_for_test(root.path(), b"{not json");
    assert_eq!(
        config(
            root.path(),
            vec![ios_entry(iphone), emulator_entry(emulator)]
        )
        .assemble()
        .err()
        .map(Code::as_str),
        Some(ActingdCode::DeviceBindingFileInvalid.as_str())
    );
}
