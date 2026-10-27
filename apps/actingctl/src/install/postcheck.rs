// SPDX-License-Identifier: AGPL-3.0-only

//! The installer's post-start checks (Workflow #421 INST-2; #421 deploy design §4): each
//! `{id, result: pass | fail | not_evaluated, fail_class, values}`, judged against S0, the
//! configuration and the install state, never against per-release expectations. The Runtime's
//! own report of its start (`StartReport`) is read when the started build knows it; a build
//! that does not (a rollback target before INST-2) is checked from its status, its
//! configuration facts and its start log's `OUTCOME actingd:` lines, and what they cannot show
//! is `not_evaluated`. A failed fail-class check is what an automatic rollback acts on;
//! `not_evaluated` never is. Fix round 1 (review S2 H2): a Runtime that is gone at check time
//! fails `ready` (the owner the start made ready must still answer), and a `StartReport` read
//! that fails for any other reason than an older build fails the fail-class checks it feeds;
//! (review S2 H1) `config` is judged against the file as it was when the operation began, so an
//! edit after the adopt is no failure.
//!
//! | id | judged | fail-class |
//! |---|---|---|
//! | `ready` | the start reached readiness, and its owner still answers | yes |
//! | `open` | the open's phases within the deadline, no `ledger_open_slow` | no |
//! | `no_seeding` | a root that ran before loaded its tagged state, not seeded again | yes |
//! | `bindings` | every configured instance bound | yes |
//! | `packs` | every pack of the register registered, with its count | yes |
//! | `pauses` | S0's pauses plus the hold on every instance it took | yes |
//! | `instructions` | S0's run-time instructions, none dropped | yes |
//! | `no_dispatch` | no dispatch on a held instance since the start | yes |
//! | `new_keys` | each key the update adds `default`, or `explicit` where the file names it | yes |
//! | `config` | the configuration's SHA-256 is the one the operation began with | yes |
//! | `indexes` | the open noted no incomplete derived index | no |
//! | `one_way` | the one-way watch (INST S6, a later slice) | no |

use super::files;
use super::hold;
use super::host::Host;
use super::journal::S0;
use super::runtime::{ConfigSeen, RuntimeSeen};
use super::state::InstallState;
use actingcommand_contract::outcome::{Category, Outcome, OutcomeEnvelope};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

/// How much of a start log's beginning the open's lines are read from.
const LOG_HEAD_BYTES: u64 = 4 * 1024 * 1024;
const DAEMON_LINE: &str = "OUTCOME actingd: ";
const PHASES: [&str; 8] = [
    "sql_read_ms",
    "verify_ms",
    "material_ms",
    "restore_ms",
    "meta_ms",
    "tail_ms",
    "index_ms",
    "import_ms",
];

/// A check's result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CheckResult {
    Pass,
    Fail,
    NotEvaluated,
}

/// One post-start check.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct CheckRow {
    pub(crate) id: &'static str,
    pub(crate) result: CheckResult,
    pub(crate) fail_class: bool,
    pub(crate) values: Value,
}

impl CheckRow {
    /// Whether this check failed and is of the class an automatic rollback acts on.
    pub(crate) fn fails_class(&self) -> bool {
        self.fail_class && self.result == CheckResult::Fail
    }
}

fn check(id: &'static str, fail_class: bool, result: CheckResult, values: Value) -> CheckRow {
    CheckRow {
        id,
        result,
        fail_class,
        values,
    }
}

fn verdict(passes: bool) -> CheckResult {
    if passes {
        CheckResult::Pass
    } else {
        CheckResult::Fail
    }
}

/// What the checks judge against.
pub(crate) struct CheckInputs<'a> {
    /// The start's own readiness (`ready_ms`); `None` when it did not reach it.
    pub(crate) ready: Option<u64>,
    /// The owner epoch the start made ready, which must still answer.
    pub(crate) started_epoch: Option<&'a str>,
    /// The start's log.
    pub(crate) log: &'a Path,
    /// S0 of the operation that stopped this Runtime, when there is one.
    pub(crate) s0: Option<&'a S0>,
    /// The hold: its reason and the instances it took.
    pub(crate) hold: Option<&'a hold::PauseIntent>,
    /// The root's configuration document and the SHA-256 of the live file (`sha256:<hex>`).
    pub(crate) config: &'a Value,
    pub(crate) config_sha256: &'a str,
    /// The SHA-256 of the configuration file when the operation began (its journal's inputs).
    pub(crate) begun_config_sha256: Option<&'a str>,
    pub(crate) state: &'a InstallState,
    /// The configuration keys the update adds (absent from the previous defaults).
    pub(crate) added_keys: &'a [String],
}

/// The open as a start log's daemon lines report it: the sum of its phases, its deadline,
/// whether `ledger_open_slow` followed, whether it carried a note (the derived frame index).
struct LoggedOpen {
    sum_ms: u64,
    deadline_ms: Option<u64>,
    slow: bool,
    noted: bool,
}

fn logged_open(log: &Path) -> Option<LoggedOpen> {
    let file = std::fs::File::open(log).ok()?;
    let mut reader = BufReader::new(file.take(LOG_HEAD_BYTES));
    let mut open: Option<LoggedOpen> = None;
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        match reader.read_until(b'\n', &mut bytes) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let line = String::from_utf8_lossy(&bytes);
        let Some(envelope) = line.trim().strip_prefix(DAEMON_LINE) else {
            continue;
        };
        let Ok(envelope) = serde_json::from_str::<OutcomeEnvelope>(envelope) else {
            continue;
        };
        let outcome = envelope.into_outcome();
        // The open's warning that it took long (`ledger_open_slow`, the Runtime host's): a
        // warning with the open's elapsed time, its deadline and its event count.
        let slow_warning = outcome.category() == Category::Warning
            && ["elapsed_ms", "timeout_ms", "event_count"]
                .iter()
                .all(|key| outcome.value(key).is_some());
        if slow_warning {
            if let Some(open) = open.as_mut() {
                open.slow = true;
            }
            continue;
        }
        let phases = PHASES
            .iter()
            .map(|key| outcome.value(key).and_then(Value::as_u64))
            .collect::<Vec<_>>();
        if open.is_none() && phases[..4].iter().all(Option::is_some) {
            open = Some(LoggedOpen {
                sum_ms: phases.into_iter().flatten().fold(0, u64::saturating_add),
                deadline_ms: outcome.value("timeout_ms").and_then(Value::as_u64),
                slow: false,
                noted: !outcome.causes().is_empty(),
            });
        }
    }
    open
}

/// The configured instance aliases.
fn configured_aliases(config: &Value) -> BTreeSet<String> {
    config
        .get("instances")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|instance| instance.get("alias")?.as_str().map(str::to_owned))
        .collect()
}

/// Every check, in table order, for the Runtime of `state_root` the start just made ready.
pub(crate) fn evaluate(
    host: &dyn Host,
    state_root: &Path,
    inputs: &CheckInputs<'_>,
) -> Vec<CheckRow> {
    let runtime = host.runtime();
    let seen: Result<Option<RuntimeSeen>, String> = runtime
        .status(state_root)
        .map_err(|failure| failure.code().as_str().to_owned());
    let config_seen: Result<ConfigSeen, String> = runtime
        .config(state_root)
        .map_err(|failure| failure.code().as_str().to_owned());
    // The report of the start: `None` from a build that does not know the read (its client
    // answered `runtime_operation_unsupported`); any other failure is kept (review S2 H2).
    let report_read: Result<Option<Value>, Outcome> = runtime.start_report(state_root);
    let report: Option<Value> = report_read.as_ref().ok().cloned().flatten();
    let report_failed = report_read
        .as_ref()
        .err()
        .map(|failure| json!({"start_report": crate::cli_line::warning(failure)}));
    let mut rows = Vec::new();
    let unread = |id: &'static str, fail_class: bool, why: &str| {
        check(
            id,
            fail_class,
            CheckResult::NotEvaluated,
            json!({"why": why}),
        )
    };
    // A fail-class check the report feeds, when the read failed: failed (never a vacuous pass).
    let report_unread = |id: &'static str| {
        report_failed
            .as_ref()
            .map(|values| check(id, true, CheckResult::Fail, values.clone()))
    };

    // ready: the start reached readiness and the owner it made ready still answers (a Runtime
    // that died after readiness fails here, review S2 H2).
    rows.push(match (inputs.ready, &seen) {
        (None, _) => check("ready", true, CheckResult::Fail, json!({"ready": false})),
        (Some(ready_ms), Ok(Some(seen)))
            if inputs
                .started_epoch
                .is_none_or(|epoch| epoch == seen.owner_epoch) =>
        {
            check(
                "ready",
                true,
                CheckResult::Pass,
                json!({"ready_ms": ready_ms, "owner_epoch": seen.owner_epoch}),
            )
        }
        (Some(ready_ms), Ok(Some(seen))) => check(
            "ready",
            true,
            CheckResult::Fail,
            json!({"ready_ms": ready_ms, "runtime": "restarted",
                "started_epoch": inputs.started_epoch, "owner_epoch": seen.owner_epoch}),
        ),
        (Some(ready_ms), Ok(None)) => check(
            "ready",
            true,
            CheckResult::Fail,
            json!({"ready_ms": ready_ms, "runtime": "gone"}),
        ),
        (Some(ready_ms), Err(why)) => check(
            "ready",
            true,
            CheckResult::Fail,
            json!({"ready_ms": ready_ms, "runtime": "status_unreadable", "why": why}),
        ),
    });

    // open
    rows.push(match &report {
        Some(report) => {
            let open = &report["open"];
            let sum = open["sum_ms"].as_u64().unwrap_or(u64::MAX);
            let deadline = open["deadline_ms"].as_u64().unwrap_or(0);
            let slow = open["slow"].as_bool().unwrap_or(true);
            check(
                "open",
                false,
                verdict(sum <= deadline && !slow),
                json!({"source": "start_report", "phases": open["phases"], "sum_ms": sum,
                    "deadline_ms": deadline, "slow": slow, "owner_read_full": open["owner_read_full"]}),
            )
        }
        None => match logged_open(inputs.log) {
            Some(open) => check(
                "open",
                false,
                verdict(
                    open.deadline_ms.is_some_and(|deadline| open.sum_ms <= deadline) && !open.slow,
                ),
                json!({"source": "start_log", "sum_ms": open.sum_ms,
                    "deadline_ms": open.deadline_ms, "slow": open.slow}),
            ),
            None => unread("open", false, "no_open_line"),
        },
    });

    // no_seeding: a root that ran before (an S0) loads the tagged state it seeded.
    rows.push(match (&report, inputs.s0) {
        (Some(report), Some(_)) => {
            let state = report["tagged_state"].as_str().unwrap_or("unknown");
            check(
                "no_seeding",
                true,
                verdict(state == "load"),
                json!({"tagged_state": state}),
            )
        }
        (Some(report), None) => check(
            "no_seeding",
            true,
            CheckResult::Pass,
            json!({"tagged_state": report["tagged_state"], "earlier_runtime": false}),
        ),
        (None, _) => report_unread("no_seeding")
            .unwrap_or_else(|| unread("no_seeding", true, "start_report_unavailable")),
    });

    // bindings
    let configured = configured_aliases(inputs.config);
    let bound = match (&report, &seen) {
        (Some(report), _) => Some(
            report["instances_bound"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<BTreeSet<_>>(),
        ),
        (None, Ok(Some(seen))) if report_failed.is_none() => Some(
            seen.instances
                .iter()
                .map(|instance| instance.alias.clone())
                .collect::<BTreeSet<_>>(),
        ),
        _ => None,
    };
    rows.push(match bound {
        Some(bound) => {
            let missing = configured.difference(&bound).cloned().collect::<Vec<_>>();
            check(
                "bindings",
                true,
                verdict(missing.is_empty()),
                json!({"configured": configured, "missing": missing}),
            )
        }
        None => report_unread("bindings")
            .unwrap_or_else(|| unread("bindings", true, "status_unreadable")),
    });

    // packs
    rows.push(match &report {
        Some(report) => {
            let registered = report["standard_packs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|row| {
                    Some((
                        row["game"].as_str()?.to_owned(),
                        row["pack_count"].as_u64()?,
                    ))
                })
                .collect::<BTreeMap<_, _>>();
            let missing = inputs
                .state
                .packs
                .iter()
                .filter(|pack| registered.get(&pack.game).is_none_or(|count| *count == 0))
                .map(|pack| pack.game.clone())
                .collect::<Vec<_>>();
            check(
                "packs",
                true,
                verdict(missing.is_empty()),
                json!({"registered": registered, "missing": missing}),
            )
        }
        None => report_unread("packs")
            .unwrap_or_else(|| unread("packs", true, "start_report_unavailable")),
    });

    // pauses
    rows.push(match (inputs.s0.and_then(|s0| s0.pauses.as_ref()), &seen) {
        (Some(pauses), Ok(Some(seen))) => {
            let mut expected = hold::pause_reasons_of(pauses);
            if let Some(hold) = inputs.hold {
                for alias in &hold.instances {
                    expected.insert(alias.clone(), hold.reason.as_str().to_owned());
                }
            }
            let actual = seen
                .instances
                .iter()
                .filter_map(|instance| {
                    Some((
                        instance.alias.clone(),
                        instance.pause.as_ref()?.reason.as_str().to_owned(),
                    ))
                })
                .collect::<BTreeMap<_, _>>();
            let global_expected = pauses["global"]["reason"].as_str().map(str::to_owned);
            let global_actual = seen
                .global_pause
                .as_ref()
                .map(|pause| pause.reason.as_str().to_owned());
            check(
                "pauses",
                true,
                verdict(expected == actual && global_expected == global_actual),
                json!({"expected": expected, "actual": actual,
                    "global_expected": global_expected, "global_actual": global_actual}),
            )
        }
        (None, _) => unread("pauses", true, "no_s0"),
        (Some(_), _) => unread("pauses", true, "status_unreadable"),
    });

    // instructions
    rows.push(
        match (
            inputs.s0.and_then(|s0| s0.instructions.as_ref()),
            &config_seen,
        ) {
            (Some(_), Ok(_)) if report_failed.is_some() => {
                report_unread("instructions").unwrap_or_else(|| unread("instructions", true, ""))
            }
            (Some(recorded), Ok(config)) => {
                let expected = hold::instructions_of(recorded);
                let dropped = report
                    .as_ref()
                    .and_then(|report| report["instructions_dropped"].as_array().cloned())
                    .unwrap_or_default();
                check(
                    "instructions",
                    true,
                    verdict(expected == config.instructions && dropped.is_empty()),
                    json!({"expected": hold::instructions_value(&expected),
                        "actual": hold::instructions_value(&config.instructions),
                        "dropped": dropped}),
                )
            }
            (None, _) => unread("instructions", true, "no_s0"),
            (Some(_), Err(code)) => unread("instructions", true, code.as_str()),
        },
    );

    // no_dispatch
    let held = inputs
        .hold
        .map(|hold| hold.instances.clone())
        .unwrap_or_default();
    rows.push(match (&report, &seen) {
        (Some(report), _) => {
            let dispatched = report["dispatches_since_start"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|row| {
                    let alias = row["instance_alias"].as_str()?;
                    (held.iter().any(|known| known == alias) && row["count"].as_u64()? > 0)
                        .then(|| alias.to_owned())
                })
                .collect::<Vec<_>>();
            check(
                "no_dispatch",
                true,
                verdict(dispatched.is_empty()),
                json!({"source": "start_report", "dispatched": dispatched}),
            )
        }
        (None, _) if report_failed.is_some() => {
            report_unread("no_dispatch").unwrap_or_else(|| unread("no_dispatch", true, ""))
        }
        (None, Ok(Some(seen))) => {
            let busy = seen
                .instances
                .iter()
                .filter(|instance| held.contains(&instance.alias) && instance.lease_active)
                .map(|instance| instance.alias.clone())
                .collect::<Vec<_>>();
            check(
                "no_dispatch",
                true,
                verdict(busy.is_empty()),
                json!({"source": "status", "lease_active": busy}),
            )
        }
        (None, _) => unread("no_dispatch", true, "status_unreadable"),
    });

    // new_keys
    rows.push(match &config_seen {
        Ok(config) => {
            let mut wrong = Vec::new();
            let mut unseen = Vec::new();
            for key in inputs.added_keys {
                let named = actingcommand_runtime_config::member(inputs.config, key)
                    .is_some_and(|value| !value.is_null());
                let wanted = if named { "explicit" } else { "default" };
                match config.sources.get(key) {
                    Some(source) if source == wanted => {}
                    Some(source) => {
                        wrong.push(json!({"key": key, "wanted": wanted, "source": source}))
                    }
                    None => unseen.push(key.clone()),
                }
            }
            let result = if !wrong.is_empty() {
                CheckResult::Fail
            } else if !unseen.is_empty() && unseen.len() == inputs.added_keys.len() {
                CheckResult::NotEvaluated
            } else {
                CheckResult::Pass
            };
            check(
                "new_keys",
                true,
                result,
                json!({"keys": inputs.added_keys, "wrong": wrong, "unseen": unseen}),
            )
        }
        Err(code) => unread("new_keys", true, code.as_str()),
    });

    // config: the file as it was when the operation began (review S2 H1: an edit after the
    // adopt, a catalog switch or `install.hold_after_start`, is no failure; a change during the
    // operation is). A `state.json` hash that differs is reported beside it, never failed.
    let committed = inputs.state.config_sha256.as_deref();
    rows.push(match inputs.begun_config_sha256 {
        Some(begun) => check(
            "config",
            true,
            verdict(begun == inputs.config_sha256),
            json!({"at_begin": begun, "live": inputs.config_sha256, "committed": committed,
                "committed_differs": committed.is_some_and(|committed| committed != inputs.config_sha256)}),
        ),
        None => unread("config", true, "no_begin_hash"),
    });

    // indexes
    rows.push(match &report {
        Some(report) => {
            let note = report["open"]["index_note"].clone();
            check(
                "indexes",
                false,
                verdict(note.is_null()),
                json!({"source": "start_report", "index_note": note}),
            )
        }
        None => match logged_open(inputs.log) {
            Some(open) => check(
                "indexes",
                false,
                verdict(!open.noted),
                json!({"source": "start_log", "noted": open.noted}),
            ),
            None => unread("indexes", false, "no_open_line"),
        },
    });

    // one_way: the one-way watch is a later slice (S6).
    rows.push(unread("one_way", false, "watch_not_in_this_build"));
    rows
}

/// The ids of the failed fail-class checks.
pub(crate) fn failed(rows: &[CheckRow]) -> Vec<String> {
    rows.iter()
        .filter(|row| row.fails_class())
        .map(|row| row.id.to_owned())
        .collect()
}

/// How many checks did not pass (a mismatch the caveat `post_check_mismatch` counts).
pub(crate) fn mismatched(rows: &[CheckRow]) -> usize {
    rows.iter()
        .filter(|row| row.result == CheckResult::Fail)
        .count()
}

/// The rows as the journal and the result list them.
pub(crate) fn values(rows: &[CheckRow]) -> Vec<Value> {
    rows.iter()
        .map(|row| serde_json::to_value(row).unwrap_or(Value::Null))
        .collect()
}

/// The keys an update adds: those of the plan's defaults diff the live build's defaults lack.
pub(crate) fn added_keys(plan: Option<&Value>) -> Vec<String> {
    plan.and_then(|plan| plan.get("defaults_diff"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|row| row.get("live").is_none_or(Value::is_null))
        .filter_map(|row| row.get("key")?.as_str().map(str::to_owned))
        .collect()
}

/// The SHA-256 of the root's configuration file as `state.json` spells it.
pub(crate) fn config_sha256(path: &Path) -> String {
    files::read_all(path)
        .map(|bytes| format!("sha256:{}", files::sha256_bytes(&bytes)))
        .unwrap_or_default()
}
