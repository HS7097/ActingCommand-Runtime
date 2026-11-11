// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-5B2 (register §1.287, "Slice LAB-5B2"; coordinator work order
// rd-1108/arch-013/brief-lab5b2.md): the flag list of each strict command of the debugging and
// authoring modules is one constant its check uses, and the operation table's row says what it
// accepts.
// - `strict_rows_match_their_commands`: for each strict row (`own_flags::STRICT_ROWS`), in both
//   directions: every option and switch the row declares is an option or a switch of its command
//   (`own_flags::accepted`, read from the command's constants), but actinglab's global options,
//   which actinglab's parser takes before the command sees its flags; every flag of the command is
//   declared by the row in the same form, a global option in any form (the instance as the row's
//   positional) and a flag the command reads in place of a positional as that positional; and the
//   row's positionals (the instance, handed on as `--instance`, not among them) fit the count the
//   command checks. A failure names the row, the flag and the direction.
// - `every_debugging_and_authoring_row_has_one_kind`: every planned row of the two modules (every
//   row actinglab offers there, and `record draft`, which LAB-5 offers) is in exactly one of the
//   four kinds (strict, lenient, table-driven, no command), each kind lists only such rows, once,
//   and the register answers for exactly the strict rows.
// The operation table this build embeds and the code's constants only: no Runtime, no device, no
// file, no clock.

use crate::cli_module::{self, own_flags, words};
use actingcommand_lab_ops::table::{
    Operation, OperationModule, OperationStatus, ParamForm, ParamType,
};
use std::collections::BTreeSet;

/// A parameter's flag as the command line writes it.
fn flag(name: &str) -> String {
    format!("--{}", name.replace('_', "-"))
}

/// Whether `flag` is one of actinglab's global options, which its parser takes anywhere on the
/// command line before the command sees its flags (`words::GLOBAL_OPTIONS`).
fn global(flag: &str) -> bool {
    words::GLOBAL_OPTIONS.contains(&flag)
}

/// The positional words `operation`'s own command is given: every positional of the row but the
/// instance, which the command line hands on as actinglab's global `--instance`
/// (`engine::arguments`).
fn row_positionals(operation: &Operation) -> own_flags::Positionals {
    let mut least = 0;
    let mut most = Some(0);
    for param in operation.params.iter().filter(|param| {
        param.form == ParamForm::Positional && param.value_type != ParamType::Instance
    }) {
        if param.required {
            least += 1;
        }
        most = if param.repeat {
            None
        } else {
            most.map(|count| count + 1)
        };
    }
    own_flags::Positionals { least, most }
}

#[test]
fn strict_rows_match_their_commands() {
    let table = cli_module::table().expect("the operation table");
    let mut failures = Vec::new();
    for name in own_flags::STRICT_ROWS {
        let operation = table
            .operation(name)
            .unwrap_or_else(|| panic!("{name} is a row of the table"));
        let accepted =
            own_flags::accepted(name).unwrap_or_else(|| panic!("the register answers {name}"));

        // Row to command: every option and switch the row declares, but a global option.
        for param in &operation.params {
            let written = flag(&param.name);
            let (form, taken) = match param.form {
                ParamForm::Positional => continue,
                _ if global(&written) => continue,
                ParamForm::Option => ("option", &accepted.options),
                ParamForm::Switch => ("switch", &accepted.switches),
            };
            if !taken.contains(&written.as_str()) {
                failures.push(format!(
                    "{name}: {written}: row to command: the row declares it as a {form}, its \
                     command does not take it as one"
                ));
            }
        }

        // Command to row: every flag of the command's constants.
        let flags = accepted
            .options
            .iter()
            .map(|written| (*written, ParamForm::Option))
            .chain(
                accepted
                    .switches
                    .iter()
                    .map(|written| (*written, ParamForm::Switch)),
            );
        for (written, form) in flags {
            let declared = operation
                .params
                .iter()
                .find(|param| flag(&param.name) == written);
            let fits = match declared {
                None => false,
                Some(_) if global(written) => true,
                Some(param) if accepted.for_positionals.contains(&written) => {
                    param.form == ParamForm::Positional
                }
                Some(param) => param.form == form,
            };
            if !fits {
                failures.push(format!(
                    "{name}: {written}: command to row: its command takes it as {form:?}, the row \
                     declares {:?}",
                    declared.map(|param| param.form)
                ));
            }
        }

        // The positionals: the least the command checks, and its most where it checks one.
        let given = row_positionals(operation);
        let checked = accepted.positionals;
        if given.least != checked.least || checked.most.is_some_and(|most| given.most != Some(most))
        {
            failures.push(format!(
                "{name}: positionals: the row gives its command {given:?}, the command checks \
                 {checked:?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "strict rows that differ from their command:\n{}",
        failures.join("\n")
    );
}

#[test]
fn every_debugging_and_authoring_row_has_one_kind() {
    let table = cli_module::table().expect("the operation table");
    let kinds: [(&str, &[&str]); 4] = [
        ("strict", &own_flags::STRICT_ROWS),
        ("lenient", &own_flags::LENIENT_ROWS),
        ("table-driven", &own_flags::TABLE_DRIVEN_ROWS),
        ("no command", &own_flags::NO_COMMAND_ROWS),
    ];
    let planned = table
        .operations
        .iter()
        .filter(|operation| {
            matches!(
                operation.module,
                OperationModule::Debugging | OperationModule::Authoring
            ) && operation.status == OperationStatus::Planned
        })
        .collect::<Vec<_>>();
    let mut failures = Vec::new();
    for operation in &planned {
        let name = operation.name.as_str();
        // Every row actinglab offers is planned (`words::offered`).
        let found = kinds
            .iter()
            .filter(|(_, rows)| rows.contains(&name))
            .map(|(kind, _)| *kind)
            .collect::<Vec<_>>();
        if found.len() != 1 {
            failures.push(format!(
                "{name}: in the kinds {found:?}, not in exactly one (offered: {})",
                words::offered(operation)
            ));
        }
        let strict = own_flags::STRICT_ROWS.contains(&name);
        if own_flags::accepted(name).is_some() != strict {
            failures.push(format!(
                "{name}: the register answers it: {}, a strict row: {strict}",
                own_flags::accepted(name).is_some()
            ));
        }
    }
    for (kind, rows) in kinds {
        let mut listed = BTreeSet::new();
        for row in rows {
            if !listed.insert(row) {
                failures.push(format!("{kind} lists {row} twice"));
            }
            if !planned.iter().any(|operation| operation.name == *row) {
                failures.push(format!(
                    "{kind} lists {row}, which is no planned row of the debugging or authoring \
                     module"
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "rows of the debugging and authoring modules out of their kinds:\n{}",
        failures.join("\n")
    );
}
