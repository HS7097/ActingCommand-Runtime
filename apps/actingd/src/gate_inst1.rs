// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #421 INST-1, test row IT-13 (frozen model INST-M v1.0 §4.4.1, §12;
// contracts/actingd-check-config.md, "Invocation"): `check-config --install-root <root>`, run
// from a build folder that is no install root (this test binary's), judges the install root as
// `<root>`: its adb is `<root>\tools\platform-tools\adb.exe` and the default vision root is
// `<root>\vision`. A root that holds no `runtime\BUILD-MANIFEST.json` is
// `check_config_install_root_invalid` with the given root as `path`.

use super::*;
use serde_json::{Value, json};
use std::fs;

fn arguments(values: &[&str]) -> Vec<std::ffi::OsString> {
    values.iter().map(std::ffi::OsString::from).collect()
}

#[test]
fn inst1_it_13_check_config_install_root_judges_adb_and_vision_against_the_named_root() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("runtime")).expect("the runtime folder");
    fs::write(root.join("runtime").join("BUILD-MANIFEST.json"), b"{}").expect("the build manifest");
    // The root's vision folder: one flat OCR model and the runtime library (a placeholder; no
    // model is loaded by check-config).
    let model = root.join("vision").join("models").join("neutral.model");
    fs::create_dir_all(&model).expect("the model folder");
    fs::write(model.join("rec.onnx"), b"model").expect("the recognizer");
    fs::write(model.join("keys.txt"), b"a").expect("the dictionary");
    fs::create_dir_all(root.join("vision").join("ort")).expect("the runtime folder");
    fs::write(root.join("vision").join("ort").join("onnxruntime.dll"), b"")
        .expect("the runtime library");
    // The configuration lies elsewhere: without `--install-root` its default vision root would
    // be next to it.
    let elsewhere = temp.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).expect("the configuration folder");
    let config = elsewhere.join("actingd.config.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "schema_version": "actingcommand.actingd.config.v1",
            "state_root": temp.path().join("state"),
            "bind_host": "127.0.0.1",
            "secret_fingerprint_salt": "0123456789abcdef",
            "vision": {"execution_provider": "cpu"},
            "instances": []
        }))
        .expect("the configuration JSON"),
    )
    .expect("write the configuration");
    let config_word = config.to_str().expect("a UTF-8 path");
    let root_word = root.to_str().expect("a UTF-8 path");

    let reported = check_config::run(arguments(&[
        "check-config",
        "--config",
        config_word,
        "--install-root",
        root_word,
    ]))
    .unwrap_or_else(|error| panic!("check-config passes: {}", error.outcome()));
    let report = reported.data.get("report").expect("the report");
    let canonical = fs::canonicalize(&root).expect("the canonical root");
    let adb = canonical
        .join("tools")
        .join("platform-tools")
        .join("adb.exe");
    assert_eq!(
        report["adb_default"]["path"],
        Value::from(adb.to_string_lossy().into_owned()),
        "{report}"
    );
    let vision = fs::canonicalize(root.join("vision")).expect("the canonical vision root");
    assert_eq!(
        report["vision"]["root"],
        Value::from(vision.to_string_lossy().into_owned()),
        "{report}"
    );

    // A folder without `runtime\BUILD-MANIFEST.json` is no install root.
    let plain = temp.path().join("plain");
    fs::create_dir_all(&plain).expect("a plain folder");
    let refused = check_config::run(arguments(&[
        "check-config",
        "--config",
        config_word,
        "--install-root",
        plain.to_str().expect("a UTF-8 path"),
    ]))
    .expect_err("a plain folder is refused");
    let outcome = refused.outcome();
    assert_eq!(
        outcome.code().as_str(),
        "check_config_install_root_invalid",
        "{outcome}"
    );
    assert_eq!(
        outcome.value("path"),
        Some(&Value::from(plain.to_string_lossy().into_owned())),
        "{outcome}"
    );
}
