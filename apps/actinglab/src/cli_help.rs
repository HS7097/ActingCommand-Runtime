// SPDX-License-Identifier: AGPL-3.0-only

//! `actinglab help` for people (Workflow #434 CLI-HELP; register §1.313;
//! `contracts/lab-two-entries.md` §9.3, "Help"; the coordinator's rulings of 2026-11-11, not
//! Alice's words).
//!
//! - **Which lines ask for help** ([`asked`]). A line with `--help` anywhere asks for the help of
//!   its command words (actinglab's global options and their values left out, up to the first
//!   other flag); so does a line of one of actinglab's own groups followed by the word `help`
//!   (`lab help`). `cli_module::route` gives either line to actinglab's own `help`, with those
//!   words, before anything else reads it: nothing runs and nothing is sent. A row's words keep a
//!   `help` after them as the row's word (`status help` names the instance `help`, as before).
//! - **The list** ([`list`]). Every command this build offers, grouped: the offered rows of the
//!   operation table this build embeds, by module (core, debugging, authoring, rescue; a row a
//!   platform does not offer is not listed), then actinglab's own commands that no row serves or
//!   replaces ([`OWN_WITHOUT_ROW`]). Each entry: its name (a row's name, which `serve` and MCP call
//!   it), its parameters in short form, its one sentence (a row's `help`, read from the table,
//!   never copied) and the table's three facts (`writes`, `irreversible`, `runtime`; none for an
//!   own command without a row, which the table does not describe).
//! - **The detail** ([`topic`]). `help <words>` and `<words> --help` describe the entry the words
//!   name (a row's name, the words of the own command that serves it, or those of an own command
//!   it replaces), or list the entries under them when they are a group's; words beyond a
//!   command's own (its positionals) are left out.
//! - **The text for people** ([`people`]): built from the JSON line the same answer prints with
//!   `--json`, so that the two never differ. Plain ASCII, at most [`WIDTH`] columns, no colour, no
//!   paging; the same on every platform but for the rows a platform does not offer.

use crate::cli_module::{self, authoring, engine, words};
use crate::{CliError, CliOutcome, ErrorKind};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_lab_ops::cli_line::{self, Exit};
use actingcommand_lab_ops::codes::ArgumentProblem;
use actingcommand_lab_ops::table::{
    Operation, OperationModule, OperationTable, Param, ParamForm, ParamType, RuntimeNeed,
};
use serde_json::{Value, json};

/// The word that asks for help first on a command line: `help`, actinglab's own command.
pub(crate) const HELP: &str = "help";
/// The flag that asks for help anywhere on a command line.
pub(crate) const HELP_FLAG: &str = "--help";
/// The widest line of the text for people.
pub(crate) const WIDTH: usize = 100;
/// Where a listing's entries begin.
const INDENT: usize = 2;
/// Where a listing's sentences begin.
const COLUMN: usize = 34;
/// The module word of actinglab's own commands that no row serves or replaces.
pub(crate) const OWN: &str = "own";
/// The resident mode's word: the CLI module's, no command of actinglab's own dispatch.
pub(crate) const SERVE: &str = "serve";
/// The global option of the counter that forwarded the work (MCP's actinglab child).
const VIA: &str = "--via";

/// actinglab's global options that take a value (`crate::cli_parse`); the other words of
/// `words::GLOBAL_OPTIONS` take none.
const VALUE_OPTIONS: [&str; 12] = [
    "--run-root",
    "--instance",
    "--instances",
    "--profile",
    "--resource-root",
    "--via",
    "--game",
    "--server",
    "--runtime-endpoint",
    "--capture-backend",
    "--backend",
    "--touch-backend",
];

/// actinglab's global options as the text for people lists them (CLI-HELP ruling 1), each with
/// one phrase; the retired `--require-session` is not listed.
pub(crate) const GLOBAL_OPTION_PHRASES: [(&str, &str); 18] = [
    (
        "--json",
        "print the one JSON line of the answer instead of this text",
    ),
    (
        "--help",
        "print the help of the command words before it; nothing runs",
    ),
    (
        "--instance <instance>",
        "the instance a command acts on: an alias, an instance_id or an ADB port; on an operation \
         row it stands for the row's <instance>, never beside it",
    ),
    (
        "--run-root <path>",
        "the folder actinglab's own commands keep their run results in",
    ),
    (
        "--resource-root <path>",
        "the resource root actinglab's own commands read",
    ),
    (
        "--game <id>",
        "with --server: picks an own command's instance from actinglab's user configuration when \
         no --instance names one, and its scope",
    ),
    ("--server <id>", "with --game: the same choice and scope"),
    (
        "--runtime-endpoint <127.0.0.1:port>",
        "the Runtime endpoint paths reports and doctor checks",
    ),
    (
        "--capture-backend <name>",
        "auto, auto-fastest, adb, droidcast_raw or nemu_ipc: the capture an own command asks for",
    ),
    ("--backend <name>", "the same as --capture-backend"),
    (
        "--touch-backend <name>",
        "auto, auto-fastest, maatouch, minitouch or adb_shell_input: the touch an own command asks \
         for",
    ),
    (
        "--dry-run",
        "check and preview without executing or storing, where a command offers it",
    ),
    (
        "--verbose",
        "more detail in the answers of observe, do, ensure and wait",
    ),
    ("--version", "print actinglab's version instead"),
    (
        "--via <mcp|ui>",
        "set by the program that forwards the work (MCP's actinglab child), not by people",
    ),
    (
        "--instances <id,id,...>",
        "read by no command; the offline commands refuse it",
    ),
    (
        "--profile <name>",
        "read by no command; the offline commands refuse it",
    ),
    (
        "--quiet",
        "read by no command; the offline commands refuse it",
    ),
];

/// One of actinglab's own commands that no row of the operation table serves or replaces (CLI-HELP
/// ruling 4): its words, its parameters in short form and its one sentence, written from what its
/// code does.
#[derive(Debug)]
pub(crate) struct OwnCommand {
    pub(crate) words: &'static str,
    pub(crate) usage: &'static str,
    pub(crate) sentence: &'static str,
}

/// Every own command of actinglab without a row (the 34 commands of the inventory,
/// `ratchet/actinglab_commands.json`, that no offered row names, serves or replaces), and `serve`,
/// the CLI module's resident mode, which is no command of the inventory.
pub(crate) static OWN_WITHOUT_ROW: [OwnCommand; 35] = [
    OwnCommand {
        words: "help",
        usage: "help [<command words>]",
        sentence: "Prints every command this build offers with its sentence, or one command's \
                   parameters and facts, or the commands under a group of words.",
    },
    OwnCommand {
        words: "version",
        usage: "version",
        sentence: "Prints actinglab's name, its CLI version, the Runtime build it carries and its \
                   schema version.",
    },
    OwnCommand {
        words: "paths",
        usage: "paths",
        sentence: "Prints the user configuration file's path and the run root, resource root, \
                   Runtime endpoint and adb path in effect.",
    },
    OwnCommand {
        words: "capabilities",
        usage: "capabilities",
        sentence: "Prints what this build declares, unchecked: its commands and their needs, exit \
                   codes, schema domains, backends and the packs under the resource root.",
    },
    OwnCommand {
        words: "doctor",
        usage: "doctor",
        sentence: "Checks the user configuration file, adb, the resource and run roots and the \
                   Runtime endpoint, and reports each result without failing on it.",
    },
    OwnCommand {
        words: "schema",
        usage: "schema [<kind>]",
        sentence: "Prints the fixed schema description of one kind (task, control, pack, package, \
                   ledger or a Lab command), or of all of them.",
    },
    OwnCommand {
        words: "list",
        usage: "list [<kind>]",
        sentence: "Lists the commands this build declares, or the targets, pages, tasks, bundles \
                   or controls found under the resource root.",
    },
    OwnCommand {
        words: "detect",
        usage: "detect --task <id> [options]",
        sentence: "Runs one environment detector of the resource root on a saved scene for the \
                   instance and stores its result for env resolve and env status.",
    },
    OwnCommand {
        words: "env resolve",
        usage: "env resolve --task <id> [<input>] [options]",
        sentence: "Resolves the env markers of a path, a value or one key from the instance's \
                   stored detection result; refused when it is missing or stale.",
    },
    OwnCommand {
        words: "env status",
        usage: "env status --task <id>",
        sentence: "Reports whether the instance's stored detection result for one task is fresh, \
                   stale or still to be detected.",
    },
    OwnCommand {
        words: "lab validate",
        usage: "lab validate --zip <file>",
        sentence: "Validates one Lab package archive offline and answers its control, entry task \
                   and resources, and the recognition targets it does not support.",
    },
    OwnCommand {
        words: "lab replay-evidence",
        usage: "lab replay-evidence --zip <file> --expected-sha256 <sha256>",
        sentence: "Checks an evidence archive against its expected SHA-256 offline and answers its \
                   size, its hashes and its manifest.",
    },
    OwnCommand {
        words: "lab arbitrator",
        usage: "lab arbitrator",
        sentence: "Retired: refuses every line with legacy_lab2_arbitrator_retired.",
    },
    OwnCommand {
        words: "lab vendor-stdio-selftest",
        usage: "lab vendor-stdio-selftest",
        sentence: "Internal self-test: redirects this process's stdout and stderr as around a \
                   vendor library call, writes two lines and answers whether each was captured.",
    },
    OwnCommand {
        words: "package dry-run",
        usage: "package dry-run --package <path> | --zip <file> --fixture <png>... --out <file> \
                [options]",
        sentence: "Simulates one package offline over recorded frames, with no Runtime and no \
                   device, and writes its decision into a new result archive.",
    },
    OwnCommand {
        words: "package run",
        usage: "package run --zip <file> [--out <path>]",
        sentence: "Validates a package archive offline, writes a blocked result archive when \
                   asked, then refuses to run it with lab_lease_required.",
    },
    OwnCommand {
        words: "operation run",
        usage: "operation run",
        sentence: "Refuses every line with lab_lease_required: an operation runs only through the \
                   Runtime's scheduler admission.",
    },
    OwnCommand {
        words: "control inspect",
        usage: "control inspect [--control <file>]",
        sentence: "Reserved: echoes the control it names with the status reserved, reading no \
                   file.",
    },
    OwnCommand {
        words: "control verify",
        usage: "control verify --candidate <file> --candidate-id <id>",
        sentence: "Checks that a candidate control file parses as JSON and answers validated; it \
                   clicks nothing.",
    },
    OwnCommand {
        words: "control probe-click",
        usage: "control probe-click [options]",
        sentence: "Refuses every line and clicks nothing: effect_not_navigation_only or \
                   unresolved_coords by its options, else lab_lease_required.",
    },
    OwnCommand {
        words: "control export",
        usage: "control export",
        sentence: "Reserved: refuses every line with not_implemented.",
    },
    OwnCommand {
        words: "control diff",
        usage: "control diff --candidate <file> --stable <file>",
        sentence: "Hashes a candidate and a stable control file with SHA-256 and answers both \
                   hashes and whether they are the same.",
    },
    OwnCommand {
        words: "session throat-policy",
        usage: "session throat-policy",
        sentence: "Prints the fixed policy document of the retired session layer and the Runtime \
                   boundary; reads nothing.",
    },
    OwnCommand {
        words: "session capture-policy",
        usage: "session capture-policy",
        sentence: "Prints the fixed policy document of fresh-frame capture and the order of the \
                   capture backends; reads nothing.",
    },
    OwnCommand {
        words: "session record-policy",
        usage: "session record-policy",
        sentence: "Prints the fixed policy document of how a Lab recording is authorized; reads \
                   nothing.",
    },
    OwnCommand {
        words: "session self-heal-policy",
        usage: "session self-heal-policy",
        sentence: "Prints the fixed policy document of the self-heal flow; reads nothing.",
    },
    OwnCommand {
        words: "session contract",
        usage: "session contract",
        sentence: "Prints the fixed session access contract document; reads nothing.",
    },
    OwnCommand {
        words: "session api",
        usage: "session api",
        sentence: "Prints the fixed contract of the session commands and their envelope; reads \
                   nothing.",
    },
    OwnCommand {
        words: "session transport",
        usage: "session transport [<action>] [--endpoint <host:port>]",
        sentence: "Prints the fixed transport contract; check tests that an endpoint is loopback \
                   and accepts a connection, and plan is retired.",
    },
    OwnCommand {
        words: "session capture",
        usage: "session capture [diagnose] [options]",
        sentence: "Captures one frame of an instance through the Runtime into a PNG file, or with \
                   diagnose compares two frames to tell a stale capture.",
    },
    OwnCommand {
        words: "run open",
        usage: "run open <run-id>",
        sentence: "Reserved: echoes the run id and the run root with the status reserved, reading \
                   no run.",
    },
    OwnCommand {
        words: "run summary",
        usage: "run summary <run-id> --state-root <dir>",
        sentence: "Reads the Runtime's summary of one run from the Runtime of the state root it \
                   names, changing nothing.",
    },
    OwnCommand {
        words: "run export",
        usage: "run export <run-id> --out <path>",
        sentence: "Writes a placeholder archive for one run to the path it names; it reads no run.",
    },
    OwnCommand {
        words: "report export",
        usage: "report export --last-error --out <path>",
        sentence: "Writes a placeholder last-error report archive to the path it names; it reads \
                   no error data.",
    },
    OwnCommand {
        words: SERVE,
        usage: "serve --stdio [--front mcp]",
        sentence: "For programs: answers each JSON line {op, params} on stdin with the line that \
                   operation's --json prints, until stdin ends.",
    },
];

/// The text for people of an own command without a row, in place of the table's facts.
const OWN_FACTS: &str = "actinglab's own command, with no row in the operation table: its \
     parameters are those of the line above, and whether it writes, is irreversible or needs the \
     Runtime is not recorded.";

/// The sections of the list, in order: each module's word and the title its entries go under.
const SECTIONS: [(&str, &str); 5] = [
    ("core", "Core module:"),
    ("debugging", "Debugging module:"),
    ("authoring", "Authoring module:"),
    ("rescue", "Rescue module (this root's installation):"),
    (
        OWN,
        "actinglab's own commands, with no row in the operation table:",
    ),
];

/// The modules whose offered rows the list holds, in its order.
const MODULES: [OperationModule; 4] = [
    OperationModule::Core,
    OperationModule::Debugging,
    OperationModule::Authoring,
    OperationModule::Rescue,
];

/// The words of the command line `arguments` that ask for help: `help` and the command words
/// help is asked for, after the line's `--json` and `--via <counter>`; `None` for a line that does
/// not ask for help (CLI-HELP rulings 2 and 6). With `--help` anywhere, the words are the line's
/// command words (a leading `help` left out when words follow it); without it, only one of
/// actinglab's own groups followed by `help` asks, for that group.
pub(crate) fn asked(arguments: &[String]) -> Option<Vec<String>> {
    let found = command_words(arguments);
    let topic = if arguments.iter().any(|argument| argument == HELP_FLAG) {
        if found.len() > 1 && found[0] == HELP {
            found[1..].to_vec()
        } else {
            found
        }
    } else {
        match found.as_slice() {
            [group, word] if word == HELP && own_group(group) => vec![group.clone()],
            _ => return None,
        }
    };
    let mut line = Vec::new();
    if arguments
        .iter()
        .any(|argument| argument == cli_module::JSON)
    {
        line.push(cli_module::JSON.to_owned());
    }
    if let Some(pair) = arguments.windows(2).find(|pair| pair[0] == VIA) {
        line.extend(pair.iter().cloned());
    }
    line.push(HELP.to_owned());
    line.extend(topic);
    Some(line)
}

/// Whether `word` is one of actinglab's own groups, whose word followed by `help` lists the
/// group's commands: the groups of its own dispatch (`crate::OWN_GROUPS`), `run` and `record`.
fn own_group(word: &str) -> bool {
    crate::OWN_GROUPS.contains(&word) || matches!(word, "run" | "record")
}

/// The command words of `arguments`: its words up to its first flag that is no global option,
/// actinglab's global options (with their values), `--json` and `--help` left out.
fn command_words(arguments: &[String]) -> Vec<String> {
    let mut found = Vec::new();
    let mut index = 0;
    while let Some(word) = arguments.get(index) {
        let word = word.as_str();
        index += 1;
        if VALUE_OPTIONS.contains(&word) {
            index += 1;
            continue;
        }
        if word == cli_module::JSON || word == HELP_FLAG || words::GLOBAL_OPTIONS.contains(&word) {
            continue;
        }
        if word.starts_with("--") {
            break;
        }
        found.push(word.to_owned());
    }
    found
}

/// Whether `help` without `--json` (and with no `--via`, which `main` answers with the envelope)
/// is the line `invocation` asks for: `main` then prints [`people`] of its answer.
pub(crate) fn for_people(invocation: &crate::OwnInvocation) -> bool {
    invocation
        .as_ref()
        .is_ok_and(|own| own.command.as_slice() == [HELP] && !own.global.json)
}

/// One entry of the help: an offered row, or one of actinglab's own commands without one.
#[derive(Debug, Clone, Copy)]
enum Entry {
    Row(&'static Operation),
    Own(&'static OwnCommand),
}

impl Entry {
    /// The words that name the entry on the command line: a row's [`row_names`], an own
    /// command's words.
    fn names(&self) -> Vec<String> {
        match self {
            Self::Row(operation) => row_names(operation),
            Self::Own(own) => vec![own.words.to_owned()],
        }
    }

    /// The entry as data (CLI-HELP ruling 3): its name, module, sentence, parameters in short form
    /// and one by one, the table's three facts, the words that serve it and those it replaces.
    /// `options` are `help`'s `command_options`, whose lines an own command's entry carries.
    fn value(&self, options: &Value) -> Value {
        match self {
            Self::Row(operation) => json!({
                "name": operation.name,
                "module": module_word(operation.module),
                "help": operation.help,
                "usage": usage(operation),
                "params": operation
                    .params
                    .iter()
                    .map(|param| param_value(operation, param))
                    .collect::<Vec<_>>(),
                "writes": operation.writes,
                "irreversible": operation.irreversible,
                "runtime": runtime_word(operation.runtime),
                "spellings": spellings(operation),
                "replaces": replaced(operation),
            }),
            Self::Own(own) => json!({
                "name": own.words,
                "module": OWN,
                "help": own.sentence,
                "usage": own.usage,
                "params": [],
                "options": options.get(own.words).cloned().unwrap_or_else(|| json!([])),
                "writes": null,
                "irreversible": null,
                "runtime": null,
                "spellings": [own.words],
                "replaces": [],
            }),
        }
    }
}

/// Every entry of the help, in the list's order: the offered rows of `table` by module, each
/// module's in the table's order, then [`OWN_WITHOUT_ROW`].
fn entries(table: &'static OperationTable) -> Vec<Entry> {
    let mut entries = Vec::new();
    for module in MODULES {
        entries.extend(
            table
                .operations
                .iter()
                .filter(|operation| operation.module == module && words::offered(operation))
                .map(Entry::Row),
        );
    }
    entries.extend(OWN_WITHOUT_ROW.iter().map(Entry::Own));
    entries
}

/// The operation table this build embeds did not read: help cannot list its rows.
fn unreadable(outcome: &Outcome) -> CliError {
    CliError::new(
        ErrorKind::UsageValidation,
        outcome.code().as_str(),
        "the operation table this build embeds does not read, so help cannot list its rows",
        &[],
    )
    .with_details(json!({"outcome": cli_line::outcome_value(outcome, Detail::Full)}))
}

/// `help`'s new member `list` (CLI-HELP ruling 3): every entry as data, in the list's order.
/// `options` are `help`'s `command_options`.
pub(crate) fn list(options: &Value) -> CliOutcome<Value> {
    let table = cli_module::table().map_err(|outcome| unreadable(&outcome))?;
    Ok(Value::Array(
        entries(table)
            .iter()
            .map(|entry| entry.value(options))
            .collect(),
    ))
}

/// `help <words>` (CLI-HELP ruling 2): `{"command": …}`, the entry the words name, or
/// `{"group", "members"}`, the entries whose names begin with them, each with the `words` that
/// name it there; else the same for the longest beginning of the words that names either (a
/// command's positionals left out). Words that begin no command are refused as `help`'s unknown
/// word, as a read list refuses one. `options` are `help`'s `command_options`.
pub(crate) fn topic(asked: &[String], options: &Value) -> CliOutcome<Value> {
    let table = cli_module::table().map_err(|outcome| unreadable(&outcome))?;
    let entries = entries(table);
    for count in (1..=asked.len()).rev() {
        let named = asked[..count].join(" ");
        if let Some(entry) = entries.iter().find(|entry| entry.names().contains(&named)) {
            return Ok(json!({"command": entry.value(options)}));
        }
        let members = members(&entries, &named, options);
        if !members.is_empty() {
            return Ok(json!({"group": named, "members": members}));
        }
    }
    let first = asked.first().map_or(HELP, String::as_str);
    Err(cli_module::refused_word(
        HELP,
        first,
        ArgumentProblem::Unknown,
    ))
}

/// The entries named under the words `group`, each with the `words` that name it there.
fn members(entries: &[Entry], group: &str, options: &Value) -> Vec<Value> {
    let prefix = format!("{group} ");
    let mut members = Vec::new();
    for entry in entries {
        for name in entry.names() {
            if name.starts_with(&prefix) {
                let mut member = entry.value(options);
                member["words"] = Value::String(name);
                members.push(member);
            }
        }
    }
    members
}

/// The words that name the offered row `operation` on the command line: its [`spellings`], then
/// the own commands it replaces ([`replaced`]).
pub(crate) fn row_names(operation: &Operation) -> Vec<String> {
    let mut names = spellings(operation);
    names.extend(replaced(operation));
    names
}

/// The words of actinglab's own command that serves `operation` (`engine::COMMANDS`).
fn served(operation: &Operation) -> Option<String> {
    engine::COMMANDS
        .iter()
        .find(|(name, _, _)| *name == operation.name)
        .map(|(_, command, _)| command.join(" "))
}

/// Whether actinglab's own command serves `operation` under the row's own name: its command line
/// is that command's, which takes the instance only as `--instance` (register §1.307).
fn own_named(operation: &Operation) -> bool {
    served(operation).is_some_and(|command| command == operation.name)
}

/// A row's spellings (CLI-HELP rulings 2 and 5): its name, which `serve` and MCP call it, first,
/// then the words of the own command that serves it under other words.
fn spellings(operation: &Operation) -> Vec<String> {
    let mut spellings = vec![operation.name.clone()];
    if let Some(command) = served(operation).filter(|command| *command != operation.name) {
        spellings.push(command);
    }
    spellings
}

/// The own commands of actinglab the row `operation` replaces: its `source` entries `actinglab
/// <words>` (up to a flag) that are commands of actinglab's own dispatch and none of its
/// spellings. Each keeps its own parameters.
fn replaced(operation: &Operation) -> Vec<String> {
    let spellings = spellings(operation);
    let mut replaced: Vec<String> = Vec::new();
    for source in &operation.source {
        let Some(own) = authoring::own_words(source) else {
            continue;
        };
        let own = own
            .into_iter()
            .take_while(|word| !word.starts_with("--"))
            .collect::<Vec<_>>();
        let command = own
            .iter()
            .take(2)
            .map(|word| (*word).to_owned())
            .collect::<Vec<_>>();
        if command.is_empty() || !crate::own_command(&command) {
            continue;
        }
        let own = own.join(" ");
        if !spellings.contains(&own) && !replaced.contains(&own) {
            replaced.push(own);
        }
    }
    replaced
}

/// A row's parameters in short form: its words, then its positionals in order (`[<name>]` when
/// not required) and its required options; `[options]` for the rest. The instance as CLI-IN's
/// note says ([`written`]).
fn usage(operation: &Operation) -> String {
    let mut parts = vec![operation.name.clone()];
    let mut more = false;
    for param in &operation.params {
        let written = written(operation, param);
        match (param.form, param.required) {
            (ParamForm::Positional | ParamForm::Option, true) => parts.push(written),
            (ParamForm::Positional, false) => parts.push(format!("[{written}]")),
            _ => more = true,
        }
    }
    if more {
        parts.push("[options]".to_owned());
    }
    parts.join(" ")
}

/// How the command line writes `param` of `operation`: a positional `<name>`, an option
/// `--name <value>`, a switch `--name`, `...` after one that repeats; the instance of a row
/// actinglab's own command serves under the row's name `--instance <instance>`, the only spelling
/// its command line takes (register §1.307; every other row's positional instance is also written
/// `--instance <instance>`, as its note says).
fn written(operation: &Operation, param: &Param) -> String {
    let repeat = if param.repeat { "..." } else { "" };
    match param.form {
        ParamForm::Positional
            if param.value_type == ParamType::Instance && own_named(operation) =>
        {
            format!("{} <instance>", words::INSTANCE_FLAG)
        }
        ParamForm::Positional => format!("<{}>{repeat}", param.name),
        ParamForm::Option => format!(
            "{} <{}>{repeat}",
            flag(&param.name),
            value_word(&param.value_type)
        ),
        ParamForm::Switch => flag(&param.name),
    }
}

/// A parameter's flag: `--` and its name with `_` as `-` (§9.1).
fn flag(name: &str) -> String {
    format!("--{}", name.replace('_', "-"))
}

/// One parameter as data: as the table declares it, and as the command line writes it.
fn param_value(operation: &Operation, param: &Param) -> Value {
    json!({
        "name": param.name,
        "form": form_word(param.form),
        "type": type_word(&param.value_type),
        "required": param.required,
        "repeat": param.repeat,
        "default": param.default,
        "note": param.note,
        "written": written(operation, param),
    })
}

fn module_word(module: OperationModule) -> &'static str {
    match module {
        OperationModule::Core => "core",
        OperationModule::Debugging => "debugging",
        OperationModule::Authoring => "authoring",
        OperationModule::Rescue => "rescue",
    }
}

fn runtime_word(runtime: RuntimeNeed) -> &'static str {
    match runtime {
        RuntimeNeed::Required => "required",
        RuntimeNeed::Optional => "optional",
        RuntimeNeed::Offline => "offline",
    }
}

fn form_word(form: ParamForm) -> &'static str {
    match form {
        ParamForm::Positional => "positional",
        ParamForm::Option => "option",
        ParamForm::Switch => "switch",
    }
}

/// A parameter's type as the table writes it (§9.1).
fn type_word(value_type: &ParamType) -> String {
    let word = match value_type {
        ParamType::Instance => "instance",
        ParamType::String => "string",
        ParamType::Integer => "integer",
        ParamType::Unsigned => "unsigned",
        ParamType::Decimal => "decimal",
        ParamType::Boolean => "boolean",
        ParamType::Duration => "duration",
        ParamType::Path => "path",
        ParamType::File => "file",
        ParamType::Dir => "dir",
        ParamType::Json => "json",
        ParamType::Key => "key",
        ParamType::KeyValue => "key_value",
        ParamType::ListOfStrings => "list<string>",
        ParamType::Enum(words) => return format!("enum:{}", words.join("|")),
        ParamType::RequestId => "request_id",
        ParamType::RunRef => "run_ref",
        ParamType::OperationId => "operation_id",
        ParamType::MaterialRef => "material_ref",
        ParamType::Code => "code",
        ParamType::Scope => "scope",
        ParamType::TagOrCommit => "tag_or_commit",
        ParamType::Sha256 => "sha256",
    };
    word.to_owned()
}

/// An option's value as its short form writes it: the type, its words for an enumeration.
fn value_word(value_type: &ParamType) -> String {
    match value_type {
        ParamType::Enum(words) => words.join("|"),
        ParamType::ListOfStrings => "word,word,...".to_owned(),
        ParamType::KeyValue => "KEY=VALUE".to_owned(),
        other => type_word(other),
    }
}

/// The text for people of `help`'s answer `exit`: the list, one command's detail or one group's
/// commands. `None` for a failure, which prints as every own command's does.
pub(crate) fn people(exit: &Exit) -> Option<String> {
    if exit.code != 0 {
        return None;
    }
    let line = exit.line.as_ref().ok()?;
    let printed = if let Some(entry) = line.get("command") {
        detail(entry)
    } else if let Some(group) = line.get("group").and_then(Value::as_str) {
        group_lines(group, array(line, "members"))
    } else {
        listing(array(line, "list"))
    };
    Some(printed.join("\n"))
}

/// The list for people (CLI-HELP ruling 1): the usage lines, the global options, then every entry
/// under its section.
fn listing(entries: &[Value]) -> Vec<String> {
    let mut out = vec![
        "usage: actinglab [<global options>] <command words> [<parameters>]".to_owned(),
        "       actinglab help [<command words>]".to_owned(),
        "       actinglab <command words> --help".to_owned(),
        String::new(),
    ];
    out.extend(lines(
        "A command's words, then its parameters. help <command words>, or <command words> \
         --help, prints one command's parameters and facts, or the commands under a group's \
         words, and runs nothing. A row's <instance> may also be written --instance <instance>.",
        0,
        0,
    ));
    out.push(String::new());
    out.push("Global options:".to_owned());
    for (option, phrase) in GLOBAL_OPTION_PHRASES {
        columns(&mut out, option, phrase);
    }
    for (module, title) in SECTIONS {
        let section = entries
            .iter()
            .filter(|entry| text(entry, "module") == module)
            .collect::<Vec<_>>();
        if section.is_empty() {
            continue;
        }
        out.push(String::new());
        out.push(title.to_owned());
        for entry in section {
            columns(&mut out, text(entry, "usage"), &sentence(entry));
        }
    }
    out
}

/// The detail for people of one entry (CLI-HELP ruling 2): its usage and its other spellings, its
/// sentence, the table's facts and each parameter; for an own command without a row, its usage,
/// its sentence and its option lines.
fn detail(entry: &Value) -> Vec<String> {
    let mut out = lines(&format!("actinglab {}", text(entry, "usage")), 0, 4);
    let spellings = strings(entry, "spellings");
    if spellings.len() > 1 {
        let also = spellings[1..]
            .iter()
            .map(|words| format!("actinglab {words}"))
            .collect::<Vec<_>>()
            .join(", ");
        out.extend(lines(
            &format!("Also: {also}, actinglab's own command that serves this row."),
            0,
            4,
        ));
    }
    let replaces = strings(entry, "replaces");
    if !replaces.is_empty() {
        let replaced = replaces
            .iter()
            .map(|words| format!("actinglab {words}"))
            .collect::<Vec<_>>()
            .join(", ");
        out.extend(lines(
            &format!(
                "Replaces: {replaced}; each is actinglab's own command, with parameters of its own."
            ),
            0,
            4,
        ));
    }
    out.push(String::new());
    out.extend(lines(&sentence(entry), 0, 0));
    out.push(String::new());
    if text(entry, "module") == OWN {
        out.extend(lines(OWN_FACTS, 0, 0));
        let options = strings(entry, "options");
        if !options.is_empty() {
            out.push(String::new());
            out.push("Options:".to_owned());
            for option in options {
                out.extend(lines(&option, INDENT, INDENT + 2));
            }
        }
        return out;
    }
    out.extend(lines(
        &format!(
            "Module: {}. Writes: {}. Irreversible: {}. Runtime: {}.",
            text(entry, "module"),
            yes_no(entry, "writes"),
            yes_no(entry, "irreversible"),
            runtime_phrase(text(entry, "runtime"))
        ),
        0,
        0,
    ));
    let params = array(entry, "params");
    if !params.is_empty() {
        out.push(String::new());
        out.push("Parameters:".to_owned());
        for param in params {
            columns(&mut out, text(param, "written"), &param_phrase(param));
        }
    }
    out
}

/// The commands of one group for people (CLI-HELP ruling 2): each by the words that name it under
/// the group, a row named otherwise said so before its sentence.
fn group_lines(group: &str, members: &[Value]) -> Vec<String> {
    let mut out = lines(
        &format!(
            "actinglab {group}: the commands under these words. (row <name>) names the row that \
             serves them; actinglab help <command words> prints one command's parameters and \
             facts."
        ),
        0,
        0,
    );
    out.push(String::new());
    for member in members {
        let named = text(member, "words");
        let name = text(member, "name");
        let usage = text(member, "usage");
        let (left, before) = if named == name {
            (usage.to_owned(), String::new())
        } else if strings(member, "spellings")
            .iter()
            .any(|words| words == named)
        {
            let parameters = usage.strip_prefix(name).unwrap_or("");
            (format!("{named}{parameters}"), format!("(row {name}) "))
        } else {
            (named.to_owned(), format!("(replaced by row {name}) "))
        };
        columns(&mut out, &left, &format!("{before}{}", sentence(member)));
    }
    out
}

/// An entry's sentence, or what stands in for a row the table gives none.
fn sentence(entry: &Value) -> String {
    match entry.get("help").and_then(Value::as_str) {
        Some(help) if !help.trim().is_empty() => help.to_owned(),
        _ => "(the operation table holds no sentence for this row)".to_owned(),
    }
}

/// One parameter's phrase: its type, whether it is required, repeats, its default and its note.
fn param_phrase(param: &Value) -> String {
    let mut phrase = if text(param, "form") == "switch" {
        "switch".to_owned()
    } else {
        text(param, "type").to_owned()
    };
    phrase.push_str(if boolean(param, "required") {
        ", required"
    } else {
        ", optional"
    });
    if boolean(param, "repeat") {
        phrase.push_str(", repeatable");
    }
    if let Some(default) = param.get("default").and_then(Value::as_str) {
        phrase.push_str(", default ");
        phrase.push_str(default);
    }
    if let Some(note) = param.get("note").and_then(Value::as_str) {
        phrase.push_str("; ");
        phrase.push_str(note);
    }
    phrase
}

/// The table's `runtime` of a row (§9.1), for people.
fn runtime_phrase(runtime: &str) -> String {
    match runtime {
        "required" => "required, refused while it is stopped".to_owned(),
        "optional" => "optional, works while it is stopped and does more while it runs".to_owned(),
        "offline" => "not needed".to_owned(),
        other => other.to_owned(),
    }
}

fn yes_no(value: &Value, member: &str) -> &'static str {
    if boolean(value, member) { "yes" } else { "no" }
}

fn boolean(value: &Value, member: &str) -> bool {
    value.get(member).and_then(Value::as_bool).unwrap_or(false)
}

fn text<'a>(value: &'a Value, member: &str) -> &'a str {
    value.get(member).and_then(Value::as_str).unwrap_or("")
}

fn array<'a>(value: &'a Value, member: &str) -> &'a [Value] {
    match value.get(member).and_then(Value::as_array) {
        Some(items) => items.as_slice(),
        None => &[],
    }
}

fn strings(value: &Value, member: &str) -> Vec<String> {
    array(value, member)
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// One entry of a listing: `left` from column [`INDENT`] and `right` wrapped from [`COLUMN`]; a
/// `left` too wide for its column stands on lines of its own, `right` below it.
pub(crate) fn columns(out: &mut Vec<String>, left: &str, right: &str) {
    let left = ascii(left);
    let mut right = lines(right, COLUMN, COLUMN).into_iter();
    if INDENT + left.len() + 2 <= COLUMN {
        let first = right.next().unwrap_or_default();
        let first = first.get(COLUMN..).unwrap_or("");
        let line = format!(
            "{}{left:<width$}{first}",
            " ".repeat(INDENT),
            width = COLUMN - INDENT
        );
        out.push(line.trim_end().to_owned());
    } else {
        out.extend(lines(&left, INDENT, INDENT + 2));
    }
    out.extend(right);
}

/// `text` broken at spaces into lines of at most [`WIDTH`] columns, the first from column `first`,
/// the others from column `rest`; a word longer than a line is broken where the line ends.
fn lines(text: &str, first: usize, rest: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    let indent = |done: usize| if done == 0 { first } else { rest };
    for word in ascii(text).split_whitespace() {
        let mut word = word.to_owned();
        loop {
            let at = indent(out.len());
            let room = WIDTH - at;
            if line.is_empty() {
                if word.len() <= room {
                    line = word;
                    break;
                }
                let tail = word.split_off(room);
                out.push(format!("{}{word}", " ".repeat(at)));
                word = tail;
            } else if line.len() + 1 + word.len() <= room {
                line.push(' ');
                line.push_str(&word);
                break;
            } else {
                out.push(format!("{}{line}", " ".repeat(at)));
                line.clear();
            }
        }
    }
    if !line.is_empty() || out.is_empty() {
        let at = indent(out.len());
        let last = format!("{}{line}", " ".repeat(at));
        out.push(last.trim_end().to_owned());
    }
    out
}

/// `text` in plain ASCII: a tab or a line end as a space, any other control character or a
/// character beyond ASCII as `?`.
fn ascii(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            '\t' | '\n' | '\r' => ' ',
            character if character.is_ascii() && !character.is_ascii_control() => character,
            _ => '?',
        })
        .collect()
}
