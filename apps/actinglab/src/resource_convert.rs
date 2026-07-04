// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, ResolvedResourceRoot, canonical_game,
    maa_task_graph,
};
use serde_json::{Map, Value, json};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

const GENERATED_BY: &str = "actinglab resource convert";
const CONVERTER_SCHEMA_VERSION: &str = "0.5";
const OUTPUT_SCHEMA_VERSION: &str = "0.5";
const FULL_FRAME_SENTINEL: &str = "full_frame";

pub(super) fn run_resource_convert(
    global: &GlobalOptions,
    flags: &FlagArgs,
    resource_root: &ResolvedResourceRoot,
) -> CliOutcome<Value> {
    let repo = &resource_root.root;
    let game_override = flags.optional("--game").or_else(|| global.game.clone());
    let game_override = game_override.as_deref().map(canonical_game).transpose()?;
    let server_override = flags.optional("--server").or_else(|| global.server.clone());
    let locale_override = flags.optional("--locale");
    let mut converter = OperationConverter::load(
        repo,
        game_override.as_deref(),
        server_override.as_deref(),
        locale_override.as_deref(),
    )?;
    let maa_tasks_root = flags.optional_path("--maa-tasks");
    if let Some(tasks_root) = maa_tasks_root.as_deref() {
        converter.load_maa_task_overlays(tasks_root)?;
    }
    let outputs = converter.build_all()?;
    let dry_run = global.dry_run || flags.bool("--dry-run");
    if !dry_run {
        outputs.write(repo)?;
    }
    let mut summary = json!({
        "repo": resource_root.input.display().to_string(),
        "resource_root": repo.display().to_string(),
        "resource_layout": resource_root.layout,
        "game": converter.game,
        "server": converter.server,
        "locale": converter.locale,
        "dry_run": dry_run,
        "bundles": converter.bundles.len(),
        "targets": outputs.pack.get("targets").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "pages": outputs.pages.get("pages").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "edges": outputs.navigation.get("navigation").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "page_operations": outputs.navigation.get("page_operations").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "index_tasks": outputs.index.get("operations").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "primitives": outputs.primitives.get("primitives").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "status": if dry_run { "validated" } else { "written" }
    });
    if let Some(tasks_root) = maa_tasks_root {
        let object = summary
            .as_object_mut()
            .expect("resource convert summary is object");
        object.insert(
            "source_mode".to_string(),
            Value::String("maa_tasks".to_string()),
        );
        object.insert(
            "maa_tasks_root".to_string(),
            Value::String(tasks_root.display().to_string()),
        );
        object.insert(
            "maa_compiled_tasks".to_string(),
            Value::Number(serde_json::Number::from(
                converter.maa_task_overlays.len() as u64
            )),
        );
    }
    Ok(summary)
}

#[derive(Debug)]
pub(super) struct OperationConverter {
    pub(super) root: PathBuf,
    pub(super) game: String,
    pub(super) server: String,
    pub(super) locale: String,
    pub(super) coordinate_space: Value,
    pub(super) defaults: Value,
    resource_ids: HashSet<String>,
    pub(super) bundles: Vec<Bundle>,
    existing_navigation: Option<Value>,
    maa_task_overlays: HashMap<String, Value>,
}

#[derive(Debug, Clone)]
pub(super) struct Bundle {
    pub(super) task_id: String,
    pub(super) dir: PathBuf,
    pub(super) data: Value,
}

#[derive(Debug)]
pub(super) struct ConvertOutputs {
    pub(super) pack: Value,
    pub(super) pages: Value,
    pub(super) navigation: Value,
    pub(super) index: Value,
    pub(super) primitives: Value,
}

impl ConvertOutputs {
    fn write(&self, repo: &Path) -> CliOutcome<()> {
        let game = required_string(&self.pack, "game")?;
        let server = required_string(&self.pack, "server")?;
        let stem = format!("{game}.{server}");
        write_json_file(
            &repo.join("recognition").join(format!("{stem}.pack.json")),
            &self.pack,
        )?;
        write_json_file(
            &repo.join("recognition").join(format!("{stem}.pages.json")),
            &self.pages,
        )?;
        write_json_file(
            &repo
                .join("navigation")
                .join(format!("{stem}.navigation.json")),
            &self.navigation,
        )?;
        write_json_file(
            &repo.join("operations").join("operations.index.json"),
            &self.index,
        )?;
        write_json_file(
            &repo.join("operations").join("operations.primitives.json"),
            &self.primitives,
        )
    }
}

impl OperationConverter {
    pub(super) fn load(
        root: &Path,
        game_override: Option<&str>,
        server_override: Option<&str>,
        locale_override: Option<&str>,
    ) -> CliOutcome<Self> {
        let root = root.to_path_buf();
        let ops_dir = root.join("operations");
        let resources = read_json_value(&ops_dir.join("resources.json"))?;
        let resource_ids = resource_ids(&resources)?;
        let bundles = load_bundles(&ops_dir)?;
        let first = bundles.first().ok_or_else(|| {
            CliError::package_invalid(format!(
                "no Operation Bundles found under {}",
                ops_dir.display()
            ))
        })?;
        let game = game_override
            .map(str::to_string)
            .or_else(|| string_field(&first.data, "game"))
            .unwrap_or_else(|| "bluearchive".to_string());
        let server = server_override
            .map(str::to_string)
            .or_else(|| first_server_scope(&first.data))
            .unwrap_or_else(|| "jp".to_string());
        let locale = locale_override
            .map(str::to_string)
            .unwrap_or_else(|| default_locale(&game).to_string());
        let coordinate_space =
            first.data.get("coordinate_space").cloned().ok_or_else(|| {
                CliError::package_invalid("first bundle missing coordinate_space")
            })?;
        let defaults = first
            .data
            .get("defaults")
            .cloned()
            .ok_or_else(|| CliError::package_invalid("first bundle missing defaults"))?;
        let existing_navigation_path = root
            .join("navigation")
            .join(format!("{game}.{server}.navigation.json"));
        let existing_navigation = if existing_navigation_path.exists() {
            Some(read_json_value(&existing_navigation_path)?)
        } else {
            None
        };
        let converter = Self {
            root,
            game,
            server,
            locale,
            coordinate_space,
            defaults,
            resource_ids,
            bundles,
            existing_navigation,
            maa_task_overlays: HashMap::new(),
        };
        converter.validate_bundles()?;
        Ok(converter)
    }

    pub(super) fn load_maa_task_overlays(&mut self, tasks_root: &Path) -> CliOutcome<()> {
        let graph = maa_task_graph::compile_maa_task_graph_family(tasks_root)?;
        self.maa_task_overlays = graph
            .tasks()
            .iter()
            .map(|(task_id, task)| (task_id.clone(), task.clone()))
            .collect();
        Ok(())
    }

    fn enrich_template_source(&self, source: &Value, source_task_id: &str) -> CliOutcome<Value> {
        if self.maa_task_overlays.is_empty() {
            return Ok(source.clone());
        }
        let explicit_task_id =
            string_field(source, "maa_task").or_else(|| string_field(source, "maa_task_id"));
        let task_id = explicit_task_id.or_else(|| {
            self.maa_task_overlays
                .contains_key(source_task_id)
                .then(|| source_task_id.to_string())
        });
        let Some(task_id) = task_id else {
            return Ok(source.clone());
        };
        let Some(compiled) = self.maa_task_overlays.get(&task_id) else {
            return Err(CliError::package_invalid(format!(
                "MAA task overlay '{task_id}' was requested but was not found"
            )));
        };
        let mut out = source.as_object().cloned().ok_or_else(|| {
            CliError::package_invalid(format!(
                "resource template source for MAA task '{task_id}' must be a JSON object"
            ))
        })?;
        copy_maa_template_field(
            &mut out,
            compiled,
            "threshold",
            &["threshold", "templThreshold"],
        )?;
        copy_maa_template_field(
            &mut out,
            compiled,
            "method",
            &["method", "matchMethod", "match_method"],
        )?;
        copy_maa_template_field(&mut out, compiled, "mask", &["mask", "maskRange"])?;
        copy_maa_template_field(&mut out, compiled, "rect_move", &["rect_move", "rectMove"])?;
        Ok(Value::Object(out))
    }

    pub(super) fn build_all(&self) -> CliOutcome<ConvertOutputs> {
        let pack = self.build_pack()?;
        validate_pack_targets_exist(&self.root, &pack)?;
        let pages = self.build_pages()?;
        validate_page_rule_targets(&pack, &self.bundles)?;
        let navigation = self.build_navigation()?;
        let index = self.build_index()?;
        let primitives = self.build_primitives()?;
        validate_converted_guard_references(&pack, &pages, &primitives)?;
        Ok(ConvertOutputs {
            pages,
            navigation,
            index,
            primitives,
            pack,
        })
    }

    pub(super) fn build_selected(&self, task_ids: &[String]) -> CliOutcome<ConvertOutputs> {
        let selected = self
            .bundles
            .iter()
            .filter(|bundle| task_ids.iter().any(|task_id| task_id == &bundle.task_id))
            .cloned()
            .collect::<Vec<_>>();
        if selected.is_empty() {
            return Err(CliError::package_invalid(format!(
                "none of the selected tasks exist: {}",
                task_ids.join(", ")
            )));
        }
        let subset = Self {
            root: self.root.clone(),
            game: self.game.clone(),
            server: self.server.clone(),
            locale: self.locale.clone(),
            coordinate_space: self.coordinate_space.clone(),
            defaults: self.defaults.clone(),
            resource_ids: self.resource_ids.clone(),
            bundles: selected,
            existing_navigation: self.existing_navigation.clone(),
            maa_task_overlays: self.maa_task_overlays.clone(),
        };
        subset.validate_bundles()?;
        subset.build_all()
    }

    fn validate_bundles(&self) -> CliOutcome<()> {
        let mut errors = Vec::new();
        for bundle in &self.bundles {
            if !matches!(
                bundle.data.get("schema_version").and_then(Value::as_str),
                Some("0.3" | "0.4" | "0.5")
            ) {
                errors.push(format!(
                    "{}: unsupported schema_version, expected 0.3, 0.4, or 0.5",
                    bundle.task_json_path().display()
                ));
            }
            if let Some(metric) = bundle
                .data
                .get("defaults")
                .and_then(|defaults| defaults.get("match_metric"))
                .and_then(Value::as_str)
                && !matches!(metric, "ccorr_normed" | "ccoeff_normed")
            {
                errors.push(format!(
                    "{}: defaults.match_metric unsupported: {metric:?}",
                    bundle.task_json_path().display()
                ));
            }
            for anchor in array_field(&bundle.data, "anchors") {
                let template = string_field(anchor, "template").unwrap_or_default();
                if !bundle.dir.join(&template).is_file() {
                    errors.push(format!(
                        "{}: anchor {:?} template missing on disk: {}",
                        bundle.task_json_path().display(),
                        anchor.get("id").and_then(Value::as_str),
                        bundle.dir.join(&template).display()
                    ));
                }
            }
            for verify_template in array_field(&bundle.data, "verify_templates") {
                let template = string_field(verify_template, "template").unwrap_or_default();
                if !bundle.dir.join(&template).is_file() {
                    errors.push(format!(
                        "{}: verify_template {:?} template missing on disk: {}",
                        bundle.task_json_path().display(),
                        verify_template.get("id").and_then(Value::as_str),
                        bundle.dir.join(&template).display()
                    ));
                }
            }
            for operation in array_field(&bundle.data, "operations") {
                validate_click_shape(&bundle.task_json_path(), operation, &mut errors);
                if let Some(template) = operation.get("verify_template").and_then(Value::as_str)
                    && !bundle.dir.join(template).is_file()
                {
                    errors.push(format!(
                        "{}: op {:?} verify_template missing on disk: {}",
                        bundle.task_json_path().display(),
                        operation.get("id").and_then(Value::as_str),
                        bundle.dir.join(template).display()
                    ));
                }
                for resource in operation
                    .get("consumes")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .chain(
                        operation
                            .get("produces")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten(),
                    )
                {
                    let Some(resource_id) = resource.as_str() else {
                        continue;
                    };
                    if !self.resource_ids.contains(resource_id) {
                        errors.push(format!(
                            "{}: op {:?} references unknown resource id {resource_id:?}",
                            bundle.task_json_path().display(),
                            operation.get("id").and_then(Value::as_str)
                        ));
                    }
                }
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(CliError::package_invalid(format!(
                "resource convert validation failed:\n  - {}",
                errors.join("\n  - ")
            )))
        }
    }

    fn build_pack(&self) -> CliOutcome<Value> {
        let mut targets = HashMap::<String, Value>::new();
        let mut order = Vec::<String>::new();
        for bundle in &self.bundles {
            for anchor in array_field(&bundle.data, "anchors") {
                let anchor_id = required_string(anchor, "id")?;
                let target_id = anchor_target_id(&anchor_id);
                let source = self.enrich_template_source(anchor, &anchor_id)?;
                let template = required_string(&source, "template")?;
                let target = pack_target(
                    &source,
                    &target_id,
                    &repo_rel(&self.root, &bundle.dir.join(&template))?,
                    region_to_pack(required_field(&source, "region")?)?,
                    source.get("threshold").cloned().unwrap_or_else(|| {
                        required_field(&self.defaults, "template_threshold")
                            .cloned()
                            .unwrap_or(Value::Null)
                    }),
                    color_check_to_pack(source.get("color_check"))?,
                    None,
                );
                add_first_target(&mut targets, &mut order, target_id, target);
            }
            for color_probe in array_field(&bundle.data, "color_probes") {
                let target_id = required_string(color_probe, "id")?;
                let target = color_target(
                    &target_id,
                    region_to_pack(required_field(color_probe, "region")?)?,
                    required_field(color_probe, "expected")?.clone(),
                    None,
                );
                add_first_target(&mut targets, &mut order, target_id, target);
            }
            for verify_template in array_field(&bundle.data, "verify_templates") {
                let target_id = required_string(verify_template, "id")?;
                let source = self.enrich_template_source(verify_template, &target_id)?;
                let template = required_string(&source, "template")?;
                let target = pack_target(
                    &source,
                    &target_id,
                    &repo_rel(&self.root, &bundle.dir.join(&template))?,
                    region_to_pack(required_field(&source, "region")?)?,
                    source.get("threshold").cloned().unwrap_or_else(|| {
                        required_field(&self.defaults, "template_threshold")
                            .cloned()
                            .unwrap_or(Value::Null)
                    }),
                    None,
                    None,
                );
                add_first_target(&mut targets, &mut order, target_id, target);
            }
            for operation in array_field(&bundle.data, "operations") {
                let Some(template) = operation.get("verify_template").and_then(Value::as_str)
                else {
                    continue;
                };
                let target_id = template_target_id(template);
                let operation_id = operation
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or(target_id.as_str());
                let source = self.enrich_template_source(operation, operation_id)?;
                let target = pack_target(
                    &source,
                    &target_id,
                    &repo_rel(&self.root, &bundle.dir.join(template))?,
                    Value::String(FULL_FRAME_SENTINEL.to_string()),
                    source
                        .get("threshold")
                        .cloned()
                        .unwrap_or(required_field(&self.defaults, "template_threshold")?.clone()),
                    None,
                    None,
                );
                add_first_target(&mut targets, &mut order, target_id, target);
            }
        }
        propagate_color_checks(&mut targets, &order);
        Ok(ordered_object([
            (
                "schema_version",
                Value::String(OUTPUT_SCHEMA_VERSION.to_string()),
            ),
            (
                "converter_schema_version",
                Value::String(CONVERTER_SCHEMA_VERSION.to_string()),
            ),
            ("generated", Value::Bool(true)),
            ("generated_by", Value::String(GENERATED_BY.to_string())),
            ("game", Value::String(self.game.clone())),
            ("server", Value::String(self.server.clone())),
            ("locale", Value::String(self.locale.clone())),
            ("coordinate_space", self.coordinate_space.clone()),
            ("defaults", self.defaults.clone()),
            (
                "targets",
                Value::Array(
                    order
                        .iter()
                        .filter_map(|id| targets.get(id).cloned())
                        .collect(),
                ),
            ),
        ]))
    }

    fn build_pages(&self) -> CliOutcome<Value> {
        let declared_anchor_ids = self.declared_anchor_ids();
        let mut pages = HashMap::<String, Value>::new();
        let mut order = Vec::<String>::new();
        for bundle in &self.bundles {
            for key in ["entry_page", "target_page"] {
                if let Some(anchor_id) = bundle.data.get(key).and_then(Value::as_str) {
                    add_page(
                        &self.game,
                        anchor_id,
                        &declared_anchor_ids,
                        &mut pages,
                        &mut order,
                    );
                }
            }
            for operation in array_field(&bundle.data, "operations") {
                for key in ["from", "to"] {
                    if let Some(anchor_id) = operation.get(key).and_then(Value::as_str) {
                        add_page(
                            &self.game,
                            anchor_id,
                            &declared_anchor_ids,
                            &mut pages,
                            &mut order,
                        );
                    }
                }
            }
        }
        self.apply_page_rules(&mut pages)?;
        Ok(ordered_object([
            (
                "schema_version",
                Value::String(OUTPUT_SCHEMA_VERSION.to_string()),
            ),
            (
                "converter_schema_version",
                Value::String(CONVERTER_SCHEMA_VERSION.to_string()),
            ),
            ("generated", Value::Bool(true)),
            ("generated_by", Value::String(GENERATED_BY.to_string())),
            (
                "pages",
                Value::Array(
                    order
                        .iter()
                        .filter_map(|id| pages.get(id).cloned())
                        .collect(),
                ),
            ),
        ]))
    }

    fn apply_page_rules(&self, pages: &mut HashMap<String, Value>) -> CliOutcome<()> {
        for bundle in &self.bundles {
            let Some(rules) = bundle.data.get("page_rules").and_then(Value::as_object) else {
                continue;
            };
            for (page_key, rule) in rules {
                let page_id = normalize_page_rule_id(&self.game, page_key);
                let page = pages.get_mut(&page_id).ok_or_else(|| {
                    CliError::package_invalid(format!(
                        "{}: page_rules references unknown page '{page_key}'",
                        bundle.task_json_path().display()
                    ))
                })?;
                for field in ["required", "optional", "forbidden"] {
                    if let Some(values) = rule.get(field) {
                        append_unique_strings(page, field, values, &bundle.task_json_path())?;
                    }
                }
                if let Some(groups) = rule.get("any_of") {
                    append_any_of_groups(page, groups, &bundle.task_json_path())?;
                }
            }
        }
        Ok(())
    }

    fn build_navigation(&self) -> CliOutcome<Value> {
        let control_points = self
            .existing_navigation
            .as_ref()
            .and_then(|navigation| navigation.get("control_points"))
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        let mut edges = HashMap::<String, Value>::new();
        let mut edge_order = Vec::<String>::new();
        for bundle in &self.bundles {
            for operation in array_field(&bundle.data, "operations") {
                if !is_page_change(operation) {
                    continue;
                }
                let edge_id = required_string(operation, "id")?;
                let provenance = operation.get("provenance").unwrap_or(&Value::Null);
                let source = provenance
                    .get("navigation_ref")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let from = required_string(operation, "from")?;
                let to = required_string(operation, "to")?;
                let edge = ordered_object([
                    ("id", Value::String(edge_id.clone())),
                    ("from_page", page_or_any(&self.game, &from)),
                    ("to_page", page_or_any(&self.game, &to)),
                    (
                        "click",
                        click_to_navigation(required_field(operation, "click")?)?,
                    ),
                    ("source", Value::String(source.to_string())),
                ]);
                if !edges.contains_key(&edge_id) {
                    edges.insert(edge_id.clone(), edge);
                    edge_order.push(edge_id);
                }
            }
        }
        let page_operations = self.build_page_operations()?;
        Ok(ordered_object([
            (
                "schema_version",
                Value::String(OUTPUT_SCHEMA_VERSION.to_string()),
            ),
            (
                "converter_schema_version",
                Value::String(CONVERTER_SCHEMA_VERSION.to_string()),
            ),
            ("generated", Value::Bool(true)),
            ("generated_by", Value::String(GENERATED_BY.to_string())),
            ("game", Value::String(self.game.clone())),
            ("server", Value::String(self.server.clone())),
            ("coordinate_space", self.coordinate_space.clone()),
            ("control_points", control_points),
            (
                "navigation",
                Value::Array(
                    edge_order
                        .iter()
                        .filter_map(|id| edges.get(id).cloned())
                        .collect(),
                ),
            ),
            ("page_operations", Value::Array(page_operations.clone())),
            ("destructive_actions", Value::Array(page_operations)),
        ]))
    }

    fn build_page_operations(&self) -> CliOutcome<Vec<Value>> {
        let mut page_operations = Vec::new();
        for bundle in &self.bundles {
            for operation in array_field(&bundle.data, "operations") {
                if operation.get("to") != Some(&Value::Null) {
                    continue;
                }
                let verify_template = operation
                    .get("verify_template")
                    .and_then(Value::as_str)
                    .map(template_target_id)
                    .map(Value::String)
                    .unwrap_or(Value::Null);
                page_operations.push(ordered_object([
                    ("task_id", Value::String(bundle.task_id.clone())),
                    (
                        "page",
                        page_or_any(&self.game, &required_string(operation, "from")?),
                    ),
                    ("id", Value::String(required_string(operation, "id")?)),
                    (
                        "purpose",
                        Value::String(string_field(operation, "purpose").unwrap_or_default()),
                    ),
                    (
                        "click",
                        click_to_navigation(required_field(operation, "click")?)?,
                    ),
                    (
                        "expect_after",
                        operation
                            .get("expect_after")
                            .cloned()
                            .unwrap_or(Value::Null),
                    ),
                    ("verify_template", verify_template),
                    (
                        "consumes",
                        operation
                            .get("consumes")
                            .cloned()
                            .unwrap_or_else(|| Value::Array(Vec::new())),
                    ),
                    (
                        "produces",
                        operation
                            .get("produces")
                            .cloned()
                            .unwrap_or_else(|| Value::Array(Vec::new())),
                    ),
                ]));
            }
        }
        Ok(page_operations)
    }

    fn build_index(&self) -> CliOutcome<Value> {
        let mut operations = Vec::new();
        for bundle in &self.bundles {
            operations.push(ordered_object([
                ("task_id", Value::String(bundle.task_id.clone())),
                (
                    "goal",
                    Value::String(string_field(&bundle.data, "goal").unwrap_or_default()),
                ),
                (
                    "entry_page",
                    bundle
                        .data
                        .get("entry_page")
                        .cloned()
                        .unwrap_or(Value::Null),
                ),
                (
                    "target_page",
                    bundle
                        .data
                        .get("target_page")
                        .cloned()
                        .unwrap_or(Value::Null),
                ),
                (
                    "server_scope",
                    bundle
                        .data
                        .get("server_scope")
                        .cloned()
                        .unwrap_or_else(|| Value::Array(Vec::new())),
                ),
                (
                    "op_count",
                    Value::Number(array_field(&bundle.data, "operations").len().into()),
                ),
                (
                    "has_unresolved_coords",
                    Value::Bool(has_unresolved_coords(&bundle.data)),
                ),
                (
                    "bundle_path",
                    Value::String(format!("operations/{}", bundle.task_id)),
                ),
            ]));
        }
        Ok(ordered_object([
            (
                "schema_version",
                Value::String(OUTPUT_SCHEMA_VERSION.to_string()),
            ),
            (
                "converter_schema_version",
                Value::String(CONVERTER_SCHEMA_VERSION.to_string()),
            ),
            ("game", Value::String(self.game.clone())),
            ("server", Value::String(self.server.clone())),
            ("generated", Value::Bool(true)),
            ("generated_by", Value::String(GENERATED_BY.to_string())),
            ("operations", Value::Array(operations)),
        ]))
    }

    fn build_primitives(&self) -> CliOutcome<Value> {
        let mut seen = HashSet::<(String, String)>::new();
        let mut primitives = Vec::new();
        for bundle in &self.bundles {
            for operation in array_field(&bundle.data, "operations") {
                let operation_id = required_string(operation, "id")?;
                if !seen.insert((bundle.task_id.clone(), operation_id.clone())) {
                    continue;
                }
                let verify_template = operation
                    .get("verify_template")
                    .and_then(Value::as_str)
                    .map(template_target_id)
                    .map(Value::String)
                    .unwrap_or(Value::Null);
                let guard = self.operation_guard(bundle, operation)?;
                let click = self.operation_click(bundle, operation, &guard)?;
                primitives.push(ordered_object([
                    ("id", Value::String(operation_id)),
                    ("task_id", Value::String(bundle.task_id.clone())),
                    (
                        "purpose",
                        Value::String(string_field(operation, "purpose").unwrap_or_default()),
                    ),
                    (
                        "from",
                        operation.get("from").cloned().unwrap_or(Value::Null),
                    ),
                    ("to", operation.get("to").cloned().unwrap_or(Value::Null)),
                    ("click", click),
                    (
                        "expect_after",
                        operation
                            .get("expect_after")
                            .cloned()
                            .unwrap_or(Value::Null),
                    ),
                    ("verify_template", verify_template),
                    ("guard", guard),
                    (
                        "unguarded_trusted_coordinate",
                        Value::Bool(
                            operation
                                .get("unguarded_trusted_coordinate")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                        ),
                    ),
                    (
                        "consumes",
                        operation
                            .get("consumes")
                            .cloned()
                            .unwrap_or_else(|| Value::Array(Vec::new())),
                    ),
                    (
                        "produces",
                        operation
                            .get("produces")
                            .cloned()
                            .unwrap_or_else(|| Value::Array(Vec::new())),
                    ),
                ]));
            }
        }
        Ok(ordered_object([
            (
                "schema_version",
                Value::String(OUTPUT_SCHEMA_VERSION.to_string()),
            ),
            (
                "converter_schema_version",
                Value::String(CONVERTER_SCHEMA_VERSION.to_string()),
            ),
            ("game", Value::String(self.game.clone())),
            ("server", Value::String(self.server.clone())),
            ("generated", Value::Bool(true)),
            ("generated_by", Value::String(GENERATED_BY.to_string())),
            ("primitives", Value::Array(primitives)),
        ]))
    }

    fn operation_click(
        &self,
        bundle: &Bundle,
        operation: &Value,
        guard: &Value,
    ) -> CliOutcome<Value> {
        let click = required_field(operation, "click")?;
        if click.get("kind").and_then(Value::as_str) == Some("offset") {
            return Ok(click.clone());
        }
        let Some(rect_move) = self.operation_rect_move(bundle, operation)? else {
            return Ok(click.clone());
        };
        let target_id = guard
            .get("target_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CliError::package_invalid(format!(
                    "operation '{}' has rect_move but cannot resolve a template guard target",
                    required_string(operation, "id").unwrap_or_else(|_| "<unknown>".to_string())
                ))
            })?;
        Ok(ordered_object([
            ("kind", Value::String("offset".to_string())),
            ("target_id", Value::String(target_id.to_string())),
            ("offset", rect_move),
        ]))
    }

    fn operation_rect_move(&self, bundle: &Bundle, operation: &Value) -> CliOutcome<Option<Value>> {
        if let Some(rect_move) = operation.get("rect_move") {
            return Ok(Some(rect_move.clone()));
        }
        let operation_id = required_string(operation, "id")?;
        let source = self.enrich_template_source(operation, &operation_id)?;
        if let Some(rect_move) = source.get("rect_move") {
            return Ok(Some(rect_move.clone()));
        }
        if let Some(verify_template) = operation
            .get("verify_template")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            if let Some(verify) =
                array_field(&bundle.data, "verify_templates")
                    .iter()
                    .find(|entry| {
                        entry.get("template").and_then(Value::as_str) == Some(verify_template)
                    })
            {
                let target_id = required_string(verify, "id")?;
                let source = self.enrich_template_source(verify, &target_id)?;
                if let Some(rect_move) = source.get("rect_move") {
                    return Ok(Some(rect_move.clone()));
                }
            }
            if let Some(anchor) = array_field(&bundle.data, "anchors").iter().find(|entry| {
                entry.get("template").and_then(Value::as_str) == Some(verify_template)
            }) {
                let anchor_id = required_string(anchor, "id")?;
                let source = self.enrich_template_source(anchor, &anchor_id)?;
                if let Some(rect_move) = source.get("rect_move") {
                    return Ok(Some(rect_move.clone()));
                }
            }
        }
        let from = required_string(operation, "from")?;
        if let Some(anchor) = array_field(&bundle.data, "anchors")
            .iter()
            .find(|entry| entry.get("id").and_then(Value::as_str) == Some(from.as_str()))
        {
            let source = self.enrich_template_source(anchor, &from)?;
            if let Some(rect_move) = source.get("rect_move") {
                return Ok(Some(rect_move.clone()));
            }
        }
        Ok(None)
    }

    fn operation_guard(&self, bundle: &Bundle, operation: &Value) -> CliOutcome<Value> {
        if let Some(guard) = operation.get("guard") {
            return Ok(guard.clone());
        }
        if operation
            .get("unguarded_trusted_coordinate")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Ok(Value::Null);
        }
        let operation_id = required_string(operation, "id")?;
        if let Some(verify_template) = operation
            .get("verify_template")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            if let Some(verify) =
                array_field(&bundle.data, "verify_templates")
                    .iter()
                    .find(|entry| {
                        entry.get("template").and_then(Value::as_str) == Some(verify_template)
                    })
            {
                return self.operation_guard_from_verify_template(
                    operation,
                    verify,
                    verify_template,
                );
            }
            if let Some(anchor) = array_field(&bundle.data, "anchors").iter().find(|entry| {
                entry.get("template").and_then(Value::as_str) == Some(verify_template)
            }) {
                return self.operation_guard_from_anchor(operation, anchor, verify_template);
            }
            return self.operation_guard_from_operation_verify_template(operation, verify_template);
        }
        self.operation_guard_from_source_anchor(bundle, operation, &operation_id)
    }

    fn operation_guard_from_verify_template(
        &self,
        operation: &Value,
        verify: &Value,
        verify_template: &str,
    ) -> CliOutcome<Value> {
        let target_id = required_string(verify, "id")?;
        let expected_rect =
            region_to_guard_rect(required_field(verify, "region")?, &self.coordinate_space)?;
        Ok(ordered_object([
            (
                "page_id",
                page_or_any(&self.game, &required_string(operation, "from")?),
            ),
            ("target_id", Value::String(target_id)),
            ("expected_rect", expected_rect),
            (
                "verify_template",
                Value::String(verify_template.to_string()),
            ),
        ]))
    }

    fn operation_guard_from_anchor(
        &self,
        operation: &Value,
        anchor: &Value,
        verify_template: &str,
    ) -> CliOutcome<Value> {
        let target_id = anchor_target_id(&required_string(anchor, "id")?);
        let expected_rect =
            region_to_guard_rect(required_field(anchor, "region")?, &self.coordinate_space)?;
        Ok(ordered_object([
            (
                "page_id",
                page_or_any(&self.game, &required_string(operation, "from")?),
            ),
            ("target_id", Value::String(target_id)),
            ("expected_rect", expected_rect),
            (
                "verify_template",
                Value::String(verify_template.to_string()),
            ),
        ]))
    }

    fn operation_guard_from_operation_verify_template(
        &self,
        operation: &Value,
        verify_template: &str,
    ) -> CliOutcome<Value> {
        Ok(ordered_object([
            (
                "page_id",
                page_or_any(&self.game, &required_string(operation, "from")?),
            ),
            (
                "target_id",
                Value::String(template_target_id(verify_template)),
            ),
            (
                "expected_rect",
                click_to_guard_rect(required_field(operation, "click")?)?,
            ),
            (
                "verify_template",
                Value::String(verify_template.to_string()),
            ),
        ]))
    }

    fn operation_guard_from_source_anchor(
        &self,
        bundle: &Bundle,
        operation: &Value,
        operation_id: &str,
    ) -> CliOutcome<Value> {
        let from = required_string(operation, "from")?;
        let anchor = array_field(&bundle.data, "anchors")
            .iter()
            .find(|entry| entry.get("id").and_then(Value::as_str) == Some(from.as_str()))
            .ok_or_else(|| {
                CliError::package_invalid(format!(
                    "operation '{operation_id}' cannot synthesize guard without verify_template or a matching source anchor; add guard or set unguarded_trusted_coordinate"
                ))
            })?;
        let template = required_string(anchor, "template")?;
        self.operation_guard_from_anchor(operation, anchor, &template)
    }

    fn declared_anchor_ids(&self) -> BTreeSet<String> {
        let mut ids = BTreeSet::new();
        for bundle in &self.bundles {
            for anchor in array_field(&bundle.data, "anchors") {
                if let Some(id) = anchor.get("id").and_then(Value::as_str) {
                    ids.insert(id.to_string());
                }
            }
        }
        ids
    }
}

impl Bundle {
    pub(super) fn task_json_path(&self) -> PathBuf {
        self.dir.join("task.json")
    }
}

fn load_bundles(ops_dir: &Path) -> CliOutcome<Vec<Bundle>> {
    let mut entries = fs::read_dir(ops_dir)
        .map_err(|err| {
            CliError::package_invalid(format!("failed to read {}: {err}", ops_dir.display()))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            CliError::package_invalid(format!("failed to read {}: {err}", ops_dir.display()))
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    let mut bundles = Vec::new();
    for entry in entries {
        let dir = entry.path();
        let task_json = dir.join("task.json");
        if !dir.is_dir() || !task_json.is_file() {
            continue;
        }
        let data = read_json_value(&task_json)?;
        let task_id = required_string(&data, "task_id")?;
        bundles.push(Bundle { task_id, dir, data });
    }
    Ok(bundles)
}

fn resource_ids(resources: &Value) -> CliOutcome<HashSet<String>> {
    let mut ids = HashSet::new();
    for resource in array_field(resources, "resources") {
        ids.insert(required_string(resource, "id")?);
    }
    Ok(ids)
}

fn validate_click_shape(path: &Path, operation: &Value, errors: &mut Vec<String>) {
    let Some(click) = operation.get("click").and_then(Value::as_object) else {
        errors.push(format!(
            "{}: op {:?} missing click object",
            path.display(),
            operation.get("id").and_then(Value::as_str)
        ));
        return;
    };
    match click.get("kind").and_then(Value::as_str) {
        Some("point") => require_click_keys(path, operation, click, &["x", "y"], errors, "click"),
        Some("long_press") | Some("long_tap") => require_click_keys(
            path,
            operation,
            click,
            &["x", "y", "duration_ms"],
            errors,
            "click",
        ),
        Some("rect") | Some("specific_rect") => require_click_keys(
            path,
            operation,
            click,
            &["x", "y", "width", "height"],
            errors,
            "click",
        ),
        Some("offset") => {
            require_click_keys(path, operation, click, &["offset"], errors, "click");
            let Some(offset) = click.get("offset").and_then(Value::as_object) else {
                errors.push(format!(
                    "{}: op {:?} click.offset must be a rect object",
                    path.display(),
                    operation.get("id").and_then(Value::as_str)
                ));
                return;
            };
            require_click_keys(
                path,
                operation,
                offset,
                &["x", "y", "width", "height"],
                errors,
                "click.offset",
            );
        }
        Some("drag") => {
            require_click_keys(
                path,
                operation,
                click,
                &["from", "to", "duration_ms"],
                errors,
                "click",
            );
            for endpoint in ["from", "to"] {
                let Some(rect) = click.get(endpoint).and_then(Value::as_object) else {
                    errors.push(format!(
                        "{}: op {:?} click.{endpoint} must be a rect object",
                        path.display(),
                        operation.get("id").and_then(Value::as_str)
                    ));
                    continue;
                };
                require_click_keys(
                    path,
                    operation,
                    rect,
                    &["x", "y", "width", "height"],
                    errors,
                    &format!("click.{endpoint}"),
                );
            }
        }
        other => errors.push(format!(
            "{}: op {:?} unknown click kind {other:?}",
            path.display(),
            operation.get("id").and_then(Value::as_str)
        )),
    }
}

fn require_click_keys(
    path: &Path,
    operation: &Value,
    object: &Map<String, Value>,
    keys: &[&str],
    errors: &mut Vec<String>,
    label: &str,
) {
    for key in keys {
        if !object.contains_key(*key) {
            errors.push(format!(
                "{}: op {:?} {label} missing {key:?}",
                path.display(),
                operation.get("id").and_then(Value::as_str)
            ));
        }
    }
}

fn copy_maa_template_field(
    out: &mut Map<String, Value>,
    compiled: &Value,
    output_key: &str,
    input_keys: &[&str],
) -> CliOutcome<()> {
    if out.contains_key(output_key) {
        return Ok(());
    }
    let Some((input_key, value)) = input_keys
        .iter()
        .find_map(|key| compiled.get(*key).map(|value| (*key, value)))
    else {
        return Ok(());
    };
    let value = match output_key {
        "method" => normalize_maa_method(value)?,
        "mask" if input_key == "maskRange" => normalize_maa_mask_range(value)?,
        "rect_move" if input_key == "rectMove" => normalize_maa_rect(value)?,
        _ => value.clone(),
    };
    out.insert(output_key.to_string(), value);
    Ok(())
}

fn normalize_maa_method(value: &Value) -> CliOutcome<Value> {
    let method = value.as_str().ok_or_else(|| {
        CliError::package_invalid("MAA template method must be a string when provided")
    })?;
    let normalized = match method {
        "ncc" | "NCC" | "MatchTemplate" | "match_template" | "TemplateMatch" => "ncc",
        "rgb_count" | "RGBCount" | "rgbCount" => "rgb_count",
        "hsv_count" | "HSVCount" | "hsvCount" => "hsv_count",
        other => other,
    };
    Ok(Value::String(normalized.to_string()))
}

fn normalize_maa_mask_range(value: &Value) -> CliOutcome<Value> {
    if let Some(object) = value.as_object() {
        if object.contains_key("type") {
            return Ok(value.clone());
        }
        let lower = required_u8_field(value, "lower")?;
        let upper = required_u8_field(value, "upper")?;
        return Ok(json!({"type":"range","lower":lower,"upper":upper}));
    }
    let values = value.as_array().ok_or_else(|| {
        CliError::package_invalid("MAA maskRange must be [lower, upper] or an object")
    })?;
    if values.len() != 2 {
        return Err(CliError::package_invalid(
            "MAA maskRange must contain exactly two values",
        ));
    }
    let lower = value_to_u8(&values[0], "MAA maskRange lower")?;
    let upper = value_to_u8(&values[1], "MAA maskRange upper")?;
    Ok(json!({"type":"range","lower":lower,"upper":upper}))
}

fn normalize_maa_rect(value: &Value) -> CliOutcome<Value> {
    if let Some(object) = value.as_object() {
        let x = required_i64_field(value, "x")?;
        let y = required_i64_field(value, "y")?;
        let width = object
            .get("width")
            .or_else(|| object.get("w"))
            .ok_or_else(|| CliError::package_invalid("MAA rectMove object missing width"))?
            .as_i64()
            .ok_or_else(|| CliError::package_invalid("MAA rectMove width must be an integer"))?;
        let height = object
            .get("height")
            .or_else(|| object.get("h"))
            .ok_or_else(|| CliError::package_invalid("MAA rectMove object missing height"))?
            .as_i64()
            .ok_or_else(|| CliError::package_invalid("MAA rectMove height must be an integer"))?;
        return Ok(json!({"x":x,"y":y,"width":width,"height":height}));
    }
    let values = value.as_array().ok_or_else(|| {
        CliError::package_invalid("MAA rectMove must be [x, y, width, height] or an object")
    })?;
    if values.len() != 4 {
        return Err(CliError::package_invalid(
            "MAA rectMove must contain exactly four values",
        ));
    }
    Ok(json!({
        "x": value_to_i64(&values[0], "MAA rectMove x")?,
        "y": value_to_i64(&values[1], "MAA rectMove y")?,
        "width": value_to_i64(&values[2], "MAA rectMove width")?,
        "height": value_to_i64(&values[3], "MAA rectMove height")?
    }))
}

fn pack_target(
    source: &Value,
    id: &str,
    template_path: &str,
    region: Value,
    threshold: Value,
    color_check: Option<Value>,
    click: Option<Value>,
) -> Value {
    let mut target = ordered_map([
        ("type", Value::String("template".to_string())),
        ("id", Value::String(id.to_string())),
        ("template_path", Value::String(template_path.to_string())),
        ("region", region),
        ("threshold", threshold),
    ]);
    if let Some(click) = click {
        target.insert("click".to_string(), click);
    }
    for key in ["method", "mask", "rect_move"] {
        if let Some(value) = source.get(key) {
            target.insert(key.to_string(), value.clone());
        }
    }
    if let Some(color_check) = color_check {
        target.insert("color_check".to_string(), color_check);
    }
    Value::Object(target)
}

fn color_target(id: &str, region: Value, expected: Value, click: Option<Value>) -> Value {
    let mut target = ordered_map([
        ("type", Value::String("color".to_string())),
        ("id", Value::String(id.to_string())),
        ("region", region),
        ("expected", expected),
    ]);
    if let Some(click) = click {
        target.insert("click".to_string(), click);
    }
    Value::Object(target)
}

fn add_first_target(
    targets: &mut HashMap<String, Value>,
    order: &mut Vec<String>,
    id: String,
    target: Value,
) {
    if targets.contains_key(&id) {
        return;
    }
    targets.insert(id.clone(), target);
    order.push(id);
}

fn propagate_color_checks(targets: &mut HashMap<String, Value>, order: &[String]) {
    let mut by_basename = HashMap::<String, Value>::new();
    for id in order {
        let Some(target) = targets.get(id).and_then(Value::as_object) else {
            continue;
        };
        if !id.starts_with("page/") {
            continue;
        }
        let Some(color_check) = target.get("color_check") else {
            continue;
        };
        if let Some(template_path) = target.get("template_path").and_then(Value::as_str)
            && let Some(name) = Path::new(template_path)
                .file_name()
                .and_then(|name| name.to_str())
        {
            by_basename
                .entry(name.to_string())
                .or_insert_with(|| color_check.clone());
        }
    }
    for id in order {
        if !id.starts_with("template/") {
            continue;
        }
        let Some(target) = targets.get_mut(id).and_then(Value::as_object_mut) else {
            continue;
        };
        if target.contains_key("color_check") {
            continue;
        }
        let Some(template_path) = target.get("template_path").and_then(Value::as_str) else {
            continue;
        };
        let Some(name) = Path::new(template_path)
            .file_name()
            .and_then(|name| name.to_str())
        else {
            continue;
        };
        if let Some(color_check) = by_basename.get(name) {
            target.insert("color_check".to_string(), color_check.clone());
        }
    }
}

fn add_page(
    game: &str,
    anchor_id: &str,
    declared_anchor_ids: &BTreeSet<String>,
    pages: &mut HashMap<String, Value>,
    order: &mut Vec<String>,
) {
    if anchor_id.is_empty() || anchor_id == "any" {
        return;
    }
    let page_id = page_id(game, anchor_id);
    if pages.contains_key(&page_id) {
        return;
    }
    let requirements = resolve_page_requirements(anchor_id, declared_anchor_ids);
    let required = requirements
        .required
        .into_iter()
        .map(Value::String)
        .collect();
    let any_of = requirements
        .any_of
        .into_iter()
        .map(|group| Value::Array(group.into_iter().map(Value::String).collect()))
        .collect::<Vec<_>>();
    let mut page = ordered_map([
        ("id", Value::String(page_id.clone())),
        ("required", Value::Array(required)),
        ("optional", Value::Array(Vec::new())),
        ("forbidden", Value::Array(Vec::new())),
    ]);
    if !any_of.is_empty() {
        page.insert("any_of".to_string(), Value::Array(any_of));
    }
    pages.insert(page_id.clone(), Value::Object(page));
    order.push(page_id);
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PageRequirements {
    required: Vec<String>,
    any_of: Vec<Vec<String>>,
}

fn resolve_page_requirements(
    anchor_id: &str,
    declared_anchor_ids: &BTreeSet<String>,
) -> PageRequirements {
    if declared_anchor_ids.contains(anchor_id) {
        return PageRequirements {
            required: vec![anchor_target_id(anchor_id)],
            any_of: Vec::new(),
        };
    }
    let prefix = format!("{anchor_id}_");
    let variants = declared_anchor_ids
        .iter()
        .filter(|id| id.starts_with(&prefix))
        .map(|id| anchor_target_id(id))
        .collect::<Vec<_>>();
    if variants.is_empty() {
        PageRequirements {
            required: vec![anchor_target_id(anchor_id)],
            any_of: Vec::new(),
        }
    } else {
        PageRequirements {
            required: Vec::new(),
            any_of: vec![variants],
        }
    }
}

fn is_page_change(operation: &Value) -> bool {
    let Some(to) = operation.get("to").and_then(Value::as_str) else {
        return false;
    };
    let from = operation.get("from").and_then(Value::as_str);
    if from == Some(to) {
        return false;
    }
    from != Some("any") || to != "any"
}

fn has_unresolved_coords(bundle: &Value) -> bool {
    array_field(bundle, "operations").iter().any(|operation| {
        let Some(click) = operation.get("click") else {
            return false;
        };
        click.get("kind").and_then(Value::as_str) == Some("point")
            && click.get("x").and_then(Value::as_i64) == Some(0)
            && click.get("y").and_then(Value::as_i64) == Some(0)
    })
}

fn region_to_pack(region: &Value) -> CliOutcome<Value> {
    match region.get("mode").and_then(Value::as_str) {
        Some("full_frame") => Ok(Value::String(FULL_FRAME_SENTINEL.to_string())),
        Some("rect") => {
            let rect = required_field(region, "rect")?;
            Ok(ordered_object([
                ("x", required_field(rect, "x")?.clone()),
                ("y", required_field(rect, "y")?.clone()),
                ("width", required_field(rect, "width")?.clone()),
                ("height", required_field(rect, "height")?.clone()),
            ]))
        }
        other => Err(CliError::package_invalid(format!(
            "unknown region mode: {other:?}"
        ))),
    }
}

fn region_to_guard_rect(region: &Value, coordinate_space: &Value) -> CliOutcome<Value> {
    match region.get("mode").and_then(Value::as_str) {
        Some("rect") => {
            let rect = required_field(region, "rect")?;
            Ok(ordered_object([
                ("x", required_field(rect, "x")?.clone()),
                ("y", required_field(rect, "y")?.clone()),
                ("width", required_field(rect, "width")?.clone()),
                ("height", required_field(rect, "height")?.clone()),
            ]))
        }
        Some("full_frame") => Ok(ordered_object([
            ("x", Value::Number(0.into())),
            ("y", Value::Number(0.into())),
            ("width", required_field(coordinate_space, "width")?.clone()),
            (
                "height",
                required_field(coordinate_space, "height")?.clone(),
            ),
        ])),
        other => Err(CliError::package_invalid(format!(
            "unknown guard region mode: {other:?}"
        ))),
    }
}

fn color_check_to_pack(color_check: Option<&Value>) -> CliOutcome<Option<Value>> {
    let Some(color_check) = color_check else {
        return Ok(None);
    };
    if color_check.is_null() {
        return Ok(None);
    }
    let mut output = color_check.clone();
    if let Some(object) = output.as_object_mut()
        && let Some(region) = color_check.get("region")
    {
        object.insert("region".to_string(), region_to_pack(region)?);
    }
    Ok(Some(output))
}

fn click_to_navigation(click: &Value) -> CliOutcome<Value> {
    match click.get("kind").and_then(Value::as_str) {
        Some("point") => Ok(ordered_object([
            ("kind", Value::String("point".to_string())),
            (
                "point",
                Value::String(format!(
                    "{},{}",
                    required_field(click, "x")?,
                    required_field(click, "y")?
                )),
            ),
        ])),
        Some("long_press") | Some("long_tap") => Ok(ordered_object([
            ("kind", Value::String("long_press".to_string())),
            ("x", required_field(click, "x")?.clone()),
            ("y", required_field(click, "y")?.clone()),
            ("duration_ms", required_field(click, "duration_ms")?.clone()),
        ])),
        Some("rect") | Some("specific_rect") => Ok(ordered_object([
            ("kind", Value::String("rect".to_string())),
            ("x", required_field(click, "x")?.clone()),
            ("y", required_field(click, "y")?.clone()),
            ("width", required_field(click, "width")?.clone()),
            ("height", required_field(click, "height")?.clone()),
        ])),
        Some("offset") => Ok(ordered_object([
            ("kind", Value::String("offset".to_string())),
            (
                "target_id",
                click.get("target_id").cloned().unwrap_or(Value::Null),
            ),
            ("offset", required_field(click, "offset")?.clone()),
        ])),
        Some("drag") => Ok(ordered_object([
            ("kind", Value::String("drag".to_string())),
            ("from", required_field(click, "from")?.clone()),
            ("to", required_field(click, "to")?.clone()),
            ("duration_ms", required_field(click, "duration_ms")?.clone()),
        ])),
        other => Err(CliError::package_invalid(format!(
            "unknown click kind: {other:?}"
        ))),
    }
}

fn click_to_guard_rect(click: &Value) -> CliOutcome<Value> {
    match click.get("kind").and_then(Value::as_str) {
        Some("point") | Some("long_press") | Some("long_tap") => Ok(ordered_object([
            ("x", required_field(click, "x")?.clone()),
            ("y", required_field(click, "y")?.clone()),
            ("width", Value::Number(1.into())),
            ("height", Value::Number(1.into())),
        ])),
        Some("rect") | Some("specific_rect") => Ok(ordered_object([
            ("x", required_field(click, "x")?.clone()),
            ("y", required_field(click, "y")?.clone()),
            ("width", required_field(click, "width")?.clone()),
            ("height", required_field(click, "height")?.clone()),
        ])),
        Some("drag") => {
            let rect = required_field(click, "from")?;
            Ok(ordered_object([
                ("x", required_field(rect, "x")?.clone()),
                ("y", required_field(rect, "y")?.clone()),
                ("width", required_field(rect, "width")?.clone()),
                ("height", required_field(rect, "height")?.clone()),
            ]))
        }
        other => Err(CliError::package_invalid(format!(
            "cannot synthesize guard expected_rect from click kind: {other:?}"
        ))),
    }
}

fn page_or_any(game: &str, anchor_id: &str) -> Value {
    if anchor_id == "any" {
        Value::String("any".to_string())
    } else {
        Value::String(page_id(game, anchor_id))
    }
}

fn page_id(game: &str, anchor_id: &str) -> String {
    format!("{game}/{anchor_id}")
}

fn normalize_page_rule_id(game: &str, page_key: &str) -> String {
    if page_key.contains('/') {
        page_key.to_string()
    } else {
        page_id(game, page_key)
    }
}

fn anchor_target_id(anchor_id: &str) -> String {
    format!("page/{anchor_id}")
}

fn template_target_id(template_rel: &str) -> String {
    let stem = Path::new(template_rel)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(template_rel);
    let upper = stem.to_ascii_uppercase();
    for (prefix, namespace) in [
        ("BUTTON_", "button"),
        ("POPUP_", "popup"),
        ("PAGE_", "page"),
    ] {
        if upper.starts_with(prefix) {
            return format!("{namespace}/{}", stem[prefix.len()..].to_ascii_lowercase());
        }
    }
    format!("template/{}", stem.to_ascii_lowercase())
}

fn validate_pack_targets_exist(root: &Path, pack: &Value) -> CliOutcome<()> {
    let mut errors = Vec::new();
    for target in array_field(pack, "targets") {
        let Some(path) = target.get("template_path").and_then(Value::as_str) else {
            continue;
        };
        if !root.join(path).is_file() {
            errors.push(format!(
                "pack target {:?} template_path missing on disk: {path}",
                target.get("id").and_then(Value::as_str)
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(CliError::package_invalid(format!(
            "resource convert validation failed:\n  - {}",
            errors.join("\n  - ")
        )))
    }
}

fn validate_page_rule_targets(pack: &Value, bundles: &[Bundle]) -> CliOutcome<()> {
    let targets = array_field(pack, "targets")
        .iter()
        .filter_map(|target| target.get("id").and_then(Value::as_str).map(str::to_string))
        .collect::<HashSet<_>>();
    let mut errors = Vec::new();
    for bundle in bundles {
        let Some(rules) = bundle.data.get("page_rules").and_then(Value::as_object) else {
            continue;
        };
        let source = bundle.task_json_path();
        for field in ["required", "optional", "forbidden"] {
            for (page_key, rule) in rules {
                for target in array_field(rule, field) {
                    let target_id = target.as_str().unwrap_or("");
                    if targets.contains(target_id) {
                        continue;
                    }
                    errors.push(format!(
                        "{}: page_rules.{page_key}.{field} target '{target_id}' does not exist in pack",
                        source.display()
                    ));
                }
            }
        }
        for (page_key, rule) in rules {
            for group in array_field(rule, "any_of") {
                for target in group.as_array().into_iter().flatten() {
                    let target_id = target.as_str().unwrap_or("");
                    if targets.contains(target_id) {
                        continue;
                    }
                    errors.push(format!(
                        "{}: page_rules.{page_key}.any_of target '{target_id}' does not exist in pack",
                        source.display()
                    ));
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(CliError::package_invalid(format!(
            "resource convert page rule validation failed:\n  - {}",
            errors.join("\n  - ")
        )))
    }
}

fn validate_converted_guard_references(
    pack: &Value,
    pages: &Value,
    primitives: &Value,
) -> CliOutcome<()> {
    let game = pack.get("game").and_then(Value::as_str).unwrap_or("");
    let targets = array_field(pack, "targets")
        .iter()
        .filter_map(|target| {
            target
                .get("id")
                .and_then(Value::as_str)
                .map(|id| (id.to_string(), target))
        })
        .collect::<HashMap<_, _>>();
    let page_ids = array_field(pages, "pages")
        .iter()
        .filter_map(|page| page.get("id").and_then(Value::as_str).map(str::to_string))
        .collect::<HashSet<_>>();
    let mut errors = Vec::new();
    for operation in array_field(primitives, "primitives") {
        let operation_id = operation
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("<unknown>");
        let Some(guard) = operation.get("guard").filter(|guard| !guard.is_null()) else {
            continue;
        };
        let page_id = guard.get("page_id").and_then(Value::as_str).unwrap_or("");
        if !converted_page_id_exists(game, &page_ids, page_id) {
            errors.push(format!(
                "operation '{operation_id}' guard.page_id '{page_id}' does not exist in pages"
            ));
        }
        let target_id = guard.get("target_id").and_then(Value::as_str).unwrap_or("");
        let Some(target) = targets.get(target_id) else {
            errors.push(format!(
                "operation '{operation_id}' guard.target_id '{target_id}' does not exist in pack"
            ));
            continue;
        };
        if guard
            .get("verify_template")
            .and_then(Value::as_str)
            .is_some()
            && target.get("type").and_then(Value::as_str) != Some("template")
        {
            errors.push(format!(
                "operation '{operation_id}' guard.verify_template points to non-template target '{target_id}'"
            ));
        }
        if guard.get("color_probe").and_then(Value::as_str).is_some()
            && target.get("type").and_then(Value::as_str) != Some("color")
        {
            errors.push(format!(
                "operation '{operation_id}' guard.color_probe points to non-color target '{target_id}'"
            ));
        }
        if operation.pointer("/click/kind").and_then(Value::as_str) == Some("offset") {
            if guard
                .get("verify_template")
                .and_then(Value::as_str)
                .is_none()
            {
                errors.push(format!(
                    "operation '{operation_id}' offset click requires a template guard that can produce matched_rect"
                ));
            }
            if target.get("type").and_then(Value::as_str) != Some("template") {
                errors.push(format!(
                    "operation '{operation_id}' offset click guard target '{target_id}' must be a template target"
                ));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(CliError::package_invalid(format!(
            "resource convert guard validation failed:\n  - {}",
            errors.join("\n  - ")
        )))
    }
}

fn converted_page_id_exists(game: &str, page_ids: &HashSet<String>, guard_page_id: &str) -> bool {
    guard_page_id == "any"
        || page_ids.contains(guard_page_id)
        || (!game.is_empty() && page_ids.contains(&page_id(game, guard_page_id)))
}

fn read_json_value(path: &Path) -> CliOutcome<Value> {
    let text = fs::read_to_string(path).map_err(|err| {
        CliError::package_invalid(format!("failed to read {}: {err}", path.display()))
    })?;
    serde_json::from_str(&text).map_err(|err| {
        CliError::package_invalid(format!("failed to parse {}: {err}", path.display()))
    })
}

fn write_json_file(path: &Path, value: &Value) -> CliOutcome<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::package_invalid(format!("failed to create {}: {err}", parent.display()))
        })?;
    }
    let mut text = serde_json::to_string_pretty(value).map_err(|err| {
        CliError::package_invalid(format!("failed to serialize {}: {err}", path.display()))
    })?;
    text.push('\n');
    fs::write(path, text).map_err(|err| {
        CliError::package_invalid(format!("failed to write {}: {err}", path.display()))
    })
}

fn repo_rel(root: &Path, path: &Path) -> CliOutcome<String> {
    let rel = path.strip_prefix(root).map_err(|err| {
        CliError::package_invalid(format!(
            "path {} is outside repo {}: {err}",
            path.display(),
            root.display()
        ))
    })?;
    Ok(rel
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

fn ordered_object<const N: usize>(fields: [(&str, Value); N]) -> Value {
    Value::Object(ordered_map(fields))
}

fn ordered_map<const N: usize>(fields: [(&str, Value); N]) -> Map<String, Value> {
    let mut map = Map::new();
    for (key, value) in fields {
        map.insert(key.to_string(), value);
    }
    map
}

fn array_field<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn append_unique_strings(
    page: &mut Value,
    field: &str,
    values: &Value,
    source: &Path,
) -> CliOutcome<()> {
    let values = values.as_array().ok_or_else(|| {
        CliError::package_invalid(format!(
            "{}: page_rules.{field} must be an array",
            source.display()
        ))
    })?;
    let Some(page_object) = page.as_object_mut() else {
        return Err(CliError::package_invalid(format!(
            "{}: generated page is not an object",
            source.display()
        )));
    };
    let target_list = page_object
        .get_mut(field)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            CliError::package_invalid(format!(
                "{}: generated page missing {field} array",
                source.display()
            ))
        })?;
    let mut seen = target_list
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    for value in values {
        let Some(id) = value.as_str() else {
            return Err(CliError::package_invalid(format!(
                "{}: page_rules.{field} entries must be strings",
                source.display()
            )));
        };
        if seen.insert(id.to_string()) {
            target_list.push(Value::String(id.to_string()));
        }
    }
    Ok(())
}

fn append_any_of_groups(page: &mut Value, groups: &Value, source: &Path) -> CliOutcome<()> {
    let groups = groups.as_array().ok_or_else(|| {
        CliError::package_invalid(format!(
            "{}: page_rules.any_of must be an array",
            source.display()
        ))
    })?;
    let Some(page_object) = page.as_object_mut() else {
        return Err(CliError::package_invalid(format!(
            "{}: generated page is not an object",
            source.display()
        )));
    };
    page_object
        .entry("any_of")
        .or_insert_with(|| Value::Array(Vec::new()));
    let target_groups = page_object
        .get_mut("any_of")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            CliError::package_invalid(format!(
                "{}: generated page missing any_of array",
                source.display()
            ))
        })?;
    let mut seen_groups = target_groups
        .iter()
        .map(canonical_group_key)
        .collect::<CliOutcome<BTreeSet<_>>>()?;
    for group in groups {
        let group_values = group.as_array().ok_or_else(|| {
            CliError::package_invalid(format!(
                "{}: page_rules.any_of entries must be arrays",
                source.display()
            ))
        })?;
        let mut group_ids = Vec::new();
        for value in group_values {
            let Some(id) = value.as_str() else {
                return Err(CliError::package_invalid(format!(
                    "{}: page_rules.any_of target entries must be strings",
                    source.display()
                )));
            };
            group_ids.push(id.to_string());
        }
        let key = group_ids.join("\u{1f}");
        if seen_groups.insert(key) {
            target_groups.push(Value::Array(
                group_ids.into_iter().map(Value::String).collect(),
            ));
        }
    }
    Ok(())
}

fn canonical_group_key(group: &Value) -> CliOutcome<String> {
    let values = group
        .as_array()
        .ok_or_else(|| CliError::package_invalid("generated page any_of group is not an array"))?;
    let mut ids = Vec::new();
    for value in values {
        let Some(id) = value.as_str() else {
            return Err(CliError::package_invalid(
                "generated page any_of target is not a string",
            ));
        };
        ids.push(id.to_string());
    }
    Ok(ids.join("\u{1f}"))
}

fn required_field<'a>(value: &'a Value, key: &str) -> CliOutcome<&'a Value> {
    value
        .get(key)
        .ok_or_else(|| CliError::package_invalid(format!("missing field {key}")))
}

fn required_string(value: &Value, key: &str) -> CliOutcome<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| CliError::package_invalid(format!("missing string field {key}")))
}

fn required_i64_field(value: &Value, key: &str) -> CliOutcome<i64> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| CliError::package_invalid(format!("missing integer field {key}")))
}

fn required_u8_field(value: &Value, key: &str) -> CliOutcome<u8> {
    let raw = value
        .get(key)
        .ok_or_else(|| CliError::package_invalid(format!("missing integer field {key}")))?;
    value_to_u8(raw, key)
}

fn value_to_i64(value: &Value, label: &str) -> CliOutcome<i64> {
    value
        .as_i64()
        .ok_or_else(|| CliError::package_invalid(format!("{label} must be an integer")))
}

fn value_to_u8(value: &Value, label: &str) -> CliOutcome<u8> {
    let raw = value_to_i64(value, label)?;
    u8::try_from(raw).map_err(|_| {
        CliError::package_invalid(format!("{label} must be an integer between 0 and 255"))
    })
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn first_server_scope(bundle: &Value) -> Option<String> {
    bundle
        .get("server_scope")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn default_locale(game: &str) -> &'static str {
    match game {
        "arknights" => "zh-CN",
        "azurlane" => "ja-JP",
        "bluearchive" => "ja-JP",
        _ => "ja-JP",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_target_ids_like_python_converter() {
        assert_eq!(anchor_target_id("home"), "page/home");
        assert_eq!(
            template_target_id("assets/BUTTON_ALL_COLLECT.png"),
            "button/all_collect"
        );
        assert_eq!(
            template_target_id("assets/POPUP_MOMOTALK.png"),
            "popup/momotalk"
        );
        assert_eq!(template_target_id("assets/PAGE_HOME.png"), "page/home");
        assert_eq!(
            template_target_id("assets/DOCK_CHECK.png"),
            "template/dock_check"
        );
    }

    #[test]
    fn converts_region_and_click_shapes() {
        let rect = json!({"mode":"rect","rect":{"x":1,"y":2,"width":3,"height":4}});
        assert_eq!(
            region_to_pack(&rect).unwrap(),
            json!({"x":1,"y":2,"width":3,"height":4})
        );
        assert_eq!(
            region_to_pack(&json!({"mode":"full_frame"})).unwrap(),
            Value::String("full_frame".to_string())
        );
        assert_eq!(
            click_to_navigation(&json!({"kind":"point","x":12,"y":34})).unwrap(),
            json!({"kind":"point","point":"12,34"})
        );
        assert_eq!(
            click_to_navigation(&json!({"kind":"rect","x":1,"y":2,"width":3,"height":4})).unwrap(),
            json!({"kind":"rect","x":1,"y":2,"width":3,"height":4})
        );
        assert_eq!(
            click_to_navigation(&json!({"kind":"drag","from":{"x":1,"y":2,"width":3,"height":4},"to":{"x":5,"y":6,"width":7,"height":8},"duration_ms":900})).unwrap(),
            json!({"kind":"drag","from":{"x":1,"y":2,"width":3,"height":4},"to":{"x":5,"y":6,"width":7,"height":8},"duration_ms":900})
        );
        assert_eq!(
            click_to_navigation(&json!({"kind":"offset","target_id":"page/home","offset":{"x":1,"y":2,"width":3,"height":4}})).unwrap(),
            json!({"kind":"offset","target_id":"page/home","offset":{"x":1,"y":2,"width":3,"height":4}})
        );
        assert_eq!(
            click_to_navigation(&json!({"kind":"long_press","x":12,"y":34,"duration_ms":700}))
                .unwrap(),
            json!({"kind":"long_press","x":12,"y":34,"duration_ms":700})
        );
    }

    #[test]
    fn resolves_page_anchor_variants_as_any_of_group() {
        let ids = BTreeSet::from([
            "home".to_string(),
            "operator_0".to_string(),
            "operator_1".to_string(),
        ]);
        assert_eq!(
            resolve_page_requirements("home", &ids),
            PageRequirements {
                required: vec!["page/home".to_string()],
                any_of: Vec::new()
            }
        );
        assert_eq!(
            resolve_page_requirements("operator", &ids),
            PageRequirements {
                required: Vec::new(),
                any_of: vec![vec![
                    "page/operator_0".to_string(),
                    "page/operator_1".to_string()
                ]]
            }
        );
    }

    #[test]
    fn build_pages_emits_any_of_for_anchor_variants() {
        let converter = OperationConverter {
            root: PathBuf::from("."),
            game: "arknights".to_string(),
            server: "cn".to_string(),
            locale: "zh-CN".to_string(),
            coordinate_space: json!({"width":1280,"height":720}),
            defaults: json!({"template_threshold":0.9}),
            resource_ids: HashSet::new(),
            bundles: vec![Bundle {
                task_id: "operator-check".to_string(),
                dir: PathBuf::from("operations/operator-check"),
                data: json!({
                    "schema_version": "0.5",
                    "task_id": "operator-check",
                    "anchors": [
                        {"id":"operator_0","template":"assets/OPERATOR_0.png","region":{"mode":"rect","rect":{"x":1,"y":2,"width":3,"height":4}}},
                        {"id":"operator_1","template":"assets/OPERATOR_1.png","region":{"mode":"rect","rect":{"x":5,"y":6,"width":7,"height":8}}}
                    ],
                    "entry_page": "operator",
                    "target_page": "operator",
                    "operations": []
                }),
            }],
            existing_navigation: None,
            maa_task_overlays: HashMap::new(),
        };

        let pages = converter.build_pages().unwrap();
        let operator = pages.pointer("/pages/0").unwrap();
        assert_eq!(operator.pointer("/required"), Some(&json!([])));
        assert_eq!(
            operator.pointer("/any_of"),
            Some(&json!([["page/operator_0", "page/operator_1"]]))
        );
    }

    #[test]
    fn build_pages_applies_page_rules() {
        let converter = OperationConverter {
            root: PathBuf::from("."),
            game: "arknights".to_string(),
            server: "cn".to_string(),
            locale: "zh-CN".to_string(),
            coordinate_space: json!({"width":1280,"height":720}),
            defaults: json!({"template_threshold":0.9}),
            resource_ids: HashSet::new(),
            bundles: vec![Bundle {
                task_id: "home-check".to_string(),
                dir: PathBuf::from("operations/home-check"),
                data: json!({
                    "schema_version": "0.5",
                    "task_id": "home-check",
                    "anchors": [
                        {"id":"home","template":"assets/HOME.png","region":{"mode":"rect","rect":{"x":1,"y":2,"width":3,"height":4}}},
                        {"id":"mission_result_negative","template":"assets/MISSION_RESULT.png","region":{"mode":"rect","rect":{"x":10,"y":20,"width":30,"height":40}}}
                    ],
                    "entry_page": "home",
                    "target_page": "home",
                    "page_rules": {
                        "home": {
                            "optional": ["page/extra_context"],
                            "forbidden": ["page/mission_result_negative"]
                        }
                    },
                    "operations": []
                }),
            }],
            existing_navigation: None,
            maa_task_overlays: HashMap::new(),
        };

        let pages = converter.build_pages().unwrap();
        let home = pages.pointer("/pages/0").unwrap();
        assert_eq!(
            home.pointer("/optional/0").and_then(Value::as_str),
            Some("page/extra_context")
        );
        assert_eq!(
            home.pointer("/forbidden/0").and_then(Value::as_str),
            Some("page/mission_result_negative")
        );
    }

    #[test]
    fn build_pages_rejects_unknown_page_rule() {
        let converter = OperationConverter {
            root: PathBuf::from("."),
            game: "arknights".to_string(),
            server: "cn".to_string(),
            locale: "zh-CN".to_string(),
            coordinate_space: json!({"width":1280,"height":720}),
            defaults: json!({"template_threshold":0.9}),
            resource_ids: HashSet::new(),
            bundles: vec![Bundle {
                task_id: "home-check".to_string(),
                dir: PathBuf::from("operations/home-check"),
                data: json!({
                    "schema_version": "0.5",
                    "task_id": "home-check",
                    "anchors": [{"id":"home","template":"assets/HOME.png","region":{"mode":"rect","rect":{"x":1,"y":2,"width":3,"height":4}}}],
                    "entry_page": "home",
                    "target_page": "home",
                    "page_rules": {"missing": {"forbidden": ["page/home"]}},
                    "operations": []
                }),
            }],
            existing_navigation: None,
            maa_task_overlays: HashMap::new(),
        };

        let err = converter.build_pages().expect_err("unknown page rule");
        assert!(err.message.contains("unknown page"));
    }

    #[test]
    fn validate_page_rule_targets_rejects_missing_targets() {
        let pack = json!({"targets":[{"id":"page/home"}]});
        let bundles = vec![Bundle {
            task_id: "home-check".to_string(),
            dir: PathBuf::from("operations/home-check"),
            data: json!({
                "page_rules": {
                    "home": {
                        "required": ["page/home"],
                        "forbidden": ["page/missing"]
                    }
                }
            }),
        }];

        let err = validate_page_rule_targets(&pack, &bundles).expect_err("missing target");
        assert!(err.message.contains("page/missing"));
    }

    #[test]
    fn color_check_region_is_flattened() {
        let input = json!({
            "region":{"mode":"rect","rect":{"x":1,"y":2,"width":3,"height":4}},
            "expected":[10,20,30]
        });
        assert_eq!(
            color_check_to_pack(Some(&input)).unwrap().unwrap(),
            json!({"region":{"x":1,"y":2,"width":3,"height":4},"expected":[10,20,30]})
        );
    }

    #[test]
    fn build_pack_includes_color_probe_targets() {
        let converter = OperationConverter {
            root: PathBuf::from("."),
            game: "arknights".to_string(),
            server: "cn".to_string(),
            locale: "zh-CN".to_string(),
            coordinate_space: json!({"width":1280,"height":720}),
            defaults: json!({"template_threshold":0.95}),
            resource_ids: HashSet::new(),
            bundles: vec![Bundle {
                task_id: "daily-check".to_string(),
                dir: PathBuf::from("operations/daily-check"),
                data: json!({
                    "schema_version": "0.3",
                    "task_id": "daily-check",
                    "anchors": [],
                    "color_probes": [{
                        "id": "color/home-status",
                        "region": {"mode":"rect","rect":{"x":10,"y":20,"width":30,"height":40}},
                        "expected": [10, 20, 30]
                    }],
                    "operations": []
                }),
            }],
            existing_navigation: None,
            maa_task_overlays: HashMap::new(),
        };

        let pack = converter.build_pack().unwrap();
        let target_value = pack.pointer("/targets/0").expect("color target value");
        let target = target_value.as_object().expect("color target");
        assert_eq!(target.get("type").and_then(Value::as_str), Some("color"));
        assert_eq!(
            target.get("id").and_then(Value::as_str),
            Some("color/home-status")
        );
        assert_eq!(
            target_value.pointer("/region/x").and_then(Value::as_i64),
            Some(10)
        );
        assert_eq!(
            target_value.pointer("/expected/2").and_then(Value::as_u64),
            Some(30)
        );
    }

    #[test]
    fn build_pack_includes_verify_template_targets() {
        let root = std::env::current_dir().unwrap();
        let converter = OperationConverter {
            root: root.clone(),
            game: "arknights".to_string(),
            server: "cn".to_string(),
            locale: "zh-CN".to_string(),
            coordinate_space: json!({"width":1280,"height":720}),
            defaults: json!({"template_threshold":0.95}),
            resource_ids: HashSet::new(),
            bundles: vec![Bundle {
                task_id: "daily-check".to_string(),
                dir: root.join("operations/daily-check"),
                data: json!({
                    "schema_version": "0.3",
                    "task_id": "daily-check",
                    "anchors": [],
                    "verify_templates": [{
                        "id": "template/mail-ready",
                        "template": "assets/VERIFY_MAIL_READY.png",
                        "region": {"mode":"rect","rect":{"x":10,"y":20,"width":30,"height":40}},
                        "threshold": 0.97
                    }],
                    "operations": []
                }),
            }],
            existing_navigation: None,
            maa_task_overlays: HashMap::new(),
        };

        let pack = converter.build_pack().unwrap();
        let target_value = pack
            .pointer("/targets/0")
            .expect("verify-template target value");
        let target = target_value.as_object().expect("verify-template target");
        assert_eq!(target.get("type").and_then(Value::as_str), Some("template"));
        assert_eq!(
            target.get("id").and_then(Value::as_str),
            Some("template/mail-ready")
        );
        assert_eq!(
            target.get("template_path").and_then(Value::as_str),
            Some("operations/daily-check/assets/VERIFY_MAIL_READY.png")
        );
        assert_eq!(
            target_value.pointer("/region/y").and_then(Value::as_i64),
            Some(20)
        );
        assert_eq!(
            target_value.pointer("/threshold").and_then(Value::as_f64),
            Some(0.97)
        );
    }

    fn write_synthetic_maa_convert_fixture() -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let task_dir = root.path().join("operations/synthetic-maa");
        fs::create_dir_all(task_dir.join("assets")).unwrap();
        fs::write(task_dir.join("assets/HOME.png"), b"synthetic").unwrap();
        fs::write(task_dir.join("assets/TERMINAL.png"), b"synthetic").unwrap();
        fs::write(
            root.path().join("operations/resources.json"),
            serde_json::to_vec_pretty(&json!({"resources":[]})).unwrap(),
        )
        .unwrap();
        fs::write(
            task_dir.join("task.json"),
            serde_json::to_vec_pretty(&json!({
                "schema_version": "0.5",
                "task_id": "synthetic-maa",
                "game": "arknights",
                "server_scope": ["cn"],
                "coordinate_space": {"width":1280,"height":720},
                "defaults": {"template_threshold":0.5},
                "anchors": [{
                    "id": "home",
                    "maa_task": "Check@Base",
                    "template": "assets/HOME.png",
                    "region": {"mode":"rect","rect":{"x":10,"y":20,"width":30,"height":40}}
                }, {
                    "id": "terminal",
                    "template": "assets/TERMINAL.png",
                    "region": {"mode":"rect","rect":{"x":50,"y":60,"width":30,"height":40}}
                }],
                "operations": [{
                    "id": "tap_home",
                    "purpose": "synthetic rectMove",
                    "from": "home",
                    "to": "terminal",
                    "click": {"kind":"point","x":100,"y":100},
                    "expect_after": {"page_id":"terminal","timeout_ms":500},
                    "consumes": [],
                    "produces": []
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        let maa_dir = root.path().join("maa-tasks");
        fs::create_dir_all(&maa_dir).unwrap();
        fs::write(
            maa_dir.join("tasks.json"),
            serde_json::to_vec_pretty(&json!({
                "Base": {
                    "template": "BASE.png",
                    "templThreshold": 0.67,
                    "method": "RGBCount",
                    "maskRange": [7, 199],
                    "rectMove": [1, 2, 3, 4],
                    "next": ["Helper"]
                },
                "Helper": {
                    "template": "HELPER.png",
                    "next": ["Stop"]
                },
                "Check@Base": {
                    "templThreshold": 0.91,
                    "rectMove": [11, 22, 33, 44],
                    "next": ["Base#next"]
                }
            }))
            .unwrap(),
        )
        .unwrap();
        (root, maa_dir)
    }

    #[test]
    fn maa_tasks_mode_feeds_expanded_template_fields_into_pack_targets() {
        let (root, maa_dir) = write_synthetic_maa_convert_fixture();

        let mut converter = OperationConverter::load(root.path(), None, None, None).unwrap();
        converter.load_maa_task_overlays(&maa_dir).unwrap();
        let outputs = converter.build_all().unwrap();
        let target = outputs.pack.pointer("/targets/0").unwrap();

        assert_eq!(
            target.pointer("/id").and_then(Value::as_str),
            Some("page/home")
        );
        assert_eq!(
            target.pointer("/threshold").and_then(Value::as_f64),
            Some(0.91)
        );
        assert_eq!(
            target.pointer("/method").and_then(Value::as_str),
            Some("rgb_count")
        );
        assert_eq!(
            target.pointer("/mask/type").and_then(Value::as_str),
            Some("range")
        );
        assert_eq!(
            target.pointer("/mask/lower").and_then(Value::as_u64),
            Some(7)
        );
        assert_eq!(
            target.pointer("/mask/upper").and_then(Value::as_u64),
            Some(199)
        );
        assert_eq!(
            target.pointer("/rect_move"),
            Some(&json!({"x":11,"y":22,"width":33,"height":44}))
        );
        let primitive = outputs.primitives.pointer("/primitives/0").unwrap();
        assert_eq!(
            primitive.pointer("/click/kind").and_then(Value::as_str),
            Some("offset")
        );
        assert_eq!(
            primitive
                .pointer("/click/target_id")
                .and_then(Value::as_str),
            Some("page/home")
        );
        assert_eq!(
            primitive.pointer("/click/offset"),
            Some(&json!({"x":11,"y":22,"width":33,"height":44}))
        );
        assert_eq!(
            primitive
                .pointer("/expect_after/page_id")
                .and_then(Value::as_str),
            Some("terminal")
        );
    }

    #[test]
    fn resource_convert_accepts_explicit_maa_tasks_mode() {
        let (root, maa_dir) = write_synthetic_maa_convert_fixture();
        let flags = FlagArgs::parse(&[
            "--maa-tasks".to_string(),
            maa_dir.display().to_string(),
            "--dry-run".to_string(),
        ])
        .unwrap();
        let summary = run_resource_convert(
            &GlobalOptions::default(),
            &flags,
            &ResolvedResourceRoot {
                input: root.path().to_path_buf(),
                root: root.path().to_path_buf(),
                layout: "direct",
            },
        )
        .unwrap();

        assert_eq!(
            summary.get("source_mode").and_then(Value::as_str),
            Some("maa_tasks")
        );
        assert_eq!(
            summary.get("maa_compiled_tasks").and_then(Value::as_u64),
            Some(3)
        );
        assert_eq!(summary.get("targets").and_then(Value::as_u64), Some(2));
    }

    #[test]
    fn default_operation_bundle_mode_does_not_apply_maa_overlay_fields() {
        let root = std::env::current_dir().unwrap();
        let converter = OperationConverter {
            root: root.clone(),
            game: "arknights".to_string(),
            server: "cn".to_string(),
            locale: "zh-CN".to_string(),
            coordinate_space: json!({"width":1280,"height":720}),
            defaults: json!({"template_threshold":0.5}),
            resource_ids: HashSet::new(),
            bundles: vec![Bundle {
                task_id: "synthetic-maa".to_string(),
                dir: root.join("operations/synthetic-maa"),
                data: json!({
                    "schema_version": "0.5",
                    "task_id": "synthetic-maa",
                    "anchors": [{
                        "id": "home",
                        "maa_task": "Check@Base",
                        "template": "assets/HOME.png",
                        "region": {"mode":"rect","rect":{"x":10,"y":20,"width":30,"height":40}}
                    }],
                    "operations": []
                }),
            }],
            existing_navigation: None,
            maa_task_overlays: HashMap::new(),
        };

        let pack = converter.build_pack().unwrap();
        assert_eq!(
            pack.pointer("/targets/0"),
            Some(&json!({
                "type": "template",
                "id": "page/home",
                "template_path": "operations/synthetic-maa/assets/HOME.png",
                "region": {"x":10,"y":20,"width":30,"height":40},
                "threshold": 0.5
            }))
        );
    }

    #[test]
    fn build_primitives_synthesizes_guard_from_operation_verify_template() {
        let root = tempfile::tempdir().unwrap();
        let task_dir = root.path().join("operations/daily-check");
        fs::create_dir_all(task_dir.join("assets")).unwrap();
        fs::write(task_dir.join("assets/VERIFY_READY.png"), b"png").unwrap();
        let converter = OperationConverter {
            root: root.path().to_path_buf(),
            game: "arknights".to_string(),
            server: "cn".to_string(),
            locale: "zh-CN".to_string(),
            coordinate_space: json!({"width":1280,"height":720}),
            defaults: json!({"template_threshold":0.95}),
            resource_ids: HashSet::new(),
            bundles: vec![Bundle {
                task_id: "daily-check".to_string(),
                dir: task_dir,
                data: json!({
                    "schema_version": "0.3",
                    "task_id": "daily-check",
                    "anchors": [],
                    "verify_templates": [{
                        "id": "template/verify_ready",
                        "template": "assets/VERIFY_READY.png",
                        "region": {"mode":"rect","rect":{"x":10,"y":20,"width":30,"height":40}},
                        "threshold": 0.97
                    }],
                    "operations": [{
                        "id": "home_to_target",
                        "purpose": "go target",
                        "from": "home",
                        "to": "target",
                        "click": {"kind":"rect","x":100,"y":110,"width":20,"height":25},
                        "verify_template": "assets/VERIFY_READY.png"
                    }]
                }),
            }],
            existing_navigation: None,
            maa_task_overlays: HashMap::new(),
        };

        let outputs = converter.build_all().unwrap();
        let primitive = outputs
            .primitives
            .pointer("/primitives/0")
            .expect("primitive");

        assert_eq!(
            primitive.pointer("/guard/page_id").and_then(Value::as_str),
            Some("arknights/home")
        );
        assert_eq!(
            primitive
                .pointer("/guard/target_id")
                .and_then(Value::as_str),
            Some("template/verify_ready")
        );
        assert_eq!(
            primitive.pointer("/guard/expected_rect"),
            Some(&json!({"x":10,"y":20,"width":30,"height":40}))
        );
        assert_eq!(
            outputs
                .primitives
                .get("converter_schema_version")
                .and_then(Value::as_str),
            Some(CONVERTER_SCHEMA_VERSION)
        );
    }

    #[test]
    fn build_primitives_synthesizes_guard_from_source_anchor_without_operation_verify_template() {
        let root = tempfile::tempdir().unwrap();
        let task_dir = root.path().join("operations/open-terminal");
        fs::create_dir_all(task_dir.join("assets")).unwrap();
        fs::write(task_dir.join("assets/HOME.png"), b"png").unwrap();
        let converter = OperationConverter {
            root: root.path().to_path_buf(),
            game: "arknights".to_string(),
            server: "cn".to_string(),
            locale: "zh-CN".to_string(),
            coordinate_space: json!({"width":1280,"height":720}),
            defaults: json!({"template_threshold":0.95}),
            resource_ids: HashSet::new(),
            bundles: vec![Bundle {
                task_id: "open-terminal".to_string(),
                dir: task_dir,
                data: json!({
                    "schema_version": "0.3",
                    "task_id": "open-terminal",
                    "anchors": [{
                        "id": "home",
                        "template": "assets/HOME.png",
                        "region": {"mode":"rect","rect":{"x":200,"y":300,"width":40,"height":50}},
                        "threshold": 0.8
                    }],
                    "operations": [{
                        "id": "home_to_terminal",
                        "purpose": "go terminal",
                        "from": "home",
                        "to": "terminal",
                        "click": {"kind":"rect","x":100,"y":110,"width":20,"height":25},
                        "verify_template": null
                    }]
                }),
            }],
            existing_navigation: None,
            maa_task_overlays: HashMap::new(),
        };

        let outputs = converter.build_all().unwrap();
        let primitive = outputs.primitives.pointer("/primitives/0").unwrap();

        assert_eq!(
            primitive.pointer("/guard/page_id").and_then(Value::as_str),
            Some("arknights/home")
        );
        assert_eq!(
            primitive
                .pointer("/guard/target_id")
                .and_then(Value::as_str),
            Some("page/home")
        );
        assert_eq!(
            primitive.pointer("/guard/expected_rect"),
            Some(&json!({"x":200,"y":300,"width":40,"height":50}))
        );
        assert_eq!(
            primitive
                .pointer("/guard/verify_template")
                .and_then(Value::as_str),
            Some("assets/HOME.png")
        );
    }

    #[test]
    fn build_primitives_synthesizes_any_page_guard_from_matching_anchor_template() {
        let root = tempfile::tempdir().unwrap();
        let task_dir = root.path().join("operations/return-home");
        fs::create_dir_all(task_dir.join("assets")).unwrap();
        fs::write(task_dir.join("assets/HOME_BUTTON.png"), b"png").unwrap();
        let converter = OperationConverter {
            root: root.path().to_path_buf(),
            game: "azurlane".to_string(),
            server: "jp".to_string(),
            locale: "ja-JP".to_string(),
            coordinate_space: json!({"width":1280,"height":720}),
            defaults: json!({"template_threshold":0.9}),
            resource_ids: HashSet::new(),
            bundles: vec![Bundle {
                task_id: "return-home".to_string(),
                dir: task_dir,
                data: json!({
                    "schema_version": "0.3",
                    "task_id": "return-home",
                    "anchors": [{
                        "id": "home",
                        "template": "assets/HOME_BUTTON.png",
                        "region": {"mode":"rect","rect":{"x":1100,"y":20,"width":60,"height":40}},
                        "threshold": 0.9
                    }],
                    "operations": [{
                        "id": "goto_home",
                        "purpose": "return home",
                        "from": "any",
                        "to": "home",
                        "click": {"kind":"rect","x":1100,"y":20,"width":60,"height":40},
                        "verify_template": "assets/HOME_BUTTON.png"
                    }]
                }),
            }],
            existing_navigation: None,
            maa_task_overlays: HashMap::new(),
        };

        let outputs = converter.build_all().unwrap();
        let primitive = outputs.primitives.pointer("/primitives/0").unwrap();

        assert_eq!(
            primitive.pointer("/guard/page_id").and_then(Value::as_str),
            Some("any")
        );
        assert_eq!(
            primitive
                .pointer("/guard/target_id")
                .and_then(Value::as_str),
            Some("page/home")
        );
    }

    #[test]
    fn build_primitives_synthesizes_guard_from_operation_verify_template_click_rect() {
        let root = tempfile::tempdir().unwrap();
        let task_dir = root.path().join("operations/return-home");
        fs::create_dir_all(task_dir.join("assets")).unwrap();
        fs::write(task_dir.join("assets/HOME_ICON.png"), b"png").unwrap();
        let converter = OperationConverter {
            root: root.path().to_path_buf(),
            game: "bluearchive".to_string(),
            server: "jp".to_string(),
            locale: "ja-JP".to_string(),
            coordinate_space: json!({"width":1280,"height":720}),
            defaults: json!({"template_threshold":0.9}),
            resource_ids: HashSet::new(),
            bundles: vec![Bundle {
                task_id: "return-home".to_string(),
                dir: task_dir,
                data: json!({
                    "schema_version": "0.3",
                    "task_id": "return-home",
                    "anchors": [],
                    "operations": [{
                        "id": "tap_home",
                        "purpose": "tap home",
                        "from": "any",
                        "to": "home",
                        "click": {"kind":"point","x":1236,"y":25},
                        "verify_template": "assets/HOME_ICON.png"
                    }]
                }),
            }],
            existing_navigation: None,
            maa_task_overlays: HashMap::new(),
        };

        let outputs = converter.build_all().unwrap();
        let primitive = outputs.primitives.pointer("/primitives/0").unwrap();

        assert_eq!(
            primitive.pointer("/guard/page_id").and_then(Value::as_str),
            Some("any")
        );
        assert_eq!(
            primitive
                .pointer("/guard/target_id")
                .and_then(Value::as_str),
            Some("template/home_icon")
        );
        assert_eq!(
            primitive.pointer("/guard/expected_rect"),
            Some(&json!({"x":1236,"y":25,"width":1,"height":1}))
        );
    }

    #[test]
    fn build_primitives_rejects_unmatched_verify_template_without_rect_guard_source() {
        let root = tempfile::tempdir().unwrap();
        let task_dir = root.path().join("operations/daily-check");
        fs::create_dir_all(task_dir.join("assets")).unwrap();
        fs::write(task_dir.join("assets/VERIFY_READY.png"), b"png").unwrap();
        let converter = OperationConverter {
            root: root.path().to_path_buf(),
            game: "arknights".to_string(),
            server: "cn".to_string(),
            locale: "zh-CN".to_string(),
            coordinate_space: json!({"width":1280,"height":720}),
            defaults: json!({"template_threshold":0.95}),
            resource_ids: HashSet::new(),
            bundles: vec![Bundle {
                task_id: "daily-check".to_string(),
                dir: task_dir,
                data: json!({
                    "schema_version": "0.3",
                    "task_id": "daily-check",
                    "anchors": [],
                    "operations": [{
                        "id": "home_to_target",
                        "purpose": "go target",
                        "from": "home",
                        "to": "target",
                        "click": {"kind":"offset","target_id":"target/button","offset":{"x":1,"y":2,"width":3,"height":4}},
                        "verify_template": "assets/VERIFY_READY.png"
                    }]
                }),
            }],
            existing_navigation: None,
            maa_task_overlays: HashMap::new(),
        };

        let err = converter
            .build_all()
            .expect_err("guard synthesis should fail");

        assert!(
            err.message
                .contains("cannot synthesize guard expected_rect from click kind")
        );
    }

    #[test]
    fn converted_offset_click_rejects_color_probe_guard() {
        let pack = json!({
            "game": "arknights",
            "targets": [{
                "type": "color",
                "id": "target/button"
            }]
        });
        let pages = json!({
            "pages": [{
                "id": "arknights/home"
            }]
        });
        let primitives = json!({
            "primitives": [{
                "id": "tap_offset",
                "from": "home",
                "click": {
                    "kind": "offset",
                    "target_id": "target/button",
                    "offset": {"x": 1, "y": 2, "width": 3, "height": 4}
                },
                "guard": {
                    "page_id": "arknights/home",
                    "target_id": "target/button",
                    "expected_rect": {"x": 10, "y": 20, "width": 30, "height": 40},
                    "color_probe": "target/button"
                }
            }]
        });

        let err = validate_converted_guard_references(&pack, &pages, &primitives)
            .expect_err("offset click must require template matched_rect source");

        assert!(err.message.contains("requires a template guard"));
        assert!(err.message.contains("must be a template target"));
    }

    #[test]
    fn build_primitives_allows_explicit_trusted_unguarded_coordinate() {
        let root = tempfile::tempdir().unwrap();
        let task_dir = root.path().join("operations/daily-check");
        fs::create_dir_all(&task_dir).unwrap();
        let converter = OperationConverter {
            root: root.path().to_path_buf(),
            game: "arknights".to_string(),
            server: "cn".to_string(),
            locale: "zh-CN".to_string(),
            coordinate_space: json!({"width":1280,"height":720}),
            defaults: json!({"template_threshold":0.95}),
            resource_ids: HashSet::new(),
            bundles: vec![Bundle {
                task_id: "daily-check".to_string(),
                dir: task_dir,
                data: json!({
                    "schema_version": "0.3",
                    "task_id": "daily-check",
                    "anchors": [],
                    "operations": [{
                        "id": "home_to_target",
                        "purpose": "go target",
                        "from": "home",
                        "to": "target",
                        "click": {"kind":"rect","x":100,"y":110,"width":20,"height":25},
                        "verify_template": null,
                        "unguarded_trusted_coordinate": true
                    }]
                }),
            }],
            existing_navigation: None,
            maa_task_overlays: HashMap::new(),
        };

        let outputs = converter.build_all().unwrap();
        let primitive = outputs.primitives.pointer("/primitives/0").unwrap();

        assert!(primitive.get("guard").is_some_and(Value::is_null));
        assert_eq!(
            primitive
                .get("unguarded_trusted_coordinate")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn arknights_default_locale_matches_current_resource_pack() {
        assert_eq!(default_locale("arknights"), "zh-CN");
        assert_eq!(default_locale("bluearchive"), "ja-JP");
        assert_eq!(default_locale("azurlane"), "ja-JP");
    }
}
