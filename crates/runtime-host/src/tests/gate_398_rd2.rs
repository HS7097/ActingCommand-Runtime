// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #398 RD-2 (model-398 v1.1 §4, §12 rows RD-2-T2, T3, T4, T6, T9 and T12): the wired
// device link of a bound handset, through the host. The fake provider's watch reports nothing by
// itself; each test hands the host the watch's lists as its reader does
// (`device_link_event_for_test`) and runs the timers on a manual clock
// (`device_link_tick_for_test`). Every expected value comes from the model: the state table of
// §4.2, the restore of §4.3, the in-flight rule of §4.4 and the records of §4.7. The serial is a
// neutral fake, distinct from the other test modules', and no record names it.

use super::gate_398_rd1b::{bound, last_availability, resolved_handset};
use super::instance_workers::phases;
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::outcome::CauseRelation;
use actingcommand_contract::{
    DeviceLinkState, RuntimeFactRecord, RuntimeFactScope, RuntimeLifecyclePhase,
};
use actingcommand_device::handset_label;
use actingcommand_execution_kernel::DeviceLinkWatchEvent;

const GATE_WAIT: Duration = Duration::from_secs(120);
const SERIAL: &str = "TESTSERIAL0031";
const DEVICE_LINK: &str = "device.link";
const DEVICE_CONNECTED: &str = "device.connected";
const SELFCHECK_PREFIX: &str = "backend.selfcheck.";

// A started host whose policy instance is a handset bound to `SERIAL`, beside an emulator
// instance, with a device-link watch.
struct HandsetHost {
    _root: TempDir,
    host: RuntimeHost,
    state: Arc<FakeState>,
    watches: Arc<FakeLinkWatches>,
    handset: InstanceId,
}

fn handset_host(clock: Option<Arc<ManualRuntimeClock>>) -> HandsetHost {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.observed_open.store(true, Ordering::Release);
    let emulator_state = Arc::new(FakeState::default());
    emulator_state.observed_open.store(true, Ordering::Release);
    let watches = Arc::new(FakeLinkWatches::default());
    let handset = instance_id();
    let config = config(&root).with_policy_inputs(PolicyInputSnapshot::new(
        pending_policy_facts(),
        pending_policy_resources(),
    ));
    let config = match clock {
        Some(clock) => config.with_runtime_clock(clock),
        None => config,
    };
    let host = RuntimeHost::start(
        config,
        Arc::new(
            FakeProvider::from_entries([
                (
                    POLICY_INSTANCE_ALIAS.to_owned(),
                    handset,
                    Arc::clone(&state),
                ),
                (
                    POLICY_INSTANCE_ALIAS_B.to_owned(),
                    instance_id(),
                    emulator_state,
                ),
            ])
            .with_device_endpoints()
            .with_resolved_for(
                POLICY_INSTANCE_ALIAS,
                resolved_handset(
                    POLICY_INSTANCE_ALIAS,
                    handset,
                    bound(POLICY_INSTANCE_ALIAS, SERIAL),
                ),
            )
            .with_link_watch(Arc::clone(&watches)),
        ),
    )
    .expect("runtime host with a handset");
    HandsetHost {
        _root: root,
        host,
        state,
        watches,
        handset,
    }
}

// One list of the watch: the handset's state.
fn list(host: &RuntimeHost, state: DeviceLinkState) {
    host.device_link_event_for_test(DeviceLinkWatchEvent::Links(BTreeMap::from([(
        POLICY_INSTANCE_ALIAS.to_owned(),
        state,
    )])))
    .expect("the host applies the list");
}

fn all_events(host: &RuntimeHost) -> Vec<PersistedEvent> {
    host.query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events")
}

// The outcomes of the `device_link_observed` records, in order.
fn link_records(events: &[PersistedEvent]) -> Vec<actingcommand_contract::outcome::Outcome> {
    phases(events)
        .into_iter()
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::DeviceLinkObserved { outcome } => Some(outcome),
            _ => None,
        })
        .collect()
}

fn records_of(
    events: &[PersistedEvent],
    code: HostCode,
) -> Vec<actingcommand_contract::outcome::Outcome> {
    link_records(events)
        .into_iter()
        .filter(|outcome| outcome.code().as_str() == code.as_str())
        .collect()
}

fn instance_facts(host: &RuntimeHost, instance_id: InstanceId) -> Vec<RuntimeFactRecord> {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let receipt = host
        .process_request_for_test(
            &runtime_request(&ids, RuntimeOperation::RuntimeFactSnapshot),
            ConnectionId::new(398).expect("connection"),
        )
        .expect("fact snapshot receipt");
    let Some(RuntimeResult::RuntimeFactSnapshot { snapshot }) = receipt.result() else {
        panic!("a fact snapshot: {receipt:?}");
    };
    snapshot
        .records
        .iter()
        .filter(|record| record.scope == RuntimeFactScope::Instance { instance_id })
        .cloned()
        .collect()
}

fn fact(facts: &[RuntimeFactRecord], key: &str) -> Option<ContractFactValue> {
    facts
        .iter()
        .find(|record| record.key == key)
        .map(|record| record.value.clone())
}

// T12: no record names the bound serial.
fn assert_no_serial(events: &[PersistedEvent]) {
    for event in events {
        let shown = format!("{:?}", event.payload());
        assert!(
            !shown.contains(SERIAL),
            "a record names the serial: {shown}"
        );
    }
}

// T9 (§4.1): a configuration without a bound handset opens no watch process; one with a bound
// handset opens exactly one, and the close stops it.
#[test]
fn rd2_t9_only_a_bound_handset_opens_the_watch() {
    let root = TempDir::new().expect("tempdir");
    let watches = Arc::new(FakeLinkWatches::default());
    let emulators = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id(), Arc::default())
                .with_device_endpoints()
                .with_link_watch(Arc::clone(&watches)),
        ),
    )
    .expect("runtime host with emulators only");
    emulators.close().expect("close host");
    assert_eq!(watches.opened(), 0);

    let run = handset_host(None);
    assert_eq!(run.watches.opened(), 1);
    run.host.close().expect("close host");
    assert_eq!(run.watches.stopped(), 1);
}

// T2 and T6 (§4.2, §4.3): an idle handset that leaves `device` is unavailable under
// `device_link:<state>` with `device.link` and `device.connected = false`, its self-check facts
// dropped and one `device_link_waiting` per state; the same state again records nothing;
// `unauthorized` only waits, with no command sent. The emulator stays available, and nothing is
// fatal.
#[test]
fn rd2_t2_t6_an_idle_handset_that_leaves_device_waits_once_per_state_and_is_unavailable() {
    let run = handset_host(None);
    let host = &run.host;
    list(host, DeviceLinkState::Device);
    let linked = instance_facts(host, run.handset);
    assert_eq!(
        fact(&linked, DEVICE_LINK),
        Some(ContractFactValue::String("device".to_owned()))
    );
    assert_eq!(
        fact(&linked, DEVICE_CONNECTED),
        Some(ContractFactValue::Boolean(true))
    );

    list(host, DeviceLinkState::Absent);
    list(host, DeviceLinkState::Absent);
    let events = all_events(host);
    let waiting = records_of(&events, HostCode::DeviceLinkWaiting);
    let [absent] = waiting.as_slice() else {
        panic!("one wait for absent: {waiting:?}");
    };
    assert_eq!(outcome_str(absent, "device_link_state"), Some("absent"));
    assert_eq!(
        outcome_str(absent, "device_label"),
        Some(handset_label(POLICY_INSTANCE_ALIAS).as_str())
    );
    assert_eq!(
        last_availability(&events, POLICY_INSTANCE_ALIAS),
        (false, "device_link:absent".to_owned())
    );
    let away = instance_facts(host, run.handset);
    assert_eq!(
        fact(&away, DEVICE_LINK),
        Some(ContractFactValue::String("absent".to_owned()))
    );
    assert_eq!(
        fact(&away, DEVICE_CONNECTED),
        Some(ContractFactValue::Boolean(false))
    );
    assert!(
        !away
            .iter()
            .any(|record| record.key.starts_with(SELFCHECK_PREFIX)),
        "{away:?}"
    );

    list(host, DeviceLinkState::Unauthorized);
    let events = all_events(host);
    let states = records_of(&events, HostCode::DeviceLinkWaiting)
        .iter()
        .map(|outcome| outcome_str(outcome, "device_link_state").map(str::to_owned))
        .collect::<Vec<_>>();
    assert_eq!(
        states,
        [Some("absent".to_owned()), Some("unauthorized".to_owned())]
    );
    assert_eq!(
        last_availability(&events, POLICY_INSTANCE_ALIAS),
        (false, "device_link:unauthorized".to_owned())
    );
    assert!(run.watches.reconnects().is_empty());
    assert!(last_availability(&events, POLICY_INSTANCE_ALIAS_B).0);
    assert!(
        !phases(&events)
            .iter()
            .any(|(_, phase)| matches!(phase, RuntimeLifecyclePhase::RecoveryLadderStarted { .. }))
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    assert_no_serial(&events);
    run.host.close().expect("close host");
}

// T3 (§4.3): back at `device`, the preparation is queued only once `device_link.settle_ms`
// (default 3000) passed on the host clock; it runs on a self-check claim and passes, so the
// handset is available again with `device.connected = true` and one `device_link_restored`.
#[test]
fn rd2_t3_a_handset_back_at_device_is_prepared_after_the_settle_wait_and_restored() {
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let run = handset_host(Some(Arc::clone(&clock)));
    let host = &run.host;
    list(host, DeviceLinkState::Device);
    list(host, DeviceLinkState::Absent);
    list(host, DeviceLinkState::Device);
    host.device_link_tick_for_test().expect("tick");
    clock.advance(2_999);
    host.device_link_tick_for_test().expect("tick");
    let events = all_events(host);
    assert!(records_of(&events, HostCode::DeviceLinkRestored).is_empty());
    assert_eq!(
        last_availability(&events, POLICY_INSTANCE_ALIAS),
        (false, "device_link:absent".to_owned())
    );

    clock.advance(1);
    host.device_link_tick_for_test().expect("tick");
    wait_until(GATE_WAIT, || {
        !records_of(&all_events(host), HostCode::DeviceLinkRestored).is_empty()
    });
    let events = all_events(host);
    let restored = records_of(&events, HostCode::DeviceLinkRestored);
    let [restored] = restored.as_slice() else {
        panic!("one restore: {restored:?}");
    };
    assert_eq!(
        outcome_str(restored, "device_label"),
        Some(handset_label(POLICY_INSTANCE_ALIAS).as_str())
    );
    assert!(last_availability(&events, POLICY_INSTANCE_ALIAS).0);
    let back = instance_facts(host, run.handset);
    assert_eq!(
        fact(&back, DEVICE_LINK),
        Some(ContractFactValue::String("device".to_owned()))
    );
    assert_eq!(
        fact(&back, DEVICE_CONNECTED),
        Some(ContractFactValue::Boolean(true))
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    assert_no_serial(&events);
    run.host.close().expect("close host");
}

// T4 (§4.4; spec §5, the condition is the result): a run whose device fails while its handset
// is not `device` ends with `device_link_lost`, the handset's label and link state its values
// and the device failure (a capture backend operation failure) its `caused_by` link; no input
// reaches the device. The capture fails as an adb command against a missing handset does, not
// transient, and still only the run ends: the receipt is not fatal and the Runtime runs.
#[test]
fn rd2_t4_a_run_whose_device_fails_after_the_cable_left_ends_with_device_link_lost() {
    let run = handset_host(None);
    let host = &run.host;
    let package = super::gate_369s5::direct_run_package(&run._root, &run.state);
    list(host, DeviceLinkState::Device);
    list(host, DeviceLinkState::Absent);
    run.state.fail_capture.store(true, Ordering::Release);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let failed = host
        .process_request_for_test(
            &runtime_request(
                &ids,
                RuntimeOperation::run_contained_task(
                    POLICY_INSTANCE_ALIAS,
                    ids.mint_holder_id().expect("holder"),
                    ContainedTaskRequest::new(package.0.clone(), package.1.clone())
                        .expect("task request"),
                ),
            ),
            ConnectionId::new(399).expect("connection"),
        )
        .expect("the run's receipt");

    assert_eq!(failed.state(), RuntimeReceiptState::Failed, "{failed:?}");
    assert!(!failed.error_projection().expect("the run's failure").fatal);
    let outcome = failed.outcome();
    assert_eq!(outcome.code().as_str(), HostCode::DeviceLinkLost.as_str());
    assert_eq!(outcome_str(outcome, "device_link_state"), Some("absent"));
    assert_eq!(
        outcome_str(outcome, "device_label"),
        Some(handset_label(POLICY_INSTANCE_ALIAS).as_str())
    );
    let causes = outcome_links(outcome, CauseRelation::CausedBy);
    let [cause] = causes.as_slice() else {
        panic!("one cause: {causes:?}");
    };
    assert_eq!(cause.code().as_str(), "capture_backend_operation_failed");
    assert_eq!(run.state.input_count.load(Ordering::Acquire), 0);
    assert!(host.fatal_error().expect("runtime health").is_none());
    assert_no_serial(&all_events(host));
    run.host.close().expect("close host");
}
