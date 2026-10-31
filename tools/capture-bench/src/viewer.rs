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

use crate::clock::Micros;
use crate::hub::Hub;
use crate::journal::Journal;
use crate::locks::LockOwner;
use crate::views::ViewMessage;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
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
            "subscribe",
            json!({ "instance": context.instance, "viewer_id": context.subscriber, "lease": lease }),
        );
        let file = format!("viewer-{}.jsonl", context.subscriber);
        Ok(Box::new(InProcessViewer {
            context,
            owner,
            lease: Some(lease.lease_id),
            last_renew: now,
            last_read: 0,
            file,
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
                self.lease = None;
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
            .close_mailbox(self.context.subscriber);
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
}

fn write_message(
    stdin: &mut ChildStdin,
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
            "subscribe",
            json!({ "instance": context.instance, "viewer_id": context.subscriber, "lease": lease, "pid": child.id() }),
        );
        let lease_id = Arc::new(Mutex::new(Some(lease.lease_id)));
        let stopped = Arc::new(AtomicBool::new(false));
        // The writer: the subscriber's one-frame mailbox to the child's stdin.
        {
            let hub = Arc::clone(&context.hub);
            let subscriber = context.subscriber;
            let journal = Arc::clone(&context.journal);
            let _ = std::thread::Builder::new()
                .name(format!("viewer-writer-{subscriber}"))
                .spawn(move || {
                    let hello = json!({ "op": "lease", "lease_id": lease.lease_id, "ttl_ms": lease.ttl_us / 1_000 });
                    if stdin.write_all(format!("{hello}\n").as_bytes()).is_err() {
                        return;
                    }
                    // §5.1: a QPC handshake when the viewer starts and then every minute.
                    let mut next_ping = 0;
                    while let Some(message) = hub.views.wait_take(subscriber) {
                        let now = hub.clock.now_us();
                        if now >= next_ping {
                            next_ping = now + 60_000_000;
                            let ping = json!({ "op": "ping", "bench_us": now });
                            if stdin.write_all(format!("{ping}\n").as_bytes()).is_err() {
                                return;
                            }
                        }
                        if let Err(error) = write_message(&mut stdin, &message, now) {
                            journal.event(
                                "viewer_write_failed",
                                json!({ "viewer_id": subscriber, "error": error.to_string() }),
                            );
                            return;
                        }
                    }
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
                                    *lock(&lease_id) = None;
                                }
                            }
                            (Some("unsubscribe"), Some(lease)) => {
                                hub.unsubscribe(&owner, instance, lease, grace_us);
                                *lock(&lease_id) = None;
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
        }))
    }
}

impl ViewerHandle for ChildViewer {
    fn subscriber(&self) -> u64 {
        self.subscriber
    }

    fn tick(&mut self, _now: Micros) {}

    fn stop(&mut self, unsubscribe: bool) {
        let owner = LockOwner::new(format!("viewer-stop/{}", self.subscriber));
        let lease = lock(&self.lease).take();
        if unsubscribe && let Some(lease) = lease {
            self.context_hub
                .unsubscribe(&owner, self.instance, lease, self.grace_us);
        }
        self.context_hub
            .views
            .broadcast(Some(self.instance), ViewMessage::CellEnded);
        self.context_hub.views.close_mailbox(self.subscriber);
        // The child ends when its stdin closes; a child still there after that is stopped (it
        // is ours, in our job object).
        if !self.stopped.load(Ordering::Acquire) && matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
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
}

fn host_micros() -> u64 {
    crate::winproc::qpc_micros().unwrap_or(0)
}

pub fn view_main(args: ViewArgs) -> i32 {
    let log_path = args
        .run_dir
        .join(format!("viewer-{}.jsonl", args.viewer_id));
    let Ok(log) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    else {
        return 2;
    };
    let log = Arc::new(Mutex::new(std::io::BufWriter::new(log)));
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
    let mut input = BufReader::new(std::io::stdin().lock());
    let mut last_read = std::time::Instant::now();
    let mut line = String::new();
    loop {
        line.clear();
        match input.read_line(&mut line) {
            Ok(0) | Err(_) => break,
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
                    break;
                }
                if let Some(fps) = args.slow_fps {
                    let period = std::time::Duration::from_secs_f64(1.0 / fps.max(0.01));
                    let elapsed = last_read.elapsed();
                    if elapsed < period {
                        std::thread::sleep(period - elapsed);
                    }
                    last_read = std::time::Instant::now();
                }
                json!({
                    "kind": "frame",
                    "viewer_id": args.viewer_id,
                    "instance": message["instance"],
                    "preview_seq": message["preview_seq"],
                    "t_view_recv_us": now,
                    "t_pub_us": message["t_pub_us"],
                })
            }
            Some("ping") => {
                let pong =
                    json!({ "op": "pong", "bench_us": message["bench_us"], "viewer_us": now });
                let mut out = lock(&out);
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
                let mut log = lock(&log);
                let _ = log.write_all(format!("{record}\n").as_bytes());
                break;
            }
            _ => continue,
        };
        let mut log = lock(&log);
        if log.write_all(format!("{record}\n").as_bytes()).is_err() {
            break;
        }
    }
    done.store(true, Ordering::Release);
    let _ = lock(&log).flush();
    0
}
