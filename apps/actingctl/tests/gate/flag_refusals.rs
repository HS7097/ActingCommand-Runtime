// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #382 A1-FLAG (test plan A-1 G10, the flag row; Fail Loud, #341 precedent): every
// actingctl subcommand refuses a flag it does not take, a flag given twice and a word that is
// neither a flag nor one of its positional words; nothing is ignored silently. The refusal is the
// one result line of Workflow #378 A5 whose outcome is `actingctl_usage_invalid` naming the word
// (`argument`) with `argument_problem` `unknown` or `repeated`, and the exit of an error, 1
// (model v2.2 §7.6: usage included). `mcp-serve` and `mcp-config` print no result line (§7.3):
// their developer line carries the same outcome.

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

// The word of the table that stands for the temporary state or install root.
const ROOT: &str = "{root}";
// A word no subcommand takes.
const STRAY: &str = "extra";
const PACKAGE_REF: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const QUERY: &str = r#"{"action":"query","transition_id":"transition-flags"}"#;

// Where a subcommand prints its refusal.
#[derive(Clone, Copy)]
enum Line {
    // The one stdout result line (Workflow #378 A5; the watchdog's report v2).
    Result,
    // The stderr developer line of `mcp-serve` and `mcp-config`.
    Developer,
}

struct Subcommand {
    // An invocation the subcommand takes.
    words: &'static [&'static str],
    // A flag actingctl takes elsewhere that this subcommand does not, with its value there.
    foreign: &'static [&'static str],
    // A flag of `words` given once more, with its value.
    again: &'static [&'static str],
    line: Line,
}

const fn result(
    words: &'static [&'static str],
    foreign: &'static [&'static str],
    again: &'static [&'static str],
) -> Subcommand {
    Subcommand {
        words,
        foreign,
        again,
        line: Line::Result,
    }
}

// The subcommands of every platform and build.
const CLIENT: &[Subcommand] = &[
    result(
        &[
            "agent-publish-facts",
            "--state-root",
            ROOT,
            "--record-file",
            "facts.json",
        ],
        &["--program"],
        &["--state-root", ROOT],
    ),
    result(
        &[
            "agent-apply-resource-targets",
            "--state-root",
            ROOT,
            "--policy-file",
            "policy.json",
        ],
        &["--instance", "neutral.instance"],
        &["--policy-file", "policy.json"],
    ),
    result(
        &[
            "task-offset",
            "neutral.task",
            "250",
            "--state-root",
            ROOT,
            "--instance",
            "neutral.instance",
        ],
        &["--reason", "operator"],
        &["--instance", "neutral.instance"],
    ),
    result(
        &["request-shutdown", "--state-root", ROOT, "--wait", "60"],
        &["--max-frames", "2"],
        &["--wait", "60"],
    ),
    result(
        &[
            "install-transition",
            "--state-root",
            ROOT,
            "--action-json",
            QUERY,
        ],
        &["--recover"],
        &["--action-json", QUERY],
    ),
    result(
        &[
            "observe",
            "--state-root",
            ROOT,
            "--instance",
            "neutral.instance",
        ],
        &["--interval-ms", "1000"],
        &["--instance", "neutral.instance"],
    ),
    result(
        &[
            "reset",
            "--state-root",
            ROOT,
            "--instance",
            "neutral.instance",
        ],
        &["--expect", "home"],
        &["--state-root", ROOT],
    ),
    result(
        &["status", "--state-root", ROOT, "--config"],
        &["--program"],
        &["--config"],
    ),
    result(
        &["config", "set", "neutral.key=1", "--state-root", ROOT],
        &["--instance", "neutral.instance"],
        &["--state-root", ROOT],
    ),
    result(
        &["config", "clear", "neutral.key", "--state-root", ROOT],
        &["--program"],
        &["--state-root", ROOT],
    ),
    result(
        &["facts", "--program", "--state-root", ROOT],
        &["--config"],
        &["--program"],
    ),
    // Workflow #389 MCP-4: `facts --instance` and `--key-prefix`.
    result(
        &[
            "facts",
            "--instance",
            "neutral.instance",
            "--key-prefix",
            "session.",
            "--state-root",
            ROOT,
        ],
        &["--config"],
        &["--key-prefix", "session."],
    ),
    result(
        &["monitor-status", "--state-root", ROOT],
        &["--recover"],
        &["--state-root", ROOT],
    ),
    result(
        &[
            "monitor-set",
            "--state-root",
            ROOT,
            "--instance",
            "neutral.instance",
            "--interval-ms",
            "1000",
            "--expect",
            "home",
            "--recover",
        ],
        &["--max-frames", "2"],
        &["--recover"],
    ),
    result(
        &[
            "monitor-clear",
            "--state-root",
            ROOT,
            "--instance",
            "neutral.instance",
        ],
        &["--expect", "home"],
        &["--instance", "neutral.instance"],
    ),
    result(
        &[
            "emulator",
            "status",
            "--state-root",
            ROOT,
            "--instance",
            "neutral.instance",
        ],
        &["--package", "neutral-task.zip"],
        &["--instance", "neutral.instance"],
    ),
    result(
        &[
            "emulator",
            "start",
            "--state-root",
            ROOT,
            "--instance",
            "neutral.instance",
        ],
        &["--package", "neutral-task.zip"],
        &["--instance", "neutral.instance"],
    ),
    result(
        &[
            "emulator",
            "stop",
            "--state-root",
            ROOT,
            "--instance",
            "neutral.instance",
        ],
        &["--package", "neutral-task.zip"],
        &["--instance", "neutral.instance"],
    ),
    result(
        &[
            "emulator",
            "restart",
            "--state-root",
            ROOT,
            "--instance",
            "neutral.instance",
        ],
        &["--package", "neutral-task.zip"],
        &["--instance", "neutral.instance"],
    ),
    result(
        &["emulator", "discover", "--state-root", ROOT],
        &["--instance", "neutral.instance"],
        &["--state-root", ROOT],
    ),
    result(
        &[
            "pause",
            "--state-root",
            ROOT,
            "--instance",
            "neutral.instance",
            "--reason",
            "operator",
            "--drain-timeout-ms",
            "60000",
        ],
        &["--wait", "60"],
        &["--reason", "operator"],
    ),
    result(
        &[
            "resume",
            "--state-root",
            ROOT,
            "--instance",
            "neutral.instance",
        ],
        &["--reason", "operator"],
        &["--instance", "neutral.instance"],
    ),
    result(
        &["selfcheck", "neutral.instance", "--state-root", ROOT],
        &["--interval-ms", "1000"],
        &["--state-root", ROOT],
    ),
    result(
        &[
            "stream",
            "--state-root",
            ROOT,
            "--instance",
            "neutral.instance",
            "--max-frames",
            "2",
            "--interval-ms",
            "250",
        ],
        &["--expect", "home"],
        &["--max-frames", "2"],
    ),
    result(
        &[
            "task-run",
            "--state-root",
            ROOT,
            "--instance",
            "neutral.instance",
            "--package",
            "neutral-task.zip",
            "--expected-sha256",
            PACKAGE_REF,
        ],
        &["--interval-ms", "1000"],
        &["--package", "neutral-task.zip"],
    ),
    result(
        &["events", "--state-root", ROOT, "--limit", "5"],
        &["--program"],
        &["--limit", "5"],
    ),
    result(
        &[
            "observe",
            "--state-root",
            ROOT,
            "--instance",
            "neutral.instance",
            "--export",
            ROOT,
        ],
        &["--interval-ms", "1000"],
        &["--export", ROOT],
    ),
    result(
        &["runs", "--state-root", ROOT, "--limit", "5"],
        &["--program"],
        &["--limit", "5"],
    ),
    result(
        &["diagnose", "--state-root", ROOT],
        &["--limit", "5"],
        &["--state-root", ROOT],
    ),
    // Workflow #389 MCP-8: `overview` and `--sections`.
    result(
        &["overview", "--state-root", ROOT, "--sections", "instances"],
        &["--limit", "5"],
        &["--sections", "instances"],
    ),
    result(
        &["packs", "--root", ROOT],
        &["--state-root", ROOT],
        &["--root", ROOT],
    ),
    // Workflow #389 MCP-14: the list flags of `codes`.
    result(
        &["codes", "--prefix", "outcome_", "--limit", "5"],
        &["--state-root", ROOT],
        &["--limit", "5"],
    ),
];

// `start`, `restart` and `watchdog` are built on Windows only.
#[cfg(windows)]
const WINDOWS: &[Subcommand] = &[
    result(
        &["start", "--root", ROOT],
        &["--wait-idle", "5"],
        &["--root", ROOT],
    ),
    result(
        &["restart", "--root", ROOT, "--wait-idle", "5"],
        &["--instance", "neutral.instance"],
        &["--wait-idle", "5"],
    ),
    result(
        &["watchdog", "status", "--root", ROOT],
        &["--from-task"],
        &["--root", ROOT],
    ),
    result(
        &["watchdog", "run-once", "--root", ROOT, "--from-task"],
        &["--state-root", ROOT],
        &["--from-task"],
    ),
    result(
        &["watchdog", "install", "--root", ROOT],
        &["--from-task"],
        &["--root", ROOT],
    ),
    result(
        &["watchdog", "uninstall", "--root", ROOT],
        &["--from-task"],
        &["--root", ROOT],
    ),
];

// `mcp-serve` and `mcp-config` are built with the `mcp` feature only.
#[cfg(feature = "mcp")]
const MCP: &[Subcommand] = &[
    Subcommand {
        words: &["mcp-serve", "--root", ROOT],
        foreign: &["--instance", "neutral.instance"],
        again: &["--root", ROOT],
        line: Line::Developer,
    },
    Subcommand {
        words: &["mcp-config", "--client", "claude"],
        foreign: &["--root", ROOT],
        again: &["--client", "claude"],
        line: Line::Developer,
    },
];

fn subcommands() -> Vec<&'static Subcommand> {
    #[allow(unused_mut)]
    let mut subcommands = CLIENT.iter().collect::<Vec<_>>();
    #[cfg(windows)]
    subcommands.extend(WINDOWS);
    #[cfg(feature = "mcp")]
    subcommands.extend(MCP);
    subcommands
}

fn run(root: &Path, words: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .args(words.iter().map(|word| {
            if *word == ROOT {
                root.as_os_str()
            } else {
                OsStr::new(word)
            }
        }))
        .env_remove("ACTINGCOMMAND_DETAIL")
        .stdin(Stdio::null())
        .output()
        .expect("run actingctl")
}

// The refusal as `(code, argument, argument_problem)`, read from the line the subcommand prints
// it on; `Err` says what is not a refusal line.
fn refusal(output: &Output, line: Line) -> Result<[Option<String>; 3], String> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    match line {
        Line::Result => {
            let mut lines = stdout.lines();
            let (Some(first), None) = (lines.next(), lines.next()) else {
                return Err(format!(
                    "not one stdout line: {stdout:?}; stderr: {stderr:?}"
                ));
            };
            let parsed: serde_json::Value = serde_json::from_str(first)
                .map_err(|error| format!("stdout is not JSON ({error}): {first}"))?;
            let outcome = &parsed["outcome"];
            let text = |value: &serde_json::Value| value.as_str().map(str::to_owned);
            Ok([
                text(&outcome["code"]),
                text(&outcome["values"]["argument"]),
                text(&outcome["values"]["argument_problem"]),
            ])
        }
        // `ERROR actingctl: <code> <key>=<value>… <usage>`.
        Line::Developer => {
            if !stdout.is_empty() {
                return Err(format!("stdout is not empty: {stdout:?}"));
            }
            let first = stderr.lines().next().unwrap_or_default();
            let mut words = first.split_whitespace();
            if (words.next(), words.next()) != (Some("ERROR"), Some("actingctl:")) {
                return Err(format!("no developer line of an error: {stderr:?}"));
            }
            let code = words.next().map(str::to_owned);
            let rest = words.collect::<Vec<_>>();
            let value = |key: &str| {
                rest.iter()
                    .find_map(|word| word.strip_prefix(key)?.strip_prefix('='))
                    .map(str::to_owned)
            };
            Ok([code, value("argument"), value("argument_problem")])
        }
    }
}

// What is wrong with the refusal of `words`, or `None` when it is the required one.
fn mismatch(
    root: &Path,
    subcommand: &Subcommand,
    words: &[&str],
    argument: &str,
    problem: &str,
) -> Option<String> {
    let output = run(root, words);
    let required = [
        Some("actingctl_usage_invalid".to_owned()),
        Some(argument.to_owned()),
        Some(problem.to_owned()),
    ];
    let found = match refusal(&output, subcommand.line) {
        Ok(found) => found,
        Err(reason) => return Some(format!("{words:?}: exit {}, {reason}", output.status)),
    };
    (output.status.code() != Some(1) || found != required).then(|| {
        format!(
            "{words:?}: exit {}, (code, argument, argument_problem) {found:?}, required {required:?}",
            output.status
        )
    })
}

fn assert_all_refused(wrong: &[String]) {
    assert!(
        wrong.is_empty(),
        "{} invocations are not refused as required:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn gate_every_subcommand_refuses_a_flag_it_does_not_take_naming_it() {
    let root = TempDir::new().expect("tempdir");
    let wrong = subcommands()
        .into_iter()
        .filter_map(|subcommand| {
            let words = [subcommand.words, subcommand.foreign].concat();
            mismatch(
                root.path(),
                subcommand,
                &words,
                subcommand.foreign[0],
                "unknown",
            )
        })
        .collect::<Vec<_>>();
    assert_all_refused(&wrong);
}

#[test]
fn gate_every_subcommand_refuses_a_flag_given_twice_naming_it() {
    let root = TempDir::new().expect("tempdir");
    let wrong = subcommands()
        .into_iter()
        .filter_map(|subcommand| {
            let words = [subcommand.words, subcommand.again].concat();
            mismatch(
                root.path(),
                subcommand,
                &words,
                subcommand.again[0],
                "repeated",
            )
        })
        .collect::<Vec<_>>();
    assert_all_refused(&wrong);
}

#[test]
fn gate_every_subcommand_refuses_a_stray_word_naming_it() {
    let root = TempDir::new().expect("tempdir");
    let stray: &[&str] = &[STRAY];
    let wrong = subcommands()
        .into_iter()
        .filter_map(|subcommand| {
            let words = [subcommand.words, stray].concat();
            mismatch(root.path(), subcommand, &words, STRAY, "unknown")
        })
        .collect::<Vec<_>>();
    assert_all_refused(&wrong);
}
