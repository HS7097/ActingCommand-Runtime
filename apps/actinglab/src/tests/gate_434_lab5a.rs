// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-5A (register §1.287, "Slice LAB-5A"; contracts/lab-two-entries.md §9.1 to
// §9.3; coordinator work order rd-1108/arch-013/brief-lab5a.md): every row actinglab offers
// declares all of its parameters, no command line changes what it accepts or refuses, and
// `serve --stdio` refuses a parameter its row does not declare.
// - `lab5a_rows_take_what_they_took`: no offered row has `params_more`. The router: every row
//   actinglab's own command serves names its command in `engine::COMMANDS`, and the rows whose
//   command line took their command's further flags hand their flags on as written, with
//   actinglab's global options (each of which actinglab's parser takes globally). For every
//   offered row that passes the words on the command line and had `params_more` (`runs`, `run
//   evidence`, `gap`, `devices bind`, `watch`, the three `signatures` rows, `unpin`,
//   `regression`, `op status`, `release verify`, `release stage`, the six `agent` rows, and the
//   lenient `recover`, `debug-package` and `export-evidence`): a command line with a flag it
//   passed through before is handed on as before (the command line its own command gets; the
//   member the module reads, and for `runs`, `run evidence` and `gap` the flag `lab-ops`' parser
//   reads, shown by its refusal of the value), and a command line with a flag the row does not
//   declare is refused by name, but handed on for the three lenient rows, whose `serve --stdio`
//   line is refused. Also `runs --instance <x>`, a `seed` above the signed range and a `decimal`.
// Fakes only: a state root in a temporary folder on which no Runtime runs; no device, no clock,
// no sleep.

use super::*;
use crate::cli_module::{self, Route, engine, words};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_lab_ops::cli_line;
use actingcommand_lab_ops::initiator::ENVIRONMENT_VARIABLE;
use actingcommand_lab_ops::table::{OperationModule, OperationTable, ParamForm, ParamType};

/// A run id, as `run show` and `run evidence` take one.
const RUN: &str = "run_0123456789abcdef0123456789abcdef";

fn words_of(line: &[&str]) -> Vec<String> {
    line.iter().map(|word| (*word).to_owned()).collect()
}

fn table() -> &'static OperationTable {
    cli_module::table().expect("the operation table")
}

/// The refusal's `argument` and `argument_problem`.
fn refusal(outcome: &Outcome) -> (Value, Value) {
    let value = cli_line::outcome_value(outcome, Detail::Full);
    assert_eq!(value["code"], "actingctl_usage_invalid", "{value}");
    (
        value["values"]["argument"].clone(),
        value["values"]["argument_problem"].clone(),
    )
}

/// The parameters the words give the module for `line`.
fn parsed(line: &[&str]) -> Value {
    let call = words::parse(table(), &words_of(line)).unwrap_or_else(|outcome| {
        panic!(
            "{line:?}: {}",
            cli_line::outcome_value(&outcome, Detail::Full)
        )
    });
    Value::Object(call.params)
}

/// The words' refusal of `line`.
fn refused(line: &[&str]) -> (Value, Value) {
    match words::parse(table(), &words_of(line)) {
        Ok(call) => panic!("{line:?} must be refused, not read as {call:?}"),
        Err(outcome) => refusal(&outcome),
    }
}

/// The command line `main`'s route gives actinglab's own command for `line`.
fn own_route(line: &[&str]) -> Vec<String> {
    match cli_module::route(words_of(line)) {
        Route::Own(own) => own,
        other => panic!("{line:?} is given to actinglab's own command: {other:?}"),
    }
}

/// The details of the usage error `main`'s route answers for `line`.
fn route_refused(line: &[&str]) -> Value {
    match cli_module::route(words_of(line)) {
        Route::Refused { error, .. } => {
            assert_eq!(error.code, "validation_failed", "{line:?}");
            error.details.expect("the refusal's details")
        }
        other => panic!("{line:?} is refused: {other:?}"),
    }
}

/// `line` goes to the CLI module as it is.
fn module_route(line: &[&str]) {
    match cli_module::route(words_of(line)) {
        Route::Module(module) => assert_eq!(module, words_of(line)),
        other => panic!("{line:?} is the CLI module's: {other:?}"),
    }
}

/// The `serve --stdio` call of `request`, or its refusal.
fn served(request: Value) -> Result<Value, (Value, Value)> {
    words::from_json(table(), &request.to_string())
        .map(|call| Value::Object(call.params))
        .map_err(|outcome| refusal(&outcome))
}

/// The refusal one module line of `line` answers: its `argument` and `argument_problem`.
fn module_refusal(line: &[&str]) -> (Value, Value) {
    let exit = cli_module::run_line(&words_of(line));
    let answered = exit.line.expect("the module's line");
    assert_eq!(exit.code, 1, "{line:?}: {answered}");
    assert_eq!(
        answered["outcome"]["code"], "actingctl_usage_invalid",
        "{answered}"
    );
    (
        answered["outcome"]["values"]["argument"].clone(),
        answered["outcome"]["values"]["argument_problem"].clone(),
    )
}

#[test]
fn lab5a_rows_take_what_they_took() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let table = table();
    let unknown = json!("unknown");

    // No offered row has `params_more` (§9.2): each declares its parameters.
    for operation in table.operations.iter().filter(|row| words::offered(row)) {
        assert!(
            operation.params_more.is_none(),
            "{} is offered with params_more",
            operation.name
        );
    }

    // The router: every row actinglab's own command serves (the debugging and authoring rows but
    // the four the module serves, and `app launch|restart|stop`) has one entry in
    // `engine::COMMANDS`, each entry an offered row; the rows whose command line is read as
    // declared are those that never took their command's further flags.
    let module_rows = ["run start", "run stop", "reset", "drill"];
    let mut commands = BTreeSet::new();
    for (name, command, line) in engine::COMMANDS {
        assert!(commands.insert(name), "{name} twice in engine::COMMANDS");
        assert!(!command.is_empty(), "{name}");
        let operation = table
            .operation(name)
            .unwrap_or_else(|| panic!("{name} is a row"));
        assert!(words::offered(operation), "{name} is offered");
        let declared = matches!(
            name,
            "app launch" | "app restart" | "app stop" | "user-config" | "user-config set"
        );
        assert_eq!(line == engine::Line::Declared, declared, "{name}: {line:?}");
        // The engine writes a flag by its value: no option of its rows is a boolean value.
        for param in &operation.params {
            assert!(
                param.form != ParamForm::Option || param.value_type != ParamType::Boolean,
                "{name}'s option {} is a boolean",
                param.name
            );
        }
    }
    for operation in table.operations.iter().filter(|row| words::offered(row)) {
        let engine_row = match operation.module {
            OperationModule::Debugging | OperationModule::Authoring => {
                !module_rows.contains(&operation.name.as_str())
            }
            OperationModule::Core => operation.name.starts_with("app "),
            OperationModule::Rescue => false,
        };
        assert_eq!(
            commands.contains(operation.name.as_str()),
            engine_row,
            "{}",
            operation.name
        );
    }
    // actinglab's global options, which such a row's command line hands on, are each taken by
    // actinglab's parser as a global option (or refused there for its value), never left to the
    // command.
    for option in words::GLOBAL_OPTIONS {
        if let Ok(invocation) = parse_invocation(["lab", "watch", option, "x"], true) {
            let left = invocation
                .command
                .iter()
                .chain(&invocation.args)
                .any(|word| word == option);
            assert!(
                !left,
                "{option} is actinglab's global option: {:?}",
                invocation.args
            );
        }
    }

    // The rows actinglab's own command serves: the flag they passed through before is handed on
    // as before; an undeclared flag is refused by name.
    let handed_on: [(&[&str], &[&str]); 10] = [
        (
            &["watch", "--from-sequence", "5", "--verbose"],
            &["lab", "watch", "--from-sequence", "5", "--verbose"],
        ),
        (
            &["--json", "watch", "--after", "3"],
            &["--json", "lab", "watch", "--after", "3"],
        ),
        (
            &[
                "signatures",
                "register",
                "--signature-id",
                "neutral.signature",
                "--signature-version",
                "1",
            ],
            &[
                "lab",
                "signatures",
                "register",
                "--signature-id",
                "neutral.signature",
                "--signature-version",
                "1",
            ],
        ),
        (
            &[
                "signatures",
                "match",
                "--input-state-root",
                "state",
                "--input-through",
                "3",
            ],
            &[
                "lab",
                "signatures",
                "match",
                "--input-state-root",
                "state",
                "--input-through",
                "3",
            ],
        ),
        (
            &["signatures", "retire", "--registration-sequence", "9"],
            &[
                "lab",
                "signatures",
                "retire",
                "--registration-sequence",
                "9",
            ],
        ),
        (
            &[
                "unpin",
                "--artifact-id",
                "a1",
                "--pin-sequence",
                "2",
                "--pin-event-id",
                "e1",
            ],
            &[
                "lab",
                "unpin",
                "--artifact-id",
                "a1",
                "--pin-sequence",
                "2",
                "--pin-event-id",
                "e1",
            ],
        ),
        (
            &[
                "regression",
                "--state-root",
                "state",
                "--evidence",
                "page.json",
                "--package",
                "pack.zip",
            ],
            &[
                "recognition-regression",
                "--state-root",
                "state",
                "--evidence",
                "page.json",
                "--package",
                "pack.zip",
            ],
        ),
        // A seed above the signed range, handed on as written.
        (
            &[
                "recover",
                "neutral.a",
                "--to",
                "home",
                "--seed",
                "18446744073709551615",
            ],
            &[
                "session",
                "recover",
                "--instance",
                "neutral.a",
                "--to",
                "home",
                "--seed",
                "18446744073709551615",
            ],
        ),
        (
            &["debug-package", "--package", "pack.zip"],
            &["lab", "debug-package", "--package", "pack.zip"],
        ),
        (
            &[
                "export-evidence",
                "--package",
                "pack.zip",
                "--out",
                "evidence.zip",
                "--outcome",
                "failure",
            ],
            &[
                "lab",
                "export-evidence",
                "--package",
                "pack.zip",
                "--out",
                "evidence.zip",
                "--outcome",
                "failure",
            ],
        ),
    ];
    for (line, own) in handed_on {
        assert_eq!(own_route(line), words_of(own), "{line:?}");
    }
    let strict: [&[&str]; 6] = [
        &["watch", "--frobnicate", "1"],
        &["signatures", "register", "--frobnicate", "1"],
        &[
            "signatures",
            "match",
            "--input-state-root",
            "state",
            "--frobnicate",
            "1",
        ],
        &["signatures", "retire", "--frobnicate", "1"],
        &[
            "unpin",
            "--artifact-id",
            "a1",
            "--pin-sequence",
            "2",
            "--pin-event-id",
            "e1",
            "--frobnicate",
            "1",
        ],
        &[
            "regression",
            "--state-root",
            "state",
            "--evidence",
            "page.json",
            "--frobnicate",
            "1",
        ],
    ];
    for line in strict {
        assert_eq!(
            route_refused(line),
            json!({"argument": "--frobnicate", "argument_problem": "unknown"}),
            "{line:?}"
        );
    }
    // The three lenient rows still hand an undeclared flag and a further word on, on the command
    // line (the debt of `words::LENIENT_ON_COMMAND_LINE`).
    assert_eq!(
        own_route(&["recover", "neutral.a", "--frobnicate", "x", "extra"]),
        words_of(&[
            "session",
            "recover",
            "extra",
            "--instance",
            "neutral.a",
            "--frobnicate",
            "x"
        ])
    );
    assert_eq!(
        own_route(&["debug-package", "--package", "pack.zip", "--frobnicate"]),
        words_of(&[
            "lab",
            "debug-package",
            "--package",
            "pack.zip",
            "--frobnicate"
        ])
    );
    assert_eq!(
        own_route(&[
            "export-evidence",
            "--out",
            "evidence.zip",
            "--frobnicate",
            "1"
        ]),
        words_of(&[
            "lab",
            "export-evidence",
            "--out",
            "evidence.zip",
            "--frobnicate",
            "1"
        ])
    );

    // The core rows: the member the module reads, under the name it had.
    let core: [&[&str]; 4] = [
        &["runs", "--limit", "5"],
        &["run", "evidence", RUN, "--limit", "5"],
        &["gap", "--capability", "x", "--why", "y"],
        &["devices", "bind", "neutral.a", "TESTSERIAL0001"],
    ];
    for line in core {
        module_route(line);
    }
    assert_eq!(
        parsed(&[
            "runs",
            "--limit",
            "5",
            "--state",
            "open",
            "--dispatch",
            "manual"
        ]),
        json!({"limit": 5, "state": "open", "dispatch": "manual"})
    );
    assert_eq!(
        parsed(&["runs", "--since-unix-ms", "18446744073709551615"]),
        json!({"since_unix_ms": 18_446_744_073_709_551_615_u64})
    );
    // `runs --instance <x>` stays accepted, as the instance (words::POSITIONAL_FLAGS).
    module_route(&["runs", "--instance", "neutral.a"]);
    assert_eq!(
        parsed(&["runs", "--instance", "neutral.a"]),
        json!({"instance": "neutral.a"})
    );
    assert_eq!(
        refused(&["runs", "neutral.a", "--instance", "neutral.b"]),
        (json!("--instance"), json!("repeated"))
    );
    assert_eq!(
        parsed(&["run", "evidence", RUN, "--from-step", "2", "--limit", "10"]),
        json!({"id": RUN, "from_step": 2, "limit": 10})
    );
    assert_eq!(
        parsed(&["gap", "--capability", "x", "--why", "y"]),
        json!({"capability": "x", "why": "y"})
    );
    assert_eq!(
        parsed(&["devices", "bind", "neutral.a", "TESTSERIAL0001"]),
        json!({"instance": "neutral.a", "handle": "TESTSERIAL0001"})
    );
    let undeclared: [&[&str]; 4] = [
        &["runs", "--frobnicate", "1"],
        &["run", "evidence", RUN, "--frobnicate", "1"],
        &[
            "gap",
            "--capability",
            "x",
            "--why",
            "y",
            "--frobnicate",
            "1",
        ],
        &[
            "devices",
            "bind",
            "neutral.a",
            "TESTSERIAL0001",
            "--frobnicate",
            "1",
        ],
    ];
    for line in undeclared {
        assert_eq!(
            refused(line),
            (json!("--frobnicate"), unknown.clone()),
            "{line:?}"
        );
    }
    // `lab-ops`' parsers read the flags the rows hand on: they refuse the values, before any
    // connection or write.
    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("state");
    fs::create_dir_all(&state_root).unwrap();
    let _state = use_runtime_state_root(&state_root);
    assert_eq!(
        module_refusal(&["runs", "--limit", "0"]),
        (json!("--limit"), json!("out_of_range"))
    );
    assert_eq!(
        module_refusal(&["run", "evidence", RUN, "--from-step", "4294967296"]),
        (json!("--from-step"), json!("out_of_range"))
    );
    assert_eq!(
        module_refusal(&["gap", "--capability", "x", "--why", "a\u{7}b"]),
        (json!("--why"), json!("not_recognised"))
    );

    // The rescue module's rows (offered on Windows, where it runs); the flags they pass on are
    // `crates/lab-rescue`'s `gate_434_lab5a`.
    if cfg!(windows) {
        let rescue: [(&[&str], Value); 9] = [
            (
                &["op", "status", "--check-rollback"],
                json!({"check_rollback": true}),
            ),
            (
                &["release", "verify", "release", "--facts", "facts.json"],
                json!({"dir": "release", "facts": "facts.json"}),
            ),
            (
                &["release", "stage", "release", "--local-build"],
                json!({"dir": "release", "local_build": true}),
            ),
            (
                &[
                    "agent",
                    "add",
                    "--name",
                    "n1",
                    "--client",
                    "neutral",
                    "--native-id",
                    "id1",
                ],
                json!({"name": "n1", "client": "neutral", "native_id": "id1"}),
            ),
            (&["agent", "remove", "--name", "n1"], json!({"name": "n1"})),
            (&["agent", "list"], json!({})),
            (
                &["agent", "claude-hook", "start"],
                json!({"action": "start"}),
            ),
            (
                &["agent", "claude-register", "--name", "n1"],
                json!({"name": "n1"}),
            ),
            (&["agent", "notice"], json!({})),
        ];
        for (line, params) in rescue {
            module_route(line);
            assert_eq!(parsed(line), params, "{line:?}");
            let with_flag = [line, &["--frobnicate", "1"][..]].concat();
            assert_eq!(
                refused(&with_flag),
                (json!("--frobnicate"), unknown.clone()),
                "{with_flag:?}"
            );
        }
        assert_eq!(
            refused(&["agent", "claude-hook", "begin"]),
            (json!("begin"), json!("not_recognised"))
        );
    }

    // `serve --stdio` refuses a parameter the row does not declare, the lenient rows' too, and
    // takes the flags they passed through before as their declared parameters.
    for request in [
        json!({"op": "watch", "params": {"frobnicate": 1}}),
        json!({"op": "runs", "params": {"frobnicate": "1"}}),
        json!({"op": "recover", "params": {"instance": "neutral.a", "frobnicate": "x"}}),
        json!({"op": "debug-package", "params": {"package": "pack.zip", "frobnicate": true}}),
        json!({"op": "export-evidence", "params": {"out": "evidence.zip", "frobnicate": "1"}}),
        json!({"op": "recover", "params": {"instance": "neutral.a", "words": ["extra"]}}),
    ] {
        let answer = served(request.clone()).expect_err("an undeclared member is refused");
        assert_eq!(answer.1, unknown, "{request}");
    }
    assert_eq!(
        served(json!({"op": "watch", "params": {"after": 3, "event_type": "a.b"}})),
        Ok(json!({"after": 3, "event_type": "a.b"}))
    );
    // A seed above the signed range is taken and handed on as written; a negative or fractional
    // one is refused.
    let seed = json!({"instance": "neutral.a", "seed": 18_446_744_073_709_551_615_u64});
    let params = served(json!({"op": "recover", "params": seed.clone()})).expect("the seed");
    assert_eq!(params, seed);
    let recover = table.operation("recover").expect("recover");
    let Value::Object(members) = params else {
        panic!("an object");
    };
    assert_eq!(
        engine::arguments(recover, &members),
        Some(words_of(&[
            "session",
            "recover",
            "--instance",
            "neutral.a",
            "--seed",
            "18446744073709551615"
        ]))
    );
    assert!(
        served(json!({"op": "do", "params": {"seed": 18_446_744_073_709_551_615_u64}})).is_ok()
    );
    for seed in [json!(-1), json!(1.5), json!("1")] {
        assert_eq!(
            served(json!({"op": "do", "params": {"seed": seed}})),
            Err((json!("seed"), json!("wrong_type")))
        );
    }
    // A decimal: a JSON number, or its text as the command line gives it; handed on as written.
    for threshold in [json!(0.85), json!("0.85")] {
        let params = served(json!({"op": "record amend", "params": {"threshold": threshold}}))
            .expect("a decimal");
        let Value::Object(members) = params else {
            panic!("an object");
        };
        assert_eq!(
            engine::arguments(table.operation("record amend").expect("a row"), &members),
            Some(words_of(&["record", "amend", "--threshold", "0.85"]))
        );
    }
    for threshold in [json!("high"), json!("NaN"), json!(true)] {
        assert_eq!(
            served(json!({"op": "record amend", "params": {"threshold": threshold}})),
            Err((json!("threshold"), json!("wrong_type")))
        );
    }
    // The ids that take a request_id or a run_id (`run_ref`).
    assert_eq!(
        served(json!({"op": "run show", "params": {"id": RUN}})),
        Ok(json!({"id": RUN}))
    );
}
