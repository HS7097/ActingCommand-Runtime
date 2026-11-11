// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-5B3 (register §1.287, "Slice LAB-5B3"; coordinator work order
// rd-1108/arch-013/brief-lab5b3.md): each lenient command of the debugging and authoring modules
// declares the flags it reads where it starts, its test builds hold every read to that list, and
// the operation table's row says what it reads.
// - `lenient_rows_match_their_reads`: for each lenient row (`own_flags::LENIENT_ROWS`), in both
//   directions: every option and switch the row declares, but actinglab's global options, is on
//   its command's read list in the same form; every option and switch of the list is declared by
//   the row in the same form, a global option in any form; no flag the command reads only to
//   refuse it is a parameter of the row; the option the command also takes as its first
//   positional is an option of the row; the row's positionals, the instance not among them, fit
//   the count the command checks; and the register answers for exactly the lenient rows, each
//   list naming its row. A failure names the row, the flag and the direction.
// - `a_held_read_outside_its_list_fails`: flags read as before when no list holds them; once held,
//   every read of them and of their copies is recorded with its form, and a name outside the list,
//   or read in the form the list does not give it, fails with the row and the name; a strict row
//   held to its register's forms fails on an option read as a switch and a switch read for its
//   value, and leaves another name to its command's own refusal.
// - `every_lenient_command_reads_through_its_list`: each of the 24 commands, and the six strict
//   ones whose switch lists the recording checks (`observe`, `do`, `ensure`, `wait`, `package fmt`,
//   `package loading`), on a command line answered before any Runtime connection, refused but
//   `record status`, records the reads that come first under its row: the declaration is where the
//   command starts.
// No Runtime, no device: the embedded operation table, the code's lists and temporary files.

use super::gate_434_lab5b2::{flag, global, row_positionals};
use super::{env_lock, set_missing_config_env, use_runtime_state_root};
use crate::cli_module::{self, own_flags};
use crate::flag_args::FlagArgs;
use crate::flag_args::reads::{self, FlagReads, Form, Read, ReadPart};
use crate::run_cli;
use actingcommand_lab_ops::table::{OperationModule, OperationStatus, ParamForm};
use std::panic::{AssertUnwindSafe, catch_unwind};
use tempfile::TempDir;

#[test]
fn lenient_rows_match_their_reads() {
    let table = cli_module::table().expect("the operation table");
    let mut failures = Vec::new();
    for name in own_flags::LENIENT_ROWS {
        let operation = table
            .operation(name)
            .unwrap_or_else(|| panic!("{name} is a row of the table"));
        let reads = own_flags::reads(name).unwrap_or_else(|| panic!("the register answers {name}"));
        if reads.list.row != name {
            failures.push(format!(
                "{name}: its command's read list names the row {}",
                reads.list.row
            ));
        }

        // Row to command: every option and switch the row declares, but a global option.
        for param in &operation.params {
            let written = flag(&param.name);
            let (form, read) = match param.form {
                ParamForm::Positional => continue,
                _ if global(&written) => continue,
                ParamForm::Option => ("option", &reads.options),
                ParamForm::Switch => ("switch", &reads.switches),
            };
            if !read.contains(&written.as_str()) {
                failures.push(format!(
                    "{name}: {written}: row to command: the row declares it as a {form}, its \
                     command does not read it as one"
                ));
            }
        }

        // Command to row: every option and switch of the read list.
        let flags = reads
            .options
            .iter()
            .map(|written| (*written, ParamForm::Option))
            .chain(
                reads
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
                Some(param) => param.form == form,
            };
            if !fits {
                failures.push(format!(
                    "{name}: {written}: command to row: its command reads it as {form:?}, the row \
                     declares {:?}",
                    declared.map(|param| param.form)
                ));
            }
        }

        // A flag the command reads only to refuse it is no parameter of the row.
        for written in &reads.refused {
            if let Some(param) = operation
                .params
                .iter()
                .find(|param| flag(&param.name) == *written)
            {
                failures.push(format!(
                    "{name}: {written}: command to row: its command refuses it, the row declares \
                     it as {:?}",
                    param.form
                ));
            }
        }

        // The positionals: the option the first one stands for, and the count the command checks.
        if let Some(written) = reads.first_positional {
            let declared = operation
                .params
                .iter()
                .any(|param| param.form == ParamForm::Option && flag(&param.name) == written);
            if !declared || !reads.options.contains(&written) {
                failures.push(format!(
                    "{name}: {written}: positionals: its command reads its first positional in \
                     place of it, an option of the row: {declared}, of its read list: {}",
                    reads.options.contains(&written)
                ));
            }
        }
        let given = row_positionals(operation);
        let checked = reads.positionals;
        if given.least != checked.least || checked.most.is_some_and(|most| given.most != Some(most))
        {
            failures.push(format!(
                "{name}: positionals: the row gives its command {given:?}, the command checks \
                 {checked:?}"
            ));
        }
    }

    // The register answers for exactly the lenient rows of the two modules.
    for operation in table.operations.iter().filter(|operation| {
        matches!(
            operation.module,
            OperationModule::Debugging | OperationModule::Authoring
        ) && operation.status == OperationStatus::Planned
    }) {
        let name = operation.name.as_str();
        let lenient = own_flags::LENIENT_ROWS.contains(&name);
        if own_flags::reads(name).is_some() != lenient {
            failures.push(format!(
                "{name}: the register answers its reads: {}, a lenient row: {lenient}",
                own_flags::reads(name).is_some()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "lenient rows that differ from their command's reads:\n{}",
        failures.join("\n")
    );
}

/// The list the recording test holds its flags to.
const PROBE_READS: FlagReads = FlagReads {
    row: "probe",
    parts: &[&ReadPart {
        options: &["--value"],
        switches: &["--switch"],
        refused: &["--refused"],
    }],
};

fn words(line: &[&str]) -> Vec<String> {
    line.iter().map(|word| (*word).to_owned()).collect()
}

/// The message of the failure `read` raises; fails the test when it raises none.
fn failure<T>(read: impl FnOnce() -> T) -> String {
    let payload = catch_unwind(AssertUnwindSafe(|| {
        read();
    }))
    .expect_err("the read fails the test");
    payload
        .downcast_ref::<String>()
        .cloned()
        .expect("a formatted failure")
}

#[test]
fn a_held_read_outside_its_list_fails() {
    let flags = FlagArgs::parse(&words(&[
        "--value",
        "v",
        "--switch",
        "--refused",
        "x",
        "--other",
        "o",
    ]))
    .expect("flags");
    reads::take();

    // No list holds them yet: every name reads as before, and nothing is recorded.
    assert_eq!(flags.optional("--other").as_deref(), Some("o"));
    assert!(reads::take().is_empty());

    // Held, and a copy with them: what the list gives reads as before, each read recorded.
    flags.reading(&PROBE_READS);
    let copy = flags.clone();
    assert_eq!(flags.optional("--value").as_deref(), Some("v"));
    assert!(flags.bool("--switch"));
    assert_eq!(copy.values("--refused"), ["x"]);
    assert!(copy.given("--value"));
    assert_eq!(flags.count("--switch"), 1);
    let recorded = reads::take();
    assert_eq!(
        recorded,
        [
            ("--value", Form::Value),
            ("--switch", Form::Switch),
            ("--refused", Form::Value),
            ("--value", Form::Presence),
            ("--switch", Form::Presence),
        ]
        .map(|(name, form)| Read {
            row: "probe",
            name: name.to_owned(),
            form,
        })
    );

    // A name outside the list fails, naming the row and the name, on the copy too; so does a
    // name read in the form the list does not give it.
    for (message, expected) in [
        (
            failure(|| flags.optional("--other")),
            "probe reads --other, which its read list does not hold",
        ),
        (
            failure(|| copy.values("--unlisted")),
            "probe reads --unlisted, which its read list does not hold",
        ),
        (
            failure(|| flags.bool("--value")),
            "probe reads --value as a switch, its read list holds it as an option",
        ),
        (
            failure(|| copy.optional("--switch")),
            "probe reads --switch for its value, its read list holds it as a switch",
        ),
    ] {
        assert!(message.contains(expected), "{message}");
    }
    reads::take();

    // A strict row (work order point 4) is held to the forms its register gives its flags.
    let strict = FlagArgs::parse(&words(&["--package", "p", "--check"])).expect("flags");
    strict.reading_forms("package fmt");
    assert!(strict.bool("--check"));
    assert_eq!(strict.optional("--package").as_deref(), Some("p"));
    assert!(!strict.bool("--unlisted"));
    for (message, expected) in [
        (
            failure(|| strict.bool("--package")),
            "package fmt reads --package as a switch, the register gives it as an option",
        ),
        (
            failure(|| strict.optional("--check")),
            "package fmt reads --check for its value, the register gives it as a switch",
        ),
    ] {
        assert!(message.contains(expected), "{message}");
    }
    assert!(
        reads::take().iter().all(|read| read.row == "package fmt"),
        "a strict row's reads are recorded under it"
    );
}

#[test]
fn every_lenient_command_reads_through_its_list() {
    let _guard = env_lock();
    let temp = TempDir::new().unwrap();
    let runtime = temp.path().join("runtime-must-not-exist");
    let _runtime_env = use_runtime_state_root(&runtime);
    set_missing_config_env();
    let path = |name: &str| temp.path().join(name).display().to_string();
    let (state, package, evidence, repo) = (
        path("session"),
        path("package"),
        path("evidence.zip"),
        path("repo"),
    );
    let (state, package, evidence, repo) = (
        state.as_str(),
        package.as_str(),
        evidence.as_str(),
        repo.as_str(),
    );
    const LEGACY: [&str; 3] = ["--via-daemon", "--local", "--state-dir"];
    let legacy_and = |more: &[&'static str]| LEGACY.iter().chain(more).copied().collect::<Vec<_>>();
    let record = |action: &'static str| {
        vec![
            "--instance",
            "neutral.a",
            "record",
            action,
            "--state-dir",
            state,
        ]
    };
    let session = ["--state-dir", "--instance"].to_vec();
    let frame_input = legacy_and(&["--scene", "--capture", "--package", "--zip"]);
    // (row, command line, exit code, error code, the reads it records first under the row): the 24
    // lenient rows, then six strict ones.
    let lines: [(&str, Vec<&str>, i32, &str, Vec<&str>); 30] = [
        (
            "touch-probe",
            vec!["--touch-backend", "auto", "touch-probe"],
            2,
            "validation_failed",
            vec!["--touch-backend"],
        ),
        (
            "recover",
            vec!["session", "recover", "--stale-capture", "--seed", "1"],
            2,
            "validation_failed",
            legacy_and(&["--seed", "--dry-run", "--stale-capture"]),
        ),
        (
            "tap-target",
            vec!["tap-target"],
            2,
            "validation_failed",
            legacy_and(&["--target"]),
        ),
        (
            "navigate",
            vec!["navigate"],
            2,
            "validation_failed",
            legacy_and(&["--to"]),
        ),
        (
            "debug-package",
            vec!["lab", "debug-package"],
            2,
            "package_invalid",
            vec!["--package", "--zip"],
        ),
        (
            "export-evidence",
            vec![
                "lab",
                "export-evidence",
                "--package",
                package,
                "--out",
                evidence,
                "--outcome",
                "neither",
            ],
            2,
            "validation_failed",
            vec!["--package", "--zip", "--out", "--outcome"],
        ),
        (
            "recognize-artifact",
            vec!["recognize-artifact"],
            2,
            "validation_failed",
            vec!["--request"],
        ),
        (
            "record start",
            record("start"),
            2,
            "validation_failed",
            vec!["--state-dir", "--instance", "--task-id"],
        ),
        ("record status", record("status"), 0, "", session.clone()),
        (
            "record candidates",
            record("candidates"),
            3,
            "record_session_not_active",
            session.clone(),
        ),
        (
            "record amend",
            record("amend"),
            3,
            "record_session_not_active",
            session.clone(),
        ),
        (
            "record step",
            record("step"),
            3,
            "record_session_not_active",
            session,
        ),
        (
            "package validate",
            vec!["package", "validate"],
            2,
            "validation_failed",
            vec!["--zip"],
        ),
        (
            "package inspect",
            vec!["package", "inspect"],
            2,
            "validation_failed",
            vec!["--zip"],
        ),
        (
            "package bundle",
            vec!["package", "bundle"],
            2,
            "validation_failed",
            vec![
                "--maintenance",
                "--source-repository",
                "--source-commit",
                "--applications",
            ],
        ),
        (
            "resource compile-maa",
            vec![
                "resource",
                "compile-maa",
                "--repo",
                repo,
                "--maa-tasks",
                repo,
                "--facts",
                "--task",
            ],
            2,
            "validation_failed",
            vec!["--repo", "--maa-tasks", "--facts", "--task"],
        ),
        (
            "operation validate",
            vec!["operation", "validate", "--repo", repo],
            2,
            "validation_failed",
            vec!["--repo", "--operation-dir"],
        ),
        (
            "operation inspect",
            vec!["operation", "inspect"],
            2,
            "validation_failed",
            vec!["--operation-dir"],
        ),
        (
            "operation explain",
            vec!["operation", "explain"],
            2,
            "validation_failed",
            vec!["--operation-dir"],
        ),
        (
            "recognize",
            vec!["recognize"],
            2,
            "validation_failed",
            legacy_and(&["--target"]),
        ),
        (
            "detect-page",
            vec!["detect-page"],
            2,
            "package_invalid",
            frame_input.clone(),
        ),
        (
            "current-page",
            vec!["current-page"],
            2,
            "package_invalid",
            frame_input.clone(),
        ),
        (
            "is-visible",
            vec!["is-visible"],
            2,
            "package_invalid",
            frame_input,
        ),
        (
            "locate",
            vec!["locate"],
            2,
            "validation_failed",
            legacy_and(&["--template"]),
        ),
        // The strict commands whose switch lists the recording checks (work order point 4).
        (
            "observe",
            vec!["observe"],
            2,
            "package_invalid",
            vec![
                "--capture",
                "--with-frame",
                "--preview",
                "--instance",
                "--package",
                "--zip",
            ],
        ),
        (
            "do",
            vec!["do"],
            2,
            "validation_failed",
            vec![
                "--dry-run",
                "--capture",
                "--after-page",
                "--after-timeout-ms",
                "--target",
            ],
        ),
        (
            "ensure",
            vec!["ensure"],
            2,
            "validation_failed",
            vec!["--dry-run", "--capture", "--to", "--page"],
        ),
        (
            "wait",
            vec!["wait"],
            2,
            "package_invalid",
            vec!["--capture", "--instance", "--package", "--zip"],
        ),
        (
            "package fmt",
            vec!["package", "fmt"],
            2,
            "validation_failed",
            vec!["--check", "--package"],
        ),
        (
            "package loading",
            vec!["package", "loading"],
            2,
            "validation_failed",
            vec!["--write", "--check"],
        ),
    ];
    let mut failures = Vec::new();
    reads::take();
    for (row, line, exit_code, code, expected) in lines {
        let result = run_cli(std::iter::once("--json").chain(line.iter().copied()), true);
        let recorded = reads::take();
        let answered = (
            result.exit_code(),
            result
                .envelope
                .error
                .as_ref()
                .map_or("", |error| error.code.as_str()),
        );
        if answered != (exit_code, code) {
            failures.push(format!(
                "{row}: {line:?} answered {answered:?}, not ({exit_code}, {code}): {}",
                result.envelope_json()
            ));
        }
        if let Some(read) = recorded.iter().find(|read| read.row != row) {
            failures.push(format!("{row}: {line:?} recorded a read under {read:?}"));
        }
        for name in expected {
            if !recorded.iter().any(|read| read.name == name) {
                failures.push(format!(
                    "{row}: {line:?} recorded no read of {name}: {recorded:?}"
                ));
            }
        }
    }
    assert!(!runtime.exists(), "no command line reached the Runtime");
    assert!(
        failures.is_empty(),
        "commands that do not read through their lists:\n{}",
        failures.join("\n")
    );
}
