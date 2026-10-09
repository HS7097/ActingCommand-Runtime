// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #381 A: the watchdog after a held start stopped (R5′) and after a later formal close
//! that no log covers (R5b). Test plan A-2 G3a, G3c and G3d read fixture state roots with fixed
//! times; H-1 G2 reads the state root a real held start left (I7).

#[allow(dead_code)]
#[path = "../../../../tests/support/held_runtime.rs"]
mod held_runtime;

use super::decide::{self, Decision, Journal, Stage, StartProbes};
use super::observe;
use super::state::{StartRecord, WatchdogState};
use actingcommand_runtime_host::{
    PreparationCheckpoint, PreparationTestAction, RuntimeHost, RuntimeHostConfig,
};
use held_runtime::{HELD_SALT, HeldStart, NoInstances};
use serde_json::json;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;

const T0: u64 = 1_800_000_000_000;
const STOPPED: &str =
    "FATAL actingd: runtime host error install_startup_stopped during install_transition";
const STOPPED_WITH_CAUSE: &str = "FATAL actingd: runtime host error install_startup_stopped during install_transition cause=ledger_failure cause_operation=append_runtime_event";
const HELD_TIMEOUT: &str =
    "FATAL actingd: runtime host error held_timeout during install_transition";
const RELEASE_TIMEOUT: &str =
    "FATAL actingd: runtime host error release_timeout during install_transition";

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
        observe::observe_owner(&self.state, std::slice::from_ref(&self.logs))
            .unwrap_or_else(|failure| panic!("observe: {}: {}", failure.code, failure.detail))
    }

    /// One owner whose start stopped: its journal closed, then its FATAL line.
    fn stopped_owner(line: &str) -> Self {
        let fixture = Self::new();
        fixture.journal(&[("epoch_held", T0, Some(T0 + 10_000))]);
        fixture.log(
            "actingd-1.log",
            &format!("actingd ledger_open total_ms=1\n{line}\n"),
            T0 + 10_050,
        );
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

/// G3a (R5′): the registered top codes of a stopped held start lead to a start through rows
/// 7a-7f, with or without a cause; a configuration FATAL still holds; an exhausted budget still
/// stops the start.
#[test]
fn gate_a_stopped_held_start_is_started_again_and_other_fatals_still_hold() {
    let fresh = WatchdogState::default();
    for line in [STOPPED, STOPPED_WITH_CAUSE, HELD_TIMEOUT, RELEASE_TIMEOUT] {
        assert_eq!(
            decision(&Fixture::stopped_owner(line).observed(), &fresh),
            Decision::Start,
            "{line}"
        );
    }
    let config = "FATAL actingd: adb_install_missing";
    assert_eq!(
        decision(&Fixture::stopped_owner(config).observed(), &fresh).name(),
        "fatal_hold",
        "{config}"
    );
    let exhausted = WatchdogState {
        starts: (1..=3)
            .map(|minute| StartRecord {
                at_unix_ms: T0 + 60_000 - minute * 60_000,
                outcome: "exited_during_startup".to_owned(),
                ..StartRecord::default()
            })
            .collect(),
        ..WatchdogState::default()
    };
    assert_eq!(
        decision(
            &Fixture::stopped_owner(STOPPED_WITH_CAUSE).observed(),
            &exhausted
        )
        .name(),
        "budget_exhausted"
    );
}

/// G3c: every FATAL line format classes by its registered top code alone: the three codes of a
/// stopped held start are restartable; any other FATAL, an install operation included, holds.
#[test]
fn gate_fatal_line_formats_class_by_the_registered_top_code_only() {
    let rows = [
        (STOPPED, "start_due"),
        (STOPPED_WITH_CAUSE, "start_due"),
        (HELD_TIMEOUT, "start_due"),
        (RELEASE_TIMEOUT, "start_due"),
        (
            "FATAL actingd: runtime host error install_preparation_not_authorized during install_transition",
            "fatal_hold",
        ),
        (
            "FATAL actingd: runtime host error ledger_failure during append_runtime_event",
            "fatal_hold",
        ),
        (
            "FATAL actingd: runtime host error agent_ledger_failure during recover_agent_dispatcher",
            "fatal_hold",
        ),
        ("FATAL actingd: usage_invalid", "fatal_hold"),
        (
            "FATAL actingd: resource_package_invalid: C:\\packages\\missing.zip",
            "fatal_hold",
        ),
        (
            "FATAL acforward: Cannot run C:\\root\\runtime",
            "fatal_hold",
        ),
    ];
    for (line, class) in rows {
        let observed = Fixture::stopped_owner(line).observed();
        let decided = match decide::decide_observed(&observed, &WatchdogState::default()) {
            Stage::Decided(decision) => decision.name(),
            Stage::ConsiderStart => "start_due",
        };
        assert_eq!(decided, class, "{line}");
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
        &format!("actingd ledger_open total_ms=1\n{STOPPED_WITH_CAUSE}\n"),
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
        "actingd ledger_open total_ms=1\nactingd shutdown accepted\n",
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
        operation: "append_runtime_event",
    });
    let stopped = start.finish().err().expect("the held start stops");
    drop(installer);
    // actingd's last act: its FATAL line, in the log its starter gave it.
    let fatal_log = logs.join("actingd-held.log");
    fs::write(
        &fatal_log,
        format!("FATAL actingd: {}\n", stopped.complete_message()),
    )
    .expect("write the FATAL log");
    let read_root = || {
        observe::observe_owner(&state, std::slice::from_ref(&logs))
            .unwrap_or_else(|failure| panic!("observe: {}: {}", failure.code, failure.detail))
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
