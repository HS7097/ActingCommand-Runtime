// SPDX-License-Identifier: AGPL-3.0-only

//! Tab completion of `actinglab` in PowerShell and bash (Workflow #434 CLI-COMP; register §1.318;
//! `contracts/lab-two-entries.md` §9.3, "Completion"; the coordinator's eight rulings of
//! 2026-11-11, not Alice's words).
//!
//! - **The scripts** ([`POWERSHELL`], [`BASH`]; `actinglab completion powershell|bash`,
//!   [`run_completion`]). Plain ASCII, printed for a person to load from the shell's profile:
//!   actinglab and the installer never edit a profile. Each registers itself for `actinglab` and
//!   `actinglab.exe` (`Register-ArgumentCompleter -Native`, `complete -o default -F`) and is thin:
//!   at each Tab it calls the program the line names (its first word, as typed: `actinglab` on the
//!   path, or the path the person typed) back with the words typed so far, each in the word form
//!   ([`WORD_MARK`]), and the index of the word being completed, so that a new row or a renamed
//!   word needs no new script. With no candidate the shell completes file names itself, which is
//!   how a `path`, `file` or `dir` parameter is completed (ruling 6).
//! - **The callback** ([`CALLBACK`], not for people). `main` answers a line that begins with it
//!   before anything else reads the line ([`callback_line`]); the dispatch arm answers it as data
//!   on any other line ([`run_callback`]). It prints the candidates, one per line, nothing else;
//!   with `--json` the line of an own command with the member `candidates`. It runs nothing,
//!   sends nothing that changes anything and writes no file, and it exits 0 with no candidate when
//!   it has none or anything goes wrong, writing nothing on stderr (ruling 2).
//! - **The candidates** ([`candidates`]), from what this build knows (ruling 3): the command
//!   words `actinglab help` lists, by prefix, at each position (`help <words>` as the words
//!   themselves); after a row's own name its options and switches (not one already on the line
//!   unless it repeats), an enumeration's words after its option or in its positional's place, and
//!   the global options its line takes; after an own command without a row (or a row's other
//!   spelling) the global options only (ruling 5: their own flags are the next slice's).
//! - **The instances** ([`instance_aliases`]): wherever a row's parameter is an instance (its
//!   positional, `--instance`, or the instance option of its own), the aliases of the running
//!   Runtime, asked with `Status`, the read that resolves `<instance>` today, within the wait
//!   ([`WAIT_FLAG`], default [`DEFAULT_WAIT_MS`]); when no Runtime answers in time, those of the
//!   selected configuration the offline rows read; never an error (rulings 3, 4 and 7).

use crate::CliOutcome;
use crate::cli_help;
use crate::cli_module::{self, authoring, engine, own_line, words};
use crate::flag_args::FlagArgs;
use crate::flag_args::reads::FlagReads;
use actingcommand_lab_ops::cli_line::Exit;
use actingcommand_lab_ops::codes::ArgumentProblem;
use actingcommand_lab_ops::packs;
use actingcommand_lab_ops::table::{
    Operation, OperationModule, OperationTable, Param, ParamForm, ParamType,
};
use actingcommand_runtime_client::RuntimeClient;
use serde_json::{Value, json};
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

/// The command that prints a shell's completion script.
pub(crate) const COMPLETION: &str = "completion";
/// The completion scripts' callback: an own command that is not for people.
pub(crate) const CALLBACK: &str = "__complete";
/// The callback's parameter that bounds, in milliseconds, its wait for the running Runtime's
/// answer (ruling 4); the scripts pass none. `0` asks no Runtime.
pub(crate) const WAIT_FLAG: &str = "--wait-ms";
/// The callback's wait without [`WAIT_FLAG`].
pub(crate) const DEFAULT_WAIT_MS: u64 = 500;
/// The mark that begins each typed word on the callback's line (the word form). After it, the
/// word with `%` written `%25`, `"` written `%22`, a space `%20` and a tab `%09`; the callback
/// reads any `%` and two hex digits as that byte. No word of the form is empty, holds a space or
/// a quote, or reads as one of actinglab's options, so Windows PowerShell 5.1, which drops an
/// empty word and breaks a quoted one on a native program's command line, passes each whole.
pub(crate) const WORD_MARK: char = ':';

/// Workflow #434 CLI-COMP: the flags and words `completion` reads: no flag, and the shell.
pub(crate) const COMPLETION_READS: FlagReads = FlagReads {
    row: COMPLETION,
    parts: &[],
    positionals: &["<shell>"],
};

/// The shells a script is printed for, each with its script.
pub(crate) const SHELLS: [(&str, &str); 2] = [("powershell", POWERSHELL), ("bash", BASH)];

/// The PowerShell script (Windows PowerShell 5.1 and PowerShell 7).
pub(crate) const POWERSHELL: &str = r#"# Tab completion of actinglab for PowerShell (Workflow #434 CLI-COMP).
# On in this session:
#   actinglab completion powershell | Out-String | Invoke-Expression
# On in every session:
#   Add-Content $PROFILE 'actinglab completion powershell | Out-String | Invoke-Expression'
# Each Tab asks the actinglab the line names for the candidates of the word being typed; with
# none, PowerShell completes file names.
Register-ArgumentCompleter -Native -CommandName 'actinglab', 'actinglab.exe' -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)
    $words = @()
    foreach ($element in $commandAst.CommandElements) {
        if ($element.Extent.StartOffset -ge $cursorPosition) { break }
        if ($element -is [System.Management.Automation.Language.StringConstantExpressionAst]) {
            $words += , $element.Value
        } else {
            $words += , $element.Extent.Text
        }
    }
    if ($wordToComplete -eq '') { $words += , '' }
    $marked = @(foreach ($word in $words) {
        ':' + $word.Replace('%', '%25').Replace('"', '%22').Replace(' ', '%20').Replace("`t", '%09')
    })
    & $words[0] __complete ($words.Count - 1) $marked 2>$null | ForEach-Object {
        $text = $_
        if ($text -notmatch '^[\w./\\:=+%-]+$') { $text = "'" + $text.Replace("'", "''") + "'" }
        [System.Management.Automation.CompletionResult]::new($text, $_, 'ParameterValue', $_)
    }
}
"#;

/// The bash script.
pub(crate) const BASH: &str = r#"# Tab completion of actinglab for bash (Workflow #434 CLI-COMP).
# On in this shell:
#   source <(actinglab completion bash)
# On in every shell:
#   echo 'source <(actinglab completion bash)' >> ~/.bashrc
# Each Tab asks the actinglab the line names for the candidates of the word being typed; with
# none, bash completes file names (complete -o default).
_actinglab_complete() {
    local word
    local -a marked=()
    for word in "${COMP_WORDS[@]:0:COMP_CWORD+1}"; do
        word=${word//\%/%25}
        word=${word//\"/%22}
        word=${word// /%20}
        word=${word//$'\t'/%09}
        marked+=(":$word")
    done
    COMPREPLY=()
    while IFS= read -r word; do
        COMPREPLY+=("$word")
    done < <("${COMP_WORDS[0]}" __complete "$COMP_CWORD" "${marked[@]}" 2>/dev/null)
}
complete -o default -F _actinglab_complete actinglab actinglab.exe
"#;

/// actinglab's global options that the help lists but completion does not offer: `--via`, which
/// the program that forwards the work sets, and the three that no command reads.
const NOT_OFFERED: [&str; 4] = ["--via", "--instances", "--profile", "--quiet"];

/// `completion <shell>`: the script of `powershell` or `bash` as `{shell, script}`. Without a
/// shell it is refused `required`, another word `not_recognised`, as a read list refuses.
pub(crate) fn run_completion(args: &[String]) -> CliOutcome<Value> {
    let words = FlagArgs::parse_reading(args, &COMPLETION_READS)?.positionals;
    let Some(shell) = words.first() else {
        return Err(cli_module::refused_word(
            COMPLETION,
            "<shell>",
            ArgumentProblem::Required,
        ));
    };
    let Some((_, script)) = SHELLS.iter().find(|(name, _)| *name == shell.as_str()) else {
        return Err(cli_module::refused_word(
            COMPLETION,
            shell,
            ArgumentProblem::NotRecognised,
        ));
    };
    Ok(json!({"shell": shell, "script": script}))
}

/// Whether `completion` without `--json` (and with no `--via`, which `main` answers with the
/// envelope) is the line `invocation` asks for: `main` then prints its script as it is.
pub(crate) fn for_people(invocation: &crate::OwnInvocation) -> bool {
    invocation
        .as_ref()
        .is_ok_and(|own| own.command.as_slice() == [COMPLETION] && !own.global.json)
}

/// The script `completion`'s answer `exit` carries, as a person loads it; `None` for a failure,
/// which prints as every own command's does.
pub(crate) fn script_text(exit: &Exit) -> Option<String> {
    if exit.code != 0 {
        return None;
    }
    let script = exit.line.as_ref().ok()?.get("script")?.as_str()?;
    Some(script.trim_end_matches('\n').to_owned())
}

/// Where the callback asks for the instances' aliases.
pub(crate) struct Sources {
    /// The state root of the Runtime the CLI module's rows reach, when it is located.
    pub(crate) state_root: Option<PathBuf>,
    /// The install root whose selected configuration the offline rows read, when there is one.
    pub(crate) install_root: Option<PathBuf>,
    /// How long the Runtime's answer is waited for.
    pub(crate) wait: Duration,
}

impl Sources {
    /// This process's: the CLI module's place (`cli_module::core_ops`), read only when an
    /// instance is to be completed.
    pub(crate) fn here(wait: Duration) -> Self {
        let located = (cli_module::core_ops::caller().place)();
        Self {
            state_root: located.state_root.ok(),
            install_root: located.root,
            wait,
        }
    }
}

/// `main`'s first step (ruling 2): when the command line is the callback's, its first word
/// [`CALLBACK`] as the scripts write it, prints the callback's answer on stdout and ends with exit
/// 0, before anything else reads the line; `None` for every other line. A word that is no UTF-8
/// is read with its bytes replaced, and a stdout that refuses the text is left as it is: a
/// completion never prints an error into a person's prompt.
pub(crate) fn callback_line(arguments: impl IntoIterator<Item = OsString>) -> Option<ExitCode> {
    let mut arguments = arguments.into_iter();
    if arguments.next()?.to_str() != Some(CALLBACK) {
        return None;
    }
    let args = arguments
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let text = callback_text(&args, &Sources::here);
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    let _ = writer
        .write_all(text.as_bytes())
        .and_then(|()| writer.flush());
    Some(ExitCode::SUCCESS)
}

/// What the callback prints for `args`, the words after [`CALLBACK`]: the candidates, one per
/// line; with `--json` the line its dispatch arm answers (`{candidates}`); nothing for a line it
/// cannot read. `here` gives the places the instances are asked from, for the line's wait.
pub(crate) fn callback_text(args: &[String], here: &dyn Fn(Duration) -> Sources) -> String {
    let found = asked(args)
        .map(|asked| answered(&asked, here))
        .unwrap_or_default();
    if args.iter().any(|arg| arg == cli_module::JSON) {
        let exit = own_line::success(CALLBACK, data(&found));
        return match cli_module::stdout_text(&exit, true) {
            Ok(line) => format!("{line}\n"),
            Err(_) => String::new(),
        };
    }
    found
        .iter()
        .map(|candidate| format!("{candidate}\n"))
        .collect()
}

/// The dispatch arm of [`CALLBACK`]: a line that reaches the dispatch (one where the word is not
/// the first, which no script writes) answers the candidates as data, `{candidates}`, empty for a
/// line it cannot read.
pub(crate) fn run_callback(args: &[String]) -> CliOutcome<Value> {
    let found = asked(args)
        .map(|asked| answered(&asked, &Sources::here))
        .unwrap_or_default();
    Ok(data(&found))
}

fn data(found: &[String]) -> Value {
    json!({"candidates": found})
}

/// The callback's line, read.
struct Asked {
    /// The index of the word being completed among `words`; the program's word is 0.
    index: usize,
    /// The words typed so far, the program's word first, decoded from the word form.
    words: Vec<String>,
    /// The wait for the Runtime's answer.
    wait: Duration,
}

/// The callback's arguments `args` read: `--json`, `--wait-ms <ms>`, the index and the words in
/// the word form, in any order; `None` for a line that holds anything else, no index, a wait or an
/// index that is no whole number, or a word that does not decode to UTF-8.
fn asked(args: &[String]) -> Option<Asked> {
    let mut index = None;
    let mut wait_ms = DEFAULT_WAIT_MS;
    let mut words = Vec::new();
    let mut at = 0;
    while let Some(arg) = args.get(at) {
        at += 1;
        if let Some(word) = arg.strip_prefix(WORD_MARK) {
            words.push(decoded(word)?);
            continue;
        }
        match arg.as_str() {
            cli_module::JSON => {}
            WAIT_FLAG => {
                wait_ms = args.get(at)?.parse().ok()?;
                at += 1;
            }
            number if index.is_none() => index = Some(number.parse::<usize>().ok()?),
            _ => return None,
        }
    }
    Some(Asked {
        index: index?,
        words,
        wait: Duration::from_millis(wait_ms),
    })
}

/// A word of the word form after its mark: each `%` and two hex digits as that byte, every other
/// byte as it is; `None` when the bytes are no UTF-8.
fn decoded(word: &str) -> Option<String> {
    let bytes = word.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while let Some(&byte) = bytes.get(at) {
        if byte == b'%'
            && let Some(pair) = bytes.get(at + 1..at + 3)
            && pair.iter().all(u8::is_ascii_hexdigit)
            && let Ok(text) = std::str::from_utf8(pair)
            && let Ok(value) = u8::from_str_radix(text, 16)
        {
            out.push(value);
            at += 3;
            continue;
        }
        out.push(byte);
        at += 1;
    }
    String::from_utf8(out).ok()
}

fn answered(asked: &Asked, here: &dyn Fn(Duration) -> Sources) -> Vec<String> {
    candidates(&asked.words, asked.index, &mut || {
        instance_aliases(&here(asked.wait))
    })
}

/// The candidates of the word at `index` of `line` (the program's word first), by its prefix,
/// sorted, each once; `instances` gives the instances' aliases, asked only at an instance's
/// place. None for an index that is no typed word's or the program's, for words that begin no
/// command, and when the operation table this build embeds does not read (which no test can
/// reach: the table is the build's own).
pub(crate) fn candidates(
    line: &[String],
    index: usize,
    instances: &mut dyn FnMut() -> Vec<String>,
) -> Vec<String> {
    let Ok(table) = cli_module::table() else {
        return Vec::new();
    };
    if index == 0 || index > line.len() {
        return Vec::new();
    }
    let current = line.get(index).map_or("", String::as_str);
    let names = names(table);
    let mut found = match position(&names, &line[1..index]) {
        Position::GlobalValue(option) if option == words::INSTANCE_FLAG => instances(),
        Position::GlobalValue(_) | Position::Unknown => Vec::new(),
        Position::Command {
            typed,
            help,
            skipped,
        } => command_candidates(&names, &typed, help, &skipped, current, instances),
        Position::Row(row, after) => row_candidates(row, &after, current, instances),
        Position::Own if flagged(current) => people_options(),
        Position::Own => Vec::new(),
    };
    found.retain(|candidate| candidate.starts_with(current));
    found.sort();
    found.dedup();
    found
}

/// One command a line may begin with: its words, and the row they name when they are the row's
/// own name.
struct Name {
    words: Vec<String>,
    row: Option<&'static Operation>,
}

/// Every command `actinglab help` lists here, by its words: each offered row (the authoring rows
/// only where section 8.3 lists them, as `help`'s list leaves them out) by its name, by the words
/// of the own command that serves it under other words and by those of each own command it
/// replaces (`cli_help::row_names`), and the own commands without a row
/// (`cli_help::OWN_WITHOUT_ROW`), but the callback, which is not for people.
fn names(table: &'static OperationTable) -> Vec<Name> {
    let authoring_listed = authoring::module().is_ok_and(|module| module.installed());
    let mut names = Vec::new();
    for operation in table.operations.iter().filter(|operation| {
        words::offered(operation)
            && (authoring_listed || operation.module != OperationModule::Authoring)
    }) {
        for spelled in cli_help::row_names(operation) {
            names.push(Name {
                row: (spelled == operation.name).then_some(operation),
                words: split(&spelled),
            });
        }
    }
    names.extend(
        cli_help::OWN_WITHOUT_ROW
            .iter()
            .filter(|own| own.words != CALLBACK)
            .map(|own| Name {
                words: split(own.words),
                row: None,
            }),
    );
    names
}

fn split(spelled: &str) -> Vec<String> {
    spelled.split(' ').map(str::to_owned).collect()
}

/// Whether the words `own` begin with the words `typed`.
fn begins(own: &[String], typed: &[&str]) -> bool {
    own.len() >= typed.len()
        && own
            .iter()
            .zip(typed)
            .all(|(own, word)| own.as_str() == *word)
}

/// What the words before the one being completed make of the line.
enum Position<'a> {
    /// The value of actinglab's global option that takes one.
    GlobalValue(&'a str),
    /// A command position: the command words typed so far (after `help`, the words it
    /// describes), and the global options and values typed among them.
    Command {
        typed: Vec<&'a str>,
        help: bool,
        skipped: Vec<String>,
    },
    /// After the words of a row's own name: every other word typed (the global options among
    /// the command words, then the words after them).
    Row(&'static Operation, Vec<String>),
    /// After the words of an own command without a row, or of a row's other spelling.
    Own,
    /// Words that begin no command.
    Unknown,
}

/// Where the words `before` leave the word being completed: the longest run of command words
/// that begins a name, actinglab's global options (with their values) skipped among them, up to
/// the first other flag or the first word that begins no name.
fn position<'a>(names: &[Name], before: &'a [String]) -> Position<'a> {
    let mut typed: Vec<&'a str> = Vec::new();
    let mut skipped = Vec::new();
    let mut help = false;
    let mut at = 0;
    while let Some(word) = before.get(at).map(String::as_str) {
        if cli_help::VALUE_OPTIONS.contains(&word) {
            let Some(value) = before.get(at + 1) else {
                return Position::GlobalValue(word);
            };
            skipped.extend([word.to_owned(), value.clone()]);
            at += 2;
            continue;
        }
        if global_switch(word) {
            skipped.push(word.to_owned());
            at += 1;
            continue;
        }
        if word.starts_with("--") {
            break;
        }
        if typed.is_empty() && !help && word == cli_help::HELP {
            help = true;
        } else {
            typed.push(word);
            if !names.iter().any(|name| begins(&name.words, &typed)) {
                typed.pop();
                break;
            }
        }
        at += 1;
    }
    let rest = &before[at..];
    if rest.is_empty() {
        return Position::Command {
            typed,
            help,
            skipped,
        };
    }
    if help {
        return Position::Unknown;
    }
    let named = names
        .iter()
        .filter(|name| name.words == typed)
        .collect::<Vec<_>>();
    if let Some(row) = named.iter().find_map(|name| name.row) {
        let mut after = skipped;
        after.extend(rest.iter().cloned());
        return Position::Row(row, after);
    }
    if named.is_empty() {
        Position::Unknown
    } else {
        Position::Own
    }
}

/// Whether `word` is one of actinglab's global options that take no value, `--json` and
/// `--help` among them.
fn global_switch(word: &str) -> bool {
    word == cli_module::JSON
        || word == cli_help::HELP_FLAG
        || (words::GLOBAL_OPTIONS.contains(&word) && !cli_help::VALUE_OPTIONS.contains(&word))
}

/// Whether the word being completed is a flag.
fn flagged(current: &str) -> bool {
    current.starts_with('-')
}

/// At a command position: the next words of every name that begins with `typed`; and when
/// `typed` is a row's own name, what follows it ([`row_candidates`]); a flag there is a global
/// option of an own command or of the first word, or a group's `--help`. After `help`, only the
/// words.
fn command_candidates(
    names: &[Name],
    typed: &[&str],
    help: bool,
    skipped: &[String],
    current: &str,
    instances: &mut dyn FnMut() -> Vec<String>,
) -> Vec<String> {
    let mut found = names
        .iter()
        .filter(|name| name.words.len() > typed.len() && begins(&name.words, typed))
        .map(|name| name.words[typed.len()].clone())
        .collect::<Vec<_>>();
    if help {
        return found;
    }
    let named = names
        .iter()
        .filter(|name| name.words == typed)
        .collect::<Vec<_>>();
    if let Some(row) = named.iter().find_map(|name| name.row) {
        found.extend(row_candidates(row, skipped, current, instances));
    } else if flagged(current) {
        if typed.is_empty() || !named.is_empty() {
            found.extend(people_options());
        } else {
            found.push(cli_help::HELP_FLAG.to_owned());
        }
    }
    found
}

/// What the line of the row `row` takes after the words `after`: a value for an option that
/// waits for one; else, for a word that is no flag, its next positional's candidates, or its
/// flags where no positional is left and nothing is typed yet; for a flag, its flags.
fn row_candidates(
    row: &'static Operation,
    after: &[String],
    current: &str,
    instances: &mut dyn FnMut() -> Vec<String>,
) -> Vec<String> {
    let line = RowLine::read(row, after);
    if let Some(pending) = line.pending {
        return values(pending, instances);
    }
    if !flagged(current) {
        if let Some(param) = line.next_positional(row) {
            return values(Pending::Param(param), instances);
        }
        if !current.is_empty() {
            return Vec::new();
        }
    }
    flags(row, &line)
}

/// What waits for its value at the word being completed.
#[derive(Clone, Copy)]
enum Pending {
    /// A parameter of the row: its instances, or its enumeration's words.
    Param(&'static Param),
    /// `--instance`, the row's positional instance or the own command's.
    Instance,
    /// Another global option's value: none.
    Other,
}

/// The values of what waits: an instance's aliases, an enumeration's words, nothing else (a path,
/// a key, an id or free text; the shell completes file names).
fn values(pending: Pending, instances: &mut dyn FnMut() -> Vec<String>) -> Vec<String> {
    match pending {
        Pending::Instance => instances(),
        Pending::Param(param) => match &param.value_type {
            ParamType::Instance => instances(),
            ParamType::Enum(choices) => choices.clone(),
            _ => Vec::new(),
        },
        Pending::Other => Vec::new(),
    }
}

/// The words typed after a row's own name, read as the row reads them.
struct RowLine {
    /// The option the word being completed is the value of.
    pending: Option<Pending>,
    /// The flags on the line, each by its name with `-` as `_`.
    seen: Vec<String>,
    /// The positional words given.
    positionals: usize,
    /// Whether `--instance` names the instance (register §1.307).
    instance_flagged: bool,
}

impl RowLine {
    fn read(row: &'static Operation, after: &[String]) -> Self {
        let mut line = Self {
            pending: None,
            seen: Vec::new(),
            positionals: 0,
            instance_flagged: false,
        };
        let mut at = 0;
        while let Some(word) = after.get(at).map(String::as_str) {
            at += 1;
            let last = at == after.len();
            let Some(flag) = word.strip_prefix("--") else {
                line.positionals += 1;
                continue;
            };
            let name = flag.replace('-', "_");
            let declared = row
                .params
                .iter()
                .find(|param| param.form != ParamForm::Positional && param.name == name);
            line.seen.push(name);
            let takes_value = match declared {
                Some(param) => (param.form == ParamForm::Option).then_some(Pending::Param(param)),
                None if word == words::INSTANCE_FLAG => {
                    line.instance_flagged = true;
                    Some(Pending::Instance)
                }
                None if cli_help::VALUE_OPTIONS.contains(&word) => Some(Pending::Other),
                None => None,
            };
            if let Some(pending) = takes_value {
                if last {
                    line.pending = Some(pending);
                } else {
                    at += 1;
                }
            }
        }
        line
    }

    /// The row's positional the next positional word fills: in order, the last one again when it
    /// repeats.
    fn next_positional(&self, row: &'static Operation) -> Option<&'static Param> {
        let placed = placed(row, self.instance_flagged);
        placed
            .get(self.positionals)
            .copied()
            .or_else(|| placed.last().copied().filter(|param| param.repeat))
    }
}

/// The positionals of `row` its command line takes in their places: all, but the instance where
/// the line names it by `--instance` or where actinglab's own command serves the row under its
/// name and takes the instance only so (register §1.307).
fn placed(row: &'static Operation, instance_flagged: bool) -> Vec<&'static Param> {
    let flag_only = instance_flagged || cli_help::own_named(row);
    row.params
        .iter()
        .filter(|param| {
            param.form == ParamForm::Positional
                && !(flag_only && param.value_type == ParamType::Instance)
        })
        .collect()
}

/// Whether the line of `row` may still name its positional instance by `--instance`: not given
/// yet, by the flag or in its place.
fn instance_open(row: &'static Operation, line: &RowLine) -> bool {
    if line.instance_flagged {
        return false;
    }
    let place = row
        .params
        .iter()
        .filter(|param| param.form == ParamForm::Positional)
        .position(|param| param.value_type == ParamType::Instance);
    match place {
        Some(_) if cli_help::own_named(row) => true,
        Some(place) => line.positionals <= place,
        None => false,
    }
}

/// The flags of the line of `row`: its options and switches (`--name`, a `_` as `-`); the global
/// options its line takes (all a person gives where the row hands them to its command as written,
/// else `--json` and `--help`); `--instance` while it may still name the row's positional
/// instance. A flag already on the line is left out unless the row's parameter repeats.
fn flags(row: &'static Operation, line: &RowLine) -> Vec<String> {
    let mut found = row
        .params
        .iter()
        .filter(|param| param.form != ParamForm::Positional)
        .map(|param| flag(&param.name))
        .collect::<Vec<_>>();
    if engine::as_written(row) {
        found.extend(
            people_options()
                .into_iter()
                .filter(|option| option != words::INSTANCE_FLAG),
        );
    } else {
        found.extend([cli_module::JSON, cli_help::HELP_FLAG].map(str::to_owned));
    }
    if instance_open(row, line) {
        found.push(words::INSTANCE_FLAG.to_owned());
    }
    found.retain(|option| {
        let name = option.trim_start_matches('-').replace('-', "_");
        !line.seen.contains(&name)
            || row.params.iter().any(|param| {
                param.repeat && param.form != ParamForm::Positional && param.name == name
            })
    });
    found
}

/// A parameter's flag: `--` and its name with `_` as `-` (§9.1).
fn flag(name: &str) -> String {
    format!("--{}", name.replace('_', "-"))
}

/// actinglab's global options a person gives, as `help` lists them (`--require-session`, retired,
/// is not among them), but [`NOT_OFFERED`].
pub(crate) fn people_options() -> Vec<String> {
    cli_help::GLOBAL_OPTION_PHRASES
        .iter()
        .filter_map(|(shown, _)| shown.split(' ').next())
        .filter(|option| !NOT_OFFERED.contains(option))
        .map(str::to_owned)
        .collect()
}

/// The aliases of the instances (rulings 3, 4 and 7): the running Runtime's, asked with `Status`,
/// the read that resolves `<instance>` today, which records nothing, within `sources.wait`; when no
/// Runtime answers in time, or the wait is 0, those of the selected configuration the offline rows
/// read (`packs`); none when neither can be read. Never an error.
pub(crate) fn instance_aliases(sources: &Sources) -> Vec<String> {
    sources
        .state_root
        .as_deref()
        .and_then(|state_root| runtime_aliases(state_root, sources.wait))
        .unwrap_or_else(|| configured_aliases(sources.install_root.as_deref()))
}

/// The aliases of the Runtime of `state_root`: one connection on the Lab channel, its handshake
/// (`Health`, `BindChannel`) and one `Status`, every exchange bound by one deadline `wait` from
/// now; `None` when it does not answer by then.
fn runtime_aliases(state_root: &Path, wait: Duration) -> Option<Vec<String>> {
    if wait.is_zero() {
        return None;
    }
    let deadline = Instant::now().checked_add(wait)?;
    let config = crate::lab_client_config_via(state_root, None).with_deadline(deadline);
    let client = RuntimeClient::connect(config).ok()?;
    let status = client.status().ok()?;
    Some(
        status
            .instances()
            .iter()
            .map(|instance| instance.instance_alias().to_owned())
            .collect(),
    )
}

/// The aliases of the selected configuration of `install_root` (or of the install root this
/// program runs from), as the offline row `packs` reads it; none when it cannot be read.
fn configured_aliases(install_root: Option<&Path>) -> Vec<String> {
    let Ok((installation, config)) = packs::selected_config(install_root) else {
        return Vec::new();
    };
    let Ok(answer) = packs::read(installation.as_ref(), &config, &packs::Query::default()) else {
        return Vec::new();
    };
    let Ok(document) = serde_json::to_value(answer.packs()) else {
        return Vec::new();
    };
    document["instances"]
        .as_array()
        .map(|instances| {
            instances
                .iter()
                .filter_map(|instance| instance["alias"].as_str())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}
