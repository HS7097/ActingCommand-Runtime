// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-5B5 (register §1.287, "Slice LAB-5B5"; coordinator work order
// rd-1108/arch-013/brief-lab5b5.md): the rest of actinglab's own commands, those that read their
// flags by name and ignored the rest, parse their command line by a read list, with LAB-5B4's three
// rules and its refusal.
// - `the_census_puts_every_inventory_word_in_one_group`: every command of the inventory
//   (`ratchet/actinglab_commands.json`) is in exactly one group of the register's census
//   (`own_flags::REFUSING_COMMANDS`, `TABLE_COMMANDS`, `WORK_LIST`, `NO_WORD_COMMANDS`), each
//   group's word is a command of the inventory, and each command of the work list has a read list;
//   each command of group (d) refuses its line, a flag it does not read on it too, with its own code.
// - `every_command_of_the_work_list_refuses_a_flag_it_does_not_read`: each command of the work
//   list, through every read list it is parsed by, refuses `--frobnicate` before anything runs,
//   with the line, the exit code and the developer line the CLI module gives a strict row's
//   undeclared flag (`watch --frobnicate`) but for the command's name; no read of its flags, no
//   Runtime, nothing written.
// - `the_switch_and_positional_rules_on_stream_and_monitor_policy_set`: on `stream` and `session
//   monitor-policy set`, a switch followed by a word that is not `true` or `false` leaves it a
//   positional, which each refuses (it reads none there), and takes `true` or `false` as its value;
//   `stream`'s input relay takes every word after it; the flags `monitor-policy set`'s scan let
//   through unread are refused.
// - `lines_that_pass_only_what_their_command_reads_answer_as_before`: a corpus of lines of the work
//   list's commands that pass only flags and words their command reads answer the exit and error
//   codes they answered on main, none refused by the new parse.
// - `the_reads_that_rest_on_reading_are_on_their_lists`: the reads no line of a test reaches
//   without a Runtime, a package or the Lab's state folder parse by their lists in the forms their
//   commands read them.
// - `serve_and_the_via_child_hand_the_work_list_only_what_it_reads`: the rows of the operation
//   table whose command is one of the work list (`app launch|restart|stop`, `user-config`, `user-
//   config set`) hand it, with every parameter they declare, only words its list holds; `serve
//   --stdio` answers `user-config` as the command line does; MCP's `--via mcp` child's `session app`
//   lines (`lab_ops::apps`) parse by the list.
// No Runtime, no device: temporary files, the embedded operation table and the code's lists.

use super::env_lock;
use super::gate_434_lab5b4::{Isolated, developer, printed};
use crate::cli_module::{self, engine, own_flags, serve};
use crate::flag_args::FlagArgs;
use crate::flag_args::reads::{self, FlagReads};
use crate::{
    cli_information, env_detection, lab_package_control, parse_invocation, run_cli,
    runtime_session_adapter, runtime_stream_adapter, session_management,
};
use actingcommand_lab_ops::apps::{self, ACTIONS, App};
use actingcommand_lab_ops::lab_child::Actinglab;
use actingcommand_lab_ops::table::{ParamForm, ParamType};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// A command of group (d): its words, and the code it refuses every line with.
type Refusing = (&'static [&'static str], &'static str);

/// A line of [`LINES`]: the command of the work list, its read list's row, the words.
type Line = (&'static str, &'static str, &'static [&'static str]);

/// A line of [`corpus`]: the name a refusal by the read list would give, the line, the exit code
/// and the error code it answered on main.
type CorpusLine = (&'static str, Vec<&'static str>, i32, &'static str);

/// A refused line of the switch test: the line, its command's name and the word it refuses.
type RefusedLine = (&'static [&'static str], &'static str, &'static str);

fn owned(line: &[&str]) -> Vec<String> {
    line.iter().map(|word| (*word).to_owned()).collect()
}

/// The commands of the inventory, `execute`'s, which the architecture guard keeps equal to it.
fn inventory() -> Vec<String> {
    let snapshot: Value = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ratchet/actinglab_commands.json"),
        )
        .expect("the command inventory"),
    )
    .expect("the command inventory parses");
    snapshot["commands"]
        .as_array()
        .expect("its commands")
        .iter()
        .map(|command| command.as_str().expect("a command").to_owned())
        .collect()
}

#[test]
fn the_census_puts_every_inventory_word_in_one_group() {
    let inventory = inventory();
    let groups: [(&str, Vec<&str>); 4] = [
        ("(a)", own_flags::REFUSING_COMMANDS.to_vec()),
        ("(b)", own_flags::TABLE_COMMANDS.to_vec()),
        (
            "(c)",
            own_flags::WORK_LIST
                .iter()
                .map(|(command, _)| *command)
                .collect(),
        ),
        ("(d)", own_flags::NO_WORD_COMMANDS.to_vec()),
    ];
    let mut failures = Vec::new();
    for command in &inventory {
        let found = groups
            .iter()
            .filter(|(_, words)| words.contains(&command.as_str()))
            .map(|(group, _)| *group)
            .collect::<Vec<_>>();
        if found.len() != 1 {
            failures.push(format!(
                "{command}: in the groups {found:?}, not in exactly one"
            ));
        }
    }
    let inventory_words = inventory.iter().map(String::as_str).collect::<Vec<_>>();
    for (group, words) in &groups {
        for word in words {
            if !inventory_words.contains(word) {
                failures.push(format!("{word}: in {group}, no command of the inventory"));
            }
        }
    }
    let counted = groups.iter().map(|(_, words)| words.len()).sum::<usize>();
    if counted != inventory.len() {
        failures.push(format!(
            "the groups count {counted} commands, the inventory {}",
            inventory.len()
        ));
    }
    for (command, lists) in own_flags::WORK_LIST {
        if lists.is_empty() {
            failures.push(format!("{command}: in the work list without a read list"));
        }
    }
    assert!(
        failures.is_empty(),
        "the census does not hold:\n{}",
        failures.join("\n")
    );

    // Group (d): every line refused, the flag it does not read on it too, with the command's own
    // code; nothing reaches a Runtime.
    let _guard = env_lock();
    let isolated = Isolated::new();
    let refusing: [Refusing; 3] = [
        (&["lab", "arbitrator"], "legacy_lab2_arbitrator_retired"),
        (&["control", "export"], "not_implemented"),
        (&["operation", "run"], "lab_lease_required"),
    ];
    assert_eq!(
        refusing.map(|(words, _)| words.join(" ")),
        own_flags::NO_WORD_COMMANDS.map(str::to_owned)
    );
    for (words, code) in refusing {
        for more in [&[][..], &["--frobnicate", "x"][..]] {
            let line = std::iter::once("--json")
                .chain(words.iter().copied())
                .chain(more.iter().copied());
            let result = run_cli(line, true);
            let answered = result
                .envelope
                .error
                .as_ref()
                .map(|error| error.code.as_str());
            assert_eq!(
                answered,
                Some(code),
                "{words:?} {more:?}: {}",
                result.envelope_json()
            );
        }
    }
    reads::take();
    assert!(!isolated.runtime.exists(), "no command reached the Runtime");
}

/// The lines of the next test: the command of the work list, the name its refusal gives it (its
/// read list's row) and the words of a command line that reaches that list. Workflow #467 RPT-1
/// (adapted): `report export`'s placeholder left the work list for the table's row.
const LINES: [Line; 41] = [
    ("help", "help", &["help"]),
    ("version", "version", &["version"]),
    ("paths", "paths", &["paths"]),
    ("capabilities", "capabilities", &["capabilities"]),
    ("doctor", "doctor", &["doctor"]),
    ("list", "list", &["list"]),
    ("detect", "detect", &["detect"]),
    ("stream", "stream", &["stream"]),
    ("explain", "explain", &["explain"]),
    (
        "user-config get",
        "user-config get",
        &["user-config", "get"],
    ),
    (
        "user-config set",
        "user-config set",
        &["user-config", "set"],
    ),
    ("env resolve", "env resolve", &["env", "resolve"]),
    ("env status", "env status", &["env", "status"]),
    ("lab run", "lab run", &["lab", "run"]),
    ("lab validate", "lab validate", &["lab", "validate"]),
    (
        "lab replay-evidence",
        "lab replay-evidence",
        &["lab", "replay-evidence"],
    ),
    ("lab status", "session status", &["lab", "status"]),
    (
        "lab vendor-stdio-selftest",
        "lab vendor-stdio-selftest",
        &["lab", "vendor-stdio-selftest"],
    ),
    ("package run", "package run", &["package", "run"]),
    (
        "control inspect",
        "control inspect",
        &["control", "inspect"],
    ),
    ("control verify", "control verify", &["control", "verify"]),
    (
        "control probe-click",
        "control probe-click",
        &["control", "probe-click"],
    ),
    ("control diff", "control diff", &["control", "diff"]),
    ("session status", "session status", &["session", "status"]),
    (
        "session throat-policy",
        "session throat-policy",
        &["session", "throat-policy"],
    ),
    (
        "session capture-policy",
        "session capture-policy",
        &["session", "capture-policy"],
    ),
    (
        "session record-policy",
        "session record-policy",
        &["session", "record-policy"],
    ),
    (
        "session self-heal-policy",
        "session self-heal-policy",
        &["session", "self-heal-policy"],
    ),
    (
        "session contract",
        "session contract",
        &["session", "contract"],
    ),
    ("session api", "session api", &["session", "api"]),
    (
        "session transport",
        "session transport",
        &["session", "transport"],
    ),
    (
        "session monitor-policy",
        "session monitor-policy status",
        &["session", "monitor-policy", "status"],
    ),
    (
        "session monitor-policy",
        "session monitor-policy set",
        &["session", "monitor-policy", "set"],
    ),
    (
        "session monitor-policy",
        "session monitor-policy clear",
        &["session", "monitor-policy", "clear"],
    ),
    (
        "session instance",
        "session instance",
        &["session", "instance", "list"],
    ),
    (
        "session instance",
        "session app",
        &["session", "instance", "app", "launch"],
    ),
    ("session app", "session app", &["session", "app", "launch"]),
    ("session stream", "stream", &["session", "stream"]),
    ("run open", "run open", &["run", "open"]),
    ("run export", "run export", &["run", "export"]),
    ("session stream", "stream", &["session", "stream", "check"]),
];

#[test]
fn every_command_of_the_work_list_refuses_a_flag_it_does_not_read() {
    // The lines reach every command of the work list, through every read list it is parsed by.
    let commands = LINES
        .iter()
        .map(|(command, _, _)| *command)
        .collect::<BTreeSet<_>>();
    let work = own_flags::WORK_LIST
        .iter()
        .map(|(command, _)| *command)
        .collect::<BTreeSet<_>>();
    assert_eq!(commands, work);
    for (command, lists) in own_flags::WORK_LIST {
        let named = LINES
            .iter()
            .filter(|(line_command, _, _)| *line_command == command)
            .map(|(_, name, _)| *name)
            .collect::<BTreeSet<_>>();
        let rows = lists
            .iter()
            .map(|list: &&FlagReads| list.row)
            .collect::<BTreeSet<_>>();
        assert_eq!(named, rows, "{command}");
    }

    let _guard = env_lock();
    let isolated = Isolated::new();
    // The refusal the CLI module gives a strict row's undeclared flag, as actinglab prints it.
    let strict = printed(&["--json", "watch", "--frobnicate", "x"]);
    let strict_line = strict.line.clone().expect("the strict row's line");
    assert_eq!(strict_line["message"], "watch: unknown --frobnicate");
    let unknown = json!({"argument": "--frobnicate", "argument_problem": "unknown"});
    assert_eq!(strict_line["details"], unknown);
    let strict_developer = developer(&strict).expect("the strict row's developer line");

    let mut failures = Vec::new();
    reads::take();
    for (command, name, words) in LINES {
        let mut line = vec!["--json"];
        line.extend(words);
        line.extend(["--frobnicate", "x"]);
        let exit = printed(&line);
        let recorded = reads::take();
        let Ok(answered) = exit.line.as_ref() else {
            failures.push(format!("{command}: {line:?} printed no line"));
            continue;
        };
        let message = format!("{name}: unknown --frobnicate");
        let expected_developer = strict_developer.replacen("watch: ", &format!("{name}: "), 1);
        if exit.code != strict.code
            || answered["outcome"] != strict_line["outcome"]
            || answered["details"] != unknown
            || answered["message"] != message.as_str()
            || developer(&exit).as_deref() != Some(expected_developer.as_str())
        {
            failures.push(format!(
                "{command}: {line:?} answered {answered} (exit {}, {:?}), not the strict refusal \
                 {strict_line} for {name}",
                exit.code,
                developer(&exit)
            ));
        }
        if !recorded.is_empty() {
            failures.push(format!("{command}: {line:?} read its flags: {recorded:?}"));
        }
    }
    reads::take();
    assert!(!isolated.runtime.exists(), "no command reached the Runtime");
    let written = fs::read_dir(isolated.temp.path())
        .expect("the temporary folder")
        .count();
    assert_eq!(written, 0, "no command wrote a file");
    assert!(
        failures.is_empty(),
        "commands of the work list that do not refuse a flag they do not read:\n{}",
        failures.join("\n")
    );
}

/// The refusal of `line` by `list`: its message and its details.
fn refusal(line: &[&str], list: &'static FlagReads) -> (String, Value) {
    let error = FlagArgs::parse_reading(&owned(line), list).expect_err("refused");
    (error.message, error.details.unwrap_or(Value::Null))
}

#[test]
fn the_switch_and_positional_rules_on_stream_and_monitor_policy_set() {
    let stream = &runtime_stream_adapter::STREAM_READS;
    // `stream`: a switch followed by a word that is not `true` or `false` leaves it a positional,
    // and takes `true` or `false` as its value.
    let flags = FlagArgs::parse_reading(&owned(&["check", "--require-fresh", "word"]), stream)
        .expect("stream's words");
    assert!(flags.bool("--require-fresh"));
    assert_eq!(flags.positionals, ["check", "word"]);
    let flags = FlagArgs::parse_reading(&owned(&["check", "--require-fresh", "false"]), stream)
        .expect("stream's words");
    assert!(!flags.bool("--require-fresh"));
    assert_eq!(flags.positionals, ["check"]);
    // Its input relay: the option's value is the action, every word after it the action's.
    let line = owned(&["--input-relay", "tap", "10", "20"]);
    let flags = FlagArgs::parse_reading(&line, stream).expect("the relay's words");
    assert_eq!(flags.optional("--input-relay").as_deref(), Some("tap"));
    assert_eq!(flags.positionals, ["10", "20"]);
    reads::take();

    // `session monitor-policy set`: its switches likewise; it reads no positional after its
    // action, so the word left is refused naming it.
    let set = &runtime_session_adapter::MONITOR_POLICY_SET_READS;
    let line = owned(&[
        "--recover",
        "false",
        "--capture",
        "true",
        "--expect",
        "home",
    ]);
    let flags = FlagArgs::parse_reading(&line, set).expect("set's flags");
    assert!(!flags.bool("--recover"));
    assert!(flags.bool("--capture"));
    assert_eq!(flags.optional("--expect").as_deref(), Some("home"));
    assert!(flags.positionals.is_empty());
    let refused = |word: &str| {
        (
            format!("session monitor-policy set: unknown {word}"),
            json!({"argument": word, "argument_problem": "unknown"}),
        )
    };
    assert_eq!(refusal(&["--capture", "word"], set), refused("word"));
    reads::take();

    // On the command line: `stream` refuses a word that is no relay's and no first `check`, and
    // `monitor-policy set` the word after its switch and the flags its scan let through unread.
    let _guard = env_lock();
    let isolated = Isolated::new();
    let lines: [RefusedLine; 6] = [
        (
            &["--json", "stream", "check", "--require-fresh", "word"],
            "stream",
            "word",
        ),
        (&["--json", "stream", "extra"], "stream", "extra"),
        (
            &["--json", "session", "stream", "check", "extra"],
            "stream",
            "extra",
        ),
        (
            &[
                "--json",
                "session",
                "monitor-policy",
                "set",
                "--capture",
                "word",
            ],
            "session monitor-policy set",
            "word",
        ),
        (
            &[
                "--json",
                "session",
                "monitor-policy",
                "set",
                "--capture",
                "--holder",
                "h",
            ],
            "session monitor-policy set",
            "--holder",
        ),
        (
            &[
                "--json",
                "session",
                "monitor-policy",
                "set",
                "--scene",
                "frame.png",
                "--request-timeout-ms",
                "5",
            ],
            "session monitor-policy set",
            "--request-timeout-ms",
        ),
    ];
    for (line, name, word) in lines {
        let exit = printed(line);
        let answered = exit.line.as_ref().expect("the command's line");
        assert_eq!(exit.code, 1, "{line:?}: {answered}");
        assert_eq!(answered["outcome"]["code"], "validation_failed", "{line:?}");
        assert_eq!(
            answered["message"],
            format!("{name}: unknown {word}"),
            "{line:?}"
        );
        assert_eq!(
            answered["details"],
            json!({"argument": word, "argument_problem": "unknown"}),
            "{line:?}"
        );
    }
    reads::take();
    assert!(!isolated.runtime.exists(), "no command reached the Runtime");
}

/// The lines of the next test: the name a refusal by the command's read list would give, the line,
/// and the exit code and the error code it answered on main. Each passes only flags and words its
/// command reads, with values where options need them. `lab status`, `session status` and
/// `session monitor-policy status` are not among them: each of their lines reads the Runtime (the
/// integration tests run them against one); nor is `lab vendor-stdio-selftest`, whose line runs the
/// vendor stdio diagnostic.
fn corpus() -> Vec<CorpusLine> {
    let instance = ["--instance", "neutral.a"];
    let with_instance = |words: &[&'static str]| -> Vec<&'static str> {
        instance.iter().chain(words).copied().collect()
    };
    let dry_run_instance = |words: &[&'static str]| -> Vec<&'static str> {
        std::iter::once("--dry-run")
            .chain(instance)
            .chain(words.iter().copied())
            .collect()
    };
    vec![
        ("help", vec!["help"], 0, ""),
        ("version", vec!["version"], 0, ""),
        // The global `--version` makes the line a version line, whatever its words.
        ("version", vec!["capture", "--version"], 0, ""),
        ("paths", vec!["paths"], 0, ""),
        ("capabilities", vec!["capabilities"], 0, ""),
        ("doctor", vec!["doctor"], 0, ""),
        ("list", vec!["list", "commands"], 0, ""),
        (
            "detect",
            vec!["detect", "--task", "t"],
            2,
            "validation_failed",
        ),
        ("stream", with_instance(&["stream", "check"]), 0, ""),
        (
            "stream",
            dry_run_instance(&["stream", "--input-relay", "tap", "10", "20"]),
            0,
            "",
        ),
        (
            "stream",
            with_instance(&["session", "stream", "check", "--max-frames", "2"]),
            0,
            "",
        ),
        ("explain", vec!["explain", "run-1"], 0, ""),
        ("user-config get", vec!["user-config", "get"], 0, ""),
        (
            "user-config get",
            vec!["user-config", "get", "adb_path"],
            0,
            "",
        ),
        (
            "user-config set",
            vec!["user-config", "set"],
            2,
            "validation_failed",
        ),
        (
            "env resolve",
            vec!["env", "resolve", "--task", "t", "--key", "k"],
            2,
            "validation_failed",
        ),
        (
            "env status",
            vec!["env", "status", "--task", "t"],
            2,
            "validation_failed",
        ),
        ("lab run", vec!["lab", "run"], 2, "package_invalid"),
        (
            "lab validate",
            vec!["lab", "validate"],
            2,
            "validation_failed",
        ),
        (
            "lab replay-evidence",
            vec!["lab", "replay-evidence"],
            2,
            "validation_failed",
        ),
        (
            "package run",
            vec!["package", "run"],
            2,
            "validation_failed",
        ),
        (
            "control inspect",
            vec!["control", "inspect", "--control", "c"],
            0,
            "",
        ),
        (
            "control verify",
            vec!["control", "verify"],
            2,
            "validation_failed",
        ),
        (
            "control probe-click",
            vec!["control", "probe-click", "--effect", "navigation_only"],
            3,
            "unresolved_coords",
        ),
        (
            "control diff",
            vec!["control", "diff"],
            2,
            "validation_failed",
        ),
        (
            "session throat-policy",
            vec!["session", "throat-policy"],
            0,
            "",
        ),
        (
            "session capture-policy",
            vec!["session", "capture-policy"],
            0,
            "",
        ),
        (
            "session record-policy",
            vec!["session", "record-policy"],
            0,
            "",
        ),
        (
            "session self-heal-policy",
            vec!["session", "self-heal-policy"],
            0,
            "",
        ),
        ("session contract", vec!["session", "contract"], 0, ""),
        ("session api", vec!["session", "api"], 0, ""),
        ("session transport", vec!["session", "transport"], 0, ""),
        (
            "session transport",
            vec!["session", "transport", "plan", "--endpoint", "127.0.0.1:1"],
            6,
            "trusted_remote_transport_retired",
        ),
        (
            "session monitor-policy set",
            vec![
                "--dry-run",
                "session",
                "monitor-policy",
                "set",
                "--capture",
                "--expect",
                "home",
            ],
            2,
            "dry_run_unsupported",
        ),
        (
            "session monitor-policy clear",
            vec!["--dry-run", "session", "monitor-policy", "clear"],
            2,
            "dry_run_unsupported",
        ),
        (
            "session instance",
            vec!["session", "instance", "connect"],
            6,
            "actinglab_device_authority_retired",
        ),
        (
            "session app",
            dry_run_instance(&["session", "app", "launch"]),
            2,
            "dry_run_unsupported",
        ),
        (
            "session app",
            dry_run_instance(&["session", "instance", "app", "restart"]),
            2,
            "dry_run_unsupported",
        ),
        ("run open", vec!["run", "open", "run-1"], 0, ""),
        (
            "run export",
            vec!["run", "export", "run-1"],
            2,
            "validation_failed",
        ),
        // Lines that reach the reads after a command's first refusal; the paths name no file.
        // Workflow #467 RPT-1 (adapted): `report export`'s two placeholder lines are gone with it.
        (
            "lab run",
            vec![
                "lab",
                "run",
                "--package",
                "lab5b5-missing-package",
                "--out",
                "lab5b5-missing-out.zip",
            ],
            4,
            "instance_not_found",
        ),
        (
            "lab replay-evidence",
            vec!["lab", "replay-evidence", "--zip", "lab5b5-missing.zip"],
            2,
            "validation_failed",
        ),
        (
            "control verify",
            vec![
                "control",
                "verify",
                "--candidate",
                "lab5b5-missing.json",
                "--candidate-id",
                "c",
            ],
            2,
            "validation_failed",
        ),
        (
            "control probe-click",
            vec![
                "control",
                "probe-click",
                "--effect",
                "navigation_only",
                "--expect-before",
                "a",
            ],
            3,
            "unresolved_coords",
        ),
        (
            "control diff",
            vec![
                "control",
                "diff",
                "--candidate",
                "lab5b5-missing-a.json",
                "--stable",
                "lab5b5-missing-b.json",
            ],
            2,
            "validation_failed",
        ),
    ]
}

#[test]
fn the_reads_that_rest_on_reading_are_on_their_lists() {
    // The reads no line of a test reaches without a Runtime, a package or the Lab's state folder
    // (register, "Slice LAB-5B5"): each list's line of them parses, and the reads in the forms
    // their commands read them pass under the recording (it fails a read its list does not give).
    let scope = [
        "--resource-root",
        "root",
        "--game",
        "neutral",
        "--instance",
        "neutral.a",
        "--server",
        "one",
    ];
    let read_scope = |flags: &FlagArgs| {
        assert!(flags.optional_path("--resource-root").is_some());
        for option in ["--game", "--instance", "--server"] {
            assert!(flags.optional(option).is_some(), "{option}");
        }
    };
    let line = |more: &[&str]| owned(&scope.iter().chain(more).copied().collect::<Vec<_>>());

    // `detect`, past its scope: the task, the frame and its freshness, `--dry-run`.
    let words = line(&[
        "--task",
        "t",
        "--scene",
        "frame.png",
        "--fresh-delay-ms",
        "10",
        "--require-fresh",
        "--dry-run",
    ]);
    let flags = FlagArgs::parse_reading(&words, &env_detection::DETECT_READS).expect("detect");
    read_scope(&flags);
    assert!(!flags.bool("--capture"));
    assert_eq!(flags.required("--task").ok().as_deref(), Some("t"));
    assert!(flags.optional_path("--scene").is_some());
    assert_eq!(flags.optional("--fresh-delay-ms").as_deref(), Some("10"));
    assert!(flags.bool("--require-fresh"));
    assert!(flags.bool("--dry-run"));

    // `env resolve` and `env status`, past their scope.
    let words = line(&[
        "input", "--task", "t", "--path", "p", "--value", "v", "--key", "k",
    ]);
    let flags =
        FlagArgs::parse_reading(&words, &env_detection::ENV_RESOLVE_READS).expect("env resolve");
    read_scope(&flags);
    assert!(flags.required("--task").is_ok());
    for option in ["--path", "--value", "--key"] {
        assert!(flags.optional(option).is_some(), "{option}");
    }
    assert_eq!(flags.positionals, ["input"]);
    let words = line(&["--task", "t"]);
    let flags =
        FlagArgs::parse_reading(&words, &env_detection::ENV_STATUS_READS).expect("env status");
    read_scope(&flags);
    assert!(flags.required("--task").is_ok());
    reads::take();

    // `package run`'s result file, read after its package is: the package is needed first.
    let words = owned(&["--zip", "p.zip", "--out", "r.zip"]);
    let flags = FlagArgs::parse_reading(&words, &lab_package_control::PACKAGE_RUN_READS)
        .expect("package run");
    assert!(flags.required_path("--zip").is_ok());
    assert!(flags.optional_path("--out").is_some());

    // `session monitor-policy`, whose every action connects to the Runtime after its reads.
    let instance = owned(&["--instance", "neutral.a"]);
    for list in [
        &runtime_session_adapter::MONITOR_POLICY_STATUS_READS,
        &runtime_session_adapter::MONITOR_POLICY_CLEAR_READS,
    ] {
        let flags = FlagArgs::parse_reading(&instance, list).expect(list.row);
        assert!(!flags.bool("--via-daemon"), "{}", list.row);
        assert!(flags.optional("--state-dir").is_none(), "{}", list.row);
        assert_eq!(flags.optional("--instance").as_deref(), Some("neutral.a"));
    }
    let words = owned(&[
        "--instance",
        "neutral.a",
        "--interval-ms",
        "60000",
        "--expect",
        "home",
        "--to",
        "home",
        "--scene",
        "frame.png",
        "--recover",
        "--capture",
    ]);
    let set = &runtime_session_adapter::MONITOR_POLICY_SET_READS;
    let flags = FlagArgs::parse_reading(&words, set).expect("monitor-policy set");
    assert!(flags.optional("--max-iterations").is_none());
    assert!(!flags.bool("--local"));
    for option in ["--instance", "--interval-ms", "--expect", "--to", "--scene"] {
        assert!(flags.optional(option).is_some(), "{option}");
    }
    assert!(flags.bool("--recover"));
    assert!(flags.bool("--capture"));
    reads::take();
}

#[test]
fn lines_that_pass_only_what_their_command_reads_answer_as_before() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let mut failures = Vec::new();
    for (name, line, exit_code, code) in corpus() {
        let result = run_cli(std::iter::once("--json").chain(line.iter().copied()), true);
        let error = result.envelope.error.as_ref();
        let answered = (
            result.exit_code(),
            error.map_or("", |error| error.code.as_str()),
        );
        let refused_by_the_parse = error.is_some_and(|error| {
            ["unknown", "value_missing"]
                .iter()
                .any(|problem| error.message.starts_with(&format!("{name}: {problem} ")))
        });
        if answered != (exit_code, code) || refused_by_the_parse {
            failures.push(format!(
                "{name}: {line:?} answered {answered:?}, not ({exit_code}, {code}) as before: {}",
                result.envelope_json()
            ));
        }
    }
    reads::take();
    assert!(
        !isolated.runtime.exists(),
        "no command line reached the Runtime"
    );
    assert!(
        failures.is_empty(),
        "lines that pass only what their command reads and answer otherwise:\n{}",
        failures.join("\n")
    );
}

/// A value of the parameter of `form` and `value_type`, as the test hands every declared one.
fn sample(form: ParamForm, value_type: &ParamType) -> Value {
    match (form, value_type) {
        (_, ParamType::Instance) => json!("neutral.a"),
        (ParamForm::Switch, _) => json!(true),
        _ => json!("x"),
    }
}

#[test]
fn serve_and_the_via_child_hand_the_work_list_only_what_it_reads() {
    let table = cli_module::table().expect("the operation table");
    let work = own_flags::WORK_LIST
        .iter()
        .map(|(command, _)| *command)
        .collect::<Vec<_>>();
    // Every row whose command is one of the work list, with every parameter it declares: the line
    // the engine hands its command (`serve --stdio`'s, and the short words' on the command line).
    let mut served = Vec::new();
    for (name, words, _) in engine::COMMANDS {
        let command = parse_invocation(words.iter().copied(), true)
            .expect("the row's command words parse")
            .command
            .join(" ");
        if !work.contains(&command.as_str()) {
            continue;
        }
        let operation = table
            .operation(name)
            .unwrap_or_else(|| panic!("{name} is a row of the table"));
        let params = operation
            .params
            .iter()
            .map(|param| (param.name.clone(), sample(param.form, &param.value_type)))
            .collect::<Map<_, _>>();
        let line = engine::arguments(operation, &params).expect("the row's command line");
        let invocation = parse_invocation(line, true).expect("the row's line parses");
        assert_eq!(invocation.command.join(" "), command, "{name}");
        served.push((name, command, invocation.args));
    }
    assert_eq!(
        served.iter().map(|(name, _, _)| *name).collect::<Vec<_>>(),
        [
            "app launch",
            "app restart",
            "app stop",
            "user-config",
            "user-config set"
        ]
    );
    for (name, command, args) in &served {
        let parsed = match command.as_str() {
            // `session app` reads its action first, then its flags by its list.
            "session app" => FlagArgs::parse_reading(
                args.get(1..).unwrap_or_default(),
                &session_management::SESSION_APP_READS,
            ),
            "user-config get" => {
                FlagArgs::parse_reading(args, &cli_information::USER_CONFIG_GET_READS)
            }
            "user-config set" => {
                FlagArgs::parse_reading(args, &cli_information::USER_CONFIG_SET_READS)
            }
            other => panic!("{name}: {other} is no command this test reads"),
        };
        assert!(parsed.is_ok(), "{name}: {args:?}: {parsed:?}");
    }
    reads::take();

    // `serve --stdio` answers `user-config` as the command line answers the same words.
    let _guard = env_lock();
    let isolated = Isolated::new();
    let whole = json!({"op": "user-config", "params": {}});
    let one = json!({"op": "user-config", "params": {"key": "adb_path"}});
    let input = format!("{whole}\n{one}");
    let mut output = Vec::new();
    let mut errors = Vec::new();
    serve::serve(table, input.as_bytes(), &mut output, &mut errors).expect("the loop ends");
    let output = String::from_utf8(output).unwrap();
    let lines = output.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2, "{output}");
    let typed: [&[&str]; 2] = [
        &["--json", "user-config", "get"],
        &["--json", "user-config", "get", "adb_path"],
    ];
    for (line, words) in lines.iter().zip(typed) {
        let exit = printed(words);
        assert_eq!(exit.code, 0, "{words:?}");
        let text = cli_module::stdout_text(&exit, true).expect("the command line's line");
        assert_eq!(*line, text, "{words:?}");
    }
    reads::take();
    assert!(!isolated.runtime.exists(), "no command reached the Runtime");

    // MCP's actinglab child (`lab_ops::apps`, `--json --via mcp session app <action> --instance
    // <alias>`): its line parses by `session app`'s list, the action first.
    let lab = Actinglab {
        program: Path::new("actinglab"),
        installation: None,
        state_root: None,
        via: "mcp",
    };
    for action in ACTIONS {
        let app = App::new(action, "neutral.a").expect("an action of session app");
        let line = apps::command(&lab, &app)
            .get_args()
            .map(|word| word.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let invocation = parse_invocation(line.clone(), false).expect("the child's line parses");
        assert_eq!(invocation.command, ["session", "app"], "{line:?}");
        assert_eq!(invocation.args.first().map(String::as_str), Some(action));
        assert_eq!(invocation.global.instance.as_deref(), Some("neutral.a"));
        let parsed = FlagArgs::parse_reading(
            &invocation.args[1..],
            &session_management::SESSION_APP_READS,
        );
        assert!(parsed.is_ok(), "{line:?}: {parsed:?}");
    }
    reads::take();
}
