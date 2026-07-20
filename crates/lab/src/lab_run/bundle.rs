// SPDX-License-Identifier: AGPL-3.0-only

fn lab_control_from_admitted(package: &AdmittedPackage) -> CliOutcome<LabControl> {
    let control = package.control();
    let resolution = control.resolution();
    let frame_store = control.frame_store();
    Ok(LabControl {
        package_id: control.package_id().to_string(),
        execution_mode: control.execution_mode().as_str().to_string(),
        game: control.game().to_string(),
        server: control.server().to_string(),
        resolution: Resolution {
            width: resolution.width(),
            height: resolution.height(),
        },
        entry_task_id: control.entry_task().as_str().to_string(),
        capture_interval_ms: Some(control.capture_interval_ms()),
        timeout_ms: Some(control.timeout_ms()),
        step_timeout_ms: Some(control.step_timeout_ms()),
        max_steps: Some(control.max_steps() as usize),
        stop_on_error: control.stop_on_error(),
        stop_on_confirmation: Some(control.stop_on_confirmation()),
        output: control
            .output()
            .map(opaque_metadata_value)
            .transpose()?,
        capture_backend: control.capture_backend().map(str::to_string),
        frame_store: FrameStoreControl {
            similarity_threshold: frame_store.similarity_threshold(),
            tier1_ratio: frame_store.tier1_ratio(),
            tier2_ratio: frame_store.tier2_ratio(),
            tier3_ratio: frame_store.tier3_ratio(),
            hysteresis_ratio: frame_store.hysteresis_ratio(),
            max_mem_bytes: frame_store.max_mem_bytes(),
            os_reserve_bytes: frame_store.os_reserve_bytes(),
            flush_workspace_reserve_bytes: frame_store.flush_workspace_reserve_bytes(),
        },
        producer: control.producer_present().then(|| json!({"present": true})),
        trusted_execution: control
            .trusted_execution_present()
            .then(|| json!({"present": true})),
    })
}

fn opaque_metadata_value(metadata: &OpaqueMetadata) -> CliOutcome<Value> {
    serde_json::to_value(metadata).map_err(|error| {
        CliError::package_invalid(format!(
            "failed to project admitted package metadata: {error}"
        ))
    })
}

fn load_lab_resources_from_admitted(
    package: AdmittedPackage,
    control: &LabControl,
) -> CliOutcome<LabResources> {
    let operation_bundle = operation_bundle_from_admitted(package.entry_task())?;
    let stem = format!("{}.{}", control.game, control.server);
    let navigation_loaded = package.navigation().is_some();
    let manifest = json!({
        "entry_task_id": control.entry_task_id,
        "package_id": control.package_id,
        "admission": "closed"
    });
    let evaluator = package.evaluator().clone();
    let detector = package.detector().clone();

    Ok(LabResources {
        package,
        resource_root: PathBuf::from("resources"),
        manifest_path: PathBuf::from("resources/manifest.json"),
        manifest,
        operation_path: PathBuf::from(format!(
            "resources/operations/{}/task.json",
            control.entry_task_id
        )),
        operation_bundle,
        pack_path: PathBuf::from(format!("resources/recognition/{stem}.pack.json")),
        pages_path: PathBuf::from(format!("resources/recognition/{stem}.pages.json")),
        evaluator,
        detector,
        navigation_path: navigation_loaded.then(|| {
            PathBuf::from(format!("resources/navigation/{stem}.navigation.json"))
        }),
        navigation_loaded,
    })
}

fn operation_bundle_from_admitted(task: &AdmittedTask) -> CliOutcome<OperationBundle> {
    let defaults = task.defaults();
    let operations = task
        .operations()
        .iter()
        .map(operation_from_admitted)
        .collect::<CliOutcome<Vec<_>>>()?;
    Ok(OperationBundle {
        task_id: task.key().as_str().to_string(),
        goal: task.goal().to_string(),
        defaults: OperationDefaults {
            template_threshold: defaults.template_threshold(),
            color_max_distance: defaults.color_max_distance(),
            timeout_ms: defaults.timeout_ms(),
            max_attempts: defaults.max_attempts(),
            retry_interval_ms: defaults.retry_interval_ms(),
            pre_delay_ms: defaults.pre_delay_ms(),
            post_delay_ms: defaults.post_delay_ms(),
            pre_wait_freezes_ms: defaults.pre_wait_freezes_ms(),
            post_wait_freezes_ms: defaults.post_wait_freezes_ms(),
        },
        entry_page: task.entry_page().map(PageKey::qualified),
        target_page: task.target_page().map(PageKey::qualified),
        error_pages: task.error_pages().iter().map(PageKey::qualified).collect(),
        recovery: task
            .recovery()
            .map(|task| TaskRecovery(task.as_str().to_string())),
        max_task_retries: task.max_task_retries(),
        page_rules: BTreeMap::new(),
        operations,
    })
}

fn operation_from_admitted(operation: &AdmittedOperation) -> CliOutcome<Operation> {
    Ok(Operation {
        id: operation.key().operation().to_string(),
        purpose: operation.purpose().to_string(),
        from: page_selector_text(operation.from()),
        to: operation.to().map(PageKey::qualified),
        click: operation_click_from_admitted(operation.action()),
        verify_template: operation
            .verify_template()
            .map(|asset| asset.as_str().to_string()),
        expect_after: operation.expect_after().map(|expectation| OperationExpectation {
            page_id: expectation.page().qualified(),
            timeout_ms: expectation.timeout_ms(),
            interval_ms: expectation.interval_ms(),
        }),
        timeout_ms: operation.timeout_ms(),
        max_attempts: operation.max_attempts(),
        retry_interval_ms: operation.retry_interval_ms(),
        pre_delay_ms: operation.pre_delay_ms(),
        post_delay_ms: operation.post_delay_ms(),
        pre_wait_freezes_ms: operation.pre_wait_freezes_ms(),
        post_wait_freezes_ms: operation.post_wait_freezes_ms(),
        retryable: operation.retryable(),
        effect: operation.navigation_only().then(|| "navigation_only".to_string()),
        on_error: operation
            .on_error()
            .map(|task| task.as_str().to_string()),
        guard: operation.guard().map(operation_guard_from_admitted),
        unguarded_trusted_coordinate: operation.unguarded_trusted_coordinate(),
        consumes: operation.consumes().to_vec(),
        produces: operation.produces().to_vec(),
        verified_live: operation.verified_live(),
        provenance: operation
            .provenance()
            .map(opaque_metadata_value)
            .transpose()?,
    })
}

fn page_selector_text(selector: &PageSelector) -> String {
    match selector {
        PageSelector::Any => "any".to_string(),
        PageSelector::Exact(page) => page.qualified(),
    }
}

fn operation_guard_from_admitted(guard: &AdmittedGuard) -> OperationGuard {
    let (verify_template, color_probe) = match guard.verification() {
        GuardVerification::Template { asset } => (Some(asset.as_str().to_string()), None),
        GuardVerification::Color { probe } => (None, Some(probe.as_str().to_string())),
    };
    OperationGuard {
        page_id: guard.page().qualified(),
        target_id: guard.target().as_str().to_string(),
        expected_rect: pack_rect(guard.expected_rect()),
        verify_template,
        color_probe,
    }
}

fn operation_click_from_admitted(action: &AdmittedAction) -> OperationClick {
    match action {
        AdmittedAction::Tap { point, .. } => OperationClick {
            kind: "point".to_string(),
            x: Some(point.x()),
            y: Some(point.y()),
            width: None,
            height: None,
            from_rect: None,
            to_rect: None,
            duration_ms: None,
            offset: None,
        },
        AdmittedAction::LongTap { point, duration } => OperationClick {
            kind: "long_tap".to_string(),
            x: Some(point.x()),
            y: Some(point.y()),
            width: None,
            height: None,
            from_rect: None,
            to_rect: None,
            duration_ms: Some(duration.milliseconds()),
            offset: None,
        },
        AdmittedAction::Drag {
            from, to, duration, ..
        } => OperationClick {
            kind: "drag".to_string(),
            x: None,
            y: None,
            width: None,
            height: None,
            from_rect: Some(PackRect {
                x: from.x(),
                y: from.y(),
                width: 1,
                height: 1,
            }),
            to_rect: Some(PackRect {
                x: to.x(),
                y: to.y(),
                width: 1,
                height: 1,
            }),
            duration_ms: Some(duration.milliseconds()),
            offset: None,
        },
        AdmittedAction::TargetTap {
            target: _,
            mode,
            offset,
        } => OperationClick {
            kind: match mode {
                TargetTapMode::Deterministic => "target",
                TargetTapMode::Center => "target_center",
            }
            .to_string(),
            x: None,
            y: None,
            width: None,
            height: None,
            from_rect: None,
            to_rect: None,
            duration_ms: None,
            offset: offset.map(|offset| PackRect {
                x: offset.x(),
                y: offset.y(),
                width: offset.width(),
                height: offset.height(),
            }),
        },
    }
}

fn pack_rect(rect: BoundedRect) -> PackRect {
    PackRect {
        x: rect.x(),
        y: rect.y(),
        width: rect.width(),
        height: rect.height(),
    }
}

#[derive(Debug, Clone)]
struct LabControl {
    package_id: String,
    execution_mode: String,
    game: String,
    server: String,
    resolution: Resolution,
    entry_task_id: String,
    capture_interval_ms: Option<u64>,
    timeout_ms: Option<u64>,
    step_timeout_ms: Option<u64>,
    max_steps: Option<usize>,
    stop_on_error: Option<bool>,
    stop_on_confirmation: Option<bool>,
    output: Option<Value>,
    capture_backend: Option<String>,
    frame_store: FrameStoreControl,
    producer: Option<Value>,
    trusted_execution: Option<Value>,
}

impl LabControl {
    fn capture_backend_choice(&self) -> CliOutcome<Option<CaptureBackendChoice>> {
        self.capture_backend
            .as_deref()
            .map(CaptureBackendChoice::parse)
            .transpose()
            .map_err(|err| CliError::package_invalid(err.to_string()))
    }
}

#[derive(Debug, Clone, Copy)]
struct Resolution {
    width: u32,
    height: u32,
}

#[derive(Debug)]
struct LabResources {
    package: AdmittedPackage,
    resource_root: PathBuf,
    manifest_path: PathBuf,
    manifest: Value,
    operation_path: PathBuf,
    operation_bundle: OperationBundle,
    pack_path: PathBuf,
    pages_path: PathBuf,
    evaluator: RecognitionEvaluator,
    detector: PageDetector,
    navigation_path: Option<PathBuf>,
    navigation_loaded: bool,
}

impl LabResources {
    fn has_operation_bundle(&self, task_id: &str) -> CliOutcome<bool> {
        Ok(self
            .package
            .tasks()
            .any(|task| task.key().as_str() == task_id))
    }

    fn operation_asset_for_task(&self, task_id: &str, relative: &str) -> CliOutcome<&[u8]> {
        let local = format!("operations/{task_id}/{relative}");
        let key = self
            .package
            .assets()
            .map(|(key, _)| key)
            .find(|key| key.as_str() == relative || key.as_str() == local)
            .ok_or_else(|| {
                CliError::package_invalid(format!(
                    "admitted operation asset '{relative}' for task '{task_id}' is unavailable"
                ))
            })?;
        self.package.asset_bytes(key).ok_or_else(|| {
            CliError::package_invalid(format!(
                "admitted operation asset bytes for '{}' are unavailable",
                key.as_str()
            ))
        })
    }

}

#[derive(Debug)]
struct RunState {
    control: LabControl,
    resources: LabResources,
    current_page: Option<String>,
    failed_step_id: Option<String>,
}

#[derive(Debug, Clone)]
struct OperationBundle {
    task_id: String,
    goal: String,
    defaults: OperationDefaults,
    entry_page: Option<String>,
    target_page: Option<String>,
    error_pages: Vec<String>,
    recovery: Option<TaskRecovery>,
    max_task_retries: Option<u32>,
    page_rules: BTreeMap<String, Value>,
    operations: Vec<Operation>,
}

#[derive(Debug, Clone)]
struct TaskRecovery(String);

impl TaskRecovery {
    fn task_id(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy)]
struct OperationDefaults {
    template_threshold: f32,
    color_max_distance: Option<f32>,
    timeout_ms: Option<u64>,
    max_attempts: Option<u32>,
    retry_interval_ms: Option<u64>,
    pre_delay_ms: Option<u64>,
    post_delay_ms: Option<u64>,
    pre_wait_freezes_ms: Option<u64>,
    post_wait_freezes_ms: Option<u64>,
}

impl Default for OperationDefaults {
    fn default() -> Self {
        Self {
            template_threshold: DEFAULT_TEMPLATE_THRESHOLD,
            color_max_distance: None,
            timeout_ms: None,
            max_attempts: None,
            retry_interval_ms: None,
            pre_delay_ms: None,
            post_delay_ms: None,
            pre_wait_freezes_ms: None,
            post_wait_freezes_ms: None,
        }
    }
}

impl OperationDefaults {
    fn to_json(self) -> Value {
        json!({
            "template_threshold": self.template_threshold,
            "color_max_distance": self.color_max_distance,
            "timeout_ms": self.timeout_ms,
            "max_attempts": self.max_attempts,
            "retry_interval_ms": self.retry_interval_ms,
            "pre_delay_ms": self.pre_delay_ms,
            "post_delay_ms": self.post_delay_ms,
            "pre_wait_freezes_ms": self.pre_wait_freezes_ms,
            "post_wait_freezes_ms": self.post_wait_freezes_ms
        })
    }
}

#[derive(Debug, Clone)]
struct Operation {
    id: String,
    purpose: String,
    from: String,
    to: Option<String>,
    click: OperationClick,
    verify_template: Option<String>,
    expect_after: Option<OperationExpectation>,
    timeout_ms: Option<u64>,
    max_attempts: Option<u32>,
    retry_interval_ms: Option<u64>,
    pre_delay_ms: Option<u64>,
    post_delay_ms: Option<u64>,
    pre_wait_freezes_ms: Option<u64>,
    post_wait_freezes_ms: Option<u64>,
    retryable: Option<bool>,
    effect: Option<String>,
    on_error: Option<String>,
    guard: Option<OperationGuard>,
    unguarded_trusted_coordinate: bool,
    consumes: Vec<String>,
    produces: Vec<String>,
    verified_live: Option<bool>,
    provenance: Option<Value>,
}

impl Operation {
    fn input_action(
        &self,
        resolution: &Resolution,
        seed_base: u64,
        target: Option<&TargetEvaluation>,
    ) -> CliOutcome<LabInputAction> {
        self.click.input_action(
            resolution,
            seed_base ^ hash_text(&self.id),
            self.guard.as_ref(),
            target,
        )
    }

    fn expected_after_page(&self) -> Option<&str> {
        self.expect_after
            .as_ref()
            .map(|expectation| expectation.page_id.as_str())
            .or(self.to.as_deref())
    }

    fn after_timeout_ms(&self, defaults: OperationDefaults, default_timeout_ms: u64) -> u64 {
        self.timeout_ms
            .or(defaults.timeout_ms)
            .or_else(|| {
                self.expect_after
                    .as_ref()
                    .and_then(|expectation| expectation.timeout_ms)
            })
            .unwrap_or(default_timeout_ms)
    }

    fn flow_policy(&self, defaults: OperationDefaults) -> OperationFlowPolicy {
        let retryable = self.retryable.unwrap_or_else(|| self.is_navigation_only());
        let requested_attempts = self
            .max_attempts
            .or(defaults.max_attempts)
            .unwrap_or(if retryable { 3 } else { 1 });
        OperationFlowPolicy {
            retryable,
            max_attempts: if retryable {
                requested_attempts.max(1)
            } else {
                1
            },
            retry_interval_ms: self
                .retry_interval_ms
                .or(defaults.retry_interval_ms)
                .unwrap_or(DEFAULT_RETRY_INTERVAL_MS),
            pre_delay_ms: self.pre_delay_ms.or(defaults.pre_delay_ms).unwrap_or(0),
            post_delay_ms: self.post_delay_ms.or(defaults.post_delay_ms).unwrap_or(0),
            pre_wait_freezes_ms: self
                .pre_wait_freezes_ms
                .or(defaults.pre_wait_freezes_ms)
                .unwrap_or(0),
            post_wait_freezes_ms: self
                .post_wait_freezes_ms
                .or(defaults.post_wait_freezes_ms)
                .unwrap_or(DEFAULT_POST_WAIT_FREEZES_MS),
        }
    }

    fn is_navigation_only(&self) -> bool {
        self.effect.as_deref() == Some("navigation_only")
            || (self
                .to
                .as_deref()
                .is_some_and(|page| !page.trim().is_empty())
                && self.consumes.is_empty()
                && self.produces.is_empty())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OperationFlowPolicy {
    retryable: bool,
    max_attempts: u32,
    retry_interval_ms: u64,
    pre_delay_ms: u64,
    post_delay_ms: u64,
    pre_wait_freezes_ms: u64,
    post_wait_freezes_ms: u64,
}

impl OperationFlowPolicy {
    fn to_json(self) -> Value {
        json!({
            "retryable": self.retryable,
            "max_attempts": self.max_attempts,
            "retry_interval_ms": self.retry_interval_ms,
            "pre_delay_ms": self.pre_delay_ms,
            "post_delay_ms": self.post_delay_ms,
            "pre_wait_freezes_ms": self.pre_wait_freezes_ms,
            "post_wait_freezes_ms": self.post_wait_freezes_ms
        })
    }
}

#[derive(Debug, Clone)]
struct OperationExpectation {
    page_id: String,
    timeout_ms: Option<u64>,
    interval_ms: Option<u64>,
}

impl OperationExpectation {
    fn to_json(&self) -> Value {
        json!({
            "page_id": self.page_id.as_str(),
            "timeout_ms": self.timeout_ms,
            "interval_ms": self.interval_ms
        })
    }
}

#[derive(Debug, Clone)]
struct OperationGuard {
    page_id: String,
    target_id: String,
    expected_rect: PackRect,
    verify_template: Option<String>,
    color_probe: Option<String>,
}

impl OperationGuard {
    fn to_json(&self) -> Value {
        json!({
            "page_id": self.page_id.as_str(),
            "target_id": self.target_id.as_str(),
            "expected_rect": rect_json(self.expected_rect),
            "verify_template": self.verify_template.as_deref(),
            "color_probe": self.color_probe.as_deref()
        })
    }
}

#[derive(Debug, Clone)]
struct OperationClick {
    kind: String,
    x: Option<i32>,
    y: Option<i32>,
    width: Option<i32>,
    height: Option<i32>,
    from_rect: Option<PackRect>,
    to_rect: Option<PackRect>,
    duration_ms: Option<u64>,
    offset: Option<PackRect>,
}

impl OperationClick {
    fn input_action(
        &self,
        resolution: &Resolution,
        seed: u64,
        guard: Option<&OperationGuard>,
        target: Option<&TargetEvaluation>,
    ) -> CliOutcome<LabInputAction> {
        match self.kind.as_str() {
            "rect" | "specific_rect" => {
                let rect = derive_absolute_coordinate_rect(
                    self.kind.as_str(),
                    self.required_rect()?,
                    guard,
                    target,
                )?;
                validate_click_rect(rect, resolution, false)?;
                Ok(LabInputAction::Tap(actual_click_point(rect, seed)))
            }
            "point" => {
                let rect = derive_absolute_coordinate_rect(
                    "point",
                    self.required_point_rect("point")?,
                    guard,
                    target,
                )?;
                validate_click_rect(rect, resolution, false)?;
                Ok(LabInputAction::Tap(actual_explicit_point(rect, seed)))
            }
            "long_press" | "long_tap" => {
                let rect = derive_absolute_coordinate_rect(
                    "long_press",
                    self.required_point_rect("long_press")?,
                    guard,
                    target,
                )?;
                validate_click_rect(rect, resolution, false)?;
                Ok(LabInputAction::LongTap {
                    point: actual_explicit_point(rect, seed),
                    duration_ms: self.duration_ms.unwrap_or(600),
                })
            }
            "offset" => {
                let guard = guard.ok_or_else(|| {
                    CliError::package_invalid("offset click requires guard metadata")
                })?;
                let target = target.ok_or_else(|| {
                    CliError::package_invalid("offset click requires matched template target")
                })?;
                if target.id != guard.target_id {
                    return Err(CliError::package_invalid(format!(
                        "offset click matched target '{}' does not match guard target_id '{}'",
                        target.id, guard.target_id
                    )));
                }
                let matched_rect = matched_template_rect(target)?;
                let offset = self
                    .offset
                    .ok_or_else(|| CliError::package_invalid("offset click missing offset rect"))?;
                let rect = translated_target_rect(matched_rect, offset)?;
                validate_click_rect(rect, resolution, false)?;
                Ok(LabInputAction::Tap(actual_click_point(rect, seed)))
            }
            "target" | "target_center" => {
                let guard = guard.ok_or_else(|| {
                    CliError::package_invalid("target click requires guard metadata")
                })?;
                let target = target.ok_or_else(|| {
                    CliError::package_invalid("target click requires matched template target")
                })?;
                if target.id != guard.target_id {
                    return Err(CliError::package_invalid(format!(
                        "target click matched target '{}' does not match guard target_id '{}'",
                        target.id, guard.target_id
                    )));
                }
                let matched_rect = matched_template_rect(target)?;
                let rect = if let Some(offset) = self.offset {
                    translated_target_rect(matched_rect, offset)?
                } else {
                    matched_rect
                };
                validate_click_rect(rect, resolution, false)?;
                let point = if self.kind == "target_center" {
                    actual_center_point(rect, seed)
                } else {
                    actual_click_point(rect, seed)
                };
                Ok(LabInputAction::Tap(point))
            }
            "drag" => {
                let declared_from = self
                    .from_rect
                    .ok_or_else(|| CliError::package_invalid("drag click missing from rect"))?;
                let to = self
                    .to_rect
                    .ok_or_else(|| CliError::package_invalid("drag click missing to rect"))?;
                let from = derive_absolute_coordinate_rect("drag", declared_from, guard, target)?;
                let to = derive_absolute_coordinate_rect("drag", to, guard, target)?;
                validate_click_rect(from, resolution, false)?;
                validate_click_rect(to, resolution, false)?;
                Ok(LabInputAction::Drag {
                    from: actual_click_point(from, seed ^ hash_text("drag.from")),
                    to: actual_click_point(to, seed ^ hash_text("drag.to")),
                    duration_ms: self.duration_ms.unwrap_or(300),
                })
            }
            other => Err(CliError::package_invalid(format!(
                "unknown operation click kind '{other}'"
            ))),
        }
    }

    fn required_rect(&self) -> CliOutcome<PackRect> {
        Ok(PackRect {
            x: self
                .x
                .ok_or_else(|| CliError::package_invalid("rect click missing x"))?,
            y: self
                .y
                .ok_or_else(|| CliError::package_invalid("rect click missing y"))?,
            width: self
                .width
                .ok_or_else(|| CliError::package_invalid("rect click missing width"))?,
            height: self
                .height
                .ok_or_else(|| CliError::package_invalid("rect click missing height"))?,
        })
    }

    fn required_point_rect(&self, kind: &str) -> CliOutcome<PackRect> {
        Ok(PackRect {
            x: self
                .x
                .ok_or_else(|| CliError::package_invalid(format!("{kind} click missing x")))?,
            y: self
                .y
                .ok_or_else(|| CliError::package_invalid(format!("{kind} click missing y")))?,
            width: 1,
            height: 1,
        })
    }
}

fn translated_target_rect(matched: PackRect, offset: PackRect) -> CliOutcome<PackRect> {
    Ok(PackRect {
        x: matched.x.checked_add(offset.x).ok_or_else(|| {
            CliError::package_invalid("target click translated x coordinate overflow")
        })?,
        y: matched.y.checked_add(offset.y).ok_or_else(|| {
            CliError::package_invalid("target click translated y coordinate overflow")
        })?,
        width: offset.width,
        height: offset.height,
    })
}

fn matched_template_rect(target: &TargetEvaluation) -> CliOutcome<PackRect> {
    if target.kind != TargetKind::Template {
        return Err(CliError::package_invalid(format!(
            "template-target matched_rect required, got {:?}",
            target.kind
        )));
    }
    let template = target
        .template
        .ok_or_else(|| CliError::package_invalid("template target missing matched_rect"))?;
    if !target.passed {
        return Err(CliError::package_invalid(format!(
            "template target '{}' did not pass guard evaluation",
            target.id
        )));
    }
    let rect = PackRect {
        x: template.x,
        y: template.y,
        width: template.width,
        height: template.height,
    };
    if rect.width <= 0 || rect.height <= 0 {
        return Err(CliError::package_invalid(format!(
            "matched_rect dimensions must be positive: {}x{}",
            rect.width, rect.height
        )));
    }
    Ok(rect)
}

fn derive_absolute_coordinate_rect(
    kind: &str,
    declared: PackRect,
    guard: Option<&OperationGuard>,
    target: Option<&TargetEvaluation>,
) -> CliOutcome<PackRect> {
    let Some(guard) = guard else {
        return Ok(declared);
    };
    let target =
        target.ok_or_else(|| CliError::package_invalid(format!("{kind} requires guard target")))?;
    if target.id != guard.target_id {
        return Err(CliError::package_invalid(format!(
            "{kind} matched target '{}' does not match guard target_id '{}'",
            target.id, guard.target_id
        )));
    }
    Ok(declared)
}

#[derive(Debug, Clone, Copy)]
enum LabInputAction {
    Tap(ActualClickPoint),
    LongTap {
        point: ActualClickPoint,
        duration_ms: u64,
    },
    Drag {
        from: ActualClickPoint,
        to: ActualClickPoint,
        duration_ms: u64,
    },
}

impl LabInputAction {
    fn to_json(self) -> Value {
        match self {
            LabInputAction::Tap(point) => {
                json!({"kind": "tap", "actual_click_point": point.to_json()})
            }
            LabInputAction::Drag {
                from,
                to,
                duration_ms,
            } => {
                json!({"kind": "drag", "from": from.to_json(), "to": to.to_json(), "duration_ms": duration_ms})
            }
            LabInputAction::LongTap { point, duration_ms } => {
                json!({"kind": "long_tap", "actual_click_point": point.to_json(), "duration_ms": duration_ms})
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ActualClickPoint {
    seed: u64,
    algorithm: &'static str,
    rect: PackRect,
    x: i32,
    y: i32,
}

impl ActualClickPoint {
    fn to_json(self) -> Value {
        json!({
            "seed": self.seed,
            "algorithm": self.algorithm,
            "rect": rect_json(self.rect),
            "point": {"x": self.x, "y": self.y}
        })
    }
}

fn actual_click_point(rect: PackRect, seed: u64) -> ActualClickPoint {
    let mut state = if seed == 0 {
        0x9e37_79b9_7f4a_7c15
    } else {
        seed
    };
    let x_offset = next_u64(&mut state) % rect.width as u64;
    let y_offset = next_u64(&mut state) % rect.height as u64;
    ActualClickPoint {
        seed,
        algorithm: "xorshift64_uniform_rect_v1",
        rect,
        x: rect.x + x_offset as i32,
        y: rect.y + y_offset as i32,
    }
}

fn actual_explicit_point(rect: PackRect, seed: u64) -> ActualClickPoint {
    ActualClickPoint {
        seed,
        algorithm: "explicit_point_v1",
        rect,
        x: rect.x,
        y: rect.y,
    }
}

fn actual_center_point(rect: PackRect, seed: u64) -> ActualClickPoint {
    ActualClickPoint {
        seed,
        algorithm: "center_point_v1",
        rect,
        x: rect.x + rect.width / 2,
        y: rect.y + rect.height / 2,
    }
}

fn next_u64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

fn validate_click_rect(
    rect: PackRect,
    resolution: &Resolution,
    allow_placeholder: bool,
) -> CliOutcome<()> {
    if rect.width <= 0 || rect.height <= 0 {
        return Err(CliError::package_invalid(format!(
            "click rect dimensions must be positive: {}x{}",
            rect.width, rect.height
        )));
    }
    let right = rect
        .x
        .checked_add(rect.width - 1)
        .ok_or_else(|| CliError::package_invalid("click rect x coordinate overflow"))?;
    let bottom = rect
        .y
        .checked_add(rect.height - 1)
        .ok_or_else(|| CliError::package_invalid("click rect y coordinate overflow"))?;
    validate_click_point(rect.x, rect.y, resolution, allow_placeholder)?;
    validate_click_point(
        right,
        bottom,
        resolution,
        allow_placeholder,
    )?;
    if !allow_placeholder
        && rect.x == 0
        && rect.y == 0
        && rect.width as u32 == resolution.width
        && rect.height as u32 == resolution.height
    {
        return Err(CliError::package_invalid(
            "full-screen click rect is treated as unresolved coordinates",
        ));
    }
    Ok(())
}

fn validate_click_point(
    x: i32,
    y: i32,
    resolution: &Resolution,
    allow_placeholder: bool,
) -> CliOutcome<()> {
    if x < 0 || y < 0 || x >= resolution.width as i32 || y >= resolution.height as i32 {
        return Err(CliError::package_invalid(format!(
            "click point {x},{y} is outside {}x{}",
            resolution.width, resolution.height
        )));
    }
    if !allow_placeholder && x == 0 && y == 0 {
        return Err(CliError::package_invalid(
            "click point 0,0 is treated as unresolved coordinates",
        ));
    }
    Ok(())
}

fn validate_frame_resolution(control: &LabControl, width: u32, height: u32) -> CliOutcome<()> {
    if width != control.resolution.width || height != control.resolution.height {
        return Err(CliError::device(format!(
            "device frame resolution {width}x{height} does not match package resolution {}x{}",
            control.resolution.width, control.resolution.height
        )));
    }
    Ok(())
}
