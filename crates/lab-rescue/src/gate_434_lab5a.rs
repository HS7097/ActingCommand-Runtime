// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-5A (register §1.287, "Slice LAB-5A"; coordinator work order
//! `rd-1108/arch-013/brief-lab5a.md`): the rescue rows that took their source command's further
//! flags before (`op status`, `release verify`, `release stage` and the six `agent` rows) declare
//! them now, and the module hands each to the command under the flag it went as before: an
//! option as `--name value`, a switch that is set as `--name`, a switch given `false` (only
//! `serve --stdio` can give one) not at all; `agent claude-hook`'s action is the row's
//! positional, the word after `claude-hook` as before. Fix 1: `op status --operation <op>`,
//! which the command line still hands on undeclared, is written as before. Logic only: no root,
//! no command is run.
//! The members are written in name order, so that the map's order is the same with or without
//! serde_json's `preserve_order`.

use super::{agent_words, further_flags};
use serde_json::{Map, Value, json};
use std::ffi::OsString;
use std::path::Path;

fn params(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(members) => members,
        other => panic!("the parameters are an object: {other}"),
    }
}

fn words(line: &[&str]) -> Vec<String> {
    line.iter().map(|word| (*word).to_owned()).collect()
}

fn os_words(line: &[&str]) -> Vec<OsString> {
    line.iter().map(OsString::from).collect()
}

fn agent(sub: &str, value: Value) -> Vec<OsString> {
    agent_words(sub, &params(value), Path::new("state"))
}

#[test]
fn rescue_rows_hand_their_declared_parameters_on() {
    // `op status`: the positional is the caller's own (`--operation`); the switch as before.
    assert_eq!(
        further_flags(
            &params(json!({"check_rollback": true, "op": "op-1"})),
            &["op"]
        ),
        words(&["--check-rollback"])
    );
    assert!(
        further_flags(&params(json!({"check_rollback": false})), &["op"]).is_empty(),
        "a switch given false is not written"
    );
    // LAB-5A fix 1: the installer's own `--operation`, which the command line still hands on
    // undeclared, is written as before.
    assert_eq!(
        further_flags(&params(json!({"operation": "op-1"})), &["op"]),
        words(&["--operation", "op-1"])
    );
    // `release verify` and `release stage`: the options and the switch under their own flags.
    assert_eq!(
        further_flags(
            &params(json!({
                "dir": "release",
                "expect_commit": "c0ffee",
                "expect_tag": "v0.0.1",
                "facts": "facts.json"
            })),
            &["dir"]
        ),
        words(&[
            "--expect-commit",
            "c0ffee",
            "--expect-tag",
            "v0.0.1",
            "--facts",
            "facts.json"
        ])
    );
    assert_eq!(
        further_flags(
            &params(json!({"dir": "release", "local_build": true})),
            &["dir"]
        ),
        words(&["--local-build"])
    );
    // The `agent` rows: the sub-command, `claude-hook`'s action, the state root (not for
    // `notice`), then the options.
    assert_eq!(
        agent(
            "add",
            json!({"client": "neutral", "name": "n1", "native_id": "id1", "restore": "on"})
        ),
        os_words(&[
            "add",
            "--state-root",
            "state",
            "--client",
            "neutral",
            "--name",
            "n1",
            "--native-id",
            "id1",
            "--restore",
            "on"
        ])
    );
    assert_eq!(
        agent("remove", json!({"name": "n1"})),
        os_words(&["remove", "--state-root", "state", "--name", "n1"])
    );
    assert_eq!(
        agent("list", json!({})),
        os_words(&["list", "--state-root", "state"])
    );
    assert_eq!(
        agent("claude-hook", json!({"action": "start"})),
        os_words(&["claude-hook", "start", "--state-root", "state"])
    );
    assert_eq!(
        agent("claude-register", json!({"name": "n1"})),
        os_words(&["claude-register", "--state-root", "state", "--name", "n1"])
    );
    assert_eq!(agent("notice", json!({})), os_words(&["notice"]));
}
