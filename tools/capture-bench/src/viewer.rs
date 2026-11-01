// SPDX-License-Identifier: AGPL-3.0-only

//! §3.2 item 4 and §3.4: the viewers. A viewer subscribes, renews its lease, unsubscribes, and
//! keeps only the newest frame. In production a viewer is the `view` subcommand of the same
//! binary, a child process in the bench's job object; the bench talks to it over the child's
//! piped standard streams, which Rust's standard library creates as local named pipes on
//! Windows. In the D1 rehearsal a viewer is in-process and driven by the controller's clock.
//!
//! Protocol, bench → viewer (one JSON line, a frame line followed by its pixel bytes):
//! `{"op":"lease",…}`, `{"op":"frame","bytes":n,…}` + n bytes, `{"op":"withdrawn","gate":…}`,
//! `{"op":"cell_ended"}`, `{"op":"ended"}`, `{"op":"ping","bench_us":…}`.
//! Viewer → bench: `{"op":"renew"}`, `{"op":"unsubscribe"}`,
//! `{"op":"pong","bench_us":…,"viewer_us":…}`.
//!
//! #402 C2g, the viewer's record (`viewer-<id>.jsonl`): every record is written whole as it
//! happens (a `LineWriter`, as the journal does), and a viewer that ends normally writes a last
//! record `viewer_end`. A log without that record is incomplete, and the analysis says so: the
//! child was killed, or crashed, at that point. D2c lost up to 8 KiB of tail with no error in 10
//! of 36 viewers: the log was a `BufWriter` flushed only at the end of `view_main`, and
//! `ChildViewer`'s stop killed the child right after closing its mailbox, so a child that had
//! not yet seen the end of its standard input never reached that flush. The stop now gives it
//! time to end by itself, and `events.jsonl` records how each viewer ended (`viewer_ended`).
//!
//! #402 C2g, a lost lease: a viewer learns its lease has gone when a renewal is refused, and
//! subscribes again ([`lease_lost`]) unless a stop gate is in force. A viewer scripted to stop
//! renewing (plan §4.4 item 5, TTL 到期) never does: its expiry is what the case tests.

use crate::clock::Micros;
use crate::hub::Hub;
use crate::journal::Journal;
use crate::locks::LockOwner;
use crate::views::ViewMessage;
use serde_json::{Value, json};
use std::fs::File;
use std::io::{BufRead, BufReader, LineWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a viewer is started with.
pub struct ViewerContext {
    pub hub: Arc<Hub>,
    pub journal: Arc<Journal>,
    pub instance: usize,
    pub subscriber: u64,
    pub ttl_us: Micros,
    pub slow_fps: Option<f64>,
    pub stop_renew_at_us: Option<Micros>,
    pub unsubscribe_at_us: Option<Micros>,
    pub grace_us: Micros,
    /// #402 C2b: this viewer draws each frame in the minimal GDI window (`A_submit`).
    pub gdi: bool,
    /// #402 C2g: the cell, segment, round, path and orientation it serves, for its events.
    pub place: Value,
    /// #402 C2g: how long a stopped child may take to end by itself (`viewer_exit_wait_ms`).
    pub exit_wait_us: Micros,
}

pub trait ViewerHandle: Send {
    fn subscriber(&self) -> u64;
    /// The controller's tick (the in-process viewer renews and reads here).
    fn tick(&mut self, now: Micros);
    /// Ends the viewer; with `unsubscribe` its lease ends actively first.
    fn stop(&mut self, unsubscribe: bool);
}

pub trait ViewerLauncher: Send + Sync {
    fn start(&self, context: ViewerContext) -> Result<Box<dyn ViewerHandle>, String>;
}

fn message_line(subscriber: u64, message: &ViewMessage, now: Micros) -> Value {
    match message {
        ViewMessage::Frame(frame) => json!({
            "kind": "frame",
            "viewer_id": subscriber,
            "instance": frame.instance,
            "preview_seq": frame.preview_sequence,
            "t_view_recv_us": now,
            "t_pub_us": frame.published_us,
            "path": frame.path,
            "generation": frame.generation,
            "validity": frame.validity,
        }),
        ViewMessage::Withdrawn { instance, gate } => json!({
            "kind": "withdrawn",
            "viewer_id": subscriber,
            "instance": instance,
            "gate": gate,
            "t_us": now,
        }),
        ViewMessage::CellEnded => {
            json!({ "kind": "cell_ended", "viewer_id": subscriber, "t_us": now })
        }
        ViewMessage::Ended => json!({ "kind": "ended", "viewer_id": subscriber, "t_us": now }),
    }
}

/// `body` with the viewer's place in the plan (#402 C2g).
fn with_place(place: &Value, mut body: Value) -> Value {
    if let (Value::Object(body), Value::Object(place)) = (&mut body, place) {
        for (key, value) in place {
            body.insert(key.clone(), value.clone());
        }
    }
    body
}

/// What a viewer does when it learns its lease has gone: subscribes again and records it, or,
/// scripted to stop renewing or under a stop gate, stays without a lease. The new lease's id.
pub struct LostLease<'a> {
    pub hub: &'a Hub,
    pub journal: &'a Journal,
    pub owner: &'a LockOwner,
    pub instance: usize,
    pub subscriber: u64,
    pub ttl_us: Micros,
    pub place: &'a Value,
    pub may_resubscribe: bool,
}

pub fn lease_lost(lost: &LostLease<'_>) -> Option<u64> {
    if !lost.may_resubscribe {
        return None;
    }
    let lease = lost
        .hub
        .resubscribe(lost.owner, lost.instance, lost.subscriber, lost.ttl_us)?;
    lost.journal.event(
        "lease_resubscribed",
        with_place(
            lost.place,
            json!({ "instance": lost.instance, "viewer_id": lost.subscriber, "lease": lease.lease_id }),
        ),
    );
    Some(lease.lease_id)
}

// ---- in-process viewers (D1) ----

/// Starts in-process viewers that the controller drives.
pub struct InProcessViewers;

struct InProcessViewer {
    context: ViewerContext,
    owner: Arc<LockOwner>,
    lease: Option<u64>,
    last_renew: Micros,
    last_read: Micros,
    file: String,
    frames: u64,
}

impl ViewerLauncher for InProcessViewers {
    fn start(&self, context: ViewerContext) -> Result<Box<dyn ViewerHandle>, String> {
        let owner = LockOwner::new(format!("viewer/{}", context.subscriber));
        let lease =
            context
                .hub
                .subscribe(&owner, context.instance, context.subscriber, context.ttl_us);
        let now = context.hub.clock.now_us();
        context.journal.event(
            "viewer_started",
            with_place(
                &context.place,
                json!({ "instance": context.instance, "viewer_id": context.subscriber, "lease": lease }),
            ),
        );
        let file = format!("viewer-{}.jsonl", context.subscriber);
        Ok(Box::new(InProcessViewer {
            context,
            owner,
            lease: Some(lease.lease_id),
            last_renew: now,
            last_read: 0,
            file,
            frames: 0,
        }))
    }
}

impl ViewerHandle for InProcessViewer {
    fn subscriber(&self) -> u64 {
        self.context.subscriber
    }

    fn tick(&mut self, now: Micros) {
        let Some(lease) = self.lease else {
            return;
        };
        if self.context.unsubscribe_at_us.is_some_and(|at| now >= at) {
            self.context.hub.unsubscribe(
                &self.owner,
                self.context.instance,
                lease,
                self.context.grace_us,
            );
            self.context.journal.event(
                "unsubscribe",
                json!({ "instance": self.context.instance, "viewer_id": self.context.subscriber, "t_us": now }),
            );
            self.lease = None;
            return;
        }
        let renewing = self.context.stop_renew_at_us.is_none_or(|at| now < at);
        if renewing && now.saturating_sub(self.last_renew) >= self.context.ttl_us / 3 {
            if self
                .context
                .hub
                .renew(&self.owner, self.context.instance, lease)
            {
                self.last_renew = now;
            } else {
                self.lease = lease_lost(&LostLease {
                    hub: &self.context.hub,
                    journal: &self.context.journal,
                    owner: &self.owner,
                    instance: self.context.instance,
                    subscriber: self.context.subscriber,
                    ttl_us: self.context.ttl_us,
                    place: &self.context.place,
                    may_resubscribe: self.context.stop_renew_at_us.is_none(),
                });
                self.last_renew = now;
            }
        }
        let period = self
            .context
            .slow_fps
            .map(|fps| ((1_000_000.0 / fps) as Micros).max(1));
        if period.is_some_and(|period| now.saturating_sub(self.last_read) < period) {
            return;
        }
        self.last_read = now;
        if let Some(message) = self.context.hub.views.take(self.context.subscriber) {
            self.frames += u64::from(matches!(message, ViewMessage::Frame(_)));
            self.context.journal.append(
                &self.file,
                &message_line(self.context.subscriber, &message, now),
            );
        }
    }

    fn stop(&mut self, unsubscribe: bool) {
        if unsubscribe && let Some(lease) = self.lease.take() {
            self.context.hub.unsubscribe(
                &self.owner,
                self.context.instance,
                lease,
                self.context.grace_us,
            );
        }
        self.context
            .hub
            .views
            .retire_mailbox(self.context.subscriber);
        let now = self.context.hub.clock.now_us();
        self.context.journal.append(
            &self.file,
            &json!({
                "kind": "viewer_end",
                "viewer_id": self.context.subscriber,
                "t_us": now,
                "reason": "stopped",
                "frames": self.frames,
            }),
        );
        self.context.journal.event(
            "viewer_ended",
            with_place(
                &self.context.place,
                json!({
                    "instance": self.context.instance,
                    "viewer_id": self.context.subscriber,
                    "how": "in_process",
                    "exit_code": null,
                    "ended_before_stop": false,
                }),
            ),
        );
    }
}

// ---- child-process viewers (production) ----

/// Starts viewer child processes (`view` subcommand) in the bench's job object.
pub struct ChildViewers {
    pub executable: PathBuf,
    pub run_dir: PathBuf,
    pub job: Option<Arc<crate::winproc::Job>>,
}

struct ChildViewer {
    context_hub: Arc<Hub>,
    instance: usize,
    subscriber: u64,
    grace_us: Micros,
    lease: Arc<Mutex<Option<u64>>>,
    child: Child,
    stopped: Arc<AtomicBool>,
    /// #402 INT-1: the bench has begun to stop this viewer (its pipe may close under a write).
    stopping: Arc<AtomicBool>,
    journal: Arc<Journal>,
    place: Value,
    exit_wait_us: Micros,
}

/// #402 INT-1: the `viewer_write_failed` record. A write that fails with a closed pipe
/// (`ERROR_BROKEN_PIPE` 109, `ERROR_NO_DATA` 232) after the bench began to stop the viewer is
/// expected: the stop closes the viewer's mailbox and ends a child that is still reading, and
/// a slow consumer's pipe is still full of the frame being written then (D2, `multi_slow`'s
/// end). Any other failure, or one before the stop, is not expected.
///
/// #402 C2f: `stopping` is whether the stop has begun for this viewer in any way
/// ([`feed_viewer`]): its own stop, or a stop gate.
pub fn viewer_write_failure(subscriber: u64, error: &std::io::Error, stopping: bool) -> Value {
    let closed_pipe = error.kind() == std::io::ErrorKind::BrokenPipe
        || matches!(error.raw_os_error(), Some(109 | 232));
    let expected = stopping && closed_pipe;
    json!({
        "viewer_id": subscriber,
        "error": error.to_string(),
        "os_error": error.raw_os_error(),
        "expected": expected,
        "cause": match (stopping, closed_pipe) {
            (true, true) => "pipe_closed_by_the_viewer_stop",
            (true, false) => "write_failed_while_stopping",
            (false, _) => "write_failed_during_the_segment",
        },
    })
}

fn write_message<W: Write>(
    stdin: &mut W,
    message: &ViewMessage,
    now: Micros,
) -> std::io::Result<()> {
    match message {
        ViewMessage::Frame(frame) => {
            let header = json!({
                "op": "frame",
                "instance": frame.instance,
                "preview_seq": frame.preview_sequence,
                "generation": frame.generation,
                "t_pub_us": frame.published_us,
                "width": frame.width,
                "height": frame.height,
                "layout": frame.layout,
                "bytes": frame.pixels.len(),
            });
            stdin.write_all(format!("{header}\n").as_bytes())?;
            stdin.write_all(&frame.pixels)?;
        }
        ViewMessage::Withdrawn { gate, .. } => {
            stdin.write_all(
                format!(
                    "{}\n",
                    json!({ "op": "withdrawn", "gate": gate, "t_us": now })
                )
                .as_bytes(),
            )?;
        }
        ViewMessage::CellEnded => stdin.write_all(b"{\"op\":\"cell_ended\"}\n")?,
        ViewMessage::Ended => stdin.write_all(b"{\"op\":\"ended\"}\n")?,
    }
    stdin.flush()
}

/// The hello, then the mailbox, until the mailbox closes or a write fails. #402 C2g: with
/// `may_resubscribe` a closed mailbox (a lease that expired) does not end the viewer: the writer
/// waits for the new lease, or for the stop that retires the mailbox.
fn write_messages<W: Write>(
    stdin: &mut W,
    hub: &Hub,
    subscriber: u64,
    hello: &Value,
    may_resubscribe: bool,
) -> std::io::Result<()> {
    stdin.write_all(format!("{hello}\n").as_bytes())?;
    // §5.1: a QPC handshake when the viewer starts and then every minute.
    let mut next_ping = 0;
    loop {
        let Some(message) = hub.views.wait_take(subscriber) else {
            if may_resubscribe && hub.views.wait_reopened(subscriber) {
                continue;
            }
            return Ok(());
        };
        let now = hub.clock.now_us();
        if now >= next_ping {
            next_ping = now + 60_000_000;
            let ping = json!({ "op": "ping", "bench_us": now });
            stdin.write_all(format!("{ping}\n").as_bytes())?;
        }
        write_message(stdin, &message, now)?;
    }
}

/// The writer thread's body: the subscriber's one-frame mailbox to the child's stdin.
///
/// #402 C2f: a failed write is expected once a stop has begun, not only once this viewer's own
/// [`ViewerHandle::stop`] has. The stop gate comes first and then `ended` ends the child; the
/// `cell_ended` another viewer's stop broadcasts can then meet its closed pipe before its own
/// stop begins (D2b, STOP and hang-N: os error 232 recorded `expected: false`, the same error
/// `expected: true` on ABORT). The gate is read after the failure, so a gate set before it counts.
#[allow(clippy::too_many_arguments)]
pub fn feed_viewer<W: Write>(
    stdin: &mut W,
    hub: &Hub,
    journal: &Journal,
    instance: usize,
    subscriber: u64,
    hello: &Value,
    stopping: &AtomicBool,
    may_resubscribe: bool,
) {
    if let Err(error) = write_messages(stdin, hub, subscriber, hello, may_resubscribe) {
        let owner = LockOwner::new(format!("viewer-writer/{subscriber}"));
        let stop_begun = stopping.load(Ordering::Acquire) || hub.stop_in_force(&owner, instance);
        journal.event(
            "viewer_write_failed",
            viewer_write_failure(subscriber, &error, stop_begun),
        );
    }
}

impl ViewerLauncher for ChildViewers {
    fn start(&self, context: ViewerContext) -> Result<Box<dyn ViewerHandle>, String> {
        let mut command = Command::new(&self.executable);
        command
            .arg("view")
            .arg("--viewer-id")
            .arg(context.subscriber.to_string())
            .arg("--run-dir")
            .arg(&self.run_dir)
            .arg("--ttl-ms")
            .arg((context.ttl_us / 1_000).to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(fps) = context.slow_fps {
            command.arg("--slow-fps").arg(fps.to_string());
        }
        if context.gdi {
            command.arg("--gdi");
        }
        let now = context.hub.clock.now_us();
        if let Some(at) = context.stop_renew_at_us {
            command
                .arg("--stop-renew-after-ms")
                .arg((at.saturating_sub(now) / 1_000).to_string());
        }
        if let Some(at) = context.unsubscribe_at_us {
            command
                .arg("--unsubscribe-after-ms")
                .arg((at.saturating_sub(now) / 1_000).to_string());
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("the viewer did not start: {error}"))?;
        if let Some(job) = &self.job
            && let Err(error) = job.assign(&child)
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("the viewer is not in the job object: {error}"));
        }
        let mut stdin = child.stdin.take().ok_or("the viewer has no stdin")?;
        let stdout = child.stdout.take().ok_or("the viewer has no stdout")?;
        let owner = LockOwner::new(format!("viewer/{}", context.subscriber));
        let lease =
            context
                .hub
                .subscribe(&owner, context.instance, context.subscriber, context.ttl_us);
        context.journal.event(
            "viewer_started",
            with_place(
                &context.place,
                json!({ "instance": context.instance, "viewer_id": context.subscriber, "lease": lease, "pid": child.id() }),
            ),
        );
        let lease_id = Arc::new(Mutex::new(Some(lease.lease_id)));
        let stopped = Arc::new(AtomicBool::new(false));
        let stopping = Arc::new(AtomicBool::new(false));
        let may_resubscribe = context.stop_renew_at_us.is_none();
        // The writer: the subscriber's one-frame mailbox to the child's stdin.
        {
            let hub = Arc::clone(&context.hub);
            let subscriber = context.subscriber;
            let journal = Arc::clone(&context.journal);
            let stopping = Arc::clone(&stopping);
            let instance = context.instance;
            let hello = json!({ "op": "lease", "lease_id": lease.lease_id, "ttl_ms": lease.ttl_us / 1_000 });
            let _ = std::thread::Builder::new()
                .name(format!("viewer-writer-{subscriber}"))
                .spawn(move || {
                    feed_viewer(
                        &mut stdin,
                        &hub,
                        &journal,
                        instance,
                        subscriber,
                        &hello,
                        &stopping,
                        may_resubscribe,
                    );
                });
        }
        // The reader: renewals, unsubscribes and handshakes from the child.
        {
            let hub = Arc::clone(&context.hub);
            let journal = Arc::clone(&context.journal);
            let lease_id = Arc::clone(&lease_id);
            let instance = context.instance;
            let subscriber = context.subscriber;
            let grace_us = context.grace_us;
            let stopped = Arc::clone(&stopped);
            let stopping = Arc::clone(&stopping);
            let ttl_us = context.ttl_us;
            let place = context.place.clone();
            let _ = std::thread::Builder::new()
                .name(format!("viewer-reader-{subscriber}"))
                .spawn(move || {
                    let owner = LockOwner::new(format!("viewer-reader/{subscriber}"));
                    for line in BufReader::new(stdout).lines() {
                        let Ok(line) = line else { break };
                        let Ok(message) = serde_json::from_str::<Value>(&line) else {
                            journal.event("viewer_message_invalid", json!({ "viewer_id": subscriber }));
                            continue;
                        };
                        let current = *lock(&lease_id);
                        match (message["op"].as_str(), current) {
                            (Some("renew"), Some(lease)) => {
                                if !hub.renew(&owner, instance, lease) {
                                    // #402 C2g: under the lease's lock, so that a stop either
                                    // comes first (and no lease is taken) or sees the new lease
                                    // and ends it.
                                    let mut current = lock(&lease_id);
                                    *current = if stopping.load(Ordering::Acquire) {
                                        None
                                    } else {
                                        lease_lost(&LostLease {
                                            hub: &hub,
                                            journal: &journal,
                                            owner: &owner,
                                            instance,
                                            subscriber,
                                            ttl_us,
                                            place: &place,
                                            may_resubscribe,
                                        })
                                    };
                                }
                            }
                            (Some("unsubscribe"), current) => {
                                if let Some(lease) = current {
                                    hub.unsubscribe(&owner, instance, lease, grace_us);
                                }
                                *lock(&lease_id) = None;
                                // No lease of this viewer comes back: its writer ends.
                                hub.views.retire_mailbox(subscriber);
                                journal.event(
                                    "unsubscribe",
                                    json!({ "instance": instance, "viewer_id": subscriber, "t_us": hub.clock.now_us() }),
                                );
                            }
                            (Some("pong"), _) => {
                                let now = hub.clock.now_us();
                                let sent = message["bench_us"].as_u64().unwrap_or(0);
                                let viewer = message["viewer_us"].as_u64().unwrap_or(0);
                                let rtt = now.saturating_sub(sent);
                                let midpoint = sent + rtt / 2;
                                journal.append(
                                    "calib-handshake.jsonl",
                                    &json!({
                                        "type": "handshake",
                                        "viewer_id": subscriber,
                                        "t_us": now,
                                        "rtt_us": rtt,
                                        "one_way_diff_us": viewer as i64 - midpoint as i64,
                                    }),
                                );
                            }
                            _ => {}
                        }
                    }
                    stopped.store(true, Ordering::Release);
                });
        }
        Ok(Box::new(ChildViewer {
            context_hub: Arc::clone(&context.hub),
            instance: context.instance,
            subscriber: context.subscriber,
            grace_us: context.grace_us,
            lease: lease_id,
            child,
            stopped,
            stopping,
            journal: Arc::clone(&context.journal),
            place: context.place.clone(),
            exit_wait_us: context.exit_wait_us,
        }))
    }
}

impl ViewerHandle for ChildViewer {
    fn subscriber(&self) -> u64 {
        self.subscriber
    }

    fn tick(&mut self, _now: Micros) {}

    fn stop(&mut self, unsubscribe: bool) {
        self.stopping.store(true, Ordering::Release);
        let owner = LockOwner::new(format!("viewer-stop/{}", self.subscriber));
        let lease = lock(&self.lease).take();
        if unsubscribe && let Some(lease) = lease {
            self.context_hub
                .unsubscribe(&owner, self.instance, lease, self.grace_us);
        }
        self.context_hub
            .views
            .broadcast(Some(self.instance), ViewMessage::CellEnded);
        let ended_before_stop = self.stopped.load(Ordering::Acquire);
        self.context_hub.views.retire_mailbox(self.subscriber);
        // #402 C2g: the writer ends now, so the child sees its stdin close, writes its last
        // record and ends. It gets the plan's time for that; only a child still there after it
        // is killed (it is ours, in our job object), and its log then has no last record. Real
        // time: this is a real process (the D1 rehearsal's viewers are in-process).
        let started = std::time::Instant::now();
        let wait = std::time::Duration::from_micros(self.exit_wait_us);
        let mut wait_error = None;
        let ended = loop {
            match self.child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if started.elapsed() < wait => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Ok(None) => break None,
                Err(error) => {
                    wait_error = Some(error.to_string());
                    break None;
                }
            }
        };
        let waited_ms = started.elapsed().as_millis() as u64;
        let (how, status) = match ended {
            Some(status) => ("exited", Ok(status)),
            None => {
                let killed = self.child.kill();
                let status = self.child.wait();
                (
                    "killed",
                    killed.and(status).map_err(|error| error.to_string()),
                )
            }
        };
        let (exit_code, status_error) = match status {
            Ok(status) => (status.code(), None),
            Err(error) => (None, Some(error)),
        };
        self.journal.event(
            "viewer_ended",
            with_place(
                &self.place,
                json!({
                    "instance": self.instance,
                    "viewer_id": self.subscriber,
                    "how": how,
                    "exit_code": exit_code,
                    "waited_ms": waited_ms,
                    "ended_before_stop": ended_before_stop,
                    "error": wait_error.or(status_error),
                }),
            ),
        );
    }
}

/// The `view` subcommand (the child): reads frames, renews, writes `viewer-<id>.jsonl`.
pub struct ViewArgs {
    pub viewer_id: u64,
    pub run_dir: PathBuf,
    pub ttl_ms: u64,
    pub slow_fps: Option<f64>,
    pub stop_renew_after_ms: Option<u64>,
    pub unsubscribe_after_ms: Option<u64>,
    /// #402 C2b: draw each frame in the minimal GDI window and record `t_gdi_ret_us`.
    pub gdi: bool,
}

fn host_micros() -> u64 {
    crate::winproc::qpc_micros().unwrap_or(0)
}

/// `view_main`'s exit codes: 2, the log did not open; 3, a record could not be written; 4, the
/// bench's stream failed or stopped inside a frame. `viewer_ended` carries them.
pub const VIEW_EXIT_NO_LOG: i32 = 2;
pub const VIEW_EXIT_LOG_WRITE_FAILED: i32 = 3;
pub const VIEW_EXIT_INPUT_FAILED: i32 = 4;

/// #402 C2g: the viewer's log: each record reaches the file as a whole when it is written, as in
/// the journal (`journal.rs`, `LineWriter`), so a child killed later loses nothing before it.
pub fn open_view_log(path: &Path) -> std::io::Result<LineWriter<File>> {
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    Ok(LineWriter::new(log))
}

fn write_view_record<L: Write>(log: &mut L, record: &Value) -> std::io::Result<()> {
    log.write_all(format!("{record}\n").as_bytes())?;
    log.flush()
}

pub fn view_main(args: ViewArgs) -> i32 {
    let log_path = args
        .run_dir
        .join(format!("viewer-{}.jsonl", args.viewer_id));
    let Ok(mut log) = open_view_log(&log_path) else {
        return VIEW_EXIT_NO_LOG;
    };
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let started = std::time::Instant::now();
    let done = Arc::new(AtomicBool::new(false));
    // The renewer.
    {
        let out = Arc::clone(&out);
        let done = Arc::clone(&done);
        let ttl = args.ttl_ms.max(3);
        let stop_renew = args.stop_renew_after_ms;
        let unsubscribe = args.unsubscribe_after_ms;
        let _ = std::thread::Builder::new()
            .name("renew".to_owned())
            .spawn(move || {
                while !done.load(Ordering::Acquire) {
                    std::thread::sleep(std::time::Duration::from_millis(ttl / 3));
                    let elapsed = started.elapsed().as_millis() as u64;
                    let message = if unsubscribe.is_some_and(|at| elapsed >= at) {
                        done.store(true, Ordering::Release);
                        "{\"op\":\"unsubscribe\"}\n"
                    } else if stop_renew.is_some_and(|at| elapsed >= at) {
                        continue;
                    } else {
                        "{\"op\":\"renew\"}\n"
                    };
                    let mut out = lock(&out);
                    if out
                        .write_all(message.as_bytes())
                        .and_then(|()| out.flush())
                        .is_err()
                    {
                        return;
                    }
                }
            });
    }
    let input = BufReader::new(std::io::stdin().lock());
    let code = view_records(&args, input, &*out, &mut log);
    done.store(true, Ordering::Release);
    code
}

/// The viewer's loop: reads the bench's stream, writes each record as it happens and, when the
/// stream ends, the last record `viewer_end` (#402 C2g). Returns the exit code.
pub fn view_records<I: BufRead, O: Write, L: Write>(
    args: &ViewArgs,
    mut input: I,
    out: &Mutex<O>,
    log: &mut L,
) -> i32 {
    let mut last_read = std::time::Instant::now();
    // #402 C2b (§5.1): the minimal window; created at the first frame, at its size.
    let mut window: Option<crate::winproc::GdiWindow> = None;
    let mut window_error: Option<String> = None;
    let mut line = String::new();
    let mut frames = 0_u64;
    let reason = loop {
        line.clear();
        match input.read_line(&mut line) {
            Ok(0) => break "stdin_closed",
            Err(_) => break "input_failed",
            Ok(_) => {}
        }
        let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let now = host_micros();
        let record = match message["op"].as_str() {
            Some("frame") => {
                let bytes = message["bytes"].as_u64().unwrap_or(0) as usize;
                let mut pixels = vec![0u8; bytes];
                if input.read_exact(&mut pixels).is_err() {
                    break "input_cut_inside_a_frame";
                }
                if let Some(fps) = args.slow_fps {
                    let period = std::time::Duration::from_secs_f64(1.0 / fps.max(0.01));
                    let elapsed = last_read.elapsed();
                    if elapsed < period {
                        std::thread::sleep(period - elapsed);
                    }
                    last_read = std::time::Instant::now();
                }
                let mut gdi_ret = None;
                if args.gdi {
                    let width = message["width"].as_u64().unwrap_or(0) as u32;
                    let height = message["height"].as_u64().unwrap_or(0) as u32;
                    let bytes_per_pixel = if message["layout"] == "rgb8" { 3 } else { 4 };
                    if window.is_none() && window_error.is_none() {
                        match crate::winproc::GdiWindow::create(width, height) {
                            Ok(created) => window = Some(created),
                            Err(error) => window_error = Some(error),
                        }
                    }
                    if let Some(window) = window.as_mut() {
                        match window.present(&pixels, width, height, bytes_per_pixel) {
                            Ok(()) => gdi_ret = Some(host_micros()),
                            Err(error) => window_error = Some(error),
                        }
                    }
                }
                frames += 1;
                json!({
                    "kind": "frame",
                    "viewer_id": args.viewer_id,
                    "instance": message["instance"],
                    "preview_seq": message["preview_seq"],
                    "t_view_recv_us": now,
                    "t_pub_us": message["t_pub_us"],
                    "t_gdi_ret_us": gdi_ret,
                    "gdi": if args.gdi { Some("submit_side_proxy_not_presentation") } else { None },
                    "null_reasons": if args.gdi && gdi_ret.is_none() {
                        json!({ "t_gdi_ret_us": window_error.clone().unwrap_or_else(|| "no window".to_owned()) })
                    } else if args.gdi {
                        json!({})
                    } else {
                        json!({ "t_gdi_ret_us": "this viewer has no GDI window" })
                    },
                })
            }
            Some("ping") => {
                let pong =
                    json!({ "op": "pong", "bench_us": message["bench_us"], "viewer_us": now });
                let mut out = lock(out);
                let _ = out
                    .write_all(format!("{pong}\n").as_bytes())
                    .and_then(|()| out.flush());
                continue;
            }
            Some("withdrawn") => {
                json!({ "kind": "withdrawn", "viewer_id": args.viewer_id, "gate": message["gate"], "t_us": now })
            }
            Some("cell_ended") => {
                json!({ "kind": "cell_ended", "viewer_id": args.viewer_id, "t_us": now })
            }
            Some("ended") => {
                let record = json!({ "kind": "ended", "viewer_id": args.viewer_id, "t_us": now });
                if write_view_record(log, &record).is_err() {
                    return VIEW_EXIT_LOG_WRITE_FAILED;
                }
                break "ended";
            }
            _ => continue,
        };
        if write_view_record(log, &record).is_err() {
            return VIEW_EXIT_LOG_WRITE_FAILED;
        }
    };
    let end = json!({
        "kind": "viewer_end",
        "viewer_id": args.viewer_id,
        "t_us": host_micros(),
        "reason": reason,
        "frames": frames,
    });
    if write_view_record(log, &end).is_err() {
        return VIEW_EXIT_LOG_WRITE_FAILED;
    }
    match reason {
        "stdin_closed" | "ended" => 0,
        _ => VIEW_EXIT_INPUT_FAILED,
    }
}
