// SPDX-License-Identifier: AGPL-3.0-only

fn lab_control_from_bundle(bundle: &LoadedBundle) -> CliOutcome<LabControl> {
    let Some(control) = bundle.control() else {
        return Err(CliError::package_invalid(
            "Lab package must include control.json",
        ));
    };
    serde_json::from_value(control.clone())
        .map_err(|err| CliError::package_invalid(format!("failed to parse control.json: {err}")))
}
fn load_lab_resources_from_bundle(
    bundle: LoadedBundle,
    control: &LabControl,
) -> CliOutcome<LabResources> {
    let resource_root = PathBuf::from(bundle.resource_root());
    let manifest_path = PathBuf::from(bundle.manifest_path());
    let manifest = bundle.manifest().clone();
    validate_manifest_entry_task_id(&manifest_path, &manifest, control)?;
    let operation_path = PathBuf::from(bundle.operation_path());
    let operation_bundle: OperationBundle = serde_json::from_value(bundle.operation().clone())
        .map_err(|err| {
            CliError::package_invalid(format!(
                "failed to parse {}: {err}",
                bundle.operation_path()
            ))
        })?;
    operation_bundle.validate(control, |relative| {
        bundle
            .resource_entry(&format!(
                "operations/{}/{}",
                control.entry_task_id, relative
            ))
            .map(|_| true)
            .or_else(|err| match err {
                ContainmentError::MissingEntry { .. } => Ok(false),
                other => Err(containment_error(other)),
            })
    })?;
    validate_recovery_task_entries(&bundle, control, &operation_bundle)?;
    let pack_path = bundle
        .recognition_pack_path()
        .map(PathBuf::from)
        .ok_or_else(|| CliError::package_invalid("missing recognition pack for Lab package"))?;
    let pages_path = bundle
        .pages_path()
        .map(PathBuf::from)
        .ok_or_else(|| CliError::package_invalid("missing page set for Lab package"))?;
    let evaluator = bundle.evaluator().cloned().ok_or_else(|| {
        CliError::package_invalid("missing recognition evaluator for Lab package")
    })?;
    let detector = bundle
        .detector()
        .cloned()
        .ok_or_else(|| CliError::package_invalid("missing page detector for Lab package"))?;
    let navigation_path = bundle.navigation_path().map(PathBuf::from);
    let navigation = bundle.navigation().cloned();

    Ok(LabResources {
        bundle,
        resource_root,
        manifest_path,
        manifest,
        operation_path,
        operation_bundle,
        pack_path,
        pages_path,
        evaluator,
        detector,
        navigation_path,
        navigation,
    })
}
fn validate_manifest_entry_task_id(
    manifest_path: &Path,
    manifest: &Value,
    control: &LabControl,
) -> CliOutcome<()> {
    let Some(value) = manifest.get("entry_task_id") else {
        return Ok(());
    };
    let Some(manifest_entry_task_id) = value.as_str() else {
        return Err(CliError::package_invalid(format!(
            "{} entry_task_id must be a string when present",
            manifest_path.display()
        )));
    };
    if manifest_entry_task_id != control.entry_task_id {
        return Err(CliError::package_invalid(format!(
            "{} entry_task_id '{}' conflicts with control entry_task_id '{}'",
            manifest_path.display(),
            manifest_entry_task_id,
            control.entry_task_id
        )));
    }
    Ok(())
}

fn validate_recovery_task_entries(
    bundle: &LoadedBundle,
    control: &LabControl,
    operation_bundle: &OperationBundle,
) -> CliOutcome<()> {
    let mut task_ids = BTreeSet::new();
    if let Some(recovery) = &operation_bundle.recovery {
        task_ids.insert(recovery.task_id());
    }
    if operation_bundle
        .operations
        .iter()
        .any(|operation| operation.on_error.is_some())
    {
        task_ids.insert(DEFAULT_RECOVERY_TASK_ID);
    }
    for task_id in task_ids {
        let path = format!("operations/{task_id}/task.json");
        let bytes = match bundle.resource_entry(&path) {
            Ok(bytes) => bytes,
            Err(ContainmentError::MissingEntry { .. }) => {
                return Err(CliError::package_invalid(format!(
                    "configured recovery task '{task_id}' is missing {path}"
                )));
            }
            Err(error) => return Err(containment_error(error)),
        };
        let recovery_bundle: OperationBundle = serde_json::from_slice(bytes).map_err(|error| {
            CliError::package_invalid(format!(
                "failed to parse configured recovery task {path}: {error}"
            ))
        })?;
        recovery_bundle.validate(control, |relative| {
            bundle
                .resource_entry(&format!("operations/{task_id}/{relative}"))
                .or_else(|error| match error {
                    ContainmentError::MissingEntry { .. } => bundle.resource_entry(relative),
                    other => Err(other),
                })
                .map(|_| true)
                .or_else(|error| match error {
                    ContainmentError::MissingEntry { .. } => Ok(false),
                    other => Err(containment_error(other)),
                })
        })?;
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
struct LabControl {
    schema_version: String,
    package_id: String,
    execution_mode: String,
    game: String,
    server: String,
    resolution: Resolution,
    entry_task_id: String,
    #[serde(default)]
    capture_interval_ms: Option<u64>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    step_timeout_ms: Option<u64>,
    #[serde(default)]
    max_steps: Option<usize>,
    #[serde(default)]
    stop_on_error: Option<bool>,
    #[serde(default)]
    stop_on_confirmation: Option<bool>,
    #[serde(default)]
    allow_placeholder_coords: Option<bool>,
    #[serde(default)]
    output: Option<Value>,
    #[serde(default)]
    capture_backend: Option<String>,
    #[serde(default)]
    frame_store: FrameStoreControl,
    #[serde(default)]
    producer: Option<Value>,
    #[serde(default)]
    trusted_execution: Option<Value>,
}

impl LabControl {
    fn validate(&self) -> CliOutcome<()> {
        if self.schema_version != CONTROL_SCHEMA {
            return Err(CliError::package_invalid(format!(
                "unsupported control schema_version '{}', expected {CONTROL_SCHEMA}",
                self.schema_version
            )));
        }
        if !matches!(
            self.execution_mode.as_str(),
            "navigable_route" | "recognize_only" | "in_page_guard"
        ) {
            return Err(CliError::package_invalid(format!(
                "unsupported execution_mode '{}', expected navigable_route, recognize_only, or in_page_guard",
                self.execution_mode
            )));
        }
        for (name, value) in [
            ("package_id", &self.package_id),
            ("game", &self.game),
            ("server", &self.server),
            ("entry_task_id", &self.entry_task_id),
        ] {
            if value.trim().is_empty() {
                return Err(CliError::package_invalid(format!(
                    "control {name} is empty"
                )));
            }
        }
        if self.resolution.width == 0 || self.resolution.height == 0 {
            return Err(CliError::package_invalid(
                "control resolution width and height must be non-zero",
            ));
        }
        if self.capture_interval_ms == Some(0) {
            return Err(CliError::package_invalid(
                "capture_interval_ms must be positive when provided",
            ));
        }
        if let Some(capture_backend) = &self.capture_backend {
            CaptureBackendChoice::parse(capture_backend)
                .map_err(|err| CliError::package_invalid(err.to_string()))?;
        }
        self.frame_store
            .validate()
            .map_err(CliError::package_invalid)?;
        Ok(())
    }

    fn capture_backend_choice(&self) -> CliOutcome<Option<CaptureBackendChoice>> {
        self.capture_backend
            .as_deref()
            .map(CaptureBackendChoice::parse)
            .transpose()
            .map_err(|err| CliError::package_invalid(err.to_string()))
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct Resolution {
    width: u32,
    height: u32,
}

#[derive(Debug)]
struct LabResources {
    bundle: LoadedBundle,
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
    navigation: Option<Value>,
}

impl LabResources {
    fn has_operation_bundle(&self, task_id: &str) -> CliOutcome<bool> {
        let path = format!("operations/{task_id}/task.json");
        match self.bundle.resource_entry(&path) {
            Ok(_) => Ok(true),
            Err(ContainmentError::MissingEntry { .. }) => Ok(false),
            Err(err) => Err(containment_error(err)),
        }
    }

    fn operation_asset_for_task(&self, task_id: &str, relative: &str) -> CliOutcome<&[u8]> {
        self.bundle
            .resource_entry(&format!("operations/{}/{}", task_id, relative))
            .or_else(|err| match err {
                ContainmentError::MissingEntry { .. } => self.bundle.resource_entry(relative),
                other => Err(other),
            })
            .map_err(containment_error)
    }

}

#[derive(Debug)]
struct RunState {
    control: LabControl,
    resources: LabResources,
    current_page: Option<String>,
    failed_step_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct OperationBundle {
    schema_version: String,
    task_id: String,
    game: String,
    #[serde(default)]
    server_scope: Vec<String>,
    #[serde(default)]
    goal: String,
    coordinate_space: Resolution,
    #[serde(default)]
    defaults: OperationDefaults,
    #[serde(default)]
    anchors: Vec<OperationAnchor>,
    #[serde(default)]
    entry_page: Option<String>,
    #[serde(default)]
    target_page: Option<String>,
    #[serde(default)]
    error_pages: Vec<String>,
    #[serde(default)]
    recovery: Option<TaskRecovery>,
    #[serde(default)]
    max_task_retries: Option<u32>,
    #[serde(default)]
    on_exhausted: Option<String>,
    #[serde(default)]
    page_rules: BTreeMap<String, Value>,
    operations: Vec<Operation>,
}

impl OperationBundle {
    fn validate(
        &self,
        control: &LabControl,
        mut operation_asset_exists: impl FnMut(&str) -> CliOutcome<bool>,
    ) -> CliOutcome<()> {
        if !matches!(self.schema_version.as_str(), "0.3" | "0.4" | "0.5" | "0.6") {
            return Err(CliError::package_invalid(format!(
                "unsupported operation schema_version '{}', expected one of 0.3, 0.4, 0.5, 0.6",
                self.schema_version
            )));
        }
        if self.task_id != control.entry_task_id && self.task_id != "return_home" {
            return Err(CliError::package_invalid(format!(
                "operation task_id '{}' does not match control entry_task_id '{}'",
                self.task_id, control.entry_task_id
            )));
        }
        if self.game != control.game {
            return Err(CliError::package_invalid(format!(
                "operation game '{}' does not match control game '{}'",
                self.game, control.game
            )));
        }
        if !self.server_scope.is_empty()
            && !self
                .server_scope
                .iter()
                .any(|server| server == &control.server)
        {
            return Err(CliError::package_invalid(format!(
                "operation server_scope does not include '{}'",
                control.server
            )));
        }
        if self.coordinate_space.width != control.resolution.width
            || self.coordinate_space.height != control.resolution.height
        {
            return Err(CliError::package_invalid(format!(
                "operation coordinate_space {}x{} does not match control resolution {}x{}",
                self.coordinate_space.width,
                self.coordinate_space.height,
                control.resolution.width,
                control.resolution.height
            )));
        }
        if self.operations.is_empty() {
            return Err(CliError::package_invalid(
                "operation bundle has no operations",
            ));
        }
        self.defaults.validate()?;
        for anchor in &self.anchors {
            if anchor.id.trim().is_empty() {
                return Err(CliError::package_invalid(
                    "operation anchor id must not be empty",
                ));
            }
            if !operation_asset_exists(&anchor.template)? {
                return Err(CliError::package_invalid(format!(
                    "operation anchor '{}' references missing template {}",
                    anchor.id, anchor.template
                )));
            }
        }
        let mut ids = BTreeSet::new();
        for operation in &self.operations {
            operation.validate(control)?;
            if !ids.insert(operation.id.clone()) {
                return Err(CliError::package_invalid(format!(
                    "duplicate operation id '{}'",
                    operation.id
                )));
            }
            if let Some(template) = &operation.verify_template
                && !operation_asset_exists(template)?
            {
                return Err(CliError::package_invalid(format!(
                    "operation '{}' references missing verify_template {}",
                    operation.id, template
                )));
            }
            if let Some(guard_template) = operation
                .guard
                .as_ref()
                .and_then(|guard| guard.verify_template.as_ref())
                && !matches!(
                    operation.click.kind.as_str(),
                    "offset" | "target" | "target_center"
                )
                && !operation_asset_exists(guard_template)?
            {
                return Err(CliError::package_invalid(format!(
                    "operation '{}' guard references missing verify_template {}",
                    operation.id, guard_template
                )));
            }
        }
        self.validate_recovery()?;
        Ok(())
    }

    fn validate_recovery(&self) -> CliOutcome<()> {
        if self.max_task_retries == Some(0) {
            return Err(CliError::package_invalid(
                "operation bundle max_task_retries must be positive when provided",
            ));
        }
        if let Some(recovery) = &self.recovery {
            recovery.validate()?;
        }
        if let Some(on_exhausted) = &self.on_exhausted
            && on_exhausted != "pause"
        {
            return Err(CliError::package_invalid(format!(
                "operation bundle on_exhausted '{on_exhausted}' is unsupported; expected pause"
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum TaskRecovery {
    Kind(String),
    Config {
        kind: String,
        #[serde(default)]
        task_id: Option<String>,
    },
}

impl TaskRecovery {
    fn validate(&self) -> CliOutcome<()> {
        if self.kind() != "return_home" {
            return Err(CliError::package_invalid(format!(
                "operation bundle recovery kind '{}' is unsupported; expected return_home",
                self.kind()
            )));
        }
        if self.task_id().trim().is_empty() {
            return Err(CliError::package_invalid(
                "operation bundle recovery task_id must not be empty",
            ));
        }
        Ok(())
    }

    fn kind(&self) -> &str {
        match self {
            TaskRecovery::Kind(kind) | TaskRecovery::Config { kind, .. } => kind,
        }
    }

    fn task_id(&self) -> &str {
        match self {
            TaskRecovery::Kind(_) => DEFAULT_RECOVERY_TASK_ID,
            TaskRecovery::Config { task_id, .. } => {
                task_id.as_deref().unwrap_or(DEFAULT_RECOVERY_TASK_ID)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct OperationDefaults {
    #[serde(default = "default_template_threshold")]
    template_threshold: f32,
    #[serde(default)]
    color_max_distance: Option<f32>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    max_attempts: Option<u32>,
    #[serde(default)]
    retry_interval_ms: Option<u64>,
    #[serde(default)]
    pre_delay_ms: Option<u64>,
    #[serde(default)]
    post_delay_ms: Option<u64>,
    #[serde(default)]
    pre_wait_freezes_ms: Option<u64>,
    #[serde(default)]
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
    fn validate(self) -> CliOutcome<()> {
        for (name, value) in [
            ("timeout_ms", self.timeout_ms),
            ("max_attempts", self.max_attempts.map(u64::from)),
            ("retry_interval_ms", self.retry_interval_ms),
        ] {
            if value == Some(0) {
                return Err(CliError::package_invalid(format!(
                    "operation defaults {name} must be positive when provided"
                )));
            }
        }
        Ok(())
    }

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

fn default_template_threshold() -> f32 {
    DEFAULT_TEMPLATE_THRESHOLD
}

#[derive(Debug, Clone, Deserialize)]
struct OperationAnchor {
    id: String,
    template: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Operation {
    id: String,
    purpose: String,
    from: String,
    #[serde(default)]
    to: Option<String>,
    click: OperationClick,
    #[serde(default)]
    verify_template: Option<String>,
    #[serde(default)]
    expect_after: Option<OperationExpectation>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    max_attempts: Option<u32>,
    #[serde(default)]
    retry_interval_ms: Option<u64>,
    #[serde(default)]
    pre_delay_ms: Option<u64>,
    #[serde(default)]
    post_delay_ms: Option<u64>,
    #[serde(default)]
    pre_wait_freezes_ms: Option<u64>,
    #[serde(default)]
    post_wait_freezes_ms: Option<u64>,
    #[serde(default)]
    retryable: Option<bool>,
    #[serde(default)]
    effect: Option<String>,
    #[serde(default)]
    on_error: Option<String>,
    #[serde(default)]
    guard: Option<OperationGuard>,
    #[serde(default)]
    unguarded_trusted_coordinate: bool,
    #[serde(default)]
    consumes: Vec<String>,
    #[serde(default)]
    produces: Vec<String>,
    #[serde(default)]
    verified_live: Option<bool>,
    #[serde(default)]
    provenance: Option<Value>,
}

impl Operation {
    fn validate(&self, control: &LabControl) -> CliOutcome<()> {
        for (name, value) in [("id", &self.id), ("from", &self.from)] {
            if value.trim().is_empty() {
                return Err(CliError::package_invalid(format!(
                    "operation {name} must not be empty"
                )));
            }
        }
        self.click.validate(control)?;
        if matches!(
            self.click.kind.as_str(),
            "offset" | "target" | "target_center"
        ) {
            let guard = self.guard.as_ref().ok_or_else(|| {
                CliError::package_invalid(format!(
                    "operation '{}' {} click requires guard metadata",
                    self.id, self.click.kind
                ))
            })?;
            if let Some(target_id) = self.click.target_id.as_deref()
                && target_id != guard.target_id
            {
                return Err(CliError::package_invalid(format!(
                    "operation '{}' {} click target_id '{}' does not match guard target_id '{}'",
                    self.id, self.click.kind, target_id, guard.target_id
                )));
            }
            if guard.verify_template.is_none() {
                return Err(CliError::package_invalid(format!(
                    "operation '{}' {} click requires template guard metadata; color-probe guards cannot produce a matched_rect",
                    self.id, self.click.kind
                )));
            }
        }
        if let Some(expect_after) = &self.expect_after {
            expect_after.validate(&self.id)?;
        }
        self.validate_flow()?;
        self.validate_guard(control)
    }

    fn validate_flow(&self) -> CliOutcome<()> {
        if self.timeout_ms == Some(0) {
            return Err(CliError::package_invalid(format!(
                "operation '{}' timeout_ms must be positive when provided",
                self.id
            )));
        }
        if self.max_attempts == Some(0) {
            return Err(CliError::package_invalid(format!(
                "operation '{}' max_attempts must be positive when provided",
                self.id
            )));
        }
        if self.retry_interval_ms == Some(0) {
            return Err(CliError::package_invalid(format!(
                "operation '{}' retry_interval_ms must be positive when provided",
                self.id
            )));
        }
        if let Some(effect) = &self.effect
            && effect != "navigation_only"
        {
            return Err(CliError::package_invalid(format!(
                "operation '{}' effect '{effect}' is unsupported; expected navigation_only",
                self.id
            )));
        }
        if let Some(on_error) = &self.on_error
            && on_error != "return_home"
        {
            return Err(CliError::package_invalid(format!(
                "operation '{}' on_error '{on_error}' is unsupported; expected return_home",
                self.id
            )));
        }
        Ok(())
    }

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

    fn validate_guard(&self, control: &LabControl) -> CliOutcome<()> {
        match (&self.guard, self.unguarded_trusted_coordinate) {
            (Some(_), true) => Err(CliError::package_invalid(format!(
                "operation '{}' cannot set both guard and unguarded_trusted_coordinate",
                self.id
            ))),
            (None, true) => Ok(()),
            (None, false) => Err(CliError::package_invalid(format!(
                "operation '{}' coordinate action missing guard metadata; add guard or set unguarded_trusted_coordinate for reviewed trusted coordinates",
                self.id
            ))),
            (Some(guard), false) => guard.validate(&self.id, &self.from, control),
        }
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

#[derive(Debug, Clone, Deserialize)]
struct OperationExpectation {
    page_id: String,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    interval_ms: Option<u64>,
}

impl OperationExpectation {
    fn validate(&self, operation_id: &str) -> CliOutcome<()> {
        if self.page_id.trim().is_empty() {
            return Err(CliError::package_invalid(format!(
                "operation '{operation_id}' expect_after.page_id must not be empty"
            )));
        }
        if self.timeout_ms == Some(0) {
            return Err(CliError::package_invalid(format!(
                "operation '{operation_id}' expect_after.timeout_ms must be positive when provided"
            )));
        }
        if self.interval_ms == Some(0) {
            return Err(CliError::package_invalid(format!(
                "operation '{operation_id}' expect_after.interval_ms must be positive when provided"
            )));
        }
        Ok(())
    }

    fn to_json(&self) -> Value {
        json!({
            "page_id": self.page_id.as_str(),
            "timeout_ms": self.timeout_ms,
            "interval_ms": self.interval_ms
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
struct OperationGuard {
    page_id: String,
    target_id: String,
    expected_rect: PackRect,
    #[serde(default)]
    verify_template: Option<String>,
    #[serde(default)]
    color_probe: Option<String>,
}

impl OperationGuard {
    fn validate(
        &self,
        operation_id: &str,
        operation_from: &str,
        control: &LabControl,
    ) -> CliOutcome<()> {
        if self.page_id.trim().is_empty() {
            return Err(CliError::package_invalid(format!(
                "operation '{operation_id}' guard.page_id must not be empty"
            )));
        }
        if self.target_id.trim().is_empty() {
            return Err(CliError::package_invalid(format!(
                "operation '{operation_id}' guard.target_id must not be empty"
            )));
        }
        if !page_anchor_matches(&control.game, &self.page_id, operation_from) {
            return Err(CliError::package_invalid(format!(
                "operation '{operation_id}' guard.page_id '{}' does not match operation from '{}'",
                self.page_id, operation_from
            )));
        }
        validate_guard_rect(self.expected_rect, &control.resolution)?;
        let has_verify_target = self.verify_template.is_some() || self.color_probe.is_some();
        if !has_verify_target {
            return Err(CliError::package_invalid(format!(
                "operation '{operation_id}' guard requires verify_template or color_probe"
            )));
        }
        Ok(())
    }

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

#[derive(Debug, Clone, Deserialize)]
struct OperationClick {
    kind: String,
    #[serde(default)]
    x: Option<i32>,
    #[serde(default)]
    y: Option<i32>,
    #[serde(default)]
    width: Option<i32>,
    #[serde(default)]
    height: Option<i32>,
    #[serde(default, rename = "from")]
    from_rect: Option<PackRect>,
    #[serde(default, rename = "to")]
    to_rect: Option<PackRect>,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    offset: Option<PackRect>,
    #[serde(default)]
    target_id: Option<String>,
}

impl OperationClick {
    fn validate(&self, control: &LabControl) -> CliOutcome<()> {
        match self.kind.as_str() {
            "rect" | "specific_rect" => {
                let rect = self.required_rect()?;
                validate_click_rect(
                    rect,
                    &control.resolution,
                    control.allow_placeholder_coords.unwrap_or(false),
                )
            }
            "point" => {
                let x = self
                    .x
                    .ok_or_else(|| CliError::package_invalid("point click missing x"))?;
                let y = self
                    .y
                    .ok_or_else(|| CliError::package_invalid("point click missing y"))?;
                validate_click_point(
                    x,
                    y,
                    &control.resolution,
                    control.allow_placeholder_coords.unwrap_or(false),
                )
            }
            "long_press" | "long_tap" => {
                let x = self
                    .x
                    .ok_or_else(|| CliError::package_invalid("long_press click missing x"))?;
                let y = self
                    .y
                    .ok_or_else(|| CliError::package_invalid("long_press click missing y"))?;
                validate_click_point(
                    x,
                    y,
                    &control.resolution,
                    control.allow_placeholder_coords.unwrap_or(false),
                )?;
                if self.duration_ms.unwrap_or(0) == 0 {
                    return Err(CliError::package_invalid(
                        "long_press duration_ms must be positive",
                    ));
                }
                Ok(())
            }
            "offset" => {
                let offset = self
                    .offset
                    .ok_or_else(|| CliError::package_invalid("offset click missing offset rect"))?;
                if offset.width <= 0 || offset.height <= 0 {
                    return Err(CliError::package_invalid(format!(
                        "offset click dimensions must be positive: {}x{}",
                        offset.width, offset.height
                    )));
                }
                Ok(())
            }
            "target" | "target_center" => {
                if let Some(offset) = self.offset
                    && (offset.width <= 0 || offset.height <= 0)
                {
                    return Err(CliError::package_invalid(format!(
                        "target click offset dimensions must be positive: {}x{}",
                        offset.width, offset.height
                    )));
                }
                Ok(())
            }
            "drag" => {
                let from = self
                    .from_rect
                    .ok_or_else(|| CliError::package_invalid("drag click missing from rect"))?;
                let to = self
                    .to_rect
                    .ok_or_else(|| CliError::package_invalid("drag click missing to rect"))?;
                validate_click_rect(
                    from,
                    &control.resolution,
                    control.allow_placeholder_coords.unwrap_or(false),
                )?;
                validate_click_rect(
                    to,
                    &control.resolution,
                    control.allow_placeholder_coords.unwrap_or(false),
                )?;
                if self.duration_ms.unwrap_or(0) == 0 {
                    return Err(CliError::package_invalid(
                        "drag duration_ms must be positive",
                    ));
                }
                Ok(())
            }
            other => Err(CliError::package_invalid(format!(
                "unknown operation click kind '{other}'"
            ))),
        }
    }

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
                let rect = PackRect {
                    x: matched_rect.x + offset.x,
                    y: matched_rect.y + offset.y,
                    width: offset.width,
                    height: offset.height,
                };
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
                    PackRect {
                        x: matched_rect.x + offset.x,
                        y: matched_rect.y + offset.y,
                        width: offset.width,
                        height: offset.height,
                    }
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
    validate_click_point(rect.x, rect.y, resolution, allow_placeholder)?;
    validate_click_point(
        rect.x + rect.width - 1,
        rect.y + rect.height - 1,
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
fn validate_guard_rect(rect: PackRect, resolution: &Resolution) -> CliOutcome<()> {
    if rect.width <= 0 || rect.height <= 0 {
        return Err(CliError::package_invalid(format!(
            "guard expected_rect dimensions must be positive: {}x{}",
            rect.width, rect.height
        )));
    }
    validate_rect_point(rect.x, rect.y, resolution, "guard expected_rect")?;
    validate_rect_point(
        rect.x + rect.width - 1,
        rect.y + rect.height - 1,
        resolution,
        "guard expected_rect",
    )
}

fn validate_rect_point(x: i32, y: i32, resolution: &Resolution, label: &str) -> CliOutcome<()> {
    if x < 0 || y < 0 || x >= resolution.width as i32 || y >= resolution.height as i32 {
        return Err(CliError::package_invalid(format!(
            "{label} point {x},{y} is outside {}x{}",
            resolution.width, resolution.height
        )));
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
