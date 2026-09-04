// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_lab::{CurrentPageRequest, ReadonlyRecognitionInput, RecoveryHintResponse};
use actingcommand_page_detector::{PageTargetEvaluation, PageTargetRole};
use serde::Serialize;

const ENTRY_LIMIT: usize = 64;
const BYTE_LIMIT: usize = 32 * 1024;

pub(super) fn detect(
    resources: std::sync::Arc<actingcommand_lab::ExternallyVerifiedBundle>,
    scene: &Scene,
) -> CliOutcome<(PageDetectionOutcome, Option<RecoveryHintResponse>)> {
    let mut lab = super::super::env_detection::build_readonly_lab()?;
    let response = lab.current_page(CurrentPageRequest {
        input: ReadonlyRecognitionInput {
            resources,
            scene: Some(scene.clone()),
            scene_path: None,
            capture_config: None,
            require_fresh: false,
            fresh_delay: Duration::ZERO,
        },
    })?;
    let evaluations = response
        .evaluations
        .into_iter()
        .map(|page| {
            let targets = page
                .targets
                .into_iter()
                .map(|target| {
                    let role = match target.role.as_str() {
                        "Required" => PageTargetRole::Required,
                        "AnyOf" => PageTargetRole::AnyOf,
                        "Optional" => PageTargetRole::Optional,
                        "Forbidden" => PageTargetRole::Forbidden,
                        other => {
                            return Err(CliError::device(format!(
                                "unknown readonly target role: {other}"
                            )));
                        }
                    };
                    Ok(PageTargetEvaluation {
                        target_id: target.id,
                        role,
                        passed: target.passed,
                        message: target.message,
                    })
                })
                .collect::<CliOutcome<Vec<_>>>()?;
            let count = |role: PageTargetRole, passed_only: bool| {
                targets
                    .iter()
                    .filter(|target| target.role == role && (!passed_only || target.passed))
                    .count()
            };
            Ok(PageEvaluation {
                page_id: page.page,
                matched: page.matched,
                message: page.message,
                required_total: count(PageTargetRole::Required, false),
                required_passed: count(PageTargetRole::Required, true),
                optional_total: count(PageTargetRole::Optional, false),
                optional_passed: count(PageTargetRole::Optional, true),
                forbidden_total: count(PageTargetRole::Forbidden, false),
                forbidden_passed: count(PageTargetRole::Forbidden, true),
                any_of_total: page.any_of_total,
                any_of_passed: page.any_of_passed,
                target_results: targets,
            })
        })
        .collect::<CliOutcome<Vec<_>>>()?;
    Ok((
        PageDetectionOutcome {
            page: response.page,
            matched: response.matched,
            standby: response.standby,
            evaluations,
        },
        response.recovery_hint,
    ))
}

#[derive(Serialize)]
pub(super) struct Observation {
    schema_version: &'static str,
    page: String,
    state: &'static str,
    matched: bool,
    standby: bool,
    elements: Vec<Value>,
    unscoped_controls: Vec<Value>,
    missing: Vec<Value>,
    truncated: bool,
    omitted_count: usize,
    page_window_completeness: &'static str,
    metrics: Metrics,
}

#[derive(Serialize, Default)]
struct Metrics {
    sample_scope: &'static str,
    matched_page_count: usize,
    recognized_count: usize,
    missing_count: usize,
    unscoped_control_count: usize,
    entry_count: usize,
    emitted_count: usize,
    omitted_count: usize,
    empty_list: bool,
}

#[derive(Clone, Copy)]
enum EntryKind {
    Element,
    Control,
    Missing,
}

impl Observation {
    fn entries(&mut self, kind: EntryKind) -> &mut Vec<Value> {
        match kind {
            EntryKind::Element => &mut self.elements,
            EntryKind::Control => &mut self.unscoped_controls,
            EntryKind::Missing => &mut self.missing,
        }
    }

    fn refresh_counts(&mut self) {
        self.metrics.emitted_count =
            self.elements.len() + self.unscoped_controls.len() + self.missing.len();
        self.omitted_count = self.metrics.entry_count - self.metrics.emitted_count;
        self.metrics.omitted_count = self.omitted_count;
        self.truncated = self.omitted_count != 0;
        self.metrics.empty_list = self.metrics.recognized_count == 0;
    }

    fn append(&mut self, kind: EntryKind, entry: Value) -> CliOutcome<()> {
        self.metrics.entry_count += 1;
        match kind {
            EntryKind::Element => self.metrics.recognized_count += 1,
            EntryKind::Control => self.metrics.unscoped_control_count += 1,
            EntryKind::Missing => self.metrics.missing_count += 1,
        }
        if self.metrics.emitted_count < ENTRY_LIMIT {
            self.entries(kind).push(entry);
            self.refresh_counts();
            if serialized_len(self)? > BYTE_LIMIT {
                self.entries(kind).pop();
            }
        }
        self.refresh_counts();
        Ok(())
    }

    fn omit_last(&mut self) -> bool {
        let removed = self
            .missing
            .pop()
            .or_else(|| self.unscoped_controls.pop())
            .or_else(|| self.elements.pop())
            .is_some();
        self.refresh_counts();
        removed
    }
}

fn serialized_len(value: &impl Serialize) -> CliOutcome<usize> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .map_err(|error| CliError::device(format!("observation serialization failed: {error}")))
}

pub(super) fn build(
    view: &super::super::contained_resources::ObservationResources,
    evaluator: &RecognitionEvaluator,
    scene: &Scene,
    outcome: &PageDetectionOutcome,
) -> CliOutcome<Observation> {
    let mut observation = Observation {
        schema_version: "actingcommand.lab.offline_observation.v1",
        page: if outcome.matched {
            outcome.page.clone()
        } else {
            "unknown".to_string()
        },
        state: if outcome.matched {
            "recognized"
        } else {
            "unknown"
        },
        matched: outcome.matched,
        standby: outcome.standby,
        elements: Vec::new(),
        unscoped_controls: Vec::new(),
        missing: Vec::new(),
        truncated: false,
        omitted_count: 0,
        page_window_completeness: "unknown",
        metrics: Metrics {
            sample_scope: "single_offline_observation",
            matched_page_count: usize::from(outcome.matched),
            ..Metrics::default()
        },
    };
    let mut ids = BTreeSet::new();
    if outcome.matched {
        for edge in view
            .edges
            .iter()
            .filter(|edge| edge.from_page == outcome.page)
        {
            let entry = element(
                (
                    "navigate",
                    "",
                    &edge.id,
                    &edge.id,
                    edge.source.as_deref().unwrap_or("navigation"),
                ),
                &edge.input,
                evaluator,
                scene,
            )?;
            append_element(&mut observation, &mut ids, entry)?;
        }
        for operation in view
            .operations
            .iter()
            .filter(|operation| operation.page == outcome.page)
        {
            let label = if operation.purpose.is_empty() {
                &operation.id
            } else {
                &operation.purpose
            };
            let entry = element(
                (
                    "page_op",
                    &operation.task_id,
                    &operation.id,
                    label,
                    "page_operations",
                ),
                &operation.input,
                evaluator,
                scene,
            )?;
            append_element(&mut observation, &mut ids, entry)?;
        }
    }
    for control in &view.controls {
        let id = qualified_id("control_point", "", &control.name, "control_points")?;
        if !ids.insert(id.clone()) {
            return Err(CliError::package_invalid(format!(
                "duplicate observation element: {id}"
            )));
        }
        let actionable = !matches!(control.input, SemanticInput::TargetCenter { .. });
        observation.append(EntryKind::Control, json!({
            "id": id, "resource_id": control.name, "role": "control_point",
            "label": control.name, "source": "control_points", "scope": "unscoped",
            "recognized": false, "availability": "unknown", "actionable": actionable,
            "blocked_reason": if actionable { Value::Null } else { json!("target_not_evaluated") }, "safety": "unclassified",
            "input": semantic_input_json(&control.input)
        }))?;
    }
    if let Some(page) = outcome
        .evaluations
        .iter()
        .find(|page| outcome.matched && page.page_id == outcome.page)
    {
        let definition = view
            .pages
            .pages
            .iter()
            .find(|definition| definition.id == page.page_id)
            .ok_or_else(|| CliError::package_invalid("matched page definition is unavailable"))?;
        for target in page
            .target_results
            .iter()
            .filter(|target| !target.passed && target.role != PageTargetRole::Forbidden)
        {
            let (role, group) = match target.role {
                PageTargetRole::Required => ("required", None),
                PageTargetRole::Optional => ("optional", None),
                PageTargetRole::AnyOf => (
                    "any_of",
                    definition
                        .any_of
                        .iter()
                        .position(|group| group.contains(&target.target_id)),
                ),
                PageTargetRole::Forbidden => unreachable!(),
            };
            let group_passed = group.map(|index| {
                definition.any_of[index].iter().any(|id| {
                    page.target_results
                        .iter()
                        .any(|target| &target.target_id == id && target.passed)
                })
            });
            observation.append(
                EntryKind::Missing,
                json!({
                    "id": target.target_id, "role": role, "group_index": group,
                    "group_satisfied": group_passed, "recognized": false,
                    "reason": "target_not_matched"
                }),
            )?;
        }
    }
    observation.refresh_counts();
    while serialized_len(&observation)? > BYTE_LIMIT {
        if !observation.omit_last() {
            return Err(CliError::package_invalid(
                "observation identity exceeds its byte limit",
            ));
        }
    }
    Ok(observation)
}

fn qualified_id(role: &str, task: &str, id: &str, source: &str) -> CliOutcome<String> {
    serde_json::to_string(&(role, task, source, id)).map_err(|error| {
        CliError::device(format!(
            "observation identity serialization failed: {error}"
        ))
    })
}

fn append_element(
    observation: &mut Observation,
    ids: &mut BTreeSet<String>,
    entry: Value,
) -> CliOutcome<()> {
    let id = entry["id"]
        .as_str()
        .ok_or_else(|| CliError::device("observation element has no identity"))?;
    if !ids.insert(id.to_string()) {
        return Err(CliError::package_invalid(format!(
            "duplicate observation element: {id}"
        )));
    }
    observation.append(
        if entry["recognized"] == true {
            EntryKind::Element
        } else {
            EntryKind::Missing
        },
        entry,
    )
}

fn element(
    identity: (&str, &str, &str, &str, &str),
    input: &SemanticInput,
    evaluator: &RecognitionEvaluator,
    scene: &Scene,
) -> CliOutcome<Value> {
    let (role, task, id, label, source) = identity;
    let (recognized, actionable, reason, geometry) = match input {
        SemanticInput::TargetCenter { target_id } => {
            match evaluator
                .target_kind(target_id)
                .map_err(|error| CliError::package_invalid(error.to_string()))?
            {
                TargetKind::Template | TargetKind::Color => {
                    let evaluated = evaluator
                        .evaluate_target(scene, target_id)
                        .map_err(|error| CliError::usage(error.to_string()))?;
                    if !evaluated.passed {
                        (false, false, Some("target_not_matched"), Value::Null)
                    } else if evaluated.template.is_some() {
                        let rect = target_evaluation_rect(&evaluated)?;
                        (
                            true,
                            true,
                            None,
                            semantic_input_json(&SemanticInput::Tap {
                                rect,
                                point: rect_center(rect)?,
                            }),
                        )
                    } else {
                        (
                            true,
                            false,
                            Some("matched_template_rect_unavailable"),
                            Value::Null,
                        )
                    }
                }
                TargetKind::Ocr | TargetKind::Nn => (
                    false,
                    false,
                    Some("dynamic_target_not_evaluated"),
                    Value::Null,
                ),
                TargetKind::ClickOnly => {
                    (false, false, Some("target_not_recognizable"), Value::Null)
                }
            }
        }
        _ => (true, true, None, semantic_input_json(input)),
    };
    Ok(json!({
        "id": qualified_id(role, task, id, source)?, "resource_id": id,
        "task_id": task, "source": source, "label": label, "role": role,
        "recognized": recognized,
        "target_id": match input { SemanticInput::TargetCenter { target_id } => Some(target_id), _ => None },
        "recognition_basis": if matches!(reason, Some("dynamic_target_not_evaluated" | "target_not_recognizable")) { "not_evaluated" } else if matches!(input, SemanticInput::TargetCenter { .. }) { "target_evaluation" } else { "matched_page_declaration" },
        "availability": if recognized { "available" } else { "unavailable" },
        "actionable": actionable, "blocked_reason": reason,
        "safety": "unclassified", "input": geometry
    }))
}

pub(super) fn project(
    mut payload: Value,
    mut observation: Observation,
    flags: &FlagArgs,
) -> CliOutcome<Value> {
    let mut request =
        lab2_projection_request(flags, payload["req_id"].as_str().map(str::to_string));
    request.fields.extend(
        [
            "observation",
            "matched",
            "candidates",
            "recovery_hint",
            "frame_source",
            "frame_path",
        ]
        .map(str::to_string),
    );
    loop {
        payload["observation"] = serde_json::to_value(&observation).map_err(|error| {
            CliError::device(format!("observation serialization failed: {error}"))
        })?;
        let projected = project_record(&payload, &request)
            .map_err(|error| CliError::device(error.to_string()))?;
        if flags.bool("--verbose")
            || flags.bool("--pretty")
            || serialized_len(&projected)? <= actingcommand_ledger::MIN_PROJECTION_HARD_LIMIT_BYTES
        {
            return Ok(projected);
        }
        if !observation.omit_last() {
            return Err(CliError::package_invalid(
                "observation result exceeds the Min byte limit without removable entries",
            ));
        }
    }
}
