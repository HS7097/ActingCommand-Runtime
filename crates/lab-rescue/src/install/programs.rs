// SPDX-License-Identifier: AGPL-3.0-only

//! The programs of a build that `actingctl install` runs (Workflow #421 INST-1;
//! `contracts/install-cli.md`, "Update and rollback", "The rollback gate"): actingd's
//! `check-config` (the live build in place, a kept build with `--install-root <root>` when it
//! takes the option), its `ledger-maintenance verify`, and actingledger's bounded head read (S0)
//! and `events` pages (whether a build's own reader reads the ledger after S0). Workflow #453
//! S10: and the live build's `actingd discover`, which the rescue row `emulators` runs. Workflow
//! #453 S7: and the live build's `actingd vision manifest|check` and the Tools' `standard-pack
//! check`, which the rows `vision manifest` and `vision check` run, each ended at its bound.
//!
//! INST-1b-F fix round 1 (review S1 F4): every child runs within its deadline, a key of the
//! `install` section (`install.child.*`). A child still running then is left running, never
//! killed (the stall rule), and its failure names its `pid` and the key. Workflow #453 S10 fix 1
//! (review): the one exception is `actingd discover`, read only, which runs within
//! [`DISCOVER_TIMEOUT_MS`] and is ended there ([`Host::run_program_ending`]). Workflow #453 S7:
//! so are the vision rows' read-only children, within [`VISION_MANIFEST_TIMEOUT_MS`],
//! [`VISION_CHECK_TIMEOUT_MS`] and [`STANDARD_PACK_CHECK_TIMEOUT_MS`].

use super::files;
use super::host::{Host, NotFinished, NotRan, Ran};
use crate::codes::ActingctlCode;
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{CauseRelation, Outcome, OutcomeEnvelope, RawSource};
use actingcommand_runtime_config::install::{self as keys, InstallSettings};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::ffi::OsString;
use std::path::Path;

/// actingd's program in a build's `runtime\`.
pub(crate) const DAEMON: &str = "actingcommand-actingd.exe";
/// actingledger's program in a build's `tools\`.
pub(crate) const LEDGER_READER: &str = "actingledger.exe";
/// The ledger page the gate reads with (fixed by the contract).
pub(crate) const EVENTS_PAGE: u64 = 1024;
const CHECK_CONFIG_SCHEMA: &str = "actingcommand.actingd.check-config.v2";
/// The `actingd-config` revision whose check-config takes `--install-root` (Workflow #421
/// INST-1, Runtime v0.12.18; `contracts/component-interfaces.md`): a build whose range ends below
/// it refuses the option (review S1 F1).
pub(crate) const INSTALL_ROOT_REVISION: u64 = 17;

/// A child's deadline: the key of the `install` section that sets it, and its milliseconds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Deadline {
    key: &'static str,
    ms: u64,
}

impl Deadline {
    /// Each `actingd check-config` (`install.child.check_config_timeout_ms`).
    pub(crate) fn check_config(settings: &InstallSettings) -> Self {
        Self {
            key: keys::CHILD_CHECK_CONFIG_TIMEOUT_MS,
            ms: settings.child_check_config_timeout_ms,
        }
    }

    /// Each `actingledger` read (`install.child.ledger_read_timeout_ms`).
    pub(crate) fn ledger_read(settings: &InstallSettings) -> Self {
        Self {
            key: keys::CHILD_LEDGER_READ_TIMEOUT_MS,
            ms: settings.child_ledger_read_timeout_ms,
        }
    }

    /// The `actingd ledger-maintenance verify` (`install.child.ledger_verify_timeout_ms`).
    pub(crate) fn ledger_verify(settings: &InstallSettings) -> Self {
        Self {
            key: keys::CHILD_LEDGER_VERIFY_TIMEOUT_MS,
            ms: settings.child_ledger_verify_timeout_ms,
        }
    }
}

/// Why a child gave no usable answer.
#[derive(Debug, Clone)]
pub(crate) enum Failure {
    /// It failed, printed no usable answer, or did not run: the outcome that says why.
    Failed(Outcome),
    /// It was still running when its deadline elapsed and was left running (never killed):
    /// its process id and the key of the deadline.
    TimedOut { pid: u32, key: &'static str },
}

impl Failure {
    /// `refusal` with this failure: its outcome as the `caused_by` link (by the fatal-link
    /// rule), or the child left running as `pid` and the key of the deadline as `config_key`.
    pub(crate) fn onto(self, refusal: Outcome) -> Outcome {
        match self {
            Self::Failed(cause) => refusal.joined(CauseRelation::CausedBy, cause),
            Self::TimedOut { pid, key } => refusal.pid(pid).config_key(key),
        }
    }

    /// Whether the child was left running at its deadline.
    pub(crate) fn timed_out(&self) -> bool {
        matches!(self, Self::TimedOut { .. })
    }
}

/// Runs `program` with `arguments` within `deadline` (Workflow #434 LAB-3: also the rescue
/// module's children, `standard-pack check` and actingd's `unlock-owner`).
pub(crate) fn run(
    host: &dyn Host,
    program: &Path,
    arguments: &[&str],
    deadline: Deadline,
) -> Result<Ran, Failure> {
    let arguments = arguments.iter().map(OsString::from).collect::<Vec<_>>();
    host.run_program(program, &arguments, deadline.ms)
        .map_err(|not_ran| match not_ran {
            NotRan::Failed(cause) => Failure::Failed(cause),
            NotRan::StillRunning { pid } => Failure::TimedOut {
                pid,
                key: deadline.key,
            },
        })
}

/// A child that exited non-zero: `foreign_other_error` (`raw_source` process) with its exit
/// code and what it wrote on stderr.
pub(crate) fn exited(ran: &Ran) -> Outcome {
    let failure = Outcome::new(ContractCode::ForeignOtherError).raw_source(RawSource::Process);
    let failure = match ran.exit_code {
        Some(exit_code) => failure.exit_code(exit_code),
        None => failure,
    };
    failure.raw_text(&ran.stderr)
}

/// What a check-config answered, in the parts a plan compares (`contracts/install-cli.md`,
/// "Update and rollback": the outcome, the catalog plan, the pending approvals, the note codes,
/// the standard packs registered).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct CheckSummary {
    /// The top outcome's spelling.
    pub(crate) top: String,
    pub(crate) category: String,
    pub(crate) plan: Value,
    pub(crate) approvals: Value,
    pub(crate) notes: Vec<String>,
    pub(crate) packs: Vec<Value>,
}

impl CheckSummary {
    /// The members that differ from `other`'s.
    pub(crate) fn differs(&self, other: &Self) -> Vec<&'static str> {
        let mut differs = Vec::new();
        if self.top != other.top || self.category != other.category {
            differs.push("outcome");
        }
        if self.plan != other.plan {
            differs.push("plan");
        }
        if self.approvals != other.approvals {
            differs.push("approvals");
        }
        if self.notes != other.notes {
            differs.push("notes");
        }
        if self.packs != other.packs {
            differs.push("packs");
        }
        differs
    }
}

/// A check-config that answered.
#[derive(Debug, Clone)]
pub(crate) struct Checked {
    pub(crate) summary: CheckSummary,
    /// Its outcome, as received.
    pub(crate) received: Outcome,
    /// Whether the run that answered was given `--install-root` (review S1 F1).
    pub(crate) install_root: bool,
    /// Workflow #453 S6 fix 2: its `report` as printed beside the outcome (`null` without one).
    /// No envelope bound applies to it, so its members are whole where the outcome's notes may be
    /// cut (`causes_total`).
    pub(crate) report: Value,
}

impl Checked {
    /// Whether the configuration passed: the outcome is no error.
    pub(crate) fn passed(&self) -> bool {
        self.received.category() <= actingcommand_contract::outcome::Category::Warning
    }
}

/// `<runtime>\actingcommand-actingd.exe check-config --config <config>`, with
/// `--install-root <root>` when `install_root` names one. A build that refuses its arguments
/// then (an error before any check: no report and no value, the form of
/// `check_config_usage_invalid` and `check_config_option_invalid` in every build before
/// `actingd-config` revision 17) is asked again without the option, and
/// [`Checked::install_root`] says which run answered (review S1 F1). `check_config_failed`
/// (`path` the program) when it does not run or prints no report; a check-config still running
/// at `deadline` is [`Failure::TimedOut`].
pub(crate) fn check_config(
    host: &dyn Host,
    runtime: &Path,
    config: &Path,
    install_root: Option<&Path>,
    deadline: Deadline,
) -> Result<Checked, Failure> {
    let program = runtime.join(DAEMON);
    if let Some(root) = install_root {
        let (checked, refused_arguments) =
            check_config_once(host, &program, config, Some(root), deadline)?;
        if !refused_arguments {
            return Ok(checked);
        }
    }
    check_config_once(host, &program, config, None, deadline).map(|(checked, _)| checked)
}

/// One run of check-config: its answer, and whether it refused its arguments.
fn check_config_once(
    host: &dyn Host,
    program: &Path,
    config: &Path,
    install_root: Option<&Path>,
    deadline: Deadline,
) -> Result<(Checked, bool), Failure> {
    let config_word = config.to_string_lossy().into_owned();
    let root_word = install_root.map(|root| root.to_string_lossy().into_owned());
    let mut arguments = vec!["check-config", "--config", config_word.as_str()];
    if let Some(root) = &root_word {
        arguments.extend(["--install-root", root.as_str()]);
    }
    let failed = |ran: Option<&Ran>| {
        let outcome = Outcome::new(ActingctlCode::CheckConfigFailed).path(program);
        Failure::Failed(match ran {
            Some(ran) => {
                let outcome = match ran.exit_code {
                    Some(exit_code) => outcome.exit_code(exit_code),
                    None => outcome,
                };
                outcome.caused_by(exited(ran))
            }
            None => outcome,
        })
    };
    let ran = match run(host, program, &arguments, deadline) {
        Ok(ran) => ran,
        Err(Failure::Failed(cause)) => {
            return Err(Failure::Failed(
                Outcome::new(ActingctlCode::CheckConfigFailed)
                    .path(program)
                    .caused_by(cause),
            ));
        }
        Err(timed_out) => return Err(timed_out),
    };
    let report: Value =
        serde_json::from_slice(files::without_bom(&ran.stdout)).map_err(|_| failed(Some(&ran)))?;
    if report["schema_version"] != CHECK_CONFIG_SCHEMA {
        return Err(failed(Some(&ran)));
    }
    let received = serde_json::from_value::<OutcomeEnvelope>(report["outcome"].clone())
        .map_err(|_| failed(Some(&ran)))?
        .into_outcome();
    let notes = report["outcome"]["causes"]
        .as_array()
        .map(|causes| {
            causes
                .iter()
                .filter(|cause| cause["relation"] == "note")
                .filter_map(|cause| cause["code"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let packs = report["report"]["standard_packs"]["games"]
        .as_array()
        .map(|games| {
            games
                .iter()
                .map(|game| {
                    serde_json::json!({"game": game["game"], "pack_count": game["pack_count"]})
                })
                .collect()
        })
        .unwrap_or_default();
    let checked = Checked {
        summary: CheckSummary {
            top: received.code().as_str().to_owned(),
            category: received.category().as_str().to_owned(),
            plan: report["report"]["policy_plan"]["plan"].clone(),
            approvals: report["report"]["policy_plan"]["approvals"].clone(),
            notes,
            packs,
        },
        received,
        install_root: install_root.is_some(),
        report: report["report"].clone(),
    };
    let refused_arguments = !checked.passed()
        && report.get("report").is_none_or(Value::is_null)
        && checked.received.values().is_empty();
    Ok((checked, refused_arguments))
}

/// Workflow #453 S10: the schema of `actingd discover`'s report.
const DISCOVER_SCHEMA: &str = "actingcommand.actingd.discover.v1";

/// What `actingd discover` answered with its report: its outcome as received, its `report`
/// (`null` when it carries none, as a refusal does) and its exit code.
#[derive(Debug, Clone)]
pub(crate) struct Discovered {
    pub(crate) received: Outcome,
    pub(crate) report: Value,
    pub(crate) exit_code: Option<i32>,
}

impl Discovered {
    /// Whether the discovery completed: exit 0 and a success outcome.
    pub(crate) fn completed(&self) -> bool {
        self.exit_code == Some(0)
            && self.received.category() == actingcommand_contract::outcome::Category::Success
    }
}

/// Workflow #453 S10 fix 1 (review): the bound of `actingd discover` in the rescue row
/// `emulators`, at which the child is ended (it is read only: ending it loses nothing). The
/// child's own reads are each bounded at 10 s: the process listing, made only when no folder is
/// named (`MUMU_MANAGER_PROCESS_LIST_TIMEOUT`), then the vendor tool's `version` and `info -v all`
/// (`MUMU_MANAGER_COMMAND_TIMEOUT` each), 30 s together; as much again is the margin for the
/// process's start, the configuration's read and the ending of a read at its bound. A bound of
/// the child's own bounds, not a tunable.
pub(crate) const DISCOVER_TIMEOUT_MS: u64 = 60_000;

/// Workflow #453 S10 fix 1: why `actingd discover` gave no report.
#[derive(Debug, Clone)]
pub(crate) enum Undiscovered {
    /// It failed, printed no report, or did not run: the outcome that says why.
    Failed(Outcome),
    /// It was still running at [`DISCOVER_TIMEOUT_MS`] and was ended there; nothing it printed
    /// was read.
    Ended,
}

/// Workflow #453 S10: `<runtime>\actingcommand-actingd.exe discover --config <config>
/// --install-root <root>`, the live build's read-only listing of this machine's emulator instances,
/// within [`DISCOVER_TIMEOUT_MS`]. Its report line (`actingcommand.actingd.discover.v1`) is the
/// answer; a child that prints none (a build before the subcommand refuses its words as a start
/// would, on stderr) is its exit's failure (`foreign_other_error` with the exit code and stderr),
/// and one still running at the bound is ended there, [`Undiscovered::Ended`] (fix 1: never left
/// running, where an installer or the watchdog would take it for the root's Runtime).
pub(crate) fn discover(
    host: &dyn Host,
    runtime: &Path,
    config: &Path,
    install_root: &Path,
) -> Result<Discovered, Undiscovered> {
    let program = runtime.join(DAEMON);
    let config_word = config.to_string_lossy().into_owned();
    let root_word = install_root.to_string_lossy().into_owned();
    let arguments = [
        "discover",
        "--config",
        config_word.as_str(),
        "--install-root",
        root_word.as_str(),
    ]
    .map(OsString::from);
    let ran = host
        .run_program_ending(&program, &arguments, DISCOVER_TIMEOUT_MS)
        .map_err(|not_finished| match not_finished {
            NotFinished::Failed(cause) => Undiscovered::Failed(cause),
            NotFinished::Ended => Undiscovered::Ended,
        })?;
    let report = serde_json::from_slice::<Value>(files::without_bom(&ran.stdout))
        .ok()
        .filter(|report| report["schema_version"] == DISCOVER_SCHEMA);
    let received = report.as_ref().and_then(|report| {
        serde_json::from_value::<OutcomeEnvelope>(report["outcome"].clone())
            .ok()
            .map(OutcomeEnvelope::into_outcome)
    });
    match (report, received) {
        (Some(report), Some(received)) => Ok(Discovered {
            received,
            report: report["report"].clone(),
            exit_code: ran.exit_code,
        }),
        _ => Err(Undiscovered::Failed(exited(&ran))),
    }
}

/// Workflow #453 S7: the schema of `actingd vision`'s two reports.
const VISION_SCHEMA: &str = "actingcommand.actingd.vision.v1";

/// Workflow #453 S7: the Tools' Lab program, which runs `standard-pack check`.
pub(crate) const LAB_PROGRAM: &str = "actinglab.exe";

/// Workflow #453 S7: the bound of `actingd vision manifest` in the rescue row `vision manifest`,
/// at which the child is ended (it is read only: ending it loses nothing). The child reads the
/// registry and the runtime pin compiled into it and waits on nothing: no file of the root, no
/// other program; 30 s is the start of the process with a wide margin for a machine under load.
/// A bound of what the child does, not a tunable.
pub(crate) const VISION_MANIFEST_TIMEOUT_MS: u64 = 30_000;

/// Workflow #453 S7: the bound of `actingd vision check` in the rescue row `vision check`, at
/// which the child is ended. Fix 1 (review): the bound of the worst case the child's own bounds
/// allow (`contracts/vision-model-folders.md`, "The manifest"): it hashes at most 512 MiB in all,
/// the manifest's files (each read at most to its pinned size; about 175 MB for `directml` with
/// this build's pins) and the files of the model folders it is asked about (each at most
/// 256 MiB). 512 MiB at 4 MiB/s, a slow disk under load, is 128 s; 32 s more is the process's
/// start and the listing of `models\`, about the 30 s of `actingd vision manifest`: 160 s. A
/// bound of what the child does, not a tunable.
pub(crate) const VISION_CHECK_TIMEOUT_MS: u64 = 160_000;

/// Workflow #453 S7: the bound of each Tools `standard-pack check` the row `vision check` runs,
/// at which the child is ended. The check admits every package of one game root once, in memory
/// (each within its own 60 s deadline), touches no device and loads no model; a released game
/// root takes seconds. 300 s is five whole package deadlines. `place` runs the same command under
/// `install.child.check_config_timeout_ms` and leaves it running; this read-only row ends it.
pub(crate) const STANDARD_PACK_CHECK_TIMEOUT_MS: u64 = 300_000;

/// Workflow #453 S7 fix 1 (review): the limit of the row `vision check` in all: its children's
/// bounds add up to [`VISION_CHECK_TIMEOUT_MS`] and [`STANDARD_PACK_CHECK_TIMEOUT_MS`] per game
/// root, and the row runs no longer than this however many game roots there are. 900 s is the
/// default of `install.child.check_config_timeout_ms`, the longest `place` waits for one check
/// (it runs the same `standard-pack check` under it): a row that only reads should not keep a
/// person or an installer waiting longer in all than the installer waits for one check of one
/// game root. Each child runs within its own bound or what is left of this, whichever is less; a
/// child ended because this ran out, or not started because none was left, is refused with
/// `timeout_ms` this limit. Not a tunable.
pub(crate) const VISION_CHECK_ROW_LIMIT_MS: u64 = 900_000;

/// Workflow #453 S7: what `actingd vision manifest` or `vision check` answered with its report:
/// its outcome as received, its `report` (`null` when it carries none, as a refusal does) and
/// its exit code.
#[derive(Debug, Clone)]
pub(crate) struct VisionReported {
    pub(crate) received: Outcome,
    pub(crate) report: Value,
    pub(crate) exit_code: Option<i32>,
}

impl VisionReported {
    /// Whether the command answered: exit 0 and a success outcome.
    pub(crate) fn completed(&self) -> bool {
        self.exit_code == Some(0)
            && self.received.category() == actingcommand_contract::outcome::Category::Success
    }
}

/// Workflow #453 S7: why a child of the vision rows gave no answer.
#[derive(Debug, Clone)]
pub(crate) enum Unanswered {
    /// It failed, printed no usable answer, or did not run: the outcome that says why.
    Failed(Outcome),
    /// It was still running at its bound and was ended there; nothing it printed was read.
    Ended,
}

impl From<NotFinished> for Unanswered {
    fn from(not_finished: NotFinished) -> Self {
        match not_finished {
            NotFinished::Failed(cause) => Self::Failed(cause),
            NotFinished::Ended => Self::Ended,
        }
    }
}

/// Workflow #453 S7: `<runtime>\actingcommand-actingd.exe vision manifest --provider
/// <provider>`, the live build's manifest of a vision root, within
/// [`VISION_MANIFEST_TIMEOUT_MS`] and ended there.
pub(crate) fn vision_manifest(
    host: &dyn Host,
    runtime: &Path,
    provider: &str,
) -> Result<VisionReported, Unanswered> {
    vision(
        host,
        runtime,
        &["vision", "manifest", "--provider", provider],
        VISION_MANIFEST_TIMEOUT_MS,
    )
}

/// Workflow #453 S7: `<runtime>\actingcommand-actingd.exe vision check --config <config>
/// --install-root <root>`, the live build's check of the root's configured vision root, within
/// `timeout_ms` ([`VISION_CHECK_TIMEOUT_MS`], or less when the row's limit leaves less) and ended
/// there. Fix 1 (review): with `--model <model_ref>` for each of `models`, in their order, the
/// models the installed packs need, whose folders' identities it computes besides the
/// manifest's.
pub(crate) fn vision_check(
    host: &dyn Host,
    runtime: &Path,
    config: &Path,
    install_root: &Path,
    models: &[&str],
    timeout_ms: u64,
) -> Result<VisionReported, Unanswered> {
    let config_word = config.to_string_lossy().into_owned();
    let root_word = install_root.to_string_lossy().into_owned();
    let mut arguments = vec![
        "vision",
        "check",
        "--config",
        config_word.as_str(),
        "--install-root",
        root_word.as_str(),
    ];
    for &model in models {
        arguments.extend(["--model", model]);
    }
    vision(host, runtime, &arguments, timeout_ms)
}

/// Runs actingd's `vision` with `arguments` within `timeout_ms`, ending it there. Its report line
/// (`actingcommand.actingd.vision.v1`) is the answer; a child that prints none (a build before the
/// subcommand refuses its words as a start would, on stderr) is its exit's failure
/// (`foreign_other_error` with the exit code and stderr).
fn vision(
    host: &dyn Host,
    runtime: &Path,
    arguments: &[&str],
    timeout_ms: u64,
) -> Result<VisionReported, Unanswered> {
    let program = runtime.join(DAEMON);
    let arguments = arguments.iter().map(OsString::from).collect::<Vec<_>>();
    let ran = host.run_program_ending(&program, &arguments, timeout_ms)?;
    let report = serde_json::from_slice::<Value>(files::without_bom(&ran.stdout))
        .ok()
        .filter(|report| report["schema_version"] == VISION_SCHEMA);
    let received = report.as_ref().and_then(|report| {
        serde_json::from_value::<OutcomeEnvelope>(report["outcome"].clone())
            .ok()
            .map(OutcomeEnvelope::into_outcome)
    });
    match (report, received) {
        (Some(report), Some(received)) => Ok(VisionReported {
            received,
            report: report["report"].clone(),
            exit_code: ran.exit_code,
        }),
        _ => Err(Unanswered::Failed(exited(&ran))),
    }
}

/// Workflow #453 S7: one model a game root's packs need, as `standard-pack check` answers it
/// (its member `models`, Workflow #453 S8).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct PackModel {
    pub(crate) model_ref: String,
    pub(crate) model_sha256: String,
    pub(crate) packages: Vec<String>,
}

/// Workflow #453 S7: `<tools>\actinglab.exe --json standard-pack check --root <game root>`, as
/// `place` runs it, within `timeout_ms` ([`STANDARD_PACK_CHECK_TIMEOUT_MS`], or less when the
/// row's limit leaves less) and ended there: the models the game root's packs need (its line's
/// member `models`). A check that does not pass is its line's outcome, or its exit's failure
/// when it printed no line; a passed line without a readable `models` is `foreign_data_error`
/// with the decoder's text.
pub(crate) fn standard_pack_models(
    host: &dyn Host,
    tools: &Path,
    game_root: &Path,
    timeout_ms: u64,
) -> Result<Vec<PackModel>, Unanswered> {
    let program = tools.join(LAB_PROGRAM);
    let root_word = game_root.to_string_lossy().into_owned();
    let arguments = [
        "--json",
        "standard-pack",
        "check",
        "--root",
        root_word.as_str(),
    ]
    .map(OsString::from);
    let ran = host.run_program_ending(&program, &arguments, timeout_ms)?;
    let line = serde_json::from_slice::<Value>(files::without_bom(&ran.stdout)).ok();
    let received = line.as_ref().and_then(|line| {
        serde_json::from_value::<OutcomeEnvelope>(line["outcome"].clone())
            .ok()
            .map(OutcomeEnvelope::into_outcome)
    });
    match (line, received) {
        (Some(line), Some(received))
            if ran.exit_code == Some(0)
                && received.category() == actingcommand_contract::outcome::Category::Success =>
        {
            serde_json::from_value::<Vec<PackModel>>(line["models"].clone()).map_err(|error| {
                Unanswered::Failed(
                    Outcome::new(ContractCode::ForeignDataError)
                        .raw_source(RawSource::Json)
                        .raw_text(error.to_string()),
                )
            })
        }
        (_, Some(received)) => Err(Unanswered::Failed(received)),
        _ => Err(Unanswered::Failed(exited(&ran))),
    }
}

/// `<runtime>\actingcommand-actingd.exe ledger-maintenance verify --config <config>`: `Ok` on
/// exit 0; otherwise the child's failure, with the outcome it reported joined as its `related`
/// link (a fatal report, a ledger integrity failure say, tops it by the fatal-link rule), or
/// [`Failure::TimedOut`] when it is still running at `deadline`.
pub(crate) fn ledger_verify(
    host: &dyn Host,
    runtime: &Path,
    config: &Path,
    deadline: Deadline,
) -> Result<(), Failure> {
    let program = runtime.join(DAEMON);
    let config_word = config.to_string_lossy().into_owned();
    let ran = run(
        host,
        &program,
        &[
            "ledger-maintenance",
            "verify",
            "--config",
            config_word.as_str(),
        ],
        deadline,
    )?;
    if ran.exit_code == Some(0) {
        return Ok(());
    }
    let failure = exited(&ran);
    let reported = serde_json::from_slice::<Value>(files::without_bom(&ran.stdout))
        .ok()
        .and_then(|report| {
            serde_json::from_value::<OutcomeEnvelope>(report["outcome"].clone()).ok()
        });
    Err(Failure::Failed(match reported {
        Some(envelope) => failure.joined(CauseRelation::Related, envelope.into_outcome()),
        None => failure,
    }))
}

/// The ledger head (S0) as `<tools>\actingledger.exe --state-root <state> export --performance
/// --after 0 --through 0 --limit 1` reads it (its `snapshot_latest_sequence`). On an SQLite root
/// that is a range read: one read transaction that authenticates the keyed head row, the
/// contiguity of the stored sequences and the head row, and reads no other row (review S1 F2:
/// never the whole-ledger `open`, which checks the whole file and reads every record).
pub(crate) fn ledger_head(
    host: &dyn Host,
    tools: &Path,
    state_root: &Path,
    deadline: Deadline,
) -> Result<u64, Failure> {
    let program = tools.join(LEDGER_READER);
    let state = state_root.to_string_lossy().into_owned();
    let ran = run(
        host,
        &program,
        &[
            "--state-root",
            state.as_str(),
            "export",
            "--performance",
            "--after",
            "0",
            "--through",
            "0",
            "--limit",
            "1",
        ],
        deadline,
    )?;
    if ran.exit_code != Some(0) {
        return Err(Failure::Failed(exited(&ran)));
    }
    let report: Value = serde_json::from_slice(files::without_bom(&ran.stdout))
        .map_err(|error| Failure::Failed(files::json_failure(&error)))?;
    report
        .pointer("/data/snapshot_latest_sequence")
        .and_then(Value::as_u64)
        .ok_or_else(|| Failure::Failed(exited(&ran)))
}

/// The first page a build's reader refused, or gave no usable answer for.
#[derive(Debug, Clone)]
pub(crate) struct PageRefused {
    /// The page's `--after`.
    pub(crate) after: u64,
    /// When the reader refused the page (exited non-zero): the first sequence it refuses, as
    /// precisely as the reader allows (`first_refused`). `None` when it gave no answer or no
    /// usable one.
    pub(crate) sequence: Option<u64>,
    /// The page's failure.
    pub(crate) failure: Failure,
    /// The failure that cut the search for the sequence short (a read past its deadline, say).
    pub(crate) search: Option<Failure>,
}

impl PageRefused {
    /// `refusal` with the page's failure and the search's, as [`Failure::onto`] attaches them.
    pub(crate) fn onto(self, refusal: Outcome) -> Outcome {
        let refusal = self.failure.onto(refusal);
        match self.search {
            Some(search) => search.onto(refusal),
            None => refusal,
        }
    }
}

/// The words of one `events` page after `after`, through `through` when given.
fn page_words(state: &str, after: u64, through: Option<u64>) -> Vec<String> {
    let mut words = vec![
        "--state-root".to_owned(),
        state.to_owned(),
        "events".to_owned(),
        "--after".to_owned(),
        after.to_string(),
    ];
    if let Some(through) = through {
        words.extend(["--through".to_owned(), through.to_string()]);
    }
    words.extend(["--limit".to_owned(), EVENTS_PAGE.to_string()]);
    words
}

fn run_page(
    host: &dyn Host,
    program: &Path,
    state: &str,
    after: u64,
    through: Option<u64>,
    deadline: Deadline,
) -> Result<Ran, Failure> {
    let words = page_words(state, after, through);
    let words = words.iter().map(String::as_str).collect::<Vec<_>>();
    run(host, program, &words, deadline)
}

/// Reads every page after `after` with `<tools>\actingledger.exe --state-root <state> events
/// --after <n> --limit 1024` up to the head, each read within `deadline`; `Ok(head)` when every
/// page read with exit 0.
pub(crate) fn read_pages(
    host: &dyn Host,
    tools: &Path,
    state_root: &Path,
    after: u64,
    deadline: Deadline,
) -> Result<u64, PageRefused> {
    let program = tools.join(LEDGER_READER);
    let state = state_root.to_string_lossy().into_owned();
    let mut after = after;
    loop {
        let unusable = |failure: Failure| PageRefused {
            after,
            sequence: None,
            failure,
            search: None,
        };
        let ran = run_page(host, &program, &state, after, None, deadline).map_err(unusable)?;
        if ran.exit_code != Some(0) {
            let (sequence, search) = first_refused(host, &program, &state, after, deadline);
            return Err(PageRefused {
                after,
                sequence: Some(sequence),
                failure: Failure::Failed(exited(&ran)),
                search,
            });
        }
        let page: Value = serde_json::from_slice(files::without_bom(&ran.stdout))
            .map_err(|error| unusable(Failure::Failed(files::json_failure(&error))))?;
        let through = page["data"]["through_sequence"]
            .as_u64()
            .ok_or_else(|| unusable(Failure::Failed(exited(&ran))))?;
        match page["data"]["next_after_sequence"].as_u64() {
            Some(next) if next > after => after = next,
            _ => return Ok(through.max(after)),
        }
    }
}

/// The first sequence the reader refuses after `after`, as precisely as the reader allows
/// (review S1 F11): the least `t` whose read `--after <after> --through <t>` exits non-zero,
/// found by halving the refused page, whose rows end at `after + EVENTS_PAGE + 1`. Every read
/// also authenticates the head row (and, for events with artifacts, the eviction rows after the
/// rows read), so the answer is `after + 1` too when only those refuse. A read that gives no
/// answer ends the search at the least refusing `t` found, with that read's failure.
fn first_refused(
    host: &dyn Host,
    program: &Path,
    state: &str,
    after: u64,
    deadline: Deadline,
) -> (u64, Option<Failure>) {
    // `reading` reads (nothing read yet at `after`); `refusing` refuses (the page's own rows).
    let (mut reading, mut refusing) = (after, after + EVENTS_PAGE + 1);
    while refusing - reading > 1 {
        let probe = if reading == after {
            after + 1
        } else {
            reading + (refusing - reading) / 2
        };
        match run_page(host, program, state, after, Some(probe), deadline) {
            Ok(ran) if ran.exit_code == Some(0) => reading = probe,
            Ok(_) => refusing = probe,
            Err(failure) => return (refusing, Some(failure)),
        }
    }
    (refusing, None)
}
