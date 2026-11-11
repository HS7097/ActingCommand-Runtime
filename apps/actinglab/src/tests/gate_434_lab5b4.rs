// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-5B4 (register §1.287, "Slice LAB-5B4"; coordinator work order
// rd-1108/arch-013/brief-lab5b4.md): an own command refuses a flag it does not read, a switch takes
// no value from the next word but `true` or `false`, and a positional a command does not read is
// refused.
// - `every_own_command_refuses_a_flag_it_does_not_read`: each of the 28 commands (the 24 rows that
//   were lenient, and `run list`, `capture`, `lab receipt` and `lab evidence`, the register's
//   `own_flags::READ_LIST_ROWS` and `own_flags::UNROWED_READS`) refuses an unknown
//   flag before it does anything: `validation_failed` naming it (`argument`, `argument_problem`
//   `unknown`), the line, the exit code and the developer line those the CLI module gives a strict
//   row's undeclared flag (`watch --frobnicate`) but for the command's name; no read of its flags,
//   no Runtime, no recording session directory, no frame file. Also `record amend
//   --from-drift-diagnostics` refuses `--holder`, `--lease-holder` and `--lease-id`, which its
//   own list let through (the coordinator's ruling).
// - `a_switch_takes_no_value_from_the_next_word`: on a read list's command line a switch followed
//   by a word that is not `true` or `false` leaves the word a positional, and takes `true` or
//   `false` as its value; a positional beyond the list's is refused naming it; a strict command's
//   switches read so too (`observe`'s, and `record mark`'s, each of which is on its command's flag
//   list, as `record stop`'s); the traced `observe --capture <alias>` is refused for its
//   positional, and `observe --capture --instance <alias>` parses as a capture on that instance.
// - `the_reads_that_rested_on_reading_are_on_their_lists`: the capture freshness of `recover` and
//   `locate` and `navigate`'s step timing parse by their lists in the form their commands read
//   them; `recover --pack --pack-root --pages` reaches those reads on a planned recovery.
// - `an_option_at_the_end_of_the_line_is_refused`: `value_missing` naming the option, before
//   anything is written.
// - `lines_with_declared_flags_answer_as_before`: LAB-5B3's lines (`gate_434_lab5b3::corpus`),
//   which pass only flags their command reads, answer the exit and error codes they answered, and
//   none is refused by the new parse.
// - `serve_hands_a_formerly_lenient_row_its_declared_parameters_as_before`: `serve --stdio` on
//   `recover` and `record status` answers the line the command line gives for the same words.
// No Runtime, no device: temporary files and the embedded operation table.

use super::gate_434_lab5b3::{corpus, corpus_paths};
use super::{env_lock, set_missing_config_env, use_runtime_state_root};
use crate::cli_module::{self, Route, own_flags, own_line, serve};
use crate::commands::{navigation_recovery, session_record};
use crate::flag_args::FlagArgs;
use crate::flag_args::reads::{self, FlagReads, ReadPart};
use crate::{drive_cli, lab2_cli, parse_invocation, run_cli};
use actingcommand_lab_ops::cli_line::Exit;
use actingcommand_lab_ops::initiator::ENVIRONMENT_VARIABLE;
use serde_json::{Value, json};
use std::env;
use std::path::Path;
use tempfile::TempDir;

fn owned(line: &[&str]) -> Vec<String> {
    line.iter().map(|word| (*word).to_owned()).collect()
}

/// What actinglab prints for the command line `line`, as `main` routes it: actinglab's own command
/// parsed and run, or the CLI module's refusal of a row's short words.
fn printed(line: &[&str]) -> Exit {
    let invocation = match cli_module::route(owned(line)) {
        Route::Own(own) => parse_invocation(own, false),
        Route::Refused {
            command,
            json,
            error,
        } => Err((command, json, error)),
        Route::Module(module) => panic!("{module:?} is actinglab's own command line"),
    };
    own_line::answer(invocation).0
}

/// The developer line of `exit`, as actinglab writes it.
fn developer(exit: &Exit) -> Option<String> {
    exit.developer
        .as_ref()
        .map(|developer| developer.line(cli_module::PROGRAM))
}

/// An environment in which no command can reach a Runtime: the state root names a folder that
/// does not exist, no user configuration, no initiator variable.
struct Isolated {
    temp: TempDir,
    runtime: std::path::PathBuf,
    _runtime_env: super::RuntimeStateEnvGuard,
}

impl Isolated {
    fn new() -> Self {
        unsafe {
            env::remove_var(ENVIRONMENT_VARIABLE);
        }
        let temp = TempDir::new().unwrap();
        let runtime = temp.path().join("runtime-must-not-exist");
        let runtime_env = use_runtime_state_root(&runtime);
        set_missing_config_env();
        Self {
            temp,
            runtime,
            _runtime_env: runtime_env,
        }
    }

    fn path(&self, name: &str) -> String {
        self.temp.path().join(name).display().to_string()
    }
}

/// The 28 commands, each by the name its refusal gives it and the words of a command line that
/// reaches it: the 24 rows that were lenient (their command's words), then the four own commands
/// outside the operation table. `state` names the recording session's directory, `out` the frame
/// file `capture` would write.
fn commands<'a>(state: &'a str, out: &'a str) -> [(&'static str, Vec<&'a str>); 28] {
    let record = |action: &'a str| vec!["record", action, "--state-dir", state];
    [
        ("touch-probe", vec!["touch-probe"]),
        ("recover", vec!["session", "recover"]),
        ("tap-target", vec!["tap-target"]),
        ("navigate", vec!["navigate"]),
        ("debug-package", vec!["lab", "debug-package"]),
        ("export-evidence", vec!["lab", "export-evidence"]),
        ("recognize-artifact", vec!["recognize-artifact"]),
        ("record start", record("start")),
        ("record status", record("status")),
        ("record candidates", record("candidates")),
        ("record amend", record("amend")),
        ("record step", record("step")),
        ("package validate", vec!["package", "validate"]),
        ("package inspect", vec!["package", "inspect"]),
        ("package bundle", vec!["package", "bundle"]),
        ("resource compile-maa", vec!["resource", "compile-maa"]),
        ("operation validate", vec!["operation", "validate"]),
        ("operation inspect", vec!["operation", "inspect"]),
        ("operation explain", vec!["operation", "explain"]),
        ("recognize", vec!["recognize"]),
        ("detect-page", vec!["detect-page"]),
        ("current-page", vec!["current-page"]),
        ("is-visible", vec!["is-visible"]),
        ("locate", vec!["locate"]),
        ("run list", vec!["run", "list"]),
        ("capture", vec!["capture", "--out", out]),
        ("lab receipt", vec!["lab", "receipt"]),
        ("lab evidence", vec!["lab", "evidence"]),
    ]
}

#[test]
fn every_own_command_refuses_a_flag_it_does_not_read() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let (state, out) = (isolated.path("session"), isolated.path("frame.png"));
    // The 28 are the register's: the rows of `READ_LIST_ROWS`, then the read lists of the own
    // commands outside the table, each by the name its refusal gives it.
    let registered = own_flags::READ_LIST_ROWS
        .iter()
        .copied()
        .chain(own_flags::UNROWED_READS.iter().map(|list| list.row))
        .collect::<Vec<_>>();
    assert_eq!(registered, commands("", "").map(|(name, _)| name));
    // The refusal the CLI module gives a strict row's undeclared flag, as actinglab prints it.
    let strict = printed(&["--json", "watch", "--frobnicate", "x"]);
    let strict_line = strict.line.clone().expect("the strict row's line");
    assert_eq!(strict.code, 1, "{strict_line}");
    assert_eq!(strict_line["outcome"]["code"], "validation_failed");
    assert_eq!(strict_line["message"], "watch: unknown --frobnicate");
    let unknown = json!({"argument": "--frobnicate", "argument_problem": "unknown"});
    assert_eq!(strict_line["details"], unknown);
    let strict_developer = developer(&strict).expect("the strict row's developer line");

    let mut failures = Vec::new();
    reads::take();
    for (name, words) in commands(&state, &out) {
        let mut line = vec!["--json"];
        line.extend(&words);
        line.extend(["--frobnicate", "x"]);
        let exit = printed(&line);
        let recorded = reads::take();
        let Ok(answered) = exit.line.as_ref() else {
            failures.push(format!("{name}: {line:?} printed no line"));
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
                "{name}: {line:?} answered {answered} (exit {}, {:?}), not the strict refusal \
                 {strict_line} for {name}",
                exit.code,
                developer(&exit)
            ));
        }
        if !recorded.is_empty() {
            failures.push(format!("{name}: {line:?} read its flags: {recorded:?}"));
        }
    }
    // `record amend --from-drift-diagnostics` (the coordinator's ruling): the holder and lease
    // flags its own list let through, which amend never reads, are refused as every other.
    for flag in ["--holder", "--lease-holder", "--lease-id"] {
        let line = [
            "--json",
            "record",
            "amend",
            "--state-dir",
            state.as_str(),
            "--from-drift-diagnostics",
            "drift.json",
            flag,
            "x",
        ];
        let exit = printed(&line);
        let answered = exit.line.as_ref().expect("record amend's line");
        assert_eq!(
            answered["details"],
            json!({"argument": flag, "argument_problem": "unknown"}),
            "{answered}"
        );
    }
    reads::take();
    assert!(!isolated.runtime.exists(), "no command reached the Runtime");
    assert!(!Path::new(&state).exists(), "no recording session written");
    assert!(!Path::new(&out).exists(), "no frame written");
    assert!(
        failures.is_empty(),
        "own commands that do not refuse a flag they do not read:\n{}",
        failures.join("\n")
    );
}

/// The read list the parse tests read their words by.
const PROBE_READS: FlagReads = FlagReads {
    row: "probe",
    parts: &[&ReadPart {
        options: &["--value"],
        switches: &["--switch"],
        refused: &["--refused"],
    }],
    positionals: &["<word>"],
};

/// `line` parsed by [`PROBE_READS`].
fn probe(line: &[&str]) -> FlagArgs {
    FlagArgs::parse_reading(&owned(line), &PROBE_READS).expect("the probe's flags")
}

/// The refusal of `line` by [`PROBE_READS`]: its message and its details.
fn probe_refusal(line: &[&str]) -> (String, Value) {
    let error = FlagArgs::parse_reading(&owned(line), &PROBE_READS).expect_err("refused");
    (error.message, error.details.unwrap_or(Value::Null))
}

/// The refusal of the word `argument` for `problem`, as [`probe_refusal`] gives it.
fn refusal(message: &str, argument: &str, problem: &str) -> (String, Value) {
    (
        message.to_owned(),
        json!({"argument": argument, "argument_problem": problem}),
    )
}

#[test]
fn a_switch_takes_no_value_from_the_next_word() {
    // Followed by a word that is not `true` or `false`, a switch leaves it a positional.
    let flags = probe(&["--switch", "word"]);
    assert!(flags.bool("--switch"));
    assert_eq!(flags.positionals, ["word"]);
    // Followed by `true` or `false`, the word is its value.
    for (value, set) in [("true", true), ("false", false)] {
        let flags = probe(&["--switch", value]);
        assert_eq!(flags.bool("--switch"), set, "--switch {value}");
        assert!(flags.positionals.is_empty(), "--switch {value}");
    }
    // Followed by a flag, the switch alone; an option takes the next word as before, and none
    // that begins with `--`; a flag read only to be refused is read as before.
    let flags = probe(&["--switch", "--value", "v", "--refused", "r"]);
    assert!(flags.bool("--switch"));
    assert_eq!(flags.optional("--value").as_deref(), Some("v"));
    assert_eq!(flags.values("--refused"), ["r"]);
    let flags = probe(&["--value", "--switch"]);
    assert_eq!(flags.optional("--value").as_deref(), Some("true"));
    assert!(flags.bool("--switch"));
    // A positional beyond the list's, and a flag off it, are refused naming the word.
    assert_eq!(
        probe_refusal(&["--switch", "first", "second"]),
        refusal("probe: unknown second", "second", "unknown")
    );
    assert_eq!(
        probe_refusal(&["--other", "x"]),
        refusal("probe: unknown --other", "--other", "unknown")
    );
    // A strict command's switches read so too: `observe`'s.
    let switches = &lab2_cli::LAB2_SWITCHES;
    let line = owned(&["--capture", "neutral.a", "--scene", "frame.png"]);
    let flags = FlagArgs::parse_switches(&line, switches);
    assert!(flags.bool("--capture"));
    assert_eq!(flags.positionals, ["neutral.a"]);
    assert_eq!(flags.optional("--scene").as_deref(), Some("frame.png"));
    let flags = FlagArgs::parse_switches(&owned(&["--capture", "true"]), switches);
    assert!(flags.bool("--capture"));
    assert!(flags.positionals.is_empty());
    // `record mark`'s and `record stop`'s switches, read with `values`: each is on its command's
    // flag list, which refuses every other flag as before, and takes no value from the next word.
    for switch in session_record::RECORD_MARK_SWITCHES {
        assert!(
            session_record::RECORD_MARK_SHORTCUT_FLAGS.contains(&switch)
                || session_record::RECORD_MARK_REQUEST_FLAGS.contains(&switch),
            "{switch}"
        );
    }
    for switch in session_record::RECORD_STOP_SWITCHES {
        assert!(
            session_record::RECORD_STOP_FLAGS.contains(&switch),
            "{switch}"
        );
    }
    let line = owned(&["--close-step", "word", "--optional", "true"]);
    let flags = FlagArgs::parse_switches(&line, &session_record::RECORD_MARK_SWITCHES);
    assert_eq!(flags.positionals, ["word"]);
    assert_eq!(flags.values("--close-step"), ["true"]);
    assert_eq!(flags.values("--optional"), ["true"]);
    reads::take();

    let _guard = env_lock();
    let isolated = Isolated::new();
    // The traced line: the word after `--capture` is a positional, which `observe` does not read.
    let exit = printed(&["--json", "observe", "--capture", "neutral.a"]);
    let line = exit.line.as_ref().expect("observe's line");
    assert_eq!(exit.code, 1, "{line}");
    assert_eq!(line["outcome"]["code"], "validation_failed", "{line}");
    assert_eq!(line["message"], "observe: unknown neutral.a", "{line}");
    assert_eq!(
        line["details"],
        json!({"argument": "neutral.a", "argument_problem": "unknown"})
    );
    // `observe --capture --instance <alias>`: a capture on that instance.
    let invocation = parse_invocation(["observe", "--capture", "--instance", "neutral.a"], false)
        .expect("the invocation");
    assert_eq!(invocation.global.instance.as_deref(), Some("neutral.a"));
    let flags = FlagArgs::parse_switches(&invocation.args, switches);
    assert!(flags.bool("--capture"));
    assert!(flags.positionals.is_empty());
    // A read list's command line: `tap-target` reads one positional, its target; `capture` reads
    // the word `diagnose` only.
    for (line, word) in [
        (&["--json", "tap-target", "first", "second"][..], "second"),
        (&["--json", "capture", "other"][..], "other"),
    ] {
        let exit = printed(line);
        let answered = exit.line.as_ref().expect("the command's line");
        assert_eq!(
            answered["details"],
            json!({"argument": word, "argument_problem": "unknown"}),
            "{line:?}: {answered}"
        );
    }
    assert!(!isolated.runtime.exists(), "no command reached the Runtime");
    reads::take();
}

#[test]
fn an_option_at_the_end_of_the_line_is_refused() {
    assert_eq!(
        probe_refusal(&["--switch", "--value"]),
        refusal("probe: value_missing --value", "--value", "value_missing")
    );
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    let line = [
        "--json",
        "record",
        "step",
        "--state-dir",
        state.as_str(),
        "--kind",
    ];
    let exit = printed(&line);
    let answered = exit.line.as_ref().expect("record step's line");
    assert_eq!(exit.code, 1, "{answered}");
    assert_eq!(answered["outcome"]["code"], "validation_failed");
    assert_eq!(answered["message"], "record step: value_missing --kind");
    assert_eq!(
        answered["details"],
        json!({"argument": "--kind", "argument_problem": "value_missing"})
    );
    assert!(!Path::new(&state).exists(), "nothing written");
    assert!(!isolated.runtime.exists(), "no command reached the Runtime");
    reads::take();
}

#[test]
fn the_reads_that_rested_on_reading_are_on_their_lists() {
    // The capture freshness of `recover`'s route and of `locate`, and `navigate`'s step timing,
    // which their commands read only right before a capture or a press: on their lists, in the
    // form the commands read them (the recording fails a read its list does not give).
    let capture = owned(&["--capture", "--require-fresh", "--fresh-delay-ms", "10"]);
    for list in [
        &navigation_recovery::LOCATE_READS,
        &navigation_recovery::RECOVER_READS,
    ] {
        let flags = FlagArgs::parse_reading(&capture, list).expect("the capture flags");
        assert!(flags.bool("--capture"), "{}", list.row);
        assert!(flags.bool("--require-fresh"), "{}", list.row);
        assert_eq!(
            flags.optional("--fresh-delay-ms").as_deref(),
            Some("10"),
            "{}",
            list.row
        );
    }
    let timing = owned(&["--step-timeout-ms", "10", "--poll-ms", "5"]);
    let navigate = FlagArgs::parse_reading(&timing, &drive_cli::NAVIGATE_READS);
    let flags = navigate.expect("the step timing");
    assert_eq!(flags.optional("--step-timeout-ms").as_deref(), Some("10"));
    assert_eq!(flags.optional("--poll-ms").as_deref(), Some("5"));
    reads::take();

    // `recover`'s pack, its root and its pages: read on a planned recovery before the pack is.
    let _guard = env_lock();
    let isolated = Isolated::new();
    let pack = isolated.path("pack.json");
    let root = isolated.path("root");
    let pages = isolated.path("pages.json");
    let line = [
        "--json",
        "--dry-run",
        "session",
        "recover",
        "--pack",
        pack.as_str(),
        "--pack-root",
        root.as_str(),
        "--pages",
        pages.as_str(),
    ];
    let result = run_cli(line, true);
    let recorded = reads::take();
    let code = result
        .envelope
        .error
        .as_ref()
        .map(|error| error.code.as_str());
    assert_eq!(code, Some("validation_failed"));
    for flag in ["--pack", "--pack-root", "--pages"] {
        assert!(
            recorded.iter().any(|read| read.name == flag),
            "{flag}: {recorded:?}"
        );
    }
    assert!(!isolated.runtime.exists(), "no command reached the Runtime");
}

#[test]
fn lines_with_declared_flags_answer_as_before() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let paths = corpus_paths(&isolated.temp);
    let mut failures = Vec::new();
    for (row, line, exit_code, code, _) in corpus(&paths) {
        let result = run_cli(std::iter::once("--json").chain(line.iter().copied()), true);
        let error = result.envelope.error.as_ref();
        let answered = (
            result.exit_code(),
            error.map_or("", |error| error.code.as_str()),
        );
        let refused_by_the_parse = error.is_some_and(|error| {
            ["unknown", "value_missing"]
                .iter()
                .any(|problem| error.message.starts_with(&format!("{row}: {problem} ")))
        });
        if answered != (exit_code, code) || refused_by_the_parse {
            failures.push(format!(
                "{row}: {line:?} answered {answered:?}, not ({exit_code}, {code}) as before: {}",
                result.envelope_json()
            ));
        }
    }
    reads::take();
    assert!(!isolated.runtime.exists(), "no command reached the Runtime");
    assert!(
        failures.is_empty(),
        "lines with declared flags that answer otherwise:\n{}",
        failures.join("\n")
    );
}

#[test]
fn serve_hands_a_formerly_lenient_row_its_declared_parameters_as_before() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let state = isolated.path("session");
    let table = cli_module::table().expect("the operation table");
    let recover = json!({"instance": "neutral.a", "stale_capture": true, "seed": 1});
    let status = json!({"instance": "neutral.a", "state_dir": state});
    let recover = json!({"op": "recover", "params": recover});
    let status = json!({"op": "record status", "params": status});
    let input = format!("{recover}\n{status}");
    let mut output = Vec::new();
    let mut errors = Vec::new();
    serve::serve(table, input.as_bytes(), &mut output, &mut errors).expect("the loop ends");
    let output = String::from_utf8(output).unwrap();
    let served = output.lines().collect::<Vec<_>>();
    assert_eq!(served.len(), 2, "{output}");
    // The command line the engine gives each row's command, as a person types it.
    let words: [&[&str]; 2] = [
        &[
            "--json",
            "session",
            "recover",
            "--instance",
            "neutral.a",
            "--seed",
            "1",
            "--stale-capture",
        ],
        &[
            "--json",
            "record",
            "status",
            "--instance",
            "neutral.a",
            "--state-dir",
            state.as_str(),
        ],
    ];
    for (served, words) in served.iter().zip(words) {
        let exit = printed(words);
        let typed = cli_module::stdout_text(&exit, true).expect("the command line's line");
        assert_eq!(*served, typed, "{words:?}");
    }
    let answers = served
        .iter()
        .map(|line| serde_json::from_str::<Value>(line).expect("one JSON answer per line"))
        .collect::<Vec<_>>();
    // `recover`: its own refusal of the seed on the stale-capture path, as before.
    assert_eq!(answers[0]["outcome"]["code"], "validation_failed");
    assert_eq!(answers[0]["details"]["argument"], "--seed");
    assert_eq!(answers[0]["details"]["argument_problem"], "conflicting");
    // `record status`: answered.
    assert_eq!(answers[1]["outcome"]["code"], "lab_operation_completed");
    assert_eq!(answers[1]["status"], "not_started");
    assert!(!isolated.runtime.exists(), "no command reached the Runtime");
    reads::take();
}
