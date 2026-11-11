// SPDX-License-Identifier: AGPL-3.0-only

//! The authoring option (Workflow #434 LAB-5F; `contracts/lab-two-entries.md` §8.3; register
//! §1.287, "Slice LAB-5F"): what it decides is only whether the operations of the authoring
//! module, the rows whose `module` is `authoring` in `contracts/lab-operations.json`, are listed
//! by default. This build has the authoring code, so it serves them wherever it runs, whatever the
//! key says (LAB-5F fix 2, the coordinator's ruling of 2026-11-11: not listed is not refused).
//!
//! - **Listed** ([`module`], `lab_ops::locate::authoring_module`, read at each listing): outside
//!   any installation root, always (the coordinator's ruling of 2026-11-10 on #434, point 15: an
//!   unpacked Tools package has no configuration to read); inside one, when its selected
//!   configuration says `lab_module_authoring: true`. Absent or `false`: not listed. A selection
//!   or configuration that cannot be read, or whose key is not a boolean: not listed, and the
//!   command that lists says why once, on its developer line ([`unread_option_line`]: the path and
//!   the cause).
//! - **The listing** ([`listed`], [`listed_help`]): `help`, `list commands` and `capabilities`
//!   leave out the commands of the authoring rows where they are not listed, and a group one word
//!   shorter than such a command (`record`, `session record`). `serve --stdio` lists nothing.
//! - **A call** of an authoring row runs everywhere: nothing here stands before a command (fix 2
//!   removed the refusal `lab_command_option_disabled` that LAB-5F had placed at the start of
//!   `crate::execute`).
//! - **Which command line is a row's**: the row's `source` entries `actinglab <words>` (a note in
//!   parentheses after the words left out); a listing entry whose command words begin with those
//!   words is the row's.

use super::{PROGRAM, table, words};
use crate::{CliError, CliOutcome, ErrorKind};
use actingcommand_contract::outcome::{Category, Detail, Outcome};
use actingcommand_lab_ops::cli_line;
use actingcommand_lab_ops::locate::{self, AUTHORING_KEY, AuthoringModule};
use actingcommand_lab_ops::table::{Operation, OperationModule, OperationTable};
use serde_json::{Value, json};

/// How a row's `source` names one of actinglab's own commands.
const OWN_SOURCE: &str = "actinglab ";
/// The member of a listing entry that names its command.
const COMMAND: &str = "command";
/// The members of `help` keyed by command.
const HELP_TABLES: [&str; 2] = ["command_options", "compatibility_notes"];
/// The member of `help` that lists the commands.
const HELP_COMMANDS: &str = "commands";

/// What the place this process runs from says of the authoring module, read at each listing, so
/// that a long-running process sees a configuration changed while it runs.
pub(crate) fn module() -> Result<AuthoringModule, Outcome> {
    #[cfg(test)]
    if let Some(program) = test_place::program() {
        return locate::authoring_module_from(&program, None);
    }
    locate::authoring_module()
}

/// The command words of the source `source` when it names one of actinglab's own commands.
fn own_words(source: &str) -> Option<Vec<&str>> {
    let words = source.strip_prefix(OWN_SOURCE)?;
    let words = words.split_once(" (").map_or(words, |(words, _)| words);
    let words = words
        .split(' ')
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    (!words.is_empty()).then_some(words)
}

/// The offered authoring rows of `table`, each with the command words of one of its own commands.
fn own_commands(table: &OperationTable) -> impl Iterator<Item = (&Operation, Vec<&str>)> {
    table
        .operations
        .iter()
        .filter(|operation| {
            operation.module == OperationModule::Authoring && words::offered(operation)
        })
        .flat_map(|operation| {
            operation
                .source
                .iter()
                .filter_map(|source| own_words(source.as_str()))
                .map(move |own| (operation, own))
        })
}

/// Whether the words `words` begin with the words `prefix`.
fn begins(words: &[&str], prefix: &[&str]) -> bool {
    words.len() >= prefix.len() && words.iter().zip(prefix).all(|(word, own)| word == own)
}

/// The operation table this build embeds did not read (`super::table`): which commands are the
/// authoring module's cannot be told.
fn unreadable_table(outcome: &Outcome) -> CliError {
    CliError::new(
        ErrorKind::UsageValidation,
        outcome.code().as_str(),
        "the operation table this build embeds does not read, so the authoring module's \
         commands cannot be told",
        &[],
    )
    .with_details(json!({"outcome": cli_line::outcome_value(outcome, Detail::Full)}))
}

/// The developer line of a listing whose place's option could not be read: `cause` (the
/// selection or the configuration that does not read, or whose key is not a boolean) with its
/// path and its own cause. A warning: the listing still answers, leaving the authoring module's
/// commands out, and no call is refused because of it.
pub(crate) fn unread_option_line(cause: &Outcome) -> String {
    format!(
        "{} {PROGRAM}: {cause} the authoring module's commands are left out of this listing, \
         since {AUTHORING_KEY} could not be read",
        cli_line::level(Category::Warning)
    )
}

/// The command words of the authoring rows' own commands.
type OwnCommands = Vec<Vec<&'static str>>;

/// The own commands of the authoring rows to leave out of a listing here: `None` where they are
/// listed. A place whose option cannot be read leaves them out and says why once, on the
/// developer line ([`unread_option_line`]); a table that does not read is the error.
fn unlisted() -> CliOutcome<Option<OwnCommands>> {
    match module() {
        Ok(module) if module.installed() => return Ok(None),
        Ok(_) => {}
        Err(cause) => eprintln!("{}", unread_option_line(&cause)),
    }
    let table = table().map_err(|outcome| unreadable_table(&outcome))?;
    Ok(Some(own_commands(table).map(|(_, own)| own).collect()))
}

/// Whether the listing entry `entry` (command words, then flags) is left out with the own
/// commands `own`: it begins with one, or it is the group one word shorter than one.
fn left_out(entry: &str, own: &[Vec<&str>]) -> bool {
    let words = entry
        .split(' ')
        .take_while(|word| !word.starts_with("--"))
        .collect::<Vec<_>>();
    own.iter().any(|command| {
        begins(&words, command) || (words.len() + 1 == command.len() && begins(command, &words))
    })
}

/// The command entries `entries` (each with its `command`) of `help`, `list commands` and
/// `capabilities`, as this place lists them.
pub(crate) fn listed(entries: Vec<Value>) -> CliOutcome<Vec<Value>> {
    let Some(own) = unlisted()? else {
        return Ok(entries);
    };
    Ok(entries
        .into_iter()
        .filter(|entry| {
            !entry
                .get(COMMAND)
                .and_then(Value::as_str)
                .is_some_and(|command| left_out(command, &own))
        })
        .collect())
}

/// `help`'s answer as this place lists it: its commands, and its tables keyed by command.
pub(crate) fn listed_help(mut help: Value) -> CliOutcome<Value> {
    let Some(own) = unlisted()? else {
        return Ok(help);
    };
    for member in HELP_TABLES {
        if let Some(entries) = help.get_mut(member).and_then(Value::as_object_mut) {
            entries.retain(|command, _| !left_out(command, &own));
        }
    }
    if let Some(entries) = help.get_mut(HELP_COMMANDS).and_then(Value::as_array_mut) {
        entries.retain(|entry| {
            !entry
                .get(COMMAND)
                .and_then(Value::as_str)
                .is_some_and(|command| left_out(command, &own))
        });
    }
    Ok(help)
}

/// Test-only: the program whose place this thread reads the authoring module from, in place of
/// this process's (a made-up installation root in a temporary folder), until the guard drops.
#[cfg(test)]
pub(crate) mod test_place {
    use std::cell::RefCell;
    use std::path::PathBuf;

    thread_local! {
        static PROGRAM: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    }

    /// Resets the place when dropped.
    pub(crate) struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            PROGRAM.with(|program| program.borrow_mut().take());
        }
    }

    /// This thread reads the authoring module as a program at `program` would.
    pub(crate) fn set(program: PathBuf) -> Guard {
        PROGRAM.with(|place| *place.borrow_mut() = Some(program));
        Guard
    }

    /// The program this thread reads the module from, when a test set one.
    pub(crate) fn program() -> Option<PathBuf> {
        PROGRAM.with(|program| program.borrow().clone())
    }
}
