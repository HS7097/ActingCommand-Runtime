// SPDX-License-Identifier: AGPL-3.0-only

// #463 L1 (contracts/lab-recording.md, "One call per step" and "`--record` on device commands";
// register §1.365): the one-call form of `do --capture --record` keeps the device form's refusal of
// a dry run, and its mark request and next frame are read with `--record` only.
// - `l1_the_device_form_refuses_dry_run_and_its_flags_need_record`: `do --capture --record` with a
//   mark request and a next frame and with `--dry-run`, after the command or as the global option,
//   is `record_flag_unsupported`, as `do --capture --record --dry-run` is (the preview is `record
//   mark --dry-run`); each of `--mark-request`, `--mark-request-json`, `--next-frame-after-ms` and
//   `--next-frame-out` on `do --capture` without `--record` is `validation_failed`. Both refusals
//   come before any recording lock, Runtime connection or file: the Runtime state root the
//   environment names is never made.
// No Runtime, no device: an isolated environment whose Runtime state root does not exist.

use super::env_lock;
use super::gate_434_lab5b4::{Isolated, printed};
use serde_json::{Value, json};

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
