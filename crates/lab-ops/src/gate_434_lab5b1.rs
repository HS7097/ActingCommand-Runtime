// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-5B1 (register §1.287, "Slice LAB-5B1"; coordinator work order
//! rd-1113/brief-lab5b1.md): the parameters the code of each row `core_ops` serves reads are one
//! constant per row (`core_reads`), which that code reads them through, held equal to the row.
//! - `lab_ops_read_here_rows_read_what_they_declare`: each row of `core_reads::READ_HERE` is a
//!   planned row `core_ops` serves, and its constant holds exactly the row's declared parameters:
//!   every name its code reads is declared, every declared parameter is read.
//! - `lab_ops_handed_rows_hand_what_they_declare`: the same for the rows whose answer is a command
//!   line of this crate (`HANDED`, `OFFLINE`): the parameters the command line carries are the
//!   row's declared ones; a parameter written as LAB-5A's options were is written under the flag
//!   of its name; an offline row's command line has no state root and reads no clock.
//! - `lab_ops_handed_flags_are_what_the_parsers_take`: the flags each such command line can carry,
//!   with the module's own `--state-root` and `--root`, are the flags its parser takes
//!   (`flags_of`, or the list lifted from a parser it does not cover), but the exceptions.
//! - `lab_ops_parsers_refuse_no_declared_parameter`: each parser, fed the command line the row
//!   builds for each of its declared parameters (a made-up value of the declared type, beside the
//!   row's required ones), refuses none of them as `unknown`.
//! - `lab_ops_exceptions_are_outside_the_table`: no row an exception names declares its name.
//! - `lab_ops_a_read_outside_its_constant_fails`: a read of a name a row's constant does not hold
//!   fails the debug assertion test builds have; a declared one and an exception do not.
//!
//! A failure names the row, the parameter or flag and the direction. The embedded table and the
//! code's constants only: no Runtime, no file, no device.

use crate::core_ops::{self, MORE_WORDS, handed_words, offline_words};
use crate::core_reads::{
    self, EXCEPTIONS, HANDED, Handed, OFFLINE, Params, Part, READ_HERE, Written,
};
use crate::table::{Operation, OperationStatus, OperationTable, Param, ParamType};
use crate::{
    capability_gap, code_lookup, devices, diagnose, events, evidence_read, observe_export,
    overview, packs, perf_preset, remedy, routine_summary, run_evidence, run_list,
};
use actingcommand_contract::outcome::Outcome;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::Path;

/// The state root and the install root the command lines name; nothing reads them.
const STATE: &str = "state";
const ROOT: &str = "root";
/// A request id, as the Runtime spells one.
const REQUEST: &str = "request_0123456789abcdef0123456789abcdef";

fn table() -> OperationTable {
    crate::table::operation_table().expect("the operation table")
}

/// A parameter's flag as the command line writes it.
fn flag(name: &str) -> String {
    format!("--{}", name.replace('_', "-"))
}

/// The planned row `name`, which `core_ops` serves; a failure otherwise.
fn served_row<'t>(
    table: &'t OperationTable,
    name: &str,
    failures: &mut Vec<String>,
) -> Option<&'t Operation> {
    match table.operation(name) {
        Some(operation)
            if operation.status == OperationStatus::Planned && core_ops::serves(operation) =>
        {
            Some(operation)
        }
        Some(_) => {
            failures.push(format!("{name}: no planned row core_ops serves"));
            None
        }
        None => {
            failures.push(format!("{name}: no row of the table"));
            None
        }
    }
}

/// The names a row's constant holds against the row's declared parameters, both directions.
fn compare(operation: &Operation, names: &[&str], failures: &mut Vec<String>) {
    let row = operation.name.as_str();
    let mut seen = BTreeSet::new();
    for name in names {
        if !seen.insert(*name) {
            failures.push(format!("{row}: {name}: its constant holds it twice"));
        }
        if !operation.params.iter().any(|param| param.name == *name) {
            failures.push(format!(
                "{row}: {name}: code to row: its code reads it, the row does not declare it"
            ));
        }
    }
    for param in &operation.params {
        if !names.contains(&param.name.as_str()) {
            failures.push(format!(
                "{row}: {}: row to code: the row declares it, its code does not read it",
                param.name
            ));
        }
    }
}

fn handed_rows() -> impl Iterator<Item = &'static Handed> {
    HANDED.iter().chain(&OFFLINE)
}

#[test]
fn lab_ops_read_here_rows_read_what_they_declare() {
    let table = table();
    let mut failures = Vec::new();
    for reads in &READ_HERE {
        if let Some(operation) = served_row(&table, reads.row, &mut failures) {
            compare(operation, reads.names, &mut failures);
        }
    }
    assert!(
        failures.is_empty(),
        "rows read in this process that differ from their constant:\n{}",
        failures.join("\n")
    );
}

#[test]
fn lab_ops_handed_rows_hand_what_they_declare() {
    let table = table();
    let mut failures = Vec::new();
    for handed in handed_rows() {
        let row = handed.row;
        let Some(operation) = served_row(&table, row, &mut failures) else {
            continue;
        };
        let names = handed.params().map(|(name, _)| name).collect::<Vec<_>>();
        compare(operation, &names, &mut failures);
        if let Some(only_with) = handed.only_with
            && !names.contains(&only_with)
        {
            failures.push(format!(
                "{row}: {only_with}: hands words only with it, carries it not"
            ));
        }
        // LAB-5A's options went on under the flag of their name (`options_of`, before the slice).
        for (name, written) in handed.params() {
            if let Written::Each(written) = written
                && written != flag(name)
            {
                failures.push(format!(
                    "{row}: {name}: written as {written}, not as {}",
                    flag(name)
                ));
            }
        }
    }
    for handed in &OFFLINE {
        for part in handed.parts {
            if matches!(part, Part::StateRoot | Part::Param(_, Written::SinceNow(_))) {
                failures.push(format!(
                    "{}: an offline row's command line has {part:?}",
                    handed.row
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "rows handed to a command of lab-ops that differ from their constant:\n{}",
        failures.join("\n")
    );
}

/// The flags the command line of `handed` can carry.
fn handed_flags(handed: &Handed) -> BTreeSet<&'static str> {
    let mut flags = BTreeSet::new();
    for part in handed.parts {
        match *part {
            Part::Param(
                _,
                Written::Flag(written)
                | Written::Switch(written)
                | Written::Joined(written)
                | Written::Each(written)
                | Written::SinceNow(written),
            ) => {
                flags.insert(written);
            }
            Part::Param(_, Written::RunOrRequest(run, request)) => {
                flags.insert(run);
                flags.insert(request);
            }
            Part::Param(_, Written::Word | Written::Beside) | Part::Further => {}
            Part::StateRoot => {
                flags.insert(core_reads::STATE_ROOT_FLAG);
            }
            Part::Root => {
                flags.insert(core_reads::ROOT_FLAG);
            }
        }
    }
    flags
}

/// The flags the parser of `handed`'s command takes: `flags_of`'s, or the list lifted from a
/// parser `flags_of` does not cover.
fn parser_flags(handed: &Handed) -> &'static [&'static str] {
    let command = handed.command[0];
    if let Some(flags) = crate::flags_of(command, handed.command.get(1).copied()) {
        return flags;
    }
    match command {
        "overview" => &overview::FLAGS,
        "diagnose" => &diagnose::FLAGS,
        "runs" => &run_list::FLAGS,
        "observe" => &observe_export::FLAGS,
        "packs" => &packs::FLAGS,
        other => panic!("{}: no flag list of the command {other}", handed.row),
    }
}

#[test]
fn lab_ops_handed_flags_are_what_the_parsers_take() {
    let mut failures = Vec::new();
    for handed in handed_rows() {
        let row = handed.row;
        let written = handed_flags(handed);
        let taken = parser_flags(handed)
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        for written in &written {
            if !taken.contains(written) {
                failures.push(format!(
                    "{row}: {written}: row to parser: its command line carries it, the parser of \
                     {:?} does not take it",
                    handed.command
                ));
            }
        }
        for taken in &taken {
            let excepted = EXCEPTIONS
                .iter()
                .any(|exception| exception.rows.contains(&row) && flag(exception.name) == *taken);
            if !written.contains(taken) && !excepted {
                failures.push(format!(
                    "{row}: {taken}: parser to row: the parser of {:?} takes it, the row's command \
                     line never carries it",
                    handed.command
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "command lines that differ from their parser:\n{}",
        failures.join("\n")
    );
}

/// A made-up value of `param`'s declared type (a list of one for a repeated one).
fn made_up(param: &Param) -> Value {
    let value = match &param.value_type {
        ParamType::Instance => json!("neutral.a"),
        ParamType::Integer | ParamType::Unsigned => json!(1),
        ParamType::Boolean => json!(true),
        ParamType::Duration => json!("1s"),
        ParamType::ListOfStrings => json!(["neutral.a"]),
        ParamType::Enum(words) => json!(words[0]),
        ParamType::RequestId | ParamType::RunRef => json!(REQUEST),
        ParamType::Code => json!("actingctl_usage_invalid"),
        ParamType::Json => json!([]),
        _ => json!("neutral"),
    };
    if param.repeat { json!([value]) } else { value }
}

/// The refusal of `argv` by the parser of its command, if any (`inline`: `remedy set`'s rows go
/// beside the words).
fn parser_refusal(argv: &[OsString], inline: bool) -> Option<Outcome> {
    match argv[0].to_str().expect("UTF-8 words") {
        "codes" => code_lookup::parse_refusal(argv),
        capability_gap::COMMAND => capability_gap::parse_refusal(argv),
        "packs" => packs::parse_refusal(argv),
        "overview" => overview::parse_refusal(argv),
        routine_summary::COMMAND => routine_summary::parse_refusal(argv),
        events::COMMAND => events::cli_request(&argv[1..])
            .err()
            .map(|error| error.outcome()),
        "runs" => run_list::parse_refusal(argv),
        run_evidence::COMMAND => run_evidence::parse_refusal(argv),
        "diagnose" => diagnose::parse_refusal(argv),
        remedy::COMMAND => remedy::parse_refusal(argv, inline),
        "observe" => observe_export::parse_refusal(argv),
        perf_preset::COMMAND => perf_preset::parse_refusal(argv),
        evidence_read::COMMAND => evidence_read::parse_refusal(argv),
        devices::COMMAND => devices::parse_refusal(argv),
        other => panic!("no parser of the command {other}"),
    }
}

#[test]
fn lab_ops_parsers_refuse_no_declared_parameter() {
    let table = table();
    let mut failures = Vec::new();
    for handed in handed_rows() {
        let row = handed.row;
        let operation = table
            .operation(row)
            .unwrap_or_else(|| panic!("{row} is a row of the table"));
        // What every line of the row carries: its required parameters, and the one without which
        // it hands no words (`frame`'s `out`).
        let base = operation
            .params
            .iter()
            .filter(|param| param.required || handed.only_with == Some(param.name.as_str()))
            .map(|param| (param.name.clone(), made_up(param)))
            .collect::<Map<String, Value>>();
        for given in std::iter::once(None).chain(operation.params.iter().map(Some)) {
            let mut params = base.clone();
            if let Some(param) = given {
                params.insert(param.name.clone(), made_up(param));
            }
            let named = given.map_or("(none but the required)", |param| param.name.as_str());
            let argv = match offline_words(operation, &params, Some(Path::new(ROOT))) {
                Some(argv) => argv,
                None => {
                    match handed_words(operation, &params, Some(Path::new(ROOT)), Path::new(STATE))
                    {
                        Some(Ok(argv)) => argv,
                        Some(Err(exit)) => {
                            failures.push(format!(
                                "{row}: {named}: its words are refused: {:?}",
                                exit.line
                            ));
                            continue;
                        }
                        None => {
                            failures.push(format!("{row}: {named}: the row hands no words"));
                            continue;
                        }
                    }
                }
            };
            let inline = handed
                .params()
                .any(|(name, written)| written == Written::Beside && params.contains_key(name));
            let Some(refusal) = parser_refusal(&argv, inline) else {
                continue;
            };
            if refusal.value("argument_problem").and_then(Value::as_str) == Some("unknown") {
                failures.push(format!(
                    "{row}: {named}: its parser refuses {argv:?} as unknown, naming {}",
                    refusal
                        .value("argument")
                        .map_or_else(String::new, Value::to_string)
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "declared parameters their parser refuses:\n{}",
        failures.join("\n")
    );
}

#[test]
fn lab_ops_exceptions_are_outside_the_table() {
    let table = table();
    let mut failures = Vec::new();
    for exception in &EXCEPTIONS {
        let rows = table
            .operations
            .iter()
            .filter(|operation| {
                if exception.rows.is_empty() {
                    operation.status == OperationStatus::Planned && core_ops::serves(operation)
                } else {
                    exception.rows.contains(&operation.name.as_str())
                }
            })
            .collect::<Vec<_>>();
        for operation in &rows {
            if operation
                .params
                .iter()
                .any(|param| param.name == exception.name)
            {
                failures.push(format!(
                    "{}: {}: an exception, and the row declares it",
                    operation.name, exception.name
                ));
            }
        }
        if rows.len() < exception.rows.len().max(1) {
            failures.push(format!(
                "{}: the exception names rows the table does not have: {:?}",
                exception.name, exception.rows
            ));
        }
        if exception.why.trim().is_empty() {
            failures.push(format!(
                "{}: an exception without its reason",
                exception.name
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
#[should_panic(expected = "the code of the row reset reads neutral, which its constant")]
fn lab_ops_a_read_outside_its_constant_fails() {
    if !cfg!(debug_assertions) {
        panic!(
            "Workflow #434 LAB-5B1: a read outside a row's constant fails a debug assertion, which \
             this build does not assert"
        );
    }
    let params = Map::new();
    let reset = Params::of("reset", &params);
    // A declared parameter, and an exception every row may read.
    assert_eq!(reset.get("instance"), None);
    assert_eq!(reset.get(MORE_WORDS), None);
    // A name the row does not declare.
    assert_eq!(reset.get("neutral"), None);
}
