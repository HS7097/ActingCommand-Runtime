// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 OUT-2 (forced-compromises mapping rows O-01, O-02, R-03, S-01; FCM §2.4 row
// OUT-2): a task's designated effects may be a set; a member may be repeated up to an explicit
// cap, a constant or the value of an instance fact (S-01), so a run can loop over a counted
// business step (R-03) and leave the loop when the cap is reached, without leaning on
// `max_steps`. Every expected value comes from the FCM rows and the declaration rules of the
// contract (`contracts/resource-declarations.md`, Task schema `0.9` designated effect sets).

use super::*;
use actingcommand_contract::{
    FactContent, FactRecord, FactScope, FactTtlPolicy, FactTtlSource, FactValue,
    InstanceFactContext, InstanceFactSnapshot,
};
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Value, json};
use std::path::PathBuf;

const STEP_TIMEOUT: Duration = Duration::from_secs(10);
const TASK_TIMEOUT: Duration = Duration::from_secs(60);
const NOW_UNIX_MS: u64 = 1_000_000;
const CAP_FACT: &str = "session.example.collect.remaining";
const UNRESOLVED: &str = "contained_task_designated_effect_cap_unresolved";
// The required home entry page of a page-graph task that starts on `home`.
const HOME: &str = "neutral/home";

// What the screen shows: the left pixel names the page.
#[derive(Debug, Clone, Copy)]
enum Screen {
    Home,
    Terminal,
}

fn frame(screen: Screen) -> Frame {
    let page = match screen {
        Screen::Home => [255, 0, 0],
        Screen::Terminal => [0, 0, 255],
    };
    Frame::from_pixels(
        2,
        1,
        [page, [0, 255, 0]].concat(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
}

// The fact snapshot the runtime gives: none, or the instance's records.
enum Facts {
    Unavailable,
    Records(Vec<FactRecord>),
}

// The Runtime as the kernel sees it: every capture shows the screen of the number of inputs made
// so far (the last one repeats); it records each completed effect and counts its fact reads.
struct ScriptedRuntime {
    screens: Vec<Screen>,
    facts: Facts,
    fact_reads: usize,
    inputs: usize,
    effects: Vec<String>,
}

impl ScriptedRuntime {
    fn new(screens: Vec<Screen>, facts: Facts) -> Self {
        Self {
            screens,
            facts,
            fact_reads: 0,
            inputs: 0,
            effects: Vec::new(),
        }
    }

    fn effects_of(&self, operation_id: &str) -> usize {
        self.effects
            .iter()
            .filter(|effect| effect.as_str() == operation_id)
            .count()
    }
}

impl ContainedTaskRuntime for ScriptedRuntime {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        let screen = self.screens[self.inputs.min(self.screens.len() - 1)];
        Ok(frame(screen).into())
    }

    fn selection_state(
        &mut self,
        request: SelectionStateRequest,
    ) -> Result<SelectionState, Self::Error> {
        self.fact_reads += 1;
        Ok(match &self.facts {
            Facts::Unavailable => SelectionState::Unavailable,
            Facts::Records(records) => SelectionState::Snapshot {
                snapshot: InstanceFactSnapshot {
                    snapshot_id: "snapshot:out2".to_owned(),
                    ledger_position: 1,
                    context: InstanceFactContext {
                        instance_id: "neutral.instance".to_owned(),
                        server_id: request.server,
                        game_id: request.game,
                    },
                    records: records.clone(),
                },
                now_unix_ms: NOW_UNIX_MS,
            },
        })
    }

    fn input(
        &mut self,
        _action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.inputs += 1;
        Ok(())
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        if let ContainedTaskTrace::EffectCompleted {
            operation_label, ..
        } = trace
        {
            self.effects.push(operation_label);
        }
        Ok(())
    }
}

// An instance fact of the cap's key: `value`, valid until `expires_at_unix_ms`.
fn cap_fact(value: FactValue, expires_at_unix_ms: u64) -> FactRecord {
    let observed_at_unix_ms = NOW_UNIX_MS - 1_000;
    FactRecord {
        scope: FactScope::Instance {
            instance_id: "neutral.instance".to_owned(),
        },
        key: CAP_FACT.to_owned(),
        content: FactContent::Inline { value },
        observed_at_unix_ms,
        expires_at_unix_ms: Some(expires_at_unix_ms),
        ttl_policy: Some(FactTtlPolicy {
            minimum_ms: 1,
            maximum_ms: 3_600_000,
            source: FactTtlSource::DetectorContract,
        }),
        confidence_milli: 1_000,
        source_detector: "fixture.decision".to_owned(),
        source_snapshot_id: "snapshot:decision".to_owned(),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "c".repeat(64),
        invalidate_on: Vec::new(),
    }
}

fn control() -> TaskControl {
    serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.out2.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": STEP_TIMEOUT.as_millis() as u64,
        "timeout_ms": TASK_TIMEOUT.as_millis() as u64,
        "max_steps": 20
    }))
    .expect("task control")
}

fn pack() -> RecognitionPack {
    serde_json::from_value(json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/home",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
            {"type": "color", "id": "page/terminal",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]}
        ]
    }))
    .expect("recognition pack")
}

fn page_set() -> PageSet {
    serde_json::from_value(json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/home", "required": ["page/home"]},
            {"id": "neutral/terminal", "required": ["page/terminal"]}
        ]
    }))
    .expect("page set")
}

// A trusted tap from `from` to `to`.
fn tap(id: &str, from: &str, to: Value) -> Value {
    json!({
        "id": id,
        "from": from,
        "to": to,
        "click": {"kind": "point", "x": 0, "y": 0},
        "unguarded_trusted_coordinate": true
    })
}

// `collect` on `home` comes back to `home` (a counted business step), `leave` goes to the
// terminal page.
fn loop_operations() -> Vec<Value> {
    vec![
        tap("collect", "home", json!("home")),
        tap("leave", "home", json!("terminal")),
    ]
}

// The declaration of `effects`: "collected" when a designated effect completed, "nothing"
// otherwise, both on the terminal page.
fn declaration(effects: Value) -> Value {
    json!({
        "designated_effects": effects,
        "mappings": [
            {"outcome_key": "collected", "effect": "designated_effect_completed",
             "terminal_pages": ["terminal"]},
            {"outcome_key": "nothing", "effect": "no_designated_effect",
             "terminal_pages": ["terminal"]}
        ]
    })
}

fn scheduling(value: Value) -> SchedulingOutcomeDeclaration {
    serde_json::from_value(value).expect("scheduling outcome declaration")
}

fn task(operations: Vec<Value>, outcome: Value) -> PreparedContainedTask {
    let control = control();
    let program: TaskProgram = serde_json::from_value(json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "target_page": "terminal",
        "scheduling_outcome": outcome,
        "operations": operations
    }))
    .expect("task program");
    for operation in &program.operations {
        operation
            .validate(&control, TaskOperationDefaults::default(), "0.9")
            .expect("valid operation");
    }
    let evaluator = RecognitionEvaluator::new(PathBuf::new(), pack()).expect("evaluator");
    let detector = PageDetector::new(page_set()).expect("page detector");
    detector
        .validate(&evaluator)
        .expect("page detector targets");
    let scheduling_outcome = program.scheduling_outcome.clone();
    PreparedContainedTask {
        control,
        program,
        evaluator,
        detector,
        entry_page: None,
        scheduling_outcome,
        post_admission_ocr: None,
        post_admission_fields: None,
        entry_count: 1,
        task_count: 1,
    }
}

// The static reachability check of a page-graph task over `operations`, from every page (`None`:
// a run may start on any page, the terminal one included) or from the required home entry page.
fn coverage(
    entry: Option<&str>,
    operations: Vec<Value>,
    outcome: Value,
) -> Result<(), ContainedTaskError> {
    let operations = operations
        .into_iter()
        .map(|operation| serde_json::from_value::<TaskOperation>(operation).expect("operation"))
        .collect::<Vec<_>>();
    validate_scheduling_outcome_coverage(
        entry,
        "neutral",
        &["terminal".to_owned()],
        &["neutral/home".to_owned(), "neutral/terminal".to_owned()],
        &operations,
        &scheduling(outcome),
    )
}

fn caps(outcome: &ContainedTaskOutcome) -> Vec<(String, u32)> {
    outcome
        .designated_effect_caps
        .iter()
        .map(|cap| (cap.operation_id.clone(), cap.max_repeats))
        .collect()
}

fn succeeded(
    result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
) -> ContainedTaskOutcome {
    match result {
        Ok(outcome) => {
            assert_eq!(outcome.outcome, TaskOutcome::Success);
            outcome
        }
        Err(error) => panic!("expected the run to succeed, got {error:?}"),
    }
}

fn task_failure(
    result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
) -> ContainedTaskError {
    match result {
        Err(ContainedTaskRunError::Task(error)) => error,
        Err(other) => panic!("expected a task failure, got {other:?}"),
        Ok(outcome) => panic!(
            "expected a task failure, the run ended {:?}",
            outcome.outcome
        ),
    }
}

// FCM O-01, O-02 (an explicit cap, a constant or a fact key), S-01: what a designated effect set
// declares, and the page graphs admission takes: a capped member must leave its page a way on.
#[test]
fn out2_a_designated_effect_set_declares_caps_and_needs_a_way_on_once_capped() {
    for (case, effects) in [
        ("one member, once", json!([{"operation_id": "collect"}])),
        (
            "a constant cap",
            json!([{"operation_id": "collect", "max_repeats": 1000}]),
        ),
        (
            "a fact cap",
            json!([{"operation_id": "collect", "max_repeats": {"fact_key": CAP_FACT}}]),
        ),
        (
            "two members",
            json!([{"operation_id": "collect", "max_repeats": 3}, {"operation_id": "leave"}]),
        ),
    ] {
        scheduling(declaration(effects))
            .validate()
            .unwrap_or_else(|error| panic!("{case}: {error:?}"));
    }
    let nine = (0..9)
        .map(|index| json!({"operation_id": format!("op{index}")}))
        .collect::<Vec<_>>();
    for (case, outcome) in [
        ("an empty set", declaration(json!([]))),
        ("more than eight members", declaration(json!(nine))),
        (
            "a member twice",
            declaration(json!([{"operation_id": "collect"}, {"operation_id": "collect"}])),
        ),
        (
            "a constant cap of 0",
            declaration(json!([{"operation_id": "collect", "max_repeats": 0}])),
        ),
        (
            "a constant cap above 1000",
            declaration(json!([{"operation_id": "collect", "max_repeats": 1001}])),
        ),
        (
            "a fact cap under no fact key",
            declaration(
                json!([{"operation_id": "collect", "max_repeats": {"fact_key": "collect"}}]),
            ),
        ),
        ("both forms", {
            let mut outcome = declaration(json!([{"operation_id": "collect"}]));
            outcome["designated_operation"] = json!("collect");
            outcome
        }),
    ] {
        assert!(
            scheduling(outcome).validate().is_err(),
            "{case}: the declaration is refused"
        );
    }
    let incomplete = ExecutionKernelCode::ContainedTaskOutcomeDeclarationIncomplete.as_str();
    let refused = |result: Result<(), ContainedTaskError>| {
        result
            .expect_err("admission refuses the page graph")
            .code()
            .as_str()
            .to_owned()
    };

    // A counted loop on `home` with `leave` to go on once the cap is reached.
    coverage(
        None,
        loop_operations(),
        declaration(json!([{"operation_id": "collect", "max_repeats": 3}])),
    )
    .expect("a constant cap with a way on");
    coverage(
        None,
        loop_operations(),
        declaration(json!([{"operation_id": "collect", "max_repeats": {"fact_key": CAP_FACT}}])),
    )
    .expect("a fact cap with a way on");
    // Without `leave`, a capped `collect` leaves `home` without an operation.
    assert_eq!(
        refused(coverage(
            Some(HOME),
            vec![tap("collect", "home", json!(["home", "terminal"]))],
            declaration(json!([{"operation_id": "collect", "max_repeats": 3}])),
        )),
        incomplete
    );
    // The terminal page reached with no designated effect needs its own mapping.
    let only_completed = |effects: Value| {
        let mut outcome = declaration(effects);
        outcome["mappings"] = json!([{"outcome_key": "collected",
            "effect": "designated_effect_completed", "terminal_pages": ["terminal"]}]);
        outcome
    };
    // A fact cap may be 0: a run from `home` then leaves at once with no designated effect.
    assert_eq!(
        refused(coverage(
            Some(HOME),
            loop_operations(),
            only_completed(json!([{"operation_id": "collect",
                "max_repeats": {"fact_key": CAP_FACT}}])),
        )),
        incomplete
    );
    // A constant cap is at least 1: a run from `home` runs `collect` on its first visit.
    coverage(
        Some(HOME),
        loop_operations(),
        only_completed(json!([{"operation_id": "collect", "max_repeats": 3}])),
    )
    .expect("from home, a constant cap needs no mapping without an effect");
    // Without a required home entry page a run may start on the terminal page, with no
    // designated effect, whatever the cap (the existing page-graph rule).
    assert_eq!(
        refused(coverage(
            None,
            loop_operations(),
            only_completed(json!([{"operation_id": "collect", "max_repeats": 3}])),
        )),
        incomplete
    );
}

// FCM O-01, O-02, R-03: one run repeats each member up to its cap, the next operation of the page
// runs once the cap is reached, and the run ends on its target without `max_steps` stopping it.
#[test]
fn out2_a_counted_loop_repeats_each_member_to_its_cap_then_goes_on() {
    // `collect` up to three times, then `bonus` once, then `leave`.
    let operations = vec![
        tap("collect", "home", json!("home")),
        tap("bonus", "home", json!("home")),
        tap("leave", "home", json!("terminal")),
    ];
    let outcome = declaration(json!([
        {"operation_id": "collect", "max_repeats": 3},
        {"operation_id": "bonus"}
    ]));
    let task = task(operations, outcome);
    let mut runtime = ScriptedRuntime::new(
        vec![
            Screen::Home,
            Screen::Home,
            Screen::Home,
            Screen::Home,
            Screen::Home,
            Screen::Terminal,
        ],
        Facts::Records(Vec::new()),
    );
    let ended = succeeded(task.run(&mut runtime));
    assert_eq!(ended.final_page.as_deref(), Some("neutral/terminal"));
    assert_eq!(runtime.effects_of("collect"), 3);
    assert_eq!(runtime.effects_of("bonus"), 1);
    assert_eq!(runtime.effects_of("leave"), 1);
    assert_eq!(runtime.inputs, 5);
    assert_eq!(ended.executed_steps, 5);
    assert_eq!(
        caps(&ended),
        vec![("collect".to_owned(), 3), ("bonus".to_owned(), 1)]
    );
    // Constant caps read no fact.
    assert_eq!(runtime.fact_reads, 0);

    // When the page changes before the cap, the loop ends there: done twice of three.
    let task = self::task(
        vec![
            tap("collect", "home", json!(["home", "terminal"])),
            tap("leave", "home", json!("terminal")),
        ],
        declaration(json!([{"operation_id": "collect", "max_repeats": 3}])),
    );
    let mut runtime = ScriptedRuntime::new(
        vec![Screen::Home, Screen::Home, Screen::Terminal],
        Facts::Records(Vec::new()),
    );
    let ended = succeeded(task.run(&mut runtime));
    assert_eq!(runtime.effects_of("collect"), 2);
    assert_eq!(runtime.effects_of("leave"), 0);
    assert_eq!(caps(&ended), vec![("collect".to_owned(), 3)]);
}

// FCM S-01: a fact cap is the integer value of the instance fact, read once before the run's
// first step; a fact that gives no cap ends the run before any input.
#[test]
fn out2_a_fact_cap_is_read_once_before_the_first_step() {
    let fact_capped = || {
        task(
            loop_operations(),
            declaration(json!([{"operation_id": "collect",
                "max_repeats": {"fact_key": CAP_FACT}}])),
        )
    };
    let home_until = |inputs: usize| {
        let mut screens = vec![Screen::Home; inputs];
        screens.push(Screen::Terminal);
        screens
    };
    for (case, value, collects) in [("two", 2, 2), ("none", 0, 0)] {
        let mut runtime = ScriptedRuntime::new(
            home_until(collects + 1),
            Facts::Records(vec![cap_fact(FactValue::Integer(value), NOW_UNIX_MS)]),
        );
        let ended = succeeded(fact_capped().run(&mut runtime));
        assert_eq!(runtime.effects_of("collect"), collects, "{case}");
        assert_eq!(runtime.effects_of("leave"), 1, "{case}");
        assert_eq!(runtime.fact_reads, 1, "{case}");
        assert_eq!(
            caps(&ended),
            vec![("collect".to_owned(), u32::try_from(value).expect("a cap"))],
            "{case}"
        );
    }

    let values = |error: &ContainedTaskError| Value::Object(error.outcome().values().clone());
    for (case, facts, extra) in [
        (
            "missing",
            Facts::Records(Vec::new()),
            json!({"unknown_reason": "fact_missing"}),
        ),
        (
            "expired",
            Facts::Records(vec![cap_fact(FactValue::Integer(2), NOW_UNIX_MS - 1)]),
            json!({"unknown_reason": "fact_expired"}),
        ),
        (
            "not an integer",
            Facts::Records(vec![cap_fact(FactValue::Boolean(true), NOW_UNIX_MS)]),
            json!({"unknown_reason": "type_mismatch"}),
        ),
        (
            "above the bound",
            Facts::Records(vec![cap_fact(FactValue::Integer(1001), NOW_UNIX_MS)]),
            json!({"count": 1001, "limit_count": 1000}),
        ),
        (
            "negative",
            Facts::Records(vec![cap_fact(FactValue::Integer(-1), NOW_UNIX_MS)]),
            json!({"count": -1, "limit_count": 1000}),
        ),
        ("no fact snapshot", Facts::Unavailable, json!({})),
    ] {
        let mut runtime = ScriptedRuntime::new(home_until(3), facts);
        let error = task_failure(fact_capped().run(&mut runtime));
        assert_eq!(error.code().as_str(), UNRESOLVED, "{case}");
        let mut expected = json!({"pack_operation_id": "collect", "fact_key": CAP_FACT});
        for (key, value) in extra.as_object().expect("extra values") {
            expected[key] = value.clone();
        }
        assert_eq!(values(&error), expected, "{case}");
        assert_eq!(
            runtime.inputs, 0,
            "{case}: no input before the cap is known"
        );
    }
}
