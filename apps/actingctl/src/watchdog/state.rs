// SPDX-License-Identifier: AGPL-3.0-only

//! `<root>\watchdog\state.json`: the watchdog's operational memory across ticks (the budget,
//! sticky exhaustion, the gap grace and log-on-change). It lives outside the ledger, no Runtime
//! component reads it, and it is parsed tolerantly: unknown fields are ignored, and a document
//! of another schema version is set aside and replaced by a fresh state (rulings #374 M7).

use super::Failure;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

pub(crate) const STATE_SCHEMA: &str = "actingcommand.runtime-watchdog-state.v1";
pub(crate) const STATE_FILE: &str = "state.json";
const MAX_STATE_BYTES: u64 = 1024 * 1024;
/// The state keeps the most recent start records only.
const KEPT_STARTS: usize = 10;

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
    /// The log-on-change key of the last decision.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last_decision: Option<String>,
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct StartRecord {
    pub(crate) at_unix_ms: u64,
    pub(crate) method: String,
    pub(crate) log: String,
    /// `pending` until the start ends: `started`, `start_timeout` or a failure code.
    pub(crate) outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) actingd_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) owner_epoch: Option<String>,
}

/// What `load` found besides the state itself.
#[derive(Debug)]
pub(crate) enum Loaded {
    Fresh,
    Existing,
    /// Another schema version: set aside (by `run-once`) and replaced by a fresh state.
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

pub(crate) fn load(directory: &Path, root: &str) -> Result<(WatchdogState, Loaded), Failure> {
    let path = directory.join(STATE_FILE);
    let unreadable = |detail: String| {
        Failure::misconfigured(
            "watchdog_state_unreadable",
            format!("{}: {detail}", path.display()),
        )
    };
    let mut bytes = Vec::new();
    match fs::File::open(&path) {
        Ok(file) => {
            file.take(MAX_STATE_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| unreadable(error.to_string()))?;
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok((WatchdogState::fresh(root), Loaded::Fresh));
        }
        Err(error) => return Err(unreadable(error.to_string())),
    }
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err(unreadable("larger than 1 MiB".to_owned()));
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| unreadable(error.to_string()))?;
    let schema = value
        .get("schema_version")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if schema != STATE_SCHEMA {
        return Ok((WatchdogState::fresh(root), Loaded::OtherSchema(schema)));
    }
    let state: WatchdogState =
        serde_json::from_value(value).map_err(|error| unreadable(error.to_string()))?;
    Ok((state, Loaded::Existing))
}

/// Moves a state of another schema version aside; nothing is deleted.
pub(crate) fn set_aside(directory: &Path, now_unix_ms: u64) -> Result<PathBuf, Failure> {
    let from = directory.join(STATE_FILE);
    let to = directory.join(format!("{STATE_FILE}.other-schema-{now_unix_ms}"));
    fs::rename(&from, &to).map_err(|error| {
        Failure::misconfigured(
            "watchdog_state_unwritable",
            format!("cannot move {} aside: {error}", from.display()),
        )
    })?;
    Ok(to)
}

/// Written whole to a temporary file and flushed to disk, then renamed over the state, so an
/// unclean shutdown cannot leave a renamed file without its content.
pub(crate) fn save(directory: &Path, state: &WatchdogState) -> Result<(), Failure> {
    let path = directory.join(STATE_FILE);
    let temporary = directory.join(format!("{STATE_FILE}.tmp-{}", std::process::id()));
    let unwritable = |error: std::io::Error| {
        Failure::misconfigured(
            "watchdog_state_unwritable",
            format!("{}: {error}", path.display()),
        )
    };
    let mut bytes = serde_json::to_vec_pretty(state)
        .map_err(|error| Failure::misconfigured("watchdog_state_unwritable", error.to_string()))?;
    bytes.push(b'\n');
    let mut file = fs::File::create(&temporary).map_err(unwritable)?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(unwritable)?;
    drop(file);
    fs::rename(&temporary, &path).map_err(unwritable)
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
            br#"{"schema_version":"actingcommand.runtime-watchdog-state.v1","root":"R","exhausted_since_unix_ms":5,"later_field":{"x":1},"starts":[{"at_unix_ms":4,"outcome":"started","later":true}]}"#,
        )
        .expect("write state");
        let (state, loaded) = load(directory.path(), "R").expect("tolerant state");
        assert!(matches!(loaded, Loaded::Existing));
        assert_eq!(state.exhausted_since_unix_ms, Some(5));
        assert_eq!(state.starts[0].outcome, "started");

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
        assert_eq!(failure.code, "watchdog_state_unreadable");

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
}
