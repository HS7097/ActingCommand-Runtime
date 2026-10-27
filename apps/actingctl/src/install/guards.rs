// SPDX-License-Identifier: AGPL-3.0-only

//! The installer's guards (Workflow #421 INST-2; `contracts/install-cli.md`, "Guards"): each is
//! `{id, result: pass | fail | not_evaluated, values}`, and a `fail` or `not_evaluated` refuses
//! `guard_failed` (`install_guards`) unless the person names it with `--ack-guard <id>`. INST-2
//! evaluates `memory` (available physical memory against
//! `install.guard.min_available_memory_mb`; before a stop the running Runtime's own working set
//! counts as available, since the stop releases it (fix round 1, review S2 M2); before every
//! start attempt too), `adb` (each configured instance with a port answers `shell echo ok`
//! through an adb server already running from `<root>\tools\platform-tools\`, a probe that does
//! not answer tried once more; Tools never starts, connects or kills one), `foreign_pauses` (no
//! global pause and no unnamed pause of another party, neither of which an acknowledgment
//! passes; a pause `--known-pause` names is kept) and `schedule_near`. Decision 75 as Alice
//! changed it on 10-27: `schedule_near` is informational (it lists what is due on a held
//! instance within `install.guard.before_next_start_ms`, by the Runtime's latest plan, which the
//! hold defers and records; it never refuses, and `--ack-guard schedule_near` is accepted with no
//! effect), and there is no allowed or blocked deploy time span, so `blocked_window` is not
//! evaluated and its key is retired (`actingd-config` revision 18).

use super::files;
use super::host::{Host, NotRan};
use super::runtime::{PlanSeen, RuntimeSeen};
use super::state::Paths;
use crate::codes::{InstallGuard, InstallLocation, InstallRefusal};
use actingcommand_contract::SchedulingPauseReason;
use actingcommand_contract::outcome::Outcome;
use actingcommand_runtime_config::install::InstallSettings;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::ffi::OsString;

/// A guard's result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GuardResult {
    Pass,
    Fail,
    NotEvaluated,
}

/// One guard as a command lists it.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct GuardRow {
    pub(crate) id: &'static str,
    #[serde(skip)]
    pub(crate) guard: InstallGuard,
    pub(crate) result: GuardResult,
    pub(crate) values: Value,
    /// Whether `--ack-guard` may pass it (a global pause or an unnamed foreign pause it may not).
    #[serde(skip)]
    pub(crate) ackable: bool,
    /// Whether it only lists and never refuses (`schedule_near`, decision 75 of 10-27).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) informational: bool,
}

fn row(guard: InstallGuard, result: GuardResult, values: Value) -> GuardRow {
    GuardRow {
        id: guard.as_str(),
        guard,
        result,
        values,
        ackable: true,
        informational: false,
    }
}

impl GuardRow {
    /// Whether this row refuses its command unless acknowledged.
    pub(crate) fn refuses(&self) -> bool {
        !self.informational && self.result != GuardResult::Pass
    }
}

/// The guards INST-2 evaluates, by id.
const EVALUATED: [InstallGuard; 4] = [
    InstallGuard::Memory,
    InstallGuard::Adb,
    InstallGuard::ForeignPauses,
    InstallGuard::ScheduleNear,
];

/// The `--ack-guard` values: each the id of a guard INST-2 evaluates, each once; any other is
/// `actingctl_usage_invalid` (`unusable`). `schedule_near` stays accepted so run sheets keep
/// working, with no effect: the guard never refuses (decision 75 of 10-27).
pub(crate) fn parse_acks(values: &[&str]) -> Result<Vec<String>, Outcome> {
    let mut acks = Vec::new();
    for value in values {
        if !EVALUATED.iter().any(|guard| guard.as_str() == *value) {
            return Err(super::unusable("--ack-guard"));
        }
        if !acks.iter().any(|known: &String| known == value) {
            acks.push((*value).to_owned());
        }
    }
    Ok(acks)
}

/// `memory`: the available physical memory against the key.
pub(crate) fn memory(host: &dyn Host, settings: &InstallSettings) -> GuardRow {
    let minimum = settings.guard_min_available_memory_mb;
    match host.available_memory_mb() {
        Ok(available) => row(
            InstallGuard::Memory,
            if available >= minimum {
                GuardResult::Pass
            } else {
                GuardResult::Fail
            },
            json!({"available_mb": available, "min_available_mb": minimum}),
        ),
        Err(failure) => row(
            InstallGuard::Memory,
            GuardResult::NotEvaluated,
            json!({"min_available_mb": minimum, "failure": crate::cli_line::warning(&failure)}),
        ),
    }
}

/// `memory` before a stop (fix round 1, review S2 M2): the available physical memory plus the
/// working set of the root's running Runtime, which the stop releases before the start, against
/// the key. A working set that cannot be read counts as none.
pub(crate) fn memory_before_stop(
    host: &dyn Host,
    paths: &Paths,
    settings: &InstallSettings,
) -> GuardRow {
    let mut guard = memory(host, settings);
    let Some(available) = guard.values["available_mb"].as_u64() else {
        return guard;
    };
    let released = host.runtime_working_set_mb(&paths.root);
    let after_stop = available.saturating_add(released.unwrap_or(0));
    guard.result = if after_stop >= settings.guard_min_available_memory_mb {
        GuardResult::Pass
    } else {
        GuardResult::Fail
    };
    guard.values["runtime_working_set_mb"] = json!(released);
    guard.values["available_after_stop_mb"] = json!(after_stop);
    guard
}

/// `adb`: each configured instance with a port answers `shell echo ok` through the adb server
/// that already runs from the root's `tools\platform-tools\`. No running server is
/// `not_evaluated`; a probe still running at `install.guard.adb_timeout_ms` is left running and
/// fails its instance.
pub(crate) fn adb(
    host: &dyn Host,
    paths: &Paths,
    config: &Value,
    settings: &InstallSettings,
) -> GuardRow {
    let program = paths
        .root
        .join("tools")
        .join("platform-tools")
        .join("adb.exe");
    let targets = config
        .get("instances")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|instance| {
            let alias = instance.get("alias")?.as_str()?;
            let port = instance.get("port")?.as_u64()?;
            let address = instance
                .get("host")
                .and_then(Value::as_str)
                .unwrap_or("127.0.0.1");
            Some((alias.to_owned(), port, format!("{address}:{port}")))
        })
        .collect::<Vec<_>>();
    if targets.is_empty() {
        return row(
            InstallGuard::Adb,
            GuardResult::Pass,
            json!({"instances": []}),
        );
    }
    let server = match host.processes_under(&paths.root) {
        Ok(images) => images.iter().any(|image| {
            image
                .path
                .to_ascii_lowercase()
                .ends_with("\\tools\\platform-tools\\adb.exe")
        }),
        Err(failure) => {
            return row(
                InstallGuard::Adb,
                GuardResult::NotEvaluated,
                json!({"failure": crate::cli_line::warning(&failure)}),
            );
        }
    };
    if !server {
        return row(
            InstallGuard::Adb,
            GuardResult::NotEvaluated,
            json!({"server": "not_running", "path": files::plain(&program)}),
        );
    }
    let mut answered = Vec::new();
    let mut passes = true;
    for (alias, port, serial) in targets {
        let arguments = ["-s", serial.as_str(), "shell", "echo", "ok"]
            .iter()
            .map(OsString::from)
            .collect::<Vec<_>>();
        // A probe that does not answer is tried once more (review S2 L7): one hiccup of the
        // bridge does not refuse the deploy. One still running at its deadline is left running
        // and not started again beside it.
        let mut result = "not_run";
        for _ in 0..2 {
            result = match host.run_program(&program, &arguments, settings.guard_adb_timeout_ms) {
                Ok(ran)
                    if ran.exit_code == Some(0)
                        && String::from_utf8_lossy(&ran.stdout).trim() == "ok" =>
                {
                    "ok"
                }
                Ok(_) => "no_answer",
                Err(NotRan::StillRunning { .. }) => "timeout",
                Err(NotRan::Failed(_)) => "not_run",
            };
            if result == "ok" || result == "timeout" {
                break;
            }
        }
        passes &= result == "ok";
        answered.push(json!({"alias": alias, "port": port, "result": result}));
    }
    row(
        InstallGuard::Adb,
        if passes {
            GuardResult::Pass
        } else {
            GuardResult::Fail
        },
        json!({"instances": answered}),
    )
}

/// `foreign_pauses`: no global pause, and every instance pause of another party named by
/// `--known-pause` with exactly its reason; neither a global pause nor an unnamed pause is
/// passed by an acknowledgment (review S2 L6: "allowed only when named"). The hold's own reason
/// (`hold`, none when the command holds nothing) is no other party's.
pub(crate) fn foreign_pauses(
    seen: &RuntimeSeen,
    hold: Option<&SchedulingPauseReason>,
    known: &BTreeMap<String, String>,
) -> GuardRow {
    if let Some(global) = &seen.global_pause {
        let mut refused = row(
            InstallGuard::ForeignPauses,
            GuardResult::Fail,
            json!({"global": global}),
        );
        refused.ackable = false;
        return refused;
    }
    let unnamed = seen
        .instances
        .iter()
        .filter_map(|instance| {
            let pause = instance.pause.as_ref()?;
            let named =
                known.get(&instance.alias).map(String::as_str) == Some(pause.reason.as_str());
            (Some(&pause.reason) != hold && !named)
                .then(|| super::hold::foreign_pause_value(&instance.alias, pause))
        })
        .collect::<Vec<_>>();
    let mut guard = row(
        InstallGuard::ForeignPauses,
        if unnamed.is_empty() {
            GuardResult::Pass
        } else {
            GuardResult::Fail
        },
        json!({"unnamed": unnamed, "known": known}),
    );
    guard.ackable = unnamed.is_empty();
    guard
}

/// `schedule_near`, informational (Alice's decision 75 of 10-27): what is due on an instance the
/// hold takes within `install.guard.before_next_start_ms`: by the Runtime's latest plan, a
/// selected decision or a due task is due now, a wake within the key is due then. The hold
/// defers and records it; the row never refuses (`pass` with the list in `due`, or
/// `not_evaluated` when the plan cannot be read).
pub(crate) fn schedule_near(
    plan: Result<&PlanSeen, &Outcome>,
    now_unix_ms: u64,
    held: &[String],
    settings: &InstallSettings,
) -> GuardRow {
    let within = settings.guard_before_next_start_ms;
    let informational = |result: GuardResult, values: Value| GuardRow {
        informational: true,
        ..row(InstallGuard::ScheduleNear, result, values)
    };
    let plan = match plan {
        Ok(plan) => plan,
        Err(failure) => {
            return informational(
                GuardResult::NotEvaluated,
                json!({"failure": crate::cli_line::warning(failure)}),
            );
        }
    };
    let due = held
        .iter()
        .filter_map(|alias| {
            let entry = plan.row(alias)?;
            let now = entry.selected.is_some() || !entry.due_tasks.is_empty();
            let soon = entry
                .next_wake_unix_ms
                .is_some_and(|wake| wake <= now_unix_ms.saturating_add(within));
            (now || soon).then(|| {
                json!({
                    "alias": alias,
                    "next_wake_unix_ms": entry.next_wake_unix_ms,
                    "due_tasks": entry.due_tasks,
                    "selected": entry.selected,
                })
            })
        })
        .collect::<Vec<_>>();
    informational(
        GuardResult::Pass,
        json!({"due": due, "before_next_start_ms": within}),
    )
}

/// The refusal `guard_failed` of `operation` for every guard that refuses (did not pass and is
/// not informational) and was not acknowledged (or may not be), with every such guard as
/// `install_guards`; `Ok` otherwise.
pub(crate) fn refuse(
    operation: InstallLocation,
    rows: &[GuardRow],
    acked: &[String],
) -> Result<(), Outcome> {
    let failed = rows
        .iter()
        .filter(|row| row.refuses() && (!row.ackable || !acked.iter().any(|ack| ack == row.id)))
        .map(|row| row.guard)
        .collect::<Vec<_>>();
    if failed.is_empty() {
        return Ok(());
    }
    Err(super::refusal(operation, InstallRefusal::GuardFailed).install_guards(failed))
}

/// The rows as the journal and the result list them.
pub(crate) fn values(rows: &[GuardRow]) -> Vec<Value> {
    rows.iter()
        .map(|row| serde_json::to_value(row).unwrap_or(Value::Null))
        .collect()
}
