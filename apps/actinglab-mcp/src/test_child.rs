// SPDX-License-Identifier: AGPL-3.0-only

//! Test helper of `tests/serve_calls.rs` (Workflow #434 LAB-5G4b, `mcp_serve_children_pool`): a
//! made-up `actinglab` for the server's `serve` children. Copied beside a copy of the server as
//! its `actinglab` program, it is started as `actinglab serve --stdio --front mcp` and answers
//! each request line `{"op", "params"}` with the envelope `serve` answers under that option,
//! `{exit_code, line}`, its line naming this process and the request's row. Each process logs to
//! the folder `ACTINGCOMMAND_TEST_CHILD_LOG` names, in a file of its own, one JSON line per event:
//! its start (its words, whether `ACTINGCOMMAND_INITIATOR` is set, the state root variable), each
//! request it reads, and its end at its input's end. One row (`watch`) is held until the file
//! `release` appears in that folder; another (`record draft`) ends the process with a line on
//! stderr and exit 3, without an answer. Never shipped.
//!
//! Workflow #434 LAB-5G4b, second part (`mcp_lab_overflow_rule` and
//! `mcp_capture_records_client_action`): a request whose `instance` is `<key>` is answered with the
//! envelope a test wrote in that folder as `answer-<key>.json` when it is there, whatever its row:
//! the test's made-up line (a line far past the budget with the parts the server keeps, a
//! failure), its exit code and its request id. A file that cannot be read as JSON ends the process
//! with a line on stderr and exit 1.

use serde_json::{Map, Value, json};
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

/// The variable that names the folder this helper logs to.
const LOG_VARIABLE: &str = "ACTINGCOMMAND_TEST_CHILD_LOG";
/// The file whose presence in that folder releases a held request.
const RELEASE: &str = "release";
/// The row whose request is held until released, and the row whose request ends the process.
const HELD: &str = "watch";
const FAILED: &str = "record draft";
/// What the process writes on stderr before it ends without an answer, and its exit code.
const FAILURE_TEXT: &str = "neutral child failure text";
const FAILURE_EXIT: i32 = 3;
/// How often a held request looks for the release, and for how long at most.
const POLL: Duration = Duration::from_millis(20);
const HOLD_LIMIT: Duration = Duration::from_secs(120);
/// The file in that folder whose envelope answers a request of the instance `<key>`:
/// `answer-<key>.json`; and the exit code when it cannot be read.
const CANNED_PREFIX: &str = "answer-";
const CANNED_SUFFIX: &str = ".json";
const CANNED_EXIT: i32 = 1;

fn main() {
    let log = std::env::var_os(LOG_VARIABLE).map(PathBuf::from);
    let log = log.as_deref();
    let pid = std::process::id();
    let words = std::env::args().skip(1).collect::<Vec<_>>();
    record(
        log,
        pid,
        &json!({
            "event": "started",
            "pid": pid,
            "arguments": words,
            "initiator": std::env::var_os("ACTINGCOMMAND_INITIATOR").is_some(),
            "state_root": std::env::var("ACTINGCOMMAND_RUNTIME_STATE_ROOT").ok(),
        }),
    );
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        let request = serde_json::from_str::<Value>(&line).unwrap_or(Value::Null);
        let row = request
            .get("op")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        record(
            log,
            pid,
            &json!({"event": "request", "pid": pid, "op": row}),
        );
        if row == FAILED {
            eprintln!("{FAILURE_TEXT}");
            std::process::exit(FAILURE_EXIT);
        }
        if row == HELD {
            held(log);
        }
        let answer = canned(log, &request).unwrap_or_else(|| {
            let mut answer = Map::new();
            answer.insert("exit_code".to_owned(), json!(0));
            answer.insert("line".to_owned(), json!({"child_pid": pid, "op": row}));
            Value::Object(answer)
        });
        if writeln!(stdout, "{answer}")
            .and_then(|()| stdout.flush())
            .is_err()
        {
            break;
        }
    }
    record(log, pid, &json!({"event": "ended", "pid": pid}));
}

/// The envelope a test wrote for `request`: the file `answer-<instance>.json` in `log`, when the
/// request's parameters name an `instance` and the file is there. A file that cannot be read as
/// JSON ends the process (stderr says why, exit [`CANNED_EXIT`]), so the server answers
/// `lab_process_failed` with that text.
fn canned(log: Option<&Path>, request: &Value) -> Option<Value> {
    let key = request.get("params")?.get("instance")?.as_str()?;
    let path = log?.join(format!("{CANNED_PREFIX}{key}{CANNED_SUFFIX}"));
    if !path.is_file() {
        return None;
    }
    let read = fs::read_to_string(&path)
        .map_err(|error| error.to_string())
        .and_then(|text| serde_json::from_str::<Value>(&text).map_err(|error| error.to_string()));
    match read {
        Ok(envelope) => Some(envelope),
        Err(error) => {
            eprintln!("cannot read {}: {error}", path.display());
            std::process::exit(CANNED_EXIT);
        }
    }
}

/// Waits until the release file appears in `log`, for at most [`HOLD_LIMIT`].
fn held(log: Option<&Path>) {
    let Some(log) = log else {
        return;
    };
    let release = log.join(RELEASE);
    let until = Instant::now() + HOLD_LIMIT;
    while !release.exists() && Instant::now() < until {
        thread::sleep(POLL);
    }
}

/// Appends `event` as one line to this process's file in `log`; a write that fails is said on
/// stderr.
fn record(log: Option<&Path>, pid: u32, event: &Value) {
    let Some(log) = log else {
        return;
    };
    let path = log.join(format!("child-{pid}.jsonl"));
    let written = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| writeln!(file, "{event}"));
    if let Err(error) = written {
        eprintln!("cannot write {}: {error}", path.display());
    }
}
