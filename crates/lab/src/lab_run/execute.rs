// SPDX-License-Identifier: AGPL-3.0-only

fn capture_until_matched_page<L: LedgerSink>(
    ctx: &mut LabRunContext<'_, L>,
    capture: &mut dyn CaptureBackend,
    resources: &LabResources,
    label: &str,
    timeout_ms: u64,
    control: &LabControl,
    candidate_pages: Option<&[String]>,
) -> CliOutcome<CapturedScene> {
    let started = Instant::now();
    loop {
        ctx.wait_for_next_capture_start();
        let scene = ctx.capture_scene_with_pages(
            capture,
            &resources.evaluator,
            &resources.detector,
            label,
            candidate_pages,
        )?;
        validate_frame_resolution(control, scene.width, scene.height)?;
        if scene.matched_page.is_some() {
            return Ok(scene);
        }
        if started.elapsed() >= Duration::from_millis(timeout_ms) {
            return Ok(scene);
        }
    }
}

fn matched_page_matches_anchor(
    game: &str,
    matched_page: Option<&str>,
    expected_anchor: &str,
) -> bool {
    matched_page.is_some_and(|page| page_anchor_matches(game, page, expected_anchor))
}

fn next_current_page(game: &str, after: &CapturedScene, operation: &Operation) -> Option<String> {
    after.matched_anchor(game).or_else(|| {
        operation
            .expected_after_page()
            .map(|page| canonical_page_anchor(game, page))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationVerification {
    Verified,
    ExecutedUnverified,
    Failed,
}

impl OperationVerification {
    fn result_label(self) -> &'static str {
        match self {
            OperationVerification::Verified => "ok",
            OperationVerification::ExecutedUnverified => "executed_unverified",
            OperationVerification::Failed => "failed",
        }
    }
}

fn operation_verification_status(
    game: &str,
    operation: &Operation,
    after: &CapturedScene,
) -> OperationVerification {
    let matched_to = operation
        .expected_after_page()
        .is_some_and(|page| matched_page_matches_anchor(game, after.matched_page.as_deref(), page));
    let matched_template = operation.verify_template.is_some() && after.verify_template_matched;
    if matched_to || matched_template {
        return OperationVerification::Verified;
    }
    if operation.expected_after_page().is_none() && operation.verify_template.is_none() {
        return OperationVerification::ExecutedUnverified;
    }
    OperationVerification::Failed
}

#[derive(Debug, PartialEq)]
enum PreExecutionGuardOutcome {
    Passed {
        current_page: Option<String>,
        target: TargetEvaluation,
    },
    TrustedUnguarded,
    TargetMismatch {
        current_page: Option<String>,
        target: TargetEvaluation,
        diagnostics: Value,
    },
    Failed {
        reason: &'static str,
        current_page: Option<String>,
        diagnostics: Value,
    },
}

fn pre_execution_guard<L: LedgerSink>(
    ctx: &mut LabRunContext<'_, L>,
    capture: &mut dyn CaptureBackend,
    resources: &LabResources,
    operation: &Operation,
    game: &str,
    candidate_pages: Option<&[String]>,
) -> CliOutcome<PreExecutionGuardOutcome> {
    if operation.unguarded_trusted_coordinate {
        return Ok(PreExecutionGuardOutcome::TrustedUnguarded);
    }
    let guard = operation.guard.as_ref().ok_or_else(|| {
        CliError::package_invalid(format!(
            "operation '{}' coordinate action missing guard metadata",
            operation.id
        ))
    })?;
    ctx.event(
        "pre_execution_guard_started",
        json!({"step_id": operation.id, "guard": guard.to_json()}),
    )?;
    ctx.wait_for_next_capture_start();
    let scene = ctx.capture_scene_with_pages(
        capture,
        &resources.evaluator,
        &resources.detector,
        &format!("pre_execution_guard_{}", operation.id),
        candidate_pages,
    )?;
    evaluate_pre_execution_guard(game, operation, guard, &scene, &resources.evaluator)
}

fn evaluate_pre_execution_guard(
    game: &str,
    operation: &Operation,
    guard: &OperationGuard,
    scene: &CapturedScene,
    evaluator: &RecognitionEvaluator,
) -> CliOutcome<PreExecutionGuardOutcome> {
    let current_page = scene.matched_anchor(game);
    if !matched_page_matches_anchor(game, scene.matched_page.as_deref(), &guard.page_id) {
        return Ok(PreExecutionGuardOutcome::Failed {
            reason: "page_guard_mismatch",
            current_page,
            diagnostics: json!({
                "expected_page": guard.page_id,
                "matched_page": scene.matched_page,
                "operation_from": operation.from
            }),
        });
    }
    let target = evaluator
        .evaluate_target(&scene.scene, &guard.target_id)
        .map_err(|err| CliError::device(err.to_string()))?;
    if !target.passed {
        return Ok(PreExecutionGuardOutcome::TargetMismatch {
            current_page,
            target: target.clone(),
            diagnostics: json!({
                "guard": guard.to_json(),
                "target": target_evaluation_json(&target)
            }),
        });
    }
    Ok(PreExecutionGuardOutcome::Passed {
        current_page,
        target,
    })
}

#[derive(Debug, PartialEq)]
enum RoiStabilityOutcome {
    Passed {
        stable_frames: u32,
        observed_frames: u32,
        target: TargetEvaluation,
    },
    Failed {
        reason: &'static str,
        current_page: Option<String>,
        diagnostics: Value,
    },
}

struct RoiStabilityRequest<'a> {
    resources: &'a LabResources,
    operation: &'a Operation,
    game: &'a str,
    baseline_page: Option<String>,
    baseline_target: TargetEvaluation,
    candidate_pages: Option<&'a [String]>,
}

fn wait_for_roi_stability<L: LedgerSink>(
    ctx: &mut LabRunContext<'_, L>,
    capture: &mut dyn CaptureBackend,
    request: RoiStabilityRequest<'_>,
) -> CliOutcome<RoiStabilityOutcome> {
    let guard = request.operation.guard.as_ref().ok_or_else(|| {
        CliError::package_invalid(format!(
            "operation '{}' ROI stability gate missing guard metadata",
            request.operation.id
        ))
    })?;
    let mut gate =
        RoiStabilityGate::new(DEFAULT_ROI_STABLE_FRAMES, request.baseline_target.clone())?;
    ctx.event(
        "roi_stability_gate_started",
        json!({
            "step_id": request.operation.id,
            "required_stable_frames": DEFAULT_ROI_STABLE_FRAMES,
            "timeout_ms": DEFAULT_ROI_STABILITY_TIMEOUT_MS,
            "guard": guard.to_json(),
            "baseline_page": request.baseline_page.as_deref(),
            "baseline_target": target_evaluation_json(&request.baseline_target)
        }),
    )?;

    let started = Instant::now();
    while started.elapsed() <= Duration::from_millis(DEFAULT_ROI_STABILITY_TIMEOUT_MS) {
        ctx.wait_for_next_capture_start();
        let scene = ctx.capture_scene_with_pages(
            capture,
            &request.resources.evaluator,
            &request.resources.detector,
            &format!("roi_stability_{}", request.operation.id),
            request.candidate_pages,
        )?;
        let current_page = scene.matched_anchor(request.game);
        if !matched_page_matches_anchor(request.game, scene.matched_page.as_deref(), &guard.page_id)
        {
            return Ok(RoiStabilityOutcome::Failed {
                reason: "page_guard_mismatch",
                current_page,
                diagnostics: json!({
                    "expected_page": guard.page_id,
                    "matched_page": scene.matched_page,
                    "operation_from": request.operation.from
                }),
            });
        }
        let target = request
            .resources
            .evaluator
            .evaluate_target(&scene.scene, &guard.target_id)
            .map_err(|err| CliError::device(err.to_string()))?;
        if gate.observe(target.clone()) {
            return Ok(RoiStabilityOutcome::Passed {
                stable_frames: gate.stable_frames,
                observed_frames: gate.observed_frames,
                target,
            });
        }
    }

    Ok(RoiStabilityOutcome::Failed {
        reason: "unstable_page",
        current_page: request.baseline_page,
        diagnostics: json!({
            "guard": guard.to_json(),
            "required_stable_frames": DEFAULT_ROI_STABLE_FRAMES,
            "observed_frames": gate.observed_frames,
            "last_target": target_evaluation_json(&gate.last_target),
            "timeout_ms": DEFAULT_ROI_STABILITY_TIMEOUT_MS
        }),
    })
}

#[derive(Debug, PartialEq)]
enum ResourceDriftOutcome {
    Recovered {
        current_page: Option<String>,
        target: TargetEvaluation,
    },
    Failed {
        reason: &'static str,
        current_page: Option<String>,
        diagnostics: Value,
    },
}

struct ResourceDriftRequest<'a> {
    resources: &'a LabResources,
    operation: &'a Operation,
    game: &'a str,
    initial_page: Option<String>,
    initial_target: TargetEvaluation,
    candidate_pages: Option<&'a [String]>,
}

fn confirm_resource_drift<L: LedgerSink>(
    ctx: &mut LabRunContext<'_, L>,
    capture: &mut dyn CaptureBackend,
    request: ResourceDriftRequest<'_>,
) -> CliOutcome<ResourceDriftOutcome> {
    let guard = request.operation.guard.as_ref().ok_or_else(|| {
        CliError::package_invalid(format!(
            "operation '{}' resource drift probe missing guard metadata",
            request.operation.id
        ))
    })?;
    let mut gate = ResourceDriftGate::new(
        DEFAULT_RESOURCE_DRIFT_FRAMES,
        request.initial_target.clone(),
    )?;
    ctx.event(
        "resource_drift_probe_started",
        json!({
            "step_id": request.operation.id,
            "required_mismatch_frames": DEFAULT_RESOURCE_DRIFT_FRAMES,
            "timeout_ms": DEFAULT_ROI_STABILITY_TIMEOUT_MS,
            "guard": guard.to_json(),
            "initial_page": request.initial_page.as_deref(),
            "initial_target": target_evaluation_json(&request.initial_target)
        }),
    )?;

    let started = Instant::now();
    while started.elapsed() <= Duration::from_millis(DEFAULT_ROI_STABILITY_TIMEOUT_MS) {
        ctx.wait_for_next_capture_start();
        let scene = ctx.capture_scene_with_pages(
            capture,
            &request.resources.evaluator,
            &request.resources.detector,
            &format!("resource_drift_{}", request.operation.id),
            request.candidate_pages,
        )?;
        let current_page = scene.matched_anchor(request.game);
        if !matched_page_matches_anchor(request.game, scene.matched_page.as_deref(), &guard.page_id)
        {
            return Ok(ResourceDriftOutcome::Failed {
                reason: "page_guard_mismatch",
                current_page,
                diagnostics: json!({
                    "expected_page": guard.page_id,
                    "matched_page": scene.matched_page,
                    "operation_from": request.operation.from
                }),
            });
        }
        let target = request
            .resources
            .evaluator
            .evaluate_target(&scene.scene, &guard.target_id)
            .map_err(|err| CliError::device(err.to_string()))?;
        match gate.observe(target.clone()) {
            ResourceDriftObservation::Recovered => {
                return Ok(ResourceDriftOutcome::Recovered {
                    current_page,
                    target,
                });
            }
            ResourceDriftObservation::Drift => {
                return Ok(ResourceDriftOutcome::Failed {
                    reason: "resource_drift",
                    current_page,
                    diagnostics: resource_drift_diagnostics(
                        request.operation,
                        guard,
                        &target,
                        gate.observed_frames,
                    ),
                });
            }
            ResourceDriftObservation::Waiting => {}
        }
    }

    Ok(ResourceDriftOutcome::Failed {
        reason: "unstable_page",
        current_page: request.initial_page,
        diagnostics: json!({
            "guard": guard.to_json(),
            "required_mismatch_frames": DEFAULT_RESOURCE_DRIFT_FRAMES,
            "observed_frames": gate.observed_frames,
            "last_target": target_evaluation_json(&gate.last_target),
            "timeout_ms": DEFAULT_ROI_STABILITY_TIMEOUT_MS
        }),
    })
}

#[derive(Debug, PartialEq)]
enum ResourceDriftObservation {
    Recovered,
    Drift,
    Waiting,
}

#[derive(Debug)]
struct ResourceDriftGate {
    required_mismatch_frames: u32,
    stable_mismatch_frames: u32,
    observed_frames: u32,
    last_target: TargetEvaluation,
}

impl ResourceDriftGate {
    fn new(required_mismatch_frames: u32, initial_mismatch: TargetEvaluation) -> CliOutcome<Self> {
        if required_mismatch_frames == 0 {
            return Err(CliError::device(
                "resource drift probe requires at least one mismatch frame",
            ));
        }
        if initial_mismatch.passed {
            return Err(CliError::device(
                "resource drift probe requires an initial target mismatch",
            ));
        }
        Ok(Self {
            required_mismatch_frames,
            stable_mismatch_frames: 1,
            observed_frames: 1,
            last_target: initial_mismatch,
        })
    }

    fn observe(&mut self, target: TargetEvaluation) -> ResourceDriftObservation {
        self.observed_frames += 1;
        if target.passed {
            self.stable_mismatch_frames = 0;
            self.last_target = target;
            return ResourceDriftObservation::Recovered;
        }
        if target_measurement_stable_with(&self.last_target, &target) {
            self.stable_mismatch_frames += 1;
        } else {
            self.stable_mismatch_frames = 1;
        }
        self.last_target = target;
        if self.stable_mismatch_frames >= self.required_mismatch_frames {
            ResourceDriftObservation::Drift
        } else {
            ResourceDriftObservation::Waiting
        }
    }
}

#[derive(Debug)]
struct RoiStabilityGate {
    required_stable_frames: u32,
    stable_frames: u32,
    observed_frames: u32,
    last_target: TargetEvaluation,
}

impl RoiStabilityGate {
    fn new(required_stable_frames: u32, baseline: TargetEvaluation) -> CliOutcome<Self> {
        if required_stable_frames == 0 {
            return Err(CliError::device(
                "ROI stability gate requires at least one stable frame",
            ));
        }
        if !baseline.passed {
            return Err(CliError::device(
                "ROI stability baseline target did not pass guard evaluation",
            ));
        }
        Ok(Self {
            required_stable_frames,
            stable_frames: 1,
            observed_frames: 1,
            last_target: baseline,
        })
    }

    fn observe(&mut self, target: TargetEvaluation) -> bool {
        self.observed_frames += 1;
        if !target.passed {
            self.stable_frames = 0;
            self.last_target = target;
            return false;
        }
        if target_stable_with(&self.last_target, &target) {
            self.stable_frames += 1;
        } else {
            self.stable_frames = 1;
        }
        self.last_target = target;
        self.stable_frames >= self.required_stable_frames
    }
}

fn target_stable_with(previous: &TargetEvaluation, current: &TargetEvaluation) -> bool {
    previous.passed && current.passed && target_measurement_stable_with(previous, current)
}

pub fn target_evaluations_stable_for_wait(
    previous: &TargetEvaluation,
    current: &TargetEvaluation,
) -> bool {
    target_stable_with(previous, current)
}

fn target_measurement_stable_with(previous: &TargetEvaluation, current: &TargetEvaluation) -> bool {
    if previous.id != current.id || previous.kind != current.kind {
        return false;
    }
    if !template_evaluation_stable(previous, current) {
        return false;
    }
    color_evaluation_stable(previous, current)
}

fn template_evaluation_stable(previous: &TargetEvaluation, current: &TargetEvaluation) -> bool {
    match (previous.template, current.template) {
        (Some(previous), Some(current)) => {
            (previous.x - current.x).abs() <= ROI_TEMPLATE_POSITION_EPSILON
                && (previous.y - current.y).abs() <= ROI_TEMPLATE_POSITION_EPSILON
                && (previous.score - current.score).abs() <= ROI_TEMPLATE_SCORE_EPSILON
        }
        (None, None) => true,
        _ => false,
    }
}

fn color_evaluation_stable(previous: &TargetEvaluation, current: &TargetEvaluation) -> bool {
    match (previous.color, current.color) {
        (Some(previous), Some(current)) => {
            let mean_stable = previous
                .mean
                .iter()
                .zip(current.mean.iter())
                .all(|(previous, current)| previous.abs_diff(*current) <= ROI_COLOR_MEAN_EPSILON);
            mean_stable
                && (previous.distance - current.distance).abs() <= ROI_COLOR_DISTANCE_EPSILON
        }
        (None, None) => true,
        _ => false,
    }
}

fn resource_drift_diagnostics(
    operation: &Operation,
    guard: &OperationGuard,
    target: &TargetEvaluation,
    observed_frames: u32,
) -> Value {
    json!({
        "trigger": "resource_drift",
        "resource_status": "needs_recalibration",
        "resource_action": "mark_for_recalibration",
        "target_id": guard.target_id.as_str(),
        "expected_rect": rect_json(guard.expected_rect),
        "measured": target_evaluation_json(target),
        "observed_frames": observed_frames,
        "required_mismatch_frames": DEFAULT_RESOURCE_DRIFT_FRAMES,
        "provenance_version": operation_provenance_version(operation),
        "provenance": operation.provenance.clone().unwrap_or(Value::Null),
        "guard": guard.to_json()
    })
}

fn operation_provenance_version(operation: &Operation) -> Value {
    operation
        .provenance
        .as_ref()
        .and_then(|provenance| {
            provenance
                .get("version")
                .or_else(|| provenance.get("resource_version"))
                .or_else(|| provenance.get("pack_version"))
                .or_else(|| provenance.get("source_commit"))
                .or_else(|| provenance.get("commit"))
        })
        .cloned()
        .unwrap_or(Value::Null)
}

fn target_evaluation_json(target: &TargetEvaluation) -> Value {
    json!({
        "id": target.id.as_str(),
        "kind": format!("{:?}", target.kind),
        "passed": target.passed,
        "message": target.message.as_str(),
        "matched_rect": target.template.map(|template| rect_json(PackRect {
            x: template.x,
            y: template.y,
            width: template.width,
            height: template.height
        })),
        "template": target.template.map(|template| json!({
            "x": template.x,
            "y": template.y,
            "width": template.width,
            "height": template.height,
            "raw_score": template.raw_score,
            "score": template.score,
            "threshold": template.threshold
        })),
        "color": target.color.map(|color| json!({
            "distance": color.distance,
            "max_distance": color.max_distance,
            "mean": color.mean,
            "expected": color.expected
        }))
    })
}

fn unsupported_targets_json(targets: &[UnsupportedRecognitionTarget]) -> Vec<Value> {
    targets
        .iter()
        .map(|target| {
            json!({
                "id": target.id.as_str(),
                "reason": target.reason.as_str()
            })
        })
        .collect()
}

fn actionable_page_ids(resources: &LabResources, control: &LabControl) -> CliOutcome<Vec<String>> {
    actionable_page_ids_for_bundle(resources, control, &resources.operation_bundle)
}

fn actionable_page_ids_for_bundle(
    resources: &LabResources,
    control: &LabControl,
    bundle: &OperationBundle,
) -> CliOutcome<Vec<String>> {
    let mut pages = Vec::new();
    let mut seen = BTreeSet::new();
    if let Some(entry_page) = &bundle.entry_page
        && entry_page != "any"
    {
        push_resolved_page_id(&mut pages, &mut seen, resources, &control.game, entry_page)?;
    }
    if let Some(target_page) = &bundle.target_page {
        push_resolved_page_id(&mut pages, &mut seen, resources, &control.game, target_page)?;
    }
    for page_key in bundle.page_rules.keys() {
        // Selected task packages may retain source page_rules for pages whose
        // recognition assets were intentionally not packaged.
        if let Ok(page) = resolve_detector_page_id(resources, &control.game, page_key)
            && seen.insert(page.clone())
        {
            pages.push(page);
        }
    }
    for operation in &bundle.operations {
        if operation.from != "any" {
            push_resolved_page_id(
                &mut pages,
                &mut seen,
                resources,
                &control.game,
                &operation.from,
            )?;
        }
        if let Some(to) = &operation.to {
            push_resolved_page_id(&mut pages, &mut seen, resources, &control.game, to)?;
        }
    }
    Ok(pages)
}

fn initial_page_ids(resources: &LabResources, control: &LabControl) -> CliOutcome<Vec<String>> {
    initial_page_ids_for_bundle(resources, control, &resources.operation_bundle)
}

fn initial_page_ids_for_bundle(
    resources: &LabResources,
    control: &LabControl,
    bundle: &OperationBundle,
) -> CliOutcome<Vec<String>> {
    let mut pages = Vec::new();
    let mut seen = BTreeSet::new();
    if let Some(entry_page) = &bundle.entry_page
        && entry_page != "any"
    {
        push_resolved_page_id(&mut pages, &mut seen, resources, &control.game, entry_page)?;
    }
    if let Some(target_page) = &bundle.target_page {
        push_resolved_page_id(&mut pages, &mut seen, resources, &control.game, target_page)?;
    }
    if pages.is_empty() {
        return actionable_page_ids_for_bundle(resources, control, bundle);
    }
    Ok(pages)
}

fn operation_arrival_page_ids(
    resources: &LabResources,
    game: &str,
    operation: &Operation,
) -> CliOutcome<Option<Vec<String>>> {
    operation
        .expected_after_page()
        .map(|to| resolve_detector_page_id(resources, game, to).map(|page| vec![page]))
        .transpose()
}

fn resolve_detector_page_id(
    resources: &LabResources,
    game: &str,
    anchor: &str,
) -> CliOutcome<String> {
    let namespaced = format!("{game}/{anchor}");
    if resources.detector.contains_page(&namespaced) {
        return Ok(namespaced);
    }
    if resources.detector.contains_page(anchor) {
        return Ok(anchor.to_string());
    }
    Err(CliError::package_invalid(format!(
        "operation page anchor '{anchor}' does not resolve to a detector page id"
    )))
}

fn push_resolved_page_id(
    pages: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
    resources: &LabResources,
    game: &str,
    anchor: &str,
) -> CliOutcome<()> {
    let page = resolve_detector_page_id(resources, game, anchor)?;
    if seen.insert(page.clone()) {
        pages.push(page);
    }
    Ok(())
}

fn close_backend_after_error<T>(
    backend: &mut Option<Box<dyn InputBackend>>,
    err: CliError,
) -> CliOutcome<T> {
    if let Some(mut backend) = backend.take() {
        let close = backend.close();
        if let Err(close_err) = close {
            return Err(CliError::device(format!(
                "{}; touch backend close also failed: {}",
                err.message, close_err
            )));
        }
    }
    Err(err)
}

fn page_is_error_page(game: &str, page: Option<&str>, error_pages: &[String]) -> bool {
    let Some(page) = page else {
        return false;
    };
    error_pages
        .iter()
        .any(|expected| page_anchor_matches(game, page, expected))
        || page.contains("/negative_")
        || page.contains("/forbidden")
        || page.starts_with("negative_")
        || page.starts_with("forbidden")
}

fn scene_hits_error_page(game: &str, scene: &CapturedScene, error_pages: &[String]) -> bool {
    if page_is_error_page(game, scene.matched_page.as_deref(), error_pages) {
        return true;
    }
    scene.page_evaluations.iter().any(|evaluation| {
        evaluation.target_results.iter().any(|target| {
            target.role == PageTargetRole::Forbidden
                && target.passed
                && target_is_error_signal(game, &target.target_id, error_pages)
        })
    })
}

fn target_is_error_signal(game: &str, target_id: &str, error_pages: &[String]) -> bool {
    let anchor = target_id
        .strip_prefix("page/")
        .or_else(|| target_id.strip_prefix(&format!("{game}/")))
        .unwrap_or(target_id);
    anchor.starts_with("negative_")
        || anchor.starts_with("forbidden")
        || error_pages
            .iter()
            .any(|error_page| page_anchor_matches(game, anchor, error_page))
}

#[derive(Clone, Copy)]
struct DeviceInputRequest<'a> {
    instance_alias: &'a str,
    factory: &'a dyn InputBackendFactory,
    config: &'a TouchBackendConfig,
}

struct OperationExecutionRequest<'a> {
    device: DeviceInputRequest<'a>,
    resources: &'a LabResources,
    bundle: &'a OperationBundle,
    control: &'a LabControl,
    operation: &'a Operation,
    current_page: &'a str,
    step_index: usize,
    step_timeout_ms: u64,
    candidate_pages: Option<&'a [String]>,
}

enum OperationRunOutcome {
    Success { current_page: Option<String> },
    NeedsRecovery(RunRecoveryTrigger),
}

fn operation_recovery_task_id(
    bundle: &OperationBundle,
    operation: &Operation,
    implicit_return_home_available: bool,
) -> Option<String> {
    bundle
        .recovery
        .as_ref()
        .map(|recovery| recovery.task_id().to_string())
        .or_else(|| {
            operation
                .on_error
                .as_ref()
                .map(|_| DEFAULT_RECOVERY_TASK_ID.to_string())
        })
        .or_else(|| implicit_return_home_available.then(|| DEFAULT_RECOVERY_TASK_ID.to_string()))
}

fn run_operation_policy(
    flow: OperationFlowPolicy,
    recovery_task_id: Option<String>,
) -> CliOutcome<RunOperationPolicy> {
    RunOperationPolicy::new(
        flow.retryable,
        flow.max_attempts,
        flow.retry_interval_ms,
        recovery_task_id,
    )
    .map_err(run_decision_error)
}

fn run_failure_observation(
    operation_id: &str,
    attempt: u32,
    reason: &str,
    after_page: Option<String>,
    stage: RunFailureStage,
) -> CliOutcome<RunFailureObservation> {
    RunFailureObservation::new(operation_id, attempt, reason, after_page, stage)
        .map_err(run_decision_error)
}

fn run_decision_error(error: RunDecisionError) -> CliError {
    if error.code() == "run_decision_invalid" {
        CliError::package_invalid(error.to_string())
    } else {
        CliError::device(error.to_string())
    }
}

fn execute_operation_with_retries<L: LedgerSink>(
    ctx: &mut LabRunContext<'_, L>,
    capture: &mut dyn CaptureBackend,
    input: &mut Option<Box<dyn InputBackend>>,
    request: OperationExecutionRequest<'_>,
) -> CliOutcome<OperationRunOutcome> {
    let OperationExecutionRequest {
        device,
        resources,
        bundle,
        control,
        operation,
        current_page,
        step_index,
        step_timeout_ms,
        candidate_pages,
    } = request;
    let flow = operation.flow_policy(bundle.defaults);
    let recovery_task_id = operation_recovery_task_id(
        bundle,
        operation,
        resources.has_operation_bundle(DEFAULT_RECOVERY_TASK_ID)?,
    );
    let run_policy = run_operation_policy(flow, recovery_task_id)?;
    ctx.set_step_context(step_index, operation);
    ctx.event(
        "step_started",
        json!({"step_id": operation.id, "index": step_index, "operation_id": operation.id, "max_attempts": flow.max_attempts, "retryable": flow.retryable}),
    )?;
    ctx.event(
        "before_page_detected",
        json!({"step_id": operation.id, "page": current_page}),
    )?;

    for attempt in 1..=flow.max_attempts {
        ctx.event(
            "operation_attempt_started",
            json!({
                "step_id": operation.id,
                "attempt": attempt,
                "max_attempts": flow.max_attempts,
                "retryable": flow.retryable,
                "flow": flow.to_json()
            }),
        )?;
        ctx.sleep_ms(flow.pre_delay_ms);
        if flow.pre_wait_freezes_ms > 0 {
            ctx.event(
                "operation_pre_wait_freezes",
                json!({"step_id": operation.id, "attempt": attempt, "duration_ms": flow.pre_wait_freezes_ms}),
            )?;
            ctx.sleep_ms(flow.pre_wait_freezes_ms);
        }

        let stability_baseline = match pre_execution_guard(
            ctx,
            capture,
            resources,
            operation,
            &control.game,
            candidate_pages,
        )? {
            PreExecutionGuardOutcome::Passed {
                current_page,
                target,
            } => {
                ctx.event(
                    "pre_execution_guard_passed",
                    json!({"step_id": operation.id, "attempt": attempt, "page": current_page, "target": target_evaluation_json(&target)}),
                )?;
                Some((current_page, target))
            }
            PreExecutionGuardOutcome::TrustedUnguarded => {
                ctx.event(
                    "pre_execution_guard_skipped",
                    json!({"step_id": operation.id, "attempt": attempt, "reason": "unguarded_trusted_coordinate"}),
                )?;
                None
            }
            PreExecutionGuardOutcome::TargetMismatch {
                current_page,
                target,
                diagnostics,
            } => {
                ctx.event(
                    "pre_execution_guard_failed",
                    json!({"step_id": operation.id, "attempt": attempt, "reason": "target_guard_mismatch", "current_page": current_page.as_deref(), "diagnostics": diagnostics}),
                )?;
                match confirm_resource_drift(
                    ctx,
                    capture,
                    ResourceDriftRequest {
                        resources,
                        operation,
                        game: &control.game,
                        initial_page: current_page,
                        initial_target: target,
                        candidate_pages,
                    },
                )? {
                    ResourceDriftOutcome::Recovered {
                        current_page,
                        target,
                    } => {
                        ctx.event(
                            "pre_execution_guard_passed",
                            json!({"step_id": operation.id, "attempt": attempt, "page": current_page.as_deref(), "target": target_evaluation_json(&target), "after": "target_guard_mismatch_recovered"}),
                        )?;
                        Some((current_page, target))
                    }
                    ResourceDriftOutcome::Failed {
                        reason,
                        current_page,
                        diagnostics,
                    } => {
                        if reason == "resource_drift" {
                            ctx.event(
                                "resource_drift_detected",
                                json!({"step_id": operation.id, "attempt": attempt, "current_page": current_page.as_deref(), "diagnostics": diagnostics}),
                            )?;
                        } else {
                            ctx.event(
                                "pre_execution_guard_failed",
                                json!({"step_id": operation.id, "attempt": attempt, "reason": reason, "current_page": current_page.as_deref(), "diagnostics": diagnostics}),
                            )?;
                        }
                        ctx.event(
                            "step_failed",
                            json!({"step_id": operation.id, "reason": reason, "attempt": attempt}),
                        )?;
                        return Err(CliError::device(format!(
                            "pre-execution guard failed for operation '{}': {reason}; current_page={}",
                            operation.id,
                            current_page.unwrap_or_else(|| "unknown".to_string())
                        )));
                    }
                }
            }
            PreExecutionGuardOutcome::Failed {
                reason,
                current_page,
                diagnostics,
            } => {
                ctx.event(
                    "pre_execution_guard_failed",
                    json!({"step_id": operation.id, "attempt": attempt, "reason": reason, "current_page": current_page, "diagnostics": diagnostics}),
                )?;
                let observation = run_failure_observation(
                    &operation.id,
                    attempt,
                    reason,
                    current_page.clone(),
                    RunFailureStage::PreExecutionGuard,
                )?;
                match decide_run_operation_failure(&run_policy, observation)
                    .map_err(run_decision_error)?
                {
                    RunOperationFailureDecision::RequestRecovery(trigger) => {
                        ctx.event(
                            "operation_recovery_required",
                            json!({"step_id": operation.id, "attempts": trigger.attempts, "reason": trigger.reason.as_str(), "after_page": trigger.after_page.as_deref()}),
                        )?;
                        ctx.clear_step_context();
                        return Ok(OperationRunOutcome::NeedsRecovery(trigger));
                    }
                    RunOperationFailureDecision::Fail(_) => {
                        ctx.event(
                            "step_failed",
                            json!({"step_id": operation.id, "reason": "pre_execution_guard_failed", "attempt": attempt}),
                        )?;
                        return Err(CliError::device(format!(
                            "pre-execution guard failed for operation '{}': {reason}; current_page={}",
                            operation.id,
                            current_page.unwrap_or_else(|| "unknown".to_string())
                        )));
                    }
                    RunOperationFailureDecision::Retry { .. } => {
                        return Err(CliError::device(format!(
                            "execution-kernel returned retry for pre-execution guard failure in operation '{}'",
                            operation.id
                        )));
                    }
                }
            }
        };

        let mut action_target = None;
        if let Some((current_page, target)) = stability_baseline {
            match wait_for_roi_stability(
                ctx,
                capture,
                RoiStabilityRequest {
                    resources,
                    operation,
                    game: &control.game,
                    baseline_page: current_page,
                    baseline_target: target,
                    candidate_pages,
                },
            )? {
                RoiStabilityOutcome::Passed {
                    stable_frames,
                    observed_frames,
                    target,
                } => {
                    ctx.event(
                        "roi_stability_gate_passed",
                        json!({
                            "step_id": operation.id,
                            "attempt": attempt,
                            "stable_frames": stable_frames,
                            "observed_frames": observed_frames,
                            "target": target_evaluation_json(&target)
                        }),
                    )?;
                    action_target = Some(target);
                }
                RoiStabilityOutcome::Failed {
                    reason,
                    current_page,
                    diagnostics,
                } => {
                    ctx.event(
                        "roi_stability_gate_failed",
                        json!({"step_id": operation.id, "attempt": attempt, "reason": reason, "current_page": current_page, "diagnostics": diagnostics}),
                    )?;
                    ctx.event(
                        "step_failed",
                        json!({"step_id": operation.id, "reason": reason, "attempt": attempt}),
                    )?;
                    return Err(CliError::device(format!(
                        "ROI stability gate failed for operation '{}': {reason}; current_page={}",
                        operation.id,
                        current_page.unwrap_or_else(|| "unknown".to_string())
                    )));
                }
            }
        }

        let action = operation.admitted_input_action(&resources.package, action_target.as_ref())?;
        let action_id = ctx.id_issuer.issue(IdKind::Action).value;
        let backend = ensure_touch_backend(
            input,
            device.instance_alias,
            device.factory,
            device.config,
        )?;
        match &action {
            LabInputAction::Tap(point) => {
                let action_started = Instant::now();
                ctx.event(
                    "click_started",
                    json!({"step_id": operation.id, "attempt": attempt, "action_id": action_id.as_str(), "actual_click_point": point.to_json()}),
                )?;
                if let Err(err) = backend.tap(point.x, point.y) {
                    return close_backend_after_error(input, CliError::device(err.to_string()));
                }
                ctx.event(
                    "click_finished",
                    json!({"step_id": operation.id, "attempt": attempt, "action_id": action_id.as_str(), "actual_click_point": point.to_json()}),
                )?;
                ctx.action_durations_ms
                    .push(action_started.elapsed().as_millis() as u64);
            }
            LabInputAction::Drag {
                from,
                to,
                duration_ms,
            } => {
                let action_started = Instant::now();
                ctx.event(
                    "drag_started",
                    json!({"step_id": operation.id, "attempt": attempt, "action_id": action_id.as_str(), "from": from.to_json(), "to": to.to_json(), "duration_ms": duration_ms}),
                )?;
                if let Err(err) = backend.swipe(from.x, from.y, to.x, to.y, *duration_ms) {
                    return close_backend_after_error(input, CliError::device(err.to_string()));
                }
                ctx.event(
                    "drag_finished",
                    json!({"step_id": operation.id, "attempt": attempt, "action_id": action_id.as_str(), "from": from.to_json(), "to": to.to_json(), "duration_ms": duration_ms}),
                )?;
                ctx.action_durations_ms
                    .push(action_started.elapsed().as_millis() as u64);
            }
            LabInputAction::LongTap { point, duration_ms } => {
                let action_started = Instant::now();
                ctx.event(
                    "long_tap_started",
                    json!({"step_id": operation.id, "attempt": attempt, "action_id": action_id.as_str(), "actual_click_point": point.to_json(), "duration_ms": duration_ms}),
                )?;
                if let Err(err) = backend.long_tap(point.x, point.y, *duration_ms) {
                    return close_backend_after_error(input, CliError::device(err.to_string()));
                }
                ctx.event(
                    "long_tap_finished",
                    json!({"step_id": operation.id, "attempt": attempt, "action_id": action_id.as_str(), "actual_click_point": point.to_json(), "duration_ms": duration_ms}),
                )?;
                ctx.action_durations_ms
                    .push(action_started.elapsed().as_millis() as u64);
            }
        }

        ctx.sleep_ms(flow.post_delay_ms);
        ctx.event(
            "page_guard_started",
            json!({"step_id": operation.id, "attempt": attempt, "to": operation.to, "expect_after": operation.expect_after.as_ref().map(OperationExpectation::to_json), "verify_template": operation.verify_template}),
        )?;
        let after_result = poll_after_operation(
            ctx,
            capture,
            AfterOperationRequest {
                resources,
                task_id: &bundle.task_id,
                defaults: bundle.defaults,
                operation,
                step_timeout_ms: operation.after_timeout_ms(bundle.defaults, step_timeout_ms),
                post_wait_freezes_ms: flow.post_wait_freezes_ms,
                game: &control.game,
            },
        )?;
        let after = after_result.scene;
        let verification = operation_verification_status(&control.game, operation, &after);
        if verification == OperationVerification::Failed || !after_result.stable_confirmed {
            let after_page = after.matched_page.clone();
            let hit_error_page = scene_hits_error_page(&control.game, &after, &bundle.error_pages);
            let failure_reason = if !after_result.stable_confirmed {
                "after_page_not_stable"
            } else {
                "page_confirmation_failed"
            };
            ctx.event(
                "page_guard_failed",
                json!({"step_id": operation.id, "attempt": attempt, "expected": operation.expected_after_page(), "after_page": after_page, "error_page": hit_error_page, "reason": failure_reason}),
            )?;
            let decision_reason = if hit_error_page {
                "error_page"
            } else {
                failure_reason
            };
            let observation = run_failure_observation(
                &operation.id,
                attempt,
                decision_reason,
                after_page.clone(),
                RunFailureStage::PostExecution { hit_error_page },
            )?;
            match decide_run_operation_failure(&run_policy, observation)
                .map_err(run_decision_error)?
            {
                RunOperationFailureDecision::Retry {
                    next_attempt,
                    delay_ms,
                } => {
                    ctx.event(
                        "operation_retry_scheduled",
                        json!({"step_id": operation.id, "attempt": attempt, "next_attempt": next_attempt, "reason": failure_reason, "retry_interval_ms": delay_ms, "after_page": after_page}),
                    )?;
                    ctx.sleep_ms(delay_ms);
                    continue;
                }
                RunOperationFailureDecision::RequestRecovery(trigger) => {
                    ctx.event(
                        "operation_recovery_required",
                        json!({"step_id": operation.id, "attempts": trigger.attempts, "reason": trigger.reason.as_str(), "after_page": trigger.after_page.as_deref()}),
                    )?;
                    ctx.clear_step_context();
                    return Ok(OperationRunOutcome::NeedsRecovery(trigger));
                }
                RunOperationFailureDecision::Fail(_) => {
                    ctx.event(
                        "step_failed",
                        json!({"step_id": operation.id, "reason": "page_confirmation_failed", "attempts": attempt}),
                    )?;
                    return Err(CliError::device(format!(
                        "page confirmation failed for operation '{}' after {attempt} attempt(s)",
                        operation.id
                    )));
                }
            }
        }
        let guard_event = match verification {
            OperationVerification::Verified => "page_guard_passed",
            OperationVerification::ExecutedUnverified => "page_guard_unverified",
            OperationVerification::Failed => unreachable!("failed verification returned earlier"),
        };
        ctx.event(
            guard_event,
            json!({"step_id": operation.id, "attempt": attempt, "after_page": after.matched_page}),
        )?;
        ctx.event(
            "after_page_detected",
            json!({"step_id": operation.id, "attempt": attempt, "page": after.matched_page, "anchor": after.matched_anchor(&control.game)}),
        )?;

        let step_record = json!({
            "id": operation.id,
            "action_id": action_id.as_str(),
            "operation_id": operation.id,
            "purpose": operation.purpose,
            "from": operation.from,
            "to": operation.to,
            "expect_after": operation.expect_after.as_ref().map(OperationExpectation::to_json),
            "before_page": current_page,
            "after_page": after.matched_page,
            "after_anchor": after.matched_anchor(&control.game),
            "attempt_count": attempt,
            "retryable": flow.retryable,
            "flow": flow.to_json(),
            "click_count": if matches!(action, LabInputAction::Tap(_)) { 1 } else { 0 },
            "drag_count": if matches!(action, LabInputAction::Drag { .. }) { 1 } else { 0 },
            "long_tap_count": if matches!(action, LabInputAction::LongTap { .. }) { 1 } else { 0 },
            "actual_input": action.to_json(),
            "consumes": operation.consumes,
            "produces": operation.produces,
            "verified_live": operation.verified_live,
            "provenance": operation.provenance,
            "guard": operation.guard.as_ref().map(OperationGuard::to_json),
            "unguarded_trusted_coordinate": operation.unguarded_trusted_coordinate,
            "result": verification.result_label()
        });
        ctx.append_step_record(step_record, &action_id)?;
        ctx.event(
            "operation_attempt_finished",
            json!({"step_id": operation.id, "attempt": attempt, "result": verification.result_label()}),
        )?;
        ctx.event(
            "step_finished",
            json!({"step_id": operation.id, "action_id": action_id, "attempt_count": attempt, "result": verification.result_label()}),
        )?;
        let current_page = next_current_page(&control.game, &after, operation);
        ctx.clear_step_context();
        return Ok(OperationRunOutcome::Success { current_page });
    }

    unreachable!("operation attempt loop has at least one iteration")
}

fn capture_backend_attempt_json(attempt: &CaptureBackendAttempt) -> Value {
    json!({
        "backend": attempt.backend.as_str(),
        "ok": attempt.ok,
        "severity": if attempt.ok { "info" } else { "warning" },
        "elapsed_ms": attempt.elapsed_ms,
        "cached": attempt.cached,
        "channel_order_contract": attempt.channel_order_contract,
        "message": attempt.message.as_str(),
        "vendor_stdio": &attempt.vendor_stdio
    })
}

fn ensure_touch_backend<'a>(
    backend: &'a mut Option<Box<dyn InputBackend>>,
    instance_alias: &str,
    factory: &dyn InputBackendFactory,
    config: &TouchBackendConfig,
) -> CliOutcome<&'a mut Box<dyn InputBackend>> {
    if backend.is_none() {
        let created = factory.open(InputBackendRequest {
            instance_alias: Some(instance_alias.to_string()),
            config: config.clone(),
            observation: None,
        })?;
        *backend = Some(created);
    }
    backend
        .as_mut()
        .ok_or_else(|| CliError::device("failed to initialize touch backend"))
}

struct AfterOperationCapture {
    scene: CapturedScene,
    stable_confirmed: bool,
}

struct AfterOperationRequest<'a> {
    resources: &'a LabResources,
    task_id: &'a str,
    defaults: OperationDefaults,
    operation: &'a Operation,
    step_timeout_ms: u64,
    post_wait_freezes_ms: u64,
    game: &'a str,
}

fn poll_after_operation<L: LedgerSink>(
    ctx: &mut LabRunContext<'_, L>,
    capture: &mut dyn CaptureBackend,
    request: AfterOperationRequest<'_>,
) -> CliOutcome<AfterOperationCapture> {
    let AfterOperationRequest {
        resources,
        task_id,
        defaults,
        operation,
        step_timeout_ms,
        post_wait_freezes_ms,
        game,
    } = request;
    let started = Instant::now();
    let mut verified_since = None::<Instant>;
    let arrival_page_candidates = operation_arrival_page_ids(resources, game, operation)?;
    loop {
        ctx.wait_for_next_capture_start();
        let mut scene = ctx.capture_scene_with_pages(
            capture,
            &resources.evaluator,
            &resources.detector,
            &operation.id,
            arrival_page_candidates.as_deref(),
        )?;
        if let Some(template) = &operation.verify_template {
            scene.verify_template_matched = verify_template(
                &scene.scene,
                resources,
                task_id,
                template,
                defaults.template_threshold,
            )?;
        }
        let verification = operation_verification_status(game, operation, &scene);
        if verification == OperationVerification::ExecutedUnverified {
            return Ok(AfterOperationCapture {
                scene,
                stable_confirmed: true,
            });
        }
        if verification == OperationVerification::Verified {
            let since = *verified_since.get_or_insert_with(Instant::now);
            if post_wait_freezes_ms == 0
                || since.elapsed() >= Duration::from_millis(post_wait_freezes_ms)
            {
                return Ok(AfterOperationCapture {
                    scene,
                    stable_confirmed: true,
                });
            }
        } else {
            verified_since = None;
        }
        if started.elapsed() >= Duration::from_millis(step_timeout_ms) {
            return Ok(AfterOperationCapture {
                scene,
                stable_confirmed: false,
            });
        }
    }
}

fn verify_template(
    scene: &Scene,
    resources: &LabResources,
    task_id: &str,
    template: &str,
    threshold: f32,
) -> CliOutcome<bool> {
    let bytes = resources.operation_asset_for_task(task_id, template)?;
    let matched = scene
        .match_template(bytes, None)
        .map_err(|err| CliError::device(err.to_string()))?;
    Ok(matched.score >= threshold)
}
