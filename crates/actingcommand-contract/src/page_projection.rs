// SPDX-License-Identifier: AGPL-3.0-only

//! Shared, side-effect-free projection of already resolved single-frame facts.

use crate::{LabError, LabResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const METADATA_SCHEMA: &str = "actingcommand.page-projection-metadata.v1";
pub const PROJECTION_SCHEMA: &str = "actingcommand.page-projection.v1";
pub const ENTRY_LIMIT: usize = 64;
pub const BYTE_LIMIT: usize = 32 * 1024;
const METADATA_BYTE_LIMIT: usize = 1024 * 1024;
const INPUT_ENTRY_LIMIT: usize = 4096;

fn invalid(message: impl Into<String>) -> LabError {
    LabError::package_invalid(message)
}

fn text(value: &str) -> LabResult<()> {
    if value.trim().is_empty() {
        return Err(invalid(
            "projection metadata identity/source must be nonempty",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElementRole {
    Navigate,
    PageOp,
    ControlPoint,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionKey {
    pub role: ElementRole,
    #[serde(default)]
    pub task_id: String,
    pub resource_id: String,
    pub page: Option<String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Safety {
    Safe,
    #[default]
    Dangerous,
    Unclassified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Privacy {
    Public,
    Personal,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowCompleteness {
    Complete,
    Windowed,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionAnnotation {
    pub action: ActionKey,
    #[serde(default)]
    pub safety: Safety,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetAnnotation {
    pub target_id: String,
    pub privacy: Privacy,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldKey {
    pub task_id: String,
    pub field_id: String,
    pub target_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldAnnotation {
    pub field: FieldKey,
    pub privacy: Privacy,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageWindow {
    pub page_id: String,
    pub completeness: WindowCompleteness,
    pub scope: String,
    pub source: String,
    pub visible_rect: Option<Rect>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionMetadata {
    pub schema_version: String,
    pub actions: Vec<ActionAnnotation>,
    pub targets: Vec<TargetAnnotation>,
    pub fields: Vec<FieldAnnotation>,
    pub pages: Vec<PageWindow>,
}

/// IDs from the existing, independently validated resource documents.
#[derive(Debug, Clone)]
pub struct ProjectionCatalog {
    pub width: u32,
    pub height: u32,
    pub actions: BTreeSet<ActionKey>,
    pub targets: BTreeSet<String>,
    pub fields: BTreeSet<FieldKey>,
    pub pages: BTreeSet<String>,
    field_privacy: BTreeMap<FieldKey, Privacy>,
}

fn rows<'a>(value: &'a Value, key: &str) -> LabResult<&'a [Value]> {
    match value.get(key) {
        None => Ok(&[]),
        Some(Value::Array(values)) => Ok(values),
        _ => Err(invalid(format!(
            "projection reference catalog requires {key}[]"
        ))),
    }
}

fn id(value: &Value, key: &str) -> LabResult<String> {
    let value = value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(format!("projection reference requires {key}")))?;
    text(value)?;
    Ok(value.to_string())
}

fn unique<T: Ord>(set: &mut BTreeSet<T>, value: T) -> LabResult<()> {
    if !set.insert(value) {
        return Err(invalid(
            "duplicate projection reference/annotation identity",
        ));
    }
    Ok(())
}

impl ProjectionCatalog {
    pub fn add_operation_fields(&mut self, operation: &Value) -> LabResult<()> {
        let Some(declaration) = operation.get("post_admission_ocr") else {
            return Ok(());
        };
        if declaration.get("mode").and_then(Value::as_str) != Some("fields_v1") {
            return Ok(());
        }
        let task_id = id(operation, "task_id")?;
        for field in rows(declaration, "fields")? {
            let key = FieldKey {
                task_id: task_id.clone(),
                field_id: id(field, "id")?,
                target_id: id(field, "target_id")?,
            };
            if !self.targets.contains(&key.target_id) {
                return Err(invalid("projection field target is not declared"));
            }
            if self
                .fields
                .iter()
                .any(|f| f.task_id == key.task_id && f.field_id == key.field_id)
            {
                return Err(invalid("duplicate projection field identity"));
            }
            let privacy: Privacy = serde_json::from_value(
                field
                    .get("privacy")
                    .cloned()
                    .ok_or_else(|| invalid("projection field requires its operation privacy"))?,
            )
            .map_err(|e| invalid(e.to_string()))?;
            self.field_privacy.insert(key.clone(), privacy);
            unique(&mut self.fields, key)?;
        }
        Ok(())
    }
    /// Reads identities only; recognition and operation validation keep their existing owners.
    pub fn from_resources(pack: &Value, pages: &Value, navigation: &Value) -> LabResult<Self> {
        let dimension = |key| {
            pack["coordinate_space"][key]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n != 0)
                .ok_or_else(|| invalid("projection coordinate space must be positive u32"))
        };
        let mut catalog = Self {
            width: dimension("width")?,
            height: dimension("height")?,
            actions: BTreeSet::new(),
            targets: BTreeSet::new(),
            fields: BTreeSet::new(),
            pages: BTreeSet::new(),
            field_privacy: BTreeMap::new(),
        };
        for target in rows(pack, "targets")? {
            unique(&mut catalog.targets, id(target, "id")?)?;
        }
        for page in rows(pages, "pages")? {
            unique(&mut catalog.pages, id(page, "id")?)?;
        }
        for (array, role, page_key, id_key) in [
            ("navigation", ElementRole::Navigate, "from_page", "id"),
            ("page_operations", ElementRole::PageOp, "page", "id"),
            ("control_points", ElementRole::ControlPoint, "", "name"),
        ] {
            for action in rows(navigation, array)? {
                unique(
                    &mut catalog.actions,
                    ActionKey {
                        role,
                        resource_id: id(action, id_key)?,
                        task_id: if role == ElementRole::PageOp {
                            id(action, "task_id")?
                        } else {
                            String::new()
                        },
                        page: if page_key.is_empty() {
                            None
                        } else {
                            Some(id(action, page_key)?)
                        },
                    },
                )?;
            }
        }
        Ok(catalog)
    }
}

#[derive(Debug, Clone)]
pub struct VerifiedProjectionMetadata {
    declaration: ProjectionMetadata,
    catalog: ProjectionCatalog,
}

impl ProjectionMetadata {
    pub fn parse(bytes: &[u8]) -> LabResult<Self> {
        if bytes.len() > METADATA_BYTE_LIMIT {
            return Err(invalid("projection metadata exceeds 1 MiB"));
        }
        serde_json::from_slice(bytes)
            .map_err(|e| invalid(format!("invalid projection metadata: {e}")))
    }

    pub fn validate(self, catalog: ProjectionCatalog) -> LabResult<VerifiedProjectionMetadata> {
        if self.schema_version != METADATA_SCHEMA {
            return Err(invalid("unsupported projection metadata schema_version"));
        }
        if self.actions.len() + self.targets.len() + self.fields.len() + self.pages.len()
            > INPUT_ENTRY_LIMIT
        {
            return Err(invalid("projection metadata exceeds 4096 annotations"));
        }
        if serialized_len(&self)? > METADATA_BYTE_LIMIT {
            return Err(invalid("projection metadata exceeds 1 MiB"));
        }
        let mut seen = BTreeSet::new();
        for entry in &self.actions {
            unique(&mut seen, &entry.action)?;
            text(&entry.source)?;
            if !catalog.actions.contains(&entry.action)
                || entry
                    .action
                    .page
                    .as_ref()
                    .is_some_and(|p| !catalog.pages.contains(p))
            {
                return Err(invalid(
                    "projection action annotation has an unknown reference",
                ));
            }
        }
        let mut seen = BTreeSet::new();
        for entry in &self.targets {
            unique(&mut seen, &entry.target_id)?;
            text(&entry.source)?;
            if !catalog.targets.contains(&entry.target_id) {
                return Err(invalid(
                    "projection target annotation has an unknown reference",
                ));
            }
        }
        let mut seen = BTreeSet::new();
        for entry in &self.fields {
            unique(&mut seen, (&entry.field.task_id, &entry.field.field_id))?;
            text(&entry.source)?;
            if !catalog.fields.contains(&entry.field)
                || !catalog.targets.contains(&entry.field.target_id)
            {
                return Err(invalid(
                    "projection field annotation has an unknown reference",
                ));
            }
        }
        let mut seen = BTreeSet::new();
        for entry in &self.pages {
            unique(&mut seen, &entry.page_id)?;
            text(&entry.source)?;
            text(&entry.scope)?;
            if !catalog.pages.contains(&entry.page_id) {
                return Err(invalid("projection window has an unknown page"));
            }
            if entry.completeness == WindowCompleteness::Windowed && entry.visible_rect.is_none() {
                return Err(invalid("windowed projection requires a visible_rect"));
            }
            if let Some(rect) = entry.visible_rect {
                rect.validate(catalog.width, catalog.height)?;
            }
        }
        Ok(VerifiedProjectionMetadata {
            declaration: self,
            catalog,
        })
    }
}

impl VerifiedProjectionMetadata {
    pub fn unannotated(catalog: ProjectionCatalog) -> Self {
        Self {
            declaration: ProjectionMetadata {
                schema_version: METADATA_SCHEMA.to_string(),
                actions: vec![],
                targets: vec![],
                fields: vec![],
                pages: vec![],
            },
            catalog,
        }
    }

    /// Selection is permitted only after the complete source declaration passed validation.
    pub fn select(&self, catalog: ProjectionCatalog) -> LabResult<ProjectionMetadata> {
        let mut selected = self.declaration.clone();
        selected
            .actions
            .retain(|e| catalog.actions.contains(&e.action));
        selected
            .targets
            .retain(|e| catalog.targets.contains(&e.target_id));
        selected
            .fields
            .retain(|e| catalog.fields.contains(&e.field));
        selected
            .pages
            .retain(|e| catalog.pages.contains(&e.page_id));
        selected.clone().validate(catalog)?;
        Ok(selected)
    }

    pub fn target_privacy(&self, target: &str) -> Option<Privacy> {
        self.declaration
            .targets
            .iter()
            .find(|e| e.target_id == target)
            .map(|e| e.privacy)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Point {
    pub x: u32,
    pub y: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    fn validate(self, width: u32, height: u32) -> LabResult<()> {
        if self.width == 0
            || self.height == 0
            || self.x.checked_add(self.width).is_none_or(|n| n > width)
            || self.y.checked_add(self.height).is_none_or(|n| n > height)
        {
            return Err(invalid("projection rectangle is outside the current frame"));
        }
        Ok(())
    }
    fn contains(self, point: Point) -> bool {
        point.x >= self.x
            && point.y >= self.y
            && point.x < self.x + self.width
            && point.y < self.y + self.height
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Geometry {
    Tap {
        rect: Rect,
        point: Point,
    },
    Drag {
        from_rect: Rect,
        to_rect: Rect,
        from: Point,
        to: Point,
        duration_ms: u64,
    },
}

impl Geometry {
    fn validate(&self, width: u32, height: u32) -> LabResult<()> {
        let endpoint = |rect: Rect, point: Point| -> LabResult<()> {
            rect.validate(width, height)?;
            if !rect.contains(point) {
                return Err(invalid("projection point is outside its rectangle"));
            }
            Ok(())
        };
        match *self {
            Self::Tap { rect, point } => endpoint(rect, point),
            Self::Drag {
                from_rect,
                to_rect,
                from,
                to,
                duration_ms,
            } => {
                endpoint(from_rect, from)?;
                endpoint(to_rect, to)?;
                if duration_ms == 0 || duration_ms > 600_000 {
                    return Err(invalid("projection drag duration is out of bounds"));
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FrameIdentity {
    /// The digest covers decoded row-major RGB8 bytes or a separately verified frame artifact.
    pub kind: FrameKind,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameKind {
    Rgb8,
    Artifact,
}

#[derive(Debug, Clone)]
pub enum ElementResolution {
    Declared(Geometry),
    Target {
        target_id: String,
        passed: bool,
        geometry: Option<Geometry>,
    },
    NotEvaluated {
        target_id: String,
        reason: NotEvaluatedReason,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum NotEvaluatedReason {
    DynamicTarget,
    ClickOnly,
    Unscoped,
}

#[derive(Debug, Clone)]
pub struct ElementInput {
    pub action: ActionKey,
    pub purpose: String,
    pub source: String,
    pub resolution: ElementResolution,
}

#[derive(Debug, Clone)]
pub struct MissingTarget {
    pub passed: bool,
    pub id: String,
    pub role: String,
    pub group_index: Option<usize>,
    pub group_satisfied: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct FieldInput {
    pub target_id: String,
    pub field: Option<FieldKey>,
    pub parsed: bool,
    pub raw_text: Option<String>,
    pub value: Option<Value>,
    pub detail: Option<String>,
    /// Any personal classification already carried by the recognition result is restrictive.
    pub privacy: Option<Privacy>,
}

pub struct ProjectionInput {
    pub frame: FrameIdentity,
    pub matched_pages: Vec<String>,
    pub elements: Vec<ElementInput>,
    pub missing: Vec<MissingTarget>,
    pub fields: Vec<FieldInput>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageProjection {
    pub schema_version: &'static str,
    pub page: String,
    pub state: &'static str,
    pub matched: bool,
    pub standby: bool,
    pub frame: FrameIdentity,
    pub elements: Vec<Value>,
    pub unscoped_controls: Vec<Value>,
    pub missing: Vec<Value>,
    pub fields: Vec<Value>,
    pub truncated: bool,
    pub output_truncated: bool,
    pub omitted_count: usize,
    pub page_window_completeness: WindowCompleteness,
    pub window: Option<PageWindow>,
    pub metrics: ProjectionMetrics,
    pub content_sha256: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ProjectionMetrics {
    pub sample_scope: &'static str,
    pub matched_page_count: usize,
    pub recognized_count: usize,
    pub missing_count: usize,
    pub unscoped_control_count: usize,
    pub entry_count: usize,
    pub emitted_count: usize,
    pub omitted_count: usize,
    pub empty_list: bool,
}

fn serialized_len(value: &impl Serialize) -> LabResult<usize> {
    serde_json::to_vec(value)
        .map(|v| v.len())
        .map_err(|e| invalid(e.to_string()))
}

impl PageProjection {
    fn refresh(&mut self) -> LabResult<()> {
        self.metrics.emitted_count = self.elements.len()
            + self.missing.len()
            + self.unscoped_controls.len()
            + self.fields.len();
        self.omitted_count = self.metrics.entry_count - self.metrics.emitted_count;
        self.metrics.omitted_count = self.omitted_count;
        self.output_truncated = self.omitted_count != 0;
        self.truncated =
            self.output_truncated || self.page_window_completeness == WindowCompleteness::Windowed;
        self.metrics.empty_list = self.metrics.recognized_count == 0;
        let mut content = serde_json::to_value(&self).map_err(|e| invalid(e.to_string()))?;
        content
            .as_object_mut()
            .ok_or_else(|| invalid("projection is not an object"))?
            .remove("content_sha256");
        self.content_sha256 = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&content).map_err(|e| invalid(e.to_string()))?)
        );
        Ok(())
    }

    /// Transport adapters may request a smaller envelope budget without editing projection semantics.
    pub fn fit_byte_limit(&mut self, limit: usize) -> LabResult<()> {
        self.refresh()?;
        while serialized_len(self)? > limit.min(BYTE_LIMIT) {
            if self
                .fields
                .pop()
                .or_else(|| self.missing.pop())
                .or_else(|| self.unscoped_controls.pop())
                .or_else(|| self.elements.pop())
                .is_none()
            {
                return Err(invalid("projection identity exceeds its byte limit"));
            }
            self.refresh()?;
        }
        Ok(())
    }

    fn append(&mut self, kind: &str, value: Value) -> LabResult<()> {
        self.metrics.entry_count += 1;
        let entries = match kind {
            "element" => {
                self.metrics.recognized_count += 1;
                &mut self.elements
            }
            "control" => {
                self.metrics.unscoped_control_count += 1;
                &mut self.unscoped_controls
            }
            "missing" => {
                self.metrics.missing_count += 1;
                &mut self.missing
            }
            _ => &mut self.fields,
        };
        if self.metrics.emitted_count < ENTRY_LIMIT {
            entries.push(value);
        }
        self.fit_byte_limit(BYTE_LIMIT)
    }
}

pub fn project(
    input: ProjectionInput,
    metadata: &VerifiedProjectionMetadata,
) -> LabResult<PageProjection> {
    if input.frame.sha256.len() != 64
        || !input.frame.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        || input.frame.width != metadata.catalog.width
        || input.frame.height != metadata.catalog.height
    {
        return Err(invalid(
            "projection requires the matching frame identity and coordinate space",
        ));
    }
    if input.elements.len() + input.missing.len() + input.fields.len() > INPUT_ENTRY_LIMIT {
        return Err(invalid("projection exceeds 4096 input facts"));
    }
    let matched = input.matched_pages.len() == 1;
    let page = if matched {
        input.matched_pages[0].clone()
    } else {
        "unknown".to_string()
    };
    for page in &input.matched_pages {
        if !metadata.catalog.pages.contains(page) {
            return Err(invalid("projection page is not declared"));
        }
    }
    let window = metadata
        .declaration
        .pages
        .iter()
        .find(|p| matched && p.page_id == page)
        .cloned();
    let sample_scope = match input.frame.kind {
        FrameKind::Rgb8 => "single_offline_observation",
        FrameKind::Artifact => "single_frame",
    };
    let mut output = PageProjection {
        schema_version: PROJECTION_SCHEMA,
        page,
        state: if matched {
            "recognized"
        } else if input.matched_pages.is_empty() {
            "unknown"
        } else {
            "conflict"
        },
        matched,
        standby: !matched,
        frame: input.frame,
        elements: vec![],
        unscoped_controls: vec![],
        missing: vec![],
        fields: vec![],
        truncated: false,
        output_truncated: false,
        omitted_count: 0,
        page_window_completeness: window.as_ref().map(|w| w.completeness).unwrap_or_default(),
        window,
        metrics: ProjectionMetrics {
            sample_scope,
            matched_page_count: input.matched_pages.len(),
            ..ProjectionMetrics::default()
        },
        content_sha256: String::new(),
    };
    output.fit_byte_limit(BYTE_LIMIT)?;
    if !matched {
        return Ok(output);
    }
    let mut ids = BTreeSet::new();
    for element in input.elements {
        let action = &element.action;
        if action.page.as_ref().is_some_and(|p| p != &output.page) {
            return Err(invalid("projection element belongs to another page"));
        }
        if !metadata.catalog.actions.contains(action) {
            return Err(invalid("projection action is not declared"));
        }
        unique(&mut ids, action.clone())?;
        let control = action.role == ElementRole::ControlPoint;
        let (recognized, target, basis, mut reason, mut geometry) = match element.resolution {
            ElementResolution::Declared(geometry) => (
                !control,
                None,
                "matched_page_declaration",
                None,
                Some(geometry),
            ),
            ElementResolution::Target {
                target_id,
                passed,
                geometry,
            } => {
                let reason = if !passed {
                    Some("target_not_matched")
                } else if geometry.is_none() {
                    Some("matched_template_rect_unavailable")
                } else {
                    None
                };
                (
                    passed && !control,
                    Some(target_id),
                    "target_evaluation",
                    reason,
                    if passed && !control { geometry } else { None },
                )
            }
            ElementResolution::NotEvaluated { target_id, reason } => (
                false,
                Some(target_id),
                "not_evaluated",
                Some(match reason {
                    NotEvaluatedReason::DynamicTarget => "dynamic_target_not_evaluated",
                    NotEvaluatedReason::ClickOnly => "target_not_recognizable",
                    NotEvaluatedReason::Unscoped => "target_not_evaluated",
                }),
                None,
            ),
        };
        if target
            .as_ref()
            .is_some_and(|id| !metadata.catalog.targets.contains(id))
        {
            return Err(invalid("projection target is not declared"));
        }
        if let Some(resolved) = &geometry
            && resolved
                .validate(output.frame.width, output.frame.height)
                .is_err()
        {
            reason = Some("invalid_frame_geometry");
            geometry = None;
        }
        let annotation = metadata
            .declaration
            .actions
            .iter()
            .find(|a| &a.action == action);
        let label = if element.purpose.is_empty() {
            &action.resource_id
        } else {
            &element.purpose
        };
        let identity = serde_json::to_string(&(
            action.role,
            &action.task_id,
            &element.source,
            &action.resource_id,
        ))
        .map_err(|e| invalid(e.to_string()))?;
        let entry = json!({
            "id": identity, "resource_id": action.resource_id, "task_id": action.task_id,
            "role": action.role, "source": element.source, "purpose": label, "label": label,
            "scope": if control { "unscoped" } else { "page" }, "recognized": recognized,
            "target_id": target, "recognition_basis": basis,
            "availability": if control { "unknown" } else if recognized { "available" } else { "unavailable" },
            "actionable": geometry.is_some(), "blocked_reason": reason,
            "safety": annotation.map(|a| a.safety).unwrap_or(Safety::Unclassified),
            "safety_source": annotation.map(|a| &a.source), "input": geometry,
        });
        output.append(
            if control {
                "control"
            } else if recognized {
                "element"
            } else {
                "missing"
            },
            entry,
        )?;
    }
    let mut missing_ids = BTreeSet::new();
    for missing in input.missing {
        if missing.passed || missing.role == "forbidden" {
            continue;
        }
        if !metadata.catalog.targets.contains(&missing.id)
            || !matches!(missing.role.as_str(), "required" | "optional" | "any_of")
        {
            return Err(invalid("invalid missing projection target/role"));
        }
        unique(
            &mut missing_ids,
            (
                missing.id.clone(),
                missing.role.clone(),
                missing.group_index,
            ),
        )?;
        output.append("missing", json!({"id":missing.id,"role":missing.role,"group_index":missing.group_index,
            "group_satisfied":missing.group_satisfied,"recognized":false,"reason":"target_not_matched"}))?;
    }
    let mut field_ids = BTreeSet::new();
    for field in input.fields {
        unique(
            &mut field_ids,
            (field.target_id.clone(), field.field.clone()),
        )?;
        if !metadata.catalog.targets.contains(&field.target_id)
            || field.field.as_ref().is_some_and(|f| {
                f.target_id != field.target_id || !metadata.catalog.fields.contains(f)
            })
        {
            return Err(invalid("projection value target/field is not declared"));
        }
        let target_privacy = metadata.target_privacy(&field.target_id);
        let field_privacy = field.field.as_ref().and_then(|f| {
            metadata
                .declaration
                .fields
                .iter()
                .find(|a| &a.field == f)
                .map(|a| a.privacy)
        });
        let operation_privacy = field
            .field
            .as_ref()
            .and_then(|f| metadata.catalog.field_privacy.get(f).copied());
        let personal = target_privacy == Some(Privacy::Personal)
            || field_privacy == Some(Privacy::Personal)
            || operation_privacy == Some(Privacy::Personal)
            || field.privacy == Some(Privacy::Personal);
        let public = !personal
            && target_privacy == Some(Privacy::Public)
            && (field.field.is_none()
                || (field_privacy == Some(Privacy::Public)
                    && operation_privacy == Some(Privacy::Public)));
        output.append("field", json!({"target_id":field.target_id,"field":field.field,"parsed":field.parsed,"redacted":!public,
            "privacy":if personal { Some(Privacy::Personal) } else if public { Some(Privacy::Public) } else { None },
            "raw_text":if public { field.raw_text } else { None }, "value":if public { field.value } else { None },
            "detail":if public { field.detail } else { None }}))?;
    }
    output.fit_byte_limit(BYTE_LIMIT)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> ProjectionCatalog {
        let mut catalog = ProjectionCatalog::from_resources(
            &json!({"coordinate_space":{"width":100,"height":100},"targets":[{"id":"anchor"},{"id":"value"}]}),
            &json!({"pages":[{"id":"home"},{"id":"other"}]}),
            &json!({"navigation":[{"id":"open","from_page":"home"}],"page_operations":[{"id":"claim","task_id":"task","page":"home"}],"control_points":[{"name":"back"}]}),
        ).unwrap();
        catalog.add_operation_fields(&json!({"task_id":"task","post_admission_ocr":{"mode":"fields_v1","fields":[{"id":"amount","target_id":"value","privacy":"public"}]}})).unwrap();
        catalog
    }

    fn declaration() -> ProjectionMetadata {
        ProjectionMetadata::parse(br#"{"schema_version":"actingcommand.page-projection-metadata.v1","actions":[{"action":{"role":"page_op","task_id":"task","resource_id":"claim","page":"home"},"source":"neutral/spec"}],"targets":[{"target_id":"value","privacy":"public","source":"neutral/spec"}],"fields":[{"field":{"task_id":"task","field_id":"amount","target_id":"value"},"privacy":"public","source":"neutral/spec"}],"pages":[]}"#).unwrap()
    }

    fn input() -> ProjectionInput {
        ProjectionInput {
            frame: FrameIdentity {
                kind: FrameKind::Rgb8,
                sha256: "a".repeat(64),
                width: 100,
                height: 100,
            },
            matched_pages: vec!["home".to_string()],
            elements: vec![],
            missing: vec![],
            fields: vec![],
        }
    }

    #[test]
    fn page_projection_metadata_versions_references_conflicts_and_bounds() {
        let valid = declaration();
        assert_eq!(valid.actions[0].safety, Safety::Dangerous);
        valid.clone().validate(catalog()).unwrap();
        let mut bad = valid.clone();
        bad.schema_version = "v2".to_string();
        assert!(bad.validate(catalog()).is_err());
        let mut bad = valid.clone();
        bad.actions.push(bad.actions[0].clone());
        assert!(bad.validate(catalog()).is_err());
        let mut bad = valid.clone();
        bad.targets[0].target_id = "missing".to_string();
        assert!(bad.validate(catalog()).is_err());
        let mut bad = valid.clone();
        bad.fields[0].field.field_id = "missing".to_string();
        assert!(bad.validate(catalog()).is_err());
        let mut bad = valid.clone();
        bad.actions[0].source.clear();
        assert!(bad.validate(catalog()).is_err());
        let mut bad = serde_json::to_value(&valid).unwrap();
        bad["extra"] = json!(true);
        assert!(ProjectionMetadata::parse(&serde_json::to_vec(&bad).unwrap()).is_err());
        let mut bad = valid.clone();
        bad.pages.push(PageWindow {
            page_id: "home".into(),
            completeness: WindowCompleteness::Windowed,
            scope: "visible list".into(),
            source: "neutral/spec".into(),
            visible_rect: Some(Rect {
                x: 99,
                y: 0,
                width: 2,
                height: 1,
            }),
        });
        assert!(bad.validate(catalog()).is_err());
        let mut selected = catalog();
        selected.fields.clear();
        selected.actions.clear();
        let selected = valid.validate(catalog()).unwrap().select(selected).unwrap();
        assert!(selected.actions.is_empty() && selected.fields.is_empty());
    }

    #[test]
    fn page_projection_elements_missing_and_unconfirmed_pages_use_one_semantics() {
        let catalog = catalog();
        let mut facts = input();
        for action in &catalog.actions {
            facts.elements.push(ElementInput {
                action: action.clone(),
                purpose: if action.role == ElementRole::PageOp {
                    "Collect visible item".into()
                } else {
                    String::new()
                },
                source: "neutral".into(),
                resolution: ElementResolution::Declared(Geometry::Tap {
                    rect: Rect {
                        x: 2,
                        y: 3,
                        width: 4,
                        height: 5,
                    },
                    point: Point { x: 3, y: 4 },
                }),
            });
        }
        facts.missing.push(MissingTarget {
            id: "anchor".into(),
            role: "any_of".into(),
            passed: false,
            group_index: Some(0),
            group_satisfied: Some(true),
        });
        facts.missing.push(MissingTarget {
            id: "anchor".into(),
            role: "forbidden".into(),
            passed: false,
            group_index: None,
            group_satisfied: None,
        });
        let metadata = declaration().validate(catalog.clone()).unwrap();
        let projected = project(facts, &metadata).unwrap();
        assert_eq!(projected.elements.len(), 2);
        assert_eq!(projected.elements[1]["purpose"], "Collect visible item");
        assert_eq!(projected.elements[1]["safety"], "dangerous");
        assert_eq!(
            projected.elements[0]["input"]["point"],
            json!({"x":3,"y":4})
        );
        assert_eq!(projected.unscoped_controls[0]["availability"], "unknown");
        assert_eq!(projected.missing.len(), 1);
        assert_eq!(projected.missing[0]["group_satisfied"], true);
        assert_eq!(
            projected.page_window_completeness,
            WindowCompleteness::Unknown
        );
        for pages in [vec![], vec!["home".into(), "other".into()]] {
            let mut facts = input();
            facts.matched_pages = pages;
            let projected = project(facts, &metadata).unwrap();
            assert!(!projected.matched && projected.elements.is_empty());
        }
        let mut facts = input();
        facts.elements.push(ElementInput {
            action: catalog.actions.first().unwrap().clone(),
            purpose: String::new(),
            source: "neutral".into(),
            resolution: ElementResolution::Target {
                target_id: "anchor".into(),
                passed: false,
                geometry: Some(Geometry::Tap {
                    rect: Rect {
                        x: 0,
                        y: 0,
                        width: 1,
                        height: 1,
                    },
                    point: Point { x: 0, y: 0 },
                }),
            },
        });
        let projected = project(facts, &VerifiedProjectionMetadata::unannotated(catalog)).unwrap();
        assert!(projected.elements.is_empty());
        assert!(projected.missing[0]["input"].is_null());
        assert_eq!(projected.missing[0]["safety"], "unclassified");
    }

    #[test]
    fn page_projection_private_and_unmarked_values_never_escape() {
        for mode in 0..5 {
            let mut declaration = declaration();
            if mode == 1 {
                declaration.targets[0].privacy = Privacy::Personal;
            }
            if mode == 2 {
                declaration.fields[0].privacy = Privacy::Personal;
            }
            if mode == 3 {
                declaration.targets.clear();
            }
            if mode == 4 {
                declaration.fields.clear();
            }
            let metadata = declaration.validate(catalog()).unwrap();
            let mut facts = input();
            facts.fields.push(FieldInput {
                target_id: "value".into(),
                field: Some(FieldKey {
                    task_id: "task".into(),
                    field_id: "amount".into(),
                    target_id: "value".into(),
                }),
                parsed: true,
                raw_text: Some("private text".into()),
                value: Some(json!("private value")),
                detail: Some("private detail".into()),
                privacy: None,
            });
            let projected = project(facts, &metadata).unwrap();
            assert_eq!(projected.fields[0]["redacted"], mode != 0);
            if mode != 0 {
                assert!(
                    !serde_json::to_string(&projected)
                        .unwrap()
                        .contains("private ")
                );
            }
        }
        let metadata = declaration().validate(catalog()).unwrap();
        let mut facts = input();
        facts.fields.push(FieldInput {
            target_id: "value".into(),
            field: None,
            parsed: true,
            raw_text: Some("private text".into()),
            value: None,
            detail: Some("private detail".into()),
            privacy: Some(Privacy::Personal),
        });
        let projected = project(facts, &metadata).unwrap();
        assert!(
            !serde_json::to_string(&projected)
                .unwrap()
                .contains("private ")
        );
    }

    #[test]
    fn page_projection_window_budget_and_hash_are_explicit() {
        let mut declaration = declaration();
        declaration.pages.push(PageWindow {
            page_id: "home".into(),
            completeness: WindowCompleteness::Windowed,
            scope: "visible rows".into(),
            source: "neutral/spec".into(),
            visible_rect: Some(Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            }),
        });
        let metadata = declaration.validate(catalog()).unwrap();
        let projected = project(input(), &metadata).unwrap();
        assert!(projected.truncated && !projected.output_truncated);
        assert_eq!(projected.omitted_count, 0);
        assert_eq!(
            projected.content_sha256,
            project(input(), &metadata).unwrap().content_sha256
        );
        let mut value = serde_json::to_value(&projected).unwrap();
        value.as_object_mut().unwrap().remove("content_sha256");
        assert_eq!(
            projected.content_sha256,
            format!("{:x}", Sha256::digest(serde_json::to_vec(&value).unwrap()))
        );
        let mut catalog = catalog();
        let mut facts = input();
        for n in 0..80 {
            let action = ActionKey {
                role: ElementRole::PageOp,
                task_id: "task".into(),
                resource_id: format!("op{n}"),
                page: Some("home".into()),
            };
            catalog.actions.insert(action.clone());
            facts.elements.push(ElementInput {
                action,
                purpose: if n == 0 {
                    "x".repeat(BYTE_LIMIT + 1)
                } else {
                    "visible".into()
                },
                source: "neutral".into(),
                resolution: ElementResolution::Declared(Geometry::Tap {
                    rect: Rect {
                        x: 0,
                        y: 0,
                        width: 1,
                        height: 1,
                    },
                    point: Point { x: 0, y: 0 },
                }),
            });
        }
        let mut projected =
            project(facts, &VerifiedProjectionMetadata::unannotated(catalog)).unwrap();
        assert!(projected.output_truncated);
        assert!(projected.elements.len() <= ENTRY_LIMIT);
        assert!(serialized_len(&projected).unwrap() <= BYTE_LIMIT);
        assert_eq!(projected.omitted_count, 80 - projected.elements.len());
        projected.fit_byte_limit(1800).unwrap();
        assert!(serialized_len(&projected).unwrap() <= 1800);
        assert!(projected.fit_byte_limit(1).is_err());
        let mut oversized_catalog = metadata.catalog.clone();
        oversized_catalog.pages.insert("p".repeat(BYTE_LIMIT));
        let mut facts = input();
        facts.matched_pages = vec!["p".repeat(BYTE_LIMIT)];
        assert!(
            project(
                facts,
                &VerifiedProjectionMetadata::unannotated(oversized_catalog)
            )
            .is_err()
        );
    }
}
