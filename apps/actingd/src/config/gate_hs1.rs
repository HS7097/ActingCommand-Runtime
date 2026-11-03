// SPDX-License-Identifier: AGPL-3.0-only

// #402 HS-1 (register §1.267; design `rd-1031/backend-switch/model-backend-switch.md` §2.1, §2.3,
// §7 HS-1; coordinator work order `brief-hs-1.md`): the start-up assembly reports each device
// instance's `touch_backend` and `capture_backend` by the device layer's own names (explicit, not
// settable; a fixture has neither), and `actingd-config` revision 26 relaxes `nemu_app_index`: Nemu
// touch no longer needs it in the file, and an emulator entry may name it whatever its backends.
// Nemu touch still needs Nemu capture (`nemu_input_requires_paired_capture`), a named index is
// still checked (`nemu_app_index_invalid`) and a handset still names none. The names are made up.

use super::*;
use actingcommand_contract::{ConfigParameterSource, FactScalar, IdentifierIssuer, InstanceId};
use actingcommand_runtime_config::{CAPTURE_BACKEND, TOUCH_BACKEND, instance_key};
use serde_json::{Value, json};
use tempfile::TempDir;

fn instance_id() -> InstanceId {
    IdentifierIssuer::new()
        .expect("issuer")
        .mint_instance_id()
        .expect("instance id")
}

/// A device instance entry `alias` of `id` with these members besides.
fn device(root: &TempDir, alias: &str, id: InstanceId, members: Value) -> Value {
    let mut entry = json!({
        "alias": alias, "instance_id": id.transport(),
        "application_id": "neutral.application", "adb_path": root.path().join("missing-adb.exe"),
        "connect": false, "host": "127.0.0.1", "port": 16384
    });
    for (member, value) in members.as_object().expect("members").clone() {
        entry[member] = value;
    }
    entry
}

/// An actingd configuration of these instance entries.
fn configuration(root: &TempDir, instances: Vec<Value>) -> ActingdConfigFile {
    serde_json::from_value::<ActingdConfigFile>(json!({
        "schema_version": CONFIG_SCHEMA_VERSION,
        "state_root": root.path(),
        "bind_host": "127.0.0.1",
        "bind_port": 0,
        "secret_fingerprint_salt": "0123456789abcdef",
        "instances": instances
    }))
    .expect("typed config")
}

/// The manifest row of `member` of the instance `id`, when the manifest reports one.
fn row(
    assembly: &RuntimeAssembly,
    id: InstanceId,
    member: &str,
) -> Option<actingcommand_contract::ConfigParameter> {
    let key = instance_key(
        &manifest::instance_id_text(id).expect("the id's text"),
        member,
    );
    assembly
        .manifest
        .parameters
        .iter()
        .find(|parameter| parameter.key == key)
        .cloned()
}

/// Asserts the instance's two backend rows: these names, explicit, not settable.
fn assert_backends(assembly: &RuntimeAssembly, id: InstanceId, touch: &str, capture: &str) {
    for (member, name) in [(TOUCH_BACKEND, touch), (CAPTURE_BACKEND, capture)] {
        let reported = row(assembly, id, member).unwrap_or_else(|| panic!("{member} is reported"));
        assert_eq!(
            reported.value,
            FactScalar::String(name.to_owned()),
            "{member}"
        );
        assert_eq!(reported.source, ConfigParameterSource::Explicit, "{member}");
        assert!(!reported.settable, "{member} is not settable until HS-2");
    }
}

#[test]
fn hs1_backend_rows_in_manifest() {
    let root = TempDir::new().expect("tempdir");
    let (first, second, fixture) = (instance_id(), instance_id(), instance_id());
    let assembly = configuration(
        &root,
        vec![
            // Aliases the device layer reads: `adb` touch and `screencap` capture.
            device(
                &root,
                "neutral.first",
                first,
                json!({"touch_backend": "adb", "capture_backend": "screencap"}),
            ),
            device(
                &root,
                "neutral.second",
                second,
                json!({"touch_backend": "maatouch", "capture_backend": "nemu", "port": 16416}),
            ),
            json!({
                "alias": "neutral.fixture", "instance_id": fixture.transport(),
                "fixture_backend": {
                    "frames": [{"width": 1, "height": 1, "rgb": [1, 2, 3]}],
                    "max_inputs": 0
                }
            }),
        ],
    )
    .assemble()
    .expect("runtime assembly");

    // By the device layer's own names, the file's choices.
    assert_backends(&assembly, first, "adb_shell_input", "adb");
    assert_backends(&assembly, second, "maatouch", "nemu_ipc");
    // A fixture has neither row.
    for member in [TOUCH_BACKEND, CAPTURE_BACKEND] {
        assert!(row(&assembly, fixture, member).is_none(), "{member}");
    }
    // The rows are the `instance` section's.
    assert!(assembly.manifest.parameter_sections().contains(&"instance"));
}

#[test]
fn hs1_nemu_touch_without_app_index_assembles() {
    let root = TempDir::new().expect("tempdir");
    let id = instance_id();
    // Nemu touch beside Nemu capture, no `nemu_app_index`: revision 26 takes it (0.12.38 refused it
    // as `nemu_paired_input_configuration_missing`).
    let assembly = configuration(
        &root,
        vec![device(
            &root,
            "neutral.nemu",
            id,
            json!({"touch_backend": "nemu_ipc", "capture_backend": "nemu_ipc"}),
        )],
    )
    .assemble()
    .expect("Nemu touch without nemu_app_index assembles");
    assert_backends(&assembly, id, "nemu_ipc", "nemu_ipc");
    assert_eq!(
        assembly.provider.mode_for_alias("neutral.nemu"),
        Some(ScheduledExecutionMode::DeviceRegistry)
    );

    // With an index, too.
    configuration(
        &root,
        vec![device(
            &root,
            "neutral.nemu",
            id,
            json!({"touch_backend": "nemu_ipc", "capture_backend": "nemu_ipc", "nemu_app_index": 1}),
        )],
    )
    .assemble()
    .expect("Nemu touch with nemu_app_index assembles");

    // Nemu touch still needs Nemu capture, with or without an index.
    for members in [
        json!({"touch_backend": "nemu_ipc", "capture_backend": "adb"}),
        json!({"touch_backend": "nemu_ipc", "capture_backend": "droidcast_raw", "nemu_app_index": 0}),
    ] {
        let Err((code, _)) = configuration(
            &root,
            vec![device(&root, "neutral.nemu", id, members.clone())],
        )
        .assemble_or_refusal() else {
            panic!("{members} is refused");
        };
        assert_eq!(
            code,
            actingcommand_runtime_host::HostCode::NemuInputRequiresPairedCapture.code(),
            "{members}"
        );
    }
}

#[test]
fn hs1_app_index_with_adb_touch_assembles() {
    let root = TempDir::new().expect("tempdir");
    let id = instance_id();
    // An emulator entry names the index beside any other backends: revision 26 takes it (0.12.38
    // refused it as `nemu_app_index_requires_paired_input`); only Nemu touch uses it.
    for members in [
        json!({"touch_backend": "adb_shell_input", "capture_backend": "nemu_ipc", "nemu_app_index": 0}),
        json!({"touch_backend": "maatouch", "capture_backend": "adb", "nemu_app_index": 3}),
    ] {
        let assembly = configuration(
            &root,
            vec![device(&root, "neutral.adb", id, members.clone())],
        )
        .assemble()
        .unwrap_or_else(|code| panic!("{members}: {code:?}"));
        assert_eq!(
            assembly.provider.mode_for_alias("neutral.adb"),
            Some(ScheduledExecutionMode::DeviceRegistry),
            "{members}"
        );
    }

    // A named index is still checked, whatever the backends.
    let members = json!({
        "touch_backend": "adb_shell_input", "capture_backend": "adb",
        "nemu_app_index": 2_147_483_648_u64
    });
    let Err((code, _)) =
        configuration(&root, vec![device(&root, "neutral.adb", id, members)]).assemble_or_refusal()
    else {
        panic!("an index beyond the Nemu input's range is refused");
    };
    assert_eq!(code, ActingdCode::NemuAppIndexInvalid.code());

    // A handset still names none: it has no Nemu at all.
    let handset = json!({
        "alias": "neutral.phone", "instance_id": id.transport(),
        "application_id": "neutral.application", "adb_path": root.path().join("missing-adb.exe"),
        "touch_backend": "adb_shell_input", "capture_backend": "adb", "nemu_app_index": 0,
        "device": {"kind": "handset", "transport": "usb"}
    });
    let Err((code, _)) = configuration(&root, vec![handset]).assemble_or_refusal() else {
        panic!("a handset's nemu_app_index is refused");
    };
    assert_eq!(code, ActingdCode::DeviceConfigInvalid.code());
}
