// SPDX-License-Identifier: AGPL-3.0-only

//! The authoring option (Workflow #434 LAB-5F; `contracts/lab-two-entries.md` §8.3; register
//! §1.287, "Slice LAB-5F"): the operations of the authoring module, the rows whose `module` is
//! `authoring` in `contracts/lab-operations.json`, are listed and served only where the module is
//! installed.
//!
//! - **Installed** ([`module`], `lab_ops::locate::authoring_module`, read at each call): outside
//!   any installation root, always (the coordinator's ruling of 2026-11-10 on #434, point 15: an
//!   unpacked Tools package has no configuration to read); inside one, when its selected
//!   configuration says `lab_module_authoring: true`. Absent or `false`: not installed. A
//!   configuration that cannot be read, or whose key is not a boolean: not installed, and the
//!   refusal carries why.
//! - **A call** ([`admit`]): every authoring row is served by actinglab's own command
//!   (`engine::COMMANDS`), so `crate::execute` asks first, before any of its gates and before the
//!   command runs: on actinglab's own command line, the short words of the row (which `main`'s
//!   route gives to that command), `serve --stdio` (`engine::line`) and MCP's `--via` child. Where
//!   the module is not installed the call is refused with `lab_command_option_disabled`
//!   (`operation` the row's name, `lab_option` `authoring`), nothing sent and nothing written.
//!   Rows of the core, debugging and rescue modules never meet it.
//! - **The listing** ([`listed`], [`listed_help`]): `help`, `list commands` and `capabilities`
//!   leave out the commands of the authoring rows where the module is not installed, and a group
//!   one word shorter than such a command (`record`, `session record`). `serve --stdio` lists
//!   nothing.
//! - **Which command line is a row's**: the row's `source` entries `actinglab <words>` (a note in
//!   parentheses after the words left out); a command line whose command words begin with those
//!   words is the row's, the longest such words deciding.

use super::{table, words};
use crate::codes::ActinglabCode;
use crate::{CliError, CliOutcome, ErrorKind};
use actingcommand_contract::LabOption;
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_lab_ops::cli_line;
use actingcommand_lab_ops::locate::{self, AUTHORING_KEY, AuthoringModule};
use actingcommand_lab_ops::table::{Operation, OperationModule, OperationTable};
use serde_json::{Map, Value, json};
use std::path::PathBuf;

/// How a row's `source` names one of actinglab's own commands.
const OWN_SOURCE: &str = "actinglab ";
/// The member of a listing entry that names its command.
const COMMAND: &str = "command";
/// The members of `help` keyed by command.
const HELP_TABLES: [&str; 2] = ["command_options", "compatibility_notes"];
/// The member of `help` that lists the commands.
const HELP_COMMANDS: &str = "commands";

/// What the place this process runs from says of the authoring module, read at each call, so that
/// a `serve --stdio` sees a configuration changed while it runs.
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

/// The offered authoring row whose own command the command line `line` (its command words and
/// arguments) is: the row one of whose own commands begins it, the longest.
pub(crate) fn row_of<'a>(table: &'a OperationTable, line: &[&str]) -> Option<&'a Operation> {
    own_commands(table)
        .filter(|(_, own)| begins(line, own))
        .max_by_key(|(_, own)| own.len())
        .map(|(operation, _)| operation)
}

/// actinglab's own command line `command` `args`, when it calls an authoring row where the module
/// is not installed: refused with `lab_command_option_disabled` (§8.3), before the command runs.
/// `Ok` for every other command line, and for every line where the module is installed.
pub(crate) fn admit(command: &[String], args: &[String]) -> CliOutcome<()> {
    let line = command
        .iter()
        .chain(args)
        .map(String::as_str)
        .collect::<Vec<_>>();
    let row = match table() {
        Ok(table) => match row_of(table, &line) {
            Some(operation) => Ok(operation.name.as_str()),
            None => return Ok(()),
        },
        Err(outcome) => Err(outcome),
    };
    let why = match module() {
        Ok(AuthoringModule::Configured {
            config,
            installed: false,
        }) => Ok(config),
        Ok(_) => return Ok(()),
        Err(cause) => Err(cause),
    };
    match row {
        Ok(name) => Err(refusal(name, why)),
        // Where the module is not installed, a table that does not read leaves no line known to
        // be no authoring row's.
        Err(outcome) => Err(unreadable_table(&outcome)),
    }
}

/// `lab_command_option_disabled` of the authoring row `operation` (§8.3): `operation` and
/// `lab_option` `authoring` as its values; `why`, the selected configuration that does not say
/// `true` (`config_path` in the details) or the failure that kept the option unknown (its outcome
/// whole as `cause` in the details, fail loud).
fn refusal(operation: &str, why: Result<PathBuf, Outcome>) -> CliError {
    let mut details = Map::new();
    details.insert("operation".to_owned(), json!(operation));
    details.insert("lab_option".to_owned(), json!(LabOption::Authoring));
    let reason = match why {
        Ok(config) => {
            let text = format!(
                "the selected configuration {} does not say {AUTHORING_KEY}: true",
                config.display()
            );
            details.insert(
                "config_path".to_owned(),
                json!(config.display().to_string()),
            );
            text
        }
        Err(cause) => {
            let text = format!("its option could not be read ({})", cause.code().as_str());
            details.insert(
                "cause".to_owned(),
                cli_line::outcome_value(&cause, Detail::Full),
            );
            text
        }
    };
    CliError::new(
        ErrorKind::UsageValidation,
        ActinglabCode::LabCommandOptionDisabled.as_str(),
        format!("{operation}: the authoring module is not installed here: {reason}"),
        &[],
    )
    .with_details(Value::Object(details))
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

/// The command words of the authoring rows' own commands.
type OwnCommands = Vec<Vec<&'static str>>;

/// The own commands of the authoring rows to leave out of a listing here: `None` where the module
/// is installed. A place whose option cannot be read is not installed (its calls are refused,
/// saying why); a table that does not read is the error.
fn unlisted() -> CliOutcome<Option<OwnCommands>> {
    if module().is_ok_and(|module| module.installed()) {
        return Ok(None);
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
