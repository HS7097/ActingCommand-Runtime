// SPDX-License-Identifier: AGPL-3.0-only

// #463 S2-2 (contracts/lab-recording.md, "Reviewing a recording step by step: `record review`";
// register §1.439), fix round 1: actinglab's command line of `record review` (the read is the Lab's,
// `crates/lab/src/gate_463_s22.rs`).
// - `s22_fix1_a_dry_run_without_no_preview_is_refused_before_the_recording_is_read`: under the
//   global `--dry-run`, `record review` without `--no-preview` would write its previews, so it is
//   `dry_run_unsupported` (exit 1, as every refusal on actinglab's own line) naming the form and
//   its read-only alternative, before the recording is read: on a session directory that holds
//   none it is not the read's own refusal. With `--no-preview`, and `--game`, `--server` and
//   `--locale`, which the command reads as `record ready` reads them, the read goes on and answers
//   that there is no recording (`record_session_not_active`). The session directory is never made
//   and no Runtime is reached.
// No Runtime, no device: an isolated environment whose Runtime state root does not exist.

use super::env_lock;
use super::gate_434_lab5b4::{Isolated, printed};
use serde_json::Value;
use std::path::Path;

/// What actinglab prints for `--dry-run record review` of the instance on the session directory
/// `state`, with `words` after it: its exit code and its line.
fn reviewed(state: &str, words: &[&str]) -> (u8, Value) {
    let mut line = vec![
        "--json",
        "--dry-run",
        "--instance",
        "neutral.a",
        "record",
        "review",
        "--state-dir",
        state,
    ];
    line.extend_from_slice(words);
    let exit = printed(&line);
    let shown = exit
        .line
        .clone()
        .unwrap_or_else(|text| panic!("{line:?} printed no line: {text}"));
    (exit.code, shown)
}

#[test]
fn s22_fix1_a_dry_run_without_no_preview_is_refused_before_the_recording_is_read() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");

    let (code, shown) = reviewed(&state, &[]);
    assert_eq!(code, 1, "{shown}");
    assert_eq!(shown["outcome"]["code"], "dry_run_unsupported", "{shown}");
    assert_eq!(shown["details"]["form"], "record review", "{shown}");
    assert_eq!(
        shown["details"]["read_only_alternative"], "record review --no-preview",
        "{shown}"
    );

    // With `--no-preview` nothing would be written: the line goes on to the read, which finds no
    // recording; the identity flags are the command's own.
    let (code, shown) = reviewed(
        &state,
        &[
            "--no-preview",
            "--game",
            "neutral",
            "--server",
            "test",
            "--locale",
            "en-US",
        ],
    );
    assert_eq!(code, 1, "{shown}");
    assert_eq!(
        shown["outcome"]["code"], "record_session_not_active",
        "{shown}"
    );
    assert!(
        !Path::new(&state).exists(),
        "record review made the session directory"
    );
    assert!(
        !isolated.runtime.exists(),
        "record review reached the Runtime's state root"
    );
}
