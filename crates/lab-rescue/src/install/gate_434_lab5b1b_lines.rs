// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-5B1b (register §1.287, "Slice LAB-5B1b"; coordinator work order
// rd-1113/brief-lab5b1b.md, point 3): the command lines the rescue module's handed rows build,
// written out word by word, pinned on the code as it was before the slice's refactor, which leaves
// this file untouched.
// - `rescue_handed_lines_are_pinned`: for each row whose answer is an installer command (`up`,
//   `down`, `undo`, `op status`, `op resume`, `leftovers`, `guards`, `release-hold`, `release
//   verify`, `release stage`, `adopt`), the line the module hands to `install` (test builds keep
//   each, `rescue::installed`), and for each `agent` row the words it hands to the agent channel
//   (`rescue::agent_words`): with no parameter, with each declared parameter alone and with all of
//   them, given in an order of their own, which the members written after the row's own words
//   keep; and `op status`'s `--operation`, which the command line hands on undeclared.
// The fake host of `fixture.rs` and its fake clock; the root does not exist, so that each installer
// command refuses at it once its line is kept: no device, no Runtime, nothing written but the fake
// host's temporary folder.

use super::fixture::FakeHost;
use crate::rescue::{self, Context, installed};
use actingcommand_lab_ops::initiator::Initiator;
use serde_json::{Map, Value, json};
use std::ffi::OsString;
use std::path::Path;
use tempfile::TempDir;

/// The root the module acts on: no folder, so that every installer command refuses at it.
const ROOT: &str = "neutral-root-of-no-folder";
/// The state root the agent channel's words name.
const STATE: &str = "state";
/// The hold reason `down` gives itself on a fresh fake clock (`lab-down-<ms>`).
const OWN_REASON: &str = "lab-down-1800000000000";

/// One pinned line: the row, its parameters and the words it hands on.
type Pinned = (&'static str, Value, &'static [&'static str]);

fn pin(row: &'static str, params: Value, words: &'static [&'static str]) -> Pinned {
    (row, params, words)
}

fn object(params: &Value) -> Map<String, Value> {
    match params {
        Value::Object(members) => members.clone(),
        other => panic!("the parameters are an object: {other}"),
    }
}

/// The rows whose answer is an installer command, and the line each hands to `install`.
fn installer_lines() -> Vec<Pinned> {
    vec![
        pin("up", json!({}), &["start", "--root", ROOT]),
        pin(
            "down",
            json!({}),
            &["stop", "--root", ROOT, "--hold", OWN_REASON],
        ),
        pin(
            "down",
            json!({"reason": "r1"}),
            &["stop", "--root", ROOT, "--hold", "r1"],
        ),
        pin("undo", json!({}), &["undo", "--root", ROOT]),
        pin(
            "undo",
            json!({"op": "op-1"}),
            &["undo", "--root", ROOT, "--operation", "op-1"],
        ),
        pin("op status", json!({}), &["status", "--root", ROOT]),
        pin(
            "op status",
            json!({"op": "op-1"}),
            &["status", "--root", ROOT, "--operation", "op-1"],
        ),
        pin(
            "op status",
            json!({"check_rollback": true}),
            &["status", "--root", ROOT, "--check-rollback"],
        ),
        pin(
            "op status",
            json!({"check_rollback": true, "op": "op-1"}),
            &[
                "status",
                "--root",
                ROOT,
                "--operation",
                "op-1",
                "--check-rollback",
            ],
        ),
        // The installer's own `--operation`, which the command line hands on undeclared.
        pin(
            "op status",
            json!({"operation": "op-2"}),
            &["status", "--root", ROOT, "--operation", "op-2"],
        ),
        pin("op resume", json!({}), &["resume", "--root", ROOT]),
        pin(
            "op resume",
            json!({"op": "op-1"}),
            &["resume", "--root", ROOT, "--operation", "op-1"],
        ),
        pin("leftovers", json!({}), &["leftovers", "--root", ROOT]),
        pin("guards", json!({}), &["guards", "--root", ROOT]),
        pin(
            "guards",
            json!({"for": "stop"}),
            &["guards", "--root", ROOT, "--for", "stop"],
        ),
        pin("release-hold", json!({}), &["release-hold", "--root", ROOT]),
        pin(
            "release-hold",
            json!({"reason": "r1"}),
            &["release-hold", "--root", ROOT, "--reason", "r1"],
        ),
        pin("release verify", json!({}), &["release", "verify"]),
        pin(
            "release verify",
            json!({"dir": "neutral-release"}),
            &["release", "verify", "--dir", "neutral-release"],
        ),
        pin(
            "release verify",
            json!({"facts": "neutral-facts.json"}),
            &["release", "verify", "--facts", "neutral-facts.json"],
        ),
        pin(
            "release verify",
            json!({"expect_tag": "v0.0.1"}),
            &["release", "verify", "--expect-tag", "v0.0.1"],
        ),
        pin(
            "release verify",
            json!({"expect_commit": "c0ffee00"}),
            &["release", "verify", "--expect-commit", "c0ffee00"],
        ),
        pin(
            "release verify",
            json!({
                "expect_commit": "c0ffee00",
                "facts": "neutral-facts.json",
                "dir": "neutral-release",
                "expect_tag": "v0.0.1"
            }),
            &[
                "release",
                "verify",
                "--dir",
                "neutral-release",
                "--expect-commit",
                "c0ffee00",
                "--facts",
                "neutral-facts.json",
                "--expect-tag",
                "v0.0.1",
            ],
        ),
        pin(
            "release stage",
            json!({}),
            &["release", "stage", "--root", ROOT],
        ),
        pin(
            "release stage",
            json!({"dir": "neutral-release"}),
            &[
                "release",
                "stage",
                "--root",
                ROOT,
                "--dir",
                "neutral-release",
            ],
        ),
        pin(
            "release stage",
            json!({"facts": "neutral-facts.json"}),
            &[
                "release",
                "stage",
                "--root",
                ROOT,
                "--facts",
                "neutral-facts.json",
            ],
        ),
        pin(
            "release stage",
            json!({"expect_tag": "v0.0.1"}),
            &["release", "stage", "--root", ROOT, "--expect-tag", "v0.0.1"],
        ),
        pin(
            "release stage",
            json!({"expect_commit": "c0ffee00"}),
            &[
                "release",
                "stage",
                "--root",
                ROOT,
                "--expect-commit",
                "c0ffee00",
            ],
        ),
        pin(
            "release stage",
            json!({"local_build": true}),
            &["release", "stage", "--root", ROOT, "--local-build"],
        ),
        pin(
            "release stage",
            json!({
                "local_build": true,
                "expect_tag": "v0.0.1",
                "dir": "neutral-release",
                "facts": "neutral-facts.json",
                "expect_commit": "c0ffee00"
            }),
            &[
                "release",
                "stage",
                "--root",
                ROOT,
                "--dir",
                "neutral-release",
                "--local-build",
                "--expect-tag",
                "v0.0.1",
                "--facts",
                "neutral-facts.json",
                "--expect-commit",
                "c0ffee00",
            ],
        ),
        pin("adopt", json!({}), &["adopt", "--root", ROOT]),
    ]
}

/// The `agent` rows, and the words each hands to the agent channel.
fn agent_lines() -> Vec<Pinned> {
    vec![
        pin("agent add", json!({}), &["add", "--state-root", STATE]),
        pin(
            "agent add",
            json!({"name": "n1"}),
            &["add", "--state-root", STATE, "--name", "n1"],
        ),
        pin(
            "agent add",
            json!({"client": "neutral"}),
            &["add", "--state-root", STATE, "--client", "neutral"],
        ),
        pin(
            "agent add",
            json!({"native_id": "id1"}),
            &["add", "--state-root", STATE, "--native-id", "id1"],
        ),
        pin(
            "agent add",
            json!({"dsh_app_root": "neutral-app"}),
            &[
                "add",
                "--state-root",
                STATE,
                "--dsh-app-root",
                "neutral-app",
            ],
        ),
        pin(
            "agent add",
            json!({"dsh_home": "neutral-home"}),
            &["add", "--state-root", STATE, "--dsh-home", "neutral-home"],
        ),
        pin(
            "agent add",
            json!({"dsh_node": "neutral-node"}),
            &["add", "--state-root", STATE, "--dsh-node", "neutral-node"],
        ),
        pin(
            "agent add",
            json!({"restore": "on"}),
            &["add", "--state-root", STATE, "--restore", "on"],
        ),
        pin(
            "agent add",
            json!({
                "restore": "on",
                "native_id": "id1",
                "dsh_node": "neutral-node",
                "name": "n1",
                "dsh_home": "neutral-home",
                "client": "neutral",
                "dsh_app_root": "neutral-app"
            }),
            &[
                "add",
                "--state-root",
                STATE,
                "--restore",
                "on",
                "--native-id",
                "id1",
                "--dsh-node",
                "neutral-node",
                "--name",
                "n1",
                "--dsh-home",
                "neutral-home",
                "--client",
                "neutral",
                "--dsh-app-root",
                "neutral-app",
            ],
        ),
        pin(
            "agent remove",
            json!({}),
            &["remove", "--state-root", STATE],
        ),
        pin(
            "agent remove",
            json!({"name": "n1"}),
            &["remove", "--state-root", STATE, "--name", "n1"],
        ),
        pin("agent list", json!({}), &["list", "--state-root", STATE]),
        pin(
            "agent claude-hook",
            json!({}),
            &["claude-hook", "--state-root", STATE],
        ),
        pin(
            "agent claude-hook",
            json!({"action": "start"}),
            &["claude-hook", "start", "--state-root", STATE],
        ),
        pin(
            "agent claude-register",
            json!({}),
            &["claude-register", "--state-root", STATE],
        ),
        pin(
            "agent claude-register",
            json!({"name": "n1"}),
            &["claude-register", "--state-root", STATE, "--name", "n1"],
        ),
        pin("agent notice", json!({}), &["notice"]),
    ]
}

#[test]
fn rescue_handed_lines_are_pinned() {
    let work = TempDir::new().expect("a temporary folder");
    let root = Path::new(ROOT);
    let location = rescue::location_of(root, &root.join("actingd.config.json"), Path::new(STATE));
    let mut failures = Vec::new();
    for (row, params, pinned) in installer_lines() {
        // A fresh fake clock for each line, so that `down`'s own reason is the same each time.
        let host = FakeHost::new(work.path());
        let context = Context {
            host: &host,
            location: &location,
            initiator: Initiator::LabRescue,
        };
        installed::take();
        rescue::answer_with(&context, row, &object(&params));
        let handed = installed::take();
        let pinned = vec![
            pinned
                .iter()
                .map(|word| (*word).to_owned())
                .collect::<Vec<_>>(),
        ];
        if handed != pinned {
            failures.push(format!(
                "{row} {params}: handed {handed:?}, pinned {pinned:?}"
            ));
        }
    }
    for (row, params, pinned) in agent_lines() {
        let sub = row.strip_prefix("agent ").expect("an agent row");
        let handed = rescue::agent_words(sub, &object(&params), Path::new(STATE));
        let pinned = pinned.iter().map(OsString::from).collect::<Vec<_>>();
        if handed != pinned {
            failures.push(format!(
                "{row} {params}: handed {handed:?}, pinned {pinned:?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "command lines that differ from their pin:\n{}",
        failures.join("\n")
    );
}
