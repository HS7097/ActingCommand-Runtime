// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #382 A-1 (test plan v2.2 §3.7), actingctl's binary.
// G10 (apps 13): a usage error of pause / resume, emulator, selfcheck and install-transition is
// reported before any connection (inventory-apps item 13). The grammar: contracts/
// scheduling-pause.md "CLI" (the scope is global only without --instance; --reason and
// --drain-timeout-ms are checked against their bounds, 1..=64 bytes of [a-z0-9_.-] and
// 1000..=600000, before any request is sent); contracts/emulator-control.md (status, start,
// stop and restart require --instance; discover takes none); README.md (`selfcheck <alias>`);
// distribution/windows/INSTALL.md "Host installation control" (`--action-json` with one of five
// actions, deadlines bounded at 600 s). The state root has no Runtime: a form the grammar takes
// meets the client's connection failure, and a usage error is `actingctl_usage_invalid` on the one
// result line with exit 1 (model v2.2 §7.3, §7.6). The flag rows are A1-FLAG's (flag_refusals.rs);
// what the taken forms parse to is `gate_382_a1` in the binary.
// G10 (apps 15): a Runtime refuses the request of each of these subcommands; the line is
// `{outcome}` topped by the refused receipt's outcome, and the exit is 1 (model v2.2 §7.2, §7.3,
// §7.6; scheduling-pause.md: an unknown alias is `instance_unknown` for pause and resume).

use crate::held_runtime::{HELD_SALT, NoInstances};
use actingcommand_contract::{
    EmulatorInstanceAction, EventActor, EventSource, SchedulingPauseReason, SchedulingPauseScope,
};
use actingcommand_runtime_client::{
    RuntimeChannel, RuntimeClient, RuntimeClientConfig, RuntimeClientError,
};
use actingcommand_runtime_host::{RuntimeHost, RuntimeHostConfig};
use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;
use tempfile::TempDir;

const ALIAS: &str = "neutral.instance";
// An alias the Runtime does not register.
const UNKNOWN: &str = "neutral.unregistered";
const QUERY: &str = r#"{"action":"query","transition_id":"transition-grammar"}"#;
const USAGE_INVALID: &str = "actingctl_usage_invalid";

fn actingctl(state_root: &Path, words: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .args(words)
        .arg("--state-root")
        .arg(state_root)
        .env_remove("ACTINGCOMMAND_DETAIL")
        .output()
        .expect("run actingctl")
}

/// The one stdout result line of an exit (model v2.2 §7.3); `Err` says what is not one.
fn result_line(output: &Output) -> Result<Value, String> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let (Some(line), None) = (lines.next(), lines.next()) else {
        return Err(format!(
            "not one stdout line: {stdout:?}; stderr: {:?}",
            String::from_utf8_lossy(&output.stderr)
        ));
    };
    serde_json::from_str(line).map_err(|error| format!("stdout is not JSON ({error}): {line}"))
}

/// The outcome code a form the grammar takes meets on `root`, a state root without a Runtime:
/// the client's refusal to connect.
fn unconnected_code(root: &Path) -> String {
    let Err(unconnected) = RuntimeClient::connect(RuntimeClientConfig::new(
        root,
        RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    )) else {
        panic!("a state root without a Runtime refuses the connection");
    };
    let unconnected = unconnected.outcome().code().as_str().to_owned();
    assert_ne!(unconnected, USAGE_INVALID);
    unconnected
}

/// Runs each of `usage` and `taken` on a state root without a Runtime; what does not end as the
/// grammar requires: a usage error is `actingctl_usage_invalid` with exit 1, before any
/// connection, and a taken form meets the client's refusal to connect.
fn grammar_mismatches(usage: &[&[&str]], taken: &[&[&str]]) -> Vec<String> {
    let root = TempDir::new().expect("tempdir");
    let unconnected = unconnected_code(root.path());
    let mut wrong = Vec::new();
    let required = usage
        .iter()
        .map(|words| (words, USAGE_INVALID, Some(1)))
        .chain(
            taken
                .iter()
                .map(|words| (words, unconnected.as_str(), None)),
        );
    for (words, code, exit) in required {
        let output = actingctl(root.path(), words);
        let found =
            result_line(&output).map(|line| line["outcome"]["code"].as_str().map(str::to_owned));
        let exit_ok = exit.is_none_or(|exit| output.status.code() == Some(exit));
        if !exit_ok || !matches!(&found, Ok(Some(found)) if found == code) {
            wrong.push(format!(
                "{words:?}: {}, outcome code {found:?}, required {code}",
                output.status
            ));
        }
    }
    wrong
}

#[test]
fn gate_a_usage_error_of_these_subcommands_is_reported_before_any_connection() {
    let long_reason = "r".repeat(65);
    let begin_drain_over_600_s =
        r#"{"action":"begin_drain","transition_id":"transition-grammar","timeout_ms":600001}"#;
    let usage: &[&[&str]] = &[
        &["pause", "--reason", "Operator"],
        &["pause", "--reason", long_reason.as_str()],
        &["pause", "--drain-timeout-ms", "999"],
        &["pause", "--drain-timeout-ms", "600001"],
        &["emulator"],
        &["emulator", "reboot", "--instance", ALIAS],
        // The instance subcommands of `emulator` require `--instance`.
        &["emulator", "status"],
        &["emulator", "start"],
        &["emulator", "stop"],
        &["emulator", "restart"],
        &["selfcheck"],
        &["install-transition"],
        &["install-transition", "--action-json", "{"],
        &[
            "install-transition",
            "--action-json",
            r#"{"action":"upgrade","transition_id":"transition-grammar"}"#,
        ],
        &[
            "install-transition",
            "--action-json",
            begin_drain_over_600_s,
        ],
    ];
    let taken: &[&[&str]] = &[
        &["pause"],
        &[
            "pause",
            "--instance",
            ALIAS,
            "--reason",
            "maintenance",
            "--drain-timeout-ms",
            "1000",
        ],
        &["pause", "--drain-timeout-ms", "600000"],
        &["resume"],
        &["resume", "--instance", ALIAS],
        &["emulator", "status", "--instance", ALIAS],
        &["emulator", "start", "--instance", ALIAS],
        &["emulator", "stop", "--instance", ALIAS],
        &["emulator", "restart", "--instance", ALIAS],
        &["emulator", "discover"],
        &["selfcheck", ALIAS],
        &["install-transition", "--action-json", QUERY],
    ];
    let wrong = grammar_mismatches(usage, taken);
    assert!(
        wrong.is_empty(),
        "{} invocations do not end as their grammar requires:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// The empty-alias rows of the same grammar, apart so that what they show stands on its own.
/// Only a pause or resume without `--instance` is global (contracts/scheduling-pause.md "CLI"):
/// an `--instance` given with an empty or blank alias names no instance, so it is a usage error
/// before any connection (test plan v2.2 §3.7 G10; Fail Loud), never a request for the global
/// scope.
#[test]
fn gate_an_empty_instance_alias_is_a_usage_error_not_a_global_scope() {
    let usage: &[&[&str]] = &[
        &["pause", "--instance", ""],
        &["resume", "--instance", ""],
        &["pause", "--instance", " "],
        &["resume", "--instance", " "],
    ];
    let wrong = grammar_mismatches(usage, &[]);
    assert!(
        wrong.is_empty(),
        "{} invocations do not end as their grammar requires:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// The code of the outcome a refused request carries.
fn refusal<T>(result: Result<T, RuntimeClientError>) -> String {
    match result {
        Ok(_) => panic!("the Runtime takes the request"),
        Err(error) => error.outcome().code().as_str().to_owned(),
    }
}

#[test]
fn gate_a_refused_request_of_these_subcommands_exits_1_with_the_receipts_outcome() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(root.path(), HELD_SALT),
        Arc::new(NoInstances),
    )
    .expect("start the Runtime");
    // The same requests through the typed client: the outcome each refused receipt carries.
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect to the Runtime");
    let unknown = || SchedulingPauseScope::Instance {
        instance_alias: UNKNOWN.to_owned(),
    };
    let rows: [(&[&str], String); 5] = [
        (
            &["pause", "--instance", UNKNOWN],
            refusal(client.pause_scheduling(
                unknown(),
                SchedulingPauseReason::new("operator").expect("pause reason"),
                60_000,
            )),
        ),
        (
            &["resume", "--instance", UNKNOWN],
            refusal(client.resume_scheduling(unknown())),
        ),
        (
            &["emulator", "start", "--instance", UNKNOWN],
            refusal(client.control_emulator_instance(UNKNOWN, EmulatorInstanceAction::Start)),
        ),
        (
            &["selfcheck", UNKNOWN],
            refusal(client.self_check_instance(UNKNOWN)),
        ),
        (
            &["install-transition", "--action-json", QUERY],
            refusal(
                client.install_transition(serde_json::from_str(QUERY).expect("the query action")),
            ),
        ),
    ];
    drop(client);
    assert_eq!(rows[0].1, "instance_unknown", "pause of an unknown alias");
    assert_eq!(rows[1].1, "instance_unknown", "resume of an unknown alias");
    let outputs = rows
        .iter()
        .map(|(words, _)| actingctl(root.path(), words))
        .collect::<Vec<_>>();
    host.close().expect("close the Runtime");

    for ((words, code), output) in rows.iter().zip(&outputs) {
        let line = result_line(output).unwrap_or_else(|reason| panic!("{words:?}: {reason}"));
        assert_eq!(output.status.code(), Some(1), "{words:?}: {line}");
        assert_eq!(
            line.as_object()
                .map(|members| members.keys().map(String::as_str).collect::<Vec<_>>()),
            Some(vec!["outcome"]),
            "{words:?}: {line}"
        );
        assert_eq!(line["outcome"]["code"], code.as_str(), "{words:?}: {line}");
        assert_eq!(line["outcome"]["category"], "error", "{words:?}: {line}");
    }
}
