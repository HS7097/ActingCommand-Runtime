// SPDX-License-Identifier: AGPL-3.0-only

//! The run directory's files (§5.9): `events.jsonl` (state changes, gates, sequence records,
//! closes, faults), the opportunity files, `progress.jsonl`, `heartbeat.json`, `closing.json`,
//! the cell summaries and `summary.json`. The bench writes nothing to its standard streams
//! (§3.10 item 3); every writer here is a leaf lock.
//!
//! #402 INT-1, the flush rule: every appended JSONL line is handed to the operating system as
//! soon as it is complete (a `LineWriter`: one write call per line, no `FlushFileBuffers`), so a
//! crash of the bench leaves every line written so far in the file (the system cache writes it
//! out). Only `closing`, `close_unconfirmed`, `self_exit` and [`Journal::append_durable`] also
//! wait for the disk (`FlushFileBuffers`, §3.5.2). A line costs one buffered write call of a few
//! microseconds, outside every timed span (the native calls, the publish); no line waits for
//! the disk. Whole files (`write_json`) are written beside the target under a name of their own
//! per call, synced and moved over it, one at a time.

use crate::clock::{Clock, Micros};
use crate::hub::{RecordSink, SeqRecord};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{LineWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A journal write failed. The plan does not stop the run for it (§3.11.2 B lists what stops a
/// run, and a journal write is not among them): the failure is kept, listed in `summary.json`
/// (`journal_failures`) and decides the settlement (see [`Journal::failures`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalError(pub String);

impl std::fmt::Display for JournalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

struct Appender {
    writer: LineWriter<File>,
}

impl Appender {
    fn open(path: &Path) -> Result<Self, JournalError> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|error| JournalError(format!("open {}: {error}", path.display())))?;
        Ok(Self {
            writer: LineWriter::new(file),
        })
    }

    fn line(&mut self, value: &Value, durable: bool) -> Result<(), JournalError> {
        let mut text = serde_json::to_string(value)
            .map_err(|error| JournalError(format!("encode a journal line: {error}")))?;
        text.push('\n');
        self.writer
            .write_all(text.as_bytes())
            .map_err(|error| JournalError(format!("append a journal line: {error}")))?;
        if durable {
            self.writer
                .flush()
                .map_err(|error| JournalError(format!("flush the journal: {error}")))?;
            // `sync_all` is FlushFileBuffers on Windows (§3.5.2: `closing` is on disk before the
            // close call).
            self.writer
                .get_ref()
                .sync_all()
                .map_err(|error| JournalError(format!("sync the journal: {error}")))?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), JournalError> {
        self.writer
            .flush()
            .map_err(|error| JournalError(format!("flush the journal: {error}")))
    }
}

/// The run directory and its writers.
pub struct Journal {
    root: PathBuf,
    clock: Arc<dyn Clock>,
    events: Mutex<Appender>,
    files: Mutex<BTreeMap<String, Appender>>,
    failures: Mutex<Vec<String>>,
    /// #402 INT-1: one whole-file write at a time (a leaf lock, held only while one file is
    /// written and moved).
    whole_files: Mutex<()>,
    /// #402 INT-1: the staged file of every `write_json` call has a name of its own.
    staged: AtomicU64,
}

impl Journal {
    pub fn open(root: &Path, clock: Arc<dyn Clock>) -> Result<Arc<Self>, JournalError> {
        fs::create_dir_all(root)
            .map_err(|error| JournalError(format!("create {}: {error}", root.display())))?;
        let events = Appender::open(&root.join("events.jsonl"))?;
        Ok(Arc::new(Self {
            root: root.to_path_buf(),
            clock,
            events: Mutex::new(events),
            files: Mutex::new(BTreeMap::new()),
            failures: Mutex::new(Vec::new()),
            whole_files: Mutex::new(()),
            staged: AtomicU64::new(1),
        }))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn stamp(&self, kind: &str, mut body: Value) -> Value {
        let now = self.clock.now_us();
        let unix = self.clock.unix_ms();
        if let Value::Object(map) = &mut body {
            map.insert("t_us".to_owned(), json!(now));
            map.insert("unix_ms".to_owned(), json!(unix));
            map.insert("type".to_owned(), json!(kind));
            body
        } else {
            json!({ "t_us": now, "unix_ms": unix, "type": kind, "value": body })
        }
    }

    fn note_failure(&self, error: &JournalError) {
        lock(&self.failures).push(error.0.clone());
    }

    /// The journal writes that failed. A failed write does not stop the run (the plan has no
    /// such stop, §3.11.2 B): the run goes on, `summary.json` lists every failure in
    /// `journal_failures`, and the settlement adds `internal_invariant` (exit code 30) to the
    /// results, so a run with a lost record never ends `completed` (§3.11.3 has no row of its
    /// own for it; 30 is the bench-internal result).
    pub fn failures(&self) -> Vec<String> {
        lock(&self.failures).clone()
    }

    /// Appends an event to `events.jsonl`.
    pub fn event(&self, kind: &str, body: Value) {
        let line = self.stamp(kind, body);
        if let Err(error) = lock(&self.events).line(&line, false) {
            self.note_failure(&error);
        }
    }

    /// Appends an event and puts it on disk before returning (`closing`, `close_unconfirmed`).
    pub fn event_durable(&self, kind: &str, body: Value) -> Result<(), JournalError> {
        let line = self.stamp(kind, body);
        let result = lock(&self.events).line(&line, true);
        if let Err(error) = &result {
            self.note_failure(error);
        }
        result
    }

    /// Appends a line to the JSONL file `name` (an opportunity, progress or metrics file).
    pub fn append(&self, name: &str, value: &Value) {
        let mut files = lock(&self.files);
        if !files.contains_key(name) {
            match Appender::open(&self.root.join(name)) {
                Ok(appender) => {
                    files.insert(name.to_owned(), appender);
                }
                Err(error) => {
                    drop(files);
                    self.note_failure(&error);
                    return;
                }
            }
        }
        if let Some(appender) = files.get_mut(name)
            && let Err(error) = appender.line(value, false)
        {
            drop(files);
            self.note_failure(&error);
        }
    }

    /// Appends a line to `name` and puts it on disk.
    pub fn append_durable(&self, name: &str, value: &Value) -> Result<(), JournalError> {
        self.append(name, value);
        let mut files = lock(&self.files);
        let result = match files.get_mut(name) {
            Some(appender) => appender.line(&json!({ "kind": "sync" }), true),
            None => Err(JournalError(format!("{name} is not open"))),
        };
        drop(files);
        if let Err(error) = &result {
            self.note_failure(error);
        }
        result
    }

    /// Writes a whole JSON file and puts it on disk (`heartbeat.json`, `closing.json`,
    /// `summary*.json`, `header.json`): written beside it under a name of its own
    /// ([`Self::staged_name`]), synced, then moved over it. #402 INT-1: two writers of the same
    /// file never share a staged file, and the writes are one at a time (D2's exit 30 was two
    /// closes writing `closing.json.partial` at once).
    pub fn write_json(&self, name: &str, value: &impl Serialize) -> Result<(), JournalError> {
        let text = serde_json::to_vec_pretty(value)
            .map_err(|error| JournalError(format!("encode {name}: {error}")))?;
        let target = self.root.join(name);
        let staged = self.root.join(self.staged_name(name));
        let result = {
            let _one_at_a_time = lock(&self.whole_files);
            write_staged(&staged, &target, &text)
        };
        if let Err(error) = &result {
            self.note_failure(error);
        }
        result
    }

    /// #402 INT-1: the staged file of one `write_json` call: `<name>.<pid>-<n>.partial`, `n`
    /// counting the calls of this journal.
    pub fn staged_name(&self, name: &str) -> String {
        let call = self.staged.fetch_add(1, Ordering::Relaxed);
        format!("{name}.{}-{call}.partial", std::process::id())
    }

    /// Flushes every buffered writer.
    pub fn flush(&self) {
        if let Err(error) = lock(&self.events).flush() {
            self.note_failure(&error);
        }
        let mut files = lock(&self.files);
        let mut failed = Vec::new();
        for appender in files.values_mut() {
            if let Err(error) = appender.flush() {
                failed.push(error);
            }
        }
        drop(files);
        for error in failed {
            self.note_failure(&error);
        }
    }

    pub fn now_us(&self) -> Micros {
        self.clock.now_us()
    }

    /// #402 C2b: appends raw text to `name` (the CSV coverage list), creating the folders it
    /// needs; a failure stops the run like any journal failure.
    pub fn append_text(&self, name: &str, text: &str) {
        let path = self.root.join(name);
        let written = path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| OpenOptions::new().create(true).append(true).open(&path))
            .and_then(|mut file| file.write_all(text.as_bytes()));
        if let Err(error) = written {
            self.note_failure(&JournalError(format!(
                "append to {}: {error}",
                path.display()
            )));
        }
    }

    /// #402 C2b: writes a whole binary file under the run directory (C8's saved frames).
    pub fn write_bytes(&self, name: &str, bytes: &[u8]) -> Result<(), JournalError> {
        let path = self.root.join(name);
        let written = path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&path, bytes));
        written.map_err(|error| {
            let error = JournalError(format!("write {}: {error}", path.display()));
            self.note_failure(&error);
            error
        })
    }
}

fn write_staged(staged: &Path, target: &Path, text: &[u8]) -> Result<(), JournalError> {
    let mut file = File::create(staged)
        .map_err(|error| JournalError(format!("create {}: {error}", staged.display())))?;
    file.write_all(text)
        .map_err(|error| JournalError(format!("write {}: {error}", staged.display())))?;
    file.sync_all()
        .map_err(|error| JournalError(format!("sync {}: {error}", staged.display())))?;
    drop(file);
    fs::rename(staged, target).map_err(|error| {
        JournalError(format!(
            "move {} over {}: {error}",
            staged.display(),
            target.display()
        ))
    })
}

/// The hub's records go to `events.jsonl` with the analysis schema's event types
/// (`rd-1031/exp402/analysis/schema.md` §8). The sink is called under the scheduler lock, so
/// these lines are in sequence order.
pub struct JournalRecords {
    journal: Arc<Journal>,
    instances: Vec<String>,
    gate_seqs: Mutex<BTreeMap<(Option<usize>, String), u64>>,
}

impl JournalRecords {
    pub fn new(journal: Arc<Journal>, instances: Vec<String>) -> Self {
        Self {
            journal,
            instances,
            gate_seqs: Mutex::new(BTreeMap::new()),
        }
    }

    fn name(&self, instance: Option<usize>) -> Value {
        instance
            .and_then(|index| self.instances.get(index))
            .map_or(Value::Null, |name| json!(name))
    }
}

impl RecordSink for JournalRecords {
    fn record(&self, record: &SeqRecord) {
        use crate::hub::RecordKind;
        let instance = self.name(record.instance);
        let seq = record.seq;
        let at = record.t_us;
        let (kind, mut body) = match &record.kind {
            RecordKind::Grant {
                path,
                generation,
                owner,
            } => (
                "grant",
                json!({ "seq": seq, "instance": instance, "generation": generation, "path": path, "owner": owner }),
            ),
            RecordKind::Register => ("register", json!({ "seq": seq, "instance": instance })),
            RecordKind::FormalGrant {
                registration,
                by_pump,
            } => (
                "register_end",
                json!({ "seq": seq, "register_seq": registration, "instance": instance, "how": "formal_grant", "by_pump": by_pump }),
            ),
            RecordKind::Withdraw { registration } => (
                "register_end",
                json!({ "seq": seq, "register_seq": registration, "instance": instance, "how": "withdrawn" }),
            ),
            RecordKind::GateSet { gate } => {
                lock(&self.gate_seqs).insert((record.instance, gate.clone()), seq);
                (
                    "gate",
                    json!({ "seq": seq, "instance": instance, "gate": gate }),
                )
            }
            RecordKind::GateClear { gate } => {
                let set = lock(&self.gate_seqs).remove(&(record.instance, gate.clone()));
                (
                    "gate_clear",
                    json!({ "seq": seq, "gate_seq": set, "instance": instance, "gate": gate }),
                )
            }
            RecordKind::Publish {
                grant,
                preview_sequence,
                generation,
            } => (
                "publish",
                json!({ "seq": seq, "grant_seq": grant, "instance": instance, "generation": generation, "preview_seq": preview_sequence }),
            ),
            RecordKind::Discard {
                grant,
                reason,
                generation,
            } => (
                "discarded_stale",
                json!({ "seq": seq, "grant_seq": grant, "instance": instance, "generation": generation, "reason": reason }),
            ),
            RecordKind::Manual => ("manual", json!({ "seq": seq, "instance": instance })),
            RecordKind::AuxStart { entry } => (
                "aux_start",
                json!({ "seq": seq, "instance": instance, "entry": entry }),
            ),
            RecordKind::AuxEnd { entry } => (
                "aux_end",
                json!({ "seq": seq, "instance": instance, "entry": entry }),
            ),
            RecordKind::Generation { generation } => (
                "generation",
                json!({ "seq": seq, "instance": instance, "generation": generation }),
            ),
            RecordKind::Lease {
                lease,
                subscriber,
                event,
                active,
                renewed_us,
                ttl_us,
            } => {
                let kind = match (event.as_str(), *active) {
                    ("subscribe", _) => "subscribe",
                    (_, 0) => "lease_last_end",
                    _ => "lease_end",
                };
                // `lease_last_end.t_us`: the active unsubscribe, or the last renewal (TTL).
                let ended = if event == "ttl_expired" {
                    *renewed_us
                } else {
                    at
                };
                (
                    kind,
                    json!({
                        "seq": seq,
                        "instance": instance,
                        "viewer_id": subscriber,
                        "lease": lease,
                        "event": event,
                        "active": active,
                        "ended_us": ended,
                        "ttl_ms": ttl_us / 1_000,
                    }),
                )
            }
        };
        if let Value::Object(map) = &mut body {
            // The record's own time (the stamp below adds the write time as `unix_ms`).
            map.insert("t_record_us".to_owned(), json!(at));
        }
        self.journal.event(kind, body);
    }
}

/// Both a journal and an in-memory copy (the in-run checker keeps its own state; tests read the
/// copy).
pub struct TeeRecords {
    pub first: Arc<dyn RecordSink>,
    pub second: Arc<dyn RecordSink>,
}

impl RecordSink for TeeRecords {
    fn record(&self, record: &SeqRecord) {
        self.first.record(record);
        self.second.record(record);
    }
}
