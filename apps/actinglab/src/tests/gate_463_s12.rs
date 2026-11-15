// SPDX-License-Identifier: AGPL-3.0-only

// #463 S1-2 (contracts/lab-recording.md, "Recording while playing" and "`--record` on device
// commands"; register §1.403): the flags of the captures after a recorded press, `--interval-ms`,
// `--still-ms` and `--still-max-ms`, are taken with `--record` only.
// - `s12_the_flags_of_the_captures_after_a_press_need_record`: `tap 1 2`, `swipe 1 2 3 4 300` and
//   `long-tap 1 2 800`, each with one of the three flags and without `--record`, are
//   `validation_failed` with `details{argument, reason: "record_only"}`, the argument the flag's
//   name (`interval_ms`, `still_ms`, `still_max_ms`). Every refusal comes before any recording lock
//   or Runtime connection: the Runtime state root the environment names is never made.
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
fn s12_the_flags_of_the_captures_after_a_press_need_record() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    for command in [
        &["tap", "1", "2"][..],
        &["swipe", "1", "2", "3", "4", "300"][..],
        &["long-tap", "1", "2", "800"][..],
    ] {
        for (flag, argument) in [
            ("--interval-ms", "interval_ms"),
            ("--still-ms", "still_ms"),
            ("--still-max-ms", "still_max_ms"),
        ] {
            let mut line = vec!["--json"];
            line.extend_from_slice(command);
            line.extend([flag, "50"]);
            let shown = answered(&line);
            assert_eq!(
                shown["outcome"]["code"], "validation_failed",
                "{line:?}: {shown}"
            );
            assert_eq!(shown["details"]["argument"], argument, "{line:?}: {shown}");
            assert_eq!(
                shown["details"]["reason"], "record_only",
                "{line:?}: {shown}"
            );
        }
    }
    assert!(
        !isolated.runtime.exists(),
        "a refusal reached the Runtime's state root"
    );
}
