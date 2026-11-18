// SPDX-License-Identifier: AGPL-3.0-only

// #463 PRIV-3 fix round 1 (contracts/lab-recording.md, "A recording's destination"; register
// §1.483; the Lab's tests are `crates/lab/src/gate_463_priv3.rs`): actinglab's command line hands
// `--destination` to `record start` and `record stop` as written, and checks its word there
// against the closed list the operation table gives it.
// - `priv3_fix1_the_command_line_refuses_a_third_destination`: `record start` and `record stop`
//   with `--destination remote` are `validation_failed`, the text naming the two words and the word
//   given, before any recording is read or written; no Runtime is reached.
// No Runtime, no device: an isolated environment whose Runtime state root does not exist.

use super::env_lock;
use super::gate_434_lab5b4::{Isolated, printed};

#[test]
fn priv3_fix1_the_command_line_refuses_a_third_destination() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    for words in [&["start", "--task-id", "neutral_task"][..], &["stop"][..]] {
        let mut line = vec!["--json", "--instance", "neutral.a", "record"];
        line.extend_from_slice(words);
        line.extend(["--state-dir", state.as_str(), "--destination", "remote"]);
        let shown = printed(&line)
            .line
            .unwrap_or_else(|text| panic!("{line:?} printed no line: {text}"));
        assert_eq!(
            shown["outcome"]["code"], "validation_failed",
            "{line:?}: {shown}"
        );
        assert!(
            shown["message"].as_str().is_some_and(|message| {
                message.contains("--destination must be local or public, got remote")
            }),
            "{line:?}: {shown}"
        );
    }
    assert!(
        !isolated.runtime.exists(),
        "a refusal reached the Runtime's state root"
    );
}
