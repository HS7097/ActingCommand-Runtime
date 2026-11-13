// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #437 MAPP-2a (register §1.340; `contracts/device-group.md`, "The switch before a
//! run"): the parts of the switch before a member's run, through the host, and their one entry.
//!
//! Three fixture instances A, B and D are one device group of several (`device.one`) and C is
//! alone on its device; each has its own application and the neutral startup package, whose one
//! step restarts the application and whose home is the fake device's first frame. The load
//! refuses every group of several (actingd's MAPP-1 gate), so the host is handed the member →
//! device key map and the group's name as the assembly builds them. B's run holds the device's
//! key through an admitted policy dispatch whose lease covers its run (`RUN_DEADLINE_MS`). A
//! manual Runtime clock moves only by the kernel's waits; nothing sleeps. Every expected value
//! is the contract's or the coordinator's ruling; every comparison is within one host. Neutral
//! names only.

use super::instance_workers::phases;
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    APPLICATION_FOREGROUND_FACT_KEY, InstanceStuckRecovery, RuntimeFactRecord, RuntimeFactScope,
    RuntimeLifecyclePhase,
};
use actingcommand_scheduler::ClaimKind;
use serde_json::{Value, json};

const A: &str = POLICY_INSTANCE_ALIAS;
const B: &str = POLICY_INSTANCE_ALIAS_B;
/// An instance alone on its own device, beside the group.
const C: &str = "fixture-instance-c";
/// The group's third member.
const D: &str = "fixture-instance-d";
const TASK_A: &str = "fixture.member-a";
const TASK_B: &str = "fixture.member-b";
const GROUP: &str = "device.one";
const SEED: u64 = 17;
const APP_A: &str = "neutral.member.a";
const APP_B: &str = "neutral.member.b";
const APP_C: &str = "neutral.member.c";
const APP_D: &str = "neutral.member.d";
/// A package no member is assigned.
const APP_OTHER: &str = "neutral.other.application";
/// The fake device's first pixel before any input: the startup package's home.
const HOME: [u8; 3] = [255, 0, 0];
/// The response deadline of B's run, which sizes the lease of its admitted dispatch.
const RUN_DEADLINE_MS: u64 = 600_000;
/// The response deadline of every startup package.
const STARTUP_DEADLINE_MS: u64 = 60_000;
/// The time B's run asks to keep on the key after the switch.
const AFTER_MS: u64 = 60_000;
/// The connection of every policy run's key (the host's `POLICY_CONNECTION_VALUE`).
const POLICY_CONNECTION: u64 = u64::MAX;
const TASK_PAGE: &str = "task.page";

// ---- The fixture.

/// One task of `instance` with `priority`, due at every evaluation (a local interval clock of
/// 1 ms), from the fixture catalog's generic task (as MAPP-1b's gate builds it).
fn member_task(base: &Value, id: &str, instance: &str, priority: u32) -> Value {
    let mut spec = base.clone();
    spec["id"] = json!(id);
    spec["scope"] = json!({"kind": "instance", "instance_id": instance});
    spec["priority"] = json!(priority);
    spec["trigger"] = json!({
        "kind": "clock",
        "schedule": {"kind": "interval", "clock_source": {"kind": "local"},
                     "every_ms": 1, "anchor_ms": 0}
    });
    spec["cooldown_ms"] = json!(0);
    spec["instance_overrides"] = json!([]);
    spec["produces"] = json!([]);
    spec["on_failure"]["escalation_threshold"] = json!(1000);
    spec["loop_budget"] = json!({
        "daily_limit": 48,
        "window_iteration_limit": 48,
        "max_runtime_ms": 300_000
    });
    spec
}

/// The catalog: TASK_A on A (priority 200) and TASK_B on B (300), so B's task wins the device.
fn sources() -> CatalogSources {
    let mut sources = policy_sources(1);
    let mut document: Value = serde_json::from_slice(&sources.tasks.bytes).expect("fixture tasks");
    let base = document["tasks"][0].clone();
    document["tasks"] = json!([
        member_task(&base, TASK_A, A, 200),
        member_task(&base, TASK_B, B, 300),
    ]);
    sources.tasks.bytes = serde_json::to_vec_pretty(&document).expect("task bytes");
    let mut activity: Value =
        serde_json::from_slice(&sources.activity.bytes).expect("fixture activity");
    let mut profile = activity["profiles"][1].clone();
    profile["daily_budget"] = json!(48);
    profile["max_window_iterations"] = json!(48);
    profile["minimum_interval_ms"] = json!(1);
    profile["maximum_interval_ms"] = json!(1);
    activity["profiles"] = json!([profile]);
    sources.activity.bytes = serde_json::to_vec_pretty(&activity).expect("activity bytes");
    sources
}

/// The neutral startup package (an application entry): from any screen its one step restarts
/// the instance's application, to `home`, which its recognition pack knows by pixel 0.
fn startup_package(root: &TempDir) -> PathBuf {
    let space = json!({"width": 16, "height": 9});
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.test.startup",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": space.clone(),
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": space.clone(),
        "entry_page": "any",
        "target_page": "home",
        "operations": [
            {"id": "start", "from": "any", "to": "home", "application": {"action": "restart"}}
        ]
    });
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": space,
        "defaults": {"color_max_distance": 0.0},
        "targets": [{"type": "color", "id": "page/home",
                     "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": HOME}]
    });
    let pages = json!({"schema_version": "0.3", "pages": [
        {"id": "neutral/home", "required": ["page/home"], "optional": [], "forbidden": []}
    ]});
    let files: [(&str, Value); 6] = [
        ("control.json", control),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", pack),
        ("resources/recognition/neutral.test.pages.json", pages),
        (
            "resources/navigation/neutral.test.navigation.json",
            json!({"navigation": []}),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in files {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&serde_json::to_vec(&value).expect("package JSON"))
            .expect("zip content");
    }
    let path = root.path().join("startup.zip");
    fs::write(&path, zip.finish().expect("finish zip").into_inner())
        .expect("write the startup package");
    path
}

/// A host whose instances A, B and D are the device group `GROUP` and C is alone, each with its
/// own application and the startup package, on a manual Runtime clock at the fixture's noon,
/// stuck recovery off, the catalog of [`sources`] active.
struct Switching {
    host: RuntimeHost,
    clock: Arc<ManualRuntimeClock>,
    facts: EvaluationFacts,
    resources: EvaluationResources,
    package: PathBuf,
    ids: BTreeMap<&'static str, InstanceId>,
    states: BTreeMap<&'static str, Arc<FakeState>>,
    _root: TempDir,
}

impl Switching {
    fn start() -> Self {
        let root = TempDir::new().expect("tempdir");
        let package = startup_package(&root);
        let members = [(A, APP_A), (B, APP_B), (C, APP_C), (D, APP_D)];
        let ids = members
            .iter()
            .map(|(alias, _)| (*alias, instance_id()))
            .collect::<BTreeMap<_, _>>();
        let states = members
            .iter()
            .map(|(alias, application)| {
                let state = Arc::new(FakeState::default());
                state.observed_open.store(true, Ordering::Release);
                state.physical_task_geometry.store(true, Ordering::Release);
                *state.application_id.lock().expect("fake application id") =
                    Some((*application).to_owned());
                (*alias, state)
            })
            .collect::<BTreeMap<_, _>>();
        let device = ids[A].min(ids[B]).min(ids[D]);
        let entries = members
            .iter()
            .map(|(alias, _)| ((*alias).to_owned(), ids[alias], Arc::clone(&states[alias])))
            .collect::<Vec<_>>();
        let mut facts = policy_facts();
        facts.outcomes.clear();
        facts.instances.push(InstanceSnapshot {
            instance_id: B.to_owned(),
            server_id: "fixture-server-b".to_owned(),
            game_id: "fixture-game-a".to_owned(),
            host_id: "fixture-host-b".to_owned(),
            available: true,
            capability_operation_ids: vec!["operation.observe".to_owned()],
            preferred_task_ids: Vec::new(),
        });
        // The policy's configured identities are the registered aliases, all four
        // (`validate_policy_input_authority`); C and D take A's static identity and are not
        // available to the policy, which has no task of theirs.
        for alias in [C, D] {
            let mut identity = facts.instances[0].clone();
            identity.instance_id = alias.to_owned();
            identity.available = false;
            facts.instances.push(identity);
        }
        let resources = pending_policy_resources();
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let startup = ContainedTaskRequest::new(package.to_string_lossy().into_owned())
            .and_then(|request| request.with_response_deadline_ms(STARTUP_DEADLINE_MS))
            .expect("startup package request");
        let host = RuntimeHost::start(
            config(&root)
                .with_policy_inputs(PolicyInputSnapshot::new(facts.clone(), resources.clone()))
                .with_runtime_clock(clock.clone())
                .with_stuck_recovery(
                    [A, B, C, D]
                        .into_iter()
                        .map(|alias| {
                            (
                                alias.to_owned(),
                                InstanceStuckRecovery {
                                    enabled: false,
                                    cooldown_secs: 600,
                                },
                            )
                        })
                        .collect(),
                )
                .with_startup_packages(
                    [A, B, C, D]
                        .into_iter()
                        .map(|alias| (alias.to_owned(), startup.clone()))
                        .collect(),
                )
                .with_device_keys(BTreeMap::from([
                    (ids[A], device),
                    (ids[B], device),
                    (ids[D], device),
                ]))
                .with_device_group_names(BTreeMap::from([
                    (A.to_owned(), GROUP.to_owned()),
                    (B.to_owned(), GROUP.to_owned()),
                    (D.to_owned(), GROUP.to_owned()),
                ])),
            Arc::new(FakeProvider::from_entries(entries)),
        )
        .expect("start the device group host");
        host.activate_policy_catalog(&sources())
            .expect("activate the catalog");
        Self {
            host,
            clock,
            facts,
            resources,
            package,
            ids,
            states,
            _root: root,
        }
    }

    /// One evaluation at the fixture's noon, the capacity sampled first, and B's dispatch
    /// admitted with its run's request: B's run holds the device's key.
    fn grant_b(&self) -> Box<PolicyRunContext> {
        let unix_ms = POLICY_NOW_UNIX_MS;
        self.clock.set_unix_ms(unix_ms);
        let sample_capacity = self
            .host
            .capacity_sampler_for_test()
            .expect("capacity owner");
        sample_capacity().expect("capacity sample");
        let cycle = self
            .host
            .evaluate_policy_cycle_with_test_inputs(
                &self.facts,
                &self.resources,
                EvaluationTime {
                    unix_ms,
                    monotonic_ms: unix_ms,
                },
                SEED,
                PolicyTrigger::Recovery,
            )
            .unwrap_or_else(|error| panic!("evaluate: {error}"));
        let evaluation = cycle.evaluation.as_ref().expect("an evaluated cycle");
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == TASK_B)
            .unwrap_or_else(|| panic!("B's intent: {evaluation:#?}"))
            .clone();
        let chain = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("the intent's reason chain")
            .clone();
        record_policy_approval(&self.host, &intent);
        let run = ContainedTaskRequest::new(self.package.to_string_lossy().into_owned())
            .and_then(|request| request.with_response_deadline_ms(RUN_DEADLINE_MS))
            .expect("B's run request");
        match self
            .host
            .admit_scheduled_policy_dispatch(
                &intent,
                &chain,
                &policy_context(&self.host, &intent),
                &run,
            )
            .expect("admit B's dispatch")
        {
            PolicyDispatchAdmission::Granted { context } => context,
            other => panic!("B's dispatch is granted: {other:?}"),
        }
    }

    fn id(&self, alias: &str) -> InstanceId {
        self.ids[alias]
    }

    fn state(&self, alias: &str) -> &FakeState {
        &self.states[alias]
    }

    /// Every application command that reached `alias`'s device.
    fn actions(&self, alias: &str) -> Vec<ApplicationLifecycleAction> {
        self.state(alias)
            .application_actions
            .lock()
            .expect("fake application actions")
            .clone()
    }

    /// `alias`'s foreground answers `front` from now on (`None`: no resumed activity).
    fn front(&self, alias: &str, front: Option<&'static str>) {
        *self
            .state(alias)
            .foreground_script
            .lock()
            .expect("foreground script") =
            Some(Arc::new(move |_: usize, _: usize| front.map(str::to_owned)));
    }

    /// `alias`'s program fact `key` recorded with `value` now.
    fn record_fact(&self, alias: &str, key: &str, value: &str) {
        self.host
            .record_runtime_fact_for_test(RuntimeFactRecord {
                scope: RuntimeFactScope::Instance {
                    instance_id: self.id(alias),
                },
                key: key.to_owned(),
                value: ContractFactValue::String(value.to_owned()),
                observed_at_unix_ms: POLICY_NOW_UNIX_MS,
                source: OriginModule::Runtime,
                ttl_ms: None,
            })
            .expect("record the fact");
    }

    /// Whether `alias` holds the program fact `key`.
    fn has_fact(&self, alias: &str, key: &str) -> bool {
        self.host
            .runtime_fact_for_test(
                &RuntimeFactScope::Instance {
                    instance_id: self.id(alias),
                },
                key,
            )
            .expect("read the fact")
            .is_some()
    }

    /// Every record after `after`, in order.
    fn events_after(&self, after: u64) -> Vec<PersistedEvent> {
        self.host
            .query_persisted_events_for_test(EventQuery::default())
            .expect("ledger events")
            .into_iter()
            .filter(|event| event.sequence() > after)
            .collect()
    }

    /// The ledger's last sequence.
    fn head(&self) -> u64 {
        self.events_after(0)
            .last()
            .map_or(0, PersistedEvent::sequence)
    }

    /// A request on `connection` processed as a client's.
    fn send(&self, operation: RuntimeOperation, connection: u64) -> RuntimeReceipt {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        self.host
            .process_request_for_test(
                &runtime_request(&ids, operation),
                ConnectionId::new(connection).expect("connection"),
            )
            .expect("the request's receipt")
    }

    fn close(self) {
        self.host.close().expect("close the device group host");
    }
}

fn policy_connection() -> ConnectionId {
    ConnectionId::new(POLICY_CONNECTION).expect("the policy connection")
}

/// `error` is `application_switch_failed` with `alias`, `step`, `application_id` and the step's
/// own code as `failure_code`, which is the code of its cause.
fn assert_switch_failed(
    error: &RuntimeHostError,
    alias: &str,
    step: &str,
    failure_code: &str,
    application_id: &str,
) {
    assert_eq!(
        error.code(),
        HostCode::ApplicationSwitchFailed.as_str(),
        "{error:?}"
    );
    let outcome = error.outcome();
    assert_eq!(outcome.value("alias"), Some(&json!(alias)), "{outcome:?}");
    assert_eq!(
        outcome.value("application_switch_step"),
        Some(&json!(step)),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("failure_code"),
        Some(&json!(failure_code)),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("application_id"),
        Some(&json!(application_id)),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.codes().get(1).map(|code| code.as_str()),
        Some(failure_code),
        "the step's failure is the cause: {outcome:?}"
    );
}

/// The records of `instance` among `events`.
fn records_of(events: &[PersistedEvent], instance: InstanceId) -> Vec<&PersistedEvent> {
    events
        .iter()
        .filter(|event| event.links().instance_id() == Some(&instance))
        .collect()
}

// ---- The tests.

/// F1, F4: B's run holds the device's key and A's application is stopped under it. The fake
/// device saw the stop of A's package and no command for B's; the stop's records
/// (`command.received`, `command.validated`, `scheduler.admitted`, `application.intent`,
/// `application.completed`) are A's, under A's links with the key's lease, and share one
/// causation id; nothing is recorded under B. A's `task.page` and `application.foreground` are
/// gone, B's are untouched, and B's key still holds the device.
#[test]
fn mapp2a_a_stop_acts_on_the_other_members_package_under_the_holders_key_and_forgets_its_page() {
    let switching = Switching::start();
    let context = switching.grant_b();
    let token = context.lease_token().clone();
    for alias in [A, B] {
        switching.record_fact(alias, TASK_PAGE, "neutral/home");
        switching.record_fact(alias, APPLICATION_FOREGROUND_FACT_KEY, "neutral.member.a");
    }
    let mark = switching.head();

    let stopped = switching
        .host
        .stop_member_application_for_test(&token, policy_connection(), A)
        .expect("the stop's records")
        .expect("A's application is stopped");
    assert_eq!(stopped, A);
    assert_eq!(switching.actions(A), [ApplicationLifecycleAction::Stop]);
    assert!(
        switching.actions(B).is_empty(),
        "no command for B's package"
    );

    let events = switching.events_after(mark);
    let a = switching.id(A);
    let stop_records = records_of(&events, a)
        .into_iter()
        .filter(|event| {
            matches!(
                event.event_type(),
                EventType::CommandReceived
                    | EventType::CommandValidated
                    | EventType::SchedulerAdmitted
                    | EventType::ApplicationIntent
                    | EventType::ApplicationCompleted
            )
        })
        .collect::<Vec<_>>();
    let kinds = stop_records
        .iter()
        .map(|event| event.event_type())
        .collect::<Vec<_>>();
    for kind in [
        EventType::CommandReceived,
        EventType::CommandValidated,
        EventType::SchedulerAdmitted,
        EventType::ApplicationIntent,
        EventType::ApplicationCompleted,
    ] {
        assert!(kinds.contains(&kind), "{kind:?} under A: {kinds:?}");
    }
    let causations = stop_records
        .iter()
        .map(|event| event.links().causation_id().copied())
        .collect::<BTreeSet<_>>();
    assert_eq!(causations.len(), 1, "one causation id: {causations:?}");
    assert!(
        causations.iter().all(Option::is_some),
        "the causation is set"
    );
    assert!(
        stop_records
            .iter()
            .filter(|event| event.event_type() != EventType::CommandReceived
                && event.event_type() != EventType::CommandValidated)
            .all(|event| event.links().lease_id() == Some(&token.lease_id())),
        "the stop runs on B's key"
    );
    assert!(
        records_of(&events, switching.id(B)).is_empty(),
        "nothing is recorded under B"
    );

    assert!(!switching.has_fact(A, TASK_PAGE));
    assert!(!switching.has_fact(A, APPLICATION_FOREGROUND_FACT_KEY));
    assert!(switching.has_fact(B, TASK_PAGE));
    assert!(switching.has_fact(B, APPLICATION_FOREGROUND_FACT_KEY));
    assert_eq!(
        switching.host.instance_claims_for_test(B).expect("claims"),
        (Some(ClaimKind::PolicyDispatch), Vec::new()),
        "B's run keeps the device's key"
    );
    assert!(
        switching
            .host
            .key_remaining_ms_for_test(&token, policy_connection())
            .expect("the key")
            .is_some(),
        "B's token still validates"
    );
    assert!(switching.host.fatal_error().expect("health").is_none());
    switching.close();
}

/// F4 with a failed close: B's run holds the device's key, A's application is force-stopped
/// under it, and the close of the session the stop used then fails (`backend_operation_failed`,
/// injected after the stop, as a paired session's close can fail; the fake device keeps nothing
/// open there). The stop fails the step `stop` with the close's code and A's package, and A's
/// `task.page` and `application.foreground` are gone all the same; B's are untouched and B's
/// token still validates.
#[test]
fn mapp2a_a_stop_whose_session_close_fails_still_forgets_the_stopped_members_page() {
    let switching = Switching::start();
    let context = switching.grant_b();
    let token = context.lease_token().clone();
    for alias in [A, B] {
        switching.record_fact(alias, TASK_PAGE, "neutral/home");
        switching.record_fact(alias, APPLICATION_FOREGROUND_FACT_KEY, APP_A);
    }
    switching
        .host
        .fail_next_member_close_for_test()
        .expect("the next member close fails");

    let failed = switching
        .host
        .stop_member_application_for_test(&token, policy_connection(), A)
        .expect("no Runtime failure")
        .expect_err("the failed close fails the stop");
    assert_switch_failed(
        &failed,
        B,
        "stop",
        RuntimeErrorCode::BackendOperationFailed.as_str(),
        APP_A,
    );
    assert_eq!(
        switching.actions(A),
        [ApplicationLifecycleAction::Stop],
        "the force-stop reached A"
    );
    assert!(!switching.has_fact(A, TASK_PAGE), "A's page is forgotten");
    assert!(!switching.has_fact(A, APPLICATION_FOREGROUND_FACT_KEY));
    assert!(switching.has_fact(B, TASK_PAGE));
    assert!(switching.has_fact(B, APPLICATION_FOREGROUND_FACT_KEY));
    assert!(
        switching
            .host
            .key_remaining_ms_for_test(&token, policy_connection())
            .expect("the key")
            .is_some(),
        "B's token still validates"
    );
    assert!(switching.host.fatal_error().expect("health").is_none());
    switching.close();
}

/// F1: a key of another device (C's, alone on its device) and a member outside the holder's
/// group (C, under B's key) are refused before any device step: `application_switch_failed` at
/// step `stop` caused by `instance_mismatch`, no command reaches a device and nothing is
/// recorded under the member.
#[test]
fn mapp2a_a_key_of_another_device_and_a_member_outside_the_group_are_refused_before_any_device_step()
 {
    let switching = Switching::start();
    let context = switching.grant_b();
    let b_token = context.lease_token().clone();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let granted = switching.send(
        RuntimeOperation::acquire_lease(C, ids.mint_holder_id().expect("holder")),
        71,
    );
    let Some(RuntimeResult::LeaseGranted { token: c_token }) = granted.result() else {
        panic!("C's lease: {granted:?}");
    };
    let c_token = c_token.clone();
    let mark = switching.head();

    let refused = switching
        .host
        .stop_member_application_for_test(&c_token, ConnectionId::new(71).expect("connection"), A)
        .expect("no Runtime failure")
        .expect_err("a key of another device is refused");
    assert_switch_failed(
        &refused,
        C,
        "stop",
        RuntimeErrorCode::InstanceMismatch.as_str(),
        APP_A,
    );

    let refused = switching
        .host
        .stop_member_application_for_test(&b_token, policy_connection(), C)
        .expect("no Runtime failure")
        .expect_err("a member outside the group is refused");
    assert_switch_failed(
        &refused,
        B,
        "stop",
        RuntimeErrorCode::InstanceMismatch.as_str(),
        APP_C,
    );

    for alias in [A, B, C, D] {
        assert!(
            switching.actions(alias).is_empty(),
            "no device step: {alias}"
        );
    }
    let events = switching.events_after(mark);
    for alias in [A, C] {
        assert!(
            records_of(&events, switching.id(alias)).is_empty(),
            "nothing recorded under {alias}: {events:#?}"
        );
    }
    let released = switching.send(RuntimeOperation::ReleaseLease { token: c_token }, 71);
    assert_eq!(released.state(), RuntimeReceiptState::Completed);
    switching.close();
}

/// F2 and the time-to-live ruling: B's startup package runs on the key B's run holds and the key
/// goes back to B's run, its token unchanged and valid, on success (the package's restart
/// reached B's device, its home was reached) and on failure (no home recognized); in both its
/// remaining time is at least what B's run asked to keep (`AFTER_MS`). A's queued claim stays
/// queued throughout: it is never granted or transferred the key, and no lease of B's is
/// released. A key whose remaining time does not cover the startup run and `AFTER_MS` is refused
/// before anything is recorded (`contained_task_deadline_unavailable`).
#[test]
fn mapp2a_a_startup_run_on_a_borrowed_key_hands_the_same_key_back_and_the_queued_claim_waits() {
    let switching = Switching::start();
    let context = switching.grant_b();
    let token = context.lease_token().clone();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let queued = switching.send(
        RuntimeOperation::queue_lease(
            A,
            ids.mint_holder_id().expect("holder"),
            LeaseQueuePolicy::new(LeasePriority::Normal, 3_600_000).expect("queue policy"),
        ),
        72,
    );
    let Some(RuntimeResult::LeaseQueued { status }) = queued.result() else {
        panic!("A's claim waits queued: {queued:?}");
    };
    let queued_request = status.request_id();
    let claims = (
        Some(ClaimKind::PolicyDispatch),
        vec![ClaimKind::ClientLease],
    );
    assert_eq!(
        switching.host.instance_claims_for_test(A).expect("claims"),
        claims
    );
    let mark = switching.head();

    switching
        .host
        .run_borrowed_startup_for_test(context.request(), &token, policy_connection(), AFTER_MS)
        .expect("no Runtime failure")
        .expect("the startup run reaches its home");
    assert_eq!(switching.actions(B), [ApplicationLifecycleAction::Restart]);
    assert!(switching.actions(A).is_empty());
    let remaining = switching
        .host
        .key_remaining_ms_for_test(&token, policy_connection())
        .expect("the key")
        .expect("B's token still validates");
    assert!(remaining >= AFTER_MS, "{remaining} < {AFTER_MS}");
    assert_eq!(
        switching.host.instance_claims_for_test(A).expect("claims"),
        claims
    );

    switching
        .state(B)
        .unknown_capture
        .store(true, Ordering::Release);
    let failed = switching
        .host
        .run_borrowed_startup_for_test(context.request(), &token, policy_connection(), AFTER_MS)
        .expect("no Runtime failure");
    assert!(failed.is_err(), "no home is recognized: {failed:?}");
    let remaining = switching
        .host
        .key_remaining_ms_for_test(&token, policy_connection())
        .expect("the key")
        .expect("B's token still validates after a failed startup run");
    assert!(remaining >= AFTER_MS, "{remaining} < {AFTER_MS}");
    assert_eq!(
        switching.host.instance_claims_for_test(A).expect("claims"),
        claims
    );

    let events = switching.events_after(mark);
    assert!(
        !events.iter().any(|event| {
            event.links().request_id() == Some(&queued_request)
                && matches!(
                    event.event_type(),
                    EventType::LeaseGranted | EventType::LeaseTransferred
                )
        }),
        "A's queued claim never gets the key"
    );
    assert!(
        !events.iter().any(|event| {
            event.event_type() == EventType::LeaseReleased
                && event.links().lease_id() == Some(&token.lease_id())
        }),
        "B's key is never released"
    );
    assert_eq!(
        records_of(&events, switching.id(B))
            .into_iter()
            .filter(|event| event.event_type() == EventType::TaskRequested)
            .count(),
        2,
        "two startup runs"
    );

    let mark = switching.head();
    let restarts = switching.actions(B).len();
    let refused = switching
        .host
        .run_borrowed_startup_for_test(context.request(), &token, policy_connection(), remaining)
        .expect("no Runtime failure")
        .expect_err("the key cannot cover the startup run and the run after it");
    assert_eq!(
        refused.code(),
        HostCode::ContainedTaskDeadlineUnavailable.as_str()
    );
    assert!(
        records_of(&switching.events_after(mark), switching.id(B)).is_empty(),
        "nothing is recorded"
    );
    assert_eq!(switching.actions(B).len(), restarts, "no other restart");
    assert!(switching.host.fatal_error().expect("health").is_none());
    switching.close();
}

/// The entry with B's package in front: nothing more is done and nothing is recorded (the
/// contract's step 1: no stop and no startup run). With A's package in front only A is stopped;
/// with no resumed activity, and with a package no member is assigned, every other member (A and
/// D) is stopped. After a stop B's startup package runs on the key: its restart reaches B's
/// device, its scheduling record is written under B's run's request links with the causation id
/// the stops carry, and its run runs under it.
#[test]
fn mapp2a_the_entry_stops_the_member_in_front_or_every_other_member_and_then_runs_the_startup_package()
 {
    let switching = Switching::start();
    let context = switching.grant_b();
    let token = context.lease_token().clone();
    let mark = switching.head();
    let switched = switching
        .host
        .switch_application(context.request(), &token, policy_connection(), AFTER_MS)
        .expect("no Runtime failure")
        .expect("B in front");
    assert_eq!(
        switched,
        ApplicationSwitch {
            front: Some(APP_B.to_owned()),
            stopped: Vec::new(),
            started: false,
        }
    );
    let events = switching.events_after(mark);
    for alias in [A, B, D] {
        assert!(switching.actions(alias).is_empty(), "{alias}");
        assert!(
            records_of(&events, switching.id(alias)).is_empty(),
            "no record under {alias}"
        );
    }
    switching.close();

    let switching = Switching::start();
    let context = switching.grant_b();
    let token = context.lease_token().clone();
    switching.front(B, Some(APP_A));
    let mark = switching.head();
    let switched = switching
        .host
        .switch_application(context.request(), &token, policy_connection(), AFTER_MS)
        .expect("no Runtime failure")
        .expect("A in front");
    assert_eq!(
        switched,
        ApplicationSwitch {
            front: Some(APP_A.to_owned()),
            stopped: vec![A.to_owned()],
            started: true,
        }
    );
    assert_eq!(switching.actions(A), [ApplicationLifecycleAction::Stop]);
    assert!(switching.actions(D).is_empty(), "D is not in front");
    assert_eq!(switching.actions(B), [ApplicationLifecycleAction::Restart]);
    let events = switching.events_after(mark);
    let stop_causation = records_of(&events, switching.id(A))
        .into_iter()
        .find(|event| event.event_type() == EventType::CommandReceived)
        .and_then(|event| event.links().causation_id().copied())
        .expect("the stop's causation");
    let scheduled = phases(&events)
        .into_iter()
        .find(|(_, phase)| matches!(phase, RuntimeLifecyclePhase::StartupPackageScheduled { .. }))
        .map(|(event, _)| event.clone())
        .expect("the startup package is scheduled");
    assert_eq!(scheduled.links().causation_id(), Some(&stop_causation));
    assert_eq!(
        scheduled.links().request_id(),
        Some(&context.request().request_id()),
        "under B's run's request links"
    );
    assert_eq!(scheduled.links().instance_id(), Some(&switching.id(B)));
    let requested = records_of(&events, switching.id(B))
        .into_iter()
        .find(|event| event.event_type() == EventType::TaskRequested)
        .expect("B's startup run");
    assert_eq!(requested.links().causation_id(), Some(&stop_causation));
    assert!(
        switching
            .host
            .key_remaining_ms_for_test(&token, policy_connection())
            .expect("the key")
            .is_some(),
        "the key goes back to B's run"
    );
    switching.close();

    for front in [None, Some(APP_OTHER)] {
        let switching = Switching::start();
        let context = switching.grant_b();
        let token = context.lease_token().clone();
        switching.front(B, front);
        let switched = switching
            .host
            .switch_application(context.request(), &token, policy_connection(), AFTER_MS)
            .expect("no Runtime failure")
            .expect("no member in front");
        assert_eq!(switched.front.as_deref(), front);
        assert_eq!(
            switched.stopped.iter().cloned().collect::<BTreeSet<_>>(),
            BTreeSet::from([A.to_owned(), D.to_owned()]),
            "{front:?}"
        );
        assert!(switched.started);
        assert_eq!(switching.actions(A), [ApplicationLifecycleAction::Stop]);
        assert_eq!(switching.actions(D), [ApplicationLifecycleAction::Stop]);
        assert_eq!(switching.actions(B), [ApplicationLifecycleAction::Restart]);
        assert!(switching.actions(C).is_empty(), "C is not in the group");
        switching.close();
    }
}

/// A failure at each step is `application_switch_failed` with its step, the step's own code and
/// the package it concerned. `foreground_read`: the read's ADB failure drops B's `task.page`, as
/// the foreground gate's does, and nothing is stopped. `stop`: A's stop fails at its device
/// (`application_backend_operation_failed`), A's page stays and no startup run follows.
/// `startup_package`: a key whose time does not cover the startup run and the run after it is
/// refused before any stop (`contained_task_deadline_unavailable`); a startup run that reaches
/// no home fails the step with its own code after A's stop, and the key goes back to B's run.
#[test]
fn mapp2a_a_failure_at_each_step_names_its_step_and_its_cause() {
    let switching = Switching::start();
    let context = switching.grant_b();
    let token = context.lease_token().clone();
    switching.record_fact(B, TASK_PAGE, "neutral/home");
    *switching
        .state(B)
        .foreground_error_once
        .lock()
        .expect("one-shot foreground error") =
        Some(DeviceError::transient("fixture foreground query failed"));
    let failed = switching
        .host
        .switch_application(context.request(), &token, policy_connection(), AFTER_MS)
        .expect("no Runtime failure")
        .expect_err("the read fails");
    assert_switch_failed(
        &failed,
        B,
        "foreground_read",
        HostCode::ApplicationForegroundUnknown.as_str(),
        APP_B,
    );
    assert!(!switching.has_fact(B, TASK_PAGE), "dropped as by the gate");
    for alias in [A, B, D] {
        assert!(switching.actions(alias).is_empty(), "{alias}");
    }
    switching.close();

    let switching = Switching::start();
    let context = switching.grant_b();
    let token = context.lease_token().clone();
    switching.front(B, Some(APP_A));
    switching.record_fact(A, TASK_PAGE, "neutral/home");
    *switching
        .state(A)
        .application_error
        .lock()
        .expect("application error") = Some(DeviceError::transient("fixture stop refused"));
    let failed = switching
        .host
        .switch_application(context.request(), &token, policy_connection(), AFTER_MS)
        .expect("no Runtime failure")
        .expect_err("the stop fails");
    assert_switch_failed(
        &failed,
        B,
        "stop",
        SharedCode::ApplicationBackendOperationFailed.as_str(),
        APP_A,
    );
    assert!(
        switching.has_fact(A, TASK_PAGE),
        "a failed stop forgets no page"
    );
    assert!(switching.actions(B).is_empty(), "no startup run");
    switching.close();

    let switching = Switching::start();
    let context = switching.grant_b();
    let token = context.lease_token().clone();
    switching.front(B, Some(APP_A));
    let remaining = switching
        .host
        .key_remaining_ms_for_test(&token, policy_connection())
        .expect("the key")
        .expect("B's token validates");
    let failed = switching
        .host
        .switch_application(context.request(), &token, policy_connection(), remaining)
        .expect("no Runtime failure")
        .expect_err("the key's time is short");
    assert_switch_failed(
        &failed,
        B,
        "startup_package",
        HostCode::ContainedTaskDeadlineUnavailable.as_str(),
        APP_B,
    );
    assert!(switching.actions(A).is_empty(), "refused before any stop");
    assert!(switching.actions(B).is_empty());
    switching.close();

    let switching = Switching::start();
    let context = switching.grant_b();
    let token = context.lease_token().clone();
    switching.front(B, Some(APP_A));
    switching
        .state(B)
        .unknown_capture
        .store(true, Ordering::Release);
    let failed = switching
        .host
        .switch_application(context.request(), &token, policy_connection(), AFTER_MS)
        .expect("no Runtime failure")
        .expect_err("no home is recognized");
    let outcome = failed.outcome();
    assert_eq!(failed.code(), HostCode::ApplicationSwitchFailed.as_str());
    assert_eq!(
        outcome.value("application_switch_step"),
        Some(&json!("startup_package"))
    );
    assert_eq!(outcome.value("application_id"), Some(&json!(APP_B)));
    let cause = outcome.codes().get(1).map(|code| code.as_str().to_owned());
    assert_eq!(
        outcome
            .value("failure_code")
            .and_then(Value::as_str)
            .map(str::to_owned),
        cause,
        "{outcome:?}"
    );
    assert_ne!(
        cause.as_deref(),
        Some(HostCode::ApplicationSwitchFailed.as_str())
    );
    assert_eq!(switching.actions(A), [ApplicationLifecycleAction::Stop]);
    assert!(
        switching
            .host
            .key_remaining_ms_for_test(&token, policy_connection())
            .expect("the key")
            .is_some(),
        "the key goes back to B's run"
    );
    assert!(switching.host.fatal_error().expect("health").is_none());
    switching.close();
}

/// A lone instance never reaches the new code: C's application launch and C's direct run (the
/// request paths the switch will serve from MAPP-2b on for a member) never reach the switch's
/// entry; the entry called with C's own key leaves it alone (nothing read, stopped, run or
/// recorded), and the stop and the borrowed startup run refuse C's key (`instance_mismatch`)
/// before anything. (The assembly refuses every group of several: actingd's MAPP-1 gate.)
#[test]
fn mapp2a_a_lone_instances_request_path_never_reaches_the_switch() {
    let switching = Switching::start();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let launched = switching.send(
        RuntimeOperation::ApplicationLifecycle {
            instance_alias: C.to_owned(),
            holder_id: *ids.mint_holder_id().expect("holder").transport(),
            action: ApplicationLifecycleAction::Launch,
        },
        81,
    );
    assert_eq!(
        launched.state(),
        RuntimeReceiptState::Completed,
        "{launched:?}"
    );
    let _ran = switching.send(
        RuntimeOperation::run_contained_task(
            C,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(switching.package.to_string_lossy().into_owned())
                .expect("the direct run's request"),
        ),
        82,
    );
    assert_eq!(
        switching.host.switch_entries_for_test().expect("count"),
        0,
        "C's requests never reach the switch"
    );

    let acquire = runtime_request(
        &ids,
        RuntimeOperation::acquire_lease(C, ids.mint_holder_id().expect("holder")),
    );
    let granted = switching
        .host
        .process_request_for_test(&acquire, ConnectionId::new(83).expect("connection"))
        .expect("C's lease");
    let Some(RuntimeResult::LeaseGranted { token }) = granted.result() else {
        panic!("C's lease: {granted:?}");
    };
    let token = token.clone();
    let connection = ConnectionId::new(83).expect("connection");
    let actions = switching.actions(C);
    let mark = switching.head();
    let switched = switching
        .host
        .switch_application(&acquire, &token, connection, AFTER_MS)
        .expect("no Runtime failure")
        .expect("a lone instance is left alone");
    assert_eq!(switched, ApplicationSwitch::default());
    assert_eq!(switching.host.switch_entries_for_test().expect("count"), 1);
    let refused = switching
        .host
        .run_borrowed_startup_for_test(&acquire, &token, connection, AFTER_MS)
        .expect("no Runtime failure")
        .expect_err("a lone instance borrows no key");
    assert_eq!(refused.code(), RuntimeErrorCode::InstanceMismatch.as_str());
    assert_eq!(switching.actions(C), actions, "no device step");
    assert!(
        records_of(&switching.events_after(mark), switching.id(C)).is_empty(),
        "nothing recorded"
    );
    let released = switching.send(RuntimeOperation::ReleaseLease { token }, 83);
    assert_eq!(released.state(), RuntimeReceiptState::Completed);
    switching.close();
}
