// SPDX-License-Identifier: AGPL-3.0-only

// #398 DB-2 (register §1.467; contracts/device-binding.md, "Choosing a device"; the coordinator's
// ruling 5 in work order rd-1117/brief-398-db2-devices-before-install.md): a client learns what
// the Lab it carries can do without a new interface name, by asking it for a row of the operation
// table this build embeds: `actinglab --json help <row words>` (Workflow #434 CLI-HELP, register
// §1.313), which reads nothing else and sends nothing.
// - db2_t5_help_answers_the_parameters_of_devices_and_devices_bind_as_the_table_holds_them:
//   `--json help devices` and `--json help devices bind` answer `command.params`, each parameter's
//   name, form, type and required as the table file holds them, in its order; among them what an
//   installer looks for: `devices` takes `root`, `config`, `key_salt` and `key_salt_env`, and
//   `devices bind` takes `config` and `device_key`, its `handle` no longer required. Fix round 1
//   (work order rd-1118/brief-398-db2-fix1.md, item 4): every option either row takes is among
//   them, `devices bind`'s `root` too, and each parameter's written form (`params[].written`) is
//   pinned.
// The operation table this build embeds and the table file; no Runtime, no device, no clock.

use super::gate_434_lab5b4::{Isolated, printed};
use super::*;
use serde_json::{Value, json};

/// The operation table as this repository holds it.
const TABLE: &str = include_str!("../../../../contracts/lab-operations.json");

/// Fix round 1: each row's parameters as the command line writes them (row, parameter, written
/// form), in the table's order.
const WRITTEN: [(&str, &str, &str); 12] = [
    ("devices", "config", "--config <file>"),
    ("devices", "root", "--root <dir>"),
    ("devices", "key_salt", "--key-salt <string>"),
    ("devices", "key_salt_env", "--key-salt-env <string>"),
    ("devices bind", "instance", "<instance>"),
    ("devices bind", "handle", "<handle>"),
    ("devices bind", "config", "--config <file>"),
    ("devices bind", "bound_via", "--bound-via <ui>"),
    ("devices bind", "device_key", "--device-key <string>"),
    ("devices bind", "key_salt", "--key-salt <string>"),
    ("devices bind", "key_salt_env", "--key-salt-env <string>"),
    ("devices bind", "root", "--root <dir>"),
];

/// A parameter's name, form, type and required, as data.
fn shape(param: &Value) -> Value {
    json!({
        "name": param["name"],
        "form": param["form"],
        "type": param["type"],
        "required": param["required"],
    })
}

/// The table file's parameters of the row `name`, each as [`shape`] gives it, in their order.
fn declared(name: &str) -> Vec<Value> {
    let table: Value = serde_json::from_str(TABLE).expect("the table file reads");
    let row = table["operations"]
        .as_array()
        .expect("its operations")
        .iter()
        .find(|row| row["name"] == name)
        .unwrap_or_else(|| panic!("the table has the row {name}"))
        .clone();
    row["params"]
        .as_array()
        .expect("its parameters")
        .iter()
        .map(shape)
        .collect()
}

/// The names among `params`.
fn names(params: &[Value]) -> Vec<String> {
    params
        .iter()
        .map(|param| param["name"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[test]
fn db2_t5_help_answers_the_parameters_of_devices_and_devices_bind_as_the_table_holds_them() {
    let _guard = env_lock();
    let _isolated = Isolated::new();
    let mut answered = Vec::new();
    for (name, spoken) in [
        ("devices", &["devices"][..]),
        ("devices bind", &["devices", "bind"][..]),
    ] {
        let exit = printed(&[&["--json", "help"][..], spoken].concat());
        assert_eq!(exit.code, 0, "{name}: {:?}", exit.line);
        let line = exit.line.expect("help's line");
        let command = &line["command"];
        assert_eq!(command["name"], name, "{line}");
        let given = command["params"]
            .as_array()
            .unwrap_or_else(|| panic!("{name}'s parameters: {line}"));
        let params = given.iter().map(shape).collect::<Vec<_>>();
        assert_eq!(params, declared(name), "{name}: as the table holds them");
        // Fix round 1: how the command line writes each, as a client passes it.
        let written = given
            .iter()
            .map(|param| (param["name"].clone(), param["written"].clone()))
            .collect::<Vec<_>>();
        let pinned = WRITTEN
            .iter()
            .filter(|(row, _, _)| *row == name)
            .map(|(_, param, form)| (json!(param), json!(form)))
            .collect::<Vec<_>>();
        assert_eq!(written, pinned, "{name}: each parameter's written form");
        answered.push(params);
    }

    // What an installer looks for before it offers a phone.
    let devices = names(&answered[0]);
    for name in ["root", "config", "key_salt", "key_salt_env"] {
        assert!(devices.iter().any(|given| given == name), "devices: {name}");
    }
    let bind = names(&answered[1]);
    // Fix round 1: and `devices bind`'s `--root`, the command line's own, declared now.
    for name in ["config", "device_key", "root"] {
        assert!(
            bind.iter().any(|given| given == name),
            "devices bind: {name}"
        );
    }
    let handle = answered[1]
        .iter()
        .find(|param| param["name"] == "handle")
        .expect("devices bind's handle");
    assert_eq!(
        handle["required"], false,
        "the device key may take its place"
    );
}
