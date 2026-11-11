// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-5B2 (register §1.287, "Slice LAB-5B2"): the four kinds of the planned rows of
//! the debugging and authoring modules, and, for each strict row, what its own command accepts,
//! read from the constants its check uses.
//!
//! - **Strict** ([`STRICT_ROWS`]): actinglab's own command checks the flags of its command line
//!   against lists in code, each list one named constant beside the command that its check uses.
//!   [`accepted`] gives the options, the switches and the positionals the code checks.
//! - **Lenient** ([`LENIENT_ROWS`]): the own command ignores a flag it does not read.
//! - **Table-driven** ([`TABLE_DRIVEN_ROWS`]): what reaches the code is the row's declared words.
//! - **No command** ([`NO_COMMAND_ROWS`]): no command of actinglab serves the row yet.
//!
//! The test `gate_434_lab5b2` holds each strict row equal to what its command accepts, in both
//! directions, and every planned row of the two modules in exactly one kind. Nothing else reads
//! this register, so it is built for the tests only.

use crate::commands::{device_commands, session_record};
use crate::resource_runtime_support::resource_declarations;
use crate::{
    package_build, package_cli, package_tool, recognition_regression, replay_cli, resource_restore,
    resource_runtime_support, resource_verify, runtime_debug, scheduling_cli, signature_cli,
    standard_pack_cli,
};

/// The strict rows: [`accepted`] answers for each of them and for no other row.
pub(crate) const STRICT_ROWS: [&str; 31] = [
    "tap",
    "swipe",
    "long-tap",
    "key",
    "text",
    "do",
    "ensure",
    "wait",
    "watch",
    "signatures register",
    "signatures match",
    "signatures retire",
    "unpin",
    "observe",
    "record mark",
    "record stop",
    "package check",
    "package preflight",
    "package loading",
    "package fmt",
    "package edit",
    "resource catalog",
    "resource validate",
    "resource verify",
    "resource restore",
    "standard-pack check",
    "standard-pack compose",
    "scheduling compile",
    "scheduling timeline",
    "regression",
    "replay",
];

/// The lenient rows: actinglab's own command reads its flags by name and ignores a flag it does
/// not read (`FlagArgs` takes any flag), so no list in code says what it accepts. Slice B3 gives
/// each of these commands a read list, which a test then holds equal to its row.
pub(crate) const LENIENT_ROWS: [&str; 24] = [
    "touch-probe",
    "recover",
    "tap-target",
    "navigate",
    "debug-package",
    "export-evidence",
    "recognize-artifact",
    "record start",
    "record status",
    "record candidates",
    "record amend",
    "record step",
    "package validate",
    "package inspect",
    "package bundle",
    "resource compile-maa",
    "operation validate",
    "operation inspect",
    "operation explain",
    "recognize",
    "detect-page",
    "current-page",
    "is-visible",
    "locate",
];

/// The table-driven rows: the row's declared words are what reaches the code. The CLI module
/// serves `run start`, `run stop`, `reset` and `drill` itself (`core_ops`); `user-config` and
/// `user-config set` reach actinglab's own `user-config get|set` as the row's declared positionals
/// only (`engine::Line::Declared`). Slice B1 ties the names their code reads to their rows.
pub(crate) const TABLE_DRIVEN_ROWS: [&str; 6] = [
    "run start",
    "run stop",
    "reset",
    "drill",
    "user-config",
    "user-config set",
];

/// The rows no command of actinglab serves yet: `record draft`, MCP's `ac_binding_draft` until
/// LAB-5 builds its command.
pub(crate) const NO_COMMAND_ROWS: [&str; 1] = ["record draft"];

/// The flags of `observe`, `do`, `ensure` and `wait` that take no value (read with
/// `FlagArgs::bool`); a verb's paths read some of them.
const LAB2_SWITCHES: [&str; 8] = [
    "--capture",
    "--dry-run",
    "--allow-destructive",
    "--destructive",
    "--record",
    "--preview",
    "--pretty",
    "--verbose",
];

/// The flags of `record mark` that take no value (read with `record_flag_switch`).
const RECORD_MARK_SWITCHES: [&str; 7] = [
    "--dry-run",
    "--replace-click",
    "--replace-transition",
    "--close-step",
    "--optional",
    "--not-optional",
    "--no-anchor",
];

/// How many positional words a command checks it is given: at least `least`, at most `most`
/// (`None`: it checks no most). A command that checks no count is `{0, None}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Positionals {
    pub(crate) least: usize,
    pub(crate) most: Option<usize>,
}

impl Positionals {
    /// Exactly `count` positional words.
    const fn exactly(count: usize) -> Self {
        Self {
            least: count,
            most: Some(count),
        }
    }

    /// No count checked.
    const UNCHECKED: Self = Self {
        least: 0,
        most: None,
    };
}

/// What the own command of a strict row accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Accepted {
    /// The flags that take a value, as the command line writes them (`--name-with-dashes`).
    pub(crate) options: Vec<&'static str>,
    /// The flags that take none.
    pub(crate) switches: Vec<&'static str>,
    /// The flags the command also reads in place of a positional the row declares (`do --target`
    /// for `do <target>`).
    pub(crate) for_positionals: &'static [&'static str],
    /// The positional words the command checks.
    pub(crate) positionals: Positionals,
}

/// What a command accepts whose flags are `flags` (the union of the constants its check uses,
/// each flag once), of which those in `without_value` take no value.
fn from_lists(
    flags: impl IntoIterator<Item = &'static str>,
    without_value: &[&str],
    positionals: Positionals,
) -> Accepted {
    let mut options = Vec::new();
    let mut switches = Vec::new();
    for flag in flags {
        let list = if without_value.contains(&flag) {
            &mut switches
        } else {
            &mut options
        };
        if !list.contains(&flag) {
            list.push(flag);
        }
    }
    Accepted {
        options,
        switches,
        for_positionals: &[],
        positionals,
    }
}

/// What the own command of the strict row `row` accepts, from the constants its check uses;
/// `None` for every row that is not strict. For `observe`, `do`, `ensure` and `wait`, whose flags
/// depend on the path the command line takes, the union of their paths' lists.
pub(crate) fn accepted(row: &str) -> Option<Accepted> {
    let none = Positionals::exactly(0);
    Some(match row {
        "tap" => from_lists(
            device_commands::DIRECT_TOUCH_FLAGS,
            &[],
            Positionals::exactly(2),
        ),
        "swipe" => from_lists(
            device_commands::DIRECT_TOUCH_FLAGS,
            &[],
            Positionals::exactly(5),
        ),
        "long-tap" => from_lists(
            device_commands::DIRECT_TOUCH_FLAGS,
            &[],
            Positionals::exactly(3),
        ),
        "key" => from_lists(
            device_commands::DIRECT_INPUT_FLAGS,
            &[],
            Positionals::exactly(1),
        ),
        "text" => from_lists(
            device_commands::DIRECT_INPUT_FLAGS,
            &[],
            Positionals {
                least: 1,
                most: None,
            },
        ),
        "observe" | "do" | "ensure" | "wait" => {
            let mut verb = from_lists(
                crate::lab2_cli::verb_reads(row)?,
                &LAB2_SWITCHES,
                Positionals::UNCHECKED,
            );
            if row == "do" {
                verb.for_positionals = &["--target"];
            }
            verb
        }
        "watch" => from_lists(
            runtime_debug::WATCH_QUERY_FLAGS
                .iter()
                .map(|(flag, _)| *flag)
                .chain(runtime_debug::WATCH_CURSOR_FLAGS),
            &[],
            none,
        ),
        "signatures register" => from_lists(signature_cli::options("register"), &[], none),
        "signatures match" => from_lists(signature_cli::options("match"), &[], none),
        "signatures retire" => from_lists(signature_cli::options("retire"), &[], none),
        "unpin" => from_lists(runtime_debug::UNPIN_FLAGS, &[], none),
        "record mark" => from_lists(
            session_record::RECORD_MARK_SHORTCUT_FLAGS
                .iter()
                .copied()
                .chain(session_record::RECORD_MARK_REQUEST_FLAGS),
            &RECORD_MARK_SWITCHES,
            none,
        ),
        "record stop" => from_lists(
            session_record::RECORD_STOP_FLAGS.iter().copied(),
            &["--dry-run"],
            none,
        ),
        "package check" => from_lists(package_cli::CHECK_FLAGS, &[], none),
        "package preflight" => {
            from_lists(package_cli::PREFLIGHT_FLAGS, &[], Positionals::UNCHECKED)
        }
        "package loading" => from_lists(
            package_build::LOADING_FLAGS,
            &package_build::LOADING_SWITCHES,
            none,
        ),
        "package fmt" => from_lists(package_tool::FMT_FLAGS.iter().copied(), &["--check"], none),
        "package edit" => from_lists(package_tool::EDIT_FLAGS.iter().copied(), &[], none),
        "resource catalog" => from_lists(
            resource_runtime_support::CATALOG_FLAGS,
            &[],
            Positionals::UNCHECKED,
        ),
        "resource validate" => from_lists(
            resource_declarations::VALIDATE_FLAGS,
            &[],
            Positionals::UNCHECKED,
        ),
        "resource verify" => from_lists(resource_verify::FLAGS, &[], none),
        "resource restore" => from_lists(resource_restore::RESTORE_FLAGS, &[], none),
        "standard-pack check" => from_lists(standard_pack_cli::CHECK_FLAGS, &[], none),
        "standard-pack compose" => from_lists(standard_pack_cli::COMPOSE_FLAGS, &[], none),
        "scheduling compile" => from_lists(scheduling_cli::COMPILE_FLAGS, &[], none),
        "scheduling timeline" => from_lists(scheduling_cli::TIMELINE_FLAGS, &[], none),
        "regression" => from_lists(recognition_regression::REGRESSION_FLAGS, &[], none),
        "replay" => from_lists(replay_cli::REPLAY_FLAGS, &[], none),
        _ => return None,
    })
}
