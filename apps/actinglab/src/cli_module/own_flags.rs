// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-5B2 (register §1.287, "Slice LAB-5B2"): the kinds of the planned rows of the
//! debugging and authoring modules, and, for each strict row, what its own command accepts, read
//! from the constants its check uses. Workflow #434 LAB-5B3 ("Slice LAB-5B3"): for each row whose
//! command reads its flags by name, what it reads, from its read list. Workflow #434 LAB-5B4
//! ("Slice LAB-5B4"): those commands parse their command line by their read list, so that every
//! own command of the two modules refuses a flag it does not read; none ignores one.
//!
//! - **Strict** ([`STRICT_ROWS`]): actinglab's own command checks the flags of its command line
//!   against lists in code, each list one named constant beside the command that its check uses.
//!   [`accepted`] gives the options, the switches and the positionals the code checks.
//! - **Strict by its read list** ([`READ_LIST_ROWS`], the rows that were lenient until LAB-5B4):
//!   the own command parses its command line by the read list it declares
//!   (`flag_args::FlagArgs::parse_reading`), which refuses a flag or a positional the list does
//!   not hold; [`reads`] gives its options, switches and refused flags and its positionals.
//! - **Table-driven** ([`TABLE_DRIVEN_ROWS`]): what reaches the code is the row's declared words.
//! - **No command** ([`NO_COMMAND_ROWS`]): no command of actinglab serves the row yet.
//!
//! The test `gate_434_lab5b2` holds each strict row equal to what its command accepts, in both
//! directions, and every planned row of the two modules in exactly one kind; `gate_434_lab5b3`
//! holds each row of [`READ_LIST_ROWS`] equal to its command's read list, in both directions. Only
//! the tests and the test-only recording of `flag_args` read this register, so it is built for the
//! tests only.

use crate::commands::{device_commands, navigation_recovery, session_record};
use crate::flag_args::reads::FlagReads;
use crate::resource_runtime_support::resource_declarations;
use crate::{
    drive_cli, lab_package_control, lab2_cli, maa_task_graph, package_build, package_cli,
    package_tool, readonly_cli, recognition_regression, replay_cli, resource_restore,
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

/// The rows whose own command reads its flags by name from the read list it declares (slice B3),
/// which `gate_434_lab5b3` holds equal to its row ([`reads`]). Until slice B4 they were the lenient
/// rows: their command ignored a flag it did not read. Since B4 each parses its command line by
/// its list (`FlagArgs::parse_reading`) and refuses a flag or a positional the list does not hold,
/// so they are strict too, by their list; no row of the two modules is lenient.
pub(crate) const READ_LIST_ROWS: [&str; 24] = [
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

/// How many positional words a command checks it is given: at least `least`, at most `most`
/// (`None`: it checks no most).
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

    /// Workflow #434 LAB-5B4: the positionals `names` (as `FlagReads::positionals` names them),
    /// each of which may be absent: none at least, at most as many as named.
    const fn of(names: &[&str]) -> Self {
        Self {
            least: 0,
            most: Some(names.len()),
        }
    }
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
    /// Workflow #434 LAB-5B4: the option whose value the command also takes as its first
    /// positional word, which the row does not declare (`ensure <page>` and `wait <page>` for
    /// `--page`).
    pub(crate) first_positional: Option<&'static str>,
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
        first_positional: None,
        positionals,
    }
}

/// What the own command of the strict row `row` accepts, from the constants its check uses;
/// `None` for every row that is not strict. For `observe`, `do`, `ensure` and `wait`, whose flags
/// depend on the path the command line takes, the union of their paths' lists. Workflow #434
/// LAB-5B4: the switches of the commands that have them are the code's constants
/// (`lab2_cli::LAB2_SWITCHES`, `session_record::RECORD_MARK_SWITCHES` and `RECORD_STOP_SWITCHES`,
/// `package_tool::FMT_SWITCHES`, `package_build::LOADING_SWITCHES`), which their parse reads
/// (`FlagArgs::parse_switches`); and the commands that checked no count of positionals check the
/// ones they read (`observe`, `do`, `ensure`, `wait`, `package preflight`, `resource catalog`,
/// `resource validate`).
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
        // Workflow #434 LAB-5B4: each verb checks the positionals it reads (`*_POSITIONALS`).
        "observe" | "do" | "ensure" | "wait" => {
            let positionals: &[&str] = match row {
                "observe" => &lab2_cli::OBSERVE_POSITIONALS,
                "do" => &lab2_cli::DO_POSITIONALS,
                "ensure" => &lab2_cli::ENSURE_POSITIONALS,
                _ => &lab2_cli::WAIT_POSITIONALS,
            };
            let mut verb = from_lists(
                lab2_cli::verb_reads(row)?,
                &lab2_cli::LAB2_SWITCHES,
                Positionals::of(positionals),
            );
            match row {
                "do" => verb.for_positionals = &["--target"],
                "ensure" | "wait" => verb.first_positional = Some("--page"),
                _ => {}
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
            &session_record::RECORD_MARK_SWITCHES,
            none,
        ),
        "record stop" => from_lists(
            session_record::RECORD_STOP_FLAGS.iter().copied(),
            &session_record::RECORD_STOP_SWITCHES,
            none,
        ),
        "package check" => from_lists(package_cli::CHECK_FLAGS, &[], none),
        // Workflow #434 LAB-5B4: these three check the positionals they read too: none.
        "package preflight" => from_lists(
            package_cli::PREFLIGHT_FLAGS,
            &[],
            Positionals::of(&package_cli::PREFLIGHT_POSITIONALS),
        ),
        "package loading" => from_lists(
            package_build::LOADING_FLAGS,
            &package_build::LOADING_SWITCHES,
            none,
        ),
        "package fmt" => from_lists(
            package_tool::FMT_FLAGS.iter().copied(),
            &package_tool::FMT_SWITCHES,
            none,
        ),
        "package edit" => from_lists(package_tool::EDIT_FLAGS.iter().copied(), &[], none),
        "resource catalog" => from_lists(
            resource_runtime_support::CATALOG_FLAGS,
            &[],
            Positionals::of(&resource_runtime_support::CATALOG_POSITIONALS),
        ),
        "resource validate" => from_lists(
            resource_declarations::VALIDATE_FLAGS,
            &[],
            Positionals::of(&resource_declarations::VALIDATE_POSITIONALS),
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

/// Workflow #434 LAB-5B3: what the own command of a row of [`READ_LIST_ROWS`] reads, from its
/// read list.
#[derive(Debug)]
pub(crate) struct Reads {
    /// The command's read list, which its command line is parsed by (since LAB-5B4) and its test
    /// builds hold its reads to.
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
    /// The positional words the command checks it is given: since LAB-5B4 those its list names
    /// (`FlagReads::positionals`), each of which may be absent.
    pub(crate) positionals: Positionals,
}

/// What the own command of the row `row` of [`READ_LIST_ROWS`] reads, from its read list; `None`
/// for every other row. Beside the list: the option the first positional stands in for.
pub(crate) fn reads(row: &str) -> Option<Reads> {
    let (list, first_positional): (&'static FlagReads, _) = match row {
        "touch-probe" => (&device_commands::TOUCH_PROBE_READS, None),
        "recover" => (&navigation_recovery::RECOVER_READS, None),
        "tap-target" => (&drive_cli::TAP_TARGET_READS, Some("--target")),
        "navigate" => (&drive_cli::NAVIGATE_READS, None),
        "debug-package" => (&runtime_debug::DEBUG_PACKAGE_READS, None),
        "export-evidence" => (&runtime_debug::EXPORT_EVIDENCE_READS, None),
        "recognize-artifact" => (&readonly_cli::RECOGNIZE_ARTIFACT_READS, None),
        "record start" => (&session_record::RECORD_START_READS, None),
        "record status" => (&session_record::RECORD_STATUS_READS, None),
        "record candidates" => (&session_record::RECORD_CANDIDATES_READS, Some("--step-id")),
        "record amend" => (&session_record::RECORD_AMEND_READS, Some("--step-id")),
        "record step" => (&session_record::RECORD_STEP_READS, None),
        "package validate" => (&package_cli::VALIDATE_READS, None),
        "package inspect" => (&lab_package_control::INSPECT_READS, None),
        "package bundle" => (&package_build::BUNDLE_READS, None),
        "resource compile-maa" => (&maa_task_graph::COMPILE_MAA_READS, None),
        "operation validate" => (&lab_package_control::OPERATION_VALIDATE_READS, None),
        "operation inspect" => (&lab_package_control::OPERATION_INSPECT_READS, None),
        "operation explain" => (&lab_package_control::OPERATION_EXPLAIN_READS, None),
        "recognize" => (&readonly_cli::RECOGNIZE_READS, None),
        "detect-page" => (&readonly_cli::DETECT_PAGE_READS, None),
        "current-page" => (&readonly_cli::CURRENT_PAGE_READS, None),
        "is-visible" => (&readonly_cli::IS_VISIBLE_READS, Some("--target")),
        "locate" => (&navigation_recovery::LOCATE_READS, Some("--template")),
        _ => return None,
    };
    Some(Reads {
        list,
        options: list.options(),
        switches: list.switches(),
        refused: list.refused(),
        first_positional,
        positionals: Positionals::of(list.positionals),
    })
}

/// Workflow #434 LAB-5B4: the read lists of actinglab's own commands that are no row of the
/// operation table and read their flags by name: `run list`, `capture` (`session capture` too),
/// `lab receipt` and `lab evidence`, each parsed by its list as the rows of [`READ_LIST_ROWS`].
pub(crate) const UNROWED_READS: [&FlagReads; 4] = [
    &crate::run_summary::RUN_LIST_READS,
    &device_commands::CAPTURE_READS,
    &lab2_cli::RECEIPT_READS,
    &lab2_cli::EVIDENCE_READS,
];
