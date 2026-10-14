// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #381 A: the watchdog after a held start stopped (R5′) and after a later formal close
// that no log covers (R5b). Test plan A-2 G3a, G3c and G3d read fixture state roots with fixed
// times; H-1 G2 reads the state root a real held start left (I7). Workflow #378 A6a: the logs
// hold actingd's daemon lines, and the last `OUTCOME` line, the terminal outcome, decides by its
// category (it was the FATAL line's top code).

use super::decide::{self, Decision, Journal, Stage, StartProbes};
use super::observe;
use super::state::{Recorded, StartRecord, WatchdogState};
use crate::codes::ActingctlCode;
use crate::held_runtime::{HELD_SALT, HeldStart, NoInstances};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_runtime_host::{
    PreparationCheckpoint, PreparationTestAction, RuntimeHost, RuntimeHostConfig,
};
use serde_json::json;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;

const T0: u64 = 1_800_000_000_000;

/// actingd's daemon lines of one outcome: its developer line and its `OUTCOME actingd:` line.
fn daemon_lines(level: &str, envelope: &serde_json::Value) -> String {
    format!(
        "{level} actingd: {}\nOUTCOME actingd: {envelope}\n",
        envelope["code"].as_str().expect("the envelope's code")
    )
}

/// An envelope of `code` and `category` with `values`.
fn envelope(code: &str, category: &str, values: serde_json::Value) -> serde_json::Value {
    json!({
        "schema_version": "actingcommand.outcome.v1",
        "code": code,
        "category": category,
        "values": values,
    })
}

/// The log of a start that opened its ledger and stopped by `condition` (its last daemon
/// lines: `runtime_stopped`, then the condition).
fn stopped_by(condition: &serde_json::Value) -> String {
    let level = match condition["category"].as_str() {
        Some("fatal") => "FATAL",
        _ => "ERROR",
    };
    format!(
        "{}{}{}",
        daemon_lines("INFO", &envelope("ledger_opened", "info", json!({}))),
        daemon_lines("INFO", &envelope("runtime_stopped", "info", json!({}))),
        daemon_lines(level, condition)
    )
}

/// A held start stopped by a latched failure: `install_startup_stopped`, the latched failure as
/// its `caused_by` link when `latched`.
fn held_stop(code: &str, latched: bool) -> String {
    let mut condition = envelope(code, "error", json!({"operation": "install_transition"}));
    if latched {
        condition["causes"] = json!([{
            "code": "ledger_failure",
            "category": "error",
            "relation": "caused_by",
            "values": {"operation": "append_runtime_event"},
        }]);
    }
    stopped_by(&condition)
}

/// A state root and a log directory with fixed modification times.
struct Fixture {
    _temp: TempDir,
    state: PathBuf,
    logs: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().expect("tempdir");
        let state = temp.path().join("state");
        let logs = temp.path().join("logs");
        fs::create_dir_all(&state).expect("state root");
        fs::create_dir_all(&logs).expect("log directory");
        Self {
            _temp: temp,
            state,
            logs,
        }
    }

    /// The owner journal: per owner `(epoch, started, closed)` a start record and, when closed,
    /// a close record; the file is last modified at the last close (or start).
    fn journal(&self, owners: &[(&str, u64, Option<u64>)]) {
        let mut text = String::new();
        let mut revision = 0;
        let mut modified = 0;
        for (index, (epoch, started, closed)) in owners.iter().enumerate() {
            let pid = 100 + u32::try_from(index).expect("owner index");
            let mut records = vec![(true, None)];
            if let Some(closed) = closed {
                records.push((false, Some(*closed)));
            }
            for (active, closed_at) in records {
                revision += 1;
                text.push_str(
                    &json!({
                        "schema_version": "actingcommand.runtime-owner.v2",
                        "revision": revision,
                        "owner_epoch": epoch,
                        "pid": pid,
                        "started_at_unix_ms": started,
                        "active": active,
                        "active_instances": [],
                        "closed_at_unix_ms": closed_at,
                        "resource_disposition": if active { "in_use" } else { "none" },
                    })
                    .to_string(),
                );
                text.push('\n');
                modified = closed_at.unwrap_or(*started);
            }
        }
        let path = self.state.join("owner.lock");
        fs::write(&path, text).expect("write the owner journal");
        set_modified(&path, modified);
    }

    fn log(&self, name: &str, text: &str, modified: u64) {
        let path = self.logs.join(name);
        fs::write(&path, text).expect("write a log");
        set_modified(&path, modified);
    }

    fn observed(&self) -> decide::Observed {
        observe::observe_owner(&self.state, std::slice::from_ref(&self.logs), None)
            .unwrap_or_else(|failure| panic!("observe: {failure}"))
    }

    /// One owner whose start stopped: its journal closed, then its log's last daemon lines.
    fn stopped_owner(log: &str) -> Self {
        let fixture = Self::new();
        fixture.journal(&[("epoch_held", T0, Some(T0 + 10_000))]);
        fixture.log("actingd-1.log", log, T0 + 10_050);
        fixture
    }
}

fn set_modified(path: &Path, unix_ms: u64) {
    File::options()
        .write(true)
        .open(path)
        .and_then(|file| file.set_modified(UNIX_EPOCH + Duration::from_millis(unix_ms)))
        .unwrap_or_else(|error| panic!("set the modification time of {}: {error}", path.display()));
}

fn free_probes() -> StartProbes {
    StartProbes {
        writer_busy: false,
        selection_changed: false,
        processes: Vec::new(),
        boot_unix_ms: None,
    }
}

/// Rows 2-6, then, when they consider a start, rows 7a-7f with free probes.
fn decision(observed: &decide::Observed, state: &WatchdogState) -> Decision {
    match decide::decide_observed(observed, state) {
        Stage::Decided(decision) => decision,
        Stage::ConsiderStart => decide::decide_start(&free_probes(), state, T0 + 60_000, false),
    }
}

/// G3a (R5′): a stopped held start leads to a start through rows 7a-7f, with or without a
/// cause; a fatal terminal outcome (a damaged installation) still holds, and (#672 review M1) an
/// accepted shutdown of a held start, a requested stop whose terminal outcome is
/// `runtime_stopped`, is never started again; an exhausted budget still stops the start.
#[test]
fn gate_a_stopped_held_start_is_started_again_and_other_fatals_still_hold() {
    let fresh = WatchdogState::default();
    for log in [
        held_stop("install_startup_stopped", false),
        held_stop("install_startup_stopped", true),
        held_stop("held_timeout", false),
        held_stop("release_timeout", false),
    ] {
        assert_eq!(
            decision(&Fixture::stopped_owner(&log).observed(), &fresh),
            Decision::Start,
            "{log}"
        );
    }
    let damaged = stopped_by(&envelope("adb_install_missing", "fatal", json!({})));
    assert_eq!(
        decision(&Fixture::stopped_owner(&damaged).observed(), &fresh).code(),
        ActingctlCode::WatchdogFatalHold.code(),
        "{damaged}"
    );
    let shut_down = daemon_lines(
        "INFO",
        &json!({
            "schema_version": "actingcommand.outcome.v1",
            "code": "runtime_stopped",
            "category": "info",
            "values": {},
            "causes": [{
                "code": "install_startup_shut_down",
                "category": "error",
                "relation": "caused_by",
                "values": {"operation": "install_transition"},
            }],
        }),
    );
    assert_eq!(
        decision(&Fixture::stopped_owner(&shut_down).observed(), &fresh),
        Decision::FormalClose {
            owner_epoch: "epoch_held".to_owned()
        },
        "{shut_down}"
    );
    let exhausted = WatchdogState {
        starts: (1..=3)
            .map(|minute| StartRecord {
                at_unix_ms: T0 + 60_000 - minute * 60_000,
                start_result: Recorded::of(&Outcome::new(ActingctlCode::ExitedDuringStartup)),
                ..StartRecord::default()
            })
            .collect(),
        ..WatchdogState::default()
    };
    assert_eq!(
        decision(
            &Fixture::stopped_owner(&held_stop("install_startup_stopped", true)).observed(),
            &exhausted
        )
        .code(),
        ActingctlCode::WatchdogBudgetExhausted.code()
    );
}

/// G3c (Workflow #378 A6a, model section 7.5): the terminal outcome classes by its category
/// alone, whatever its code, operation or causes: an error considers a start (a stopped held
/// start, a lost start race, a refused start), a fatal one holds, acforward's included; the
/// last `OUTCOME` line counts, not an earlier one; a log without one leaves the journal rows to
/// decide.
#[test]
fn gate_fatal_line_formats_class_by_the_registered_top_code_only() {
    let start_due = ActingctlCode::WatchdogStartDue.code();
    let fatal_hold = ActingctlCode::WatchdogFatalHold.code();
    let formal_close = ActingctlCode::WatchdogFormalClose.code();
    let fatal_then_stopped = format!(
        "{}{}",
        daemon_lines(
            "FATAL",
            &envelope("ledger_write_failed", "fatal", json!({}))
        ),
        daemon_lines("INFO", &envelope("runtime_stopped", "info", json!({})))
    );
    let rows = [
        (held_stop("install_startup_stopped", true), start_due),
        (held_stop("held_timeout", false), start_due),
        (
            stopped_by(&envelope(
                "owner_conflict",
                "error",
                json!({"operation": "acquire_owner_file"}),
            )),
            start_due,
        ),
        (
            stopped_by(&envelope("usage_invalid", "error", json!({}))),
            start_due,
        ),
        (
            stopped_by(&envelope("ledger_write_failed", "fatal", json!({}))),
            fatal_hold,
        ),
        (
            stopped_by(&envelope("adb_install_mismatch", "fatal", json!({}))),
            fatal_hold,
        ),
        (
            format!(
                "OUTCOME acforward: {}\n",
                envelope("acforward_runtime_missing", "fatal", json!({}))
            ),
            fatal_hold,
        ),
        (fatal_then_stopped, formal_close),
        ("FATAL actingd: usage_invalid\n".to_owned(), formal_close),
    ];
    for (log, class) in rows {
        let observed = Fixture::stopped_owner(&log).observed();
        let decided = match decide::decide_observed(&observed, &WatchdogState::default()) {
            Stage::Decided(decision) => decision.code(),
            Stage::ConsiderStart => Decision::Start.code(),
        };
        assert_eq!(decided, class, "{log}");
    }
}

/// G3d (R5b): a formal close that no log covers looks back to the last owner whose epoch a log
/// covers and applies the FATAL rule to it. A stopped held start before an installer's closed
/// maintenance owner is started again; an owner that closed formally keeps the formal close.
#[test]
fn gate_an_unlogged_formal_close_looks_back_to_the_last_logged_owner() {
    let owners = [
        ("epoch_held", T0, Some(T0 + 10_000)),
        ("epoch_maintenance", T0 + 20_000, Some(T0 + 21_000)),
    ];
    let stopped = Fixture::new();
    stopped.journal(&owners);
    stopped.log(
        "actingd-1.log",
        &held_stop("install_startup_stopped", true),
        T0 + 10_050,
    );
    assert_eq!(
        decision(&stopped.observed(), &WatchdogState::default()),
        Decision::Start
    );

    let formal = Fixture::new();
    formal.journal(&owners);
    formal.log(
        "actingd-1.log",
        &format!(
            "{}{}",
            daemon_lines("INFO", &envelope("ledger_opened", "info", json!({}))),
            daemon_lines("INFO", &envelope("runtime_stopped", "info", json!({})))
        ),
        T0 + 10_050,
    );
    assert_eq!(
        decision(&formal.observed(), &WatchdogState::default()),
        Decision::FormalClose {
            owner_epoch: "epoch_maintenance".to_owned()
        }
    );
}

/// H-1 G2 (I7): the state root a held start leaves after a latched failure stopped it, read by
/// the watchdog: a start (R5′). Then a later owner closes formally without a log, as an
/// installer's maintenance owner, and the tick comes seconds later: still a start (R5b).
#[test]
fn gate_the_state_root_of_a_stopped_held_start_leads_the_watchdog_to_a_start() {
    let base = TempDir::new().expect("tempdir");
    let state = base.path().join("state");
    let logs = base.path().join("logs");
    fs::create_dir_all(&logs).expect("log directory");
    let start = HeldStart::spawn(&state);
    start.reached(PreparationCheckpoint::Held);
    let installer = start.release(&state);
    start.reached(PreparationCheckpoint::Preparing);
    start.go_on(PreparationTestAction::LatchLedgerFailure {
        operation: actingcommand_runtime_host::HostLocation::AppendRuntimeEvent.location(),
    });
    let stopped = start.finish().err().expect("the held start stops");
    drop(installer);
    // actingd's last act: `runtime_stopped` and the stop's daemon lines, in the log its starter
    // gave it (Workflow #378 A6a).
    let fatal_log = logs.join("actingd-held.log");
    let condition = stopped.daemon_outcome();
    fs::write(
        &fatal_log,
        format!(
            "{}ERROR actingd: {} {}\nOUTCOME actingd: {}\n",
            daemon_lines("INFO", &envelope("runtime_stopped", "info", json!({}))),
            condition.code(),
            stopped.complete_message(),
            serde_json::to_string(&condition.envelope(Detail::Full)).expect("the envelope"),
        ),
    )
    .expect("write the stop's log");
    let read_root = || {
        observe::observe_owner(&state, std::slice::from_ref(&logs), None)
            .unwrap_or_else(|failure| panic!("observe: {failure}"))
    };
    assert_eq!(
        decision(&read_root(), &WatchdogState::default()),
        Decision::Start,
        "R5′ after the stopped held start"
    );

    let Journal::Record {
        record: held_owner, ..
    } = read_root().journal
    else {
        panic!("the held owner's journal record");
    };
    RuntimeHost::start(
        RuntimeHostConfig::new(&state, HELD_SALT),
        Arc::new(NoInstances),
    )
    .expect("a later owner starts")
    .close()
    .expect("and closes formally");
    let Journal::Record {
        record: later_owner,
        ..
    } = read_root().journal
    else {
        panic!("the later owner's journal record");
    };
    set_modified(&fatal_log, held_owner.started_at_unix_ms);
    set_modified(
        &state.join("owner.lock"),
        later_owner
            .closed_at_unix_ms
            .expect("the later owner closed")
            + 3_000,
    );
    assert_eq!(
        decision(&read_root(), &WatchdogState::default()),
        Decision::Start,
        "R5b after an unlogged formal close"
    );
}
