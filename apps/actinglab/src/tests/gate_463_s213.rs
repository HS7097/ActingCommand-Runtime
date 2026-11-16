// SPDX-License-Identifier: AGPL-3.0-only

// #463 S2-13 (contracts/lab-recording.md, "The frames around a press: `record frames`"; register
// §1.426), fix round 2: actinglab's command line of `record frames`, which reads its numbers itself
// (the read is the Lab's, `crates/lab/src/gate_463_s213.rs`).
// - `s213_the_command_line_refuses_a_number_it_cannot_read`: `--before 1.5`, `--after abc` and
//   `--step x` are `validation_failed` with `details{argument, reason: "wrong_type", value}` (#463
//   stage two, train 1, fix round: the parent's word; it was `range`), the value as given; a flag
//   given twice is its last value, as the parent's flags read theirs: `--after 3 --after abc` and
//   `--step 1 --step x` are refused for the last value, and `--step x --step 1 --after abc --after
//   3` goes on to the read (no recording: `record_session_not_active`); a line without `--step` is
//   `validation_failed` naming `--step`. The session directory is never made and no Runtime is
//   reached.
// - `s213_the_read_creates_no_session_directory`: `record frames --step 1` on a session directory
//   that does not exist answers that there is no recording (`record_session_not_active`), and with
//   `--record-id` that the recording is not there (`record_lab_unavailable`); the directory is
//   still not there and no Runtime is reached.
// No Runtime, no device: an isolated environment whose Runtime state root does not exist.

use super::env_lock;
use super::gate_434_lab5b4::{Isolated, printed};
use serde_json::{Value, json};
use std::path::Path;

/// What actinglab prints for `record frames` of the instance on the session directory `state`,
/// with `words` after it.
fn framed(state: &str, words: &[&str]) -> Value {
    let mut line = vec![
        "--json",
        "--instance",
        "neutral.a",
        "record",
        "frames",
        "--state-dir",
        state,
    ];
    line.extend_from_slice(words);
    printed(&line)
        .line
        .unwrap_or_else(|text| panic!("{line:?} printed no line: {text}"))
}

#[test]
fn s213_the_command_line_refuses_a_number_it_cannot_read() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    // The train's fix round: a value that is no whole number is `wrong_type`, as the parent's flags
    // answer it; a flag given twice is its last value, so the last one is the one refused here.
    for (words, argument, value) in [
        (
            &["--step", "1", "--before", "1.5"][..],
            "before",
            json!("1.5"),
        ),
        (
            &["--step", "1", "--after", "abc"][..],
            "after",
            json!("abc"),
        ),
        (&["--step", "x"][..], "step", json!("x")),
        (
            &["--step", "1", "--after", "3", "--after", "abc"][..],
            "after",
            json!("abc"),
        ),
        (&["--step", "1", "--step", "x"][..], "step", json!("x")),
    ] {
        let shown = framed(&state, words);
        assert_eq!(
            shown["outcome"]["code"], "validation_failed",
            "{words:?}: {shown}"
        );
        assert_eq!(shown["details"]["argument"], argument, "{words:?}: {shown}");
        assert_eq!(
            shown["details"]["reason"], "wrong_type",
            "{words:?}: {shown}"
        );
        assert_eq!(shown["details"]["value"], value, "{words:?}: {shown}");
    }
    // Given twice, the last value is the one read: the earlier values that are no whole number are
    // not, and the read goes on to the recording, which is not there.
    let shown = framed(
        &state,
        &[
            "--step", "x", "--step", "1", "--after", "abc", "--after", "3",
        ],
    );
    assert_eq!(
        shown["outcome"]["code"], "record_session_not_active",
        "{shown}"
    );
    // Without `--step`.
    let shown = framed(&state, &["--before", "2"]);
    assert_eq!(shown["outcome"]["code"], "validation_failed", "{shown}");
    assert!(
        shown["message"]
            .as_str()
            .is_some_and(|message| message.contains("--step")),
        "{shown}"
    );
    assert!(
        !Path::new(&state).exists(),
        "a refusal made the session directory"
    );
    assert!(
        !isolated.runtime.exists(),
        "a refusal reached the Runtime's state root"
    );
}

#[test]
fn s213_the_read_creates_no_session_directory() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    let shown = framed(&state, &["--step", "1"]);
    assert_eq!(
        shown["outcome"]["code"], "record_session_not_active",
        "{shown}"
    );
    assert!(
        !Path::new(&state).exists(),
        "record frames made the session directory"
    );
    let shown = framed(&state, &["--step", "1", "--record-id", "record_s213"]);
    assert_eq!(
        shown["outcome"]["code"], "record_lab_unavailable",
        "{shown}"
    );
    assert_eq!(
        shown["details"]["reason"], "lab_recording_missing",
        "{shown}"
    );
    assert!(
        !Path::new(&state).exists(),
        "record frames --record-id made the session directory"
    );
    assert!(
        !isolated.runtime.exists(),
        "record frames reached the Runtime's state root"
    );
}
