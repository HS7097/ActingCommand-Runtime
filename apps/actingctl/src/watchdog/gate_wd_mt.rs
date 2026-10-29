// SPDX-License-Identifier: AGPL-3.0-only

// WD-MT (Workflow #421, INV-1119; 0.12.28): the watchdog reads a candidate log's modification
// time from the file, not from its directory entry. On 2026-10-29 an adb server that had
// inherited actingd's stdout and stderr kept the err log open; NTFS left the directory entry at
// the log's creation time, and a stop that ended in an error was read as a formal close for 35
// minutes. Here each log on disk carries that stale time (what the directory listing presents),
// and the seam carries the fresh one (what the open file reports).

use super::decide::{self, Decision, Stage, StartProbes};
use super::observe::{self, FileTimes, LogTimes};
use super::state::WatchdogState;
use serde_json::json;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;

/// The stopped owner took the lock at `T0` and wrote its close record at `CLOSED`.
const T0: u64 = 1_800_000_000_000;
const CLOSED: u64 = T0 + 21_000_000;
/// #411 A66: the start window by default; these rows read no live owner.
const START_WINDOW_MS: u64 = 900_000;

/// The seam: the fresh time of each log a test names, by file name, next to the stale time the
/// file on disk carries; any other path is read from the file system.
struct OpenFileTimes {
    fresh: BTreeMap<String, u64>,
}

impl LogTimes for OpenFileTimes {
    fn modified_unix_ms(&self, path: &Path) -> std::io::Result<Option<u64>> {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        match self.fresh.get(&name) {
            Some(modified) => Ok(Some(*modified)),
            None => FileTimes.modified_unix_ms(path),
        }
    }
}

/// actingd's daemon lines of one outcome: its developer line and its `OUTCOME actingd:` line.
fn daemon_lines(level: &str, code: &str, category: &str, causes: serde_json::Value) -> String {
    let envelope = json!({
        "schema_version": "actingcommand.outcome.v1",
        "code": code,
        "category": category,
        "values": {},
        "causes": causes,
    });
    format!("{level} actingd: {code}\nOUTCOME actingd: {envelope}\n")
}

/// The log of a start that ran and then stopped on its own by an error condition:
/// `runtime_stopped`, then the condition (a policy evaluation refused, as on 2026-10-29 11:19).
fn stopped_by_an_error() -> String {
    format!(
        "{}{}{}",
        daemon_lines("INFO", "ledger_opened", "info", json!([])),
        daemon_lines("INFO", "runtime_stopped", "info", json!([])),
        daemon_lines(
            "ERROR",
            "policy_evaluation_rejected",
            "error",
            json!([{
                "code": "policy_evaluation_input_invalid",
                "category": "error",
                "relation": "caused_by",
                "values": {},
            }]),
        ),
    )
}

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

    /// The owner journal: per owner `(epoch, started, closed)` a start record and a close
    /// record; the file is last modified at the last close.
    fn journal(&self, owners: &[(&str, u64, u64)]) {
        let mut text = String::new();
        let mut revision = 0;
        for (index, (epoch, started, closed)) in owners.iter().enumerate() {
            let pid = 100 + u32::try_from(index).expect("owner index");
            for (active, closed_at) in [(true, None), (false, Some(*closed))] {
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
            }
        }
        let path = self.state.join("owner.lock");
        fs::write(&path, text).expect("write the owner journal");
        let last_close = owners.last().map_or(T0, |(_, _, closed)| *closed);
        set_modified(&path, last_close);
    }

    /// A log whose file on disk, and so its directory entry, says `stale`.
    fn log(&self, name: &str, text: &str, stale: u64) {
        let path = self.logs.join(name);
        fs::write(&path, text).expect("write a log");
        set_modified(&path, stale);
    }

    fn observed(&self, times: &dyn LogTimes) -> decide::Observed {
        observe::observe_owner_with(
            &self.state,
            std::slice::from_ref(&self.logs),
            None,
            START_WINDOW_MS,
            times,
        )
        .unwrap_or_else(|failure| panic!("observe: {failure}"))
    }
}

fn set_modified(path: &Path, unix_ms: u64) {
    File::options()
        .write(true)
        .open(path)
        .and_then(|file| file.set_modified(UNIX_EPOCH + Duration::from_millis(unix_ms)))
        .unwrap_or_else(|error| panic!("set the modification time of {}: {error}", path.display()));
}

/// Rows 2-6, then, when they consider a start, rows 7a-7f with free probes and a fresh budget, at
/// the tick 30 seconds after the close.
fn decision(observed: &decide::Observed) -> Decision {
    let state = WatchdogState::default();
    match decide::decide_observed(observed, &state) {
        Stage::Decided(decision) => decision,
        Stage::ConsiderStart => decide::decide_start(
            &StartProbes {
                writer_busy: false,
                selection_changed: false,
                processes: Vec::new(),
                boot_unix_ms: None,
            },
            &state,
            CLOSED + 30_000,
            false,
        ),
    }
}

/// Row 5 and row 6′ read the log's fresh time. The err log of an owner that stopped by an error
/// was created before the owner took the lock and written up to just after its close record; its
/// directory entry still says the creation time. The watchdog starts the Runtime again, the same
/// as when the entry is fresh. The same holds when a later owner (an installer's maintenance
/// owner) closed formally without a log and row 6′ looks back to the stopped one.
#[test]
fn wd_mt_a_log_that_ends_in_an_error_is_restarted_even_when_its_directory_entry_is_stale() {
    let created = T0 - 37_000;
    let closed = CLOSED;
    let last_written = closed + 200;
    let name = "actingd-autostart-20261029-052416.err.log";
    let fresh = OpenFileTimes {
        fresh: BTreeMap::from([(name.to_owned(), last_written)]),
    };

    let stopped = Fixture::new();
    stopped.journal(&[("epoch_stopped", T0, closed)]);
    stopped.log(name, &stopped_by_an_error(), created);
    assert_eq!(
        decision(&stopped.observed(&fresh)),
        Decision::Start,
        "row 5 with the fresh time"
    );
    // The fixture discriminates: the time on disk alone is older than the owner, so a reader
    // of the directory entry would find no terminal outcome and decide a formal close.
    assert_eq!(
        decision(&stopped.observed(&FileTimes)),
        Decision::FormalClose {
            owner_epoch: "epoch_stopped".to_owned()
        },
        "the stale time alone"
    );

    let looked_back = Fixture::new();
    looked_back.journal(&[
        ("epoch_stopped", T0, closed),
        ("epoch_maintenance", closed + 60_000, closed + 61_000),
    ]);
    looked_back.log(name, &stopped_by_an_error(), created);
    assert_eq!(
        decision(&looked_back.observed(&fresh)),
        Decision::Start,
        "row 6′ with the fresh time"
    );
    assert_eq!(
        decision(&looked_back.observed(&FileTimes)),
        Decision::FormalClose {
            owner_epoch: "epoch_maintenance".to_owned()
        },
        "the stale time alone"
    );
}
