// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 S3-4 (register §1.411; contracts/standard-pack-authoring.md, "A held decision by
// two flags"; coordinator work order `rd-1115/brief-463-s34-adopt-held.md`, ruling 3): `actinglab
// standard-pack adopt` takes `--held-reason <text>` and `--held-reference <text>` in place of
// `--decide`, never beside it.
// - `s34_the_held_flags_beside_decide_or_the_reference_alone_are_refused_before_anything_is_read`:
//   `--decide` with `--held-reason`, in either order and with `--held-reference` too, is
//   `validation_failed`, exit 2, no data, naming `--held-reason` with `argument_problem`
//   `conflicting`; `--decide` with `--held-reference` alone, and `--held-reference` without
//   `--held-reason`, the same naming `--held-reference`. Each comes before anything is read: the
//   checkout, the package, the decision document and the state directory the line names do not
//   exist, so a call that read any of them would be refused `adopt_refused`, and none of them is
//   made.
// No Runtime, no device, no clock: an isolated environment and paths that do not exist.

use super::env_lock;
use super::gate_434_lab5b4::Isolated;
use crate::run_cli;
use serde_json::Value;
use std::path::Path;

#[test]
fn s34_the_held_flags_beside_decide_or_the_reference_alone_are_refused_before_anything_is_read() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let repo = isolated.path("neutral-repo");
    let package = isolated.path("neutral-package");
    let decision = isolated.path("neutral-decision.json");
    let state = isolated.path("neutral-state");
    let adopt = [
        "--json",
        "standard-pack",
        "adopt",
        "--repo",
        repo.as_str(),
        "--package",
        package.as_str(),
        "--state-dir",
        state.as_str(),
    ];
    let decide = ["--decide", decision.as_str()];
    let reason = ["--held-reason", "Not run through on a real device yet."];
    let reference = ["--held-reference", "neutral reference 1"];
    for (flags, named) in [
        ([&decide[..], &reason[..]].concat(), "--held-reason"),
        ([&reason[..], &decide[..]].concat(), "--held-reason"),
        (
            [&decide[..], &reason[..], &reference[..]].concat(),
            "--held-reason",
        ),
        ([&decide[..], &reference[..]].concat(), "--held-reference"),
        (reference.to_vec(), "--held-reference"),
    ] {
        let line = [&adopt[..], &flags[..]].concat();
        let result = run_cli(line.clone(), true);
        let shown = result.envelope_json();
        assert_eq!(result.exit_code(), 2, "{line:?}: {shown}");
        assert!(result.envelope.data.is_none(), "{line:?}: {shown}");
        let error = result
            .envelope
            .error
            .as_ref()
            .unwrap_or_else(|| panic!("{line:?} was not refused: {shown}"));
        assert_eq!(error.code, "validation_failed", "{line:?}: {shown}");
        let details = error.details.clone().unwrap_or(Value::Null);
        assert_eq!(details["argument"], named, "{line:?}: {shown}");
        assert_eq!(
            details["argument_problem"], "conflicting",
            "{line:?}: {shown}"
        );
    }
    for path in [&repo, &package, &decision, &state] {
        assert!(!Path::new(path).exists(), "{path} was made");
    }
    assert!(
        !isolated.runtime.exists(),
        "a refusal reached the Runtime's state root"
    );
}
