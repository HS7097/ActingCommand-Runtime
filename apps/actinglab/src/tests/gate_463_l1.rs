// SPDX-License-Identifier: AGPL-3.0-only

// #463 L1 (contracts/lab-recording.md, "One call per step" and "`--record` on device commands";
// register §1.370): the one-call form of `do --capture --record` keeps the device form's refusal of
// a dry run, and its mark request and next frame are read with `--record` only.
// - `l1_the_device_form_refuses_dry_run_and_its_flags_need_record`: `do --capture --record` with a
//   mark request and a next frame and with `--dry-run`, after the command or as the global option,
//   is `record_flag_unsupported`, as `do --capture --record --dry-run` is (the preview is `record
//   mark --dry-run`); each of `--mark-request`, `--mark-request-json`, `--next-frame-after-ms` and
//   `--next-frame-out` on `do --capture` without `--record` is `validation_failed`. Both refusals
//   come before any recording lock, Runtime connection or file: the Runtime state root the
//   environment names is never made.
// - `l1_fix1_the_next_frames_device_configuration_is_resolved_before_the_press` (fix round 1,
//   item 2): with `--next-frame-after-ms`, a user configuration whose entry for the instance the
//   recording lock names has a capture backend no capture knows ends the call under the lock with
//   that configuration's `validation_failed` (naming `instance.<alias>.capture_backend`), before
//   the step is planned: no Runtime state root is made, no next frame's file is written, and the
//   recording state root holds nothing but the recording lock's files.
// No Runtime, no device: an isolated environment whose Runtime state root does not exist.

use super::gate_434_lab5b4::{Isolated, printed};
use super::{
    env_lock, set_config_env, set_isolated_app_env, set_missing_config_env, use_runtime_state_root,
};
use crate::{SESSION_STATE_ENV, write_user_config};
use actingcommand_lab::{InstanceConfig, UserConfig};
use actingcommand_lab_ops::initiator::ENVIRONMENT_VARIABLE;
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::path::Path;

// What actinglab prints for `line`: its line.
fn answered(line: &[&str]) -> Value {
    printed(line)
        .line
        .unwrap_or_else(|text| panic!("{line:?} printed no line: {text}"))
}

#[test]
fn l1_the_device_form_refuses_dry_run_and_its_flags_need_record() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let package = isolated.path("neutral-package");
    let request_file = isolated.path("neutral-mark-request.json");
    let next_file = isolated.path("neutral-next-frame.png");
    let request = json!({
        "schema_version": "actingcommand.lab-record-mark.v1",
        "add": [],
        "click": {"region": {"x": 8, "y": 2, "width": 4, "height": 4}}
    })
    .to_string();
    let one_call = [
        "do",
        "--capture",
        "--record",
        "--package",
        package.as_str(),
        "--mark-request-json",
        request.as_str(),
        "--next-frame-after-ms",
        "800",
        "--next-frame-out",
        next_file.as_str(),
    ];
    for line in [
        [&["--json"][..], &one_call[..], &["--dry-run"][..]].concat(),
        [&["--json", "--dry-run"][..], &one_call[..]].concat(),
    ] {
        let shown = answered(&line);
        assert_eq!(
            shown["outcome"]["code"], "record_flag_unsupported",
            "{line:?}: {shown}"
        );
    }

    for (flag, value) in [
        ("--mark-request", request_file.as_str()),
        ("--mark-request-json", request.as_str()),
        ("--next-frame-after-ms", "800"),
        ("--next-frame-out", next_file.as_str()),
    ] {
        let line = [
            "--json",
            "do",
            "--capture",
            "--tap",
            "8,2",
            "--package",
            package.as_str(),
            flag,
            value,
        ];
        let shown = answered(&line);
        assert_eq!(
            shown["outcome"]["code"], "validation_failed",
            "{flag}: {shown}"
        );
    }
    assert!(
        !isolated.runtime.exists(),
        "a refusal reached the Runtime's state root"
    );
    assert!(
        !std::path::Path::new(&next_file).exists(),
        "a refusal wrote the next frame's file"
    );
}

#[test]
fn l1_fix1_the_next_frames_device_configuration_is_resolved_before_the_press() {
    const ALIAS: &str = "neutral.instance";
    let _guard = env_lock();
    let app_env = set_isolated_app_env();
    unsafe {
        env::remove_var(SESSION_STATE_ENV);
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let runtime = app_env.path().join("runtime-must-not-exist");
    let _runtime_env = use_runtime_state_root(&runtime);
    // The user configuration gives the instance a capture backend no capture knows.
    set_config_env(app_env.path().join("config.json"));
    let mut config = UserConfig::default();
    config.instances.insert(
        ALIAS.to_owned(),
        InstanceConfig {
            capture_backend: Some("neutral-unknown-backend".to_owned()),
            ..InstanceConfig::default()
        },
    );
    write_user_config(&config).expect("user configuration");
    // The default recording state root (no ACTINGLAB_SESSION_STATE_DIR) exists, as after record
    // start.
    let state = app_env
        .path()
        .join("ActingCommand")
        .join("actinglab")
        .join("session");
    fs::create_dir_all(&state).expect("recording state root");
    let package = app_env.path().join("neutral-package").display().to_string();
    let next_file = app_env
        .path()
        .join("neutral-next-frame.png")
        .display()
        .to_string();
    let request = json!({
        "schema_version": "actingcommand.lab-record-mark.v1",
        "add": [],
        "click": {"region": {"x": 8, "y": 2, "width": 4, "height": 4}}
    })
    .to_string();

    let shown = answered(&[
        "--json",
        "--instance",
        ALIAS,
        "do",
        "--capture",
        "--package",
        package.as_str(),
        "--mark-request-json",
        request.as_str(),
        "--next-frame-after-ms",
        "0",
        "--next-frame-out",
        next_file.as_str(),
        "--record",
    ]);
    set_missing_config_env();

    assert_eq!(shown["outcome"]["code"], "validation_failed", "{shown}");
    let message = shown["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("instance.neutral.instance.capture_backend"),
        "{shown}"
    );
    assert!(
        !runtime.exists(),
        "the call reached the Runtime's state root"
    );
    assert!(
        !Path::new(&next_file).exists(),
        "the call wrote the next frame's file"
    );
    let written = fs::read_dir(&state)
        .expect("recording state root")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| !name.ends_with(".lock") && !name.ends_with(".lock.json"))
        .collect::<Vec<_>>();
    assert!(written.is_empty(), "the call wrote {written:?}");
}
