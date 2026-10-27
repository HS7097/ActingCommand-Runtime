// SPDX-License-Identifier: AGPL-3.0-only

//! The programs of a build that `actingctl install` runs (Workflow #421 INST-1;
//! `contracts/install-cli.md`, "Update and rollback", "The rollback gate"): actingd's
//! `check-config` (the live build in place, a kept build with `--install-root <root>` when it
//! takes the option), its `ledger-maintenance verify`, and actingledger's bounded head read (S0)
//! and `events` pages (whether a build's own reader reads the ledger after S0).
//!
//! INST-1b-F fix round 1 (review S1 F4): every child runs within its deadline, a key of the
//! `install` section (`install.child.*`). A child still running then is left running, never
//! killed (the stall rule), and its failure names its `pid` and the key.

use super::files;
use super::host::{Host, NotRan, Ran};
use crate::codes::ActingctlCode;
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{CauseRelation, Outcome, OutcomeEnvelope, RawSource};
use actingcommand_runtime_config::install::{self as keys, InstallSettings};
use serde::Serialize;
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

/// Runs `program` with `arguments` within `deadline`.
fn run(
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
    };
    let refused_arguments = !checked.passed()
        && report.get("report").is_none_or(Value::is_null)
        && checked.received.values().is_empty();
    Ok((checked, refused_arguments))
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
