// SPDX-License-Identifier: AGPL-3.0-only

//! #503 MEM (0.12.102; register §1.556; `contracts/emulator-control.md`, "Commit headroom and
//! spacing"): an automatic emulator start waits, with the start turn and its key in hand, until the
//! host's available commit is at or above `emulator_control.start_commit_reserve_bytes` and
//! `emulator_control.start_spacing_ms` has passed since the latest emulator Start of any kind
//! returned. A person's, an agent's or the Lab's control, any stop and the ladder's restart rung
//! are never held; their Starts only stamp the spacing.
//!
//! Every host here takes the commit seam (`with_commit_sample_for_test`: a scripted read, each read
//! also sent to the test), the Runtime's manual clock, internet access and instances
//! emulator-controlled from their start (`x_state`). The spacing is read from the Runtime's own
//! stamp of the latest Start (`emulator_start_returned_for_test`): a Start the gate let through too
//! early would leave an earlier stamp. Every wait returns as soon as what it waits for is read,
//! reported or recorded; `GATE_WAIT` only bounds a broken run. No clock is read in this file.

use super::gate_369_s6c::{
    GATE_WAIT, connectivity, control, instance_scope, of_request, pause, send, stop,
};
use super::gate_idle1p::{emulator_starts, stops, x_state};
use super::instance_workers::{ladder_finished, wait_until};
use super::*;
use crate::codes::HostCode;
use crate::host::{CommitReadForTest, CommitSampleForTest};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    ConfigSettingChange, EmulatorInstanceAction, RecoveryDrillMode, RuntimeConfigManifest,
    RuntimePayload,
};
use actingcommand_host_metrics::{CommitSample, HostMetricsCode, NetworkConnectivity};
use actingcommand_runtime_config::emulator_control::{
    START_COMMIT_RESERVE_BYTES, START_SPACING_MS,
};
use actingcommand_runtime_config::{
    DefaultsFile, START_EMULATOR, annotate_settable, instance_key, mechanism_rows,
    render_parameters,
};
use serde_json::{Value, json};

const A: &str = "mem.instance.a";
const B: &str = "mem.instance.b";
const C: &str = "mem.instance.c";
const GIB: u64 = 1 << 30;
/// The scripted commit limit.
const LIMIT: u64 = 100 * GIB;
/// `emulator_control.start_commit_reserve_bytes`'s default.
const RESERVE: u64 = 17_179_869_184;
/// `emulator_control.start_spacing_ms`'s default.
const SPACING_MS: u64 = 30_000;
/// How far the manual clock moves between a Start and the next Stop of the same emulator, as a
/// real host's clock moves: each records the instance's `device.connected` at the clock, and a
/// second record of one key at the same millisecond is refused as not newer
/// (`runtime_fact_stale`).
const LATER_MS: u64 = 1_000;
/// The gate's three codes.
const GATE_CODES: [HostCode; 3] = [
    HostCode::EmulatorAutostartCommitShort,
    HostCode::EmulatorAutostartCommitReady,
    HostCode::EmulatorAutostartCommitUnknown,
];

/// The commit seam: the reads of a script in order, then `fallback`'s value of `LIMIT`, or a
/// failed read (`performance_memory_status_failed`) while `failing`; each read is also sent to
/// `reads`.
struct ScriptedCommit {
    fallback: Arc<AtomicU64>,
    failing: Arc<AtomicBool>,
    reads: mpsc::Receiver<CommitReadForTest>,
    seam: CommitSampleForTest,
}

fn scripted_commit(script: Vec<u64>, fallback: u64) -> ScriptedCommit {
    let script = std::sync::Mutex::new(VecDeque::from(script));
    let fallback = Arc::new(AtomicU64::new(fallback));
    let failing = Arc::new(AtomicBool::new(false));
    let (sender, reads) = mpsc::channel();
    let sender = std::sync::Mutex::new(sender);
    let (then, fails) = (Arc::clone(&fallback), Arc::clone(&failing));
    let seam: CommitSampleForTest = Arc::new(move || {
        let read = if fails.load(Ordering::Acquire) {
            Err(HostMetricsCode::PerformanceMemoryStatusFailed.code())
        } else {
            let bytes = script
                .lock()
                .expect("commit script")
                .pop_front()
                .unwrap_or_else(|| then.load(Ordering::Acquire));
            Ok(CommitSample {
                limit_bytes: LIMIT,
                available_bytes: bytes,
            })
        };
        let _ = sender.lock().expect("commit reads").send(read);
        read
    });
    ScriptedCommit {
        fallback,
        failing,
        reads,
        seam,
    }
}

/// The next commit read the gate took.
fn next_read(commit: &ScriptedCommit) -> CommitReadForTest {
    commit.reads.recv_timeout(GATE_WAIT).expect("a commit read")
}

/// The available commit of a read that succeeded.
fn available(read: CommitReadForTest) -> u64 {
    read.expect("a commit sample").available_bytes
}

/// A started host of `instances` on `clock`, with the commit seam of `commit` and internet access;
/// the instances `starting` name `start_emulator`, in this order; `configure` adds to the
/// configuration.
fn mem_host(
    root: &TempDir,
    instances: &[(&str, InstanceId, &Arc<FakeState>)],
    starting: &[&str],
    clock: &Arc<ManualRuntimeClock>,
    commit: &ScriptedCommit,
    configure: impl FnOnce(RuntimeHostConfig) -> RuntimeHostConfig,
) -> RuntimeHost {
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let entries = instances
        .iter()
        .map(|(alias, instance, state)| ((*alias).to_owned(), *instance, Arc::clone(state)))
        .collect::<Vec<_>>();
    RuntimeHost::start(
        configure(
            config(root)
                .with_runtime_clock(clock.clone())
                .with_emulator_autostart(starting.iter().map(|alias| (*alias).to_owned()).collect())
                .with_network_connectivity_for_test(read)
                .with_commit_sample_for_test(Arc::clone(&commit.seam)),
        ),
        Arc::new(FakeProvider::from_entries(entries)),
    )
    .expect("runtime host")
}

/// The configuration actingd assembles over the defaults file from `file` for the instances `ids`.
fn manifest(ids: &[String], file: &Value) -> RuntimeConfigManifest {
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let mut parameters = render_parameters(&mechanism_rows(), ids, &defaults, file, &|_| None)
        .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

fn id_text(instance: InstanceId) -> String {
    match serde_json::to_value(instance).expect("instance id encodes") {
        Value::String(text) => text,
        other => panic!("an instance id is a string: {other}"),
    }
}

/// One `ConfigureSettings` of `key` to `value`, as `actingctl config set` sends it; completed.
fn configure_key(host: &RuntimeHost, key: &str, value: Value, connection: u64) {
    let receipt = send(
        host,
        RuntimeOperation::ConfigureSettings {
            changes: vec![ConfigSettingChange {
                key: key.to_owned(),
                value: Some(value),
            }],
            expected_version: None,
        },
        connection,
    );
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
}

/// The report lines once `done` holds for them (none is taken).
fn await_lines(host: &RuntimeHost, done: impl Fn(&[Outcome]) -> bool) -> Vec<Outcome> {
    host.emulator_autostart_reports()
        .await_lines_for_test(done, GATE_WAIT)
        .expect("the awaited report lines")
}

fn count(lines: &[Outcome], code: HostCode) -> usize {
    lines.iter().filter(|line| line.is(code)).count()
}

fn of_alias(line: &Outcome, code: HostCode, alias: &str) -> bool {
    line.is(code) && line.value("alias") == Some(&json!(alias))
}

/// Whether the configured start of `alias` was reported started.
fn started(lines: &[Outcome], alias: &str) -> bool {
    lines
        .iter()
        .any(|line| of_alias(line, HostCode::EmulatorAutostartStarted, alias))
}

/// The report lines once the configured start of `alias` was reported started.
fn await_started(host: &RuntimeHost, alias: &str) -> Vec<Outcome> {
    await_lines(host, |lines| started(lines, alias))
}

/// The one line of `code` among `lines`.
fn the_one(lines: &[Outcome], code: HostCode) -> Outcome {
    let found = lines
        .iter()
        .filter(|line| line.is(code))
        .cloned()
        .collect::<Vec<_>>();
    let [one] = &found[..] else {
        panic!("one {}: {lines:?}", code.as_str());
    };
    one.clone()
}

/// The gate's lines among `lines`.
fn gate_lines(lines: &[Outcome]) -> usize {
    GATE_CODES.iter().map(|code| count(lines, *code)).sum()
}

/// When the latest emulator Start returned, as the spacing reads it.
fn stamp(host: &RuntimeHost) -> Option<u64> {
    host.emulator_start_returned_for_test()
        .expect("the start gate's memory")
}

/// One `runtime.failed` record of the gate.
struct GateRecord {
    instance: Option<InstanceId>,
    severity: EventSeverity,
    outcome: Outcome,
}

/// The gate's `runtime.failed` records, in ledger order.
fn gate_records(host: &RuntimeHost) -> Vec<GateRecord> {
    host.query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events")
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(record))
                if GATE_CODES.iter().any(|code| record.outcome().is(*code)) =>
            {
                Some(GateRecord {
                    instance: event.links().instance_id().copied(),
                    severity: event.severity(),
                    outcome: record.outcome().clone(),
                })
            }
            _ => None,
        })
        .collect()
}

// A day change: three configured instances are due at once. The first starts on ample headroom;
// its emulator takes the headroom, and the next one, first in line, holds the turn and its key
// with one `emulator_autostart_commit_short` for the whole episode while the third waits behind
// it; neither starts. The headroom comes back (as when an idle action stops an emulator): the
// next one still waits out the spacing after the first Start returned, then goes on with one
// `emulator_autostart_commit_ready` and starts; the third starts no sooner than the spacing after
// it. Both lines are also `runtime.failed` records under the instance first in line.
#[test]
fn mem_1_a_day_change_holds_the_one_in_line_and_those_behind_it_with_one_short_for_the_episode() {
    let root = TempDir::new().expect("tempdir");
    let (a, b, c) = (instance_id(), instance_id(), instance_id());
    let (a_state, b_state, c_state) = (x_state(), x_state(), x_state());
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let commit = scripted_commit(vec![24 * GIB], 8 * GIB);
    let host = mem_host(
        &root,
        &[(A, a, &a_state), (B, b, &b_state), (C, c, &c_state)],
        &[A, B, C],
        &clock,
        &commit,
        |config| config,
    );
    stop(&host, A, 5_001);
    stop(&host, B, 5_002);
    stop(&host, C, 5_003);
    assert_eq!(stamp(&host), None, "no Start yet");

    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 3);
    assert_eq!(available(next_read(&commit)), 24 * GIB, "A's read");
    await_started(&host, A);
    assert_eq!(stamp(&host), Some(0), "A's Start returned at 0");

    // B, first in line, reads the headroom short and holds the turn; C waits behind it.
    assert_eq!(available(next_read(&commit)), 8 * GIB, "B's first read");
    let lines = await_lines(&host, |lines| {
        count(lines, HostCode::EmulatorAutostartCommitShort) == 1
    });
    let short = the_one(&lines, HostCode::EmulatorAutostartCommitShort);
    assert_eq!(short.value("alias"), Some(&json!(B)));
    assert_eq!(short.value("commit_available_bytes"), Some(&json!(8 * GIB)));
    assert_eq!(short.value("commit_limit_bytes"), Some(&json!(LIMIT)));
    assert_eq!(short.value("commit_reserve_bytes"), Some(&json!(RESERVE)));
    assert_eq!(short.value("idle_action"), None);
    clock.advance(5_000);
    assert_eq!(available(next_read(&commit)), 8 * GIB, "B's second read");
    assert_eq!(emulator_starts(&b_state), 0);
    assert_eq!(emulator_starts(&c_state), 0);

    // The headroom is back at 10 s: the spacing after A's Start still holds B until 30 s.
    commit.fallback.store(24 * GIB, Ordering::Release);
    clock.advance(5_000);
    assert_eq!(available(next_read(&commit)), 24 * GIB, "B's read at 10 s");
    clock.advance(20_000);
    assert_eq!(available(next_read(&commit)), 24 * GIB, "B's read at 30 s");
    let lines = await_started(&host, B);
    assert_eq!(stamp(&host), Some(SPACING_MS), "B's Start returned at 30 s");
    let ready = the_one(&lines, HostCode::EmulatorAutostartCommitReady);
    assert_eq!(ready.value("alias"), Some(&json!(B)));
    assert_eq!(ready.value("elapsed_ms"), Some(&json!(SPACING_MS)));
    assert_eq!(
        ready.value("commit_available_bytes"),
        Some(&json!(24 * GIB))
    );
    assert_eq!(ready.value("commit_reserve_bytes"), Some(&json!(RESERVE)));

    // C, now in line, waits out the spacing after B's Start.
    assert_eq!(available(next_read(&commit)), 24 * GIB, "C's read at 30 s");
    assert_eq!(emulator_starts(&c_state), 0);
    clock.advance(SPACING_MS);
    let lines = await_started(&host, C);
    assert_eq!(
        stamp(&host),
        Some(2 * SPACING_MS),
        "C's Start returned at 60 s"
    );
    assert_eq!(count(&lines, HostCode::EmulatorAutostartCommitShort), 1);
    assert_eq!(count(&lines, HostCode::EmulatorAutostartCommitReady), 1);
    assert_eq!(count(&lines, HostCode::EmulatorAutostartCommitUnknown), 0);
    for state in [&a_state, &b_state, &c_state] {
        assert_eq!(emulator_starts(state), 1);
    }
    let records = gate_records(&host);
    let [short_record, ready_record] = &records[..] else {
        panic!("a short and a ready record: {}", records.len());
    };
    assert!(
        short_record
            .outcome
            .is(HostCode::EmulatorAutostartCommitShort)
    );
    assert_eq!(
        (short_record.instance, short_record.severity),
        (Some(b), EventSeverity::Warning)
    );
    assert!(
        ready_record
            .outcome
            .is(HostCode::EmulatorAutostartCommitReady)
    );
    assert_eq!(
        (ready_record.instance, ready_record.severity),
        (Some(b), EventSeverity::Info)
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// The spacing after a person's start: with ample headroom, an automatic start whose turn comes
// right after a person started the instance's emulator (and stopped it a second later) waits until
// 30 s after that Start returned, held at 29 999 ms and started at 30 000 ms, and the spacing alone
// tells nothing.
#[test]
fn mem_2_the_spacing_holds_an_automatic_start_after_a_persons_start_for_its_length() {
    let root = TempDir::new().expect("tempdir");
    let a = instance_id();
    let a_state = x_state();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let commit = scripted_commit(Vec::new(), 32 * GIB);
    let host = mem_host(
        &root,
        &[(A, a, &a_state)],
        &[A],
        &clock,
        &commit,
        |config| config,
    );
    stop(&host, A, 5_011);
    let started_by_hand = control(&host, A, EmulatorInstanceAction::Start, 5_012);
    assert_eq!(
        started_by_hand.state(),
        RuntimeReceiptState::Completed,
        "{started_by_hand:?}"
    );
    assert_eq!(stamp(&host), Some(0), "the person's Start returned at 0");
    clock.advance(LATER_MS);
    stop(&host, A, 5_013);

    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    assert_eq!(available(next_read(&commit)), 32 * GIB, "the read at 1 s");
    clock.advance(SPACING_MS - 1 - LATER_MS);
    assert_eq!(
        available(next_read(&commit)),
        32 * GIB,
        "the read at 29 999 ms"
    );
    clock.advance(1);
    let lines = await_started(&host, A);
    assert_eq!(
        stamp(&host),
        Some(SPACING_MS),
        "the automatic Start at 30 s"
    );
    assert_eq!(
        emulator_starts(&a_state),
        2,
        "the person's and the automatic one"
    );
    assert_eq!(gate_lines(&lines), 0, "the spacing alone tells nothing");
    assert!(gate_records(&host).is_empty());
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// Never held: while A's automatic start waits for headroom, a person's stop and start of B, a
// device drill of B's ladder (its emulator-restart rung's Stop and Start) and a person's start of
// A itself are driven at once; each Start stamps the spacing. A's claim, still waiting after them,
// yields to the person's control of A as it yields today.
#[test]
fn mem_3_a_persons_start_a_stop_and_the_ladders_rung_are_never_held_with_the_headroom_short() {
    let root = TempDir::new().expect("tempdir");
    let (a, b) = (instance_id(), instance_id());
    let (a_state, b_state) = (x_state(), x_state());
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let commit = scripted_commit(Vec::new(), 8 * GIB);
    let host = mem_host(
        &root,
        &[(A, a, &a_state), (B, b, &b_state)],
        &[A],
        &clock,
        &commit,
        |config| config,
    );
    stop(&host, A, 5_021);
    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    assert_eq!(available(next_read(&commit)), 8 * GIB, "A's first read");
    await_lines(&host, |lines| {
        lines
            .iter()
            .any(|line| of_alias(line, HostCode::EmulatorAutostartCommitShort, A))
    });

    stop(&host, B, 5_022);
    let started_by_hand = control(&host, B, EmulatorInstanceAction::Start, 5_023);
    assert_eq!(
        started_by_hand.state(),
        RuntimeReceiptState::Completed,
        "{started_by_hand:?}"
    );
    assert_eq!(emulator_starts(&b_state), 1, "the person's Start of B");
    assert_eq!(stamp(&host), Some(0));
    // The drill comes a second later (`LATER_MS`): its rung's Stop records B's `device.connected`
    // after the person's Start did. Waited out as the drill gates wait for a drill: its
    // `recovery_ladder_finished` recorded and its key released.
    clock.advance(LATER_MS);
    let receipt = send(
        &host,
        RuntimeOperation::DrillRecoveryLadder {
            instance_alias: B.to_owned(),
            mode: RecoveryDrillMode::Device,
        },
        5_024,
    );
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let Some(RuntimeResult::RecoveryLadderDrillQueued { drill }) = receipt.result() else {
        panic!("a drill's receipt names the drill: {receipt:?}");
    };
    let ladder = drill.ladder_request_id;
    wait_until("the drill's end and its release", || {
        ladder_finished(&of_request(&host, ladder)).is_some()
            && host
                .instance_claims_for_test(B)
                .expect("instance claims")
                .0
                .is_none()
    });
    let ended =
        ladder_finished(&of_request(&host, ladder)).map(|(_, result, rungs)| (result, rungs));
    assert_eq!(
        (stops(&b_state), emulator_starts(&b_state)),
        (2, 2),
        "the person's Stop and Start of B, then the rung's: {ended:?}"
    );
    assert_eq!(
        stamp(&host),
        Some(LATER_MS),
        "the rung's Start stamped the spacing"
    );

    clock.advance(5_000);
    assert_eq!(available(next_read(&commit)), 8 * GIB, "A still waits");
    assert_eq!(emulator_starts(&a_state), 0);
    let started_by_hand = control(&host, A, EmulatorInstanceAction::Start, 5_025);
    assert_eq!(
        started_by_hand.state(),
        RuntimeReceiptState::Completed,
        "{started_by_hand:?}"
    );
    let lines = await_lines(&host, |lines| {
        lines
            .iter()
            .any(|line| of_alias(line, HostCode::EmulatorAutostartSuperseded, A))
    });
    assert_eq!(emulator_starts(&a_state), 1, "the person's Start of A");
    assert_eq!(stamp(&host), Some(LATER_MS + 5_000));
    assert_eq!(count(&lines, HostCode::EmulatorAutostartCommitShort), 1);
    assert_eq!(count(&lines, HostCode::EmulatorAutostartStarted), 0);
    assert_eq!(gate_records(&host).len(), 1, "the short record alone");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// A pause and a shutdown end the wait as they end a hold today: A's instance pause ends A's
// waiting claim as held (`instance_paused`), starting nothing; B, next in line, takes the turn and
// waits in the same short episode with no second short line; the host's close (a drain) ends B's
// wait as interrupted, starting nothing.
#[test]
fn mem_4_a_pause_and_a_shutdown_end_the_wait_as_they_end_a_hold_today() {
    let root = TempDir::new().expect("tempdir");
    let (a, b) = (instance_id(), instance_id());
    let (a_state, b_state) = (x_state(), x_state());
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let commit = scripted_commit(Vec::new(), 8 * GIB);
    let host = mem_host(
        &root,
        &[(A, a, &a_state), (B, b, &b_state)],
        &[A, B],
        &clock,
        &commit,
        |config| config,
    );
    stop(&host, A, 5_031);
    stop(&host, B, 5_032);
    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 2);
    assert_eq!(available(next_read(&commit)), 8 * GIB, "A's read");

    pause(&host, instance_scope(A), 5_033);
    let lines = await_lines(&host, |lines| {
        lines.iter().any(|line| {
            of_alias(line, HostCode::EmulatorAutostartSkipped, A)
                && line.value("skip_reason") == Some(&json!("instance_paused"))
        })
    });
    let short = the_one(&lines, HostCode::EmulatorAutostartCommitShort);
    assert_eq!(short.value("alias"), Some(&json!(A)));
    wait_until("A's key released", || {
        host.instance_claims_for_test(A)
            .expect("instance claims")
            .0
            .is_none()
    });

    assert_eq!(available(next_read(&commit)), 8 * GIB, "B's read");
    clock.advance(5_000);
    assert_eq!(available(next_read(&commit)), 8 * GIB, "B's second read");
    let reports = host.emulator_autostart_reports();
    host.close().expect("close host");
    let lines = reports
        .await_lines_for_test(
            |lines| {
                lines
                    .iter()
                    .any(|line| of_alias(line, HostCode::EmulatorAutostartInterrupted, B))
            },
            GATE_WAIT,
        )
        .expect("B's wait interrupted");
    assert_eq!(count(&lines, HostCode::EmulatorAutostartCommitShort), 1);
    assert_eq!(count(&lines, HostCode::EmulatorAutostartStarted), 0);
    assert_eq!(emulator_starts(&a_state), 0);
    assert_eq!(emulator_starts(&b_state), 0);
}

// `start_emulator` turned off ends the wait as it ends a hold today: A's waiting claim releases its
// key, starts nothing and says `start_emulator_off`.
#[test]
fn mem_5_start_emulator_turned_off_ends_the_wait() {
    let root = TempDir::new().expect("tempdir");
    let a = instance_id();
    let a_state = x_state();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let commit = scripted_commit(Vec::new(), 8 * GIB);
    let entry = json!({"instance_id": id_text(a), "stuck_recovery": false, "start_emulator": true});
    let host = mem_host(
        &root,
        &[(A, a, &a_state)],
        &[A],
        &clock,
        &commit,
        |config| {
            config.with_config_manifest(manifest(&[id_text(a)], &json!({ "instances": [entry] })))
        },
    );
    stop(&host, A, 5_041);
    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    assert_eq!(available(next_read(&commit)), 8 * GIB, "A's read");

    configure_key(
        &host,
        &instance_key(&id_text(a), START_EMULATOR),
        json!(false),
        5_042,
    );
    let lines = await_lines(&host, |lines| {
        lines.iter().any(|line| {
            of_alias(line, HostCode::EmulatorAutostartSkipped, A)
                && line.value("skip_reason") == Some(&json!("start_emulator_off"))
        })
    });
    assert_eq!(count(&lines, HostCode::EmulatorAutostartCommitShort), 1);
    assert_eq!(count(&lines, HostCode::EmulatorAutostartStarted), 0);
    assert_eq!(emulator_starts(&a_state), 0);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// The reserve 0 turns the headroom check off: with a short headroom an automatic start goes on at
// once, reads no commit and tells nothing.
#[test]
fn mem_6_the_reserve_0_never_holds_and_reads_nothing() {
    let root = TempDir::new().expect("tempdir");
    let a = instance_id();
    let a_state = x_state();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let commit = scripted_commit(Vec::new(), GIB);
    let file = json!({"emulator_control": {"start_commit_reserve_bytes": 0}});
    let host = mem_host(
        &root,
        &[(A, a, &a_state)],
        &[A],
        &clock,
        &commit,
        |config| config.with_config_manifest(manifest(&[], &file)),
    );
    stop(&host, A, 5_051);
    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    let lines = await_started(&host, A);
    assert!(commit.reads.try_recv().is_err(), "no commit read");
    assert_eq!(gate_lines(&lines), 0);
    assert_eq!(emulator_starts(&a_state), 1);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// A configuration whose file names neither key holds at the defaults: a byte under 16 GiB is short,
// exactly 16 GiB goes on, and the next automatic start waits 30 s after the first Start returned
// (held at 29 999 ms, started at 30 000 ms after it).
#[test]
fn mem_7_a_configuration_without_the_keys_holds_at_the_defaults() {
    let root = TempDir::new().expect("tempdir");
    let (a, b) = (instance_id(), instance_id());
    let (a_state, b_state) = (x_state(), x_state());
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let commit = scripted_commit(vec![RESERVE - 1], RESERVE);
    let host = mem_host(
        &root,
        &[(A, a, &a_state), (B, b, &b_state)],
        &[A, B],
        &clock,
        &commit,
        |config| config.with_config_manifest(manifest(&[], &json!({}))),
    );
    stop(&host, A, 5_061);
    stop(&host, B, 5_062);
    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 2);
    assert_eq!(available(next_read(&commit)), RESERVE - 1, "A's first read");
    let lines = await_lines(&host, |lines| {
        count(lines, HostCode::EmulatorAutostartCommitShort) == 1
    });
    let short = the_one(&lines, HostCode::EmulatorAutostartCommitShort);
    assert_eq!(short.value("commit_reserve_bytes"), Some(&json!(RESERVE)));
    assert_eq!(
        short.value("commit_available_bytes"),
        Some(&json!(RESERVE - 1))
    );

    clock.advance(5_000);
    assert_eq!(available(next_read(&commit)), RESERVE, "A's read at 5 s");
    let lines = await_started(&host, A);
    assert_eq!(stamp(&host), Some(5_000));
    let ready = the_one(&lines, HostCode::EmulatorAutostartCommitReady);
    assert_eq!(ready.value("elapsed_ms"), Some(&json!(5_000)));
    assert_eq!(ready.value("commit_available_bytes"), Some(&json!(RESERVE)));

    assert_eq!(available(next_read(&commit)), RESERVE, "B's read at 5 s");
    clock.advance(SPACING_MS - 1);
    assert_eq!(
        available(next_read(&commit)),
        RESERVE,
        "B's read at 34 999 ms"
    );
    clock.advance(1);
    await_started(&host, B);
    assert_eq!(stamp(&host), Some(5_000 + SPACING_MS));
    assert_eq!(emulator_starts(&a_state), 1);
    assert_eq!(emulator_starts(&b_state), 1);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// Both keys are set while the Runtime runs, as `actingctl config set` sends them, and take effect
// at the next tick of the waiting claim, without a restart: a lower reserve lets A go on with the
// read it has (its ready line names the new reserve), and a spacing of 0 lets B go on at once.
#[test]
fn mem_8_both_keys_set_by_an_instruction_take_effect_at_the_next_tick() {
    let root = TempDir::new().expect("tempdir");
    let (a, b) = (instance_id(), instance_id());
    let (a_state, b_state) = (x_state(), x_state());
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let commit = scripted_commit(Vec::new(), 8 * GIB);
    let host = mem_host(
        &root,
        &[(A, a, &a_state), (B, b, &b_state)],
        &[A, B],
        &clock,
        &commit,
        |config| config.with_config_manifest(manifest(&[], &json!({}))),
    );
    stop(&host, A, 5_071);
    stop(&host, B, 5_072);
    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 2);
    assert_eq!(available(next_read(&commit)), 8 * GIB, "A's first read");
    clock.advance(5_000);
    assert_eq!(available(next_read(&commit)), 8 * GIB, "A's second read");
    assert_eq!(emulator_starts(&a_state), 0);

    configure_key(&host, START_COMMIT_RESERVE_BYTES, json!(4 * GIB), 5_073);
    let lines = await_started(&host, A);
    assert_eq!(stamp(&host), Some(5_000));
    let short = the_one(&lines, HostCode::EmulatorAutostartCommitShort);
    assert_eq!(short.value("commit_reserve_bytes"), Some(&json!(RESERVE)));
    let ready = the_one(&lines, HostCode::EmulatorAutostartCommitReady);
    assert_eq!(ready.value("commit_reserve_bytes"), Some(&json!(4 * GIB)));
    assert_eq!(ready.value("elapsed_ms"), Some(&json!(5_000)));

    assert_eq!(available(next_read(&commit)), 8 * GIB, "B's first read");
    clock.advance(5_000);
    assert_eq!(available(next_read(&commit)), 8 * GIB, "B's second read");
    assert_eq!(emulator_starts(&b_state), 0);
    configure_key(&host, START_SPACING_MS, json!(0), 5_074);
    await_started(&host, B);
    assert_eq!(stamp(&host), Some(10_000), "B at once, 5 s after A");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// A commit read that fails lets the start go on, says so once per failing streak
// (`emulator_autostart_commit_unknown` with the read's code) and leaves the spacing in force.
#[test]
fn mem_9_a_failing_read_starts_and_says_so_once() {
    let root = TempDir::new().expect("tempdir");
    let (a, b) = (instance_id(), instance_id());
    let (a_state, b_state) = (x_state(), x_state());
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let commit = scripted_commit(Vec::new(), 32 * GIB);
    commit.failing.store(true, Ordering::Release);
    let host = mem_host(
        &root,
        &[(A, a, &a_state), (B, b, &b_state)],
        &[A, B],
        &clock,
        &commit,
        |config| config,
    );
    stop(&host, A, 5_081);
    stop(&host, B, 5_082);
    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 2);
    assert!(next_read(&commit).is_err(), "A's read fails");
    let lines = await_started(&host, A);
    assert_eq!(stamp(&host), Some(0));
    let unknown = the_one(&lines, HostCode::EmulatorAutostartCommitUnknown);
    assert_eq!(unknown.value("alias"), Some(&json!(A)));
    assert_eq!(
        unknown.value("failure_code"),
        Some(&json!(
            HostMetricsCode::PerformanceMemoryStatusFailed
                .code()
                .as_str()
        ))
    );
    assert_eq!(unknown.value("commit_reserve_bytes"), Some(&json!(RESERVE)));

    assert!(next_read(&commit).is_err(), "B's read fails");
    clock.advance(SPACING_MS - 1);
    assert!(next_read(&commit).is_err(), "B's read at 29 999 ms");
    clock.advance(1);
    let lines = await_started(&host, B);
    assert_eq!(stamp(&host), Some(SPACING_MS));
    assert_eq!(count(&lines, HostCode::EmulatorAutostartCommitUnknown), 1);
    assert_eq!(count(&lines, HostCode::EmulatorAutostartCommitShort), 0);
    let records = gate_records(&host);
    let [record] = &records[..] else {
        panic!("one unknown record: {}", records.len());
    };
    assert!(record.outcome.is(HostCode::EmulatorAutostartCommitUnknown));
    assert_eq!(
        (record.instance, record.severity),
        (Some(a), EventSeverity::Warning)
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
