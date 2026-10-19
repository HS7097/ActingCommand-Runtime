// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B15, slice R4 (host gate key 113; register §1.114; contracts/remedies.md sections
// 5 and 7; model-b15 §7, §9, §10, §12, §14): the host runs the remedies packages declare. Its
// remedy table is read from the bound packages; the journal of each pair is its policy runs
// since its latest root blocked run, live and rebuilt at start; the evaluation applies the
// plan, its deferrals at position 8a of the eligibility callback; an admission refuses a request
// or retry whose round closed; the plan's stops, lifts and expiries are recorded once each.
//
// A neutral fixture: one instance (the fixture alias); its frames are 2x1 and their first pixel
// is the screen: red `home`, blue `page_b`, white `result`. Each run starts on `home` and its one
// tap shows the page the test chose for that run. Package `main` (page graph) goes from `home` to
// `page_b` or `result` and maps `page_b` to the key `blocked_k`, whose remedy serves the need
// `need_a` with the capability `cap_x` (its package default per test), and `result` to `done`.
// Package `sink` goes from `home` to `result` and provides `cap_x`. The catalog's task `t_main`
// is due at every evaluation; `t_sink`'s trigger is never true by itself. No catalog predicate
// references an outcome key. Every expected value is the brief's requirement or model-b15's.

use super::*;
use actingcommand_contract::outcome::Outcome;
use actingcommand_policy::{PolicyEvaluation, RemedyTableEdge};
use serde_json::{Value, json};

const MAIN_TASK: &str = "t_main";
const SINK_TASK: &str = "t_sink";
const MAIN_PROCEDURE: &str = "procedure.main";
const SINK_PROCEDURE: &str = "procedure.sink";
const MAIN_PACKAGE: &str = "neutral.test.main";
const SINK_PACKAGE: &str = "neutral.test.sink";
const BLOCKED_KEY: &str = "blocked_k";
const DONE_KEY: &str = "done";
const HOME: [u8; 3] = [255, 0, 0];
const PAGE_B: [u8; 3] = [0, 0, 255];
const RESULT: [u8; 3] = [255, 255, 255];
// The fixture catalog's activity window is 08:00 to 22:00 UTC; `POLICY_NOW_UNIX_MS` is 12:00 UTC.
const WINDOW_END_UNIX_MS: u64 = POLICY_NOW_UNIX_MS + 36_000_000;

const REQUESTED: &str = "policy_remedy_requested";
const RETRY: &str = "policy_remedy_retry";
const ALREADY_SELECTED: &str = "instance_already_selected";
const UNCHOSEN: &str = "policy_remedy_unchosen";
const UNAVAILABLE: &str = "policy_remedy_unavailable";
const LIFTED: &str = "policy_remedy_lifted";
const EXPIRED: &str = "policy_remedy_expired";
const ROUND_CLOSED: &str = "policy_remedy_round_closed";

// ---- Packages.

// One package: from `home` to `page_b` or `result` when it declares the remedy `remedy` on the
// mapping of `page_b`, otherwise from `home` to `result` with no outcome declaration; with
// `provides` when one is given.
struct Pack {
    package_id: &'static str,
    remedy: Option<Value>,
    provides: Option<Value>,
}

impl Pack {
    fn write(&self, directory: &Path) {
        let blocking = self.remedy.is_some();
        let pages: &[(&str, [u8; 3])] = if blocking {
            &[("home", HOME), ("page_b", PAGE_B), ("result", RESULT)]
        } else {
            &[("home", HOME), ("result", RESULT)]
        };
        let mut control = json!({
            "schema_version": "Lab-1y.control.v2",
            "package_id": self.package_id,
            "execution_mode": "navigable_route",
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 2, "height": 1},
            "entry_task_id": "task",
            "capture_interval_ms": 10,
            "step_timeout_ms": 600,
            "timeout_ms": 8000,
            "max_steps": 2
        });
        if let Some(provides) = &self.provides {
            control["provides"] = provides.clone();
        }
        let ends = if blocking {
            json!(["page_b", "result"])
        } else {
            json!("result")
        };
        let mut task = json!({
            "schema_version": "0.9",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "locale": "en-US",
            "goal": "B15 R4 fixture",
            "coordinate_space": {"width": 2, "height": 1},
            "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
            "color_probes": pages
                .iter()
                .map(|(page, colour)| json!({
                    "id": format!("{page}_colour"),
                    "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
                    "expected": colour
                }))
                .collect::<Vec<_>>(),
            "page_rules": pages
                .iter()
                .map(|(page, _)| ((*page).to_owned(), json!({"required": [format!("{page}_colour")]})))
                .collect::<serde_json::Map<_, _>>(),
            "entry_page": "home",
            "target_page": ends.clone(),
            "operations": [{
                "id": "go",
                "from": "home",
                "to": ends,
                "click": {"kind": "point", "x": 1, "y": 0},
                "unguarded_trusted_coordinate": true,
                "retryable": false
            }]
        });
        if let Some(remedy) = &self.remedy {
            task["scheduling_outcome"] = json!({"mappings": [
                {"outcome_key": BLOCKED_KEY, "effect": "no_designated_effect",
                 "terminal_pages": ["page_b"], "remedy": remedy},
                {"outcome_key": DONE_KEY, "effect": "no_designated_effect",
                 "terminal_pages": ["result"]}
            ]});
        }
        for (file, value) in [
            ("control.json", control),
            (
                "resources/operations/resources.json",
                json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
            ),
            ("resources/operations/task/task.json", task),
        ] {
            let path = directory.join(file);
            fs::create_dir_all(path.parent().expect("a package file's directory"))
                .expect("package directory");
            fs::write(
                path,
                serde_json::to_vec_pretty(&value).expect("package JSON"),
            )
            .expect("package file");
        }
    }
}

// `main` with the remedy of `need_a` over `capabilities`, its package default `default`.
fn main_pack(capabilities: &[&str], default: Option<&[&str]>) -> Pack {
    let mut remedy = json!({"need": "need_a", "capabilities": capabilities});
    if let Some(default) = default {
        remedy["default"] = json!(default);
    }
    Pack {
        package_id: MAIN_PACKAGE,
        remedy: Some(remedy),
        provides: None,
    }
}

// `sink`, providing `cap_x`.
fn sink_pack() -> Pack {
    Pack {
        package_id: SINK_PACKAGE,
        remedy: None,
        provides: Some(json!([{"capability": "cap_x"}])),
    }
}

// ---- The catalog, the manifest and the screen.

// The fixture catalog with `t_main` (due at every evaluation) and `t_sink` (never due by itself),
// and room in the activity window for every run of a test.
fn catalog() -> CatalogSources {
    let mut sources = policy_sources(1);
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("fixture tasks");
    let mut main = tasks["tasks"][0].clone();
    main["id"] = json!(MAIN_TASK);
    main["procedure_ref"] = json!(MAIN_PROCEDURE);
    main["trigger"] = json!({
        "kind": "clock",
        "schedule": {"kind": "interval", "clock_source": {"kind": "local"}, "every_ms": 1,
                     "anchor_ms": 0}
    });
    main["loop_budget"]["window_iteration_limit"] = json!(8);
    let mut sink = main.clone();
    sink["id"] = json!(SINK_TASK);
    sink["procedure_ref"] = json!(SINK_PROCEDURE);
    sink["priority"] = json!(50);
    sink["trigger"] = main["feedback_stop"].clone();
    tasks["tasks"] = json!([main, sink]);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("fixture task bytes");
    let mut activity: Value =
        serde_json::from_slice(&sources.activity.bytes).expect("fixture activity");
    activity["profiles"][0]["max_window_iterations"] = json!(8);
    sources.activity.bytes = serde_json::to_vec_pretty(&activity).expect("fixture activity bytes");
    sources
}

// Each task's procedure bound at its package's location, as actingd binds a scheduled procedure.
fn manifest(main: &Path, sink: &Path) -> ProcedureManifest {
    ProcedureManifest::new([(MAIN_PROCEDURE, main), (SINK_PROCEDURE, sink)].map(
        |(procedure_ref, path)| {
            ProcedureBinding::new(
                procedure_ref,
                "operation.observe",
                vec!["after_observation".to_owned()],
            )
            .expect("procedure binding")
            .with_package_path(path.to_string_lossy())
        },
    ))
    .expect("procedure manifest")
}

// The fixture's facts without a scheduling outcome (the catalog references none).
fn facts() -> EvaluationFacts {
    let mut facts = policy_facts();
    facts.outcomes.clear();
    facts
}

// The device: `home` until the run's tap, then the page the test chose for the run.
struct Screen {
    taps_before: Arc<AtomicUsize>,
    after_tap: Arc<std::sync::Mutex<[u8; 3]>>,
}

impl Screen {
    fn install(state: &FakeState) -> Self {
        let taps_before = Arc::new(AtomicUsize::new(0));
        let after_tap = Arc::new(std::sync::Mutex::new(RESULT));
        let (before, after) = (Arc::clone(&taps_before), Arc::clone(&after_tap));
        *state.pixel_script.lock().expect("pixel script") =
            Some(Arc::new(move |_: usize, inputs: usize| {
                let page = if inputs <= before.load(Ordering::Acquire) {
                    HOME
                } else {
                    *after.lock().expect("the page after the tap")
                };
                vec![(0, page)]
            }));
        Self {
            taps_before,
            after_tap,
        }
    }

    // The next run starts on `home` and its tap shows `page`.
    fn next(&self, state: &FakeState, page: [u8; 3]) {
        self.taps_before
            .store(state.input_count.load(Ordering::Acquire), Ordering::Release);
        *self.after_tap.lock().expect("the page after the tap") = page;
    }
}

// ---- The fixture.

struct Fixture {
    root: TempDir,
    main: PathBuf,
    sink: PathBuf,
    instance: InstanceId,
    state: Arc<FakeState>,
    screen: Screen,
    clock: Arc<ManualRuntimeClock>,
    host: RuntimeHost,
}

fn start_host(
    root: &TempDir,
    main: &Path,
    sink: &Path,
    clock: &Arc<ManualRuntimeClock>,
    state: &Arc<FakeState>,
    instance: InstanceId,
) -> RuntimeHost {
    RuntimeHost::start(
        config(root)
            .with_runtime_clock(clock.clone())
            .with_procedure_manifest(manifest(main, sink)),
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, instance, Arc::clone(state))
                .fixture_simulation(),
        ),
    )
    .expect("runtime host")
}

impl Fixture {
    // A host with `main` and `sink` written and bound and the catalog active, its clock at the
    // fixture's time.
    fn new(main: &Pack, sink: &Pack) -> Self {
        let root = TempDir::new().expect("tempdir");
        let (main_path, sink_path) = (root.path().join("main"), root.path().join("sink"));
        main.write(&main_path);
        sink.write(&sink_path);
        let state = Arc::new(FakeState::default());
        let screen = Screen::install(&state);
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let instance = instance_id();
        let host = start_host(&root, &main_path, &sink_path, &clock, &state, instance);
        host.activate_policy_catalog(&catalog())
            .expect("activate the catalog");
        Self {
            root,
            main: main_path,
            sink: sink_path,
            instance,
            state,
            screen,
            clock,
            host,
        }
    }

    // The host closed and started again on the same state root and instance, its clock at `at`.
    fn restart(self, at: u64) -> Self {
        let Self {
            root,
            main,
            sink,
            instance,
            state,
            screen,
            host,
            ..
        } = self;
        host.close().expect("close the runtime host");
        let clock = Arc::new(ManualRuntimeClock::new(at, 0));
        let host = start_host(&root, &main, &sink, &clock, &state, instance);
        Self {
            root,
            main,
            sink,
            instance,
            state,
            screen,
            clock,
            host,
        }
    }

    // One full evaluation at `at`, the Runtime clock moved there first.
    fn evaluate(&self, at: u64, seed: u64) -> PolicyEvaluation {
        advance_manual_clock_to(&self.clock, at);
        self.host
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
            .expect("evaluate the policy")
            .evaluation
            .expect("a full evaluation")
    }

    // The request of `task_id`'s package, at its bound location.
    fn request(&self, task_id: &str) -> ContainedTaskRequest {
        let path = if task_id == MAIN_TASK {
            &self.main
        } else {
            &self.sink
        };
        ContainedTaskRequest::new(path.to_string_lossy().into_owned())
            .expect("contained task request")
    }

    // Admits `intent` as the scheduler does (capacity sampled at the moved clock, its approval
    // recorded).
    fn admit(
        &self,
        intent: &DispatchIntent,
        chain: &DecisionReasonChain,
    ) -> RuntimeHostResult<PolicyDispatchAdmission> {
        self.host
            .capacity_sampler_for_test()
            .expect("capacity owner")()
        .expect("capacity sample at the admission time");
        record_policy_approval(&self.host, intent);
        self.host.admit_scheduled_policy_dispatch(
            intent,
            chain,
            &policy_context(&self.host, intent),
            &self.request(&intent.task_id),
        )
    }

    // Admits `intent`, runs its package with the device showing `page` after the tap, and settles
    // the successful run.
    fn run(
        &self,
        intent: &DispatchIntent,
        chain: &DecisionReasonChain,
        page: [u8; 3],
    ) -> Box<PolicyRunContext> {
        let PolicyDispatchAdmission::Granted { context } =
            self.admit(intent, chain).expect("policy admission")
        else {
            panic!("expected one policy run context")
        };
        self.screen.next(&self.state, page);
        let receipt = self
            .host
            .run_scheduled_contained_task(&context, &self.request(&intent.task_id))
            .expect("the scheduled run");
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
        self.host
            .complete_scheduled_policy_run(&context, &receipt)
            .expect("the run settles");
        context
    }

    // Evaluates at `at`, then runs the dispatch of `task_id` it must select, the device showing
    // `page` after the tap.
    fn evaluate_and_run(
        &self,
        at: u64,
        seed: u64,
        task_id: &str,
        page: [u8; 3],
    ) -> (PolicyEvaluation, DecisionReasonChain, Box<PolicyRunContext>) {
        let evaluation = self.evaluate(at, seed);
        let (intent, chain) = dispatched(&evaluation, task_id)
            .unwrap_or_else(|| panic!("a dispatch of {task_id}: {evaluation:#?}"));
        let context = self.run(&intent, &chain, page);
        (evaluation, chain, context)
    }

    // The persisted records of `event_type`, in ledger order.
    fn records(&self, event_type: EventType) -> Vec<PersistedEvent> {
        self.host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(event_type),
                ..EventQuery::default()
            })
            .expect("ledger records")
    }

    // The `signal:remedy:<kind>:` planning signals recorded, in ledger order.
    fn remedy_signals(&self, kind: &str) -> Vec<Outcome> {
        let prefix = format!("signal:remedy:{kind}:");
        self.records(EventType::PolicyPlanningSignalObserved)
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Policy(PolicyPayload::PlanningSignalObserved(payload))
                    if payload.signal_id().starts_with(&prefix) =>
                {
                    Some(payload.outcome().clone())
                }
                _ => None,
            })
            .collect()
    }

    // The reasons of every `policy.dispatch_intent` and `policy.dispatch_admitted` record of
    // `task_id`, one list per record, in ledger order.
    fn dispatch_reasons(&self, task_id: &str) -> Vec<Vec<Outcome>> {
        let mut records = self.records(EventType::PolicyDispatchIntent);
        records.extend(self.records(EventType::PolicyDispatchAdmitted));
        records.sort_by_key(PersistedEvent::sequence);
        records
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Policy(
                    PolicyPayload::DispatchIntent(payload)
                    | PolicyPayload::DispatchAdmitted(payload),
                ) if payload.task_id() == task_id => Some(
                    payload
                        .reasons()
                        .iter()
                        .map(|reason| reason.0.clone())
                        .collect(),
                ),
                _ => None,
            })
            .collect()
    }
}

// The evaluation's dispatch of `task_id` with its reason chain, if it has one.
fn dispatched(
    evaluation: &PolicyEvaluation,
    task_id: &str,
) -> Option<(DispatchIntent, DecisionReasonChain)> {
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == task_id && intent.instance_id == POLICY_INSTANCE_ALIAS)?
        .clone();
    let chain = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the intent's reason chain")
        .clone();
    Some((intent, chain))
}

// The reasons of the evaluation's decision of `task_id` on the fixture instance.
fn decision_reasons(evaluation: &PolicyEvaluation, task_id: &str) -> Vec<Outcome> {
    evaluation
        .decisions
        .iter()
        .filter(|decision| {
            decision.task_id == task_id
                && decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS)
        })
        .flat_map(|decision| decision.reasons.iter().map(|reason| reason.outcome.clone()))
        .collect()
}

fn has_code(outcomes: &[Outcome], code: &str) -> bool {
    outcomes
        .iter()
        .any(|outcome| outcome.code().as_str() == code)
}

fn of_code<'a>(outcomes: &'a [Outcome], code: &str) -> Vec<&'a Outcome> {
    outcomes
        .iter()
        .filter(|outcome| outcome.code().as_str() == code)
        .collect()
}

// The outcome's value of `key` as text.
fn text<'a>(outcome: &'a Outcome, key: &str) -> Option<&'a str> {
    outcome.value(key).and_then(Value::as_str)
}

// When the instance may run its next dispatch after the run of `context`.
fn next_eligible(context: &PolicyRunContext) -> u64 {
    context.admission().activity.next_eligible_unix_ms
}

// ---- Tests.

// Requirements 2, 3 and 7: a run of `t_main` that ends on `blocked_k` opens a round; the next
// evaluation requests `t_sink` (its dispatch carries `policy_remedy_requested` naming the round),
// promoted ahead of `t_main`, which it holds; after the remedy's success the next one retries
// `t_main` once (`policy_remedy_retry`); a retry that ends on `done` resolves the round, so the
// next dispatch of `t_main` carries no remedy reason. A blocked run is a success: no failure is recorded and the
// failure streak stays 0.
#[test]
fn b15h_a_blocked_run_is_followed_by_its_remedy_and_one_retry_and_the_chain_reasons_are_recorded() {
    let fixture = Fixture::new(&main_pack(&["cap_x"], Some(&["cap_x"])), &sink_pack());
    let (_, _, blocked) = fixture.evaluate_and_run(POLICY_NOW_UNIX_MS, 7, MAIN_TASK, PAGE_B);
    let round = blocked.decision_id().to_owned();

    let (requesting, request_chain, remedy) =
        fixture.evaluate_and_run(next_eligible(&blocked), 8, SINK_TASK, RESULT);
    assert!(
        dispatched(&requesting, MAIN_TASK).is_none(),
        "the blocked task is held: {requesting:#?}"
    );
    // The request is promoted ahead of the instance's other candidates and takes the instance
    // (model-b15 §7.3, `contracts/remedies.md` section 5.3: one winner per instance), so `t_main`
    // is settled by `instance_already_selected` before its eligibility is asked, where its hold
    // (position 8a) would be its reason.
    assert!(
        request_chain.reasons.iter().any(|reason| {
            reason.outcome.code().as_str() == "rank_breakdown"
                && reason.detail.contains(" promoted=1 ")
        }),
        "{request_chain:?}"
    );
    let main_reasons = decision_reasons(&requesting, MAIN_TASK);
    assert_eq!(
        main_reasons.last().map(|outcome| outcome.code().as_str()),
        Some(ALREADY_SELECTED),
        "{main_reasons:?}"
    );
    let chain = request_chain
        .reasons
        .iter()
        .map(|reason| reason.outcome.clone())
        .collect::<Vec<_>>();
    let requested = of_code(&chain, REQUESTED);
    assert_eq!(requested.len(), 1, "{chain:?}");
    assert_eq!(text(requested[0], "task_id"), Some(MAIN_TASK));
    assert_eq!(text(requested[0], "decision_id"), Some(round.as_str()));
    assert_eq!(text(requested[0], "outcome_key"), Some(BLOCKED_KEY));
    assert_eq!(text(requested[0], "need"), Some("need_a"));
    assert_eq!(text(requested[0], "capability"), Some("cap_x"));
    assert_eq!(text(requested[0], "remedy_source"), Some("package"));

    let (_, retry_chain, retried) =
        fixture.evaluate_and_run(next_eligible(&remedy), 9, MAIN_TASK, RESULT);
    let chain = retry_chain
        .reasons
        .iter()
        .map(|reason| reason.outcome.clone())
        .collect::<Vec<_>>();
    let retry = of_code(&chain, RETRY);
    assert_eq!(retry.len(), 1, "{chain:?}");
    assert_eq!(text(retry[0], "task_id"), Some(MAIN_TASK));
    assert_eq!(text(retry[0], "decision_id"), Some(round.as_str()));
    assert_eq!(text(retry[0], "capability"), Some("cap_x"));

    // The round is resolved: the next dispatch of `t_main` follows its own trigger.
    let after = fixture.evaluate(next_eligible(&retried), 10);
    let (_, after_chain) = dispatched(&after, MAIN_TASK).expect("t_main follows its trigger");
    assert!(
        !after_chain
            .reasons
            .iter()
            .any(|reason| { [REQUESTED, RETRY].contains(&reason.outcome.code().as_str()) }),
        "{after_chain:?}"
    );
    assert!(dispatched(&after, SINK_TASK).is_none());

    // The ledger: the reasons in the dispatch records, and no failure.
    let sink_records = fixture.dispatch_reasons(SINK_TASK);
    assert!(!sink_records.is_empty());
    assert!(
        sink_records
            .iter()
            .all(|reasons| has_code(reasons, REQUESTED)),
        "{sink_records:?}"
    );
    let main_records = fixture.dispatch_reasons(MAIN_TASK);
    assert_eq!(
        main_records
            .iter()
            .filter(|reasons| has_code(reasons, RETRY))
            .count(),
        2,
        "the retry's intent and admission: {main_records:?}"
    );
    for event in fixture.records(EventType::PolicyExecutionRecorded) {
        let EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload)) = event.payload()
        else {
            panic!("an execution record");
        };
        assert!(
            matches!(
                payload.policy_result(),
                PolicyExecutionOutcome::Succeeded { .. }
            ),
            "{:?}",
            payload.policy_result()
        );
    }
    let streak = fixture
        .host
        .runtime_fact_snapshot()
        .expect("the program facts")
        .records
        .into_iter()
        .find(|record| record.key == format!("task.{MAIN_TASK}.failure_streak"))
        .expect("t_main's failure streak");
    assert_eq!(streak.value, ContractFactValue::Integer(0));
    fixture.host.close().expect("close the runtime host");
}

// Requirements 3 and 5: with no target record, no configuration row and no package default the
// round stops for a person: one `signal:remedy:stopped:` record with `policy_remedy_unchosen`
// (`no_choice`), and `t_main` is deferred with it at position 8a, ahead of its activity interval,
// and not dispatched. Once an agent publishes `session.remedy.choices` allowing `cap_x`, the
// next evaluation records one `policy_remedy_lifted`; `t_main` follows its trigger again and,
// blocked again, its remedy is requested by the target record.
#[test]
fn b15h_no_choice_stops_once_and_a_written_choice_lifts_it() {
    let fixture = Fixture::new(&main_pack(&["cap_x"], None), &sink_pack());
    let (_, _, blocked) = fixture.evaluate_and_run(POLICY_NOW_UNIX_MS, 7, MAIN_TASK, PAGE_B);
    let round = blocked.decision_id().to_owned();

    // Inside the activity interval of the blocked run: the stop, not the interval, defers it.
    let stopped = fixture.evaluate(POLICY_NOW_UNIX_MS + 2_000, 8);
    assert!(dispatched(&stopped, MAIN_TASK).is_none(), "{stopped:#?}");
    assert!(dispatched(&stopped, SINK_TASK).is_none(), "{stopped:#?}");
    let reasons = decision_reasons(&stopped, MAIN_TASK);
    assert!(has_code(&reasons, UNCHOSEN), "{reasons:?}");
    let again = fixture.evaluate(POLICY_NOW_UNIX_MS + 4_000, 9);
    assert!(dispatched(&again, MAIN_TASK).is_none(), "{again:#?}");
    let stops = fixture.remedy_signals("stopped");
    assert_eq!(stops.len(), 1, "{stops:?}");
    assert_eq!(stops[0].code().as_str(), UNCHOSEN);
    assert_eq!(text(&stops[0], "remedy_problem"), Some("no_choice"));
    assert_eq!(text(&stops[0], "task_id"), Some(MAIN_TASK));
    assert_eq!(text(&stops[0], "decision_id"), Some(round.as_str()));

    // An agent's target record allows `cap_x` for `need_a`.
    let written_at = POLICY_NOW_UNIX_MS + 5_000;
    advance_manual_clock_to(&fixture.clock, written_at);
    fixture
        .host
        .publish_fact(FactRecord {
            scope: FactScope::Instance {
                instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            },
            key: "session.remedy.choices".to_owned(),
            content: FactContent::Inline {
                value: ContractFactValue::RecordList(vec![BTreeMap::from([
                    (
                        "row".to_owned(),
                        actingcommand_contract::FactScalar::String("allow".to_owned()),
                    ),
                    (
                        "need".to_owned(),
                        actingcommand_contract::FactScalar::String("need_a".to_owned()),
                    ),
                    (
                        "capability".to_owned(),
                        actingcommand_contract::FactScalar::String("cap_x".to_owned()),
                    ),
                ])]),
            },
            observed_at_unix_ms: written_at,
            expires_at_unix_ms: None,
            ttl_policy: None,
            confidence_milli: 1_000,
            source_detector: "agent.fixture".to_owned(),
            source_snapshot_id: "snapshot:remedy-choices".to_owned(),
            schema_version: "fact.v1".to_owned(),
            resource_bundle_hash: "a".repeat(64),
            invalidate_on: Vec::new(),
        })
        .expect("publish the target record");

    let (lifting, lifting_chain, blocked_again) =
        fixture.evaluate_and_run(next_eligible(&blocked), 10, MAIN_TASK, PAGE_B);
    let lifts = fixture.remedy_signals("lifted");
    assert_eq!(lifts.len(), 1, "{lifts:?}");
    assert_eq!(lifts[0].code().as_str(), LIFTED);
    assert_eq!(text(&lifts[0], "decision_id"), Some(round.as_str()));
    assert!(
        !has_code(&decision_reasons(&lifting, MAIN_TASK), UNCHOSEN),
        "{lifting:#?}"
    );
    assert!(
        !lifting_chain
            .reasons
            .iter()
            .any(|reason| reason.outcome.code().as_str() == RETRY),
        "the lifted task follows its own trigger: {lifting_chain:?}"
    );

    // The remedy proceeds: the new blocked run's remedy is requested, allowed by the record.
    let requesting = fixture.evaluate(next_eligible(&blocked_again), 11);
    let (_, chain) = dispatched(&requesting, SINK_TASK)
        .unwrap_or_else(|| panic!("the remedy is requested: {requesting:#?}"));
    let reasons = chain
        .reasons
        .iter()
        .map(|reason| reason.outcome.clone())
        .collect::<Vec<_>>();
    let requested = of_code(&reasons, REQUESTED);
    assert_eq!(requested.len(), 1, "{reasons:?}");
    assert_eq!(
        text(requested[0], "decision_id"),
        Some(blocked_again.decision_id())
    );
    assert_eq!(text(requested[0], "remedy_source"), Some("target"));
    assert_eq!(fixture.remedy_signals("lifted").len(), 1);
    assert_eq!(fixture.remedy_signals("stopped").len(), 1);
    fixture.host.close().expect("close the runtime host");
}

// Requirement 2: a round open when the Runtime stops is rebuilt at start from the ledger (the
// blocked run's admission and terminal, the remedy's dispatch and settlement): after a restart
// between the provider's success and the retry, the retry follows, exactly once.
#[test]
fn b15h_an_open_round_survives_a_restart_and_retries_once() {
    let fixture = Fixture::new(&main_pack(&["cap_x"], Some(&["cap_x"])), &sink_pack());
    let (_, _, blocked) = fixture.evaluate_and_run(POLICY_NOW_UNIX_MS, 7, MAIN_TASK, PAGE_B);
    let round = blocked.decision_id().to_owned();
    let (_, _, remedy) = fixture.evaluate_and_run(next_eligible(&blocked), 8, SINK_TASK, RESULT);

    let restart_at = next_eligible(&remedy);
    let fixture = fixture.restart(restart_at);
    let (_, chain, retried) = fixture.evaluate_and_run(restart_at, 9, MAIN_TASK, RESULT);
    let reasons = chain
        .reasons
        .iter()
        .map(|reason| reason.outcome.clone())
        .collect::<Vec<_>>();
    let retry = of_code(&reasons, RETRY);
    assert_eq!(retry.len(), 1, "{reasons:?}");
    assert_eq!(text(retry[0], "decision_id"), Some(round.as_str()));

    let after = fixture.evaluate(next_eligible(&retried), 10);
    let (_, after_chain) = dispatched(&after, MAIN_TASK).expect("t_main follows its trigger");
    assert!(
        !after_chain
            .reasons
            .iter()
            .any(|reason| reason.outcome.code().as_str() == RETRY),
        "{after_chain:?}"
    );
    let intents_with_retry = fixture
        .records(EventType::PolicyDispatchIntent)
        .iter()
        .filter(|event| match event.payload() {
            EventPayload::Policy(PolicyPayload::DispatchIntent(payload)) => payload
                .reasons()
                .iter()
                .any(|reason| reason.0.code().as_str() == RETRY),
            _ => false,
        })
        .count();
    assert_eq!(intents_with_retry, 1, "exactly one retry");
    fixture.host.close().expect("close the runtime host");
}

// Requirement 4: a request whose remedy was served between its evaluation and its admission is
// refused as a rejected admission with `policy_remedy_round_closed` (the round's blocked task,
// the instance, the round): no lease and no run. The served remedy also moved the instance's
// task states, which the staleness check would refuse first (`policy_facts_stale`); the
// admission's projection is pinned at the request's evaluation position so the request reaches
// the remedy check.
#[test]
fn b15h_a_remedy_served_between_evaluation_and_admission_is_refused_round_closed() {
    let fixture = Fixture::new(&main_pack(&["cap_x"], Some(&["cap_x"])), &sink_pack());
    let (_, _, blocked) = fixture.evaluate_and_run(POLICY_NOW_UNIX_MS, 7, MAIN_TASK, PAGE_B);
    let round = blocked.decision_id().to_owned();

    let at = next_eligible(&blocked);
    let first = fixture.evaluate(at, 8);
    let (late, late_chain) =
        dispatched(&first, SINK_TASK).unwrap_or_else(|| panic!("a request: {first:#?}"));
    assert!(has_code(
        &late_chain
            .reasons
            .iter()
            .map(|reason| reason.outcome.clone())
            .collect::<Vec<_>>(),
        REQUESTED
    ));
    // The same request again, evaluated later, is admitted and serves the remedy.
    fixture.evaluate_and_run(at + 2_000, 9, SINK_TASK, RESULT);

    let leases = fixture.records(EventType::LeaseGranted).len();
    fixture
        .host
        .capacity_sampler_for_test()
        .expect("capacity owner")()
    .expect("capacity sample at the admission time");
    record_policy_approval(&fixture.host, &late);
    fixture
        .host
        .override_next_policy_outcome_projection_position_for_test(late.input_ledger_position)
        .expect("pin the admission's projection");
    let error = fixture
        .host
        .admit_scheduled_policy_dispatch(
            &late,
            &late_chain,
            &policy_context(&fixture.host, &late),
            &fixture.request(SINK_TASK),
        )
        .expect_err("the round's layer is served");
    assert_eq!(error.code(), ROUND_CLOSED, "{error:?}");
    let rejected = fixture
        .records(EventType::PolicyDispatchRejected)
        .into_iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Policy(PolicyPayload::DispatchRejected(payload))
                if payload.decision_id() == late.decision_id =>
            {
                payload
                    .rejection()
                    .map(|rejection| rejection.outcome.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(rejected.len(), 1, "{rejected:?}");
    assert_eq!(rejected[0].code().as_str(), ROUND_CLOSED);
    assert_eq!(text(&rejected[0], "task_id"), Some(MAIN_TASK));
    assert_eq!(
        text(&rejected[0], "instance_id"),
        Some(POLICY_INSTANCE_ALIAS)
    );
    assert_eq!(text(&rejected[0], "decision_id"), Some(round.as_str()));
    assert_eq!(
        fixture.records(EventType::LeaseGranted).len(),
        leases,
        "no lease"
    );
    assert!(
        !fixture
            .records(EventType::PolicyDispatchAdmitted)
            .iter()
            .any(|event| match event.payload() {
                EventPayload::Policy(PolicyPayload::DispatchAdmitted(payload)) => {
                    payload.decision_id() == late.decision_id
                }
                _ => false,
            }),
        "no run"
    );
    fixture.host.close().expect("close the runtime host");
}

// Requirements 3 and 5: a round still open when the activity window of its root blocked run ends
// expires: one `signal:remedy:expired:` record with `policy_remedy_expired` and the window's end,
// written once across the evaluations after it.
#[test]
fn b15h_a_round_open_at_its_window_end_records_one_expiry() {
    let fixture = Fixture::new(&main_pack(&["cap_x"], Some(&["cap_x"])), &sink_pack());
    let (_, _, blocked) = fixture.evaluate_and_run(POLICY_NOW_UNIX_MS, 7, MAIN_TASK, PAGE_B);
    let round = blocked.decision_id().to_owned();
    // The request is evaluated but never admitted: the round stays open.
    let open = fixture.evaluate(next_eligible(&blocked), 8);
    assert!(dispatched(&open, SINK_TASK).is_some(), "{open:#?}");
    assert!(fixture.remedy_signals("expired").is_empty());

    fixture.evaluate(WINDOW_END_UNIX_MS + 1_000, 9);
    fixture.evaluate(WINDOW_END_UNIX_MS + 3_000, 10);
    let expiries = fixture.remedy_signals("expired");
    assert_eq!(expiries.len(), 1, "{expiries:?}");
    assert_eq!(expiries[0].code().as_str(), EXPIRED);
    assert_eq!(text(&expiries[0], "task_id"), Some(MAIN_TASK));
    assert_eq!(text(&expiries[0], "decision_id"), Some(round.as_str()));
    assert_eq!(text(&expiries[0], "outcome_key"), Some(BLOCKED_KEY));
    assert_eq!(
        expiries[0]
            .value("window_end_unix_ms")
            .and_then(Value::as_u64),
        Some(WINDOW_END_UNIX_MS)
    );
    fixture.host.close().expect("close the runtime host");
}

// Requirements 1 and 3: a remedy whose capability no catalog task provides on the instance is a
// table finding `capability_unbound`, and its blocked run stops the round
// `policy_remedy_unavailable` (`capability_unbound`).
#[test]
fn b15h_a_capability_without_provider_task_stops_unavailable() {
    let fixture = Fixture::new(&main_pack(&["cap_y"], Some(&["cap_y"])), &sink_pack());
    let (_, _, blocked) = fixture.evaluate_and_run(POLICY_NOW_UNIX_MS, 7, MAIN_TASK, PAGE_B);
    let stopped = fixture.evaluate(next_eligible(&blocked), 8);
    assert!(dispatched(&stopped, MAIN_TASK).is_none(), "{stopped:#?}");
    assert!(dispatched(&stopped, SINK_TASK).is_none(), "{stopped:#?}");
    assert!(has_code(
        &decision_reasons(&stopped, MAIN_TASK),
        UNAVAILABLE
    ));
    let stops = fixture.remedy_signals("stopped");
    assert_eq!(stops.len(), 1, "{stops:?}");
    assert_eq!(stops[0].code().as_str(), UNAVAILABLE);
    assert_eq!(
        text(&stops[0], "remedy_problem"),
        Some("capability_unbound")
    );

    let table = fixture
        .host
        .remedy_table_for_test()
        .expect("the policy host")
        .expect("the remedy table");
    assert!(
        table.findings.contains(&crate::remedy::RemedyFinding {
            problem: actingcommand_contract::outcome::RemedyProblem::CapabilityUnbound,
            task_id: MAIN_TASK.to_owned(),
            outcome_key: BLOCKED_KEY.to_owned(),
            need: "need_a".to_owned(),
            capability: Some("cap_y".to_owned()),
            instance_id: Some(POLICY_INSTANCE_ALIAS.to_owned()),
            package_id: Some(MAIN_PACKAGE.to_owned()),
        }),
        "{:?}",
        table.findings
    );
    fixture.host.close().expect("close the runtime host");
}

// Requirement 1: two tasks whose packages provide each other's remedy form a cycle; the table
// refuses both edges, and a blocked run of either stops `policy_remedy_unavailable` (`cycle`)
// instead of requesting the other.
#[test]
fn b15h_two_tasks_providing_each_others_remedy_are_refused_as_a_cycle() {
    let mut main = main_pack(&["cap_x"], Some(&["cap_x"]));
    main.provides = Some(json!([{"capability": "cap_y"}]));
    let sink = Pack {
        package_id: SINK_PACKAGE,
        remedy: Some(json!({"need": "need_b", "capabilities": ["cap_y"], "default": ["cap_y"]})),
        provides: Some(json!([{"capability": "cap_x"}])),
    };
    let fixture = Fixture::new(&main, &sink);
    let (_, _, blocked) = fixture.evaluate_and_run(POLICY_NOW_UNIX_MS, 7, MAIN_TASK, PAGE_B);
    let stopped = fixture.evaluate(next_eligible(&blocked), 8);
    assert!(dispatched(&stopped, SINK_TASK).is_none(), "{stopped:#?}");
    assert!(dispatched(&stopped, MAIN_TASK).is_none(), "{stopped:#?}");
    let stops = fixture.remedy_signals("stopped");
    assert_eq!(stops.len(), 1, "{stops:?}");
    assert_eq!(stops[0].code().as_str(), UNAVAILABLE);
    assert_eq!(text(&stops[0], "remedy_problem"), Some("cycle"));

    let table = fixture
        .host
        .remedy_table_for_test()
        .expect("the policy host")
        .expect("the remedy table");
    for (blocked_task_id, provider_task_id) in [(MAIN_TASK, SINK_TASK), (SINK_TASK, MAIN_TASK)] {
        assert!(
            table.table.refused_edges.contains(&RemedyTableEdge {
                blocked_task_id: blocked_task_id.to_owned(),
                provider_task_id: provider_task_id.to_owned(),
            }),
            "{:?}",
            table.table.refused_edges
        );
    }
    fixture.host.close().expect("close the runtime host");
}

// Requirement 5: a stop is recorded once per round, checked against the planning-signal
// projection before writing: later evaluations and a restart record no second one, and the root
// pair stays deferred with it after the restart.
#[test]
fn b15h_a_stop_is_recorded_once_across_evaluations_and_a_restart() {
    let fixture = Fixture::new(&main_pack(&["cap_x"], None), &sink_pack());
    let (_, _, blocked) = fixture.evaluate_and_run(POLICY_NOW_UNIX_MS, 7, MAIN_TASK, PAGE_B);
    fixture.evaluate(POLICY_NOW_UNIX_MS + 2_000, 8);
    fixture.evaluate(POLICY_NOW_UNIX_MS + 4_000, 9);
    assert_eq!(fixture.remedy_signals("stopped").len(), 1);

    let restart_at = next_eligible(&blocked);
    let fixture = fixture.restart(restart_at);
    let after = fixture.evaluate(restart_at, 10);
    assert!(dispatched(&after, MAIN_TASK).is_none(), "{after:#?}");
    assert!(
        has_code(&decision_reasons(&after, MAIN_TASK), UNCHOSEN),
        "{:?}",
        decision_reasons(&after, MAIN_TASK)
    );
    fixture.evaluate(restart_at + 2_000, 11);
    let stops = fixture.remedy_signals("stopped");
    assert_eq!(stops.len(), 1, "{stops:?}");
    assert!(fixture.remedy_signals("lifted").is_empty());
    fixture.host.close().expect("close the runtime host");
}

// Requirement 2: the key of a blocked run is read from its terminal's `scheduling_disposition`
// although the catalog references no outcome key of `t_main`, and it opens a round: the next
// evaluation requests the remedy for that key.
#[test]
fn b15h_a_blocked_key_the_catalog_does_not_reference_still_opens_a_round() {
    let compiled = compile_catalog(&catalog()).expect("the catalog compiles");
    assert!(compiled.referenced_outcome_keys(MAIN_TASK).is_empty());
    let fixture = Fixture::new(&main_pack(&["cap_x"], Some(&["cap_x"])), &sink_pack());
    let (_, _, blocked) = fixture.evaluate_and_run(POLICY_NOW_UNIX_MS, 7, MAIN_TASK, PAGE_B);
    let keys = fixture
        .host
        .query_persisted_events_for_test(EventQuery {
            run_id: Some(blocked.run_id()),
            event_type: Some(EventType::TaskCompleted),
            ..EventQuery::default()
        })
        .expect("the run's terminal")
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                TaskSemanticFact::TerminalCommitted {
                    scheduling_disposition: Some(disposition),
                    ..
                } => Some(disposition.outcome_key().to_owned()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(keys, [BLOCKED_KEY]);

    let requesting = fixture.evaluate(next_eligible(&blocked), 8);
    let (_, chain) = dispatched(&requesting, SINK_TASK)
        .unwrap_or_else(|| panic!("the round requests its remedy: {requesting:#?}"));
    let reasons = chain
        .reasons
        .iter()
        .map(|reason| reason.outcome.clone())
        .collect::<Vec<_>>();
    let requested = of_code(&reasons, REQUESTED);
    assert_eq!(requested.len(), 1, "{reasons:?}");
    assert_eq!(text(requested[0], "outcome_key"), Some(BLOCKED_KEY));
    assert_eq!(
        text(requested[0], "decision_id"),
        Some(blocked.decision_id())
    );
    fixture.host.close().expect("close the runtime host");
}
