// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #469 D4b (0.12.63; register §1.401; the owner's approval on board #469 on 11-15 of the
// four corrections that change what the program does, this one of them), fix round 2: the host
// rows, modelled on the page row of `gate_387_d1cg.rs`. A scheduled run on the ladder fixture's
// physical instance (`ladder_setup_with_package`) of a package whose pages are colours and whose
// one located OCR target a scripted provider reads, at a step's guard or at an operation's `when`.
//
// A provider that fails there ends the run with the class-5 `contained_task_vision_provider_failed`
// before any input. Its lease end starts no ladder: no ladder claim holds or waits for the
// instance, the run's lease hands its key to no ladder, and no ladder starts at the task stage.
// Its terminal record is one direction ① counts toward the default `stability_burst`: the run's
// `task.failed`, linked to its instance, of warning severity or worse, whose code is of the class
// `environment_fault` (what `count_stability_record` reads; that three such failures of one
// instance within the window register one burst is the ladder test
// `d4b_a_vision_provider_failure_counts_toward_the_burst_and_starts_no_ladder_at_once`). A read
// that cannot conclude while the provider answered, a real evaluation error, ends the same fixture
// with `contained_task_guard_evaluation_failed` at the guard and `contained_task_recognition_failed`
// at the `when`, and its lease end hands the key to a ladder that starts at the task stage by
// `pack_logic`, as before. An expected identity (a select step's `choice_of` read back after its
// tap) has no row here: the kernel's `gate_469_d4b` reads it.

use super::instance_workers::{
    admit_ladder_run, all_events, ladder_setup_with_package, phases, wait_for_ladder_end,
};
use super::*;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::LadderClass;
use actingcommand_contract::{
    LeasePayload, RecoveryLadderTrigger, RecoveryTriggerMatch, RecoveryTriggerStage,
    RuntimeLifecyclePhase,
};
use actingcommand_execution_kernel::ExecutionKernelCode;
use actingcommand_recognition_pack::{OcrProviderDetection, OcrProviderTextBlock};
use actingcommand_scheduler::ClaimKind;
use serde_json::json;

const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
// The fake device's physical frame (`FakeState`, 16x9 without an extent of its own): its first
// pixel names the page, red for `home`.
const WIDTH: u32 = 16;
const HEIGHT: u32 = 9;
// The located OCR target the guard or the `when` evaluates, and the text it expects.
const TEXT: &str = "text/name";
const NAME: &str = "NAME";

// What the scripted provider answers to every text read.
#[derive(Debug, Clone, Copy)]
enum Answer {
    // The provider fails.
    Fails,
    // The provider reads a text that is not the name and reports that the screen held more boxes
    // than it kept: the located read cannot conclude, a real evaluation error.
    CrowdedMiss,
}

#[derive(Debug)]
struct ScriptedOcr {
    answer: Answer,
    reads: AtomicU64,
}

impl VisionProvider for ScriptedOcr {
    fn require_ocr_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Ok(())
    }

    fn require_nn_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        self.reads.fetch_add(1, Ordering::AcqRel);
        match self.answer {
            Answer::Fails => Err(VisionProviderError::new(
                VisionProviderErrorCode::Internal,
                "scripted provider failure",
            )),
            Answer::CrowdedMiss => Ok(OcrProviderResult {
                ppocr_diagnostics: Vec::new(),
                text: "OTHER".to_owned(),
                blocks: vec![OcrProviderTextBlock {
                    text: "OTHER".to_owned(),
                    rect: request.region,
                    confidence: Some(0.9),
                }],
                confidence: Some(0.9),
                detection: Some(OcrProviderDetection {
                    detected: 65,
                    limit: 64,
                }),
            }),
        }
    }

    fn classify(
        &self,
        _request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }
}

// Where the run evaluates the name.
#[derive(Debug, Clone, Copy)]
enum Place {
    // The guard of `claim`, a tap on the located name from `home` to `terminal`.
    Guard,
    // The `when` of `open`, a trusted tap from `home` to `terminal`, a candidate while the name is
    // present.
    When,
}

// The package of `place`: from its entry page `home` to its target page `terminal`, one operation.
fn package(place: Place) -> Vec<u8> {
    let operation = match place {
        Place::Guard => json!({
            "id": "claim", "from": "home", "to": "terminal",
            "click": {"kind": "target", "target_id": TEXT},
            "guard": {"page_id": "home", "target_id": TEXT,
                      "expected_rect": {"x": 0, "y": 0, "width": WIDTH, "height": HEIGHT},
                      "verify_text": TEXT},
            "retryable": false
        }),
        Place::When => json!({
            "id": "open", "from": "home", "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "when": {"present": TEXT},
            "retryable": false
        }),
    };
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "entry_page": "home",
        "target_page": "terminal",
        "operations": [operation]
    });
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.d4b.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": HEIGHT},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 2000,
        "timeout_ms": 5000,
        "max_steps": 10
    });
    let colour = |id: &str, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            colour("page/home", [255, 0, 0]),
            colour("page/terminal", [0, 0, 255]),
            {
                "type": "ocr", "id": TEXT,
                "region": {"x": 0, "y": 0, "width": WIDTH, "height": HEIGHT},
                "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact",
                "expected": [NAME], "case_sensitive": false, "minimum_confidence": 0.5,
                "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256,
                "locate": {"normalize": "identity_v1", "blocks": "detect",
                           "line_merge": {"gap_factor": 0}}
            }
        ]
    });
    let pages = json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/home", "required": ["page/home"], "optional": [], "forbidden": []},
            {"id": "neutral/terminal", "required": ["page/terminal"], "optional": [],
             "forbidden": []}
        ]
    });
    let files = [
        ("control.json", control),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", pack),
        ("resources/recognition/neutral.test.pages.json", pages),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, contents) in files {
        zip.start_file(path, options).expect("package entry");
        zip.write_all(&serde_json::to_vec(&contents).expect("package JSON"))
            .expect("package content");
    }
    zip.finish().expect("finish package").into_inner()
}

// One failed scheduled run: its host (still open), the code it ended with, its run and its lease,
// and the root the host runs on (dropped after the host).
struct Scheduled {
    host: RuntimeHost,
    code: String,
    run_id: RunId,
    run_lease: LeaseId,
    _root: TempDir,
}

// The scheduled run of `place`'s package whose provider answers `answer`: it fails, not fatally,
// after the provider was asked.
fn scheduled(place: Place, answer: Answer) -> Scheduled {
    let root = TempDir::new().expect("tempdir");
    let (host_config, request) = ladder_setup_with_package(root.path(), &package(place));
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let vision = Arc::new(ScriptedOcr {
        answer,
        reads: AtomicU64::new(0),
    });
    let host = RuntimeHost::start(
        host_config,
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id(), Arc::clone(&state))
                .with_vision_provider(vision.clone()),
        ),
    )
    .expect("runtime host");
    let context = admit_ladder_run(&host, &request);
    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the scheduled run fails");
    assert!(!error.is_fatal(), "{place:?}, {answer:?}: {}", error.code());
    assert!(
        vision.reads.load(Ordering::Acquire) > 0,
        "{place:?}, {answer:?}: the provider was asked"
    );
    Scheduled {
        host,
        code: error.code().to_owned(),
        run_id: context.run_id(),
        run_lease: context.lease_token().lease_id(),
        _root: root,
    }
}

// Whether the run's lease end handed its key to a ladder: a transfer from the run's lease.
fn handed_to_a_ladder(events: &[PersistedEvent], run_lease: LeaseId) -> bool {
    events.iter().any(|event| {
        matches!(
            event.payload(),
            EventPayload::Lease(LeasePayload::Transferred(transfer))
                if transfer.from_lease_id() == run_lease
        )
    })
}

// The trigger of every ladder that started at the task stage.
fn task_stage_ladders(events: &[PersistedEvent]) -> Vec<RecoveryLadderTrigger> {
    phases(events)
        .into_iter()
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::RecoveryLadderStarted { trigger, .. }
                if trigger.stage == RecoveryTriggerStage::Task =>
            {
                Some(trigger)
            }
            _ => None,
        })
        .collect()
}

// The run's one task terminal, its `task.failed`.
fn terminal_of(events: &[PersistedEvent], run_id: RunId) -> &PersistedEvent {
    let mut terminals = events.iter().filter(|event| {
        event.event_type() == EventType::TaskFailed && event.links().run_id() == Some(&run_id)
    });
    let terminal = terminals.next().expect("the run's task.failed");
    assert!(terminals.next().is_none(), "one task.failed of the run");
    terminal
}

// A provider failure at the guard or at the `when` of a scheduled run: the run ends with the
// provider code, its lease end starts no ladder, and its terminal record is one the default burst
// counts.
#[test]
fn d4b_a_provider_failure_at_a_guard_or_a_when_starts_no_ladder_and_counts_toward_the_burst() {
    let provider = ExecutionKernelCode::ContainedTaskVisionProviderFailed.as_str();
    for place in [Place::Guard, Place::When] {
        let run = scheduled(place, Answer::Fails);
        assert_eq!(run.code, provider, "{place:?}");
        // The decision is made inside the failed lease end, before the run returns.
        let (holder, queued) = run
            .host
            .instance_claims_for_test(POLICY_INSTANCE_ALIAS)
            .expect("instance claims");
        assert_ne!(holder, Some(ClaimKind::Ladder), "{place:?}");
        assert!(!queued.contains(&ClaimKind::Ladder), "{place:?}");
        let events = all_events(&run.host);
        assert!(
            !handed_to_a_ladder(&events, run.run_lease),
            "{place:?}: the run's lease end hands its key to no ladder"
        );
        assert!(
            task_stage_ladders(&events).is_empty(),
            "{place:?}: no ladder starts at the task stage"
        );
        // What direction ① reads of the record: its instance, its severity and its code's class.
        let terminal = terminal_of(&events, run.run_id);
        let code = terminal
            .payload()
            .outcome()
            .map(|outcome| outcome.code().as_str());
        assert_eq!(code, Some(provider), "{place:?}");
        assert!(terminal.links().instance_id().is_some(), "{place:?}");
        assert!(
            matches!(
                terminal.severity(),
                EventSeverity::Warning | EventSeverity::Error | EventSeverity::Fatal
            ),
            "{place:?}"
        );
        assert_eq!(
            code.and_then(actingcommand_runtime_config::ladder_class_of),
            Some(LadderClass::EnvironmentFault),
            "{place:?}"
        );
        assert!(run.host.fatal_error().expect("runtime health").is_none());
        run.host.close().expect("close host");
    }
}

// A real evaluation error at the guard or at the `when` of the same scheduled run ends with the
// place's own class-4 code, and its lease end hands the key to a ladder that starts at the task
// stage by `pack_logic`, matched by the class, as before.
#[test]
fn d4b_a_real_evaluation_error_at_a_guard_or_a_when_still_starts_the_ladder_at_once() {
    for (place, code) in [
        (
            Place::Guard,
            SharedCode::ContainedTaskGuardEvaluationFailed.as_str(),
        ),
        (
            Place::When,
            SharedCode::ContainedTaskRecognitionFailed.as_str(),
        ),
    ] {
        let run = scheduled(place, Answer::CrowdedMiss);
        assert_eq!(run.code, code, "{place:?}");
        wait_for_ladder_end(&run.host, POLICY_INSTANCE_ALIAS);
        let events = all_events(&run.host);
        assert!(
            handed_to_a_ladder(&events, run.run_lease),
            "{place:?}: the run's lease end hands its key to the ladder"
        );
        let ladders = task_stage_ladders(&events);
        let [trigger] = ladders.as_slice() else {
            panic!("{place:?}: one ladder at the task stage: {ladders:?}");
        };
        assert_eq!(trigger.run_id, Some(run.run_id), "{place:?}");
        assert_eq!(trigger.failure_code.as_str(), code, "{place:?}");
        assert_eq!(trigger.rule.as_deref(), Some("pack_logic"), "{place:?}");
        assert_eq!(
            trigger.matched,
            Some(RecoveryTriggerMatch::Class {
                ladder_class: LadderClass::PackLogicMismatch
            }),
            "{place:?}"
        );
        let terminal = terminal_of(&events, run.run_id);
        assert_eq!(
            terminal
                .payload()
                .outcome()
                .map(|outcome| outcome.code().as_str()),
            Some(code),
            "{place:?}"
        );
        assert!(run.host.fatal_error().expect("runtime health").is_none());
        run.host.close().expect("close host");
    }
}
