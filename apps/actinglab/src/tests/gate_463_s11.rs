// SPDX-License-Identifier: AGPL-3.0-only

// #463 S1-1 (contracts/lab-recording.md, "Recording while playing" and "`--record` on device
// commands"; register §1.402): `tap`, `swipe` and `long-tap` take `--record` and its wait
// `--after-ms`, and refuse what the device recording forms refuse.
// - `s11_the_device_commands_refuse_a_dry_run_and_after_ms_needs_record`: `tap 1 2 --record` with
//   `--dry-run`, after the command or as the global option, and `swipe` and `long-tap` the same,
//   are `record_flag_unsupported`; `tap 1 2 --after-ms 5` (without `--record`) is
//   `validation_failed` with `details{argument: "after_ms", reason: "record_only"}`; `key back
//   --record` is `record_flag_unsupported`. Every refusal comes before any recording lock or Runtime
//   connection: the Runtime state root the environment names is never made.
// No Runtime, no device: an isolated environment whose Runtime state root does not exist.

use super::env_lock;
use super::gate_434_lab5b4::{Isolated, printed};
use serde_json::Value;

// What actinglab prints for `line`: its line.
fn answered(line: &[&str]) -> Value {
    printed(line)
        .line
        .unwrap_or_else(|text| panic!("{line:?} printed no line: {text}"))
}

#[test]
fn s11_the_device_commands_refuse_a_dry_run_and_after_ms_needs_record() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    for line in [
        &["--json", "tap", "1", "2", "--record", "--dry-run"][..],
        &["--json", "--dry-run", "tap", "1", "2", "--record"][..],
        &[
            "--json",
            "swipe",
            "1",
            "2",
            "3",
            "4",
            "300",
            "--record",
            "--dry-run",
        ][..],
        &[
            "--json",
            "long-tap",
            "1",
            "2",
            "800",
            "--record",
            "--dry-run",
        ][..],
        &["--json", "key", "back", "--record"][..],
    ] {
        let shown = answered(line);
        assert_eq!(
            shown["outcome"]["code"], "record_flag_unsupported",
            "{line:?}: {shown}"
        );
    }
    let shown = answered(&["--json", "tap", "1", "2", "--after-ms", "5"]);
    assert_eq!(shown["outcome"]["code"], "validation_failed", "{shown}");
    assert_eq!(shown["details"]["argument"], "after_ms", "{shown}");
    assert_eq!(shown["details"]["reason"], "record_only", "{shown}");
    assert!(
        !isolated.runtime.exists(),
        "a refusal reached the Runtime's state root"
    );
}
