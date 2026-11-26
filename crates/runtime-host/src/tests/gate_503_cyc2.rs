// SPDX-License-Identifier: AGPL-3.0-only

// #503 CYC-2 (0.12.85; register §1.528, the coordinator's rulings 1 to 3), the host's half. The
// remedy fixture of `gate_411_b15h.rs` (one instance; `t_main`, whose package declares a remedy,
// and `t_sink`, whose package provides it), with given evaluation times; a delay is put into a
// call or a wait by advancing the test thread's lap clock, no clock read and no sleep.
// - h1 (ruling 1): a delay put into one call of the evaluation's setup and one into the wait for
//   the catalog store's memo lock show under their names among the three slowest calls and in
//   `evaluation_setup_ms`; the wait is not counted in the call that waited.
// - h2 (ruling 2): the remedy journal holds a run admitted under an older catalog generation and
//   the store's memo keeps one generation, the active one, so until 0.12.85 every cycle whose memo
//   did not hold the run's generation read it from its files again; now the first cycle reads it,
//   and the next, after another generation took the memo, reads none, and the window end the
//   journal keeps is the one the run's generation gives.
// - h3 (ruling 3): a summary whose capacity sample leaves the disk gate's input as it was, and a
//   stutter, wake the policy driver for no cycle; a change of the capacity state, a pacing change
//   and a change of the dispatch share each wake it once; the driver's cycle count stands still
//   between them.

use super::gate_411_b15h::{
    Fixture, MAIN_TASK, RESULT, catalog, facts, main_pack, next_eligible, sink_pack,
};
use super::*;
use crate::policy_host::{
    CycleStretch, plan_cycle_stretch_delay_for_test, take_cycle_stretch_delay_for_test,
};
use crate::{PolicyCycle, SetupCall};
use actingcommand_host_metrics::{
    CapacitySample, CapacityTarget, HostSample, HostSampler, ProcessLoadThresholds,
};
use serde_json::{Value, json};
use std::sync::Mutex;
use std::time::Duration;

/// The delay put into a call or a wait: longer than any call of these cycles takes by itself.
const DELAY: Duration = Duration::from_secs(600);
const DELAY_MICROS: u64 = 600_000_000;

/// The fixture with `main` declaring the remedy of `need_a` by `cap_x`, and `sink` providing it.
fn remedy_fixture() -> Fixture {
    Fixture::new(&main_pack(&["cap_x"], Some(&["cap_x"])), &sink_pack())
}

/// One evaluated cycle at `at` on the fixture's facts, the Runtime clock moved there first.
fn remedy_cycle(fixture: &Fixture, at: u64, seed: u64) -> PolicyCycle {
    advance_manual_clock_to(&fixture.clock, at);
    let cycle = fixture
        .host
        .evaluate_policy_cycle_with_test_inputs(
            &facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: at,
                monotonic_ms: at,
            },
            seed,
            PolicyTrigger::FactsChanged,
        )
        .expect("evaluate the policy");
    assert!(cycle.evaluation.is_some(), "a full evaluation at {at}");
    cycle
}

/// The fixture's catalog as its version `version`: another generation of the same tasks.
fn catalog_version(version: u64) -> CatalogSources {
    let mut sources = catalog();
    for source in [
        &mut sources.tasks,
        &mut sources.pools,
        &mut sources.activity,
        &mut sources.timeline,
    ] {
        let mut document: Value =
            serde_json::from_slice(&source.bytes).expect("fixture catalog document");
        document["catalog"]["catalog_version"] = json!(version);
        source.bytes = serde_json::to_vec_pretty(&document).expect("fixture catalog bytes");
    }
    sources
}

#[test]
fn cyc2_h1_a_delay_put_into_a_setup_call_and_into_the_memo_wait_shows_under_its_name() {
    let fixture = remedy_fixture();
    let (_, _, ran) = fixture.evaluate_and_run(POLICY_NOW_UNIX_MS, 7, MAIN_TASK, RESULT);

    // The next cycle's remedy journal reads the run's window end from its generation, through the
    // memo's lock: a delay in the run slots' call and one in that wait.
    plan_cycle_stretch_delay_for_test(CycleStretch::SetupCall(SetupCall::RunSlots), DELAY);
    plan_cycle_stretch_delay_for_test(CycleStretch::CatalogMemoWait, DELAY);
    let cycle = remedy_cycle(&fixture, next_eligible(&ran), 8);
    let measurement = cycle
        .measurement
        .expect("an evaluated cycle carries its measurement");
    let stretches = measurement.stretches;
    let calls = stretches.evaluation_setup_calls;
    let slowest = calls.slowest();
    let delayed = slowest
        .iter()
        .filter(|(_, micros)| *micros >= DELAY_MICROS)
        .map(|(call, _)| *call)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        delayed,
        BTreeSet::from([SetupCall::RunSlots, SetupCall::CatalogMemoWait]),
        "both delays among the three slowest: {slowest:?}"
    );
    assert_eq!(
        slowest.first().map(|(_, micros)| *micros >= DELAY_MICROS),
        Some(true),
        "the slowest first: {slowest:?}"
    );
    assert!(
        calls.micros(SetupCall::RemedyPlan) < DELAY_MICROS,
        "the memo's wait is counted apart from the remedy plan that waited ({} us)",
        calls.micros(SetupCall::RemedyPlan)
    );
    assert!(
        stretches.evaluation_setup_micros >= 2 * DELAY_MICROS,
        "evaluation_setup_ms still covers the whole stretch ({} us)",
        stretches.evaluation_setup_micros
    );
    for (name, micros) in [
        ("cycle_inputs", stretches.cycle_inputs_micros),
        ("policy_lock_wait", stretches.policy_lock_wait_micros),
        ("projection", measurement.projection_micros),
        ("evaluation", measurement.elapsed_micros),
    ] {
        assert!(micros < DELAY_MICROS, "no delay in {name} ({micros} us)");
    }

    // A cycle with nothing planned: no call near the delay.
    let quiet = remedy_cycle(&fixture, next_eligible(&ran) + 60_000, 9)
        .measurement
        .expect("an evaluated cycle carries its measurement")
        .stretches;
    assert!(
        quiet.evaluation_setup_micros < DELAY_MICROS,
        "{} us",
        quiet.evaluation_setup_micros
    );
    fixture.host.close().expect("close the runtime host");
}

#[test]
fn cyc2_h2_the_remedy_journal_reads_a_runs_generation_once_and_no_cycle_reads_it_again() {
    let fixture = remedy_fixture();
    // A run under the first generation, then a second and a third generation activated, and a memo
    // of one generation, which holds the active one: the run's generation is in its files alone.
    // (Review of #1899: the first build ran a second run under the second generation, whose
    // admission needs an approval of that catalog; the fixture's one approval id names the first,
    // and the test stopped there on `approval_target_identity_conflict`.)
    let (_, _, first) = fixture.evaluate_and_run(POLICY_NOW_UNIX_MS, 7, MAIN_TASK, RESULT);
    for version in [2, 3] {
        fixture
            .host
            .activate_policy_catalog(&catalog_version(version))
            .expect("activate a later generation");
    }
    fixture
        .host
        .set_catalog_memo_capacity_for_test(1)
        .expect("a memo of one generation");

    // The first cycle after it reads the run's generation from its files, once.
    let at = next_eligible(&first);
    plan_cycle_stretch_delay_for_test(CycleStretch::CatalogRead, DELAY);
    let reading = remedy_cycle(&fixture, at, 9);
    assert_eq!(
        take_cycle_stretch_delay_for_test(CycleStretch::CatalogRead),
        None,
        "the first cycle read a generation from its files"
    );
    let calls = reading
        .measurement
        .expect("an evaluated cycle carries its measurement")
        .stretches
        .evaluation_setup_calls;
    assert!(
        calls.micros(SetupCall::CatalogReads) >= DELAY_MICROS,
        "the read is named in the setup: {:?}",
        calls.slowest()
    );

    // A fourth generation is activated, and the memo of one holds it: the run's generation is in
    // its files alone again (the activation read its own generation, before the delay below is
    // planned). Until 0.12.85 the next cycle read the run's generation from them once more, as
    // every cycle did whose memo did not hold it; now it reads no generation: the wait is gone.
    fixture
        .host
        .activate_policy_catalog(&catalog_version(4))
        .expect("activate the fourth generation");
    plan_cycle_stretch_delay_for_test(CycleStretch::CatalogRead, DELAY);
    let quiet = remedy_cycle(&fixture, at + 60_000, 10);
    assert_eq!(
        take_cycle_stretch_delay_for_test(CycleStretch::CatalogRead),
        Some(DELAY),
        "no generation read from its files in the next cycle"
    );
    let stretches = quiet
        .measurement
        .expect("an evaluated cycle carries its measurement")
        .stretches;
    assert_eq!(
        stretches
            .evaluation_setup_calls
            .micros(SetupCall::CatalogReads),
        0
    );
    assert!(
        stretches.evaluation_setup_micros < DELAY_MICROS,
        "{} us",
        stretches.evaluation_setup_micros
    );

    // What the journal hands the remedy plan is what reading the run's generation gives: its
    // window end, the one its own generation gives now.
    let ends = fixture
        .host
        .remedy_window_ends_for_test()
        .expect("the remedy journal's window ends");
    assert_eq!(ends.len(), 1, "the run: {ends:?}");
    for (decision_id, kept, read) in &ends {
        assert_eq!(kept, read, "{decision_id}");
    }
    fixture.host.close().expect("close the runtime host");
}

/// Free bytes far above the default soft threshold (2 GiB).
const SUFFICIENT: u64 = 64 * 1024 * 1024 * 1024;
/// Under the soft threshold, above the hard one (512 MiB).
const SOFT: u64 = 1024 * 1024 * 1024;
/// Under the hard threshold.
const HARD: u64 = 256 * 1024 * 1024;

/// The host's capacity sampler: the actual volumes of the host's targets, each with the free bytes
/// the test scripts.
struct ScriptedCapacity {
    free: Arc<Mutex<u64>>,
}

impl HostSampler for ScriptedCapacity {
    fn sample_capacity(&mut self, targets: &[CapacityTarget]) -> Vec<CapacitySample> {
        let free = *self.free.lock().expect("scripted free bytes");
        actingcommand_host_metrics::sample_capacity(targets)
            .into_iter()
            .map(|mut sample| {
                if sample.available_bytes.is_ok() {
                    sample.available_bytes = Ok(free);
                }
                sample
            })
            .collect()
    }

    fn sample(
        &mut self,
        _observed_at_unix_ms: u64,
        _owned_processes: &BTreeMap<u32, String>,
        _top_process_count: usize,
        _thresholds: ProcessLoadThresholds,
    ) -> Result<HostSample, actingcommand_contract::outcome::Code> {
        panic!("a capacity-only host enables no performance counters")
    }
}

/// The policy driver of actingd as far as its count goes: one evaluated cycle for each take of the
/// host's wakes that names a trigger.
struct DriverCount {
    wakes: crate::PolicyWakes,
    cycles: u64,
}

impl DriverCount {
    /// What woke the driver since the last take, with the records counted under their types; a
    /// take that names a trigger is one more cycle.
    fn take(&mut self) -> (Vec<PolicyTrigger>, Vec<(EventType, u64)>) {
        let taken = self.wakes.take_counted(Duration::ZERO);
        if !taken.0.is_empty() {
            self.cycles += 1;
        }
        taken
    }
}

fn no_wake() -> (Vec<PolicyTrigger>, Vec<(EventType, u64)>) {
    (Vec::new(), Vec::new())
}

fn summaries(host: &RuntimeHost) -> usize {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::PerformanceSummary),
        ..EventQuery::default()
    })
    .expect("the summaries")
    .len()
}

#[test]
fn cyc2_h3_a_summary_or_stutter_that_changes_no_input_wakes_no_cycle_a_change_wakes_one() {
    let root = TempDir::new().expect("tempdir");
    // Neither the frame cleaner nor the performance counters: no periodic sampler runs, so only
    // the test's re-samples record a summary.
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&root)
            .with_frame_retention_enabled(false)
            .with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    let free = Arc::new(Mutex::new(SUFFICIENT));
    host.replace_capacity_sampler_for_test(Box::new(ScriptedCapacity {
        free: Arc::clone(&free),
    }))
    .expect("the scripted capacity sampler");
    let resample = host
        .capacity_sampler_for_test()
        .expect("the capacity owner");
    // A summary interval later (60 s, the default: this host has no performance configuration),
    // the capacity owner's sample: one summary recorded, as the summaries production records by
    // interval are (review of #1899: a second later no summary was due with the state unchanged).
    let summarise = || {
        clock.advance(60_000);
        resample().expect("a capacity summary");
    };
    let mut driver = DriverCount {
        wakes: host.open_policy_wakes().expect("open the policy wakes"),
        cycles: 0,
    };
    // A first summary of the scripted volumes, whatever the start's sample of the real ones was.
    summarise();
    driver.take();
    let settled = driver.cycles;

    // A summary whose volumes and states are those of the last: recorded, and no cycle.
    let before = summaries(&host);
    summarise();
    assert_eq!(summaries(&host), before + 1, "the summary is recorded");
    assert_eq!(driver.take(), no_wake(), "the same capacity state");
    assert_eq!(driver.cycles, settled);

    // A stutter and a summary that changes nothing, from any append point: no cycle.
    for event_type in [
        EventType::PerformanceStutterDetected,
        EventType::PerformanceSummary,
    ] {
        host.raise_policy_wake_for_test(event_type)
            .expect("the record's wake");
        assert_eq!(driver.take(), no_wake(), "{event_type:?}");
    }
    assert_eq!(driver.cycles, settled);

    // The capacity state changes to soft pressure: one cycle, for the summary and the pressure's
    // start.
    *free.lock().expect("scripted free bytes") = SOFT;
    summarise();
    assert_eq!(
        driver.take(),
        (
            vec![PolicyTrigger::ResourcesChanged],
            vec![
                (EventType::PerformanceSummary, 1),
                (EventType::PerformancePressureStarted, 1)
            ]
        )
    );
    assert_eq!(driver.cycles, settled + 1);

    // The same state again: the summary is recorded and wakes nothing.
    let before = summaries(&host);
    summarise();
    assert_eq!(summaries(&host), before + 1);
    assert_eq!(driver.take(), no_wake(), "still soft pressure");
    assert_eq!(driver.cycles, settled + 1);

    // Soft to hard pressure: the pressure goes on, so the summary alone changes the gate's input
    // and wakes one cycle.
    *free.lock().expect("scripted free bytes") = HARD;
    summarise();
    assert_eq!(
        driver.take(),
        (
            vec![PolicyTrigger::ResourcesChanged],
            vec![(EventType::PerformanceSummary, 1)]
        )
    );
    assert_eq!(driver.cycles, settled + 2);
    summarise();
    assert_eq!(driver.take(), no_wake(), "still hard pressure");

    // A pacing change and a change of the dispatch share each wake one cycle at once.
    for (index, event_type) in [
        EventType::PerformancePacingChanged,
        EventType::PerformanceDispatchShareChanged,
    ]
    .into_iter()
    .enumerate()
    {
        host.raise_policy_wake_for_test(event_type)
            .expect("the record's wake");
        assert_eq!(
            driver.take(),
            (vec![PolicyTrigger::ResourcesChanged], vec![(event_type, 1)]),
            "{event_type:?}"
        );
        assert_eq!(
            driver.cycles,
            settled + 3 + u64::try_from(index).expect("a small index")
        );
    }

    // Every other record of the performance thread wakes the driver as before.
    for event_type in [
        EventType::PerformancePressureStarted,
        EventType::PerformancePressureEnded,
        EventType::PerformanceMonitorDegraded,
        EventType::PerformanceMonitorRecovered,
        EventType::PerformanceBalanceChanged,
        EventType::PerformancePacingChanged,
        EventType::PerformanceDispatchShareChanged,
        EventType::PerformanceRunHold,
    ] {
        assert_eq!(
            policy_trigger_for_event(event_type),
            Some(PolicyTrigger::ResourcesChanged),
            "{event_type:?}"
        );
    }
    for event_type in [
        EventType::PerformanceStutterDetected,
        EventType::PerformanceSummary,
    ] {
        assert_eq!(policy_trigger_for_event(event_type), None, "{event_type:?}");
    }
    drop(driver);
    host.close().expect("close the runtime host");
}
