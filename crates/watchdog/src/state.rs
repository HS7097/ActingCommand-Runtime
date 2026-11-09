// SPDX-License-Identifier: AGPL-3.0-only

//! `<root>\watchdog\state.json`: the watchdog's operational memory across ticks (the budget,
//! sticky exhaustion, the gap grace and log-on-change). It lives outside the ledger, no Runtime
//! component reads it, and it is parsed tolerantly: unknown fields are ignored, and a document
//! of another schema version is set aside and replaced by a fresh state (rulings #374 M7).
//!
//! Version 2 (Workflow #378 A5, model v2.2 §7.5 and L6) keeps results as `{code, values}`: the
//! last decision (`last_decision`) and each start's result (`starts[].start_result`). A version
//! 1 document is another schema: it is set aside and the budget restarts (no old-version
//! compatibility, Alice 10-11).

use super::{io_failure, json_failure};
use crate::codes::ActingctlCode;
use actingcommand_contract::outcome::{Code, CodeStr, IoOp, Outcome};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

pub(crate) const STATE_SCHEMA: &str = "actingcommand.runtime-watchdog-state.v2";
pub(crate) const STATE_FILE: &str = "state.json";
const MAX_STATE_BYTES: u64 = 1024 * 1024;
/// The state keeps the most recent start records only.
const KEPT_STARTS: usize = 10;

/// A result as the state keeps it: its code and its values (model §7.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Recorded {
    pub(crate) code: CodeStr,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub(crate) values: Map<String, Value>,
}

impl Recorded {
    pub(crate) fn of(outcome: &Outcome) -> Self {
        Self {
            code: outcome.code().clone(),
            values: outcome.values().clone(),
        }
    }

    /// Whether the kept result carries `code`.
    pub(crate) fn is(&self, code: impl Into<Code>) -> bool {
        self.code == code.into()
    }

    /// The kept result as a received outcome, to travel as a link: its code, the category the
    /// catalog this build embeds gives it, and its values. A code the catalog does not list
    /// stays visible as `outcome_relay_unregistered`.
    pub(crate) fn outcome(&self) -> Outcome {
        let relayed = Outcome::relayed(self.code.as_str());
        if relayed.code() != &self.code {
            return relayed;
        }
        let mut body = Map::new();
        body.insert(
            "code".to_owned(),
            Value::String(self.code.as_str().to_owned()),
        );
        body.insert(
            "category".to_owned(),
            Value::String(relayed.category().as_str().to_owned()),
        );
        body.insert("values".to_owned(), Value::Object(self.values.clone()));
        serde_json::from_value(Value::Object(body)).unwrap_or(relayed)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct WatchdogState {
    pub(crate) schema_version: String,
    pub(crate) root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last_tick_unix_ms: Option<u64>,
    /// The end of the previous task tick; the gap grace is measured from it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last_task_tick_end_unix_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) grace_until_unix_ms: Option<u64>,
    /// The last decision's code and values; log-on-change compares them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last_decision: Option<Recorded>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) decision_since_unix_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) exhausted_since_unix_ms: Option<u64>,
    /// The recorded start of the last live owner the watchdog did not start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) formal_start_at_unix_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) process_present_since_unix_ms: Option<u64>,
    pub(crate) starts: Vec<StartRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct StartRecord {
    pub(crate) at_unix_ms: u64,
    pub(crate) method: String,
    pub(crate) log: String,
    /// `watchdog_start_pending` until the start ends: `watchdog_started_runtime`,
    /// `start_timeout` or the failure's code, each with its values.
    pub(crate) start_result: Recorded,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) actingd_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) owner_epoch: Option<String>,
}

impl Default for StartRecord {
    /// A start that has not ended yet.
    fn default() -> Self {
        Self {
            at_unix_ms: 0,
            method: String::new(),
            log: String::new(),
            start_result: Recorded::of(&Outcome::new(ActingctlCode::WatchdogStartPending)),
            generation: None,
            actingd_pid: None,
            owner_epoch: None,
        }
    }
}

/// What `load` found besides the state itself.
#[derive(Debug)]
pub(crate) enum Loaded {
    Fresh,
    Existing,
    /// Another schema version (the version 1 included): set aside (by `run-once` and
    /// `install`) and replaced by a fresh state.
    OtherSchema(String),
}

impl WatchdogState {
    pub(crate) fn fresh(root: &str) -> Self {
        Self {
            schema_version: STATE_SCHEMA.to_owned(),
            root: root.to_owned(),
            ..Self::default()
        }
    }

    pub(crate) fn push_start(&mut self, start: StartRecord) {
        self.starts.push(start);
        let excess = self.starts.len().saturating_sub(KEPT_STARTS);
        self.starts = self.starts.split_off(excess);
    }

    /// The most recent start after the last observed formal start.
    pub(crate) fn last_start(&self) -> Option<&StartRecord> {
        let floor = self.formal_start_at_unix_ms.unwrap_or(0);
        self.starts.last().filter(|start| start.at_unix_ms > floor)
    }
}

/// Reads the state; a missing file is a fresh state, another schema version a fresh state
/// with [`Loaded::OtherSchema`], anything unreadable `watchdog_state_unreadable` (`path`).
pub(crate) fn load(directory: &Path, root: &str) -> Result<(WatchdogState, Loaded), Outcome> {
    let path = directory.join(STATE_FILE);
    let unreadable = |error: &std::io::Error, op: IoOp| {
        io_failure(ActingctlCode::WatchdogStateUnreadable, &path, error, op)
    };
    let mut bytes = Vec::new();
    match fs::File::open(&path) {
        Ok(file) => {
            file.take(MAX_STATE_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| unreadable(&error, IoOp::Read))?;
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok((WatchdogState::fresh(root), Loaded::Fresh));
        }
        Err(error) => return Err(unreadable(&error, IoOp::Open)),
    }
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err(Outcome::new(ActingctlCode::WatchdogStateUnreadable)
            .path(&path)
            .limit_bytes(MAX_STATE_BYTES));
    }
    let malformed = |error: &serde_json::Error| {
        Outcome::new(ActingctlCode::WatchdogStateUnreadable)
            .path(&path)
            .caused_by(json_failure(error))
    };
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| malformed(&error))?;
    let schema = value
        .get("schema_version")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if schema != STATE_SCHEMA {
        return Ok((WatchdogState::fresh(root), Loaded::OtherSchema(schema)));
    }
    let state: WatchdogState = serde_json::from_value(value).map_err(|error| malformed(&error))?;
    Ok((state, Loaded::Existing))
}

/// Moves a state of another schema version aside; nothing is deleted.
pub(crate) fn set_aside(directory: &Path, now_unix_ms: u64) -> Result<PathBuf, Outcome> {
    let from = directory.join(STATE_FILE);
    let to = directory.join(format!("{STATE_FILE}.other-schema-{now_unix_ms}"));
    move_aside(&from, &to)?;
    Ok(to)
}

/// Renames a state file aside (`watchdog_state_unwritable` with `path` and `to_path`).
pub(crate) fn move_aside(from: &Path, to: &Path) -> Result<(), Outcome> {
    fs::rename(from, to).map_err(|error| {
        io_failure(
            ActingctlCode::WatchdogStateUnwritable,
            from,
            &error,
            IoOp::Rename,
        )
        .to_path(to)
    })
}

/// Written whole to a temporary file and flushed to disk, then renamed over the state, so an
/// unclean shutdown cannot leave a renamed file without its content.
pub(crate) fn save(directory: &Path, state: &WatchdogState) -> Result<(), Outcome> {
    let path = directory.join(STATE_FILE);
    let temporary = directory.join(format!("{STATE_FILE}.tmp-{}", std::process::id()));
    let unwritable = |at: &Path, error: &std::io::Error, op: IoOp| {
        io_failure(ActingctlCode::WatchdogStateUnwritable, at, error, op)
    };
    let mut bytes = serde_json::to_vec_pretty(state).map_err(|error| {
        Outcome::new(ActingctlCode::WatchdogStateUnwritable)
            .path(&path)
            .caused_by(json_failure(&error))
    })?;
    bytes.push(b'\n');
    let mut file = fs::File::create(&temporary)
        .map_err(|error| unwritable(&temporary, &error, IoOp::Create))?;
    file.write_all(&bytes)
        .map_err(|error| unwritable(&temporary, &error, IoOp::Write))?;
    file.sync_all()
        .map_err(|error| unwritable(&temporary, &error, IoOp::Sync))?;
    drop(file);
    fs::rename(&temporary, &path).map_err(|error| unwritable(&path, &error, IoOp::Rename))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_reads_tolerantly_and_sets_other_schemas_aside() {
        let directory = tempfile::tempdir().expect("tempdir");
        let (fresh, loaded) = load(directory.path(), "R").expect("absent state");
        assert!(matches!(loaded, Loaded::Fresh));
        assert_eq!(fresh.schema_version, STATE_SCHEMA);

        // Unknown fields, such as a later release's, are ignored.
        fs::write(
            directory.path().join(STATE_FILE),
            br#"{"schema_version":"actingcommand.runtime-watchdog-state.v2","root":"R","exhausted_since_unix_ms":5,"later_field":{"x":1},"starts":[{"at_unix_ms":4,"start_result":{"code":"watchdog_started_runtime"},"later":true}]}"#,
        )
        .expect("write state");
        let (state, loaded) = load(directory.path(), "R").expect("tolerant state");
        assert!(matches!(loaded, Loaded::Existing));
        assert_eq!(state.exhausted_since_unix_ms, Some(5));
        assert!(
            state.starts[0]
                .start_result
                .is(ActingctlCode::WatchdogStartedRuntime)
        );

        // Another schema version is replaced by a fresh state, never a misconfiguration.
        fs::write(
            directory.path().join(STATE_FILE),
            br#"{"schema_version":"actingcommand.runtime-watchdog-state.v9"}"#,
        )
        .expect("write state");
        let (state, loaded) = load(directory.path(), "R").expect("other schema");
        assert!(matches!(loaded, Loaded::OtherSchema(ref schema) if schema.ends_with("v9")));
        assert_eq!(state, WatchdogState::fresh("R"));
        let aside = set_aside(directory.path(), 7).expect("set aside");
        assert!(aside.is_file());
        assert!(!directory.path().join(STATE_FILE).exists());

        // A corrupt document is a misconfiguration: the watchdog never guesses.
        fs::write(directory.path().join(STATE_FILE), b"{not json").expect("write state");
        let failure = load(directory.path(), "R").expect_err("corrupt state");
        assert!(failure.is(ActingctlCode::WatchdogStateUnreadable));

        let mut state = WatchdogState::fresh("R");
        for at_unix_ms in 0..12 {
            state.push_start(StartRecord {
                at_unix_ms,
                ..StartRecord::default()
            });
        }
        assert_eq!(state.starts.len(), 10);
        assert_eq!(state.starts[0].at_unix_ms, 2);
        save(directory.path(), &state).expect("save state");
        assert_eq!(load(directory.path(), "R").expect("saved state").0, state);
    }

    /// Workflow #378 A5 (model v2.2 §7.5, L6): state.json v2 keeps the last decision and each
    /// start's result as `{code, values}` and reads back what it wrote; a version 1 document
    /// is another schema, set aside, and the state (the budget with it) starts fresh.
    #[test]
    fn state_v2_round_trips_results_and_sets_a_v1_document_aside() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut state = WatchdogState::fresh("R");
        let alive = Outcome::new(ActingctlCode::WatchdogAlive)
            .owner_epoch("epoch_a")
            .pid(41_u32);
        state.last_decision = Some(Recorded::of(&alive));
        let failed = Outcome::new(ActingctlCode::ExitedDuringStartup)
            .log(r"C:\Install\watchdog\actingd-1.log")
            .exit_code(1);
        state.push_start(StartRecord {
            at_unix_ms: 9,
            method: "wmi".to_owned(),
            start_result: Recorded::of(&failed),
            ..StartRecord::default()
        });
        state.push_start(StartRecord {
            at_unix_ms: 10,
            ..StartRecord::default()
        });
        save(directory.path(), &state).expect("save state");

        let saved: Value = serde_json::from_slice(
            &fs::read(directory.path().join(STATE_FILE)).expect("read state"),
        )
        .expect("state JSON");
        assert_eq!(
            saved["schema_version"],
            "actingcommand.runtime-watchdog-state.v2"
        );
        assert_eq!(
            saved["last_decision"],
            serde_json::json!({ "code": "watchdog_alive", "values": { "owner_epoch": "epoch_a", "pid": 41 } })
        );
        assert_eq!(
            saved["starts"][0]["start_result"]["code"],
            "exited_during_startup"
        );
        assert_eq!(saved["starts"][0]["start_result"]["values"]["exit_code"], 1);
        assert_eq!(
            saved["starts"][1]["start_result"]["code"],
            "watchdog_start_pending"
        );
        assert!(saved["starts"][0].get("outcome").is_none());

        let (loaded, kind) = load(directory.path(), "R").expect("saved state");
        assert!(matches!(kind, Loaded::Existing));
        assert_eq!(loaded, state);
        assert!(
            loaded
                .last_decision
                .as_ref()
                .is_some_and(|last| last.is(ActingctlCode::WatchdogAlive))
        );
        assert!(
            loaded.starts[0]
                .start_result
                .is(ActingctlCode::ExitedDuringStartup)
        );
        assert!(
            loaded.starts[1]
                .start_result
                .is(ActingctlCode::WatchdogStartPending)
        );

        // A version 1 document (a key string and `starts[].outcome`) is set aside.
        fs::write(
            directory.path().join(STATE_FILE),
            br#"{"schema_version":"actingcommand.runtime-watchdog-state.v1","root":"R","last_decision":"alive:epoch_a","exhausted_since_unix_ms":5,"starts":[{"at_unix_ms":4,"outcome":"started"}]}"#,
        )
        .expect("write a v1 state");
        let (fresh, kind) = load(directory.path(), "R").expect("v1 state");
        assert!(
            matches!(kind, Loaded::OtherSchema(ref schema) if schema == "actingcommand.runtime-watchdog-state.v1")
        );
        assert_eq!(fresh, WatchdogState::fresh("R"));
        let aside = set_aside(directory.path(), 11).expect("set the v1 state aside");
        assert!(aside.is_file());
        assert!(!directory.path().join(STATE_FILE).exists());
    }
}
