// SPDX-License-Identifier: AGPL-3.0-only

// WATCH-P (0.12.32, Alice's ruling 112: 「暂停中的实例，监视只看状态、不重启、不识别绑定，和「玩家持有」
// 一样；恢复时照常检查」; production 10-29 21:35 and 21:38: the watch restarted an instance twice while it
// was paused for a person's own check). An instance its own scheduling pause holds, in any stage, is
// polled and recorded, but the watch requests no restart (R3) and no recognition (EMU-BIND) for it;
// the skip is `emulator_autostart_skipped` with `skip_reason` `instance_paused`, once per pause
// revision and kind, and counts against no bound. The resume adds nothing: the next poll restarts or
// recognizes as today. On the scripted emulator (HOST-I1), a scripted connectivity status and the
// Runtime's manual clock, with the S6c helpers. Every wait returns as soon as what it waits for is
// polled or reported; `GATE_WAIT` only bounds a broken run.

use super::emulator_controller::{
    EmulatorCall, FAKE_EMULATOR_PORT, FakeEmulatorController, running_state, stopped_state,
};
use super::gate_369_s6c::{
    GATE_WAIT, START, autostart_host, claims_after, connectivity, head, instance_scope, pause,
    resume,
};
use super::instance_workers::phases;
use super::*;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{InstancePauseStage, RuntimeLifecyclePhase};
use actingcommand_host_metrics::NetworkConnectivity;

/// R2: the default polling interval (`emulator_control.watch_interval_ms`).
const WATCH_INTERVAL_MS: u64 = 60_000;
/// R3: the default restart bound (`emulator_control.restart_attempts`).
const RESTART_ATTEMPTS: u64 = 3;
const FIRST: &str = "watch_p.first";
const SECOND: &str = "watch_p.second";
/// The stages one pause revision passes through in the polls below, one per poll: one poll more
/// than the restart bound allows.
const STAGES: [InstancePauseStage; 4] = [
    InstancePauseStage::Released,
    InstancePauseStage::Draining,
    InstancePauseStage::Paused,
    InstancePauseStage::Released,
];

// An emulator whose process runs but whose Android has not started and which reports no port.
fn booting_state() -> crate::InstanceState {
    crate::InstanceState {
        process_started: true,
        android_started: false,
        adb_port: None,
        player_state: None,
        launch_err_code: 0,
        launch_err_msg: String::new(),
    }
}

// A discovery-bound fake whose emulator was stopped when the host started: the Runtime holds it
// pending, and its opens are refused until a port is bound (#411 A48).
fn pending_state() -> Arc<FakeState> {
    let state = Arc::new(FakeState {
        emulator: FakeEmulatorController::pending(),
        ..FakeState::default()
    });
    state.emulator_control.store(true, Ordering::Release);
    state.refuse_pending_opens.store(true, Ordering::Release);
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    state.observed_open.store(true, Ordering::Release);
    state
}

fn reported(lines: &[Outcome], code: &str) -> usize {
    lines
        .iter()
        .filter(|line| line.code().as_str() == code)
        .count()
}

// The report lines once `done` holds for them (none is taken).
fn await_reports(host: &RuntimeHost, done: impl Fn(&[Outcome]) -> bool) -> Vec<Outcome> {
    host.emulator_autostart_reports()
        .await_lines_for_test(done, GATE_WAIT)
        .expect("the awaited report lines")
}

fn events(host: &RuntimeHost) -> Vec<PersistedEvent> {
    host.query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events")
}

// The outcomes of the `device_state_observed` records under `instance` whose code is `code`, in
// order.
fn watch_records(host: &RuntimeHost, instance: InstanceId, code: &str) -> Vec<Outcome> {
    let events = events(host);
    phases(&events)
        .into_iter()
        .filter(|(event, _)| event.links().instance_id() == Some(&instance))
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::DeviceStateObserved { outcome }
                if outcome.code().as_str() == code =>
            {
                Some(outcome)
            }
            _ => None,
        })
        .collect()
}

// The `emulator_autostart_skipped` records under `instance` that a pause made.
fn paused_skips(host: &RuntimeHost, instance: InstanceId) -> Vec<Outcome> {
    watch_records(host, instance, "emulator_autostart_skipped")
        .into_iter()
        .filter(|outcome| outcome.value("skip_reason").is_some())
        .collect()
}

// The events of `event_type` under `instance` after `after`.
fn recorded_after(
    host: &RuntimeHost,
    instance: InstanceId,
    event_type: EventType,
    after: u64,
) -> usize {
    events(host)
        .iter()
        .filter(|event| {
            event.sequence() > after
                && event.event_type() == event_type
                && event.links().instance_id() == Some(&instance)
        })
        .count()
}

// The emulator control actions the scripted emulator was asked for.
fn control_calls(state: &FakeState) -> Vec<EmulatorCall> {
    state
        .emulator
        .calls()
        .into_iter()
        .filter(|call| matches!(call, EmulatorCall::Control(_)))
        .collect()
}

fn control_starts(state: &FakeState) -> usize {
    control_calls(state)
        .iter()
        .filter(|call| **call == EmulatorCall::Control(START))
        .count()
}

fn assert_paused_skip(skip: &Outcome, alias: &str, watch_kind: &str) {
    assert_eq!(skip.value("alias"), Some(&serde_json::json!(alias)));
    assert_eq!(
        skip.value("watch_kind"),
        Some(&serde_json::json!(watch_kind))
    );
    assert_eq!(
        skip.value("skip_reason"),
        Some(&serde_json::json!("instance_paused"))
    );
}

fn completed(receipt: &RuntimeReceipt) {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
}

// (1), (3), (5): a configured instance found stopped at every poll while its own pause holds it, in
// each stage (one poll more than the bound allows): no restart claim, no Start, no queued or
// used-up record, and one skipped record for the pause revision. After the resume the next poll
// restarts it as today, the first restart of the window. A second pause revision records its own
// skip once.
#[test]
fn watch_p_1_a_paused_instance_is_never_restarted_and_its_skip_is_recorded_once_per_revision() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| {
        config.with_runtime_clock(clock.clone())
    });
    completed(&pause(&host, instance_scope(FIRST), 401));
    let before = head(&host);
    host.start_emulator_watch().expect("the watch starts");

    for (poll, stage) in (1..).zip(STAGES) {
        assert!(
            host.set_instance_pause_stage_for_test(FIRST, stage)
                .expect("the pause's stage"),
            "the instance is paused"
        );
        state.emulator.script_read(stopped_state());
        clock.advance(WATCH_INTERVAL_MS);
        host.await_emulator_watch_polls_for_test(poll, GATE_WAIT)
            .expect("the poll");
        assert!(
            claims_after(&host, instance, before).is_empty(),
            "no restart claim in stage {stage:?}"
        );
        assert_eq!(
            host.instance_claims_for_test(FIRST).expect("claims"),
            (None, vec![]),
            "nothing holds or waits for the instance in stage {stage:?}"
        );
    }
    let skips = paused_skips(&host, instance);
    let [skip] = &skips[..] else {
        panic!("one skip for the pause revision: {skips:?}");
    };
    assert_paused_skip(skip, FIRST, "restart");
    assert_eq!(
        reported(
            &await_reports(&host, |_| true),
            "emulator_autostart_skipped"
        ),
        1,
        "the skip is one report line"
    );
    assert!(watch_records(&host, instance, "emulator_autostart_queued").is_empty());
    assert!(
        watch_records(&host, instance, "emulator_watch_restart_exhausted").is_empty(),
        "the skips use up no bound"
    );
    assert_eq!(
        watch_records(&host, instance, "emulator_watch_state_changed").len(),
        1,
        "the polls go on and record the reading"
    );
    assert_eq!(control_starts(&state), 0, "no Start while paused");

    // The resume's reconnect reads the emulator running and passes; the next poll finds it
    // stopped and restarts it, the first restart of the window; its claim reads once more before
    // the Start.
    state
        .emulator
        .script_read(running_state(FAKE_EMULATOR_PORT));
    completed(&resume(&host, instance_scope(FIRST), 402));
    state.emulator.script_read(stopped_state());
    state.emulator.script_read(stopped_state());
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(5, GATE_WAIT)
        .expect("the first poll after the resume");
    await_reports(&host, |lines| {
        reported(lines, "emulator_autostart_started") == 1
    });
    assert_eq!(control_starts(&state), 1, "one Start after the resume");
    let queued = watch_records(&host, instance, "emulator_autostart_queued");
    let [queued] = &queued[..] else {
        panic!("one restart after the resume: {queued:?}");
    };
    assert_eq!(queued.value("attempts"), Some(&serde_json::json!(1)));
    assert_eq!(
        queued.value("limit_count"),
        Some(&serde_json::json!(RESTART_ATTEMPTS))
    );
    assert_eq!(
        queued.value("watch_kind"),
        Some(&serde_json::json!("restart"))
    );

    // A second pause revision: its first poll records its own skip, its second none.
    completed(&pause(&host, instance_scope(FIRST), 403));
    for poll in 6..=7 {
        state.emulator.script_read(stopped_state());
        clock.advance(WATCH_INTERVAL_MS);
        host.await_emulator_watch_polls_for_test(poll, GATE_WAIT)
            .expect("a poll of the second pause");
    }
    let skips = paused_skips(&host, instance);
    assert_eq!(skips.len(), 2, "one skip per pause revision: {skips:?}");
    assert_paused_skip(&skips[1], FIRST, "restart");
    assert_eq!(
        watch_records(&host, instance, "emulator_autostart_queued").len(),
        1
    );
    assert_eq!(control_starts(&state), 1, "no Start under the second pause");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (2), (3): a paused instance whose emulator runs with Android and its port while the Runtime holds
// its endpoint pending gets no recognition claim in any stage, one skipped record (`watch_kind`
// `recognize`) for the revision, and stays pending. The resume's reconnect finds Android not
// started yet and binds nothing; the next poll recognizes and binds it as today.
#[test]
fn watch_p_2_a_paused_running_instance_is_not_recognized_until_the_resume() {
    let root = TempDir::new().expect("tempdir");
    let state = pending_state();
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(FIRST, instance, Arc::clone(&state))),
    )
    .expect("runtime host started before its emulator");
    completed(&pause(&host, instance_scope(FIRST), 501));
    let before = head(&host);
    host.start_emulator_watch().expect("the watch starts");

    for (poll, stage) in (1..).zip(STAGES) {
        assert!(
            host.set_instance_pause_stage_for_test(FIRST, stage)
                .expect("the pause's stage"),
            "the instance is paused"
        );
        state
            .emulator
            .script_read(running_state(FAKE_EMULATOR_PORT));
        clock.advance(WATCH_INTERVAL_MS);
        host.await_emulator_watch_polls_for_test(poll, GATE_WAIT)
            .expect("the poll");
        assert!(
            claims_after(&host, instance, before).is_empty(),
            "no recognition claim in stage {stage:?}"
        );
    }
    let skips = paused_skips(&host, instance);
    let [skip] = &skips[..] else {
        panic!("one skip for the pause revision: {skips:?}");
    };
    assert_paused_skip(skip, FIRST, "recognize");
    assert!(watch_records(&host, instance, "emulator_autostart_queued").is_empty());
    assert!(watch_records(&host, instance, "emulator_watch_restart_exhausted").is_empty());
    assert_eq!(
        state.emulator.bound_port(),
        None,
        "the endpoint stays pending"
    );
    assert_eq!(
        recorded_after(&host, instance, EventType::RuntimeInstanceBound, before),
        0
    );
    assert!(
        control_calls(&state).is_empty(),
        "nothing started or stopped"
    );

    // The resume's reconnect reads the emulator before its Android has started: it binds nothing.
    // The next poll reads it running, then the claim's read on its key and the recognition's own.
    state.emulator.script_read(booting_state());
    completed(&resume(&host, instance_scope(FIRST), 502));
    assert_eq!(
        state.emulator.bound_port(),
        None,
        "the reconnect bound nothing"
    );
    let resumed = head(&host);
    for _ in 0..3 {
        state
            .emulator
            .script_read(running_state(FAKE_EMULATOR_PORT));
    }
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(5, GATE_WAIT)
        .expect("the first poll after the resume");
    await_reports(&host, |lines| {
        lines.iter().any(|line| {
            line.code().as_str() == "emulator_autostart_skipped"
                && line.value("watch_kind") == Some(&serde_json::json!("recognize"))
                && line.value("skip_reason").is_none()
        })
    });
    assert_eq!(
        state.emulator.bound_port(),
        Some(FAKE_EMULATOR_PORT),
        "recognized and bound after the resume"
    );
    assert_eq!(
        recorded_after(&host, instance, EventType::RuntimeInstanceBound, resumed),
        1
    );
    let queued = watch_records(&host, instance, "emulator_autostart_queued");
    let [queued] = &queued[..] else {
        panic!("one recognition after the resume: {queued:?}");
    };
    assert_eq!(
        queued.value("watch_kind"),
        Some(&serde_json::json!("recognize"))
    );
    assert_eq!(queued.value("attempts"), Some(&serde_json::json!(1)));
    assert_eq!(paused_skips(&host, instance).len(), 1);
    assert!(
        control_calls(&state).is_empty(),
        "nothing started or stopped"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// (4): in the same poll, an unpaused configured instance found stopped is restarted exactly as
// before WATCH-P (one queued record with the same values and no `skip_reason`, one claim, one
// Start), while the paused one beside it is skipped.
#[test]
fn watch_p_3_an_unpaused_instance_beside_a_paused_one_is_restarted_as_before() {
    let root = TempDir::new().expect("tempdir");
    let first_state = Arc::new(FakeState::default());
    let second_state = Arc::new(FakeState::default());
    let (first, second) = (instance_id(), instance_id());
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(
        &root,
        &[
            (FIRST, first, &first_state),
            (SECOND, second, &second_state),
        ],
        read,
        |config| config.with_runtime_clock(clock.clone()),
    );
    completed(&pause(&host, instance_scope(FIRST), 601));
    let before = head(&host);
    first_state.emulator.script_read(stopped_state());
    // The second's poll read, then its claim's read before the Start.
    second_state.emulator.script_read(stopped_state());
    second_state.emulator.script_read(stopped_state());

    host.start_emulator_watch().expect("the watch starts");
    clock.advance(WATCH_INTERVAL_MS);
    host.await_emulator_watch_polls_for_test(1, GATE_WAIT)
        .expect("the first poll");
    await_reports(&host, |lines| {
        reported(lines, "emulator_autostart_started") == 1
    });

    assert_eq!(
        control_starts(&second_state),
        1,
        "the unpaused one is started"
    );
    let queued = watch_records(&host, second, "emulator_autostart_queued");
    let [queued] = &queued[..] else {
        panic!("one restart of the unpaused instance: {queued:?}");
    };
    assert_eq!(queued.value("count"), Some(&serde_json::json!(1)));
    assert_eq!(queued.value("alias"), Some(&serde_json::json!(SECOND)));
    assert_eq!(queued.value("attempts"), Some(&serde_json::json!(1)));
    assert_eq!(
        queued.value("limit_count"),
        Some(&serde_json::json!(RESTART_ATTEMPTS))
    );
    assert_eq!(
        queued.value("watch_kind"),
        Some(&serde_json::json!("restart"))
    );
    assert_eq!(queued.value("skip_reason"), None);
    assert_eq!(claims_after(&host, second, before).len(), 1);
    assert!(paused_skips(&host, second).is_empty());

    assert_eq!(control_starts(&first_state), 0, "the paused one is not");
    assert!(claims_after(&host, first, before).is_empty());
    assert!(watch_records(&host, first, "emulator_autostart_queued").is_empty());
    let skips = paused_skips(&host, first);
    let [skip] = &skips[..] else {
        panic!("one skip of the paused instance: {skips:?}");
    };
    assert_paused_skip(skip, FIRST, "restart");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
