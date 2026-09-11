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
    bundle
        .detector()
        .cloned()
        .ok_or_else(|| CliError::package_invalid("missing page detector for Lab package"))?;
    let navigation_path = bundle.navigation_path().map(PathBuf::from);

    Ok(LabResources {
        resource_root,
        manifest_path,
        operation_path,
        operation_bundle,
        pack_path,
        pages_path,
        evaluator,
        navigation_path,
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
    #[serde(rename = "timeout_ms")]
    _timeout_ms: Option<u64>,
    #[serde(default)]
    #[serde(rename = "step_timeout_ms")]
    _step_timeout_ms: Option<u64>,
    #[serde(default)]
    #[serde(rename = "max_steps")]
    _max_steps: Option<usize>,
    #[serde(default)]
    #[serde(rename = "stop_on_error")]
    _stop_on_error: Option<bool>,
    #[serde(default)]
    #[serde(rename = "stop_on_confirmation")]
    _stop_on_confirmation: Option<bool>,
    #[serde(default)]
    allow_placeholder_coords: Option<bool>,
    #[serde(default)]
    #[serde(rename = "output")]
    _output: Option<Value>,
    #[serde(default)]
    capture_backend: Option<String>,
    #[serde(default)]
    frame_store: FrameStoreControl,
    #[serde(default)]
    #[serde(rename = "producer")]
    _producer: Option<Value>,
    #[serde(default)]
    #[serde(rename = "trusted_execution")]
    _trusted_execution: Option<Value>,
}

impl LabControl {
    fn validate(&self) -> CliOutcome<()> {
        if self.schema_version != CONTROL_SCHEMA
            && self.schema_version != actingcommand_contract::PHASED_CONTROL_SCHEMA
        {
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
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct Resolution {
    width: u32,
    height: u32,
}

#[derive(Debug)]
struct LabResources {
    resource_root: PathBuf,
    manifest_path: PathBuf,
    operation_path: PathBuf,
    operation_bundle: OperationBundle,
    pack_path: PathBuf,
    pages_path: PathBuf,
    evaluator: RecognitionEvaluator,
    navigation_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
struct OperationBundle {
    schema_version: String,
    task_id: String,
    game: String,
    #[serde(default)]
    server_scope: Vec<String>,
    #[serde(default)]
    #[serde(rename = "goal")]
    _goal: String,
    coordinate_space: Resolution,
    #[serde(default)]
    defaults: OperationDefaults,
    #[serde(default)]
    anchors: Vec<OperationAnchor>,
    #[serde(default)]
    #[serde(rename = "entry_page")]
    _entry_page: Option<String>,
    #[serde(default)]
    target_page: Option<NormalizedPageSet>,
    #[serde(default)]
    #[serde(rename = "error_pages")]
    _error_pages: Vec<String>,
    #[serde(default)]
    recovery: Option<TaskRecovery>,
    #[serde(default)]
    max_task_retries: Option<u32>,
    #[serde(default)]
    on_exhausted: Option<String>,
    #[serde(default)]
    #[serde(rename = "page_rules")]
    _page_rules: BTreeMap<String, Value>,
    operations: Vec<Operation>,
}

impl OperationBundle {
    fn validate(
        &self,
        control: &LabControl,
        mut operation_asset_exists: impl FnMut(&str) -> CliOutcome<bool>,
    ) -> CliOutcome<()> {
        if !matches!(
            self.schema_version.as_str(),
            "0.3" | "0.4" | "0.5" | "0.6" | "0.7"
        ) {
            return Err(CliError::package_invalid(format!(
                "unsupported operation schema_version '{}', expected one of 0.3, 0.4, 0.5, 0.6, 0.7",
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
        if let Some(target_pages) = &self.target_page {
            target_pages.validate("operation bundle target_page")?;
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
            operation.validate_for_schema(control, &self.schema_version)?;
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
    #[serde(rename = "template_threshold")]
    _template_threshold: f32,
    #[serde(default)]
    #[serde(rename = "color_max_distance")]
    _color_max_distance: Option<f32>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    max_attempts: Option<u32>,
    #[serde(default)]
    retry_interval_ms: Option<u64>,
    #[serde(default)]
    #[serde(rename = "pre_delay_ms")]
    _pre_delay_ms: Option<u64>,
    #[serde(default)]
    #[serde(rename = "post_delay_ms")]
    _post_delay_ms: Option<u64>,
    #[serde(default)]
    #[serde(rename = "pre_wait_freezes_ms")]
    _pre_wait_freezes_ms: Option<u64>,
    #[serde(default)]
    #[serde(rename = "post_wait_freezes_ms")]
    _post_wait_freezes_ms: Option<u64>,
}

impl Default for OperationDefaults {
    fn default() -> Self {
        Self {
            _template_threshold: DEFAULT_TEMPLATE_THRESHOLD,
            _color_max_distance: None,
            timeout_ms: None,
            max_attempts: None,
            retry_interval_ms: None,
            _pre_delay_ms: None,
            _post_delay_ms: None,
            _pre_wait_freezes_ms: None,
            _post_wait_freezes_ms: None,
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
    #[serde(rename = "purpose")]
    _purpose: String,
    from: String,
    #[serde(default)]
    to: Option<NormalizedPageSet>,
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
    #[serde(rename = "pre_delay_ms")]
    _pre_delay_ms: Option<u64>,
    #[serde(default)]
    #[serde(rename = "post_delay_ms")]
    _post_delay_ms: Option<u64>,
    #[serde(default)]
    #[serde(rename = "pre_wait_freezes_ms")]
    _pre_wait_freezes_ms: Option<u64>,
    #[serde(default)]
    #[serde(rename = "post_wait_freezes_ms")]
    _post_wait_freezes_ms: Option<u64>,
    #[serde(default)]
    #[serde(rename = "retryable")]
    _retryable: Option<bool>,
    #[serde(default)]
    effect: Option<String>,
    #[serde(default)]
    on_error: Option<String>,
    #[serde(default)]
    guard: Option<OperationGuard>,
    #[serde(default)]
    unguarded_trusted_coordinate: bool,
    #[serde(default)]
    #[serde(rename = "consumes")]
    _consumes: Vec<String>,
    #[serde(default)]
    #[serde(rename = "produces")]
    _produces: Vec<String>,
    #[serde(default)]
    #[serde(rename = "verified_live")]
    _verified_live: Option<bool>,
    #[serde(default)]
    #[serde(rename = "provenance")]
    _provenance: Option<Value>,
}

impl Operation {
    fn validate_for_schema(&self, control: &LabControl, schema_version: &str) -> CliOutcome<()> {
        for (name, value) in [("id", &self.id), ("from", &self.from)] {
            if value.trim().is_empty() {
                return Err(CliError::package_invalid(format!(
                    "operation {name} must not be empty"
                )));
            }
        }
        self.click.validate_for_schema(control, schema_version)?;
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
        self.destination_pages()?;
        self.validate_flow()?;
        self.validate_guard(control)
    }

    #[cfg(test)]
    fn validate(&self, control: &LabControl) -> CliOutcome<()> {
        self.validate_for_schema(control, "0.6")
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

    fn destination_pages(&self) -> CliOutcome<&[String]> {
        let to = self.to.as_ref().map(NormalizedPageSet::as_slice);
        let expected = self
            .expect_after
            .as_ref()
            .map(|expectation| expectation.page_id.as_slice());
        match (to, expected) {
            (Some(to), Some(expected)) if to != expected => {
                Err(CliError::package_invalid(format!(
                    "operation '{}' has conflicting to and expect_after destinations",
                    self.id
                )))
            }
            (Some(to), _) => Ok(to),
            (None, Some(expected)) => Ok(expected),
            (None, None) => Ok(&[]),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct OperationExpectation {
    page_id: NormalizedPageSet,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    interval_ms: Option<u64>,
}

impl OperationExpectation {
    fn validate(&self, operation_id: &str) -> CliOutcome<()> {
        self.page_id
            .validate(&format!("operation '{operation_id}' expect_after.page_id"))?;
        if !actingcommand_contract::postcondition_timeout_is_valid(self.timeout_ms) {
            return Err(CliError::package_invalid(format!(
                "operation '{operation_id}' expect_after.timeout_ms must be in 1..={} when provided",
                actingcommand_contract::MAX_POSTCONDITION_TIMEOUT_MS
            )));
        }
        if self.interval_ms == Some(0) {
            return Err(CliError::package_invalid(format!(
                "operation '{operation_id}' expect_after.interval_ms must be positive when provided"
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedPageSet(Vec<String>);

impl NormalizedPageSet {
    fn as_slice(&self) -> &[String] {
        &self.0
    }

    fn validate(&self, label: &str) -> CliOutcome<()> {
        if self.0.is_empty() || self.0.iter().any(|page| page.trim().is_empty()) {
            return Err(CliError::package_invalid(format!(
                "{label} must contain at least one non-empty page"
            )));
        }
        let unique = self.0.iter().collect::<BTreeSet<_>>();
        if unique.len() != self.0.len() {
            return Err(CliError::package_invalid(format!(
                "{label} must not contain duplicate pages"
            )));
        }
        Ok(())
    }
}

impl serde::Serialize for NormalizedPageSet {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if let [page] = self.0.as_slice() {
            serializer.serialize_str(page)
        } else {
            serde::Serialize::serialize(&self.0, serializer)
        }
    }
}

impl<'de> Deserialize<'de> for NormalizedPageSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Declaration {
            Singleton(String),
            Set(Vec<String>),
        }

        let mut pages = match Declaration::deserialize(deserializer)? {
            Declaration::Singleton(page) => vec![page],
            Declaration::Set(pages) => pages,
        };
        if pages.is_empty() || pages.iter().any(|page| page.trim().is_empty()) {
            return Err(serde::de::Error::custom(
                "page set must contain at least one non-empty page",
            ));
        }
        let unique = pages.iter().collect::<BTreeSet<_>>();
        if unique.len() != pages.len() {
            return Err(serde::de::Error::custom(
                "page set must not contain duplicate pages",
            ));
        }
        pages.sort();
        Ok(Self(pages))
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
    #[serde(default, alias = "from")]
    from_rect: Option<PackRect>,
    #[serde(default, alias = "to")]
    to_rect: Option<PackRect>,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    offset: Option<PackRect>,
    #[serde(default)]
    target_id: Option<String>,
    #[serde(default, flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct SegmentedSwipeClickFields {
    corner_rect: PackRect,
    horizontal_duration_ms: u64,
    corner_hold_ms: u64,
    brake_distance_px: i32,
    brake_duration_ms: u64,
}

impl OperationClick {
    fn validate_for_schema(&self, control: &LabControl, schema_version: &str) -> CliOutcome<()> {
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
            "single_touch_drag_with_vertical_brake_v1" => {
                let fields = self.segmented_swipe_fields()?;
                if schema_version != "0.7"
                    || self.x.is_some()
                    || self.y.is_some()
                    || self.width.is_some()
                    || self.height.is_some()
                    || self.to_rect.is_some()
                    || self.duration_ms.is_some()
                    || self.offset.is_some()
                    || self.target_id.is_some()
                    || fields.horizontal_duration_ms != 200
                    || fields.corner_hold_ms != 150
                    || fields.brake_distance_px != 100
                    || fields.brake_duration_ms != 200
                {
                    return Err(CliError::package_invalid(
                        "single_touch_drag_with_vertical_brake_v1 declaration is invalid",
                    ));
                }
                let from = self.from_rect.ok_or_else(|| {
                    CliError::package_invalid(
                        "single_touch_drag_with_vertical_brake_v1 missing from_rect",
                    )
                })?;
                let corner = fields.corner_rect;
                validate_click_rect(from, &control.resolution, false)?;
                validate_click_rect(corner, &control.resolution, false)?;
                if corner.y < fields.brake_distance_px {
                    return Err(CliError::package_invalid(
                        "single_touch_drag_with_vertical_brake_v1 brake endpoint is out of bounds",
                    ));
                }
                Ok(())
            }
            other => Err(CliError::package_invalid(format!(
                "unknown operation click kind '{other}'"
            ))),
        }
    }

    fn segmented_swipe_fields(&self) -> CliOutcome<SegmentedSwipeClickFields> {
        serde_json::from_value(serde_json::to_value(&self.extra).map_err(|error| {
            CliError::package_invalid(format!(
                "single_touch_drag_with_vertical_brake_v1 fields are invalid: {error}"
            ))
        })?)
        .map_err(|error| {
            CliError::package_invalid(format!(
                "single_touch_drag_with_vertical_brake_v1 fields are invalid: {error}"
            ))
        })
    }

    #[cfg(test)]
    fn validate(&self, control: &LabControl) -> CliOutcome<()> {
        self.validate_for_schema(control, "0.6")
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
