// SPDX-License-Identifier: AGPL-3.0-only

//! D1 (plan v2.1 §6.2) for what D2c found. The viewer's record: every record is in the file as it
//! happens, a last record `viewer_end` when the viewer ends normally, `viewer_ended` in the events.
//! A lost lease: the viewer subscribes again while the segment runs and no stop gate is set; the
//! scripted TTL case (§4.4 item 5) still expires and never does. The logic runs on the virtual
//! clock. The last two tests run the real `view` child, because what D2c lost was lost between a
//! real child and its stop; they read no real clock for any decision, and only wait for the child.

mod common;
mod hub_fixture;

use actingcommand_capture_bench::backend::PixelLayout;
use actingcommand_capture_bench::clock::{Clock, Micros, SystemClock, spawn_participant};
use actingcommand_capture_bench::fake::{FakeCheckScript, FakeScript};
use actingcommand_capture_bench::hub::{BudgetConfig, Hub, MemorySink};
use actingcommand_capture_bench::journal::Journal;
use actingcommand_capture_bench::locks::LockOwner;
use actingcommand_capture_bench::model::{GateKind, GlobalGate, PathKind};
use actingcommand_capture_bench::runner::{RunOutcome, run};
use actingcommand_capture_bench::safety::AcceptStartChecks;
use actingcommand_capture_bench::viewer::{
    ChildViewers, InProcessViewers, VIEW_EXIT_INPUT_FAILED, VIEW_EXIT_LOG_WRITE_FAILED, ViewArgs,
    ViewerContext, ViewerHandle, ViewerLauncher, open_view_log, view_records,
};
use actingcommand_capture_bench::views::{PreviewFrame, Validity};
use common::{cell, harness, limits, plan, preview};
use hub_fixture::{Fixture, fixture, instant_script};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const HOURS: u64 = 3_600_000;

static NEXT_DIR: AtomicU64 = AtomicU64::new(1);

fn fresh_dir(name: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "capture-bench-d1/c2g-{name}-{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).expect("the directory");
    root
}

fn jsonl(text: &str) -> Vec<Value> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("a whole JSON line"))
        .collect()
}

fn read_jsonl(path: &Path) -> Vec<Value> {
    jsonl(&std::fs::read_to_string(path).unwrap_or_default())
}

fn of_type(events: &[Value], kind: &str) -> Vec<Value> {
    events
        .iter()
        .filter(|event| event["type"] == kind)
        .cloned()
        .collect()
}

fn us(value: &Value) -> u64 {
    value
        .as_u64()
        .unwrap_or_else(|| panic!("not a number: {value}"))
}

// ---- the viewer's record ----

fn view_args(root: &Path) -> ViewArgs {
    ViewArgs {
        viewer_id: 7,
        run_dir: root.to_path_buf(),
        ttl_ms: 3_000,
        slow_fps: None,
        stop_renew_after_ms: None,
        unsubscribe_after_ms: None,
        gdi: false,
    }
}

/// The bench's stream, one message per read. At every read it copies the log as it is on disk:
/// what a kill at that moment would leave.
struct Stream {
    messages: VecDeque<Vec<u8>>,
    log: PathBuf,
    on_disk: Arc<Mutex<Vec<String>>>,
}

impl Read for Stream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.on_disk
            .lock()
            .expect("the copies")
            .push(std::fs::read_to_string(&self.log).unwrap_or_default());
        let Some(message) = self.messages.front_mut() else {
            return Ok(0);
        };
        let count = message.len().min(buffer.len());
        buffer[..count].copy_from_slice(&message[..count]);
        message.drain(..count);
        if message.is_empty() {
            self.messages.pop_front();
        }
        Ok(count)
    }
}

fn frame_message(sequence: u64) -> Vec<Vec<u8>> {
    let header =
        json!({ "op": "frame", "instance": 0, "preview_seq": sequence, "t_pub_us": 1, "bytes": 4 });
    vec![format!("{header}\n").into_bytes(), vec![0_u8; 4]]
}

/// Runs the viewer's loop over `messages`: the exit code, the file, the copies taken at each read.
fn view(name: &str, messages: Vec<Vec<u8>>) -> (i32, Vec<Value>, Vec<String>) {
    let root = fresh_dir(name);
    let path = root.join("viewer-7.jsonl");
    let mut log = open_view_log(&path).expect("the log opens");
    let on_disk = Arc::new(Mutex::new(Vec::new()));
    let input = BufReader::new(Stream {
        messages: messages.into(),
        log: path.clone(),
        on_disk: Arc::clone(&on_disk),
    });
    let code = view_records(
        &view_args(&root),
        input,
        &Mutex::new(Vec::<u8>::new()),
        &mut log,
    );
    let copies = on_disk.lock().expect("the copies").clone();
    (code, read_jsonl(&path), copies)
}

#[test]
fn every_record_is_in_the_file_as_it_happens_and_the_last_record_comes_at_the_end() {
    let messages = (1..=3).flat_map(frame_message).collect();
    let (code, rows, copies) = view("whole", messages);
    assert_eq!(code, 0);
    // A kill just before the stream's end leaves all three whole records and no last record: the
    // BufWriter of D2c left nothing here, and the analysis could not tell.
    let killed_at_the_end = jsonl(copies.last().expect("a copy"));
    assert_eq!(killed_at_the_end.len(), 3, "{killed_at_the_end:?}");
    assert!(
        killed_at_the_end
            .iter()
            .all(|record| record["kind"] == "frame"),
        "{killed_at_the_end:?}"
    );
    // And after the first frame, before the second: one record.
    assert!(
        copies.iter().any(|copy| jsonl(copy).len() == 1),
        "{copies:?}"
    );

    assert_eq!(rows.len(), 4, "{rows:?}");
    let last = &rows[3];
    assert_eq!(last["kind"], "viewer_end", "{last}");
    assert_eq!(last["viewer_id"], 7, "{last}");
    assert_eq!(last["reason"], "stdin_closed", "{last}");
    assert_eq!(last["frames"], 3, "{last}");
}

#[test]
fn the_runs_end_message_also_gets_the_last_record() {
    let (code, rows, _) = view(
        "ended",
        vec![
            b"{\"op\":\"cell_ended\"}\n".to_vec(),
            b"{\"op\":\"ended\"}\n".to_vec(),
        ],
    );
    assert_eq!(code, 0);
    let kinds: Vec<&str> = rows
        .iter()
        .map(|row| row["kind"].as_str().expect("a kind"))
        .collect();
    assert_eq!(kinds, ["cell_ended", "ended", "viewer_end"]);
    assert_eq!(rows[2]["reason"], "ended");
    assert_eq!(rows[2]["frames"], 0);
}

#[test]
fn a_stream_cut_inside_a_frame_says_so_and_does_not_exit_0() {
    let header =
        json!({ "op": "frame", "instance": 0, "preview_seq": 1, "t_pub_us": 1, "bytes": 8 });
    let (code, rows, _) = view(
        "cut",
        vec![format!("{header}\n").into_bytes(), vec![0_u8; 2]],
    );
    assert_eq!(code, VIEW_EXIT_INPUT_FAILED);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["kind"], "viewer_end");
    assert_eq!(rows[0]["reason"], "input_cut_inside_a_frame");
    assert_eq!(rows[0]["frames"], 0);
}

struct FailingLog;

impl Write for FailingLog {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("the disk is full"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_record_that_cannot_be_written_ends_the_viewer_with_its_own_exit_code() {
    let root = fresh_dir("failing");
    let messages: Vec<Vec<u8>> = frame_message(1);
    let input = BufReader::new(Stream {
        messages: messages.into(),
        log: root.join("no-such-log"),
        on_disk: Arc::new(Mutex::new(Vec::new())),
    });
    let code = view_records(
        &view_args(&root),
        input,
        &Mutex::new(Vec::<u8>::new()),
        &mut FailingLog,
    );
    assert_eq!(code, VIEW_EXIT_LOG_WRITE_FAILED);
}

// ---- a lost lease, on the virtual clock ----

/// Viewers whose ticks do not reach them during a stretch of the segment: the viewer stalls (D2c:
/// the host stalled for 34 s), the controller's lease sweep goes on.
struct StallingViewers {
    from_us: Micros,
    until_us: Micros,
}

struct StallingViewer {
    inner: Box<dyn ViewerHandle>,
    from_us: Micros,
    until_us: Micros,
}

impl ViewerLauncher for StallingViewers {
    fn start(&self, context: ViewerContext) -> Result<Box<dyn ViewerHandle>, String> {
        let started = context.hub.clock.now_us();
        Ok(Box::new(StallingViewer {
            inner: InProcessViewers.start(context)?,
            from_us: started + self.from_us,
            until_us: started + self.until_us,
        }))
    }
}

impl ViewerHandle for StallingViewer {
    fn subscriber(&self) -> u64 {
        self.inner.subscriber()
    }

    fn tick(&mut self, now: Micros) {
        if now < self.from_us || now >= self.until_us {
            self.inner.tick(now);
        }
    }

    fn stop(&mut self, unsubscribe: bool) {
        self.inner.stop(unsubscribe);
    }
}

fn run_with(
    h: &common::Harness,
    plan_text: String,
    viewers: Arc<dyn ViewerLauncher>,
) -> RunOutcome {
    let mut env = h.env(limits(), Vec::new(), Box::new(AcceptStartChecks));
    env.viewers = viewers;
    let run_dir = h.run_dir.clone();
    spawn_participant(&h.dyn_clock(), "d1-run", move || {
        run(&plan_text, &run_dir, env)
    })
    .expect("the run thread starts")
    .join()
    .expect("the run does not panic")
}

/// The time of the one `ttl_expired`, as the sweep recorded it.
fn the_expiry(events: &[Value]) -> u64 {
    let ends: Vec<Value> = of_type(events, "lease_last_end")
        .into_iter()
        .filter(|end| end["event"] == "ttl_expired")
        .collect();
    assert_eq!(ends.len(), 1, "{ends:?}");
    us(&ends[0]["t_record_us"])
}

#[test]
fn a_viewer_that_lost_its_lease_subscribes_again_and_only_the_gap_is_lost() {
    for path in ["N", "S"] {
        let name = format!("resub-{path}");
        let h = harness(
            &name,
            &name,
            FakeScript::default(),
            FakeCheckScript::default(),
        );
        let outcome = run_with(
            &h,
            plan(
                &name,
                10 * HOURS,
                &cell("CA", 60_000, &[&preview(path, 12_000)]),
            ),
            Arc::new(StallingViewers {
                from_us: 2_000_000,
                until_us: 7_000_000,
            }),
        );
        assert_eq!(outcome.exit_code, 0, "{path}: {outcome:?}");
        let events = h.events();
        let expired = the_expiry(&events);

        // The bench records it, with where it happened.
        let again = of_type(&events, "lease_resubscribed");
        assert_eq!(again.len(), 1, "{path}: {again:?}");
        assert_eq!(again[0]["viewer_id"], 1, "{}", again[0]);
        assert_eq!(again[0]["cell"], "CA", "{}", again[0]);
        assert_eq!(again[0]["path"], path, "{}", again[0]);
        assert_eq!(again[0]["round"], 1, "{}", again[0]);
        let resubscribed = us(&again[0]["t_us"]);
        assert!(expired < resubscribed, "{expired} {resubscribed}");
        assert_eq!(of_type(&events, "subscribe").len(), 2, "{path}");
        assert_eq!(h.json("summary.json")["lease_resubscribed"], 1, "{path}");

        // The gap stays skipped_no_lease; after it the frames come again.
        let opportunities = h.lines(&format!("opps-CA-{path}.jsonl"));
        let in_gap: Vec<&Value> = opportunities
            .iter()
            .filter(|record| {
                let tick = us(&record["t_tick_us"]);
                tick > expired && tick < resubscribed
            })
            .collect();
        assert!(!in_gap.is_empty(), "{path}: no opportunity in the gap");
        assert!(
            in_gap
                .iter()
                .all(|record| record["disposition"] == "skipped_no_lease"),
            "{path}: {in_gap:?}"
        );
        let after = opportunities
            .iter()
            .filter(|record| {
                us(&record["t_tick_us"]) > resubscribed && record["disposition"] == "delivered"
            })
            .count();
        assert!(after >= 10, "{path}: {after} delivered after the new lease");

        // S's grace keeps the stream over the gap: the new lease reuses it.
        if path == "S" {
            let graces = of_type(&events, "grace");
            assert!(
                graces.iter().any(|grace| grace["exit"] == "reused"),
                "{graces:?}"
            );
            assert!(
                !of_type(&events, "closing")
                    .iter()
                    .any(|closing| closing["reason"] == "grace_expired"),
                "S closed in the gap"
            );
        }

        // The viewer's own ending, in its file and in the events.
        let rows = h.lines("viewer-1.jsonl");
        let last = rows.last().expect("the viewer's file");
        assert_eq!(last["kind"], "viewer_end", "{path}: {last}");
        let frames = rows.iter().filter(|row| row["kind"] == "frame").count();
        assert_eq!(us(&last["frames"]) as usize, frames, "{path}: {last}");
        let ended = of_type(&events, "viewer_ended");
        assert_eq!(ended.len(), 1, "{path}: {ended:?}");
        assert_eq!(ended[0]["viewer_id"], 1);
        assert_eq!(ended[0]["cell"], "CA");
        assert_eq!(ended[0]["how"], "in_process");
    }
}

fn assert_expired_for_good(h: &common::Harness, outcome: &RunOutcome) {
    assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    let events = h.events();
    let expired = the_expiry(&events);
    assert!(
        of_type(&events, "lease_resubscribed").is_empty(),
        "a scripted expiry never subscribes again"
    );
    assert_eq!(of_type(&events, "subscribe").len(), 1);
    assert_eq!(h.json("summary.json")["lease_resubscribed"], 0);
    let after: Vec<Value> = h
        .lines("opps-CA-N.jsonl")
        .into_iter()
        .filter(|record| us(&record["t_tick_us"]) > expired)
        .collect();
    assert!(after.len() >= 5, "{after:?}");
    assert!(
        after
            .iter()
            .all(|record| record["disposition"] == "skipped_no_lease"),
        "{after:?}"
    );
}

#[test]
fn the_scripted_ttl_case_still_expires_and_does_not_subscribe_again() {
    // §4.4 item 5: the viewer deliberately stops renewing.
    let h = harness(
        "ttl",
        "ttl",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let segment = format!("{}\nviewer_stop_renew_ms = 1000", preview("N", 8_000));
    let outcome = h.run(plan("ttl", 10 * HOURS, &cell("CA", 60_000, &[&segment])));
    assert_expired_for_good(&h, &outcome);
}

#[test]
fn a_viewer_scripted_to_stop_renewing_does_not_subscribe_again_even_if_it_learns_of_the_loss() {
    // The stall ends the lease before the scripted stop; the viewer's next renewal is refused.
    let h = harness(
        "ttl-learns",
        "ttl-learns",
        FakeScript::default(),
        FakeCheckScript::default(),
    );
    let segment = format!("{}\nviewer_stop_renew_ms = 7000", preview("N", 10_000));
    let outcome = run_with(
        &h,
        plan("ttl-learns", 10 * HOURS, &cell("CA", 60_000, &[&segment])),
        Arc::new(StallingViewers {
            from_us: 500_000,
            until_us: 5_500_000,
        }),
    );
    assert_expired_for_good(&h, &outcome);
}

type StopGate = (&'static str, fn(&Fixture));

#[test]
fn under_a_stop_gate_the_viewer_stays_without_a_lease() {
    let gates: [StopGate; 4] = [
        ("run-stop", |fx: &Fixture| {
            fx.hub.set_global(&fx.owner, GlobalGate::RunStop, true);
        }),
        ("cell-end", |fx: &Fixture| {
            fx.hub.set_global(&fx.owner, GlobalGate::CellEnd, true);
        }),
        ("instance-stop", |fx: &Fixture| {
            fx.hub.set_gate(&fx.owner, 0, GateKind::StopClass, true);
        }),
        ("no-gate", |_: &Fixture| {}),
    ];
    for (name, gate) in gates {
        let fx = fixture(instant_script(), true);
        let root = fresh_dir(name);
        let clock: Arc<dyn Clock> = fx.clock.clone();
        let journal = Journal::open(&root, clock).expect("the journal opens");
        const VIEWER: u64 = 9;
        let mut viewer = InProcessViewers
            .start(context(&fx.hub, &journal, VIEWER, 3_000_000, 0))
            .expect("the viewer starts");
        fx.clock.sleep_us(3_500_000);
        fx.hub.sweep(&fx.owner, 0);
        assert!(!fx.hub.has_lease(&fx.owner, 0), "{name}: every lease ended");
        gate(&fx);
        viewer.tick(fx.clock.now_us());
        journal.flush();
        let events = read_jsonl(&root.join("events.jsonl"));
        let again = of_type(&events, "lease_resubscribed");
        if name == "no-gate" {
            assert_eq!(again.len(), 1, "{name}: {again:?}");
            assert!(fx.hub.has_lease(&fx.owner, 0), "{name}");
            assert_eq!(fx.hub.lease_resubscribed(), 1, "{name}");
        } else {
            assert!(again.is_empty(), "{name}: {again:?}");
            assert!(!fx.hub.has_lease(&fx.owner, 0), "{name}");
            assert_eq!(fx.hub.lease_resubscribed(), 0, "{name}");
        }
    }
}

// ---- the real child ----

fn context(
    hub: &Arc<Hub>,
    journal: &Arc<Journal>,
    subscriber: u64,
    ttl_us: Micros,
    exit_wait_us: Micros,
) -> ViewerContext {
    ViewerContext {
        hub: Arc::clone(hub),
        journal: Arc::clone(journal),
        instance: 0,
        subscriber,
        ttl_us,
        slow_fps: None,
        stop_renew_at_us: None,
        unsubscribe_at_us: None,
        grace_us: 0,
        gdi: false,
        place: json!({ "cell": "CA", "segment": "CA-r1-x", "round": 1, "path": "N", "orientation": "L" }),
        exit_wait_us,
    }
}

struct RealChild {
    hub: Arc<Hub>,
    journal: Arc<Journal>,
    root: PathBuf,
    owner: Arc<LockOwner>,
    viewers: ChildViewers,
}

fn real_child(name: &str) -> RealChild {
    let root = fresh_dir(name);
    let clock: Arc<dyn Clock> = Arc::new(SystemClock::new());
    let hub = Arc::new(Hub::new(
        Arc::clone(&clock),
        &["E1".to_owned()],
        7,
        BudgetConfig::default(),
        Arc::new(MemorySink::default()),
    ));
    let journal = Journal::open(&root, clock).expect("the journal opens");
    RealChild {
        hub,
        journal,
        owner: LockOwner::new("test"),
        viewers: ChildViewers {
            executable: PathBuf::from(env!("CARGO_BIN_EXE_ac-capture-bench")),
            run_dir: root.clone(),
            job: None,
        },
        root,
    }
}

impl RealChild {
    fn publish(&self, sequence: u64) {
        self.hub.views.publish(PreviewFrame {
            instance: 0,
            path: PathKind::Nemu,
            owner_epoch: 7,
            generation: 1,
            preview_sequence: sequence,
            grant_sequence: sequence,
            width: 2,
            height: 2,
            layout: PixelLayout::Rgba8,
            transform: "identity",
            sample_start_us: 0,
            sample_end_us: 0,
            published_us: self.hub.clock.now_us(),
            stream_sequence: None,
            device_pts_us: None,
            received_us: None,
            validity: Validity::Live,
            pixels: Arc::new(vec![0_u8; 16]),
        });
    }

    fn ended_events(&self) -> Vec<Value> {
        self.journal.flush();
        of_type(&read_jsonl(&self.root.join("events.jsonl")), "viewer_ended")
    }
}

/// A generous wait: only a child that does not end by itself meets it, and that fails the test.
const EXIT_WAIT_US: Micros = 60_000_000;

#[test]
fn stopping_a_real_child_right_after_its_last_frame_keeps_the_tail_and_the_last_record() {
    let child = real_child("child-tail");
    let mut viewer = child
        .viewers
        .start(context(
            &child.hub,
            &child.journal,
            1,
            3_000_000,
            EXIT_WAIT_US,
        ))
        .expect("the child starts");
    child.publish(1);
    // D2c's stop killed the child here, before it could flush.
    viewer.stop(true);
    let rows = read_jsonl(&child.root.join("viewer-1.jsonl"));
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0]["kind"], "frame", "{rows:?}");
    assert_eq!(rows[0]["preview_seq"], 1, "{rows:?}");
    assert_eq!(rows[1]["kind"], "viewer_end", "{rows:?}");
    assert_eq!(rows[1]["reason"], "stdin_closed", "{rows:?}");
    assert_eq!(rows[1]["frames"], 1, "{rows:?}");
    let ended = child.ended_events();
    assert_eq!(ended.len(), 1, "{ended:?}");
    assert_eq!(ended[0]["viewer_id"], 1, "{ended:?}");
    assert_eq!(ended[0]["how"], "exited", "{ended:?}");
    assert_eq!(ended[0]["exit_code"], 0, "{ended:?}");
    assert_eq!(ended[0]["cell"], "CA", "{ended:?}");
}

#[test]
fn a_child_kept_past_a_lost_lease_gets_the_new_leases_frames() {
    let child = real_child("child-lease");
    let mut viewer = child
        .viewers
        .start(context(
            &child.hub,
            &child.journal,
            1,
            300_000,
            EXIT_WAIT_US,
        ))
        .expect("the child starts");
    // The lease goes as a sweep ends it: removed, the mailbox closed. The writer must not end the
    // child for that; the child's next renewal is refused and a new lease follows.
    let lease = child
        .hub
        .snapshot(&child.owner, |state| {
            state.instances[0].leases.keys().next().copied()
        })
        .expect("the viewer's lease");
    child.hub.unsubscribe(&child.owner, 0, lease, 0);
    let started = std::time::Instant::now();
    while !child.hub.has_lease(&child.owner, 0) {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(60),
            "no new lease"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    child.publish(1);
    viewer.stop(true);
    assert_eq!(child.hub.lease_resubscribed(), 1);
    child.journal.flush();
    let events = read_jsonl(&child.root.join("events.jsonl"));
    let again = of_type(&events, "lease_resubscribed");
    assert_eq!(again.len(), 1, "{again:?}");
    assert_eq!(again[0]["viewer_id"], 1, "{again:?}");
    let rows = read_jsonl(&child.root.join("viewer-1.jsonl"));
    let last = rows.last().expect("the child's file");
    assert_eq!(last["kind"], "viewer_end", "{rows:?}");
    assert_eq!(last["frames"], 1, "the frame after the new lease: {rows:?}");
    let ended = child.ended_events();
    assert_eq!(ended.len(), 1, "{ended:?}");
    assert_eq!(ended[0]["how"], "exited", "{ended:?}");
    assert_eq!(ended[0]["exit_code"], 0, "{ended:?}");
}
