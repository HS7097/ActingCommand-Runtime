// SPDX-License-Identifier: AGPL-3.0-only

// #480 PG-1 (0.12.72; register §1.468; the owner's requirement on board #480, 11-17: after "pause
// all" the Runtime starts no emulator and launches no game by itself, and a pause she presses
// stands). Production v0.12.70 on 11-17: under the global pause the emulator watch restarted the
// one instance that carried no pause of its own, and the instance pause a person then pressed was
// lifted again when the watch's restart, which held the instance through its readiness wait,
// ended. On the scripted emulator (HOST-I1) and a scripted connectivity status, with the S6c
// helpers. Every wait returns as soon as what it waits for is reported, recorded or seen;
// `GATE_WAIT` only bounds a broken run.

use super::gate_369_s6c::{GATE_WAIT, autostart_host, connectivity, head, instance_scope, stop};
use super::instance_workers::{all_events, wait_until};
use super::*;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    FactScalar, FactValue, InstancePauseStage, RuntimePayload, SchedulingPauseReason,
};
use actingcommand_host_metrics::NetworkConnectivity;

const FIRST: &str = "pg1.first";
/// The runtime fact of the held pauses (`host.scheduling_pause`).
const PAUSE_FACT_KEY: &str = "host.scheduling_pause";

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

// A person's pause of `alias`, with the smallest drain timeout a request may name.
fn instance_pause(alias: &str) -> RuntimeRequest {
    runtime_request(
        &IdentifierIssuer::new().expect("identifier issuer"),
        RuntimeOperation::PauseScheduling {
            scope: instance_scope(alias),
            pause_reason: SchedulingPauseReason::new("pg1_pause").expect("pause reason"),
            drain_timeout_ms: 1_000,
        },
    )
}

// The stage of the pause `alias` holds of its own, if it holds one.
fn pause_stage(host: &RuntimeHost, alias: &str) -> Option<InstancePauseStage> {
    host.scheduling_pauses_for_test()
        .expect("the held pauses")
        .1
        .get(alias)
        .map(|pause| pause.stage)
}

// The rows of every record of `host.scheduling_pause` after `after`, in order.
fn pause_records_after(host: &RuntimeHost, after: u64) -> Vec<Vec<BTreeMap<String, FactScalar>>> {
    all_events(host)
        .iter()
        .filter(|event| event.sequence() > after)
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::FactRecorded(recorded))
                if recorded.record().key == PAUSE_FACT_KEY =>
            {
                match &recorded.record().value {
                    FactValue::RecordList(rows) => Some(rows.clone()),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect()
}

// Whether a record of the held pauses names a pause of `alias`.
fn names_instance(rows: &[BTreeMap<String, FactScalar>], alias: &str) -> bool {
    rows.iter()
        .any(|row| row.get("instance_alias") == Some(&FactScalar::String(alias.to_owned())))
}

// (7) Ruling 4, the 11-17 case. An autostart claim holds the instance through its Start, whose
// readiness wait ends timed out, and a person pauses the instance meanwhile, the pause's grace
// already past (`with_scheduling_pause_deadlines_for_test`). The pause's hand-back waits for the
// instance, which the Start holds, and then meets the claim's key, which the claim still holds
// once its Start was driven (`run_after_configured_start_drive_for_test`). It waits for that
// automatic claim beyond the grace, ends `released` once the claim has ended and answers success:
// nothing is lifted, and the one record of the held pauses it made names the instance.
#[test]
fn pg1_7_an_instance_pause_stands_while_an_automatic_start_outlasts_its_grace() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(&root, &[(FIRST, instance, &state)], read, |config| {
        config.with_scheduling_pause_deadlines_for_test(Duration::ZERO)
    });
    stop(&host, FIRST, 871);
    // The Start blocks in its readiness wait, which then ends timed out.
    state.block_emulator_start.store(true, Ordering::Release);
    state.fail_emulator_start.store(true, Ordering::Release);
    // Once its Start was driven, the claim keeps its key until the hand-back has met it.
    let (driven_tx, driven_rx) = mpsc::channel::<()>();
    let (met_tx, met_rx) = mpsc::channel::<()>();
    host.run_after_configured_start_drive_for_test(Box::new(move || {
        let _ = driven_tx.send(());
        let _ = met_rx.recv_timeout(GATE_WAIT);
    }))
    .expect("the claim's hook");
    host.run_when_pause_release_meets_a_lease_for_test(Box::new(move || {
        let _ = met_tx.send(());
    }))
    .expect("the hand-back's hook");
    assert_eq!(host.enqueue_emulator_autostart().expect("enqueue"), 1);
    wait_until("the configured Start", || {
        state.emulator_start_entered.load(Ordering::Acquire)
    });
    let before = head(&host);
    let pause = instance_pause(FIRST);

    let paused = thread::scope(|scope| {
        let pausing = scope.spawn(|| {
            host.process_request_for_test(&pause, ConnectionId::new(872).expect("connection"))
        });
        // The hand-back waits for the instance, which the Start holds.
        wait_until("the pause's hand-back", || {
            pause_stage(&host, FIRST) == Some(InstancePauseStage::Paused)
        });
        state.block_emulator_start.store(false, Ordering::Release);
        driven_rx
            .recv_timeout(GATE_WAIT)
            .expect("the configured Start was driven");
        pausing
            .join()
            .expect("the pause's thread")
            .expect("the pause's receipt")
    });

    assert_eq!(paused.state(), RuntimeReceiptState::Completed, "{paused:?}");
    assert!(
        matches!(
            paused.result(),
            Some(RuntimeResult::SchedulingPaused {
                drained: Some(_),
                ..
            })
        ),
        "{paused:?}"
    );
    assert_eq!(
        pause_stage(&host, FIRST),
        Some(InstancePauseStage::Released),
        "the pause stands"
    );
    let records = pause_records_after(&host, before);
    let [record] = &records[..] else {
        panic!("the pause's one record of the held pauses, nothing lifted: {records:?}");
    };
    assert!(names_instance(record, FIRST), "{record:?}");
    let lines = await_reports(&host, |lines| {
        reported(lines, "emulator_autostart_failed") == 1
    });
    assert_eq!(reported(&lines, "emulator_autostart_started"), 0);
    assert_eq!(
        host.instance_claims_for_test(FIRST).expect("claims"),
        (None, vec![]),
        "the claim has ended"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
