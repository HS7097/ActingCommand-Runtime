// SPDX-License-Identifier: AGPL-3.0-only

//! Convert verified Lab records and admitted source declarations into an authoring draft.

use crate::{
    AuthoringDraft, AuthoringFile, AuthoringProvenance, AuthoringWriteMode,
    DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES, canonical_game, canonical_locale, canonical_server,
};
use actingcommand_contract::page_projection::{Geometry, Privacy};
use actingcommand_contract::{
    ContainedLabOperationResult, EffectDisposition, EventType, LabError, LabOperationStage,
    LabResult, TerminalEvent,
};
use actingcommand_pack_containment::LoadedBundle;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

pub struct ResourceRestoreRequest {
    pub task_id: String,
    pub through_sequence: u64,
    pub entry_page: Option<String>,
    pub target_pages: Vec<String>,
    pub goal: Option<String>,
}

pub struct ResourceRestoreRecord {
    pub operation: ContainedLabOperationResult,
    pub terminal: TerminalEvent,
    pub input_event_type: Option<EventType>,
}

pub struct ResourceRestoreDraft {
    pub draft: AuthoringDraft,
    pub report: Value,
}

pub fn restore_authoring_draft(
    package: &LoadedBundle,
    records: &[ResourceRestoreRecord],
    missing_records: &[Value],
    request: &ResourceRestoreRequest,
) -> LabResult<ResourceRestoreDraft> {
    if request.task_id.is_empty()
        || request.task_id.len() > 128
        || !request
            .task_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        || request.through_sequence == 0
        || !(1..=32).contains(&(records.len() + missing_records.len()))
        || request
            .goal
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 4096)
    {
        return Err(invalid("invalid authoring identity, goal or record count"));
    }
    let pack = read_entry_json(
        package,
        package
            .recognition_pack_path()
            .ok_or_else(|| invalid("source package has no recognition declaration"))?,
    )?;
    let pages_document = read_entry_json(
        package,
        package
            .pages_path()
            .ok_or_else(|| invalid("source package has no page declaration"))?,
    )?;
    let game = canonical_game(text(&pack, "game")?)?;
    let server = canonical_server(text(&pack, "server")?)?;
    let locale = canonical_locale(text(&pack, "locale")?)?;
    let coordinate_space = pack
        .get("coordinate_space")
        .cloned()
        .ok_or_else(|| invalid("source coordinate space is missing"))?;
    let source_prefix = format!("{}/operations/", package.resource_root());
    let mut sources = Vec::new();
    for path in package
        .entry_paths()
        .filter(|path| path.starts_with(&source_prefix) && path.ends_with("/task.json"))
    {
        let source = read_entry_json(package, path)?;
        for key in [
            "anchors",
            "color_probes",
            "verify_templates",
            "operations",
            "server_scope",
        ] {
            if source.get(key).is_some_and(|value| !value.is_array()) {
                return Err(invalid(
                    "original source contains an invalid array declaration",
                ));
            }
        }
        if source
            .get("page_rules")
            .is_some_and(|value| !value.is_object())
        {
            return Err(invalid("original source page rules are not an object"));
        }
        if source["game"] != game
            || source["locale"] != locale
            || !rows(&source, "server_scope")
                .iter()
                .any(|value| value == &server)
            || source["coordinate_space"] != coordinate_space
        {
            return Err(invalid(
                "original operation source metadata is inconsistent",
            ));
        }
        sources.push((path.to_string(), source));
    }
    let defaults = sources
        .first()
        .and_then(|(_, value)| value.get("defaults"))
        .cloned()
        .ok_or_else(|| invalid("original operation source/defaults are missing"))?;
    if sources
        .iter()
        .any(|(_, source)| source["defaults"] != defaults)
    {
        return Err(invalid("source defaults cannot be faithfully combined"));
    }
    let known_pages = rows(&pages_document, "pages")
        .iter()
        .map(|page| text(page, "id").map(str::to_owned))
        .collect::<LabResult<BTreeSet<_>>>()?;
    let page_id = |value: &str| {
        if value.starts_with(&format!("{game}/")) {
            value.to_string()
        } else {
            format!("{game}/{value}")
        }
    };
    let mut authored_targets = BTreeSet::new();
    for target in &request.target_pages {
        if !known_pages.contains(&page_id(target)) || !authored_targets.insert(page_id(target)) {
            return Err(invalid(
                "target pages must be unique pages in the admitted source",
            ));
        }
    }
    if request
        .entry_page
        .as_ref()
        .is_some_and(|page| !known_pages.contains(&page_id(page)))
    {
        return Err(invalid("entry page is absent from the admitted source"));
    }
    let mut needed_pages = authored_targets.clone();
    if let Some(page) = &request.entry_page {
        needed_pages.insert(page_id(page));
    }
    for input in records {
        for observation in [
            &input.operation.record.prepared.before_projection,
            &input.operation.record.after_projection,
        ]
        .into_iter()
        .flatten()
        {
            if observation.projection.matched {
                needed_pages.insert(observation.projection.page.clone());
            }
            if json!({"width":observation.frame.width(),"height":observation.frame.height()})
                != coordinate_space
            {
                return Err(invalid("actual frame coordinate space differs from source"));
            }
        }
    }
    let mut needed_targets = BTreeSet::new();
    let mut page_targets = BTreeMap::new();
    let mut page_rules = Map::new();
    for page in &needed_pages {
        let mut current_targets = BTreeSet::new();
        let short = page.strip_prefix(&format!("{game}/")).unwrap_or(page);
        for (_, source) in &sources {
            if let Some(rule) = source.get("page_rules").and_then(|rules| rules.get(short)) {
                if let Some(previous) = page_rules.insert(short.to_string(), rule.clone())
                    && previous != *rule
                {
                    return Err(invalid("conflicting original page rules"));
                }
                for key in ["required", "optional", "forbidden"] {
                    for value in rows(rule, key) {
                        current_targets.insert(text_value(value)?.to_string());
                    }
                }
                for group in rows(rule, "any_of") {
                    for value in group
                        .as_array()
                        .ok_or_else(|| invalid("invalid original any_of rule"))?
                    {
                        current_targets.insert(text_value(value)?.to_string());
                    }
                }
            }
            for anchor in rows(source, "anchors") {
                let id = text(anchor, "id")?;
                if id == short || id.starts_with(&format!("{short}_")) {
                    current_targets.insert(format!("page/{id}"));
                }
            }
        }
        needed_targets.extend(current_targets.iter().cloned());
        page_targets.insert(page.clone(), current_targets);
    }
    // An existing selected operation may use a direct guard target beyond its page anchor.
    for input in records {
        if let Some((_, operation)) = selected_source(&sources, &input.operation)?
            && let Some(target) = operation
                .pointer("/guard/target_id")
                .and_then(Value::as_str)
        {
            needed_targets.insert(target.to_string());
        }
    }
    let task_root = PathBuf::from("ours/operations").join(&request.task_id);
    let mut assets = BTreeMap::<String, Vec<u8>>::new();
    let mut source_entries = BTreeMap::<String, String>::new();
    let mut anchors = BTreeMap::new();
    let mut colors = BTreeMap::new();
    let mut templates = BTreeMap::new();
    let mut copied_targets = BTreeSet::new();
    let mut withheld = BTreeSet::new();
    for (path, source) in &sources {
        for (kind, output) in [
            ("anchors", &mut anchors),
            ("color_probes", &mut colors),
            ("verify_templates", &mut templates),
        ] {
            for row in rows(source, kind) {
                let id = text(row, "id")?;
                let target = if kind == "anchors" {
                    format!("page/{id}")
                } else {
                    id.to_string()
                };
                if !needed_targets.contains(&target) {
                    continue;
                }
                if package
                    .projection_metadata()
                    .and_then(|metadata| metadata.target_privacy(&target))
                    == Some(Privacy::Personal)
                {
                    withheld.insert(target);
                    continue;
                }
                let mut row = row.clone();
                if let Some(template) = row
                    .get("template")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                {
                    let copied =
                        copy_template(package, path, &template, &mut assets, &mut source_entries)?;
                    row["template"] = json!(copied);
                }
                if let Some(previous) = output.insert(id.to_string(), row.clone())
                    && previous != row
                {
                    return Err(invalid("conflicting source target declarations"));
                }
                copied_targets.insert(target);
                source_entries.insert(
                    path.clone(),
                    format!(
                        "{:x}",
                        Sha256::digest(
                            package
                                .entry(path)
                                .ok_or_else(|| invalid("source entry missing"))?
                        )
                    ),
                );
            }
        }
    }
    let mut operations = Vec::new();
    let mut provenance = Vec::new();
    let mut gaps = missing_records.to_vec();
    let mut annotations = Vec::new();
    let mut action_ids = BTreeSet::new();
    for input in records {
        let operation = &input.operation;
        let record = &operation.record;
        let prepared = &record.prepared;
        let mut record_source = record_provenance(input);
        let mut record_gaps = Vec::new();
        let before = prepared
            .before_projection
            .as_ref()
            .filter(|value| value.projection.matched);
        let after = record
            .after_projection
            .as_ref()
            .filter(|value| value.projection.matched);
        if input.input_event_type != Some(EventType::InputCommitted)
            || record.effect != EffectDisposition::Performed
        {
            record_gaps.push("input_not_committed");
        }
        if before.is_none() || after.is_none() {
            record_gaps.push("confirmed_pages_missing");
        }
        if record
            .failure
            .as_ref()
            .is_some_and(|failure| failure.stage == LabOperationStage::Input)
        {
            record_gaps.push("input_failed");
        }
        if let Some(action_id) = record.input_action_id
            && !action_ids.insert(action_id)
        {
            return Err(invalid("duplicate physical action identity"));
        }
        if let (Some(before), Some(after), Some(action_id), Some(geometry)) =
            (before, after, record.input_action_id, &prepared.geometry)
        {
            let from = &before.projection.page;
            let to = &after.projection.page;
            if let Err(error) = crate::package_build::validate_restored_geometry(
                geometry,
                before.frame.width(),
                before.frame.height(),
            ) {
                record_gaps.push("action_not_representable_by_bundle");
                record_source["bundle_geometry_error"] = json!(error.message);
            }
            let short = from.strip_prefix(&format!("{game}/")).unwrap_or(from);
            let selected = selected_source(&sources, operation)?;
            let mut guard = selected
                .and_then(|(_, source)| source.get("guard"))
                .filter(|value| !value.is_null())
                .cloned();
            if guard
                .as_ref()
                .and_then(|value| value.get("target_id"))
                .and_then(Value::as_str)
                .is_some_and(|target| !copied_targets.contains(target))
            {
                record_gaps.push("guard_or_template_missing");
                guard = None;
            }
            if let (Some(value), Some((source_path, _))) = (&mut guard, selected)
                && let Some(template) = value
                    .get("verify_template")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            {
                value["verify_template"] = json!(copy_template(
                    package,
                    source_path,
                    &template,
                    &mut assets,
                    &mut source_entries
                )?);
            }
            if guard.is_none()
                && let Some(anchor) = anchors.get(short)
            {
                guard = Some(json!({"page_id":from,"target_id":format!("page/{short}"),
                    "expected_rect":geometry_start_rect(geometry),"verify_template":anchor["template"]}));
            }
            let target = guard
                .as_ref()
                .and_then(|value| value.get("target_id"))
                .and_then(Value::as_str);
            if target.is_none_or(|target| !copied_targets.contains(target)) {
                record_gaps.push("guard_or_template_missing");
            }
            if [from, to].iter().any(|page| {
                page_targets.get(*page).is_none_or(|targets| {
                    targets.is_empty()
                        || targets
                            .iter()
                            .any(|target| !copied_targets.contains(target))
                })
            }) {
                record_gaps.push(if withheld.is_empty() {
                    "page_dependency_missing"
                } else {
                    "private_dependency_withheld"
                });
            }
            if record_gaps.is_empty() {
                let id = serde_json::to_value(action_id)
                    .map_err(|_| invalid("action identity encoding failed"))?;
                let id = text_value(&id)?.to_string();
                let purpose = selected
                    .and_then(|(_, source)| source.get("purpose"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let mut restored = json!({"id":id,"purpose":purpose,"from":from,
                    "to":if from == to { Value::Null } else { json!(to) },
                    "click":geometry_click(geometry), "guard":guard,"consumes":[],"produces":[],
                    "provenance":record_source});
                if from == to {
                    restored["expect_after"] = json!({"page_id":to});
                }
                let (safety, safety_source) = prepared
                    .selected_element
                    .as_ref()
                    .and_then(|element| {
                        let safety = element.get("safety")?.as_str()?;
                        let source = element.get("safety_source")?.as_str()?;
                        (selected.is_some()
                            && matches!(safety, "safe" | "dangerous")
                            && !source.trim().is_empty())
                        .then_some((safety.to_string(), source.to_string()))
                    })
                    .unwrap_or_else(|| {
                        (
                            "dangerous".to_string(),
                            "actinglab.resource.restore/default-classification-v1".to_string(),
                        )
                    });
                annotations.push(json!({"action":{"role":if from == to {"page_op"}else{"navigate"},
                    "task_id":if from == to {request.task_id.as_str()}else{""},"resource_id":id,"page":from},
                    "safety":safety,"source":safety_source}));
                operations.push(restored);
            }
        } else if record_gaps.is_empty() {
            record_gaps.push("action_geometry_missing");
        }
        record_source["gaps"] = json!(record_gaps);
        if !record_gaps.is_empty() {
            gaps.push(json!({"request_id":prepared.request_id,"codes":record_gaps}));
        }
        provenance.push(record_source);
    }
    let mut task = json!({"schema_version":"0.6","task_id":request.task_id,"game":game,
        "server_scope":[server],"locale":locale,"coordinate_space":coordinate_space,"defaults":defaults,
        "anchors":anchors.values().collect::<Vec<_>>(),"color_probes":colors.values().collect::<Vec<_>>(),
        "verify_templates":templates.values().collect::<Vec<_>>(),"page_rules":page_rules,"operations":operations,
        "provenance":{"source":"global_ledger","package_sha256":package.verified_hash().to_string(),
            "through_sequence":request.through_sequence,"records":provenance,"gaps":gaps,
            "source_entries":source_entries.iter().map(|(path,sha256)| json!({"path":path,"sha256":sha256})).collect::<Vec<_>>()}});
    if let Some(page) = &request.entry_page {
        task["entry_page"] = json!(page);
    }
    if !request.target_pages.is_empty() {
        task["target_page"] = if request.target_pages.len() == 1 {
            json!(request.target_pages[0])
        } else {
            json!(request.target_pages)
        };
    }
    if let Some(goal) = &request.goal {
        task["goal"] = json!(goal);
    }
    let mut files = Vec::new();
    files.push(AuthoringFile::bytes(
        task_root.join("task.json"),
        json_bytes(&task)?,
        AuthoringWriteMode::CreateIfMissing,
    )?);
    files.push(AuthoringFile::bytes(
        "ours/operations/resources.json",
        json_bytes(&json!({"schema_version":"1.0","resources":[],"resource_count":0}))?,
        AuthoringWriteMode::CreateIfMissing,
    )?);
    files.push(AuthoringFile::bytes(
        format!("ours/navigation/{game}.{server}.projection.json"),
        json_bytes(&json!({
        "schema_version":"actingcommand.page-projection-metadata.v1","actions":annotations,
        "targets":[],"fields":[],"pages":[]}))?,
        AuthoringWriteMode::CreateIfMissing,
    )?);
    for (path, bytes) in assets {
        files.push(AuthoringFile::bytes(
            task_root.join(path),
            bytes,
            AuthoringWriteMode::CreateIfMissing,
        )?);
    }
    let mut awaiting = Vec::new();
    if request.target_pages.is_empty() {
        awaiting.push("target_page");
    }
    if request.entry_page.is_none() {
        awaiting.push("entry_page");
    }
    let report = json!({"status":"draft_generated","record_count":records.len()+missing_records.len(),
        "operation_count":operations.len(),"gaps":gaps,"awaiting_author_input":awaiting,
        "package_sha256":package.verified_hash().to_string(),"through_sequence":request.through_sequence,
        "task_id":request.task_id});
    let draft = AuthoringDraft::new(
        format!("ledger-through-{}", request.through_sequence),
        &request.task_id,
        &task_root,
        files,
        AuthoringProvenance {
            record_id: format!("ledger-through-{}", request.through_sequence),
            source_artifact_ids: records
                .iter()
                .map(|input| {
                    serde_json::to_string(&input.operation.terminal_artifact.artifact.artifact_id)
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| invalid("artifact identity encoding failed"))?,
        },
    )?;
    Ok(ResourceRestoreDraft { draft, report })
}

fn selected_source<'a>(
    sources: &'a [(String, Value)],
    operation: &ContainedLabOperationResult,
) -> LabResult<Option<(&'a str, &'a Value)>> {
    let Some(element) = &operation.record.prepared.selected_element else {
        return Ok(None);
    };
    let task = element
        .get("task_id")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("selected operation task identity missing"))?;
    let id = text(element, "resource_id")?;
    let matches = sources
        .iter()
        .filter(|(_, source)| task.is_empty() || source["task_id"] == task)
        .flat_map(|(path, source)| {
            rows(source, "operations")
                .iter()
                .filter(move |value| value["id"] == id)
                .map(move |value| (path.as_str(), value))
        })
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        return Err(invalid("selected source operation is ambiguous"));
    }
    Ok(matches.first().copied())
}

fn record_provenance(input: &ResourceRestoreRecord) -> Value {
    let record = &input.operation.record;
    let prepared = &record.prepared;
    let observation =
        |frame: &Option<actingcommand_contract::LabOperationFrame>,
         projection: &Option<actingcommand_contract::ContainedPageObservation>| {
            json!({"frame":frame.as_ref().map(|frame| json!({"frame_id":frame.observation.artifact().frame_id,
            "sha256":frame.observation.artifact().sha256,"verified":frame.verified,"lease_valid":frame.lease_valid_after_capture})),
            "projection":projection.as_ref().map(|projection| json!({"page":projection.projection.page,"matched":projection.projection.matched,
                "sequence":projection.projection_sequence,"event_id":projection.projection_event_id,
                "artifact_sha256":projection.artifact.sha256,"content_sha256":projection.projection.content_sha256}))})
        };
    json!({"request_id":prepared.request_id,"correlation_id":prepared.correlation_id,"instance_id":prepared.instance_id,
        "lease_id":prepared.lease_id,"expected_package_sha256":prepared.expected_package_sha256,"actual_package_sha256":prepared.actual_package_sha256,
        "prepared":prepared_record_reference(&record.prepared_artifact),"terminal_record":prepared_record_reference(&input.operation.terminal_artifact),
        "command_terminal":input.terminal,"input_action_id":record.input_action_id,"input_intent":record.input_intent,
        "input_event":record.input_event,"input_event_type":input.input_event_type,"input_returned":record.input_returned,
        "effect":record.effect,"actual_action":prepared.action,"actual_geometry":prepared.geometry,
        "before":observation(&prepared.before_frame,&prepared.before_projection),"after":observation(&record.after_frame,&record.after_projection),
        "failure":record.failure.as_ref().map(|failure|json!({"stage":failure.stage,"code":failure.code,"event":failure.event})),
        "cleanup_failure":record.cleanup_failure.as_ref().map(|failure|json!({"stage":failure.stage,"code":failure.code,"event":failure.event}))})
}

fn prepared_record_reference(reference: &actingcommand_contract::LabEvidenceReference) -> Value {
    json!({"artifact_id":reference.artifact.artifact_id,"sha256":reference.artifact.sha256,"verified":reference.verified})
}

fn geometry_start_rect(geometry: &Geometry) -> Value {
    match geometry {
        Geometry::Tap { rect, .. } => json!(rect),
        Geometry::Drag { from_rect, .. } => json!(from_rect),
    }
}

fn geometry_click(geometry: &Geometry) -> Value {
    match geometry {
        Geometry::Tap { point, .. } => json!({"kind":"point","x":point.x,"y":point.y}),
        Geometry::Drag {
            from_rect,
            to_rect,
            duration_ms,
            ..
        } => json!({"kind":"drag","from":from_rect,"to":to_rect,"duration_ms":duration_ms}),
    }
}

fn copy_template(
    package: &LoadedBundle,
    source_path: &str,
    template: &str,
    assets: &mut BTreeMap<String, Vec<u8>>,
    source_entries: &mut BTreeMap<String, String>,
) -> LabResult<String> {
    if template.contains('\\')
        || Path::new(template)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(invalid(
            "template source path is not a direct relative entry",
        ));
    }
    let parent = source_path
        .rsplit_once('/')
        .ok_or_else(|| invalid("source task path is invalid"))?
        .0;
    let path = format!("{parent}/{template}");
    let bytes = package
        .entry(&path)
        .ok_or_else(|| invalid("declared template entry is missing"))?;
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    let extension = Path::new(template)
        .extension()
        .and_then(|value| value.to_str())
        .ok_or_else(|| invalid("template has no extension"))?;
    let destination = format!("assets/{sha256}.{extension}");
    if !assets.contains_key(&destination) {
        if assets.values().map(Vec::len).sum::<usize>() + bytes.len()
            > DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES
        {
            return Err(invalid(
                "restored assets exceed the existing buffered payload budget",
            ));
        }
        assets.insert(destination.clone(), bytes.to_vec());
    }
    source_entries.insert(path, sha256);
    Ok(destination)
}

fn read_entry_json(package: &LoadedBundle, path: &str) -> LabResult<Value> {
    serde_json::from_slice(
        package
            .entry(path)
            .ok_or_else(|| invalid("admitted source entry is missing"))?,
    )
    .map_err(|_| invalid("admitted source entry is not JSON"))
}
fn json_bytes(value: &Value) -> LabResult<Vec<u8>> {
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|_| invalid("draft JSON encoding failed"))?;
    if bytes.len() > DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES {
        return Err(invalid("draft source exceeds existing payload budget"));
    }
    Ok(bytes)
}
fn rows<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}
fn text<'a>(value: &'a Value, key: &str) -> LabResult<&'a str> {
    text_value(
        value
            .get(key)
            .ok_or_else(|| invalid("required source field missing"))?,
    )
}
fn text_value(value: &Value) -> LabResult<&str> {
    value
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| invalid("required source text missing"))
}
fn invalid(message: &str) -> LabError {
    LabError::package_invalid(format!("resource restore: {message}"))
}
