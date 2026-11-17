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
//   `devices bind` takes `config` and `device_key`, its `handle` no longer required.
// The operation table this build embeds and the table file; no Runtime, no device, no clock.

use super::gate_434_lab5b4::{Isolated, printed};
use super::*;
use serde_json::{Value, json};

/// The operation table as this repository holds it.
const TABLE: &str = include_str!("../../../../contracts/lab-operations.json");

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
        let params = command["params"]
            .as_array()
            .unwrap_or_else(|| panic!("{name}'s parameters: {line}"))
            .iter()
            .map(shape)
            .collect::<Vec<_>>();
        assert_eq!(params, declared(name), "{name}: as the table holds them");
        answered.push(params);
    }

    // What an installer looks for before it offers a phone.
    let devices = names(&answered[0]);
    for name in ["root", "config", "key_salt", "key_salt_env"] {
        assert!(devices.iter().any(|given| given == name), "devices: {name}");
    }
    let bind = names(&answered[1]);
    for name in ["config", "device_key"] {
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
