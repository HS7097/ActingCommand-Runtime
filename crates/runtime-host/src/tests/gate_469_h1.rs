// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #469 H1 (0.12.62; register §1.397; the design's place 3): an application the policy
// projection found unavailable says who withdrew it. The host reads the word
// (`availability_source`, with `device_link_state` beside `device_link` and `device_hold` beside
// `device_state`) from the active `session.instance.available` record its projection resolved, and
// sets it on the evaluator's own `instance_unavailable` reason of the instance's decisions before
// the not-run explanation reads them: the cycle's decision and the explanation's rows (the read of
// `ac_schedule` and of the diagnosis's not-run rows) carry it, the detail text does not. Every
// scenario runs one task due at 10:00 UTC (the activity window of `gate_372k2`) on the fixture
// instance, which no device self-check gates, so nothing publishes its availability again between
// two evaluations; the evaluations take the production path on a manual Runtime clock, which seeds
// nothing. A withdrawal the backend self-check gate, the device link or the device state owns is
// published through that publisher's own function; any other through the fact store's write.

use super::gate_372k2::{SLOT_TASK, at, slot_sources, slot_task};
use super::*;
use crate::codes::{AvailabilitySource, DeviceHold, HostCode};
use crate::fact_store::POLICY_INSTANCE_AVAILABLE_KEY;
use crate::policy_host::held::{HELD_SIGNAL_PREFIX, held_signal_id};
use actingcommand_contract::{DeviceLinkState, ScheduleExplanationInstance};
use actingcommand_policy::PolicyCode;
use serde_json::{Value, json};

/// The fixture host starts at 09:00, an hour before its task is due.
const START: u64 = at(0, 9, 0);
/// The detector of a publisher that is none of the Runtime's own.
const OTHER_DETECTOR: &str = "neutral.availability-publisher";

/// The fixture host on `root`, whose configuration seeds the instance's `available` with
/// `configured_available`, on `clock`.
fn open_host(
    root: &TempDir,
    registered: InstanceId,
    clock: &Arc<ManualRuntimeClock>,
    configured_available: bool,
) -> RuntimeHost {
    let mut facts = policy_facts();
    facts.outcomes.clear();
    facts.instances[0].available = configured_available;
    RuntimeHost::start(
        config(root)
            .with_policy_inputs(PolicyInputSnapshot::new(facts, policy_resources()))
            .with_runtime_clock(clock.clone()),
        Arc::new(
            FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                registered,
                Arc::new(FakeState::default()),
            )
            .fixture_simulation(),
        ),
    )
    .expect("start the fixture host")
}

/// One fixture host, and what a restart starts it from again.
struct Bench {
    host: RuntimeHost,
    clock: Arc<ManualRuntimeClock>,
    root: TempDir,
    registered: InstanceId,
    configured_available: bool,
}

impl Bench {
    fn start(configured_available: bool) -> Self {
        let root = TempDir::new().expect("tempdir");
        let registered = instance_id();
        let clock = Arc::new(ManualRuntimeClock::new(START, 0));
        let host = open_host(&root, registered, &clock, configured_available);
        host.activate_policy_catalog(&slot_sources(1, &[slot_task(&[600], 1)], None))
            .expect("activate the catalog");
        Self {
            host,
            clock,
            root,
            registered,
            configured_available,
        }
    }

    /// A clean stop and a new start on the same state root and configuration, at `unix_ms`.
    fn restart(self, unix_ms: u64) -> Self {
        let Self {
            host,
            root,
            registered,
            configured_available,
            ..
        } = self;
        host.close().expect("close the fixture host");
        let clock = Arc::new(ManualRuntimeClock::new(unix_ms, unix_ms - START));
        let host = open_host(&root, registered, &clock, configured_available);
        Self {
            host,
            clock,
            root,
            registered,
            configured_available,
        }
    }

    fn set_time(&self, unix_ms: u64) {
        self.clock.set_unix_ms(unix_ms);
        self.clock.set_monotonic_ms(unix_ms - START);
    }

    /// One evaluation at `unix_ms` on the production path, the capacity sampled first as the
    /// host's capacity monitor does while time passes.
    fn evaluate(&self, unix_ms: u64) -> PolicyCycle {
        self.set_time(unix_ms);
        let sample_capacity = self
            .host
            .capacity_sampler_for_test()
            .expect("capacity owner");
        sample_capacity().expect("capacity sample");
        self.host
            .evaluate_policy_cycle(PolicyTrigger::Recovery)
            .unwrap_or_else(|error| panic!("evaluate at {unix_ms}: {error}"))
    }

    /// Publishes, at `unix_ms`, the instance's `session.instance.available` holding `value` from
    /// a publisher that is none of the Runtime's own.
    fn publish_other(&self, unix_ms: u64, value: ContractFactValue, snapshot: &str) {
        self.set_time(unix_ms);
        self.host
            .publish_fact(FactRecord {
                scope: FactScope::Instance {
                    instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
                },
                key: POLICY_INSTANCE_AVAILABLE_KEY.to_owned(),
                content: FactContent::Inline { value },
                observed_at_unix_ms: unix_ms,
                expires_at_unix_ms: None,
                ttl_policy: None,
                confidence_milli: 1_000,
                source_detector: OTHER_DETECTOR.to_owned(),
                source_snapshot_id: snapshot.to_owned(),
                schema_version: "fact.v1".to_owned(),
                resource_bundle_hash: "c".repeat(64),
                invalidate_on: Vec::new(),
            })
            .expect("the other publisher's record");
    }

    /// The fixture instance's entry of the not-run explanation, as `ScheduleExplanation` reads it.
    fn explanation(&self) -> ScheduleExplanationInstance {
        let mut client = TestClient::connect(&self.host);
        let request = client.request(RuntimeOperation::ScheduleExplanation {
            instance: Some(POLICY_INSTANCE_ALIAS.to_owned()),
            candidates: true,
        });
        let receipt = client.send(&request);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
        let Some(RuntimeResult::ScheduleExplanation { explanation }) = receipt.result() else {
            panic!("a schedule explanation: {:?}", receipt.result());
        };
        let [entry] = explanation.instances.as_slice() else {
            panic!("one instance entry: {explanation:#?}");
        };
        entry.clone()
    }

    /// The instance's active `session.instance.available` record: its source snapshot and when
    /// it was observed.
    fn availability_record(&self) -> (String, u64) {
        let snapshot = self
            .host
            .instance_fact_snapshot(InstanceFactContext {
                instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
                server_id: "fixture-server-a".to_owned(),
                game_id: "fixture-game-a".to_owned(),
            })
            .expect("the instance's facts");
        let records = snapshot
            .records
            .iter()
            .filter(|record| record.key == POLICY_INSTANCE_AVAILABLE_KEY)
            .collect::<Vec<_>>();
        let [record] = records.as_slice() else {
            panic!("one availability record: {records:#?}");
        };
        (
            record.source_snapshot_id.clone(),
            record.observed_at_unix_ms,
        )
    }

    /// Every held record of the ledger: its signal id, its start and its gate code.
    fn held_records(&self) -> Vec<(String, Option<u64>, Option<String>)> {
        self.host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::PolicyPlanningSignalObserved),
                ..EventQuery::default()
            })
            .expect("planning signals")
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Policy(PolicyPayload::PlanningSignalObserved(payload))
                    if payload.signal_id().starts_with(HELD_SIGNAL_PREFIX) =>
                {
                    let outcome = payload.outcome();
                    Some((
                        payload.signal_id().to_owned(),
                        outcome.value("held_since_unix_ms").and_then(Value::as_u64),
                        outcome
                            .value("gate_code")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    ))
                }
                _ => None,
            })
            .collect()
    }

    fn close(self) {
        assert!(self.host.fatal_error().expect("runtime health").is_none());
        self.host.close().expect("close the fixture host");
    }
}

/// The task's one `instance_unavailable` reason in `cycle`, the outcome that leaves the host: its
/// values are `expected` and nothing else, and its detail text is the evaluator's own. The
/// explanation's candidate row and its not-running row carry the same outcome as their why.
fn assert_words(bench: &Bench, cycle: &PolicyCycle, expected: &Value) {
    let evaluation = cycle.evaluation.as_ref().expect("an evaluated cycle");
    let decision = evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == SLOT_TASK
                && decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS)
        })
        .unwrap_or_else(|| panic!("the task's decision: {evaluation:#?}"));
    let unavailable = decision
        .reasons
        .iter()
        .filter(|reason| reason.outcome.is(PolicyCode::InstanceUnavailable))
        .collect::<Vec<_>>();
    let [reason] = unavailable.as_slice() else {
        panic!("one instance_unavailable reason: {decision:#?}");
    };
    assert_eq!(
        Value::Object(reason.outcome.values().clone()),
        *expected,
        "{decision:#?}"
    );
    assert_eq!(reason.detail, "the instance is unavailable");

    let entry = bench.explanation();
    let row = entry
        .candidates
        .iter()
        .find(|row| row.task_id == SLOT_TASK)
        .unwrap_or_else(|| panic!("the task's row: {entry:#?}"));
    assert!(row.why.is(PolicyCode::InstanceUnavailable), "{row:?}");
    assert_eq!(
        Value::Object(row.why.values().clone()),
        *expected,
        "{row:?}"
    );
    assert_eq!(entry.not_running.task_id.as_deref(), Some(SLOT_TASK));
    assert_eq!(entry.not_running.why, row.why);
}

// `configured`: the configuration marks the application unavailable; the seed it publishes holds
// `false`.
#[test]
fn h1_an_application_its_configuration_marks_unavailable_says_configured() {
    let bench = Bench::start(false);
    let cycle = bench.evaluate(at(0, 10, 1));
    assert_words(
        &bench,
        &cycle,
        &json!({ "availability_source": AvailabilitySource::Configured.as_str() }),
    );
    bench.close();
}

// `self_check`: the backend self-check gate withdrew the availability
// (`backend_selfcheck:unchecked`, as at a start before the preparation).
#[test]
fn h1_an_application_the_self_check_gate_withdrew_says_self_check() {
    let bench = Bench::start(true);
    bench.set_time(at(0, 10, 0));
    bench
        .host
        .publish_backend_selfcheck_unavailable_for_test(POLICY_INSTANCE_ALIAS, "unchecked")
        .expect("the self-check gate's withdrawal");
    let cycle = bench.evaluate(at(0, 10, 1));
    assert_words(
        &bench,
        &cycle,
        &json!({ "availability_source": AvailabilitySource::SelfCheck.as_str() }),
    );
    bench.close();
}

// `device_link` with `device_link_state`: the handset's device link withdrew the availability
// for its state (`device_link:absent`).
#[test]
fn h1_an_application_its_device_link_withdrew_says_device_link_and_the_state() {
    let bench = Bench::start(true);
    bench.set_time(at(0, 10, 0));
    bench
        .host
        .publish_device_link_unavailable_for_test(POLICY_INSTANCE_ALIAS, DeviceLinkState::Absent)
        .expect("the device link's withdrawal");
    let cycle = bench.evaluate(at(0, 10, 1));
    assert_words(
        &bench,
        &cycle,
        &json!({
            "availability_source": AvailabilitySource::DeviceLink.as_str(),
            "device_link_state": DeviceLinkState::Absent.as_str()
        }),
    );
    bench.close();
}

// `device_state` with `device_hold`: the handset's device state withdrew the availability for its
// hold (`device_state:thermal`).
#[test]
fn h1_an_application_its_device_state_withdrew_says_device_state_and_the_hold() {
    let bench = Bench::start(true);
    bench.set_time(at(0, 10, 0));
    bench
        .host
        .publish_device_state_unavailable_for_test(POLICY_INSTANCE_ALIAS, DeviceHold::Thermal)
        .expect("the device state's withdrawal");
    let cycle = bench.evaluate(at(0, 10, 1));
    assert_words(
        &bench,
        &cycle,
        &json!({
            "availability_source": AvailabilitySource::DeviceState.as_str(),
            "device_hold": DeviceHold::Thermal.as_str()
        }),
    );
    bench.close();
}

// `other_publisher`: a publisher that is none of the four withdrew the availability.
#[test]
fn h1_an_application_another_publisher_withdrew_says_other_publisher() {
    let bench = Bench::start(true);
    bench.publish_other(
        at(0, 10, 0),
        ContractFactValue::Boolean(false),
        "snapshot:neutral-withdrawal",
    );
    let cycle = bench.evaluate(at(0, 10, 1));
    assert_words(
        &bench,
        &cycle,
        &json!({ "availability_source": AvailabilitySource::OtherPublisher.as_str() }),
    );
    bench.close();
}

// `facts_missing`: the active record is not a boolean, so the projection finds the fact missing;
// the host's own `policy_instance_fact_missing` note still follows on the cycle's decision.
#[test]
fn h1_an_application_whose_availability_cannot_be_read_says_facts_missing() {
    let bench = Bench::start(true);
    bench.publish_other(
        at(0, 10, 0),
        ContractFactValue::String("no".to_owned()),
        "snapshot:neutral-unreadable",
    );
    let cycle = bench.evaluate(at(0, 10, 1));
    assert_words(
        &bench,
        &cycle,
        &json!({ "availability_source": AvailabilitySource::FactsMissing.as_str() }),
    );
    let decision = cycle
        .evaluation
        .as_ref()
        .expect("an evaluated cycle")
        .decisions
        .iter()
        .find(|decision| decision.task_id == SLOT_TASK)
        .expect("the task's decision");
    assert!(
        decision
            .reasons
            .iter()
            .any(|reason| reason.outcome.is(HostCode::PolicyInstanceFactMissing)),
        "{decision:#?}"
    );
    bench.close();
}

// The word after a restart: the reason row is built from the active record the fact store reloads
// from the ledger, whole (its detector and snapshot id are the record's own members), and from no
// memory of the stopped host; the same record gives the same word. The record after the restart is
// the one the first start published (the start publishes no seed it already holds).
#[test]
fn h1_after_a_restart_the_same_record_gives_the_same_word() {
    let bench = Bench::start(false);
    let before = bench.evaluate(at(0, 10, 1));
    let expected = json!({ "availability_source": AvailabilitySource::Configured.as_str() });
    assert_words(&bench, &before, &expected);
    let record = bench.availability_record();
    assert_eq!(record.1, START, "the first start's seed");

    let bench = bench.restart(at(0, 10, 2));
    assert_eq!(bench.availability_record(), record, "the same record");
    let after = bench.evaluate(at(0, 10, 3));
    assert_words(&bench, &after, &expected);
    bench.close();
}

// The clocks the word must not touch: a row whose word changes while its code stays keeps its
// since (the held clock compares the code), and the held record written past the threshold
// (`scheduling.task_held.*` at its defaults, 30 minutes) starts where the first unavailable
// evaluation started it, under the code: what a run without the word would write.
#[test]
fn h1_a_changed_word_keeps_the_row_s_since_and_its_held_record() {
    let bench = Bench::start(true);
    let first = at(0, 10, 1);
    bench.publish_other(
        first,
        ContractFactValue::Boolean(false),
        "snapshot:neutral-withdrawal",
    );
    let cycle = bench.evaluate(first);
    assert_words(
        &bench,
        &cycle,
        &json!({ "availability_source": AvailabilitySource::OtherPublisher.as_str() }),
    );
    assert_eq!(bench.explanation().not_running.since_unix_ms, first);

    bench.publish_other(
        at(0, 10, 6),
        ContractFactValue::String("no".to_owned()),
        "snapshot:neutral-unreadable",
    );
    let cycle = bench.evaluate(at(0, 10, 6));
    assert_words(
        &bench,
        &cycle,
        &json!({ "availability_source": AvailabilitySource::FactsMissing.as_str() }),
    );
    let entry = bench.explanation();
    assert_eq!(entry.not_running.since_unix_ms, first, "{entry:#?}");
    assert_eq!(
        entry
            .candidates
            .iter()
            .find(|row| row.task_id == SLOT_TASK)
            .map(|row| row.since_unix_ms),
        Some(first),
        "{entry:#?}"
    );
    assert!(
        bench.held_records().is_empty(),
        "not yet past the threshold"
    );

    bench.evaluate(at(0, 10, 32));
    let entry = bench.explanation();
    assert_eq!(entry.not_running.since_unix_ms, first, "{entry:#?}");
    assert_eq!(
        bench.held_records(),
        [(
            held_signal_id(SLOT_TASK, POLICY_INSTANCE_ALIAS, first),
            Some(first),
            Some(PolicyCode::InstanceUnavailable.as_str().to_owned()),
        )],
        "one held record, from the first unavailable evaluation"
    );
    bench.close();
}
