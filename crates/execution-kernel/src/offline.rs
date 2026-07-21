// SPDX-License-Identifier: AGPL-3.0-only

//! Zero-device adapter for evaluating contained tasks against recorded frames.

use crate::{
    ContainedTaskGuardOutcome, ContainedTaskRunError, ContainedTaskRuntime, ContainedTaskTrace,
    PreparedContainedTask,
};
use actingcommand_contract::InputAction;
use actingcommand_device::Frame;
use actingcommand_pack_containment::Sha256Hash;
use serde::Serialize;
use std::collections::VecDeque;
use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OfflineRecognitionResult {
    pub candidate_pages: Vec<String>,
    pub matched_page: Option<String>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum OfflineDecision {
    WouldClick {
        operation_label: String,
        action: InputAction,
        guard: ContainedTaskGuardOutcome,
    },
    WouldComplete {
        final_page: Option<String>,
    },
    NoOp {
        final_page: Option<String>,
    },
    Refused {
        code: String,
        detail: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OfflineSimulationResult {
    pub mode: &'static str,
    pub executed: bool,
    pub package_id: String,
    pub package_sha256: String,
    pub semantic_fingerprint: String,
    pub decision_fingerprint: String,
    pub task_id: String,
    pub entry_count: usize,
    pub task_count: usize,
    pub capture_count: usize,
    pub recognition: Vec<OfflineRecognitionResult>,
    pub decision: OfflineDecision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflineSimulationError {
    code: String,
    detail: Option<String>,
}

impl OfflineSimulationError {
    fn new(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            detail: None,
        }
    }

    fn with_detail(code: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            detail: Some(detail.into()),
        }
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

impl fmt::Display for OfflineSimulationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "offline simulation error {}", self.code)?;
        if let Some(detail) = &self.detail {
            write!(formatter, ": {detail}")?;
        }
        Ok(())
    }
}

impl Error for OfflineSimulationError {}

/// Runs the production contained-task interpreter while intercepting its first effect intent.
/// No device backend can be injected through this boundary, and `input` never performs an effect.
pub fn simulate_contained_task(
    task: &PreparedContainedTask,
    frames: Vec<Frame>,
) -> Result<OfflineSimulationResult, OfflineSimulationError> {
    let mut runtime = OfflineRuntime::new(frames);
    let decision = if runtime.frames.is_empty() {
        OfflineDecision::Refused {
            code: "offline_fixture_missing".to_string(),
            detail: None,
        }
    } else {
        match task.run(&mut runtime) {
            Ok(outcome)
                if outcome.executed_steps == 0 && task.execution_mode() == "recognize_only" =>
            {
                OfflineDecision::NoOp {
                    final_page: outcome.final_page,
                }
            }
            Ok(outcome) if outcome.executed_steps == 0 => OfflineDecision::WouldComplete {
                final_page: outcome.final_page,
            },
            Ok(_) => {
                return Err(OfflineSimulationError::new(
                    "offline_simulation_executed_step_invariant",
                ));
            }
            Err(ContainedTaskRunError::Boundary(OfflineBoundary::EffectIntercepted)) => runtime
                .planned
                .take()
                .map(|planned| OfflineDecision::WouldClick {
                    operation_label: planned.operation_label,
                    action: planned.action,
                    guard: planned.guard,
                })
                .ok_or_else(|| {
                    OfflineSimulationError::new("offline_simulation_effect_intent_missing")
                })?,
            Err(ContainedTaskRunError::Boundary(OfflineBoundary::FixtureExhausted)) => {
                let code = if runtime
                    .recognition
                    .last()
                    .is_some_and(|result| result.matched_page.is_none())
                {
                    "contained_task_page_unknown"
                } else {
                    "offline_fixture_exhausted"
                };
                OfflineDecision::Refused {
                    code: code.to_string(),
                    detail: None,
                }
            }
            Err(ContainedTaskRunError::Boundary(OfflineBoundary::Invariant(code))) => {
                return Err(OfflineSimulationError::new(code));
            }
            Err(ContainedTaskRunError::Task(error)) => OfflineDecision::Refused {
                code: error.code().to_string(),
                detail: error.detail().map(str::to_string),
            },
        }
    };

    let semantic_fingerprint = task.semantic_fingerprint().to_string();
    let decision_fingerprint =
        fingerprint_decision(&semantic_fingerprint, &runtime.recognition, &decision)?;
    Ok(OfflineSimulationResult {
        mode: "offline_simulation",
        executed: false,
        package_id: task.package_label().to_string(),
        package_sha256: task.package_sha256().to_string(),
        semantic_fingerprint,
        decision_fingerprint,
        task_id: task.task_label().to_string(),
        entry_count: task.entry_count(),
        task_count: task.task_count(),
        capture_count: runtime.capture_count,
        recognition: runtime.recognition,
        decision,
    })
}

#[derive(Serialize)]
struct DecisionFingerprintProjection<'a> {
    schema_version: &'static str,
    semantic_fingerprint: &'a str,
    recognition: &'a [OfflineRecognitionResult],
    decision: &'a OfflineDecision,
}

fn fingerprint_decision(
    semantic_fingerprint: &str,
    recognition: &[OfflineRecognitionResult],
    decision: &OfflineDecision,
) -> Result<String, OfflineSimulationError> {
    const DOMAIN: &[u8] = b"ActingCommand first decision v1\0";
    let encoded = serde_json::to_vec(&DecisionFingerprintProjection {
        schema_version: "actingcommand.first-decision.v1",
        semantic_fingerprint,
        recognition,
        decision,
    })
    .map_err(|error| {
        OfflineSimulationError::with_detail(
            "offline_decision_fingerprint_failed",
            error.to_string(),
        )
    })?;
    let mut material = Vec::with_capacity(DOMAIN.len() + 8 + encoded.len());
    material.extend_from_slice(DOMAIN);
    material.extend_from_slice(&(encoded.len() as u64).to_be_bytes());
    material.extend_from_slice(&encoded);
    Ok(Sha256Hash::digest(&material).to_string())
}

struct PlannedEffect {
    operation_label: String,
    action: InputAction,
    guard: ContainedTaskGuardOutcome,
}

struct OfflineRuntime {
    frames: VecDeque<Frame>,
    capture_count: usize,
    recognition: Vec<OfflineRecognitionResult>,
    planned: Option<PlannedEffect>,
}

impl OfflineRuntime {
    fn new(frames: Vec<Frame>) -> Self {
        Self {
            frames: frames.into(),
            capture_count: 0,
            recognition: Vec::new(),
            planned: None,
        }
    }
}

enum OfflineBoundary {
    EffectIntercepted,
    FixtureExhausted,
    Invariant(&'static str),
}

impl ContainedTaskRuntime for OfflineRuntime {
    type Error = OfflineBoundary;

    fn capture(&mut self) -> Result<Frame, Self::Error> {
        let frame = self
            .frames
            .pop_front()
            .ok_or(OfflineBoundary::FixtureExhausted)?;
        self.capture_count += 1;
        Ok(frame)
    }

    fn input(&mut self, action: InputAction) -> Result<(), Self::Error> {
        let Some(planned) = &self.planned else {
            return Err(OfflineBoundary::Invariant(
                "offline_simulation_effect_intent_missing",
            ));
        };
        if planned.action != action {
            return Err(OfflineBoundary::Invariant(
                "offline_simulation_action_mismatch",
            ));
        }
        Err(OfflineBoundary::EffectIntercepted)
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        match trace {
            ContainedTaskTrace::RecognitionCompleted {
                candidate_pages,
                page_label,
                width,
                height,
            } => self.recognition.push(OfflineRecognitionResult {
                candidate_pages,
                matched_page: page_label,
                width,
                height,
            }),
            ContainedTaskTrace::EffectIntent {
                operation_label,
                action,
                guard,
                ..
            } => {
                if self.planned.is_some() {
                    return Err(OfflineBoundary::Invariant(
                        "offline_simulation_multiple_effects",
                    ));
                }
                self.planned = Some(PlannedEffect {
                    operation_label,
                    action,
                    guard,
                });
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExternalExpectedSha256, PreparedContainedTask};
    use actingcommand_device::{CaptureBackendName, PixelFormat};
    use actingcommand_pack_containment::Sha256Hash;
    use serde_json::{Value, json};
    use std::io::{Cursor, Write};
    use zip::{ZipWriter, write::FileOptions};

    #[test]
    fn offline_and_effecting_runtime_share_the_first_decision() {
        let package_bytes = package(PackageOptions::default());
        let task = prepare(&package_bytes);
        let offline =
            simulate_contained_task(&task, vec![home_frame(true)]).expect("offline simulation");
        let offline_action = match &offline.decision {
            OfflineDecision::WouldClick { action, .. } => action.clone(),
            other => panic!("expected would-click, got {other:?}"),
        };
        let mut effecting = EffectingRuntime::new(vec![home_frame(true), terminal_frame()]);

        let outcome = task.run(&mut effecting).expect("effecting execution");

        assert_eq!(outcome.executed_steps, 1);
        assert_eq!(effecting.actions, vec![offline_action]);
        let (recognition, decision) = effecting
            .first_decision
            .expect("effecting first-decision trace");
        assert_eq!(
            fingerprint_decision(task.semantic_fingerprint(), &recognition, &decision)
                .expect("effecting decision fingerprint"),
            offline.decision_fingerprint
        );
    }

    #[test]
    fn equivalent_page_spelling_and_json_array_order_keep_decision_identity() {
        let canonical = package(PackageOptions::default());
        let variant = package(PackageOptions {
            unqualified_page_ids: true,
            reverse_page_order: true,
            ..PackageOptions::default()
        });

        let canonical = simulate_contained_task(&prepare(&canonical), vec![home_frame(true)])
            .expect("canonical decision");
        let variant = simulate_contained_task(&prepare(&variant), vec![home_frame(true)])
            .expect("equivalent decision");

        assert_eq!(canonical.semantic_fingerprint, variant.semantic_fingerprint);
        assert_eq!(canonical.decision_fingerprint, variant.decision_fingerprint);
        assert_eq!(canonical.decision, variant.decision);
        assert_eq!(canonical.recognition, variant.recognition);
    }

    #[test]
    fn simulation_reports_complete_and_legal_no_op_without_input() {
        let navigable = package(PackageOptions::default());
        let completed = simulate_contained_task(&prepare(&navigable), vec![terminal_frame()])
            .expect("complete simulation");
        assert!(matches!(
            completed.decision,
            OfflineDecision::WouldComplete { .. }
        ));

        let recognize_only = package(PackageOptions {
            execution_mode: "recognize_only",
            ..PackageOptions::default()
        });
        let no_op = simulate_contained_task(&prepare(&recognize_only), vec![home_frame(true)])
            .expect("no-op simulation");
        assert!(matches!(no_op.decision, OfflineDecision::NoOp { .. }));
    }

    #[test]
    fn simulation_receipts_bind_typed_refusals_to_decision_fingerprints() {
        let package_bytes = package(PackageOptions::default());
        let task = prepare(&package_bytes);
        let unknown_refusal =
            simulate_contained_task(&task, vec![solid_frame([0, 0, 0], [0, 0, 0])])
                .expect("unknown-page refusal receipt");
        assert_refusal(&unknown_refusal, "contained_task_page_unknown");
        let guard_refusal =
            simulate_contained_task(&task, vec![home_frame(false)]).expect("guard refusal receipt");
        assert_refusal(&guard_refusal, "contained_task_guard_refused");
        let guard_replay = simulate_contained_task(&task, vec![home_frame(false)])
            .expect("guard refusal replay receipt");
        assert_eq!(
            guard_refusal.decision_fingerprint,
            guard_replay.decision_fingerprint
        );
        let wrong_size = Frame::from_pixels(
            1,
            1,
            vec![255, 0, 0],
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
        .expect("frame");
        let resolution_refusal =
            simulate_contained_task(&task, vec![wrong_size]).expect("resolution refusal receipt");
        assert_refusal(
            &resolution_refusal,
            "contained_task_frame_resolution_mismatch",
        );

        let conflict = package(PackageOptions {
            conflicting_page: true,
            ..PackageOptions::default()
        });
        let conflict_refusal = simulate_contained_task(&prepare(&conflict), vec![home_frame(true)])
            .expect("conflict refusal receipt");
        assert_refusal(&conflict_refusal, "contained_task_recognition_conflict");
        let distinct_refusal_fingerprints = [
            &unknown_refusal.decision_fingerprint,
            &guard_refusal.decision_fingerprint,
            &resolution_refusal.decision_fingerprint,
            &conflict_refusal.decision_fingerprint,
        ]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            distinct_refusal_fingerprints.len(),
            4,
            "semantically distinct refusals must not share a decision fingerprint"
        );
    }

    fn assert_refusal(result: &OfflineSimulationResult, expected: &str) {
        assert!(matches!(
            &result.decision,
            OfflineDecision::Refused { code, .. } if code == expected
        ));
        assert_eq!(result.decision_fingerprint.len(), 64);
        assert!(
            result
                .decision_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
    }

    #[test]
    fn admission_rejects_missing_guard_and_recovery_task() {
        let missing_guard = package(PackageOptions {
            include_guard: false,
            ..PackageOptions::default()
        });
        assert_eq!(
            prepare_error(&missing_guard).code(),
            "contained_task_guard_missing"
        );

        let missing_recovery = package(PackageOptions {
            recovery: true,
            ..PackageOptions::default()
        });
        assert_eq!(
            prepare_error(&missing_recovery).code(),
            "contained_task_recovery_missing"
        );
    }

    #[test]
    fn bounded_admitted_observation_fuzz_is_panic_free_and_deterministic() {
        const CASES: usize = 256;
        let package_bytes = package(PackageOptions::default());
        let task = prepare(&package_bytes);
        let mut state = 0x68_0ff1_1e55_cafe_u64;
        let wrong_size = Frame::from_pixels(
            1,
            1,
            vec![255, 0, 0],
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
        .expect("retained wrong-size frame");
        let retained_corpus = [
            ("would-click", home_frame(true)),
            ("guard-refusal", home_frame(false)),
            ("would-complete", terminal_frame()),
            ("unknown-page", solid_frame([0, 0, 0], [0, 0, 0])),
            ("wrong-resolution", wrong_size),
        ];
        for (label, frame) in retained_corpus {
            let first = simulate_contained_task(&task, vec![frame.clone()])
                .unwrap_or_else(|error| panic!("retained observation {label}: {error}"));
            let second = simulate_contained_task(&task, vec![frame])
                .unwrap_or_else(|error| panic!("retained observation replay {label}: {error}"));
            assert_eq!(first, second, "retained observation {label} drifted");
            match label {
                "would-click" => {
                    assert!(matches!(first.decision, OfflineDecision::WouldClick { .. }))
                }
                "guard-refusal" => assert!(matches!(
                    first.decision,
                    OfflineDecision::Refused { ref code, .. }
                        if code == "contained_task_guard_refused"
                )),
                "would-complete" => {
                    assert!(matches!(
                        first.decision,
                        OfflineDecision::WouldComplete { .. }
                    ))
                }
                "unknown-page" => assert!(matches!(
                    first.decision,
                    OfflineDecision::Refused { ref code, .. }
                        if code == "contained_task_page_unknown"
                )),
                "wrong-resolution" => assert!(matches!(
                    first.decision,
                    OfflineDecision::Refused { ref code, .. }
                        if code == "contained_task_frame_resolution_mismatch"
                )),
                _ => unreachable!("retained observation label"),
            }
        }

        let started = std::time::Instant::now();
        for case in 0..CASES {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let pixels = state.to_le_bytes()[..6].to_vec();
            let frame = Frame::from_pixels(
                2,
                1,
                pixels,
                PixelFormat::Rgb8,
                CaptureBackendName::AdbScreencap,
            )
            .expect("bounded fuzz frame");
            let evaluate = || match simulate_contained_task(&task, vec![frame.clone()]) {
                Ok(result) => format!("decision:{}", result.decision_fingerprint),
                Err(error) => format!("error:{}:{:?}", error.code(), error.detail()),
            };
            let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(evaluate))
                .unwrap_or_else(|_| panic!("admitted observation case {case} panicked"));
            let second = std::panic::catch_unwind(std::panic::AssertUnwindSafe(evaluate))
                .unwrap_or_else(|_| panic!("admitted observation replay case {case} panicked"));
            assert_eq!(first, second, "admitted observation case {case} drifted");
        }
        assert!(
            started.elapsed() <= std::time::Duration::from_secs(10),
            "bounded admitted-observation fuzz exceeded its 10 second budget"
        );
    }

    #[derive(Clone, Copy)]
    struct PackageOptions {
        execution_mode: &'static str,
        include_guard: bool,
        conflicting_page: bool,
        recovery: bool,
        unqualified_page_ids: bool,
        reverse_page_order: bool,
    }

    impl Default for PackageOptions {
        fn default() -> Self {
            Self {
                execution_mode: "navigable_route",
                include_guard: true,
                conflicting_page: false,
                recovery: false,
                unqualified_page_ids: false,
                reverse_page_order: false,
            }
        }
    }

    fn package(options: PackageOptions) -> Vec<u8> {
        let control = json!({
            "schema_version": "Lab-1y.control.v1",
            "package_id": "neutral.semantic.task",
            "execution_mode": options.execution_mode,
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 2, "height": 1},
            "entry_task_id": "task",
            "capture_interval_ms": 1,
            "step_timeout_ms": 10,
            "timeout_ms": 100,
            "max_steps": 2
        });
        let mut operation = json!({
            "id": "open_terminal",
            "from": "home",
            "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0}
        });
        if options.include_guard {
            operation["guard"] = json!({
                "page_id": "home",
                "target_id": "guard/ready",
                "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                "color_probe": "guard/ready"
            });
        }
        let mut task = json!({
            "schema_version": "0.6",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "coordinate_space": {"width": 2, "height": 1},
            "entry_page": "home",
            "target_page": "terminal",
            "operations": [operation]
        });
        if options.recovery {
            task["recovery"] = json!({"kind": "return_home", "task_id": "return_home"});
        }
        let mut pages = vec![
            json!({"id":if options.unqualified_page_ids { "home" } else { "neutral/home" },"required":["page/home"],"optional":[],"forbidden":[]}),
            json!({"id":if options.unqualified_page_ids { "terminal" } else { "neutral/terminal" },"required":["page/terminal"],"optional":[],"forbidden":[]}),
        ];
        if options.conflicting_page {
            pages.push(json!({"id":if options.unqualified_page_ids { "duplicate" } else { "neutral/duplicate" },"required":["page/home"],"optional":[],"forbidden":[]}));
        }
        if options.reverse_page_order {
            pages.reverse();
        }
        zip_entries(&[
            ("control.json", control),
            (
                "resources/manifest.json",
                json!({"schema_version":"0.3","entry_task_id":"task"}),
            ),
            ("resources/operations/task/task.json", task),
            (
                "resources/recognition/neutral.test.pack.json",
                json!({
                    "schema_version": "0.3",
                    "game": "neutral",
                    "server": "test",
                    "coordinate_space": {"width": 2, "height": 1},
                    "defaults": {"color_max_distance": 0.0},
                    "targets": [
                        {"type":"color","id":"page/home","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                        {"type":"color","id":"page/terminal","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]},
                        {"type":"color","id":"guard/ready","region":{"x":1,"y":0,"width":1,"height":1},"expected":[0,255,0]}
                    ]
                }),
            ),
            (
                "resources/recognition/neutral.test.pages.json",
                json!({"schema_version":"0.3","pages":pages}),
            ),
            (
                "resources/navigation/neutral.test.navigation.json",
                json!({
                    "schema_version":"0.3",
                    "game":"neutral",
                    "server":"test",
                    "navigation":[{
                        "id":"open_terminal",
                        "from_page":"home",
                        "to_page":"terminal",
                        "click":{"kind":"point","x":1,"y":0}
                    }],
                    "destructive_actions":[]
                }),
            ),
        ])
    }

    fn zip_entries(entries: &[(&str, Value)]) -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (path, value) in entries {
            zip.start_file(*path, options).expect("zip entry");
            serde_json::to_writer(&mut zip, value).expect("zip JSON");
            zip.write_all(b"\n").expect("zip newline");
        }
        zip.finish().expect("finish zip").into_inner()
    }

    fn prepare(bytes: &[u8]) -> PreparedContainedTask {
        let expected = Sha256Hash::digest(bytes).to_string();
        PreparedContainedTask::load(
            "neutral.instance",
            bytes,
            ExternalExpectedSha256::parse_hex(&expected).expect("hash"),
        )
        .expect("prepared task")
    }

    fn prepare_error(bytes: &[u8]) -> crate::ContainedTaskError {
        let expected = Sha256Hash::digest(bytes).to_string();
        PreparedContainedTask::load(
            "neutral.instance",
            bytes,
            ExternalExpectedSha256::parse_hex(&expected).expect("hash"),
        )
        .err()
        .expect("admission failure")
    }

    fn home_frame(guard_passes: bool) -> Frame {
        solid_frame(
            [255, 0, 0],
            if guard_passes { [0, 255, 0] } else { [0, 0, 0] },
        )
    }

    fn terminal_frame() -> Frame {
        solid_frame([0, 0, 255], [0, 0, 0])
    }

    fn solid_frame(left: [u8; 3], right: [u8; 3]) -> Frame {
        Frame::from_pixels(
            2,
            1,
            [left, right].concat(),
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
        .expect("frame")
    }

    struct EffectingRuntime {
        frames: VecDeque<Frame>,
        actions: Vec<InputAction>,
        recognition: Vec<OfflineRecognitionResult>,
        first_decision: Option<(Vec<OfflineRecognitionResult>, OfflineDecision)>,
    }

    impl EffectingRuntime {
        fn new(frames: Vec<Frame>) -> Self {
            Self {
                frames: frames.into(),
                actions: Vec::new(),
                recognition: Vec::new(),
                first_decision: None,
            }
        }
    }

    impl ContainedTaskRuntime for EffectingRuntime {
        type Error = &'static str;

        fn capture(&mut self) -> Result<Frame, Self::Error> {
            self.frames.pop_front().ok_or("fixture exhausted")
        }

        fn input(&mut self, action: InputAction) -> Result<(), Self::Error> {
            self.actions.push(action);
            Ok(())
        }

        fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
            match trace {
                ContainedTaskTrace::RecognitionCompleted {
                    candidate_pages,
                    page_label,
                    width,
                    height,
                } => self.recognition.push(OfflineRecognitionResult {
                    candidate_pages,
                    matched_page: page_label,
                    width,
                    height,
                }),
                ContainedTaskTrace::EffectIntent {
                    operation_label,
                    action,
                    guard,
                    ..
                } if self.first_decision.is_none() => {
                    self.first_decision = Some((
                        self.recognition.clone(),
                        OfflineDecision::WouldClick {
                            operation_label,
                            action,
                            guard,
                        },
                    ));
                }
                _ => {}
            }
            Ok(())
        }
    }
}
