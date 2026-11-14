// SPDX-License-Identifier: AGPL-3.0-only

//! The operation and its parameters, from the CLI's short words or from one line of
//! `serve --stdio` (Workflow #434 LAB-2; `contracts/lab-two-entries.md` §9.1, §9.3). Both give the
//! same [`Call`]: the operation's name and its parameters as JSON members named as the row names
//! them, so that the CLI's `--json` answer and the serve answer of one operation are the same.
//!
//! - The words are the longest operation name of the table that begins the command line, among
//!   the operations this program offers ([`offered`]); then the row's positionals in order (the
//!   instance among them: an alias, an `instance_id` or an ADB port), its options
//!   `--name value` (`_` written `-`) and its switches `--name`.
//! - Workflow #434 LAB-5A: a row passes on only what it declares; no offered row has
//!   `params_more` (§9.2). What the command line of a row that had `params_more` took before stays
//!   as it was, each named in one place:
//!   - a row actinglab's own command serves, whose command line took that command's further flags
//!     before LAB-5A ([`super::engine::as_written`]), hands its declared flags on as written
//!     (`--flag value`, or `--flag` alone when no value follows; the command reads and checks
//!     them, as it did), and actinglab's global options with them ([`GLOBAL_OPTIONS`], with `_`
//!     or `-` in their words, as before); Workflow #434 LAB-5B4: a flag the row does not declare,
//!     and a word beyond its positionals, are refused on every such row (`recover`,
//!     `debug-package` and `export-evidence` handed them on before, a debt now paid: their
//!     commands refuse what they do not read too);
//!   - the flags of [`KEPT_FLAGS`] go on as written too: a positional given as its flag (`run show
//!     --id <id>` and the like; `runs` and `monitor set --instance <x>` since Workflow #434 CLI-IN
//!     by the instance's own rule, below), and a flag of the source command the row does not
//!     declare (`op status --operation <op>`, `devices bind --root <r>`);
//!   - on a row of [`COMPLETED_ROWS`], an option LAB-5A declared takes no word that begins with
//!     `--` as its value, as the further flag it was took none.
//!
//!   `serve --stdio` refuses every member a row does not declare, and (Workflow #434 LAB-5G4a)
//!   every string value, or string in a list, that begins with `--` ([`from_json`]).
//! - Workflow #434 CLI-IN (register §1.307; `contracts/lab-two-entries.md` §9.3): on a row that
//!   declares a positional instance ([`positional_instance`]) actinglab's global `--instance <x>`
//!   ([`INSTANCE_FLAG`]) is that positional, after the row's words or before them
//!   ([`instance_first`]); the row's other positionals then keep their order. The positional and
//!   the flag on one line are refused (`repeated`, naming the positional instance's word; the flag
//!   given twice, `--instance`) before anything is read or sent. A line without the flag is read
//!   as before; `serve --stdio` is unchanged (it takes the parameter `instance`).
//! - Types: `integer` a JSON number, `unsigned` a JSON number from 0 to 2^64 − 1, `decimal` a
//!   decimal number kept as written, `boolean` true or false, `json` the JSON value,
//!   `list<string>` comma-separated words, `duration` a number with `ms`, `s`, `m` or `h`, kept as
//!   written; a repeated parameter is a list. Everything else is a string.
//!
//! Every refusal is `actingctl_usage_invalid` naming the word (`argument`) and the rule
//! (`argument_problem`).

use actingcommand_contract::outcome::Outcome;
use actingcommand_lab_ops::cli_line;
use actingcommand_lab_ops::codes::ArgumentProblem;
// Workflow #434 LAB-5G1: moved with the core rows' serving code into `lab-ops`, which reads them
// there; the words read them from it.
pub(crate) use actingcommand_lab_ops::core_ops::{MORE_WORDS, further_flags, parse_duration};
use actingcommand_lab_ops::table::{
    Operation, OperationModule, OperationStatus, OperationTable, Param, ParamForm, ParamType,
};
use serde_json::{Map, Value};

/// The slices whose operations this program serves: the core rows LAB-1 and LAB-1b moved into
/// `lab-ops`, the rows of LAB-2, since Workflow #429 R-6 its row `task stop`, since Workflow #429
/// R-8 its rows `pack-schedule` and `pack-schedule set`, since Workflow #449 REC-ARCH its rows
/// `record archive` and `record reopen`, and since Workflow #464 P2 its row `standard-pack build`
/// (slice `PACK-BUILD`). The rescue module's rows are those `crates/lab-rescue` serves (Workflow
/// #434 LAB-3: [`rescue_served`]); a reserved row comes with the slice that opens it.
pub(crate) const BUILT_SLICES: [&str; 7] = [
    "LAB-1",
    "LAB-1b",
    "LAB-2",
    "PACK-BUILD",
    "R-6",
    "R-8",
    "REC-ARCH",
];

/// Workflow #434 LAB-5G4c (register §1.287, "Slice LAB-5G4c"): the planned rows of a slice this
/// program does not build whose command it has all the same: `record draft`, a row of slice LAB-5
/// whose command `record draft` is (`engine::COMMANDS`; the coordinator's ruling 3 of 6775192250
/// on board #434), MCP's `ac_binding_draft` before. The new MCP server offers the row as its own
/// slice's.
pub(crate) const OWN_ROWS_BEYOND_SLICES: [&str; 1] = ["record draft"];

/// Workflow #434 LAB-3: whether the rescue module serves the rescue row `name` (on Windows, where
/// it runs); its rows of a source command not built yet, and LAB-5's, are not offered.
pub(crate) fn rescue_served(name: &str) -> bool {
    #[cfg(windows)]
    {
        actingcommand_lab_rescue::rescue::serves(name)
    }
    #[cfg(not(windows))]
    {
        let _ = name;
        false
    }
}

/// Workflow #434 LAB-5A: actinglab's global options (`crate::cli_parse::parse_invocation` takes
/// them anywhere on its command line), which the command line of a row whose flags go on as
/// written ([`super::engine::as_written`]) hands on, as it did when such a row took its command's
/// further flags. `--json` is the front end's, taken out before the words are read.
pub(crate) const GLOBAL_OPTIONS: [&str; 17] = [
    "--run-root",
    "--instance",
    "--instances",
    "--profile",
    "--resource-root",
    "--via",
    "--dry-run",
    "--verbose",
    "--quiet",
    "--game",
    "--server",
    "--runtime-endpoint",
    "--capture-backend",
    "--backend",
    "--touch-backend",
    "--require-session",
    "--version",
];

/// Workflow #434 LAB-5A (and its fix 1): the flags a row's command line still hands on as written
/// (operation, the flag's name with `_`), as it did when the row took its source command's
/// further flags: `--flag value`, or `--flag` alone (`true`) when no value follows, as the member
/// of the flag's name, given twice a list. A positional's own name fills that positional, as it
/// did (its note says so); `operation` and `root` are the source command's own flags, which no
/// row declares and the module writes as further flags, as before (the row's notes say so).
/// Only the command line: `serve --stdio` takes the declared parameters only. Workflow #434 CLI-IN:
/// `runs --instance <x>` and `monitor set --instance <x>`, two entries here before, are read by the
/// rule every row with a positional instance follows ([`INSTANCE_FLAG`]), which gives them the
/// same parameter.
pub(crate) const KEPT_FLAGS: [(&str, &str); 7] = [
    ("run show", "id"),
    ("run evidence", "id"),
    ("devices bind", "root"),
    ("op status", "op"),
    ("op status", "operation"),
    ("release verify", "dir"),
    ("release stage", "dir"),
];

/// Workflow #434 LAB-5A fix 1: the rows LAB-5A completed whose command line the words type (the
/// CLI module's and the rescue module's rows that took their source command's further flags),
/// each with the options it declared before LAB-5A. An option LAB-5A declared on such a row takes
/// no word that begins with `--` as its value (refused `value_missing`): before, the further flag
/// it was took none, and its command refused the line; an option declared before takes the next
/// word, as it did.
pub(crate) const COMPLETED_ROWS: [(&str, &[&str]); 15] = [
    ("runs", &["since"]),
    ("run evidence", &["detail"]),
    ("run show", &[]),
    ("gap", &[]),
    ("devices bind", &[]),
    ("monitor set", &["interval", "expect", "recover"]),
    ("op status", &[]),
    ("release verify", &[]),
    ("release stage", &[]),
    ("agent add", &[]),
    ("agent remove", &[]),
    ("agent list", &[]),
    ("agent claude-hook", &[]),
    ("agent claude-register", &[]),
    ("agent notice", &[]),
];

/// Workflow #434 LAB-5C: the rows that take exactly one of two parameters, the first of which the
/// row required alone before (operation, the first, the second): a line with neither is refused as
/// the first missing (`required`), as before, and one with both as the second `conflicting`, on
/// the command line and in `serve --stdio` alike. `remedy set`'s rows file, or its rows inline.
pub(crate) const ONE_OF: [(&str, &str, &str); 1] = [("remedy set", "file", "rows")];

/// Workflow #434 CLI-IN (register §1.307): actinglab's global option that names the instance. On
/// the command line of a row that declares a positional instance it is that positional: one
/// spelling for every command that acts on one instance.
pub(crate) const INSTANCE_FLAG: &str = "--instance";

/// Workflow #434 CLI-IN: the positional instance of `operation` (its positional of type
/// `instance`), which its command line also takes as [`INSTANCE_FLAG`]; `None` for a row without
/// one (`remedy set` and `wake-test` declare the instance as an option of their own, and read
/// `--instance` as that option).
pub(crate) fn positional_instance(operation: &Operation) -> Option<&Param> {
    operation.params.iter().find(|param| {
        param.form == ParamForm::Positional && param.value_type == ParamType::Instance
    })
}

/// Workflow #434 CLI-IN: a command line `words` that begins with `--instance <x>` before the words
/// of an offered row that declares a positional instance: that row, and the line as the row's words,
/// then `--instance <x>`, then the rest. `None` for every other line, which is read as before.
pub(crate) fn instance_first<'a>(
    table: &'a OperationTable,
    words: &[String],
) -> Option<(&'a Operation, Vec<String>)> {
    let [flag, value, after @ ..] = words else {
        return None;
    };
    if flag.as_str() != INSTANCE_FLAG || value.starts_with("--") {
        return None;
    }
    let operation =
        named(table, after).filter(|operation| positional_instance(operation).is_some())?;
    let count = operation.name.split(' ').count();
    let line = after[..count]
        .iter()
        .chain([flag, value])
        .chain(&after[count..])
        .cloned()
        .collect();
    Some((operation, line))
}

/// One operation and its parameters.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Call {
    pub(crate) name: String,
    pub(crate) params: Map<String, Value>,
}

/// Whether this program offers `operation`: a planned row of the core, debugging or authoring
/// module whose slice is built or (Workflow #434 LAB-5G4c) that [`OWN_ROWS_BEYOND_SLICES`] names,
/// or (Workflow #434 LAB-3) a planned rescue row the rescue module serves.
pub(crate) fn offered(operation: &Operation) -> bool {
    if operation.status != OperationStatus::Planned {
        return false;
    }
    if operation.module == OperationModule::Rescue {
        return rescue_served(&operation.name);
    }
    BUILT_SLICES.contains(&operation.slice.as_str())
        || OWN_ROWS_BEYOND_SLICES.contains(&operation.name.as_str())
}

/// The offered operation whose name begins the command line `words`, the longest; `None` when no
/// offered name does.
pub(crate) fn named<'a>(table: &'a OperationTable, words: &[String]) -> Option<&'a Operation> {
    table
        .operations
        .iter()
        .filter(|operation| offered(operation))
        .filter(|operation| {
            let name = operation.name.split(' ').collect::<Vec<_>>();
            words.len() >= name.len()
                && name
                    .iter()
                    .zip(words)
                    .all(|(own, word)| *own == word.as_str())
        })
        .max_by_key(|operation| operation.name.split(' ').count())
}

/// The call the command line `words` (after the front end's own `--json`) names.
pub(crate) fn parse(table: &OperationTable, words: &[String]) -> Result<Call, Outcome> {
    // Workflow #434 CLI-IN: `--instance <x>` may stand before the words of a row that takes the
    // instance, as it may after them.
    let reordered;
    let (operation, words) = match named(table, words) {
        Some(operation) => (operation, words),
        None => {
            let (operation, line) = instance_first(table, words).ok_or_else(|| {
                usage(words.first().map(String::as_str), ArgumentProblem::Unknown)
            })?;
            reordered = line;
            (operation, reordered.as_slice())
        }
    };
    let rest = &words[operation.name.split(' ').count()..];
    // Workflow #434 CLI-IN: a line read as before until it names the instance by `--instance` is
    // read again with the instance's place left to the flag.
    let params = match read(operation, rest, false) {
        Err(Stop::Flagged) => read(operation, rest, true),
        first => first,
    }
    .map_err(Stop::refusal)?;
    required(operation, &params)?;
    Ok(Call {
        name: operation.name.clone(),
        params,
    })
}

/// Why reading a row's words stopped.
enum Stop {
    /// The words are refused.
    Refused(Outcome),
    /// Workflow #434 CLI-IN: read with the instance's place among the positionals, the line names
    /// the instance by `--instance`.
    Flagged,
}

impl From<Outcome> for Stop {
    fn from(outcome: Outcome) -> Self {
        Self::Refused(outcome)
    }
}

impl Stop {
    /// The refusal the command line answers. A line read with the instance's place left to the
    /// flag never stops as [`Stop::Flagged`]; were it to, the flag would be given twice.
    fn refusal(self) -> Outcome {
        match self {
            Self::Refused(outcome) => outcome,
            Self::Flagged => usage(Some(INSTANCE_FLAG), ArgumentProblem::Repeated),
        }
    }
}

/// The parameters of `operation` that `rest`, the words after the row's own, give. `flagged`
/// (Workflow #434 CLI-IN): the line names the instance by `--instance`, which fills the row's
/// positional instance; the other positional words skip its place, and one word too many means the
/// line gives the positional instance as well (`repeated`, naming the word in the instance's
/// place). Without it, the flag stops the reading ([`Stop::Flagged`]) and every word is read as
/// before.
fn read(operation: &Operation, rest: &[String], flagged: bool) -> Result<Map<String, Value>, Stop> {
    let mut params = Map::new();
    let positionals = operation
        .params
        .iter()
        .filter(|param| param.form == ParamForm::Positional)
        .collect::<Vec<_>>();
    let instance = positional_instance(operation);
    // Workflow #434 CLI-IN: the instance's place among the positionals, and the positional words
    // read so far.
    let instance_place = instance.and_then(|instance| {
        positionals
            .iter()
            .position(|param| param.name == instance.name)
    });
    let mut given = Vec::new();
    let mut next = 0;
    // Workflow #434 LAB-5A: the flags of a row whose command reads them again go on as written
    // (Workflow #434 LAB-5B4: and no row hands on a flag it does not declare, or a further word).
    let as_written = super::engine::as_written(operation);
    // Workflow #434 LAB-5A fix 1: the options a row LAB-5A completed declared before it.
    let completed = COMPLETED_ROWS
        .iter()
        .find(|(row, _)| *row == operation.name)
        .map(|(_, before)| *before);
    let mut index = 0;
    while index < rest.len() {
        let word = rest[index].as_str();
        // Workflow #434 CLI-IN: `--instance <x>` on a row that declares a positional instance is
        // that positional (before this slice only `runs` and `monitor set`, `words::KEPT_FLAGS`,
        // and `recover`, a global option handed on, read it so); a second one is `repeated`.
        if word == INSTANCE_FLAG
            && let Some(param) = instance
        {
            if !flagged {
                return Err(Stop::Flagged);
            }
            let value = rest
                .get(index + 1)
                .filter(|value| !value.starts_with("--"))
                .ok_or_else(|| usage(Some(word), ArgumentProblem::ValueMissing))?;
            put(&mut params, param, typed(param, value, word)?, word)?;
            index += 2;
            continue;
        }
        if let Some(flag) = word.strip_prefix("--") {
            let name = flag.replace('-', "_");
            let declared = operation
                .params
                .iter()
                .find(|param| param.form != ParamForm::Positional && param.name == name);
            // A global option in either spelling, as a further flag was written before (`_` as
            // `-`; LAB-5A fix 1).
            let global = GLOBAL_OPTIONS.contains(&format!("--{}", name.replace('_', "-")).as_str());
            let handed_on = as_written && (declared.is_some() || global);
            let kept = KEPT_FLAGS
                .iter()
                .any(|(row, flag_name)| *row == operation.name && *flag_name == name);
            if (handed_on || kept) && name != MORE_WORDS && !name.is_empty() {
                match rest.get(index + 1).filter(|next| !next.starts_with("--")) {
                    Some(value) => {
                        push(&mut params, &name, Value::String(value.clone()));
                        index += 2;
                    }
                    None => {
                        push(&mut params, &name, Value::Bool(true));
                        index += 1;
                    }
                }
                continue;
            }
            match declared {
                Some(param) if param.form == ParamForm::Switch => {
                    put(&mut params, param, Value::Bool(true), word)?;
                    index += 1;
                }
                Some(param) => {
                    // LAB-5A fix 1: an option LAB-5A declared on a row it completed takes no
                    // word that begins with `--` (`COMPLETED_ROWS`).
                    let takes_any =
                        completed.is_none_or(|before| before.contains(&param.name.as_str()));
                    let value = rest
                        .get(index + 1)
                        .filter(|value| takes_any || !value.starts_with("--"))
                        .ok_or_else(|| usage(Some(word), ArgumentProblem::ValueMissing))?;
                    put(&mut params, param, typed(param, value, word)?, word)?;
                    index += 2;
                }
                None => return Err(usage(Some(word), ArgumentProblem::Unknown).into()),
            }
        } else {
            given.push(word);
            // Workflow #434 CLI-IN: the flag names the instance; the word is the next of the
            // row's other positionals.
            if flagged
                && let (Some(instance), Some(param)) = (instance, positionals.get(next))
                && param.name == instance.name
            {
                next += 1;
            }
            match positionals.get(next) {
                Some(param) if param.repeat => {
                    append(&mut params, &param.name, typed(param, word, word)?);
                }
                Some(param) => {
                    put(&mut params, param, typed(param, word, word)?, word)?;
                    next += 1;
                }
                // Workflow #434 CLI-IN: a word beyond the other positionals, with the flag: the
                // line gives the positional instance too, the word in the instance's place, which
                // the refusal names.
                None if flagged => {
                    let repeated = instance_place
                        .and_then(|place| given.get(place))
                        .copied()
                        .unwrap_or(word);
                    return Err(usage(Some(repeated), ArgumentProblem::Repeated).into());
                }
                None => return Err(usage(Some(word), ArgumentProblem::Unknown).into()),
            }
            index += 1;
        }
    }
    Ok(params)
}

/// The call one `serve --stdio` line `{op, params}` names; the parameters are those the CLI's
/// words give, as JSON.
pub(crate) fn from_json(table: &OperationTable, line: &str) -> Result<Call, Outcome> {
    let request = serde_json::from_str::<Value>(line).map_err(|error| {
        usage(None, ArgumentProblem::WrongType)
            .caused_by(actingcommand_lab_ops::foreign_data_error(&error))
    })?;
    let Value::Object(request) = request else {
        return Err(usage(None, ArgumentProblem::WrongType));
    };
    if let Some(member) = request
        .keys()
        .find(|member| !matches!(member.as_str(), "op" | "params"))
    {
        return Err(usage(Some(member.as_str()), ArgumentProblem::Unknown));
    }
    let name = match request.get("op") {
        Some(Value::String(name)) => name,
        Some(_) => return Err(usage(Some("op"), ArgumentProblem::WrongType)),
        None => return Err(usage(Some("op"), ArgumentProblem::Required)),
    };
    let operation = table
        .operation(name)
        .filter(|operation| offered(operation))
        .ok_or_else(|| usage(Some(name.as_str()), ArgumentProblem::Unknown))?;
    let params = match request.get("params") {
        None => Map::new(),
        Some(Value::Object(params)) => params.clone(),
        Some(_) => return Err(usage(Some("params"), ArgumentProblem::WrongType)),
    };
    // Workflow #434 LAB-5A: a member the row does not declare is refused, for every row.
    for (member, value) in &params {
        match operation.params.iter().find(|param| &param.name == member) {
            Some(param) => checked(param, value, member)?,
            None => return Err(usage(Some(member.as_str()), ArgumentProblem::Unknown)),
        }
        // Workflow #434 LAB-5G4a: and so is a value the command line of its row would read as a
        // flag, for every row.
        not_a_flag(value, member)?;
    }
    required(operation, &params)?;
    Ok(Call {
        name: operation.name.clone(),
        params,
    })
}

/// Workflow #434 LAB-5G4a: a `serve` parameter whose value is a string that begins with `--`, or
/// a list holding one, is refused (`not_recognised`, the parameter as `argument`, as today's MCP
/// server refuses it): the command line `serve` builds for its row would read it as a flag.
/// Numbers, booleans, objects and strings that begin with one dash pass.
fn not_a_flag(value: &Value, member: &str) -> Result<(), Outcome> {
    let flag = |value: &Value| value.as_str().is_some_and(|text| text.starts_with("--"));
    let found = match value {
        Value::Array(items) => items.iter().any(flag),
        other => flag(other),
    };
    if found {
        return Err(usage(Some(member), ArgumentProblem::NotRecognised));
    }
    Ok(())
}

/// A decimal number as written: finite, as Rust reads an `f64`.
fn decimal(text: &str) -> Option<f64> {
    text.parse::<f64>().ok().filter(|number| number.is_finite())
}

/// The name a refusal gives a parameter: `<name>` for a positional, `--name` for another.
fn shown(param: &Param) -> String {
    match param.form {
        ParamForm::Positional => format!("<{}>", param.name),
        _ => format!("--{}", param.name.replace('_', "-")),
    }
}

/// Every required parameter is given; Workflow #434 LAB-5C: and exactly one of each pair of
/// [`ONE_OF`].
fn required(operation: &Operation, params: &Map<String, Value>) -> Result<(), Outcome> {
    for (row, first, second) in ONE_OF {
        if row != operation.name {
            continue;
        }
        let shown_of = |name: &str| {
            operation
                .params
                .iter()
                .find(|param| param.name == name)
                .map(shown)
        };
        match (params.contains_key(first), params.contains_key(second)) {
            (false, false) => {
                return Err(usage(shown_of(first).as_deref(), ArgumentProblem::Required));
            }
            (true, true) => {
                return Err(usage(
                    shown_of(second).as_deref(),
                    ArgumentProblem::Conflicting,
                ));
            }
            _ => {}
        }
    }
    match operation
        .params
        .iter()
        .find(|param| param.required && !params.contains_key(&param.name))
    {
        Some(param) => Err(usage(
            Some(shown(param).as_str()),
            ArgumentProblem::Required,
        )),
        None => Ok(()),
    }
}

/// One declared parameter, given once unless it repeats.
fn put(
    params: &mut Map<String, Value>,
    param: &Param,
    value: Value,
    word: &str,
) -> Result<(), Outcome> {
    if param.repeat {
        append(params, &param.name, value);
        return Ok(());
    }
    if params.insert(param.name.clone(), value).is_some() {
        return Err(usage(Some(word), ArgumentProblem::Repeated));
    }
    Ok(())
}

/// One more value of a repeated parameter or of the further words: always a list.
fn append(params: &mut Map<String, Value>, name: &str, value: Value) {
    match params.get_mut(name) {
        Some(Value::Array(items)) => items.push(value),
        _ => {
            params.insert(name.to_owned(), Value::Array(vec![value]));
        }
    }
}

/// One more value of a further flag: the value itself, a list once it is given twice.
fn push(params: &mut Map<String, Value>, name: &str, value: Value) {
    match params.get_mut(name) {
        Some(Value::Array(items)) => items.push(value),
        Some(given) => {
            let first = given.take();
            *given = Value::Array(vec![first, value]);
        }
        None => {
            params.insert(name.to_owned(), value);
        }
    }
}

/// `text` as the value of `param`; `word` names it in a refusal.
fn typed(param: &Param, text: &str, word: &str) -> Result<Value, Outcome> {
    let wrong = || usage(Some(word), ArgumentProblem::WrongType);
    Ok(match &param.value_type {
        ParamType::Integer => Value::from(text.parse::<i64>().map_err(|_| wrong())?),
        ParamType::Unsigned => Value::from(text.parse::<u64>().map_err(|_| wrong())?),
        ParamType::Decimal => {
            decimal(text).ok_or_else(wrong)?;
            Value::String(text.to_owned())
        }
        ParamType::Boolean => match text {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => return Err(wrong()),
        },
        ParamType::Json => serde_json::from_str(text).map_err(|_| wrong())?,
        ParamType::ListOfStrings => Value::Array(
            text.split(',')
                .map(|item| Value::String(item.to_owned()))
                .collect(),
        ),
        ParamType::Duration => {
            parse_duration(text).ok_or_else(wrong)?;
            Value::String(text.to_owned())
        }
        ParamType::Enum(words) => {
            if !words.iter().any(|known| known == text) {
                return Err(usage(Some(word), ArgumentProblem::NotRecognised));
            }
            Value::String(text.to_owned())
        }
        _ => Value::String(text.to_owned()),
    })
}

/// A serve parameter's JSON value checked against its declared type, as [`typed`] gives it.
fn checked(param: &Param, value: &Value, member: &str) -> Result<(), Outcome> {
    let wrong = || usage(Some(member), ArgumentProblem::WrongType);
    let one = |value: &Value| -> Result<(), Outcome> {
        let fits = match &param.value_type {
            ParamType::Integer => value.is_i64() || value.is_u64(),
            ParamType::Unsigned => value.is_u64(),
            // A JSON number, or the decimal text the CLI's words give.
            ParamType::Decimal => {
                value.as_f64().is_some_and(f64::is_finite)
                    || value.as_str().and_then(decimal).is_some()
            }
            ParamType::Boolean => value.is_boolean(),
            ParamType::Json => true,
            ParamType::ListOfStrings => value
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_string)),
            ParamType::Duration => value.as_str().and_then(parse_duration).is_some(),
            ParamType::Enum(words) => value
                .as_str()
                .is_some_and(|text| words.iter().any(|known| known == text)),
            _ => value.is_string(),
        };
        if fits { Ok(()) } else { Err(wrong()) }
    };
    match (param.repeat, value) {
        (true, Value::Array(items)) if !items.is_empty() => items.iter().try_for_each(&one),
        (true, _) => Err(wrong()),
        (false, _) if param.form == ParamForm::Switch => {
            if value.is_boolean() {
                Ok(())
            } else {
                Err(wrong())
            }
        }
        (false, value) => one(value),
    }
}

fn usage(argument: Option<&str>, problem: ArgumentProblem) -> Outcome {
    cli_line::usage_invalid(argument, problem)
}
