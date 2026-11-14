// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #453 S4 (register, "Slice #453 S4"; contracts/install-cli.md, "Fresh root"; coordinator
// work order rd-1114/brief-453-s4-install-new.md, "Tests"): the first configuration's template the
// Runtime member ships, `distribution/windows/actingd.config.first.json`, judged by this build's
// own configuration reader and check-config, and the example configuration left as it was:
// - `s4_the_first_configuration_template_decodes_in_composer_mode_with_one_host_and_no_instance`:
//   the template holds no salt and no state root; filled as the rescue module's `new` fills it
//   (an absolute state root, a salt of 64 hexadecimal digits, the authoring switch) and written as
//   the configuration of a temporary install root, it loads and assembles as actingd's start does
//   (`load`, `assemble_or_refusal`), in composer mode (a `policy` without `catalog`) with one host,
//   each of its three `*_available_milli` 1000, and no instance in `instances` or in
//   `policy.facts.instances`; check-config with `--install-root` passes it;
// - `s4_the_example_configuration_is_unchanged`: `distribution/windows/actingd.config.example.json`
//   byte for byte as before the slice (CI starts the Runtime from it).
// No device, no Runtime, no clock: the reader and check-config only read the files.

use super::*;
use serde_json::{Value, json};
use std::fs;

/// The template, as the release's Runtime member carries it.
const TEMPLATE: &str = include_str!("../../../distribution/windows/actingd.config.first.json");
/// The example configuration.
const EXAMPLE: &str = include_str!("../../../distribution/windows/actingd.config.example.json");
/// The example configuration's bytes before the slice.
const EXAMPLE_BEFORE: &str = r#"{
  "schema_version": "actingcommand.actingd.config.v1",
  "state_root": "",
  "bind_host": "127.0.0.1",
  "bind_port": 0,
  "secret_fingerprint_salt": "",
  "execution": {
    "action_gap": {
      "kind": "performance"
    }
  },
  "instances": []
}
"#;

#[test]
fn s4_the_first_configuration_template_decodes_in_composer_mode_with_one_host_and_no_instance() {
    let template: Value = serde_json::from_str(TEMPLATE).expect("the template is JSON");
    // The command fills these; the template holds neither.
    assert_eq!(template["state_root"], json!(""));
    assert_eq!(template["secret_fingerprint_salt"], json!(""));
    // Composer mode, one host, no instance; the standard-pack root relative to the root.
    assert!(template["policy"].get("catalog").is_none(), "composer mode");
    let hosts = template["policy"]["resources"]["hosts"]
        .as_array()
        .expect("the hosts");
    assert_eq!(hosts.len(), 1);
    for member in [
        "cpu_available_milli",
        "gpu_available_milli",
        "io_available_milli",
    ] {
        assert_eq!(hosts[0][member], json!(1000), "{member}");
    }
    assert_eq!(template["policy"]["facts"]["instances"], json!([]));
    assert_eq!(template["instances"], json!([]));
    let packs = template["standard_packs"]["root"]
        .as_str()
        .expect("the standard-pack root");
    assert!(!Path::new(packs).is_absolute(), "{packs}");

    // Filled as `new` fills it, in a temporary install root.
    let temp = tempfile::TempDir::new().expect("tempdir");
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("runtime")).expect("the runtime folder");
    fs::write(root.join("runtime").join("BUILD-MANIFEST.json"), b"{}").expect("the manifest");
    let state = root.join("state");
    fs::create_dir_all(&state).expect("the state root");
    let mut filled = template.clone();
    filled["state_root"] = json!(state);
    filled["secret_fingerprint_salt"] = json!("0123456789abcdef".repeat(4));
    filled["lab_module_authoring"] = json!(true);
    let path = root.join("actingd.config.json");
    fs::write(
        &path,
        serde_json::to_vec_pretty(&filled).expect("configuration JSON"),
    )
    .expect("write the configuration");

    // This build's reader takes it, as the start reads it.
    let file = config::load(&path).unwrap_or_else(|code| panic!("load refused: {}", code.as_str()));
    if let Err((code, outcome)) = file.assemble_or_refusal() {
        panic!("assembly refused: {} {outcome}", code.as_str());
    }
    // And this build's check-config passes it, judged as the root's.
    let words = vec![
        std::ffi::OsString::from("check-config"),
        std::ffi::OsString::from("--config"),
        path.as_os_str().to_owned(),
        std::ffi::OsString::from("--install-root"),
        root.as_os_str().to_owned(),
    ];
    if let Err(error) = check_config::run(words) {
        panic!("check-config refused: {}", error.outcome());
    }
}

#[test]
fn s4_the_example_configuration_is_unchanged() {
    assert_eq!(EXAMPLE, EXAMPLE_BEFORE);
}
