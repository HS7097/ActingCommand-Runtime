// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #479 F-2 (register §1.482; contracts/device-binding.md, "An iOS handset";
// `actingd-config` revision 34; the coordinator's rulings 1, 2 and 3 in work order
// rd-1118/brief-479-f2-ios-handset-config-binding.md): an instance's `device` takes `system`,
// `android` (also what an absent member means) or `ios`, beside `kind: "handset"` alone. An iOS
// handset loads, reports `instance.<id>.device_system`, names no backend, and is registered with
// the private file's iOS row (its phone's key, never an identifier) or unbound; `acib_path` is its
// member alone and need not exist. An Android handset without `system` assembles as before, with no
// such row. The ids, aliases, paths and keys are made up; no file of `acib` exists.

use super::device_binding::write_private_file_for_test;
use super::*;
use crate::codes::DeviceConfigProblem;
use actingcommand_contract::{
    ConfigParameterSource, DeviceBindingProblem, DeviceBoundVia, FactScalar, IdentifierIssuer,
};
use actingcommand_runtime_host::{ResolvedAdbEndpoint, ResolvedInstanceEndpoint};
use serde_json::{Value, json};
use tempfile::TempDir;

const IPHONE: &str = "neutral.iphone";
const PHONE: &str = "neutral.phone";
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
// before, with no such row. An `acib_path` that names a file that does not exist loads.
#[test]
fn f2_config_an_ios_handset_loads_and_reports_its_system_and_an_android_one_as_before() {
    let root = TempDir::new().expect("tempdir");
    let (iphone, phone) = (instance_id(), instance_id());
    bindings(
        root.path(),
        "actingcommand.device-bindings.v2",
        vec![ios_row(IPHONE)],
    );
    let mut entry = ios_entry(iphone);
    entry["acib_path"] = json!(root.path().join("absent").join("acib.exe"));
    let assembly = config(root.path(), vec![entry])
        .assemble()
        .expect("an iOS handset assembles");
    assert!(assembly.unbound_handsets.is_empty());
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
    assert_eq!(handset_of(assembly, IPHONE), HandsetEndpoint::Ios(None));

    // An Android handset without `system`: no system row, bound by its serial as before.
    let android_root = TempDir::new().expect("tempdir");
    bindings(
        android_root.path(),
        "actingcommand.device-bindings.v1",
        vec![json!({
            "alias": PHONE,
            "serial": "TESTSERIAL0479",
            "bound_at_unix_ms": BOUND_AT_UNIX_MS,
            "bound_via": "cli"
        })],
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
// iOS handset that names a backend is `backend_kind_unsupported`; `acib_path` on an instance that
// is no iOS handset is `acib_path_without_ios`; an empty one is `instance_config_invalid`. The
// private file's row of another system than its handset's is `device_binding_file_invalid`
// `system_mismatch`.
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
    let with_touch = {
        let mut entry = ios_entry(id);
        entry["touch_backend"] = json!("adb_shell_input");
        entry
    };
    let with_capture = {
        let mut entry = ios_entry(id);
        entry["capture_backend"] = json!("adb");
        entry
    };
    let android_with_acib = {
        let mut entry = android_entry(id);
        entry["acib_path"] = json!("acib.exe");
        entry
    };
    for (entry, problem) in [
        (emulator_with_system, DeviceConfigProblem::SystemUnsupported),
        (unknown_system, DeviceConfigProblem::SystemUnsupported),
        (with_touch, DeviceConfigProblem::BackendKindUnsupported),
        (with_capture, DeviceConfigProblem::BackendKindUnsupported),
        (android_with_acib, DeviceConfigProblem::AcibPathWithoutIos),
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

    let root = TempDir::new().expect("tempdir");
    let mut empty_acib = ios_entry(id);
    empty_acib["acib_path"] = json!(" ");
    assert_eq!(
        config(root.path(), vec![empty_acib])
            .assemble()
            .err()
            .map(Code::as_str),
        Some(ActingdCode::InstanceConfigInvalid.as_str())
    );

    // A row of the other system: an iOS row for an Android handset, and the reverse.
    for (entry, row) in [
        (android_entry(id), ios_row(PHONE)),
        (
            ios_entry(id),
            json!({
                "alias": IPHONE,
                "serial": "TESTSERIAL0479",
                "bound_at_unix_ms": BOUND_AT_UNIX_MS,
                "bound_via": "cli"
            }),
        ),
    ] {
        let root = TempDir::new().expect("tempdir");
        bindings(root.path(), "actingcommand.device-bindings.v2", vec![row]);
        let config = config(root.path(), vec![entry]);
        let refused = config.device_refusal().expect("a binding file refusal");
        assert_eq!(
            refused.outcome().values()["device_binding_problem"],
            json!(DeviceBindingProblem::SystemMismatch.as_str())
        );
        assert_eq!(
            config.assemble().err().map(Code::as_str),
            Some(ActingdCode::DeviceBindingFileInvalid.as_str())
        );
    }
}
