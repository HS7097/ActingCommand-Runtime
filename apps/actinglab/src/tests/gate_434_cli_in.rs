// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 CLI-IN (register §1.306; contracts/lab-two-entries.md §9.3; coordinator work order
// rd-1111/brief-cli-instance-flag.md, the coordinator's rulings of 2026-11-11, not Alice's words):
// actinglab's global `--instance <alias>` names the instance on every command that acts on one.
// - `cli_in_the_flag_reaches_the_request_of_the_positional`: for every offered row that declares a
//   positional instance and whose command line is the CLI module's or that of an own command under
//   other words, of each kind (instance required, instance optional, own command), `--instance
//   <alias>` after the row's words, before them or between them and the other positionals gives the
//   call the positional gives, the module gets the line as it is (or the own command the same
//   words), and the row's note says so; for some rows also the words `lab-ops` is given, the stop
//   `task stop` sends and the own command's line, compared with the positional's.
// - `cli_in_both_spellings_on_one_line_are_refused_before_anything_is_sent`: on every such row the
//   positional and the flag on one line, the same instance or another, in any order, and the flag
//   twice, are refused `repeated` with the word named, the module's line exits 1 with nothing but
//   the refusal (nothing read, nothing sent), an own command's row is actinglab's
//   `validation_failed`; `--instance` without a value is `value_missing`.
// - `cli_in_the_own_commands_and_the_own_options_read_as_before`: the rows actinglab's own command
//   serves under their own name go to that command as written, `--instance` before or after the
//   words, and take no positional instance (`text <alias> <text>` types both words); their note
//   says so; `remedy set` and `wake-test` read `--instance` as their own option; a row without an
//   instance refuses `--instance` by name, after its words or before them, as before.
// - `cli_in_serve_and_the_mcp_lines_are_unchanged`: `serve --stdio` gives every such row the call
//   its parameter `instance` gave, the same as the command line's two spellings, and refuses a
//   member written as the flag; MCP's `--via mcp` child's lines are actinglab's own, as written.
// Fakes only: the operation table this build embeds and neutral aliases; no state root on which a
// Runtime runs, no device, no clock, no sleep.

use super::*;
use crate::cli_module::{self, Route, engine, words};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_lab_ops::cli_line;
use actingcommand_lab_ops::core_ops;
use actingcommand_lab_ops::initiator::ENVIRONMENT_VARIABLE;
use actingcommand_lab_ops::table::{Operation, OperationTable, Param, ParamForm, ParamType};
use serde_json::{Value, json};
use std::path::Path;

/// The instance the lines name, and another.
const ALIAS: &str = "neutral.a";
const OTHER: &str = "neutral.b";
/// A request id, as `task stop` takes one.
const REQUEST: &str = "request_0123456789abcdef0123456789abcdef";
/// The state root the words name; nothing reads it.
const STATE: &str = "state";
/// The note of a row whose command line takes `--instance` in place of its positional.
const ALSO: &str = "on the command line also --instance <x>, in place of the positional";
/// The note of a row whose own command takes only `--instance`.
const ONLY: &str = "on the command line only --instance <x>";

fn words_of(line: &[&str]) -> Vec<String> {
    line.iter().map(|word| (*word).to_owned()).collect()
}

fn table() -> &'static OperationTable {
    cli_module::table().expect("the operation table")
}

/// Whether actinglab's own command serves `operation` under the row's own name: its command line
/// is that command's (`engine::COMMANDS`, the command's words the row's name).
fn own_named(operation: &Operation) -> bool {
    engine::COMMANDS
        .iter()
        .any(|(name, command, _)| *name == operation.name && command.join(" ") == operation.name)
}

/// The offered rows that declare a positional instance, but those of [`own_named`]: the rows whose
/// command line takes `--instance` in place of the positional.
fn instance_rows() -> Vec<&'static Operation> {
    table()
        .operations
        .iter()
        .filter(|operation| words::offered(operation))
        .filter(|operation| words::positional_instance(operation).is_some())
        .filter(|operation| !own_named(operation))
        .collect()
}

/// A word of the type of `param`, as a person writes one.
fn word_of(param: &Param) -> String {
    match &param.value_type {
        ParamType::Integer | ParamType::Unsigned => "5".to_owned(),
        ParamType::RequestId | ParamType::RunRef => REQUEST.to_owned(),
        ParamType::Enum(words) => words[0].clone(),
        _ => format!("neutral-{}", param.name.replace('_', "-")),
    }
}

/// The row's words and its required positionals in their order, the instance `instance` in its
/// place (left out when `None`).
fn positional_words(operation: &Operation, instance: Option<&str>) -> Vec<String> {
    let mut line = operation
        .name
        .split(' ')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for param in operation
        .params
        .iter()
        .filter(|param| param.form == ParamForm::Positional)
    {
        if param.value_type == ParamType::Instance {
            line.extend(instance.map(str::to_owned));
        } else if param.required {
            line.push(word_of(param));
        }
    }
    line
}

/// The call the words give `line`.
fn parsed(line: &[String]) -> words::Call {
    words::parse(table(), line).unwrap_or_else(|outcome| {
        panic!(
            "{line:?}: {}",
            cli_line::outcome_value(&outcome, Detail::Full)
        )
    })
}

/// The parameters the words give `line`.
fn params(line: &[&str]) -> Value {
    Value::Object(parsed(&words_of(line)).params)
}

/// The refusal's code, `argument` and `argument_problem`.
fn refusal(outcome: &Outcome) -> (Value, Value, Value) {
    let value = cli_line::outcome_value(outcome, Detail::Full);
    (
        value["code"].clone(),
        value["values"]["argument"].clone(),
        value["values"]["argument_problem"].clone(),
    )
}

/// The words' refusal of `line`.
fn refused(line: &[String]) -> (Value, Value, Value) {
    match words::parse(table(), line) {
        Ok(call) => panic!("{line:?} must be refused, not read as {call:?}"),
        Err(outcome) => refusal(&outcome),
    }
}

/// The words the row of `line` gives its command in `lab-ops` (the state root [`STATE`], no
/// install root), as LAB-5C's test reads them.
fn handed(line: &[&str]) -> Vec<String> {
    let call = parsed(&words_of(line));
    let operation = table().operation(&call.name).expect("the call's row");
    let argv = match core_ops::offline_words(operation, &call.params, None) {
        Some(argv) => argv,
        None => match core_ops::handed_words(operation, &call.params, None, Path::new(STATE)) {
            Some(Ok(argv)) => argv,
            Some(Err(exit)) => panic!("{line:?} is refused: {:?}", exit.line),
            None => panic!("{line:?} gives lab-ops no words"),
        },
    };
    argv.into_iter()
        .map(|word| word.into_string().expect("UTF-8 words"))
        .collect()
}

/// `line` goes to the CLI module as it is.
fn module_route(line: &[String]) {
    match cli_module::route(line.to_vec()) {
        Route::Module(module) => assert_eq!(module.as_slice(), line),
        other => panic!("{line:?} is the CLI module's: {other:?}"),
    }
}

/// The command line `main`'s route gives actinglab's own command for `line`.
fn own_route(line: &[String]) -> Vec<String> {
    match cli_module::route(line.to_vec()) {
        Route::Own(own) => own,
        other => panic!("{line:?} is given to actinglab's own command: {other:?}"),
    }
}

/// The details of the usage error `main`'s route answers for `line`.
fn route_refused(line: &[String]) -> Value {
    match cli_module::route(line.to_vec()) {
        Route::Refused { error, .. } => {
            assert_eq!(error.code, "validation_failed", "{line:?}");
            error.details.expect("the refusal's details")
        }
        other => panic!("{line:?} is refused: {other:?}"),
    }
}

#[test]
fn cli_in_the_flag_reaches_the_request_of_the_positional() {
    let rows = instance_rows();
    let flag = words_of(&[words::INSTANCE_FLAG, ALIAS]);
    let mut kinds = [0_usize; 3];
    for operation in &rows {
        let name = operation.name.as_str();
        let instance = words::positional_instance(operation).expect("a positional instance");
        // The table says it, one note on every such row.
        assert!(
            instance
                .note
                .as_deref()
                .is_some_and(|note| note.contains(ALSO)),
            "{name}: {:?}",
            instance.note
        );
        let module = core_ops::serves(operation);
        let kind = match (module, instance.required) {
            (true, true) => 0,
            (true, false) => 1,
            (false, _) => 2,
        };
        kinds[kind] += 1;
        let positional = positional_words(operation, Some(ALIAS));
        let call = parsed(&positional);
        assert_eq!(
            call.params.get("instance"),
            Some(&json!(ALIAS)),
            "{positional:?}"
        );
        let bare = positional_words(operation, None);
        let name_len = operation.name.split(' ').count();
        for line in [
            [&bare[..], &flag[..]].concat(),
            [&flag[..], &bare[..]].concat(),
            [&bare[..name_len], &flag[..], &bare[name_len..]].concat(),
        ] {
            assert_eq!(parsed(&line), call, "{line:?}");
            if module {
                module_route(&line);
            } else {
                assert_eq!(own_route(&line), own_route(&positional), "{line:?}");
            }
        }
    }
    assert!(kinds.iter().all(|count| *count > 0), "{kinds:?}");

    // Instance required, a row of the CLI module.
    assert_eq!(
        params(&["emu", "start", "--instance", "16384", "--wait", "30s"]),
        json!({"instance": "16384", "wait": "30s"})
    );
    // The other positionals keep their order wherever the flag stands.
    let bind = ["devices", "bind", ALIAS, "TESTSERIAL0001"];
    let binds: [&[&str]; 3] = [
        &["devices", "bind", "--instance", ALIAS, "TESTSERIAL0001"],
        &["devices", "bind", "TESTSERIAL0001", "--instance", ALIAS],
        &["--instance", ALIAS, "devices", "bind", "TESTSERIAL0001"],
    ];
    for line in binds {
        assert_eq!(params(line), params(&bind), "{line:?}");
        assert_eq!(handed(line), handed(&bind), "{line:?}");
    }
    let stop = ["task", "stop", ALIAS, REQUEST, "--wait", "2s"];
    let stops: [&[&str]; 2] = [
        &["task", "stop", REQUEST, "--instance", ALIAS, "--wait", "2s"],
        &["task", "stop", "--wait", "2s", REQUEST, "--instance", ALIAS],
    ];
    let target = |line: &[&str]| {
        core_ops::stop_target(&parsed(&words_of(line)).params).expect("the stop's run and wait")
    };
    for line in stops {
        assert_eq!(params(line), params(&stop), "{line:?}");
        assert_eq!(target(line), target(&stop[..]), "{line:?}");
    }
    assert_eq!(
        params(&["run", "start", "pack.zip", "--instance", ALIAS]),
        params(&["run", "start", ALIAS, "pack.zip"])
    );

    // Instance optional: `pause` as LAB-2's words give it, `status` and `events` with the words
    // `lab-ops` is given, `offset`, whose instance is its last positional, and `facts`.
    assert_eq!(
        params(&[
            "pause",
            "--instance",
            ALIAS,
            "--reason",
            "neutral-hold",
            "--drain",
            "300s"
        ]),
        json!({"instance": ALIAS, "reason": "neutral-hold", "drain": "300s"})
    );
    let statuses: [&[&str]; 2] = [
        &["status", "--instance", ALIAS, "--sections", "daemon"],
        &["--instance", ALIAS, "status", "--sections", "daemon"],
    ];
    for line in statuses {
        assert_eq!(
            handed(line),
            words_of(&[
                "overview",
                "--state-root",
                STATE,
                "--instance",
                ALIAS,
                "--sections",
                "daemon"
            ]),
            "{line:?}"
        );
        assert_eq!(
            handed(line),
            handed(&["status", ALIAS, "--sections", "daemon"])
        );
    }
    assert_eq!(
        handed(&["events", "--limit", "7", "--instance", ALIAS]),
        handed(&["events", ALIAS, "--limit", "7"])
    );
    assert_eq!(
        handed(&["packs", "--instance", ALIAS]),
        words_of(&["packs", "--instance", ALIAS])
    );
    assert_eq!(
        params(&["offset", "--instance", ALIAS, "neutral-task", "5"]),
        json!({"task": "neutral-task", "ms": 5, "instance": ALIAS})
    );
    assert_eq!(
        params(&["offset", "neutral-task", "5", "--instance", ALIAS]),
        params(&["offset", "neutral-task", "5", ALIAS])
    );
    assert_eq!(
        params(&["facts", "--instance", ALIAS, "--prefix", "neutral."]),
        json!({"instance": ALIAS, "prefix": "neutral."})
    );
    // `runs` and `monitor set` read `--instance` as before (LAB-5A's lines).
    assert_eq!(
        params(&["runs", "--instance", ALIAS]),
        json!({"instance": ALIAS})
    );
    assert_eq!(
        params(&["monitor", "set", "--instance", ALIAS, "--interval", "5m"]),
        json!({"instance": ALIAS, "interval": "5m"})
    );

    // A row an own command serves under other words: the instance goes to the command as its
    // `--instance`, from either spelling, `--json` kept.
    let launch = words_of(&["session", "app", "launch", "--instance", "16384"]);
    let launches: [&[&str]; 3] = [
        &["app", "launch", "16384"],
        &["app", "launch", "--instance", "16384"],
        &["--instance", "16384", "app", "launch"],
    ];
    for line in launches {
        assert_eq!(own_route(&words_of(line)), launch, "{line:?}");
    }
    let recover = words_of(&[
        "--json",
        "session",
        "recover",
        "--instance",
        ALIAS,
        "--to",
        "home",
    ]);
    let recovers: [&[&str]; 3] = [
        &["--json", "recover", ALIAS, "--to", "home"],
        &["--json", "recover", "--to", "home", "--instance", ALIAS],
        &["--json", "--instance", ALIAS, "recover", "--to", "home"],
    ];
    for line in recovers {
        assert_eq!(own_route(&words_of(line)), recover, "{line:?}");
    }
}

#[test]
fn cli_in_both_spellings_on_one_line_are_refused_before_anything_is_sent() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let same = words_of(&[words::INSTANCE_FLAG, ALIAS]);
    let other = words_of(&[words::INSTANCE_FLAG, OTHER]);
    for operation in instance_rows() {
        let positional = positional_words(operation, Some(ALIAS));
        let bare = positional_words(operation, None);
        let name_len = operation.name.split(' ').count();
        let both = [
            // After the positional, naming another instance or the same one.
            ([&positional[..], &other[..]].concat(), ALIAS),
            ([&positional[..], &same[..]].concat(), ALIAS),
            // Before the row's words, and between them and the positionals.
            ([&other[..], &positional[..]].concat(), ALIAS),
            (
                [&positional[..name_len], &other[..], &positional[name_len..]].concat(),
                ALIAS,
            ),
            // The flag twice.
            (
                [&bare[..], &same[..], &other[..]].concat(),
                words::INSTANCE_FLAG,
            ),
        ];
        for (line, argument) in both {
            let expected = json!({"argument": argument, "argument_problem": "repeated"});
            assert_eq!(
                refused(&line),
                (
                    json!("actingctl_usage_invalid"),
                    json!(argument),
                    json!("repeated")
                ),
                "{line:?}"
            );
            if core_ops::serves(operation) {
                // The module's line: the refusal alone, exit 1; the row read nothing and sent
                // nothing.
                module_route(&line);
                let exit = cli_module::run_line(&line);
                let answered = exit.line.expect("the module's line");
                assert_eq!(exit.code, 1, "{line:?}: {answered}");
                assert_eq!(
                    answered["outcome"]["code"], "actingctl_usage_invalid",
                    "{answered}"
                );
                assert_eq!(answered["outcome"]["values"], expected, "{answered}");
                assert_eq!(
                    answered.as_object().map(|members| members.len()),
                    Some(1),
                    "{answered}"
                );
            } else {
                // A row an own command serves under other words: actinglab's usage error, and
                // the command does not run.
                assert_eq!(route_refused(&line), expected, "{line:?}");
            }
        }
    }
    // Rows with more positionals name the word in the instance's place.
    assert_eq!(
        refused(&words_of(&[
            "devices",
            "bind",
            ALIAS,
            "TESTSERIAL0001",
            "--instance",
            OTHER
        ]))
        .1,
        json!(ALIAS)
    );
    assert_eq!(
        refused(&words_of(&[
            "offset",
            "--instance",
            OTHER,
            "neutral-task",
            "5",
            ALIAS
        ]))
        .1,
        json!(ALIAS)
    );
    // `--instance` without its value.
    let missing: [&[&str]; 3] = [
        &["pause", "--instance"],
        &["pause", "--instance", "--reason", "neutral-hold"],
        &["emu", "start", "--wait", "30s", "--instance"],
    ];
    for line in missing {
        assert_eq!(
            refused(&words_of(line)),
            (
                json!("actingctl_usage_invalid"),
                json!("--instance"),
                json!("value_missing")
            ),
            "{line:?}"
        );
    }
}

#[test]
fn cli_in_the_own_commands_and_the_own_options_read_as_before() {
    let flag = words_of(&[words::INSTANCE_FLAG, ALIAS]);
    let mut own = 0;
    for operation in table().operations.iter().filter(|operation| {
        words::offered(operation)
            && own_named(operation)
            && words::positional_instance(operation).is_some()
    }) {
        own += 1;
        let instance = words::positional_instance(operation).expect("a positional instance");
        assert!(
            instance
                .note
                .as_deref()
                .is_some_and(|note| note.contains(ONLY)),
            "{}: {:?}",
            operation.name,
            instance.note
        );
        let name = operation
            .name
            .split(' ')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        for line in [
            [&name[..], &flag[..]].concat(),
            [&flag[..], &name[..]].concat(),
        ] {
            assert_eq!(own_route(&line), line, "{line:?}");
            let invocation = parse_invocation(line.clone(), true).expect("an own command line");
            assert_eq!(
                invocation.global.instance.as_deref(),
                Some(ALIAS),
                "{line:?}"
            );
        }
    }
    assert!(own > 0);
    // No own command takes a positional instance: `text` types the alias with the rest, as before.
    let text = words_of(&["text", ALIAS, "hello"]);
    assert_eq!(own_route(&text), text);
    let invocation = parse_invocation(text, true).expect("the text line");
    assert_eq!(invocation.global.instance, None);
    assert_eq!(invocation.args, words_of(&[ALIAS, "hello"]));

    // The rows that declare the instance as an option of their own read `--instance` as it.
    assert_eq!(
        params(&["remedy", "set", "--instance", ALIAS, "rows.json"]),
        json!({"instance": ALIAS, "file": "rows.json"})
    );
    assert_eq!(
        params(&["wake-test", "neutral-channel", "--instance", ALIAS]),
        json!({"channel": "neutral-channel", "instance": ALIAS})
    );

    // A row without an instance refuses the flag by name, after its words or before them.
    let without: [&[&str]; 2] = [
        &["codes", "--instance", ALIAS],
        &["--instance", ALIAS, "codes"],
    ];
    for line in without {
        assert_eq!(
            refused(&words_of(line)),
            (
                json!("actingctl_usage_invalid"),
                json!("--instance"),
                json!("unknown")
            ),
            "{line:?}"
        );
    }
}

#[test]
fn cli_in_serve_and_the_mcp_lines_are_unchanged() {
    let table = table();
    for operation in instance_rows() {
        let call = parsed(&positional_words(operation, Some(ALIAS)));
        let request = json!({"op": operation.name, "params": Value::Object(call.params.clone())});
        let served = words::from_json(table, &request.to_string()).unwrap_or_else(|outcome| {
            panic!(
                "{request}: {}",
                cli_line::outcome_value(&outcome, Detail::Full)
            )
        });
        assert_eq!(served, call, "{request}");
        let flagged = [
            positional_words(operation, None),
            words_of(&[words::INSTANCE_FLAG, ALIAS]),
        ]
        .concat();
        assert_eq!(parsed(&flagged), served, "{flagged:?}");
    }
    // `serve` takes the parameter, never the flag.
    let request = json!({"op": "pause", "params": {"--instance": ALIAS}}).to_string();
    let outcome = words::from_json(table, &request).expect_err("a member written as the flag");
    assert_eq!(
        refusal(&outcome),
        (
            json!("actingctl_usage_invalid"),
            json!("--instance"),
            json!("unknown")
        )
    );
    // MCP's `--via mcp` child: its lines stay actinglab's own, as written.
    let via: [&[&str]; 5] = [
        &[
            "--json",
            "--via",
            "mcp",
            "session",
            "app",
            "launch",
            "--instance",
            ALIAS,
        ],
        &["--json", "--via", "mcp", "observe", "--instance", ALIAS],
        &["--json", "--via", "mcp", "--instance", ALIAS, "status"],
        &["--json", "--instance", ALIAS, "--via", "mcp", "recover"],
        &["--json", "--instance", ALIAS, "recover", "--via", "mcp"],
    ];
    for line in via {
        assert_eq!(own_route(&words_of(line)), words_of(line), "{line:?}");
    }
}
