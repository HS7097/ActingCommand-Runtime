// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome, FlagArgs, GlobalOptions, ResolvedResourceRoot, canonical_game};
use serde_json::{Map, Value, json};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

const GENERATED_BY: &str = "actinglab resource convert";
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
    let converter = OperationConverter::load(
        repo,
        game_override.as_deref(),
        server_override.as_deref(),
        locale_override.as_deref(),
    )?;
    let outputs = converter.build_all()?;
    let dry_run = global.dry_run || flags.bool("--dry-run");
    if !dry_run {
        outputs.write(repo)?;
    }
    Ok(json!({
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
    }))
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
        };
        converter.validate_bundles()?;
        Ok(converter)
    }

    pub(super) fn build_all(&self) -> CliOutcome<ConvertOutputs> {
        let pack = self.build_pack()?;
        validate_pack_targets_exist(&self.root, &pack)?;
        Ok(ConvertOutputs {
            pages: self.build_pages()?,
            navigation: self.build_navigation()?,
            index: self.build_index()?,
            primitives: self.build_primitives()?,
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
        };
        subset.validate_bundles()?;
        subset.build_all()
    }

    fn validate_bundles(&self) -> CliOutcome<()> {
        let mut errors = Vec::new();
        for bundle in &self.bundles {
            if bundle.data.get("schema_version").and_then(Value::as_str) != Some("0.3") {
                errors.push(format!(
                    "{}: schema_version != 0.3",
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
                let template = required_string(anchor, "template")?;
                let target = pack_target(
                    &target_id,
                    &repo_rel(&self.root, &bundle.dir.join(&template))?,
                    region_to_pack(required_field(anchor, "region")?)?,
                    anchor.get("threshold").cloned().unwrap_or_else(|| {
                        required_field(&self.defaults, "template_threshold")
                            .cloned()
                            .unwrap_or(Value::Null)
                    }),
                    color_check_to_pack(anchor.get("color_check"))?,
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
            for operation in array_field(&bundle.data, "operations") {
                let Some(template) = operation.get("verify_template").and_then(Value::as_str)
                else {
                    continue;
                };
                let target_id = template_target_id(template);
                let target = pack_target(
                    &target_id,
                    &repo_rel(&self.root, &bundle.dir.join(template))?,
                    Value::String(FULL_FRAME_SENTINEL.to_string()),
                    required_field(&self.defaults, "template_threshold")?.clone(),
                    None,
                    None,
                );
                add_first_target(&mut targets, &mut order, target_id, target);
            }
        }
        propagate_color_checks(&mut targets, &order);
        Ok(ordered_object([
            ("schema_version", Value::String("0.3".to_string())),
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
        Ok(ordered_object([
            ("schema_version", Value::String("0.3".to_string())),
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
            ("schema_version", Value::String("0.3".to_string())),
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
            ("schema_version", Value::String("0.3".to_string())),
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
                    ("click", required_field(operation, "click")?.clone()),
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
        Ok(ordered_object([
            ("schema_version", Value::String("0.3".to_string())),
            ("game", Value::String(self.game.clone())),
            ("server", Value::String(self.server.clone())),
            ("generated", Value::Bool(true)),
            ("generated_by", Value::String(GENERATED_BY.to_string())),
            ("primitives", Value::Array(primitives)),
        ]))
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
        Some("rect") => require_click_keys(
            path,
            operation,
            click,
            &["x", "y", "width", "height"],
            errors,
            "click",
        ),
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

fn pack_target(
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
    let required = resolve_required(anchor_id, declared_anchor_ids)
        .into_iter()
        .map(Value::String)
        .collect();
    pages.insert(
        page_id.clone(),
        ordered_object([
            ("id", Value::String(page_id.clone())),
            ("required", Value::Array(required)),
            ("optional", Value::Array(Vec::new())),
            ("forbidden", Value::Array(Vec::new())),
        ]),
    );
    order.push(page_id);
}

fn resolve_required(anchor_id: &str, declared_anchor_ids: &BTreeSet<String>) -> Vec<String> {
    if declared_anchor_ids.contains(anchor_id) {
        return vec![anchor_target_id(anchor_id)];
    }
    let prefix = format!("{anchor_id}_");
    let variants = declared_anchor_ids
        .iter()
        .filter(|id| id.starts_with(&prefix))
        .map(|id| anchor_target_id(id))
        .collect::<Vec<_>>();
    if variants.is_empty() {
        vec![anchor_target_id(anchor_id)]
    } else {
        variants
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
        Some("rect") => Ok(ordered_object([
            ("kind", Value::String("rect".to_string())),
            ("x", required_field(click, "x")?.clone()),
            ("y", required_field(click, "y")?.clone()),
            ("width", required_field(click, "width")?.clone()),
            ("height", required_field(click, "height")?.clone()),
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
    }

    #[test]
    fn resolves_page_required_variants() {
        let ids = BTreeSet::from([
            "home".to_string(),
            "operator_0".to_string(),
            "operator_1".to_string(),
        ]);
        assert_eq!(resolve_required("home", &ids), vec!["page/home"]);
        assert_eq!(
            resolve_required("operator", &ids),
            vec!["page/operator_0", "page/operator_1"]
        );
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
    fn arknights_default_locale_matches_current_resource_pack() {
        assert_eq!(default_locale("arknights"), "zh-CN");
        assert_eq!(default_locale("bluearchive"), "ja-JP");
        assert_eq!(default_locale("azurlane"), "ja-JP");
    }
}
