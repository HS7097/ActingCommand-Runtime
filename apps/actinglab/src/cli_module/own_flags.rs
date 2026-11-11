// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-5B2 (register §1.287, "Slice LAB-5B2"): the four kinds of the planned rows of
//! the debugging and authoring modules, and, for each strict row, what its own command accepts,
//! read from the constants its check uses. Workflow #434 LAB-5B3 ("Slice LAB-5B3"): for each
//! lenient row, what its own command reads, from its read list.
//!
//! - **Strict** ([`STRICT_ROWS`]): actinglab's own command checks the flags of its command line
//!   against lists in code, each list one named constant beside the command that its check uses.
//!   [`accepted`] gives the options, the switches and the positionals the code checks.
//! - **Lenient** ([`LENIENT_ROWS`]): the own command ignores a flag it does not read. Each declares
//!   a read list where it starts (`flag_args::reads`, tests only); [`reads`] gives its options,
//!   switches and refused flags and the positionals the code checks or reads.
//! - **Table-driven** ([`TABLE_DRIVEN_ROWS`]): what reaches the code is the row's declared words.
//! - **No command** ([`NO_COMMAND_ROWS`]): no command of actinglab serves the row yet.
//!
//! Beside the four kinds (Workflow #434 LAB-5F fix 1), [`CORE_STRICT_ROWS`]: rows of the core
//! module that actinglab's own command serves, checked against lists in code as the strict rows
//! are (`standard-pack check`).
//!
//! The test `gate_434_lab5b2` holds each strict row and each row of [`CORE_STRICT_ROWS`] equal to
//! what its command accepts, in both directions, and every planned row of the two modules in
//! exactly one kind; `gate_434_lab5b3` holds each lenient row equal to its command's read list, in
//! both directions. Only the tests and the test-only recording of `flag_args` read this register,
//! so it is built for the tests only.

use crate::commands::{device_commands, navigation_recovery, session_record};
use crate::flag_args::reads::FlagReads;
use crate::resource_runtime_support::resource_declarations;
use crate::{
    drive_cli, lab_package_control, maa_task_graph, package_build, package_cli, package_tool,
    readonly_cli, recognition_regression, replay_cli, resource_restore, resource_runtime_support,
    resource_verify, runtime_debug, scheduling_cli, signature_cli, standard_pack_cli,
};

/// The strict rows: [`accepted`] answers for each of them and, beside them, for the rows of
/// [`CORE_STRICT_ROWS`] only.
pub(crate) const STRICT_ROWS: [&str; 30] = [
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
    "standard-pack compose",
    "scheduling compile",
    "scheduling timeline",
    "regression",
    "replay",
];

/// The lenient rows: actinglab's own command reads its flags by name and ignores a flag it does
/// not read (`FlagArgs` takes any flag), so no list in code says what it accepts. Since slice B3
/// each of these commands declares a read list where it starts, which its test builds hold its
/// reads to and `gate_434_lab5b3` holds equal to its row ([`reads`]).
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

/// Workflow #434 LAB-5F fix 1: the rows of the core module that actinglab's own command serves
/// and that are held equal to it as the strict rows are, [`accepted`] answering for them:
/// `standard-pack check`, a core row since the coordinator's ruling of 2026-11-11 (the rescue
/// module's `place` runs it as its post-check). Not one of the four kinds, which are the
/// debugging and authoring modules'. `app launch|restart|stop`, core rows actinglab's own
/// `session app` serves, are slice B1's.
pub(crate) const CORE_STRICT_ROWS: [&str; 1] = ["standard-pack check"];

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

/// What the own command of the strict row `row` (or of a row of [`CORE_STRICT_ROWS`]) accepts,
/// from the constants its check uses; `None` for every other row. For `observe`, `do`, `ensure`
/// and `wait`, whose flags depend on the path the command line takes, the union of their paths'
/// lists.
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

/// Workflow #434 LAB-5B3: what the own command of a lenient row reads, from its read list.
#[derive(Debug)]
pub(crate) struct Reads {
    /// The command's read list, which its test builds hold its reads to.
    pub(crate) list: &'static FlagReads,
    /// The flags it reads for their value, as the command line writes them.
    pub(crate) options: Vec<&'static str>,
    /// The flags it reads as switches.
    pub(crate) switches: Vec<&'static str>,
    /// The flags it reads only to refuse them.
    pub(crate) refused: Vec<&'static str>,
    /// The option whose value the command also takes as its first positional word, when the
    /// option is not given (`tap-target <target>` for `--target`).
    pub(crate) first_positional: Option<&'static str>,
    /// The positional words the command checks it is given.
    pub(crate) positionals: Positionals,
}

/// What the own command of the lenient row `row` reads, from its read list; `None` for every row
/// that is not lenient. Beside the list, as the code checks or reads them: the count of positional
/// words (exactly none for `touch-probe`, `debug-package` and `export-evidence`, no count for the
/// others; `recover --stale-capture` checks exactly none on its own path only) and the option the
/// first positional stands in for.
pub(crate) fn reads(row: &str) -> Option<Reads> {
    let none = Positionals::exactly(0);
    let unchecked = Positionals::UNCHECKED;
    let (list, first_positional, positionals): (&'static FlagReads, _, _) = match row {
        "touch-probe" => (&device_commands::TOUCH_PROBE_READS, None, none),
        "recover" => (&navigation_recovery::RECOVER_READS, None, unchecked),
        "tap-target" => (&drive_cli::TAP_TARGET_READS, Some("--target"), unchecked),
        "navigate" => (&drive_cli::NAVIGATE_READS, None, unchecked),
        "debug-package" => (&runtime_debug::DEBUG_PACKAGE_READS, None, none),
        "export-evidence" => (&runtime_debug::EXPORT_EVIDENCE_READS, None, none),
        "recognize-artifact" => (&readonly_cli::RECOGNIZE_ARTIFACT_READS, None, unchecked),
        "record start" => (&session_record::RECORD_START_READS, None, unchecked),
        "record status" => (&session_record::RECORD_STATUS_READS, None, unchecked),
        "record candidates" => (
            &session_record::RECORD_CANDIDATES_READS,
            Some("--step-id"),
            unchecked,
        ),
        "record amend" => (
            &session_record::RECORD_AMEND_READS,
            Some("--step-id"),
            unchecked,
        ),
        "record step" => (&session_record::RECORD_STEP_READS, None, unchecked),
        "package validate" => (&package_cli::VALIDATE_READS, None, unchecked),
        "package inspect" => (&lab_package_control::INSPECT_READS, None, unchecked),
        "package bundle" => (&package_build::BUNDLE_READS, None, unchecked),
        "resource compile-maa" => (&maa_task_graph::COMPILE_MAA_READS, None, unchecked),
        "operation validate" => (
            &lab_package_control::OPERATION_VALIDATE_READS,
            None,
            unchecked,
        ),
        "operation inspect" => (
            &lab_package_control::OPERATION_INSPECT_READS,
            None,
            unchecked,
        ),
        "operation explain" => (
            &lab_package_control::OPERATION_EXPLAIN_READS,
            None,
            unchecked,
        ),
        "recognize" => (&readonly_cli::RECOGNIZE_READS, None, unchecked),
        "detect-page" => (&readonly_cli::DETECT_PAGE_READS, None, unchecked),
        "current-page" => (&readonly_cli::CURRENT_PAGE_READS, None, unchecked),
        "is-visible" => (&readonly_cli::IS_VISIBLE_READS, Some("--target"), unchecked),
        "locate" => (
            &navigation_recovery::LOCATE_READS,
            Some("--template"),
            unchecked,
        ),
        _ => return None,
    };
    Some(Reads {
        list,
        options: list.options(),
        switches: list.switches(),
        refused: list.refused(),
        first_positional,
        positionals,
    })
}
