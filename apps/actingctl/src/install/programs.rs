// SPDX-License-Identifier: AGPL-3.0-only

//! The programs of a build that `actingctl install` runs (Workflow #421 INST-1;
//! `contracts/install-cli.md`, "Update and rollback", "The rollback gate"): actingd's
//! `check-config` (the live build in place, a kept build with `--install-root <root>`), its
//! `ledger-maintenance verify`, and actingledger's `open` (the ledger head, S0) and `events`
//! pages (whether a build's own reader reads the ledger after S0).

use super::files;
use super::host::{Host, Ran};
use crate::codes::ActingctlCode;
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{Outcome, OutcomeEnvelope, RawSource};
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

fn words(arguments: &[&str]) -> Vec<OsString> {
    arguments.iter().map(OsString::from).collect()
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
}

impl Checked {
    /// Whether the configuration passed: the outcome is no error.
    pub(crate) fn passed(&self) -> bool {
        self.received.category() <= actingcommand_contract::outcome::Category::Warning
    }
}

/// `<runtime>\actingcommand-actingd.exe check-config --config <config>`, with
/// `--install-root <root>` for a kept build; `check_config_failed` (`path` the program) when it
/// does not run or prints no report.
pub(crate) fn check_config(
    host: &dyn Host,
    runtime: &Path,
    config: &Path,
    install_root: Option<&Path>,
) -> Result<Checked, Outcome> {
    let program = runtime.join(DAEMON);
    let config_word = config.to_string_lossy().into_owned();
    let mut arguments = vec![
        "check-config".to_owned(),
        "--config".to_owned(),
        config_word,
    ];
    if let Some(root) = install_root {
        arguments.push("--install-root".to_owned());
        arguments.push(root.to_string_lossy().into_owned());
    }
    let arguments = arguments
        .into_iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
    let failed = |ran: Option<&Ran>| {
        let outcome = Outcome::new(ActingctlCode::CheckConfigFailed).path(&program);
        match ran {
            Some(ran) => {
                let outcome = match ran.exit_code {
                    Some(exit_code) => outcome.exit_code(exit_code),
                    None => outcome,
                };
                outcome.caused_by(exited(ran))
            }
            None => outcome,
        }
    };
    let ran = host
        .run_program(&program, &arguments)
        .map_err(|cause| failed(None).caused_by(cause))?;
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
    Ok(Checked {
        summary: CheckSummary {
            top: received.code().as_str().to_owned(),
            category: received.category().as_str().to_owned(),
            plan: report["report"]["policy_plan"]["plan"].clone(),
            approvals: report["report"]["policy_plan"]["approvals"].clone(),
            notes,
            packs,
        },
        received,
    })
}

/// `<runtime>\actingcommand-actingd.exe ledger-maintenance verify --config <config>`: `Ok` on
/// exit 0; otherwise the child's failure, with the outcome it reported as its link.
pub(crate) fn ledger_verify(host: &dyn Host, runtime: &Path, config: &Path) -> Result<(), Outcome> {
    let program = runtime.join(DAEMON);
    let config_word = config.to_string_lossy().into_owned();
    let ran = host.run_program(
        &program,
        &words(&[
            "ledger-maintenance",
            "verify",
            "--config",
            config_word.as_str(),
        ]),
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
    Err(match reported {
        Some(envelope) => failure.link(
            actingcommand_contract::outcome::CauseRelation::Related,
            envelope.into_outcome(),
        ),
        None => failure,
    })
}

/// The ledger head (`latest_sequence`) as `<tools>\actingledger.exe --state-root <state> open`
/// reads it.
pub(crate) fn ledger_head(
    host: &dyn Host,
    tools: &Path,
    state_root: &Path,
) -> Result<u64, Outcome> {
    let program = tools.join(LEDGER_READER);
    let state = state_root.to_string_lossy().into_owned();
    let ran = host.run_program(&program, &words(&["--state-root", state.as_str(), "open"]))?;
    if ran.exit_code != Some(0) {
        return Err(exited(&ran));
    }
    let report: Value = serde_json::from_slice(files::without_bom(&ran.stdout))
        .map_err(|error| files::json_failure(&error))?;
    report["data"]["latest_sequence"]
        .as_u64()
        .ok_or_else(|| exited(&ran))
}

/// The first page a build's reader refused: its first sequence and the child's failure.
pub(crate) struct PageRefused {
    pub(crate) sequence: u64,
    pub(crate) cause: Outcome,
}

/// Reads every page after `after` with `<tools>\actingledger.exe --state-root <state> events
/// --after <n> --limit 1024` up to the head; `Ok(head)` when every page read with exit 0.
pub(crate) fn read_pages(
    host: &dyn Host,
    tools: &Path,
    state_root: &Path,
    after: u64,
) -> Result<u64, PageRefused> {
    let program = tools.join(LEDGER_READER);
    let state = state_root.to_string_lossy().into_owned();
    let limit = EVENTS_PAGE.to_string();
    let mut after = after;
    loop {
        let after_word = after.to_string();
        let refused = |cause: Outcome| PageRefused {
            sequence: after + 1,
            cause,
        };
        let ran = host
            .run_program(
                &program,
                &words(&[
                    "--state-root",
                    state.as_str(),
                    "events",
                    "--after",
                    after_word.as_str(),
                    "--limit",
                    limit.as_str(),
                ]),
            )
            .map_err(refused)?;
        if ran.exit_code != Some(0) {
            return Err(refused(exited(&ran)));
        }
        let page: Value = serde_json::from_slice(files::without_bom(&ran.stdout))
            .map_err(|error| refused(files::json_failure(&error)))?;
        let through = page["data"]["through_sequence"]
            .as_u64()
            .ok_or_else(|| refused(exited(&ran)))?;
        match page["data"]["next_after_sequence"].as_u64() {
            Some(next) if next > after => after = next,
            _ => return Ok(through.max(after)),
        }
    }
}
