// SPDX-License-Identifier: AGPL-3.0-only

//! The installer's post-start checks (Workflow #421 INST-2; #421 deploy design §4): each
//! `{id, result: pass | fail | not_evaluated, fail_class, values}`, judged against S0, the
//! configuration and the install state, never against per-release expectations. The Runtime's
//! own report of its start (`StartReport`) is read when the started build knows it; a build
//! that does not (a rollback target before INST-2) is checked from its status, its
//! configuration facts and its start log's `OUTCOME actingd:` lines, and what they cannot show
//! is `not_evaluated`. A failed fail-class check is what an automatic rollback acts on;
//! `not_evaluated` never is.
//!
//! | id | judged | fail-class |
//! |---|---|---|
//! | `ready` | the start reached readiness | yes |
//! | `open` | the open's phases within the deadline, no `ledger_open_slow` | no |
//! | `no_seeding` | a root that ran before loaded its tagged state, not seeded again | yes |
//! | `bindings` | every configured instance bound | yes |
//! | `packs` | every pack of the register registered, with its count | yes |
//! | `pauses` | S0's pauses plus the hold on every instance it took | yes |
//! | `instructions` | S0's run-time instructions, none dropped | yes |
//! | `no_dispatch` | no dispatch on a held instance since the start | yes |
//! | `new_keys` | each key the update adds `default`, or `explicit` where the file names it | yes |
//! | `config` | the configuration's SHA-256 is the committed one | yes |
//! | `indexes` | the open noted no incomplete derived index | no |
//! | `one_way` | the one-way watch (INST S6, a later slice) | no |

use super::files;
use super::hold;
use super::host::Host;
use super::journal::S0;
use super::runtime::{ConfigSeen, RuntimeSeen};
use super::state::InstallState;
use actingcommand_contract::outcome::OutcomeEnvelope;
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
    /// The start's own readiness (`ready_ms`), or its failure's code.
    pub(crate) ready: Result<u64, String>,
    /// The start's log.
    pub(crate) log: &'a Path,
    /// S0 of the operation that stopped this Runtime, when there is one.
    pub(crate) s0: Option<&'a S0>,
    /// The hold: its reason and the instances it took.
    pub(crate) hold: Option<&'a hold::PauseIntent>,
    /// The root's configuration document and the SHA-256 of the live file (`sha256:<hex>`).
    pub(crate) config: &'a Value,
    pub(crate) config_sha256: &'a str,
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
        if outcome.code().as_str() == "ledger_open_slow" {
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
    let report: Option<Value> = runtime.start_report(state_root).ok().flatten();
    let mut rows = Vec::new();
    let unread = |id: &'static str, fail_class: bool, why: &str| {
        check(
            id,
            fail_class,
            CheckResult::NotEvaluated,
            json!({"why": why}),
        )
    };

    // ready
    rows.push(match &inputs.ready {
        Ok(ready_ms) => check(
            "ready",
            true,
            CheckResult::Pass,
            json!({"ready_ms": ready_ms}),
        ),
        Err(code) => check("ready", true, CheckResult::Fail, json!({"code": code})),
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
        (None, _) => unread("no_seeding", true, "start_report_unavailable"),
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
        (None, Ok(Some(seen))) => Some(
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
        None => unread("bindings", true, "status_unreadable"),
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
        None => unread("packs", true, "start_report_unavailable"),
    });

    // pauses
    rows.push(match (inputs.s0.and_then(|s0| s0.pauses.as_ref()), &seen) {
        (Some(pauses), Ok(Some(seen))) => {
            let mut expected = hold::pause_reasons_of(pauses);
            if let Some(hold) = inputs.hold {
                for alias in &hold.instances {
                    expected.insert(alias.clone(), hold.reason.clone());
                }
            }
            let actual = seen
                .instances
                .iter()
                .filter_map(|instance| {
                    Some((
                        instance.alias.clone(),
                        instance.pause.as_ref()?.reason.clone(),
                    ))
                })
                .collect::<BTreeMap<_, _>>();
            let global_expected = pauses["global"]["reason"].as_str().map(str::to_owned);
            let global_actual = seen.global_pause.as_ref().map(|pause| pause.reason.clone());
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

    // config
    rows.push(match &inputs.state.config_sha256 {
        Some(committed) => check(
            "config",
            true,
            verdict(committed == inputs.config_sha256),
            json!({"committed": committed, "live": inputs.config_sha256}),
        ),
        None => unread("config", true, "not_committed"),
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
