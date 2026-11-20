// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #489 AVAIL-1 (0.12.76; register §1.493; `contracts/scheduling/README.md`, "An
//! unavailable instance is probed again"). The 11-20 production defect: a scheduled run's backend
//! open failed, its device self-check failed with it, the backend self-check gate withdrew the
//! instance's availability, and the ladder its failure matched was suppressed by an open cool-down
//! window; nothing evaluated the instance again for 15 hours (and a second instance, whose
//! cool-down had 27.5 s left, for 16), although a self-check passed at once when a person ran one.
//! Requirements (the coordinator's work order of 11-20, tests 1 to 5, and its fix round 1, items 1
//! to 6 and the rulings R1 to R4 of 23:29 JST):
//!
//! - T1: the replay, in both shapes, with production's own code: a failed paired open
//!   (`paired_backend_open_failed`, rule `task_capture_backend`), the ladder suppressed by its
//!   cool-down (30 minutes left; 27.5 s left), the instance unavailable; at the cool-down's end the
//!   instance's self-check runs: passing, it is available again and its due task is dispatched;
//!   failing, the recovery the cool-down held back is admitted and climbs;
//! - T2: with no cool-down, the instance is probed at the first interval, then at intervals
//!   multiplied by the factor up to the cap; the third probe passes, the instance is available and
//!   nothing probes it any more;
//! - T3: a paused, a fenced and an idle-stopped instance are not probed, nor one the global pause
//!   holds, whose next due probe runs once the global pause is lifted;
//! - T4: the attention is raised once when the unavailability lasted the configured time, not at
//!   every probe, and it ends with the unavailability;
//! - T5: the probes' state of the unavailable instance says since when, why and when the next probe
//!   is, and once the instance is available again its status is, byte for byte, what it was
//!   before; the status of an instance the probes take up carries no member a reader of 0.12.75
//!   does not know (fix round 1, item 1: the member `unavailable` is declared and not answered);
//! - T6 (fix round 1, item 3): a recovery held back is released only once the window in force has
//!   ended, not at the end its suppression named;
//! - T7 (fix round 2, item 2, the coordinator's ruling of 11-21, which replaces fix round 1's
//!   item 4): after a probe's worker panicked nothing probes the instance again in that
//!   unavailability, however far the clock runs; a person is told at once, once; the probes do
//!   not mark the Runtime fatal;
//! - T8 (fix round 1, ruling R4): a failed close of the kept session, and an open that failed
//!   without a report on an instance with an explicit host and port, take the instance into the
//!   probes; no `passed` self-check stands for it in between, and a passing probe makes it
//!   available;
//! - T9 (fix round 2, item 1): with two Nemu instances, a close after the opens that fails at
//!   every probe is one unavailability: its interval grows by the factor, one failed probe each,
//!   one warning at `attention_after_ms`, and the instance's availability that each probe's
//!   passing open republishes is withdrawn again inside that probe, which holds its key;
//! - T10 (fix round 2, item 3, the coordinator's ruling 甲): an open whose report leaves an entry
//!   `unknown` takes the instance into the probes with no cause; its probes and its warning carry
//!   no `failure_code`, and its `device.self_check` record (status `unknown`) says why.
//!
//! Fix round 2 (item 6): the replay's two instances are both the policy's configured identities,
//! as every registered instance of a real host is (`validate_policy_input_authority`): on the first
//! fix round's head its evaluation refused the bystander, configured nowhere
//! (`policy_instance_metadata_untrusted`).
//!
//! Every host runs on a manual Runtime clock that only the test moves; each sweep tick of the probes
//! is run by the test as well as by the lease sweep, so the waits below only bound a broken run.
//! The instance is the policy fixture's on a bound device endpoint (`with_device_endpoints`: an
//! explicit host and port, no discovery binding), so the backend self-check gates its availability.
//! The probes' state is read from the host (`unavailable_for_test`): no status answers it in this
//! release.

use super::gate_369_s6c::{pause, resume};
use super::gate_398_rd2::instance_facts;
use super::gate_480_pg2b::instance_pause_as;
use super::instance_workers::{all_events, ladder_setup, phases, wait_for_ladder_end, wait_until};
use super::*;
use crate::codes::HostCode;
use crate::host::UnavailableForTest;
use actingcommand_contract::outcome::{CodeStr, Outcome};
use actingcommand_contract::{
    DeviceSelfCheckStatus, EffectiveDeviceConfiguration, FactPayload, InstanceStuckRecovery,
    RecoveryLadderSuppression, RecoveryTriggerStage, RunId, RuntimeConfigManifest,
    RuntimeInstanceStatus, RuntimeLifecyclePhase, RuntimePayload, SchedulingPauseMode,
    SchedulingPauseScope, SchedulingResumeSelfCheck,
};
use actingcommand_device::DeviceResourceQuiescence;
use actingcommand_runtime_config::{
    DefaultsFile, annotate_settable, mechanism_rows, render_parameters,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const CAPTURE_OPEN_FAILED: &str = "capture_backend_open_failed";
const PAIRED_OPEN_FAILED: &str = "paired_backend_open_failed";
/// The connection the test's status reads come on.
const STATUS_CONNECTION: u64 = 4_891;
/// The fixture task's next occurrence after the run at the fixture's instant, plus one default
/// policy cadence (gate_382_h4's `NEXT_DISPATCH_UNIX_MS`).
const NEXT_DISPATCH_UNIX_MS: u64 = POLICY_NOW_UNIX_MS + 3_600_000 + 1_000;
/// The scheduler's lease TTL in the test configuration (`config`, `ladder_setup`).
const LEASE_TTL_MS: u64 = 5_000;
/// The second instance of the replay, beside the policy instance on the Nemu IPC.
const BYSTANDER_ALIAS: &str = "avail1.bystander";

/// A configuration manifest of the defaults file with the sections `file` names, as actingd
/// assembles it.
fn manifest(file: &Value) -> RuntimeConfigManifest {
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let mut parameters = render_parameters(&mechanism_rows(), &[], &defaults, file, &|_| None)
        .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// The capture open fails fatally after closing the one resource it acquired (confirmed).
fn capture_open_failure() -> DeviceError {
    DeviceError::fatal("fixture capture open got no positive connection handle")
        .with_resource_summary(DeviceResourceQuiescence::Confirmed, 1)
}

/// The paired Nemu open fails with nothing of its own left (confirmed) and no report of the device
/// layer, as the 11-20 open did: the open is observed as the pair's (`nemu_pair`, `failed`, no
/// attempt), its code `paired_backend_open_failed`.
fn paired_open_failure() -> DeviceError {
    DeviceError::fatal("fixture paired open failed before the device layer reported it")
        .with_resource_summary(DeviceResourceQuiescence::Confirmed, 1)
}

/// A close of the instance's kept session that failed with what it held released (confirmed).
fn confirmed_close_failure() -> DeviceError {
    DeviceError::fatal("fixture session close failed after releasing what it held")
        .with_resource_summary(DeviceResourceQuiescence::Confirmed, 1)
}

/// A device configuration whose capture is the Nemu IPC (neutral test values): with two such
/// instances each closes its device session at every lease end and after its preparation (the
/// multi-Nemu gate), so a run opens its backends afresh, as on the 11-20 host.
fn nemu_configuration(serial: &str) -> EffectiveDeviceConfiguration {
    EffectiveDeviceConfiguration {
        input_backend: "adb_shell_input".to_owned(),
        capture_backend: "nemu_ipc".to_owned(),
        input_adb: "<sealed-test>".to_owned(),
        capture_adb: "<sealed-test>".to_owned(),
        configured_serial: None,
        resolved_serial: serial.to_owned(),
        input_command_timeout_ms: 5_000,
        capture_command_timeout_ms: 5_000,
        capture_timeout_ms: 5_000,
        configured_mumu_root: None,
        configured_capture_dll: None,
    }
}

fn code(spelling: &str) -> CodeStr {
    Outcome::relayed(spelling).code().clone()
}

fn now_unix_ms(clock: &ManualRuntimeClock) -> u64 {
    clock.sample().expect("manual clock").unix_ms
}

/// The policy instance's status, as `Status` reports it.
fn instance_status(host: &RuntimeHost) -> RuntimeInstanceStatus {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let receipt = host
        .process_request_for_test(
            &runtime_request(&ids, RuntimeOperation::Status),
            ConnectionId::new(STATUS_CONNECTION).expect("connection"),
        )
        .expect("status receipt");
    let Some(RuntimeResult::Status { status }) = receipt.result() else {
        panic!("a status result: {receipt:?}");
    };
    status
        .instances()
        .iter()
        .find(|instance| instance.instance_alias() == POLICY_INSTANCE_ALIAS)
        .expect("the instance's status")
        .clone()
}

/// A person's self-check of the policy instance (`actingctl selfcheck`): its failure code.
fn person_self_check(host: &RuntimeHost) -> Option<String> {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let receipt = host
        .process_request_for_test(
            &runtime_request(
                &ids,
                RuntimeOperation::SelfCheckInstance {
                    instance_alias: POLICY_INSTANCE_ALIAS.to_owned(),
                },
            ),
            ConnectionId::new(4_890).expect("connection"),
        )
        .expect("self-check receipt");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let Some(RuntimeResult::InstanceSelfChecked {
        selfcheck: SchedulingResumeSelfCheck { failure_code, .. },
        ..
    }) = receipt.result()
    else {
        panic!("a self-check result: {receipt:?}");
    };
    failure_code.clone()
}

/// The policy instance as the probes hold it.
fn unavailable(host: &RuntimeHost) -> Option<UnavailableForTest> {
    host.unavailable_for_test(POLICY_INSTANCE_ALIAS)
        .expect("the probes' state")
}

/// The outcomes of the `runtime.failed` records whose code is `code`, in ledger order.
fn failed_outcomes(events: &[PersistedEvent], code: HostCode) -> Vec<(&PersistedEvent, &Outcome)> {
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(record))
                if record.outcome().code().as_str() == code.as_str() =>
            {
                Some((event, record.outcome()))
            }
            _ => None,
        })
        .collect()
}

fn records(host: &RuntimeHost, code: HostCode) -> usize {
    failed_outcomes(&all_events(host), code).len()
}

fn value(outcome: &Outcome, key: &str) -> Value {
    outcome.value(key).cloned().unwrap_or(Value::Null)
}

/// The `instance_preparation_finished` records of the Runtime's own recovery preparation (a probe
/// is one; a person's self-check is a connection preparation).
fn recovery_preparations(host: &RuntimeHost) -> usize {
    phases(&all_events(host))
        .iter()
        .filter(|(_, phase)| {
            matches!(
                phase,
                RuntimeLifecyclePhase::InstancePreparationFinished {
                    stage: RecoveryTriggerStage::RecoveryPreparation,
                    ..
                }
            )
        })
        .count()
}

/// The `recovery_ladder_suppressed` records of the host: reason, `until_unix_ms`, rule.
fn suppressions(host: &RuntimeHost) -> Vec<(RecoveryLadderSuppression, u64, Option<String>)> {
    phases(&all_events(host))
        .into_iter()
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::RecoveryLadderSuppressed {
                reason,
                until_unix_ms,
                rule,
            } => Some((reason, until_unix_ms, rule)),
            _ => None,
        })
        .collect()
}

/// The ladders that started with the failed run `run_id` as their trigger.
fn ladders_of(host: &RuntimeHost, run_id: RunId) -> usize {
    phases(&all_events(host))
        .iter()
        .filter(|(_, phase)| {
            matches!(
                phase,
                RuntimeLifecyclePhase::RecoveryLadderStarted { trigger, .. }
                    if trigger.run_id == Some(run_id)
            )
        })
        .count()
}

/// The keys of the instance's `backend.selfcheck.<entry>.status` facts that hold `passed`.
fn passed_selfchecks(host: &RuntimeHost, instance: InstanceId) -> Vec<String> {
    instance_facts(host, instance)
        .into_iter()
        .filter(|record| {
            record.key.starts_with("backend.selfcheck.")
                && record.value == ContractFactValue::String("passed".to_owned())
        })
        .map(|record| record.key)
        .collect()
}

/// The `panic_caught` records of a probe's payload (its failure category `run_unavailable_probe`).
fn probe_panics(events: &[PersistedEvent]) -> Vec<&PersistedEvent> {
    events
        .iter()
        .filter(|event| {
            matches!(
                event.payload(),
                EventPayload::Runtime(RuntimePayload::Failed(record))
                    if record.outcome().code().as_str() == "panic_caught"
                        && record
                            .detail()
                            .is_some_and(|detail| detail.category() == "run_unavailable_probe")
            )
        })
        .collect()
}

/// The value `event` publishes for the policy instance's `session.instance.available`, when it is
/// such a publication.
fn availability_published(event: &PersistedEvent) -> Option<bool> {
    let EventPayload::Fact(FactPayload::Published(payload)) = event.payload() else {
        return None;
    };
    let record = payload.record();
    let scope = FactScope::Instance {
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
    };
    if record.key != crate::fact_store::POLICY_INSTANCE_AVAILABLE_KEY || record.scope != scope {
        return None;
    }
    match record.content {
        FactContent::Inline {
            value: ContractFactValue::Boolean(value),
        } => Some(value),
        _ => None,
    }
}

/// The value of the policy instance's latest `session.instance.available` publication.
fn last_availability(host: &RuntimeHost) -> Option<bool> {
    all_events(host)
        .iter()
        .filter_map(availability_published)
        .next_back()
}

/// A host whose policy instance its device self-check made unavailable, on a manual clock at the
/// fixture's instant. Its start's preparation saw no passed open (`observed_open` false), so the
/// start withdrew its availability and nothing republished it; then a person's self-check failed
/// by its capture open. Stuck recovery is off, so no ladder starts. `file` holds the sections of
/// its configuration besides the defaults.
struct Probed {
    root: TempDir,
    host: RuntimeHost,
    state: Arc<FakeState>,
    clock: Arc<ManualRuntimeClock>,
    registered: InstanceId,
}

impl Probed {
    fn start(file: &Value) -> Self {
        let probed = Self::started(file);
        probed.fail_self_check();
        probed
    }

    /// The host, before the person's self-check.
    fn started(file: &Value) -> Self {
        Self::starting(file, false)
    }

    /// Fix round 1 (R4): the host whose start's preparation passed: the instance available, its
    /// passed self-check stored and its session kept; a second later on the clock.
    fn available(file: &Value) -> Self {
        let probed = Self::starting(file, true);
        probed.clock.advance(1_000);
        probed
    }

    /// With `passing`, the start's preparation passes; its first frame takes the clock's
    /// millisecond, as a real open takes time, so the start's withdrawal and the seed it
    /// republishes are not observed at the same instant (gate_ord1c, fix round 3).
    fn starting(file: &Value, passing: bool) -> Self {
        let root = TempDir::new().expect("tempdir");
        let state = Arc::new(FakeState::default());
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        if passing {
            state.observed_open.store(true, Ordering::Release);
            let stepped = Arc::clone(&clock);
            *state.capture_hook.lock().expect("capture hook") =
                Some(Box::new(move || stepped.advance(1)));
        }
        let registered = instance_id();
        let host = RuntimeHost::start(
            config(&root)
                .with_runtime_clock(clock.clone())
                .with_config_manifest(manifest(file))
                .with_stuck_recovery(BTreeMap::from([(
                    POLICY_INSTANCE_ALIAS.to_owned(),
                    InstanceStuckRecovery {
                        enabled: false,
                        cooldown_secs: 600,
                    },
                )])),
            Arc::new(
                FakeProvider::one(POLICY_INSTANCE_ALIAS, registered, Arc::clone(&state))
                    .with_device_endpoints(),
            ),
        )
        .expect("runtime host");
        Self {
            root,
            host,
            state,
            clock,
            registered,
        }
    }

    /// A person's self-check of the instance (`actingctl selfcheck`): its failure code.
    fn self_check(&self) -> Option<String> {
        person_self_check(&self.host)
    }

    /// A person's self-check of the instance whose capture open fails: it names the open.
    fn fail_self_check(&self) {
        self.state.observed_open.store(true, Ordering::Release);
        *self
            .state
            .capture_open_error
            .lock()
            .expect("capture open error") = Some(capture_open_failure());
        assert_eq!(self.self_check().as_deref(), Some(CAPTURE_OPEN_FAILED));
    }

    /// From now on the instance's opens pass.
    fn passes(&self) {
        *self
            .state
            .capture_open_error
            .lock()
            .expect("capture open error") = None;
    }

    fn now_unix_ms(&self) -> u64 {
        now_unix_ms(&self.clock)
    }

    /// Moves the clock to the next probe's time, as the probes hold it.
    fn advance_to_next_probe(&self) {
        let next = unavailable(&self.host)
            .and_then(|unavailable| unavailable.next_probe_unix_ms)
            .expect("a next probe");
        self.clock.advance(next.saturating_sub(self.now_unix_ms()));
    }

    fn tick(&self) {
        self.host
            .unavailable_probe_tick_for_test()
            .expect("sweep tick");
    }

    fn close(self) {
        assert!(self.host.fatal_error().expect("runtime health").is_none());
        self.host.close().expect("close host");
        drop(self.root);
    }
}

/// The probe keys `first_interval_ms`, `factor`, `max_interval_ms` and `attention_after_ms`.
fn probe_keys(first: u64, factor: u32, max: u64, attention: u64) -> Value {
    json!({
        "scheduling": {
            "unavailable_probe": {
                "first_interval_ms": first,
                "factor": factor,
                "max_interval_ms": max,
                "attention_after_ms": attention,
            }
        }
    })
}

/// Fix round 2 (item 6): the policy's facts for the two registered instances of a two-Nemu host,
/// whose aliases the host requires the configured identities to be
/// (`validate_policy_input_authority`): the policy instance as `policy_facts` names it, and the
/// bystander with its static identity under its own alias, configured unavailable, so the policy
/// has no candidate of it and the self-check gate leaves it to its seed (as
/// `gate_429_r6b2::two_instance_facts` configures its second instance).
fn two_instance_facts() -> EvaluationFacts {
    let mut facts = policy_facts();
    let mut bystander = facts.instances[0].clone();
    bystander.instance_id = BYSTANDER_ALIAS.to_owned();
    bystander.available = false;
    facts.instances.push(bystander);
    facts
}

/// The configured policy inputs of a host with both instances registered.
fn two_instance_inputs() -> PolicyInputSnapshot {
    PolicyInputSnapshot::new(two_instance_facts(), policy_resources())
}

/// The policy instance (`state`) and the bystander on bound device endpoints, both capturing
/// through the Nemu IPC: each closes its session after its preparation and at every lease end
/// (the multi-Nemu gate), so every open is afresh, as on the 11-20 host.
fn two_nemu_provider(state: &Arc<FakeState>) -> FakeProvider {
    let provider = FakeProvider::from_entries([
        (
            POLICY_INSTANCE_ALIAS.to_owned(),
            instance_id(),
            Arc::clone(state),
        ),
        (
            BYSTANDER_ALIAS.to_owned(),
            instance_id(),
            Arc::new(FakeState::default()),
        ),
    ])
    .with_device_endpoints();
    let policy = provider
        .resolve(POLICY_INSTANCE_ALIAS)
        .expect("the policy instance")
        .with_configuration(nemu_configuration("TESTSERIAL0001"));
    let bystander = provider
        .resolve(BYSTANDER_ALIAS)
        .expect("the bystander")
        .with_configuration(nemu_configuration("TESTSERIAL0002"));
    provider
        .with_resolved_for(POLICY_INSTANCE_ALIAS, policy)
        .with_resolved_for(BYSTANDER_ALIAS, bystander)
}

/// The policy instance's scheduled run of `request`, evaluated at `unix_ms` with the two
/// instances' facts (fix round 2, item 6) and admitted as `instance_workers::admit_ladder_run_at`
/// admits it; the catalog is active.
fn admit_policy_run(
    host: &RuntimeHost,
    request: &ContainedTaskRequest,
    unix_ms: u64,
    seed: u64,
) -> Box<PolicyRunContext> {
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &two_instance_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms,
                monotonic_ms: unix_ms,
            },
            seed,
            PolicyTrigger::FactsChanged,
        )
        .expect("evaluate policy dispatch");
    let evaluation = cycle.evaluation.as_ref().expect("policy evaluation");
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.instance_id == POLICY_INSTANCE_ALIAS)
        .unwrap_or_else(|| panic!("the policy instance's dispatch intent: {evaluation:#?}"))
        .clone();
    let reasons = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("dispatch reason chain")
        .clone();
    record_policy_approval(host, &intent);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_scheduled_policy_dispatch(&intent, &reasons, &policy_context(host, &intent), request)
        .expect("policy admission")
    else {
        panic!("expected a policy run context")
    };
    context
}

/// Fix round 2 (item 1): the next open's first frame and the next close of the instance's input
/// each take a millisecond of `clock`, as a real open and close take time, so the gate's
/// publications within one preparation (its withdrawal, the seed a passing open republishes, the
/// withdrawal a failed close publishes) are each observed after the one before.
fn step_open_and_close(state: &FakeState, clock: &Arc<ManualRuntimeClock>) {
    let opened = Arc::clone(clock);
    *state.capture_hook.lock().expect("capture hook") = Some(Box::new(move || opened.advance(1)));
    let closed = Arc::clone(clock);
    *state.input_close_hook.lock().expect("input close hook") =
        Some(Box::new(move || closed.advance(1)));
}

/// T1's fixture: the ladder fixture's scheduled run on the policy instance, on a bound device
/// endpoint, admitted while its self-check passes, then failed by its paired open (production's
/// code, fix round 1 item 5) while the ladder's cool-down window has `cooldown_left_ms` left (the
/// window an earlier ladder opened, opened here by the test). The policy instance and a bystander
/// both capture through the Nemu IPC, so the run opens its backends afresh; both are the policy's
/// configured identities (fix round 2, item 6). The probes' first interval and cap are an hour,
/// so the first probe is the cool-down's end's.
struct Replay {
    root: TempDir,
    host: RuntimeHost,
    state: Arc<FakeState>,
    clock: Arc<ManualRuntimeClock>,
    request: ContainedTaskRequest,
    run_id: RunId,
    until_unix_ms: u64,
}

fn replay(cooldown_left_ms: u64) -> Replay {
    let root = TempDir::new().expect("tempdir");
    let (host_config, request) = ladder_setup(root.path());
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state.observed_open.store(true, Ordering::Release);
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    // The start withdraws the instance's availability at the clock and its passing preparation
    // republishes the seed: the preparation's first frame takes the clock's millisecond, as a real
    // open takes time, so the two are not observed at the same instant (gate_ord1c, fix round 3).
    let stepped = Arc::clone(&clock);
    *state.capture_hook.lock().expect("capture hook") = Some(Box::new(move || stepped.advance(1)));
    let host = RuntimeHost::start(
        host_config
            .with_runtime_clock(clock.clone())
            .with_config_manifest(manifest(&probe_keys(3_600_000, 2, 3_600_000, 3_600_000)))
            .with_policy_inputs(two_instance_inputs()),
        Arc::new(two_nemu_provider(&state)),
    )
    .expect("physical runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate policy catalog");
    let context = admit_policy_run(&host, &request, POLICY_NOW_UNIX_MS, 7);
    clock.advance(1);
    let until_unix_ms = now_unix_ms(&clock) + cooldown_left_ms;
    host.open_recovery_cooldown_for_test(POLICY_INSTANCE_ALIAS, until_unix_ms)
        .expect("the ladder's cool-down window");
    *state.nemu_open_error.lock().expect("nemu open error") = Some(paired_open_failure());
    let failure = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the scheduled run fails");
    assert!(!failure.is_fatal(), "{}", failure.code());
    assert_eq!(failure.code(), PAIRED_OPEN_FAILED);
    let replay = Replay {
        root,
        host,
        state,
        clock,
        request,
        run_id: context.run_id(),
        until_unix_ms,
    };
    replay.assert_held_back();
    replay
}

impl Replay {
    /// The 11-20 state: one `recovery_ladder_suppressed` `cooldown` of the trigger item
    /// `task_capture_backend` until the window's end, no ladder, and the instance unavailable for
    /// its failed paired open, its next probe at the cool-down's end.
    fn assert_held_back(&self) {
        assert_eq!(
            suppressions(&self.host),
            vec![(
                RecoveryLadderSuppression::Cooldown,
                self.until_unix_ms,
                Some("task_capture_backend".to_owned())
            )]
        );
        assert_eq!(
            ladders_of(&self.host, self.run_id),
            0,
            "the cool-down suppressed the ladder"
        );
        assert_eq!(last_availability(&self.host), Some(false));
        let held = unavailable(&self.host).expect("the instance is unavailable");
        assert_eq!(held.cause, Some(code(PAIRED_OPEN_FAILED)));
        assert_eq!(held.probes, 0);
        assert_eq!(
            held.next_probe_unix_ms,
            Some(self.until_unix_ms),
            "the next probe is the cool-down's end"
        );
    }

    fn now_unix_ms(&self) -> u64 {
        now_unix_ms(&self.clock)
    }

    /// Business capacity sampled at the moved clock, so the held recovery's admission reads a
    /// fresh sample (it is fresh for twice the monitor's interval on the Runtime clock).
    fn sample_capacity(&self) {
        let sample_capacity = self
            .host
            .capacity_sampler_for_test()
            .expect("capacity owner");
        sample_capacity().expect("capacity sample");
    }

    fn tick(&self) {
        self.host
            .unavailable_probe_tick_for_test()
            .expect("sweep tick");
    }

    fn close(self) {
        assert!(self.host.fatal_error().expect("runtime health").is_none());
        self.host.close().expect("close host");
        drop(self.root);
    }
}

/// T1, the first instance's shape (the cool-down had 30 minutes left): at the cool-down's end the
/// self-check runs and passes; the instance is available again, its unavailability ends with
/// `instance_unavailable_probe_passed`, the held recovery is dropped (no ladder starts), and the
/// due task is dispatched and granted at its next occurrence.
#[test]
fn avail1_t1_at_the_cooldowns_end_a_passing_self_check_makes_the_instance_available_again() {
    let replay = replay(1_800_000);
    let host = &replay.host;
    *replay
        .state
        .nemu_open_error
        .lock()
        .expect("nemu open error") = None;
    replay
        .clock
        .advance(replay.until_unix_ms.saturating_sub(replay.now_unix_ms()));
    replay.tick();
    wait_until("the passing probe's record", || {
        records(host, HostCode::InstanceUnavailableProbePassed) == 1
    });
    assert_eq!(unavailable(host), None, "the instance is available again");
    assert_eq!(last_availability(host), Some(true));
    let events = all_events(host);
    let passed = failed_outcomes(&events, HostCode::InstanceUnavailableProbePassed);
    let [(_, passed)] = passed.as_slice() else {
        panic!("one passing probe: {passed:#?}");
    };
    assert_eq!(
        (value(passed, "attempts"), value(passed, "failure_code")),
        (json!(1), json!(PAIRED_OPEN_FAILED))
    );
    assert_eq!(records(host, HostCode::InstanceUnavailableProbeFailed), 0);
    // Fix round 2 (item 4): the held recovery the passing probe dropped ends in its trigger's
    // record, `recovery_ladder_suppressed` `cooldown` once more, with the end it was held to.
    let held_back = (
        RecoveryLadderSuppression::Cooldown,
        replay.until_unix_ms,
        Some("task_capture_backend".to_owned()),
    );
    wait_until("the held recovery's drop", || suppressions(host).len() == 2);
    assert_eq!(suppressions(host), vec![held_back.clone(), held_back]);

    // The due task is dispatched and granted at its next occurrence.
    replay
        .clock
        .advance(NEXT_DISPATCH_UNIX_MS.saturating_sub(replay.now_unix_ms()));
    replay.sample_capacity();
    let next = admit_policy_run(host, &replay.request, NEXT_DISPATCH_UNIX_MS, 8);
    assert_ne!(next.run_id(), replay.run_id);
    assert_eq!(
        ladders_of(host, replay.run_id),
        0,
        "a passing self-check drops the recovery the cool-down held back"
    );
    replay.close();
}

/// T1, the second instance's shape (the cool-down had 27.5 s left, nothing after it): at the
/// cool-down's end the self-check runs and fails; `instance_unavailable_probe_failed` records it,
/// and the recovery the cool-down held back is admitted after it, as without the cool-down: its
/// ladder starts with the failed run's trigger (stage task, the run, the failed paired open, the
/// item `task_capture_backend`), and no second suppression is recorded.
#[test]
fn avail1_t1_at_the_cooldowns_end_a_failing_self_check_admits_the_held_recovery() {
    let replay = replay(27_500);
    let host = &replay.host;
    replay.clock.advance(
        replay
            .until_unix_ms
            .saturating_sub(1)
            .saturating_sub(replay.now_unix_ms()),
    );
    replay.sample_capacity();
    replay.tick();
    assert_eq!(
        records(host, HostCode::InstanceUnavailableProbeFailed),
        0,
        "no probe before the cool-down's end"
    );
    replay.clock.advance(1);
    replay.tick();
    let run_id = replay.run_id;
    let started = || {
        phases(&all_events(host))
            .into_iter()
            .find_map(|(event, phase)| match phase {
                RuntimeLifecyclePhase::RecoveryLadderStarted { trigger, .. }
                    if trigger.run_id == Some(run_id) =>
                {
                    Some((event.sequence(), trigger))
                }
                _ => None,
            })
    };
    wait_until("the held recovery's ladder", || started().is_some());
    let (started_at, trigger) = started().expect("the held recovery's ladder");
    assert_eq!(trigger.stage, RecoveryTriggerStage::Task);
    assert_eq!(trigger.failure_code.as_str(), PAIRED_OPEN_FAILED);
    assert_eq!(trigger.rule.as_deref(), Some("task_capture_backend"));
    let events = all_events(host);
    let failed = failed_outcomes(&events, HostCode::InstanceUnavailableProbeFailed);
    let [(probe, outcome)] = failed.as_slice() else {
        panic!("one failed probe: {failed:#?}");
    };
    assert!(
        probe.sequence() < started_at,
        "the probe ran before the recovery"
    );
    assert_eq!(
        (value(outcome, "attempts"), value(outcome, "failure_code")),
        (json!(1), json!(PAIRED_OPEN_FAILED))
    );
    assert_eq!(
        phases(&events)
            .iter()
            .filter(|(_, phase)| matches!(
                phase,
                RuntimeLifecyclePhase::RecoveryLadderSuppressed { .. }
            ))
            .count(),
        1,
        "the held recovery is admitted, not suppressed again"
    );
    wait_for_ladder_end(host, POLICY_INSTANCE_ALIAS);
    replay.close();
}

/// T2: with no cool-down (stuck recovery off), `first_interval_ms` 60 000, `factor` 3 and
/// `max_interval_ms` 300 000: no probe a millisecond before the first interval; the first probe at
/// it and the second 180 000 after it fail, each recorded with its number and the next probe's
/// time; the third is 300 000 (the cap, not 540 000) after the second and passes: the instance is
/// available and, however long the clock runs, nothing probes it again.
#[test]
fn avail1_t2_an_unavailable_instance_is_probed_at_growing_intervals_up_to_the_cap() {
    const FIRST_MS: u64 = 60_000;
    const CAP_MS: u64 = 300_000;
    let probed = Probed::start(&probe_keys(FIRST_MS, 3, CAP_MS, 3_600_000));
    let host = &probed.host;
    let entered = unavailable(host).expect("the instance is unavailable after its failed check");
    assert_eq!(entered.probes, 0);
    assert_eq!(
        entered.next_probe_unix_ms,
        Some(probed.now_unix_ms() + FIRST_MS),
        "the first probe is one first interval after it became unavailable"
    );
    probed.clock.advance(FIRST_MS - 1);
    probed.tick();
    assert_eq!(
        unavailable(host).expect("still unavailable").probes,
        0,
        "no probe before the first interval"
    );
    for (probe, interval_ms) in [(1_u32, 180_000_u64), (2, CAP_MS)] {
        probed.advance_to_next_probe();
        probed.tick();
        wait_until("the failed probe's record", || {
            records(host, HostCode::InstanceUnavailableProbeFailed) == probe as usize
        });
        // Only the test moves the clock: the probe ended at the time it began.
        let ended = probed.now_unix_ms();
        let after = unavailable(host).expect("still unavailable");
        assert_eq!(after.probes, probe);
        assert_eq!(
            after.next_probe_unix_ms,
            Some(ended + interval_ms),
            "probe {probe}: the next interval is the one before times the factor, at most the cap"
        );
    }
    probed.passes();
    probed.advance_to_next_probe();
    probed.tick();
    wait_until("the passing probe's record", || {
        records(host, HostCode::InstanceUnavailableProbePassed) == 1
    });
    assert_eq!(unavailable(host), None, "the third probe made it available");
    assert_eq!(last_availability(host), Some(true));
    let events = all_events(host);
    let failed = failed_outcomes(&events, HostCode::InstanceUnavailableProbeFailed);
    assert_eq!(
        failed
            .iter()
            .map(|(event, outcome)| (
                event.severity(),
                value(outcome, "attempts"),
                value(outcome, "failure_code"),
                outcome.value("next_occurrence_unix_ms").is_some(),
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                EventSeverity::Warning,
                json!(1),
                json!(CAPTURE_OPEN_FAILED),
                true
            ),
            (
                EventSeverity::Warning,
                json!(2),
                json!(CAPTURE_OPEN_FAILED),
                true
            ),
        ]
    );
    let passed = failed_outcomes(&events, HostCode::InstanceUnavailableProbePassed);
    let [(passed_event, passed)] = passed.as_slice() else {
        panic!("one passing probe: {passed:#?}");
    };
    assert_eq!(passed_event.severity(), EventSeverity::Info);
    assert_eq!(value(passed, "attempts"), json!(3));
    let prepared = recovery_preparations(host);
    probed.clock.advance(CAP_MS * 10);
    probed.tick();
    probed.tick();
    assert_eq!(recovery_preparations(host), prepared, "no probe after it");
    assert_eq!(records(host, HostCode::InstanceUnavailableProbeFailed), 2);
    assert_eq!(records(host, HostCode::InstanceUnavailableProbePassed), 1);
    probed.close();
}

/// T3, the hold `hold` puts on the unavailable instance: however far past its first interval the
/// clock runs, no probe of it runs or is recorded.
fn assert_not_probed(probed: &Probed, hold: &str) {
    let host = &probed.host;
    probed.clock.advance(60_000 * 10);
    probed.tick();
    probed.tick();
    assert_eq!(
        (
            records(host, HostCode::InstanceUnavailableProbeFailed),
            records(host, HostCode::InstanceUnavailableProbePassed),
        ),
        (0, 0),
        "{hold}: no probe"
    );
}

/// T3, a paused instance: a person's pause (`stand`) holds it; it stays unavailable with no probe
/// and no self-check at all.
#[test]
fn avail1_t3_a_paused_instance_is_not_probed() {
    let probed = Probed::start(&probe_keys(60_000, 2, 1_800_000, 3_600_000));
    let host = &probed.host;
    let receipt = host
        .process_request_for_test(
            &instance_pause_as(POLICY_INSTANCE_ALIAS, SchedulingPauseMode::Stand),
            ConnectionId::new(4_892).expect("connection"),
        )
        .expect("pause receipt");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let prepared = recovery_preparations(host);
    assert_not_probed(&probed, "paused");
    assert_eq!(recovery_preparations(host), prepared);
    assert_eq!(
        unavailable(host).map(|unavailable| unavailable.probes),
        Some(0),
        "it stays unavailable, unprobed"
    );
    probed.close();
}

/// T3, fix round 1 (item 6): the global pause (the console's "pause all") holds the instance's
/// probes as its own pause does: ten first intervals on, none was queued and no self-check ran;
/// once the global pause is lifted, the next due probe runs.
#[test]
fn avail1_t3_the_global_pause_holds_the_probes_until_it_is_lifted() {
    let probed = Probed::start(&probe_keys(60_000, 2, 1_800_000, 3_600_000));
    let host = &probed.host;
    let paused = pause(host, SchedulingPauseScope::Global, 4_893);
    assert_eq!(paused.state(), RuntimeReceiptState::Completed, "{paused:?}");
    let prepared = recovery_preparations(host);
    assert_not_probed(&probed, "globally paused");
    assert_eq!(recovery_preparations(host), prepared, "no probe was queued");
    assert_eq!(
        unavailable(host).map(|unavailable| unavailable.probes),
        Some(0),
        "it stays unavailable, unprobed"
    );
    let resumed = resume(host, SchedulingPauseScope::Global, 4_894);
    assert_eq!(
        resumed.state(),
        RuntimeReceiptState::Completed,
        "{resumed:?}"
    );
    probed.advance_to_next_probe();
    probed.tick();
    wait_until("the next due probe's record", || {
        records(host, HostCode::InstanceUnavailableProbeFailed) == 1
    });
    assert_eq!(recovery_preparations(host), prepared + 1, "the probe ran");
    probed.close();
}

/// T3, a fenced instance: the fence takes it (its own recheck owns it), so it leaves the
/// unavailable instances and no probe of it runs.
#[test]
fn avail1_t3_a_fenced_instance_is_not_probed() {
    let probed = Probed::start(&probe_keys(60_000, 2, 1_800_000, 3_600_000));
    let host = &probed.host;
    host.fence_instance_for_test(POLICY_INSTANCE_ALIAS)
        .expect("fence the instance");
    let status = instance_status(host);
    assert!(status.resource_fence().is_some(), "{status:?}");
    assert_eq!(unavailable(host), None);
    assert_not_probed(&probed, "fenced");
    assert_eq!(unavailable(host), None);
    probed.close();
}

/// T3, an instance whose emulator the idle action holds stopped (`stop_emulator` done, its wake
/// not come; the configured action is `stop_emulator`, so no check wakes it): no probe and no
/// self-check of it runs.
#[test]
fn avail1_t3_an_idle_stopped_instance_is_not_probed() {
    let mut file = probe_keys(60_000, 2, 1_800_000, 3_600_000);
    file["idle"] = json!({"action": "stop_emulator"});
    let probed = Probed::start(&file);
    let host = &probed.host;
    host.hold_emulator_stopped_for_test(POLICY_INSTANCE_ALIAS)
        .expect("the idle action holds the emulator stopped");
    let prepared = recovery_preparations(host);
    assert_not_probed(&probed, "idle-stopped");
    assert_eq!(recovery_preparations(host), prepared);
    assert_eq!(
        unavailable(host).map(|unavailable| unavailable.probes),
        Some(0),
        "it stays unavailable, unprobed"
    );
    probed.close();
}

/// T4: `attention_after_ms` 120 000 and a first interval of 600 000. A millisecond before the
/// attention time nothing is raised; at it one warning `instance_unavailable_awaiting_person` under
/// the instance, with how long, the configured time, the probes so far, the cause and the next
/// probe's time, and the probes' state says `awaiting_person`. A failing probe after it raises
/// nothing more; the passing one ends the unavailability, and with it the attention, still one
/// record.
#[test]
fn avail1_t4_the_attention_is_raised_once_and_ends_with_the_unavailability() {
    const ATTENTION_MS: u64 = 120_000;
    const FIRST_MS: u64 = 600_000;
    let probed = Probed::start(&probe_keys(FIRST_MS, 2, 1_800_000, ATTENTION_MS));
    let host = &probed.host;
    let since = unavailable(host)
        .expect("the instance is unavailable")
        .since_unix_ms;
    probed
        .clock
        .advance((since + ATTENTION_MS - 1).saturating_sub(probed.now_unix_ms()));
    probed.tick();
    assert_eq!(
        records(host, HostCode::InstanceUnavailableAwaitingPerson),
        0
    );
    assert!(!unavailable(host).expect("unavailable").awaiting_person);
    probed.clock.advance(1);
    probed.tick();
    wait_until("the attention's record", || {
        records(host, HostCode::InstanceUnavailableAwaitingPerson) == 1
    });
    let state = unavailable(host).expect("unavailable");
    assert!(state.awaiting_person, "{state:?}");
    let events = all_events(host);
    let raised = failed_outcomes(&events, HostCode::InstanceUnavailableAwaitingPerson);
    let [(event, outcome)] = raised.as_slice() else {
        panic!("one attention: {raised:#?}");
    };
    assert_eq!(event.severity(), EventSeverity::Warning);
    assert_eq!(event.links().instance_id(), Some(&probed.registered));
    assert_eq!(
        (
            value(outcome, "elapsed_ms"),
            value(outcome, "limit_ms"),
            value(outcome, "attempts"),
            value(outcome, "failure_code"),
            value(outcome, "next_occurrence_unix_ms"),
        ),
        (
            json!(ATTENTION_MS),
            json!(ATTENTION_MS),
            json!(0),
            json!(CAPTURE_OPEN_FAILED),
            json!(state.next_probe_unix_ms.expect("the next probe")),
        )
    );
    probed.advance_to_next_probe();
    probed.tick();
    wait_until("the failed probe's record", || {
        records(host, HostCode::InstanceUnavailableProbeFailed) == 1
    });
    probed.tick();
    assert_eq!(
        records(host, HostCode::InstanceUnavailableAwaitingPerson),
        1,
        "not raised again at a probe"
    );
    assert!(unavailable(host).expect("unavailable").awaiting_person);
    probed.passes();
    probed.advance_to_next_probe();
    probed.tick();
    wait_until("the passing probe's record", || {
        records(host, HostCode::InstanceUnavailableProbePassed) == 1
    });
    assert_eq!(
        unavailable(host),
        None,
        "the attention ends with the unavailability"
    );
    probed.tick();
    assert_eq!(
        records(host, HostCode::InstanceUnavailableAwaitingPerson),
        1
    );
    probed.close();
}

/// T5: before its self-check failed the probes hold the instance with no cause (fix round 2, item
/// 3: its start's opens left their entries `unknown`, the fixture's provider observing no open);
/// after it, they hold since when, why, no probe and the next one's time; after a failed probe,
/// one probe and the next time; and once a probe passed, the instance's status is, byte for byte,
/// what it was before. Each read is a second after the step before it, past every takeover
/// cool-down.
#[test]
fn avail1_t5_the_probe_state_says_it_and_an_available_instance_is_unchanged() {
    const FIRST_MS: u64 = 60_000;
    let probed = Probed::started(&probe_keys(FIRST_MS, 2, 1_800_000, 3_600_000));
    let host = &probed.host;
    let since = probed.now_unix_ms();
    probed.clock.advance(1_000);
    let before = serde_json::to_vec(&instance_status(host)).expect("the status before");
    assert_eq!(
        unavailable(host),
        Some(UnavailableForTest {
            since_unix_ms: since,
            cause: None,
            probes: 0,
            next_probe_unix_ms: Some(since + FIRST_MS),
            awaiting_person: false,
        }),
        "taken up at its start, with no cause"
    );
    probed.fail_self_check();
    let failed_at = probed.now_unix_ms();
    assert_eq!(
        unavailable(host),
        Some(UnavailableForTest {
            since_unix_ms: since,
            cause: Some(code(CAPTURE_OPEN_FAILED)),
            probes: 0,
            next_probe_unix_ms: Some(failed_at + FIRST_MS),
            awaiting_person: false,
        })
    );
    probed.advance_to_next_probe();
    probed.tick();
    wait_until("the failed probe's record", || {
        records(host, HostCode::InstanceUnavailableProbeFailed) == 1
    });
    let ended = probed.now_unix_ms();
    assert_eq!(
        unavailable(host),
        Some(UnavailableForTest {
            since_unix_ms: since,
            cause: Some(code(CAPTURE_OPEN_FAILED)),
            probes: 1,
            next_probe_unix_ms: Some(ended + 2 * FIRST_MS),
            awaiting_person: false,
        })
    );
    probed.passes();
    probed.advance_to_next_probe();
    probed.tick();
    wait_until("the passing probe's record", || {
        records(host, HostCode::InstanceUnavailableProbePassed) == 1
    });
    assert_eq!(unavailable(host), None);
    probed.clock.advance(1_000);
    let after = serde_json::to_vec(&instance_status(host)).expect("the status after");
    assert_eq!(
        String::from_utf8_lossy(&after),
        String::from_utf8_lossy(&before),
        "an available instance's status is what it was"
    );
    probed.close();
}

/// An instance's status as Runtime 0.12.75 (`945727d4`) decodes it: the members of that release's
/// `RuntimeInstanceStatus`, each as JSON, unknown members refused as that structure refuses them.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstanceStatusAsReleased {
    instance_alias: Value,
    instance_id: Value,
    lease_active: Value,
    queued_request_count: Value,
    takeover_cooldown_active: Value,
    destructive_step_active: Value,
    preempt_requested: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    backend_provenance: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capabilities: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    adb_port: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resource_package: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    game_id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pause: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resource_fence: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device_held: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    emulator_state: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ladder_busy_hold: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    activity: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    player_hold: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    player_hold_operation: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    control: Option<Value>,
}

/// T5, fix round 1 (item 1): the status built while the instance is one the probes take up
/// serialises without the member `unavailable`, and a structure with the members of 0.12.75 that
/// refuses unknown ones decodes it, every member one it knows. (On the slice's first head the
/// status carried the member, which that structure refuses.)
#[test]
fn avail1_t5_the_status_of_an_unavailable_instance_decodes_as_0_12_75_decodes_it() {
    let probed = Probed::start(&probe_keys(60_000, 2, 1_800_000, 3_600_000));
    let host = &probed.host;
    assert!(
        unavailable(host).is_some(),
        "the probes take the instance up"
    );
    let status = serde_json::to_value(instance_status(host)).expect("the status");
    assert_eq!(status.get("unavailable"), None, "{status}");
    let released = serde_json::from_value::<InstanceStatusAsReleased>(status.clone())
        .expect("a reader of 0.12.75 decodes the status");
    assert_eq!(
        serde_json::to_value(&released).expect("the decoded status"),
        status,
        "every member is one 0.12.75 knows"
    );
    probed.close();
}

/// T6, fix round 1 (item 3): the recovery held back until T1 (the 27.5 s shape) while another
/// ladder's climb opened the instance's window to T2 = T1 + 60 s (opened here by the test, as that
/// climb would): at T1 the probe runs, fails and nothing is released, the next probe is T2; a
/// millisecond before T2 nothing more; at T2 the probe fails and the recovery is admitted, once.
#[test]
fn avail1_t6_a_held_recovery_waits_for_the_window_in_force() {
    let replay = replay(27_500);
    let host = &replay.host;
    let later_unix_ms = replay.until_unix_ms + 60_000;
    host.open_recovery_cooldown_for_test(POLICY_INSTANCE_ALIAS, later_unix_ms)
        .expect("another ladder's window");
    replay
        .clock
        .advance(replay.until_unix_ms.saturating_sub(replay.now_unix_ms()));
    replay.tick();
    wait_until("the probe at the held cool-down's end", || {
        records(host, HostCode::InstanceUnavailableProbeFailed) == 1
    });
    assert_eq!(
        ladders_of(host, replay.run_id),
        0,
        "nothing is released while a window holds the instance"
    );
    assert_eq!(
        unavailable(host).and_then(|unavailable| unavailable.next_probe_unix_ms),
        Some(later_unix_ms),
        "the next probe is the end of the window in force"
    );
    replay.clock.advance(
        later_unix_ms
            .saturating_sub(1)
            .saturating_sub(replay.now_unix_ms()),
    );
    replay.sample_capacity();
    replay.tick();
    assert_eq!(
        records(host, HostCode::InstanceUnavailableProbeFailed),
        1,
        "no probe before the window's end"
    );
    assert_eq!(ladders_of(host, replay.run_id), 0);
    replay.clock.advance(1);
    replay.tick();
    wait_until("the held recovery's ladder", || {
        ladders_of(host, replay.run_id) > 0
    });
    wait_for_ladder_end(host, POLICY_INSTANCE_ALIAS);
    assert_eq!(ladders_of(host, replay.run_id), 1, "released once");
    assert_eq!(records(host, HostCode::InstanceUnavailableProbeFailed), 2);
    replay.close();
}

/// T7, fix round 2 (item 2, the coordinator's ruling of 11-21, which takes back fix round 1's
/// "the schedule goes on"): the worker of the first probe panics (the instance workers' injected
/// panic, before the probe's preparation). The panic is recorded once, `panic_caught` under the
/// probe's failure category and its instance, and no probe record follows it; a person is told
/// at once, one `instance_unavailable_awaiting_person` with `failure_code` `panic_caught`, the
/// probe counted, and no next probe; the probes' state holds the instance waiting for a person
/// with no next probe. Once the panicked payload's key lapsed at its TTL, and however far the
/// clock then runs (ten caps, past `attention_after_ms`), no probe is queued or run, still one
/// warning, and the Runtime is not fatal (`close`).
#[test]
fn avail1_t7_after_a_probe_panicked_nothing_probes_the_instance_and_a_person_is_told() {
    const FIRST_MS: u64 = 60_000;
    const CAP_MS: u64 = 1_800_000;
    let probed = Probed::start(&probe_keys(FIRST_MS, 2, CAP_MS, 3_600_000));
    let host = &probed.host;
    let since = unavailable(host).expect("unavailable").since_unix_ms;
    host.inject_instance_worker_panic_for_test()
        .expect("inject a worker panic");
    probed.advance_to_next_probe();
    let panicked_at = probed.now_unix_ms();
    let prepared = recovery_preparations(host);
    probed.tick();
    wait_until("the panic's record", || {
        !probe_panics(&all_events(host)).is_empty()
    });
    let events = all_events(host);
    let panics = probe_panics(&events);
    let [caught] = panics.as_slice() else {
        panic!("one panic of the probe: {panics:#?}");
    };
    assert_eq!(caught.severity(), EventSeverity::Warning);
    assert_eq!(caught.links().instance_id(), Some(&probed.registered));
    let told = failed_outcomes(&events, HostCode::InstanceUnavailableAwaitingPerson);
    let [(event, outcome)] = told.as_slice() else {
        panic!("one attention at the panic: {told:#?}");
    };
    assert!(
        event.sequence() < caught.sequence(),
        "told at once, before the panic's own record"
    );
    assert_eq!(event.links().instance_id(), Some(&probed.registered));
    assert_eq!(
        (
            value(outcome, "failure_code"),
            value(outcome, "attempts"),
            value(outcome, "elapsed_ms"),
            value(outcome, "next_occurrence_unix_ms"),
        ),
        (
            json!("panic_caught"),
            json!(1),
            json!(panicked_at - since),
            Value::Null,
        )
    );
    assert_eq!(
        unavailable(host).map(|state| (
            state.probes,
            state.next_probe_unix_ms,
            state.awaiting_person
        )),
        Some((1, None, true)),
        "no next probe in this unavailability"
    );
    probed.clock.advance(LEASE_TTL_MS + 1);
    host.expire_due_leases_for_test()
        .expect("the sweep's lapse of due leases");
    wait_until("the panicked payload's key lapsed", || {
        host.instance_claims_for_test(POLICY_INSTANCE_ALIAS)
            .expect("instance claims")
            .0
            .is_none()
    });
    probed.clock.advance(CAP_MS * 10);
    probed.tick();
    probed.tick();
    assert_eq!(
        recovery_preparations(host),
        prepared,
        "no probe ran after it"
    );
    assert_eq!(
        (
            records(host, HostCode::InstanceUnavailableProbeFailed),
            records(host, HostCode::InstanceUnavailableProbePassed),
            records(host, HostCode::InstanceUnavailableAwaitingPerson),
            probe_panics(&all_events(host)).len(),
        ),
        (0, 0, 1, 1)
    );
    assert_eq!(
        unavailable(host).map(|state| (state.probes, state.next_probe_unix_ms)),
        Some((1, None)),
        "nothing was queued"
    );
    probed.close();
}

/// T8, fix round 1 (rulings R1, R2, R4): the instance available, its passed self-check stored and
/// its session kept; a person's self-check whose close of that session fails with what it held
/// released withdraws it (`backend_selfcheck:unchecked`) and takes it into the probes with the
/// close's code; its passed self-check no longer stands. No probe a millisecond before the first
/// interval; at it the probe passes: available again, with the passed self-check of that probe.
#[test]
fn avail1_t8_a_failed_close_of_the_kept_session_enters_the_probes() {
    const FIRST_MS: u64 = 60_000;
    let probed = Probed::available(&probe_keys(FIRST_MS, 2, 1_800_000, 3_600_000));
    let host = &probed.host;
    assert_eq!(last_availability(host), Some(true), "the start passed");
    assert!(!passed_selfchecks(host, probed.registered).is_empty());
    assert_eq!(unavailable(host), None);
    *probed.state.close_error.lock().expect("close error") = Some(confirmed_close_failure());
    let failure = probed.self_check().expect("the close fails the self-check");
    let entered_at = probed.now_unix_ms();
    let entered = unavailable(host).expect("the failed close takes the instance into the probes");
    assert_eq!(entered.cause, Some(code(&failure)));
    assert_eq!(entered.probes, 0);
    assert_eq!(entered.next_probe_unix_ms, Some(entered_at + FIRST_MS));
    assert_eq!(last_availability(host), Some(false));
    assert_eq!(
        passed_selfchecks(host, probed.registered),
        Vec::<String>::new(),
        "no passed self-check stands for an instance the probes take up"
    );
    *probed.state.close_error.lock().expect("close error") = None;
    probed.clock.advance(FIRST_MS - 1);
    probed.tick();
    assert_eq!(
        unavailable(host).map(|unavailable| unavailable.probes),
        Some(0),
        "no probe before the first interval"
    );
    assert_eq!(
        passed_selfchecks(host, probed.registered),
        Vec::<String>::new()
    );
    probed.clock.advance(1);
    probed.tick();
    wait_until("the passing probe's record", || {
        records(host, HostCode::InstanceUnavailableProbePassed) == 1
    });
    assert_eq!(unavailable(host), None);
    assert_eq!(last_availability(host), Some(true));
    assert!(!passed_selfchecks(host, probed.registered).is_empty());
    assert_eq!(records(host, HostCode::InstanceUnavailableProbeFailed), 0);
    probed.close();
}

/// T8, fix round 1 (rulings R1, R4): the same instance (an explicit host and port, no discovery
/// binding, so no emulator watch reads it); a person's self-check whose open ends without a report
/// (its session dies at the open's first frame) records no self-check fact, withdraws the instance
/// and takes it into the probes with the open's code; at the first interval the probe passes.
#[test]
fn avail1_t8_an_open_without_a_report_on_an_explicit_endpoint_enters_the_probes() {
    const FIRST_MS: u64 = 60_000;
    let probed = Probed::available(&probe_keys(FIRST_MS, 2, 1_800_000, 3_600_000));
    let host = &probed.host;
    assert_eq!(last_availability(host), Some(true), "the start passed");
    probed
        .state
        .panic_capture_once
        .store(true, Ordering::Release);
    let failure = probed.self_check().expect("the open fails the self-check");
    let entered_at = probed.now_unix_ms();
    let entered = unavailable(host).expect("the open takes the instance into the probes");
    assert_eq!(entered.cause, Some(code(&failure)));
    assert_eq!(entered.probes, 0);
    assert_eq!(entered.next_probe_unix_ms, Some(entered_at + FIRST_MS));
    assert_eq!(last_availability(host), Some(false));
    assert!(
        instance_facts(host, probed.registered)
            .iter()
            .all(|record| !record.key.starts_with("backend.selfcheck.")),
        "the open recorded no report"
    );
    probed.clock.advance(FIRST_MS - 1);
    probed.tick();
    assert_eq!(
        unavailable(host).map(|unavailable| unavailable.probes),
        Some(0),
        "no probe before the first interval"
    );
    probed.clock.advance(1);
    probed.tick();
    wait_until("the passing probe's record", || {
        records(host, HostCode::InstanceUnavailableProbePassed) == 1
    });
    assert_eq!(unavailable(host), None);
    assert_eq!(last_availability(host), Some(true));
    probed.close();
}

/// Whether `event` is the `instance_preparation_finished` of the Runtime's own recovery
/// preparation (a probe's; a person's self-check is a connection preparation).
fn recovery_preparation_finished(event: &PersistedEvent) -> bool {
    matches!(
        event.payload(),
        EventPayload::Runtime(RuntimePayload::LifecycleObserved(observed))
            if matches!(
                observed.phase(),
                RuntimeLifecyclePhase::InstancePreparationFinished {
                    stage: RecoveryTriggerStage::RecoveryPreparation,
                    ..
                }
            )
    )
}

/// T9, fix round 2 (item 1): the replay's two Nemu instances, both configured; the policy
/// instance's start passed, then every close of its input fails with what it held released
/// (confirmed). A person's self-check so closes after its opens and takes the instance into the
/// probes with the close's code. At each of three probes the opens pass (the gate republishes the
/// seed) and the close fails again: one `instance_unavailable_probe_failed` (its number, the
/// close's code), and the same unavailability (since, cause and count kept), the next interval
/// 120 s, 240 s, 480 s (from 60 s, factor 2); one `instance_unavailable_awaiting_person` at
/// `attention_after_ms` (180 s), at the second probe's tick. The instance is never answered as
/// available to a candidate in between: each `session.instance.available` `true` a probe's
/// passing open publishes is followed by the close's `false` before that probe's own
/// `instance_preparation_finished`, inside the probe's preparation, which holds the instance's key
/// from its claim's grant until after that record (a dispatch needs a free key); the last
/// publication is `false`. The opens and closes take a millisecond each of the manual clock
/// (`step_open_and_close`), as the start's open does in the replay.
#[test]
fn avail1_t9_a_close_that_fails_after_every_probe_is_one_unavailability_whose_intervals_grow() {
    const FIRST_MS: u64 = 60_000;
    const ATTENTION_MS: u64 = 180_000;
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.observed_open.store(true, Ordering::Release);
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let stepped = Arc::clone(&clock);
    *state.capture_hook.lock().expect("capture hook") = Some(Box::new(move || stepped.advance(1)));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_config_manifest(manifest(&probe_keys(FIRST_MS, 2, 600_000, ATTENTION_MS)))
            .with_policy_inputs(two_instance_inputs()),
        Arc::new(two_nemu_provider(&state)),
    )
    .expect("physical runtime host");
    assert_eq!(last_availability(&host), Some(true), "the start passed");
    clock.advance(1_000);
    *state.close_error.lock().expect("close error") = Some(confirmed_close_failure());
    step_open_and_close(&state, &clock);
    let failure = person_self_check(&host).expect("the close after the opens fails the self-check");
    let entered = unavailable(&host).expect("the failed close takes the instance into the probes");
    assert_eq!(
        (
            entered.cause.clone(),
            entered.probes,
            entered.next_probe_unix_ms
        ),
        (
            Some(code(&failure)),
            0,
            Some(now_unix_ms(&clock) + FIRST_MS)
        )
    );
    let since = entered.since_unix_ms;
    let from = all_events(&host).len();
    let mut ticks = Vec::new();
    for (probe, interval_ms) in [(1_u32, 120_000_u64), (2, 240_000), (3, 480_000)] {
        let due = unavailable(&host)
            .and_then(|unavailable| unavailable.next_probe_unix_ms)
            .expect("a next probe");
        // Armed before the clock reaches the probe, which the lease sweep may queue first.
        step_open_and_close(&state, &clock);
        clock.advance(due.saturating_sub(now_unix_ms(&clock)));
        ticks.push(now_unix_ms(&clock));
        host.unavailable_probe_tick_for_test().expect("sweep tick");
        wait_until("the failed probe's record", || {
            records(&host, HostCode::InstanceUnavailableProbeFailed) == probe as usize
        });
        let after = unavailable(&host).expect("the same unavailability");
        assert_eq!(
            (
                after.since_unix_ms,
                after.cause.clone(),
                after.probes,
                after.next_probe_unix_ms
            ),
            (
                since,
                Some(code(&failure)),
                probe,
                Some(now_unix_ms(&clock) + interval_ms)
            ),
            "probe {probe}: one unavailability, its interval multiplied by the factor"
        );
    }
    let events = all_events(&host);
    let failed = failed_outcomes(&events, HostCode::InstanceUnavailableProbeFailed);
    assert_eq!(
        failed
            .iter()
            .map(|(_, outcome)| (value(outcome, "attempts"), value(outcome, "failure_code")))
            .collect::<Vec<_>>(),
        [1, 2, 3]
            .into_iter()
            .map(|probe| (json!(probe), json!(failure.as_str())))
            .collect::<Vec<_>>()
    );
    assert_eq!(records(&host, HostCode::InstanceUnavailableProbePassed), 0);
    let told = failed_outcomes(&events, HostCode::InstanceUnavailableAwaitingPerson);
    let [(_, told)] = told.as_slice() else {
        panic!("one attention: {told:#?}");
    };
    assert_eq!(
        (
            value(told, "elapsed_ms"),
            value(told, "attempts"),
            value(told, "failure_code"),
            value(told, "next_occurrence_unix_ms"),
        ),
        (
            json!(ticks[1] - since),
            json!(1),
            json!(failure.as_str()),
            json!(ticks[1]),
        ),
        "at the second probe's tick, the first at which it lasted attention_after_ms"
    );
    let mut standing = None;
    let mut republished = 0_usize;
    let mut prepared = 0_usize;
    for event in &events[from..] {
        if let Some(available) = availability_published(event) {
            standing = Some(available);
            republished += usize::from(available);
        }
        if recovery_preparation_finished(event) {
            prepared += 1;
            assert_eq!(
                standing,
                Some(false),
                "probe {prepared}: the seed its open republished is withdrawn again inside it"
            );
        }
    }
    assert_eq!((prepared, republished, standing), (3, 3, Some(false)));
    *state.close_error.lock().expect("close error") = None;
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
    drop(root);
}

/// T10, fix round 2 (item 3, the coordinator's ruling 甲): the fixture's provider observes no
/// open, so the start's opens leave their entries `unknown`; their `device.self_check` records say
/// status `unknown` with no failure code, the gate withdraws the instance, and the probes take it
/// up with no cause. The first probe fails the same way: one `instance_unavailable_probe_failed`
/// with no `failure_code` and the next probe's time; at `attention_after_ms` one
/// `instance_unavailable_awaiting_person` with no `failure_code`; once the opens pass, a probe
/// passes, `instance_unavailable_probe_passed` with no `failure_code`, and the instance is
/// available.
#[test]
fn avail1_t10_an_open_that_leaves_an_entry_unknown_enters_the_probes_with_no_cause() {
    const FIRST_MS: u64 = 60_000;
    const ATTENTION_MS: u64 = 120_000;
    let probed = Probed::started(&probe_keys(FIRST_MS, 2, 1_800_000, ATTENTION_MS));
    let host = &probed.host;
    let checks = all_events(host)
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::DeviceSelfCheck(check)) => {
                Some((check.status(), check.failure_code().is_some()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        !checks.is_empty()
            && checks
                .iter()
                .all(|check| *check == (DeviceSelfCheckStatus::Unknown, false)),
        "the start's self-check records say unknown, with no code: {checks:?}"
    );
    assert_eq!(last_availability(host), Some(false));
    let entered = unavailable(host).expect("the unknown opens take the instance into the probes");
    assert_eq!(
        (
            entered.cause.clone(),
            entered.probes,
            entered.next_probe_unix_ms
        ),
        (None, 0, Some(probed.now_unix_ms() + FIRST_MS))
    );
    let since = entered.since_unix_ms;
    probed.advance_to_next_probe();
    probed.tick();
    wait_until("the failed probe's record", || {
        records(host, HostCode::InstanceUnavailableProbeFailed) == 1
    });
    let ended = probed.now_unix_ms();
    let events = all_events(host);
    let failed = failed_outcomes(&events, HostCode::InstanceUnavailableProbeFailed);
    let [(_, failed)] = failed.as_slice() else {
        panic!("one failed probe: {failed:#?}");
    };
    assert_eq!(
        (
            value(failed, "attempts"),
            value(failed, "failure_code"),
            value(failed, "next_occurrence_unix_ms"),
        ),
        (json!(1), Value::Null, json!(ended + 2 * FIRST_MS))
    );
    assert_eq!(
        unavailable(host).map(|unavailable| unavailable.cause),
        Some(None),
        "still no cause"
    );
    probed
        .clock
        .advance((since + ATTENTION_MS).saturating_sub(probed.now_unix_ms()));
    probed.tick();
    wait_until("the attention's record", || {
        records(host, HostCode::InstanceUnavailableAwaitingPerson) == 1
    });
    let events = all_events(host);
    let told = failed_outcomes(&events, HostCode::InstanceUnavailableAwaitingPerson);
    let [(_, told)] = told.as_slice() else {
        panic!("one attention: {told:#?}");
    };
    assert_eq!(
        (
            value(told, "elapsed_ms"),
            value(told, "attempts"),
            value(told, "failure_code"),
        ),
        (json!(ATTENTION_MS), json!(1), Value::Null)
    );
    probed.state.observed_open.store(true, Ordering::Release);
    probed.advance_to_next_probe();
    probed.tick();
    wait_until("the passing probe's record", || {
        records(host, HostCode::InstanceUnavailableProbePassed) == 1
    });
    let events = all_events(host);
    let passed = failed_outcomes(&events, HostCode::InstanceUnavailableProbePassed);
    let [(_, passed)] = passed.as_slice() else {
        panic!("one passing probe: {passed:#?}");
    };
    assert_eq!(
        (value(passed, "attempts"), value(passed, "failure_code")),
        (json!(2), Value::Null)
    );
    assert_eq!(unavailable(host), None);
    assert_eq!(last_availability(host), Some(true));
    probed.close();
}
