// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-5B1 (register §1.287, "Slice LAB-5B1"; coordinator work order
// rd-1113/brief-lab5b1.md): the table-driven rows. lab-ops' own tests (its `gate_434_lab5b1`) hold
// the constants of the rows `lab_ops::core_ops` serves to their rows and to the parsers they hand
// a command line; here:
// - `declared_rows_match_their_commands`: each row of `own_flags::DECLARED_ROWS`, whose command
//   line reaches actinglab's own command as its declared words (`engine::Line::Declared`), is held
//   equal to that command's read list in both directions: every parameter it declares is read in
//   the form `engine::arguments` writes it (the instance as `--instance`, another positional as a
//   word in its place, an option or a switch under the flag of its name), and every option, switch
//   and positional of the list is declared, but `own_flags::DECLARED_EXCEPTIONS`.
// - `every_table_driven_row_has_one_list`: every planned row of the core module, and every row
//   LAB-5B2's register names table-driven, is in exactly one list (`lab_ops::core_reads`'
//   `READ_HERE`, `HANDED` and `OFFLINE`, `own_flags::DECLARED_ROWS`, LAB-5B2's
//   `own_flags::CORE_STRICT_ROWS`); each list holds only such rows, once; `lab_ops::core_ops`
//   serves exactly the rows of its three. Workflow #434 LAB-5B1b: and every planned rescue row the
//   rescue module serves is in exactly one of its constants (`lab_rescue::rescue_reads`'
//   `READ_HERE`, `INSTALLER` and `AGENT`; on Windows, where the module runs), which hold only such
//   rows.
// - LAB-5C's cases its tests did not cover (the review of its pull request), as the code answers
//   them today, on the command line and in `serve --stdio`:
//   `lab5c_both_starting_points_are_refused_as_repeated` (`since` and `since_unix_ms` at once on
//   `events`, `runs` and `diagnose`), `lab5c_a_wait_outside_its_range_is_refused` (`events`'
//   `wait_s` above 20 and below 0) and `lab5c_parameters_reach_lab_ops_through_the_module` (each
//   of `events`' new parameters, `material --out` beside `--offset`, `packs --role`, through the
//   CLI module and not only through `core_ops`).
// Fakes only: the embedded table, a state root in a temporary folder on which no Runtime runs; no
// device. `--since` makes `core_ops` read the clock for the window's start it writes; the cases
// that give it are refused for the start given twice, whatever the clock reads.

use super::gate_434_lab5b2::flag;
use super::{RuntimeStateEnvGuard, env_lock, use_runtime_state_root};
use crate::cli_module::{self, core_ops, engine, own_flags, serve, words};
use actingcommand_lab_ops::core_reads;
use actingcommand_lab_ops::events;
use actingcommand_lab_ops::initiator::ENVIRONMENT_VARIABLE;
use actingcommand_lab_ops::table::{OperationModule, OperationStatus, ParamForm, ParamType};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::{env, fs};
use tempfile::TempDir;

/// Typed identifiers, as the Runtime spells them.
const TASK: &str = "task_0123456789abcdef0123456789abcdef";
const CORRELATION: &str = "correlation_0123456789abcdef0123456789abcdef";

fn words_of(line: &[&str]) -> Vec<String> {
    line.iter().map(|word| (*word).to_owned()).collect()
}

/// The module's exit code and line for the command line `line`.
fn module_line(line: &[&str]) -> (u8, Value) {
    let exit = cli_module::run_line(&words_of(line));
    (exit.code, exit.line.expect("the module's line"))
}

/// The line `serve --stdio` answers to `request`.
fn served_line(request: &Value) -> Value {
    let table = cli_module::table().expect("the operation table");
    let mut output = Vec::new();
    let mut developer = Vec::new();
    serve::serve(
        table,
        format!("{request}\n").as_bytes(),
        &mut output,
        &mut developer,
    )
    .expect("the loop ends");
    let output = String::from_utf8(output).expect("UTF-8 lines");
    serde_json::from_str(output.trim_end())
        .unwrap_or_else(|error| panic!("{request}: {error}: {output}"))
}

/// A refusal's code, `argument` and `argument_problem`.
fn refusal(line: &Value) -> (Value, Value, Value) {
    let outcome = &line["outcome"];
    (
        outcome["code"].clone(),
        outcome["values"]["argument"].clone(),
        outcome["values"]["argument_problem"].clone(),
    )
}

/// A state root in a temporary folder on which no Runtime runs, named for the module while it
/// lives (the variable is set back first, then the folder goes).
struct NoRuntime {
    _state: RuntimeStateEnvGuard,
    temp: TempDir,
}

fn no_runtime() -> NoRuntime {
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("state");
    fs::create_dir_all(&state_root).unwrap();
    NoRuntime {
        _state: use_runtime_state_root(&state_root),
        temp,
    }
}

/// A line that reached the Runtime's connection: no usage refusal, the connection's failure.
fn reached_the_connection(line: &[&str]) {
    let (_, answered) = module_line(line);
    assert_ne!(
        answered["outcome"]["code"], "actingctl_usage_invalid",
        "{line:?}: {answered}"
    );
    assert!(
        answered["outcome"]
            .to_string()
            .contains("\"runtime_info_unavailable\""),
        "{line:?}: {answered}"
    );
}

#[test]
fn declared_rows_match_their_commands() {
    let table = cli_module::table().expect("the operation table");
    let mut failures = Vec::new();
    for (name, list) in own_flags::DECLARED_ROWS {
        let operation = table
            .operation(name)
            .unwrap_or_else(|| panic!("{name} is a row of the table"));
        if !engine::COMMANDS
            .iter()
            .any(|(row, _, line)| *row == name && *line == engine::Line::Declared)
        {
            failures.push(format!(
                "{name}: its command line does not reach its command as its declared words"
            ));
        }
        let options = list.options();
        let switches = list.switches();
        let excepted = |written: &str| {
            own_flags::DECLARED_EXCEPTIONS
                .iter()
                .any(|(row, flag, _)| *row == list.row && *flag == written)
        };
        // Row to command, in the form `engine::arguments` writes each parameter.
        let mut positionals = Vec::new();
        for param in &operation.params {
            let written = flag(&param.name);
            let read = match param.form {
                ParamForm::Positional if param.value_type == ParamType::Instance => {
                    options.contains(&"--instance")
                }
                ParamForm::Positional => {
                    positionals.push(param.name.as_str());
                    continue;
                }
                ParamForm::Option => options.contains(&written.as_str()),
                ParamForm::Switch => switches.contains(&written.as_str()),
            };
            if !read {
                failures.push(format!(
                    "{name}: {}: row to command: the row declares it, {} does not read it so",
                    param.name, list.row
                ));
            }
        }
        // Command to row: every option and switch the list reads.
        for read in options.iter().chain(&switches) {
            let declared = operation.params.iter().any(|param| match param.form {
                ParamForm::Positional => {
                    param.value_type == ParamType::Instance && *read == "--instance"
                }
                _ => flag(&param.name) == *read,
            });
            if !declared && !excepted(read) {
                failures.push(format!(
                    "{name}: {read}: command to row: {} reads it, the row does not declare it",
                    list.row
                ));
            }
        }
        // The positionals, in their order.
        let listed = list
            .positionals
            .iter()
            .map(|word| word.trim_start_matches('<').trim_end_matches('>'))
            .collect::<Vec<_>>();
        if listed != positionals {
            failures.push(format!(
                "{name}: positionals: the row gives {positionals:?}, {} reads {listed:?}",
                list.row
            ));
        }
    }
    for (row, written, why) in own_flags::DECLARED_EXCEPTIONS {
        let read = own_flags::DECLARED_ROWS.iter().any(|(_, list)| {
            list.row == row
                && (list.options().contains(&written) || list.switches().contains(&written))
        });
        if !read || why.trim().is_empty() {
            failures.push(format!(
                "{row}: {written}: an exception no list reads, or without its reason"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "rows whose declared words differ from their command:\n{}",
        failures.join("\n")
    );
}

/// Workflow #434 LAB-5B1b: the rescue module's constants (`lab_rescue::rescue_reads`), each with
/// its rows; none where the module does not run.
#[cfg(windows)]
fn rescue_lists() -> Vec<(&'static str, Vec<&'static str>)> {
    use actingcommand_lab_rescue::rescue_reads;
    vec![
        (
            "lab_rescue::rescue_reads::READ_HERE",
            rescue_reads::READ_HERE
                .iter()
                .map(|reads| reads.row)
                .collect(),
        ),
        (
            "lab_rescue::rescue_reads::INSTALLER",
            rescue_reads::INSTALLER
                .iter()
                .map(|handed| handed.row)
                .collect(),
        ),
        (
            "lab_rescue::rescue_reads::AGENT",
            rescue_reads::AGENT
                .iter()
                .map(|handed| handed.row)
                .collect(),
        ),
    ]
}

#[cfg(not(windows))]
fn rescue_lists() -> Vec<(&'static str, Vec<&'static str>)> {
    Vec::new()
}

#[test]
fn every_table_driven_row_has_one_list() {
    let table = cli_module::table().expect("the operation table");
    let read_here = core_reads::READ_HERE
        .iter()
        .map(|reads| reads.row)
        .collect::<Vec<_>>();
    let handed = core_reads::HANDED
        .iter()
        .map(|handed| handed.row)
        .collect::<Vec<_>>();
    let offline = core_reads::OFFLINE
        .iter()
        .map(|handed| handed.row)
        .collect::<Vec<_>>();
    let declared = own_flags::DECLARED_ROWS
        .iter()
        .map(|(row, _)| *row)
        .collect::<Vec<_>>();
    let core_lists: [(&str, &[&str]); 5] = [
        ("lab_ops::core_reads::READ_HERE", &read_here),
        ("lab_ops::core_reads::HANDED", &handed),
        ("lab_ops::core_reads::OFFLINE", &offline),
        ("own_flags::DECLARED_ROWS", &declared),
        ("own_flags::CORE_STRICT_ROWS", &own_flags::CORE_STRICT_ROWS),
    ];
    // Workflow #434 LAB-5B1b: and the rescue module's constants, so that every row it serves is in
    // exactly one, and a row it serves later and no constant holds fails.
    let rescue = rescue_lists();
    let lists = core_lists
        .into_iter()
        .chain(rescue.iter().map(|(list, rows)| (*list, rows.as_slice())))
        .collect::<Vec<_>>();
    let rows = table
        .operations
        .iter()
        .filter(|operation| {
            operation.status == OperationStatus::Planned
                && (operation.module == OperationModule::Core
                    || own_flags::TABLE_DRIVEN_ROWS.contains(&operation.name.as_str())
                    || (operation.module == OperationModule::Rescue
                        && words::rescue_served(&operation.name)))
        })
        .collect::<Vec<_>>();
    let mut failures = Vec::new();
    for operation in &rows {
        let name = operation.name.as_str();
        let found = lists
            .iter()
            .filter(|(_, listed)| listed.contains(&name))
            .map(|(list, _)| *list)
            .collect::<Vec<_>>();
        if found.len() != 1 {
            failures.push(format!(
                "{name}: in the lists {found:?}, not in exactly one"
            ));
        }
        let constants = [&read_here, &handed, &offline]
            .iter()
            .any(|listed| listed.contains(&name));
        if core_ops::serves(operation) != constants {
            failures.push(format!(
                "{name}: lab_ops::core_ops serves it: {}, a constant of lab_ops::core_reads: \
                 {constants}",
                core_ops::serves(operation)
            ));
        }
    }
    for (list, listed) in lists {
        let mut seen = BTreeSet::new();
        for row in listed {
            if !seen.insert(*row) {
                failures.push(format!("{list} lists {row} twice"));
            }
            if !rows.iter().any(|operation| operation.name == *row) {
                failures.push(format!(
                    "{list} lists {row}, which is no planned core row and no table-driven row"
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "table-driven rows out of their lists:\n{}",
        failures.join("\n")
    );
}

#[test]
fn lab5c_both_starting_points_are_refused_as_repeated() {
    let _guard = env_lock();
    let _state = no_runtime();
    // `since`, a duration back from now, goes on as `--since-unix-ms` before `since_unix_ms` goes
    // on under the same flag; the operation layer refuses the second before any connection.
    for row in ["events", "runs", "diagnose"] {
        let line = [row, "--since", "1h", "--since-unix-ms", "5"];
        let (code, answered) = module_line(&line);
        assert_eq!(
            (code, refusal(&answered)),
            (
                1,
                (
                    json!("actingctl_usage_invalid"),
                    json!("--since-unix-ms"),
                    json!("repeated")
                )
            ),
            "{line:?}: {answered}"
        );
        let served =
            served_line(&json!({"op": row, "params": {"since": "1h", "since_unix_ms": 5}}));
        assert_eq!(refusal(&served), refusal(&answered), "{row}: {served}");
    }
}

#[test]
fn lab5c_a_wait_outside_its_range_is_refused() {
    let _guard = env_lock();
    let _state = no_runtime();
    // Above its most: refused by `events`' read, which names the parameter as MCP names it, with
    // its range (the command line takes any integer for it).
    let (code, answered) = module_line(&["events", "--wait-s", "21"]);
    let outcome = &answered["outcome"];
    assert_eq!(
        (
            code,
            refusal(&answered),
            outcome["values"]["range_min"].clone(),
            outcome["values"]["range_max"].clone()
        ),
        (
            1,
            (
                json!("actingctl_usage_invalid"),
                json!("wait_s"),
                json!("out_of_range")
            ),
            json!(0),
            json!(events::MAX_WAIT_S)
        ),
        "{answered}"
    );
    let served = served_line(&json!({"op": "events", "params": {"wait_s": 21}}));
    assert_eq!(served["outcome"], answered["outcome"], "{served}");
    // Below its least: the flag's value is no unsigned integer, refused as of the wrong type,
    // naming the flag (today's MCP server answers `out_of_range` for it).
    let (code, answered) = module_line(&["events", "--wait-s", "-1"]);
    assert_eq!(
        (code, refusal(&answered)),
        (
            1,
            (
                json!("actingctl_usage_invalid"),
                json!("--wait-s"),
                json!("wrong_type")
            )
        ),
        "{answered}"
    );
    let served = served_line(&json!({"op": "events", "params": {"wait_s": -1}}));
    assert_eq!(served["outcome"], answered["outcome"], "{served}");
}

#[test]
fn lab5c_parameters_reach_lab_ops_through_the_module() {
    let _guard = env_lock();
    let state = no_runtime();
    // Each parameter `events` gained goes through the CLI module to `lab-ops`, whose read takes
    // it, and the line goes on to the Runtime, which the state root does not hold.
    let lines: [&[&str]; 14] = [
        &["events", "--view", "errors"],
        &["events", "--since-unix-ms", "1000"],
        &["events", "--until-unix-ms", "2000"],
        &["events", "--through-sequence", "9"],
        &["events", "--event-type", "runtime.failed"],
        &[
            "events",
            "--exclude-event-types",
            "runtime.failed,fact.published",
        ],
        &["events", "--fact-key", "application.foreground"],
        &["events", "--min-severity", "warning"],
        &["events", "--max-severity", "error"],
        &["events", "--origin-module", "actingctl"],
        &["events", "--task-id", TASK],
        &["events", "--correlation-id", CORRELATION],
        &["events", "--detail", "summary"],
        &["events", "--wait-s", "3"],
    ];
    for line in lines {
        reached_the_connection(line);
    }
    // `material --out` beside `--offset`: read after the connection is made (LAB-5C fix 1, N1),
    // so with no Runtime the line is the connection's failure, not `conflicting`.
    let out = state.temp.path().join("exports");
    let out = out.to_str().expect("a UTF-8 path");
    reached_the_connection(&["material", "1:2:3", "--out", out, "--offset", "5"]);
    // `packs --role` reaches `packs`, offline, which takes the role: whatever it answers here, it
    // refuses no `--role`.
    let (_, answered) = module_line(&["packs", "--role", "startup"]);
    assert_ne!(
        answered["outcome"]["values"]["argument"], "--role",
        "{answered}"
    );
}
