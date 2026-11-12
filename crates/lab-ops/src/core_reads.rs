// SPDX-License-Identifier: AGPL-3.0-only

//! The parameters the code of each row [`crate::core_ops`] serves reads (Workflow #434 LAB-5B1;
//! register §1.287, "Slice LAB-5B1"; `contracts/lab-two-entries.md` §9.1): one constant per
//! planned row, which that code reads the row's parameters through (`Params`, this crate's) and
//! the slice's tests hold equal to the row's declared parameters, in both directions.
//!
//! - **Read in this process** ([`READ_HERE`]): the rows whose parameters `core_ops` reads itself,
//!   each with the names its code reads ([`Reads`]).
//! - **Handed to a command of this crate** ([`HANDED`], with the state root; [`OFFLINE`], without
//!   one): the rows whose answer is the command line of a parser of this crate, each with that
//!   command line ([`Handed`]): every parameter of the row in its place, as the word or the flag it
//!   becomes ([`Written`]), beside the module's own `--state-root` and `--root`. The code builds
//!   the command line from it; it is the mapping from parameter to flag.
//! - **Exceptions** ([`EXCEPTIONS`]): the names outside the operation table that the code reads or
//!   hands on, or that a parser takes, by design, each with its reason.
//!
//! Every read of a parameter by its name goes through `Params`, which holds it to the row's
//! constant: a read of a name the constant does not hold, and no exception names, fails a debug
//! assertion naming the row and the name. Test builds assert it (the dev profile, which `cargo
//! test` builds); a release build reads as before. Reserved rows are no one's: no front offers
//! them, and no code reads their parameters.

use crate::core_ops::MORE_WORDS;
use crate::{
    capability_gap, devices, evidence_read, perf_preset, remedy, routine_summary, run_evidence,
};
use serde_json::{Map, Value};

/// The flag of the module's own state root.
pub const STATE_ROOT_FLAG: &str = "--state-root";
/// The flag of the install root the program runs from.
pub const ROOT_FLAG: &str = "--root";

/// A row whose parameters `core_ops` reads in this process, and the names its code reads.
#[derive(Debug)]
pub struct Reads {
    /// The row's name in the operation table.
    pub row: &'static str,
    /// The names its code reads, each once.
    pub names: &'static [&'static str],
}

/// How a parameter is written into the command line its row hands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Written {
    /// A positional word: the value as one word.
    Word,
    /// The flag and the value as one word.
    Flag(&'static str),
    /// The flag alone when the switch is set.
    Switch(&'static str),
    /// The flag and the list's items joined by commas.
    Joined(&'static str),
    /// The flag with each item of the value: `true` the flag alone, `false` nothing, another value
    /// the flag and the value (the row's options as Workflow #434 LAB-5A handed them on).
    Each(&'static str),
    /// The flag and the time the duration reaches back to from now, in unix milliseconds.
    SinceNow(&'static str),
    /// The first flag with a run id, the second with another value.
    RunOrRequest(&'static str, &'static str),
    /// No word: handed beside the words (`remedy set`'s rows inline).
    Beside,
}

/// One part of the command line a row hands on, in its place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// The row's parameter of this name, written as it says.
    Param(&'static str, Written),
    /// [`STATE_ROOT_FLAG`] and the module's state root.
    StateRoot,
    /// [`ROOT_FLAG`] and the install root, when the program runs from one.
    Root,
    /// The members the row does not declare that the command line hands on, each under the flag
    /// of its name ([`EXCEPTIONS`]: `devices bind`'s `root`).
    Further,
}

/// A row whose answer is the command line of a parser of this crate.
#[derive(Debug)]
pub struct Handed {
    /// The row's name in the operation table.
    pub row: &'static str,
    /// The command's words.
    pub command: &'static [&'static str],
    /// The parameter without which the row is served in this process instead (`frame`: `out`).
    pub only_with: Option<&'static str>,
    /// The rest of the command line, in order.
    pub parts: &'static [Part],
}

impl Handed {
    /// The row's parameters the command line carries, in its order, each with how it is written.
    pub fn params(&self) -> impl Iterator<Item = (&'static str, Written)> + '_ {
        self.parts.iter().filter_map(|part| match *part {
            Part::Param(name, written) => Some((name, written)),
            _ => None,
        })
    }
}

/// A name outside the operation table that the code reads or hands on, or that a parser of this
/// crate takes, by design.
#[derive(Debug)]
pub struct Exception {
    /// The rows it applies to; none: every row of this module.
    pub rows: &'static [&'static str],
    /// The member's name (a flag's without `--`, with `_` for `-`).
    pub name: &'static str,
    /// Why.
    pub why: &'static str,
}

/// The exceptions, each with its reason.
pub const EXCEPTIONS: [Exception; 3] = [
    Exception {
        rows: &[],
        name: MORE_WORDS,
        why: "no row declares it: the further words of a row that handed them on before Workflow \
              #434 LAB-5B4; `core_ops::answer` reads it on every row it serves in this process, to \
              refuse it by name",
    },
    Exception {
        rows: &["devices bind"],
        name: "root",
        why: "`devices`' own flag, which the row does not declare and its command line hands on \
              (actinglab's `words::KEPT_FLAGS`); written by `Part::Further` after the module's own \
              `--root`, which `devices` takes only when the module located none",
    },
    Exception {
        rows: &["codes", "events"],
        name: "cursor",
        why: "the parser's `--cursor`, which the rows do not declare: the MCP front keeps these \
              cursors bound to its own session (Workflow #434 LAB-5C) and appends the flag itself",
    },
];

const fn reads(row: &'static str, names: &'static [&'static str]) -> Reads {
    Reads { row, names }
}

/// The rows whose parameters `core_ops` reads in this process.
pub static READ_HERE: [Reads; 30] = [
    reads("facts", &["instance", "program", "config", "prefix"]),
    reads("schedule", &["instance"]),
    reads("run show", &["id"]),
    reads("material", &["ref", "offset", "max_bytes", "out"]),
    reads("targets", &["instance"]),
    reads(
        "targets set",
        &["instance", "file", "targets", "valid_days"],
    ),
    reads("frames", &["instance", "count", "interval"]),
    reads("discover", &[]),
    reads("pause", &["instance", "reason", "drain"]),
    reads("resume", &["instance", "expect"]),
    reads("config", &[]),
    reads("config set", &["changes", "expected_version"]),
    reads("config clear", &["keys", "expected_version"]),
    reads(
        "monitor set",
        &["instance", "interval", "expect", "recover"],
    ),
    reads("monitor clear", &["instance"]),
    reads("offset", &["task", "ms", "instance"]),
    reads("facts publish", &["file"]),
    reads("emu start", &["instance", "wait"]),
    reads("emu stop", &["instance", "wait"]),
    reads("emu restart", &["instance", "wait"]),
    reads("emu selfcheck", &["instance"]),
    reads("shutdown", &["wait"]),
    reads("wake-test", &["channel", "instance"]),
    reads("task stop", &["instance", "request", "wait"]),
    reads("pack-schedule", &["instance"]),
    reads(
        "pack-schedule set",
        &["scope", "package_id", "rows", "expected_version"],
    ),
    reads(
        "run start",
        &["instance", "package", "recovery", "deadline", "wait"],
    ),
    reads("run stop", &["id", "cancel_only"]),
    reads("reset", &["instance"]),
    reads("drill", &["instance", "device"]),
];

const fn word(name: &'static str) -> Part {
    Part::Param(name, Written::Word)
}

const fn flag(name: &'static str, flag: &'static str) -> Part {
    Part::Param(name, Written::Flag(flag))
}

const fn each(name: &'static str, flag: &'static str) -> Part {
    Part::Param(name, Written::Each(flag))
}

/// The instance of a row that names it, as the commands of this crate take it.
const INSTANCE: Part = flag("instance", "--instance");
/// `since`, a duration back from now, as the commands of this crate take a window's start.
const SINCE: Part = Part::Param("since", Written::SinceNow("--since-unix-ms"));

const fn handed(
    row: &'static str,
    command: &'static [&'static str],
    parts: &'static [Part],
) -> Handed {
    Handed {
        row,
        command,
        only_with: None,
        parts,
    }
}

/// The rows whose answer is the command line of a parser of this crate that reads the Runtime of
/// the module's state root (or, `devices`, its binding file).
pub static HANDED: [Handed; 16] = [
    handed(
        "status",
        &["overview"],
        &[
            Part::StateRoot,
            INSTANCE,
            Part::Param("sections", Written::Joined("--sections")),
            Part::Root,
        ],
    ),
    handed(
        "summary",
        &[routine_summary::COMMAND],
        &[
            Part::StateRoot,
            INSTANCE,
            Part::Param("hour", Written::Switch(routine_summary::HOUR)),
            Part::Param("day", Written::Switch(routine_summary::DAY)),
            flag("since", routine_summary::SINCE),
        ],
    ),
    handed(
        "events",
        &["events"],
        &[
            Part::StateRoot,
            INSTANCE,
            SINCE,
            Part::Param("type", Written::Joined("--event-types")),
            flag("run", "--run-id"),
            flag("request", "--request-id"),
            flag("after", "--after-sequence"),
            flag("limit", "--limit"),
            // Workflow #434 LAB-5C: what MCP `ac_events` took, after the words the row gave before.
            flag("view", "--view"),
            flag("since_unix_ms", "--since-unix-ms"),
            flag("until_unix_ms", "--until-unix-ms"),
            flag("through_sequence", "--through-sequence"),
            flag("event_type", "--event-type"),
            flag("fact_key", "--fact-key"),
            flag("min_severity", "--min-severity"),
            flag("max_severity", "--max-severity"),
            flag("origin_module", "--origin-module"),
            flag("task_id", "--task-id"),
            flag("correlation_id", "--correlation-id"),
            flag("detail", "--detail"),
            flag("wait_s", "--wait-s"),
            Part::Param(
                "exclude_event_types",
                Written::Joined("--exclude-event-types"),
            ),
        ],
    ),
    handed(
        "runs",
        &["runs"],
        &[
            Part::StateRoot,
            INSTANCE,
            SINCE,
            each("since_unix_ms", "--since-unix-ms"),
            each("until_unix_ms", "--until-unix-ms"),
            each("task_prefix", "--task-prefix"),
            each("dispatch", "--dispatch"),
            each("state", "--state"),
            each("limit", "--limit"),
            each("cursor", "--cursor"),
        ],
    ),
    handed(
        "run evidence",
        &[run_evidence::COMMAND, run_evidence::ACTION],
        &[
            Part::StateRoot,
            Part::Param("id", Written::RunOrRequest("--run-id", "--request-id")),
            flag("detail", "--detail"),
            each("from_step", "--from-step"),
            each("limit", "--limit"),
        ],
    ),
    handed(
        "diagnose",
        &["diagnose"],
        &[
            Part::StateRoot,
            Part::Root,
            INSTANCE,
            flag("task", "--task-id"),
            SINCE,
            // Workflow #434 LAB-5C: the window's start in unix ms, as MCP `ac_diagnose` took it.
            flag("since_unix_ms", "--since-unix-ms"),
        ],
    ),
    handed(
        "remedy",
        &[remedy::COMMAND, remedy::GET],
        &[Part::StateRoot, INSTANCE],
    ),
    handed(
        "remedy set",
        &[remedy::COMMAND, remedy::SET],
        &[
            Part::StateRoot,
            INSTANCE,
            flag("game", "--game"),
            flag("file", "--rows"),
            // Workflow #434 LAB-5C: the rows given inline go beside the words.
            Part::Param("rows", Written::Beside),
        ],
    ),
    handed(
        "remedy lift",
        &[remedy::COMMAND, remedy::LIFT],
        &[Part::StateRoot, INSTANCE],
    ),
    Handed {
        row: "frame",
        command: &["observe"],
        only_with: Some("out"),
        parts: &[Part::StateRoot, INSTANCE, flag("out", "--export")],
    },
    handed(
        "preset",
        &[perf_preset::COMMAND, perf_preset::GET],
        &[Part::StateRoot],
    ),
    handed(
        "preset set",
        &[perf_preset::COMMAND, perf_preset::SET],
        &[word("name"), Part::StateRoot],
    ),
    handed(
        "evidence read",
        &[evidence_read::COMMAND, evidence_read::CONFIRM_READ],
        &[
            Part::StateRoot,
            flag("artifact", "--artifact"),
            flag("run", "--run"),
        ],
    ),
    handed(
        "devices",
        &[devices::COMMAND, devices::LIST],
        &[Part::StateRoot, Part::Root, Part::Further],
    ),
    handed(
        "devices bind",
        &[devices::COMMAND, devices::BIND],
        &[
            word("instance"),
            word("handle"),
            Part::StateRoot,
            Part::Root,
            Part::Further,
        ],
    ),
    handed(
        "devices unbind",
        &[devices::COMMAND, devices::UNBIND],
        &[word("instance"), Part::StateRoot, Part::Further],
    ),
];

/// The rows whose answer is the command line of a parser of this crate that reads no Runtime and
/// no state root.
pub static OFFLINE: [Handed; 3] = [
    handed(
        "codes",
        &["codes"],
        &[
            word("code"),
            // Workflow #434 LAB-5C: the list's filters and page (the survey's difference 8).
            flag("prefix", "--prefix"),
            flag("owner", "--owner"),
            flag("limit", "--limit"),
        ],
    ),
    handed(
        "gap",
        &[capability_gap::COMMAND],
        &[each("capability", "--capability"), each("why", "--why")],
    ),
    handed(
        "packs",
        &["packs"],
        &[
            INSTANCE,
            Part::Root,
            // Workflow #434 LAB-5C: the rows of one role.
            flag("role", "--role"),
        ],
    ),
];

/// The row's constant: its [`Reads`] or its [`Handed`].
#[derive(Clone, Copy)]
enum List {
    Read(&'static Reads),
    Handed(&'static Handed),
    /// A row of no constant: a reserved row, which no front offers.
    Without,
}

/// The parameters of one call of a row, read by name through the row's constant.
#[derive(Clone, Copy)]
pub(crate) struct Params<'a> {
    map: &'a Map<String, Value>,
    row: &'a str,
    list: List,
}

impl<'a> Params<'a> {
    /// The parameters `map` of the row `row`.
    pub(crate) fn of(row: &'a str, map: &'a Map<String, Value>) -> Self {
        let list = match READ_HERE.iter().find(|reads| reads.row == row) {
            Some(reads) => List::Read(reads),
            None => HANDED
                .iter()
                .chain(&OFFLINE)
                .find(|handed| handed.row == row)
                .map_or(List::Without, List::Handed),
        };
        Self { map, row, list }
    }

    /// The value of the parameter `name`, when given.
    pub(crate) fn get(self, name: &str) -> Option<&'a Value> {
        self.held(name);
        self.map.get(name)
    }

    /// Whether the parameter `name` is given.
    pub(crate) fn contains_key(self, name: &str) -> bool {
        self.held(name);
        self.map.contains_key(name)
    }

    /// Every member as given, for the code that reads no member by name (the refusal of a member
    /// the row does not declare, the further flags).
    pub(crate) fn map(self) -> &'a Map<String, Value> {
        self.map
    }

    /// A read of `name`: one the row's constant does not hold, and no exception names, fails a
    /// debug assertion.
    fn held(self, name: &str) {
        debug_assert!(
            self.holds(name),
            "Workflow #434 LAB-5B1: the code of the row {} reads {name}, which its constant in \
             lab_ops::core_reads does not hold",
            self.row
        );
    }

    fn holds(self, name: &str) -> bool {
        let listed = match self.list {
            List::Read(reads) => reads.names.contains(&name),
            List::Handed(handed) => handed.params().any(|(param, _)| param == name),
            List::Without => false,
        };
        listed
            || EXCEPTIONS.iter().any(|exception| {
                exception.name == name
                    && (exception.rows.is_empty() || exception.rows.contains(&self.row))
            })
    }
}
