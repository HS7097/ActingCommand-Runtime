// SPDX-License-Identifier: AGPL-3.0-only

//! Production semantic commands admit resources only through a contained in-memory bundle.
//! Workflow #336 PKG-S4 (option 丙): a package is named by its path only; no reference or
//! digest is taken, and admission is structural.

use super::{CliError, CliOutcome, FlagArgs, NavigationGraph, parse_navigation_graph_value};
use actingcommand_contract::ContainedTaskRequest;
use actingcommand_lab::ExternallyVerifiedBundle;
use actingcommand_pack_containment::ContainmentLimits;
use actingcommand_page_detector::PageDetector;
use actingcommand_recognition_pack::RecognitionEvaluator;
use actingcommand_resource_tooling::{PublishedPackageReader, open_published_package};
use std::path::PathBuf;
use std::sync::Arc;

pub(super) struct ObservationResources {
    pub metadata: actingcommand_contract::page_projection::VerifiedProjectionMetadata,
    /// #411 Lab L2: every row of the generated navigation, in its order: an edge with one page and
    /// (#411 A13) an operation whose `to` is a page set, each an element of the page projection
    /// like any edge (resource-declarations.md), with its click as the reader takes it.
    pub navigation: Vec<super::NavigationRow>,
    pub operations: Vec<PageOperation>,
    pub multi_destination: Vec<MultiDestinationOperation>,
    pub controls: Vec<super::ControlPoint>,
    pub pages: actingcommand_page_detector::PageSet,
}

pub(super) struct PageOperation {
    pub id: String,
    pub task_id: String,
    pub page: String,
    pub purpose: String,
    /// #411 Lab L2: its click may be a segmented drag.
    pub click: super::NavigationClick,
}

/// Workflow #308 SEL-L6a (FCM T-03): a click operation whose `to` is a page set. A route edge has
/// one destination; the generated navigation keeps such an operation with all its pages
/// (`to_pages`, Workflow #390 LAB-2) but no route passes it, and observe lists it from the task
/// file with its pages as the page set names them and its declared click. #411 A13: its element
/// of the page projection comes from the generated navigation row
/// ([`ObservationResources::navigation`]).
pub(super) struct MultiDestinationOperation {
    pub id: String,
    pub task_id: String,
    pub from_page: String,
    pub to_pages: Vec<String>,
    pub click: serde_json::Value,
}

pub(super) fn observation_resources(
    resources: &ExternallyVerifiedBundle,
) -> CliOutcome<ObservationResources> {
    let bundle = resources.loaded_bundle();
    let navigation = bundle.navigation().ok_or_else(|| {
        CliError::package_invalid("externally verified resource bundle has no navigation graph")
    })?;
    let array = |name: &str| -> CliOutcome<&[serde_json::Value]> {
        match navigation.get(name) {
            None if name != "navigation" => Ok(&[]),
            Some(serde_json::Value::Array(values)) => Ok(values),
            _ => Err(CliError::package_invalid(format!(
                "navigation resource requires {name}[]"
            ))),
        }
    };
    // Workflow #390 LAB-2: the navigation collects an operation whose `to` is a page set with
    // every page it may reach (`to_pages`); `actions` lists it below from its task file, and it is
    // no route edge. #411 A13: the projection takes it from this row, as it takes an edge.
    let navigation_rows = array("navigation")?
        .iter()
        .map(super::parse_navigation_row)
        .collect::<CliOutcome<Vec<_>>>()?;
    let operations = array("page_operations")?
        .iter()
        .map(|value| {
            Ok(PageOperation {
                id: super::required_string_field(value, "id")?.to_string(),
                task_id: super::required_string_field(value, "task_id")?.to_string(),
                page: super::required_string_field(value, "page")?.to_string(),
                purpose: match value.get("purpose") {
                    None | Some(serde_json::Value::Null) => String::new(),
                    Some(serde_json::Value::String(value)) => value.clone(),
                    _ => {
                        return Err(CliError::package_invalid(
                            "page operation purpose must be text",
                        ));
                    }
                },
                click: super::parse_navigation_click(super::required_value_field(value, "click")?)?,
            })
        })
        .collect::<CliOutcome<Vec<_>>>()?;
    let controls = array("control_points")?
        .iter()
        .map(super::parse_control_point)
        .collect::<CliOutcome<Vec<_>>>()?;
    // Read-only mapping validates declared forms without constructing execution exclusion rectangles.
    for value in array("destructive_actions")? {
        let click = super::required_value_field(value, "click")?;
        if click.get("kind").is_none() {
            super::parse_navigation_tap_rect(click)?;
        } else {
            super::parse_navigation_click(click)?;
        }
    }
    let pages = bundle
        .pages_path()
        .and_then(|path| bundle.entry(path))
        .ok_or_else(|| CliError::package_invalid("contained page definitions are missing"))?;
    let pages =
        std::str::from_utf8(pages).map_err(|error| CliError::package_invalid(error.to_string()))?;
    let pages_value: serde_json::Value = serde_json::from_str(pages.trim_start_matches('\u{feff}'))
        .map_err(|error| CliError::package_invalid(error.to_string()))?;
    let pages = actingcommand_page_detector::load_page_set_from_json_str(
        pages.trim_start_matches('\u{feff}'),
    )
    .map_err(|error| CliError::package_invalid(error.to_string()))?;
    let metadata = match bundle.projection_metadata() {
        Some(metadata) => metadata.clone(),
        None => {
            let pack = bundle
                .recognition_pack_path()
                .and_then(|path| bundle.entry(path))
                .ok_or_else(|| {
                    CliError::package_invalid("contained recognition pack is missing")
                })?;
            let pack: serde_json::Value = serde_json::from_slice(pack)
                .map_err(|error| CliError::package_invalid(error.to_string()))?;
            actingcommand_contract::page_projection::VerifiedProjectionMetadata::unannotated(
                actingcommand_contract::page_projection::ProjectionCatalog::from_resources(
                    &pack,
                    &pages_value,
                    navigation,
                )?,
            )
        }
    };
    Ok(ObservationResources {
        metadata,
        navigation: navigation_rows,
        operations,
        multi_destination: multi_destination_operations(bundle)?,
        controls,
        pages,
    })
}

/// The click operations of every task file of `bundle` whose `to` is a page set; an application
/// effect and a select step are not clicks, as the generated navigation leaves them out too.
fn multi_destination_operations(
    bundle: &actingcommand_pack_containment::LoadedBundle,
) -> CliOutcome<Vec<MultiDestinationOperation>> {
    let prefix = format!(
        "{}/operations/",
        bundle.resource_root().trim_end_matches('/')
    );
    let mut listed = Vec::new();
    for path in bundle
        .entry_paths()
        .filter(|path| path.starts_with(&prefix) && path.ends_with("/task.json"))
    {
        let task = bundle
            .entry(path)
            .map(serde_json::from_slice::<serde_json::Value>)
            .transpose()
            .map_err(|error| CliError::package_invalid(format!("{path}: {error}")))?
            .ok_or_else(|| CliError::package_invalid(format!("{path} is not readable")))?;
        let Some(operations) = task.get("operations").and_then(serde_json::Value::as_array) else {
            continue;
        };
        for operation in operations {
            let Some(serde_json::Value::Array(to)) = operation.get("to") else {
                continue;
            };
            let effect = |name: &str| operation.get(name).is_some_and(|value| !value.is_null());
            if effect("application") || effect("select") {
                continue;
            }
            let game = super::required_string_field(&task, "game")?;
            // A page names its game unless it already does, as the packaged page set does.
            let page = |anchor: &str| {
                if anchor == "any" || anchor.contains('/') {
                    anchor.to_string()
                } else {
                    format!("{game}/{anchor}")
                }
            };
            listed.push(MultiDestinationOperation {
                id: super::required_string_field(operation, "id")?.to_string(),
                task_id: super::required_string_field(&task, "task_id")?.to_string(),
                from_page: page(super::required_string_field(operation, "from")?),
                to_pages: to
                    .iter()
                    .map(|anchor| {
                        anchor.as_str().map(page).ok_or_else(|| {
                            CliError::package_invalid(format!(
                                "{path}: a page of operation to must be text"
                            ))
                        })
                    })
                    .collect::<CliOutcome<Vec<_>>>()?,
                click: super::required_value_field(operation, "click")?.clone(),
            });
        }
    }
    Ok(listed)
}

/// #411 Lab L7: the guard of a click operation of a package, where the kernel checks it before the
/// operation's input (`guard_outcome`): the frame's page must be the guard's page (`page_id`,
/// read in its task's `game`; `any` is every page), then its target must pass.
pub(super) struct OperationGuardPage {
    pub task_id: String,
    pub operation_id: String,
    pub game: String,
    pub page_id: String,
}

/// #411 Lab L7: the guards whose target is `target` of the click operations of every task file of
/// `bundle`, in task-file and operation order: the operations `target` belongs to. An application
/// effect and a select step are not clicks, and an operation without a guard (an unguarded
/// trusted coordinate) has no target.
pub(super) fn operation_guards(
    bundle: &actingcommand_pack_containment::LoadedBundle,
    target: &str,
) -> CliOutcome<Vec<OperationGuardPage>> {
    let prefix = format!(
        "{}/operations/",
        bundle.resource_root().trim_end_matches('/')
    );
    let mut guards = Vec::new();
    for path in bundle
        .entry_paths()
        .filter(|path| path.starts_with(&prefix) && path.ends_with("/task.json"))
    {
        let task = bundle
            .entry(path)
            .map(serde_json::from_slice::<serde_json::Value>)
            .transpose()
            .map_err(|error| CliError::package_invalid(format!("{path}: {error}")))?
            .ok_or_else(|| CliError::package_invalid(format!("{path} is not readable")))?;
        let Some(operations) = task.get("operations").and_then(serde_json::Value::as_array) else {
            continue;
        };
        for operation in operations {
            let effect = |name: &str| operation.get(name).is_some_and(|value| !value.is_null());
            if effect("application") || effect("select") {
                continue;
            }
            let Some(guard) = operation.get("guard").filter(|guard| !guard.is_null()) else {
                continue;
            };
            if guard.get("target_id").and_then(serde_json::Value::as_str) != Some(target) {
                continue;
            }
            guards.push(OperationGuardPage {
                task_id: super::required_string_field(&task, "task_id")?.to_string(),
                operation_id: super::required_string_field(operation, "id")?.to_string(),
                game: super::required_string_field(&task, "game")?.to_string(),
                page_id: super::required_string_field(guard, "page_id")?.to_string(),
            });
        }
    }
    Ok(guards)
}

/// Workflow #396 LAB-3: every bundle evaluates OCR with actinglab's OCR provider, the
/// Runtime's, when it has one.
pub(super) fn load(flags: &FlagArgs, command: &str) -> CliOutcome<Arc<ExternallyVerifiedBundle>> {
    let vision_provider = super::offline_ocr::lab_ocr().provider();
    // `--package <path>`: the path chooses the reading (a directory, a `.json` or a `.zip`
    // container, an assembled ZIP); `--zip <path>`: a published assembled `.zip`.
    if flags.optional("--package").is_some() {
        let input = PackageInput::open(flags)?;
        let deadline = std::time::Instant::now()
            .checked_add(std::time::Duration::from_millis(
                ContainedTaskRequest::DEFAULT_RESPONSE_DEADLINE_MS,
            ))
            .ok_or_else(|| CliError::package_invalid("package admission deadline overflow"))?;
        let instance = format!("semantic_{}", command.replace('-', "_"));
        let admitted = ExternallyVerifiedBundle::load_path(
            &instance,
            input.path(),
            false,
            vision_provider,
            deadline,
        )
        .map(Arc::new)
        .map_err(|error| CliError::package_invalid(error.to_string()));
        return finish_package_use(admitted, input.close());
    }
    let logical_zip = explicit_path(flags, "--zip")?;
    let zip = open_published_package(&logical_zip)?;
    let metadata = zip.metadata()?;
    let limit = ContainmentLimits::default().max_compressed_bytes;
    if metadata.len() > limit {
        return Err(CliError::package_invalid(format!(
            "semantic resource package {} is {} bytes, above the {limit}-byte containment limit",
            zip.path().display(),
            metadata.len()
        )));
    }
    let bytes = zip.read_all()?;
    let instance = format!("semantic_{}", command.replace('-', "_"));
    match vision_provider {
        Some(provider) => {
            ExternallyVerifiedBundle::load_with_vision_provider(&instance, &bytes, provider)
        }
        None => ExternallyVerifiedBundle::load(&instance, &bytes),
    }
    .map(Arc::new)
    .map_err(|error| CliError::package_invalid(error.to_string()))
}

/// Retains the existing publication pin for ZIP material throughout an RPC.
/// Source directories are located here and verified exclusively by containment.
/// Workflow #336 PKG-S4 (option 丙): the package by its path only. `--package <path>` is read as
/// its path chooses; `--zip <path>` is a published `.zip`, whose publication generation stays
/// referenced until `close`. No reference or digest is taken.
pub(super) struct PackageInput {
    path: PathBuf,
    reader: Option<PublishedPackageReader>,
}

impl PackageInput {
    pub fn open(flags: &FlagArgs) -> CliOutcome<Self> {
        if flags.optional("--package").is_some() && flags.optional("--zip").is_some() {
            return Err(CliError::usage("provide one package locator"));
        }
        let published = flags.optional("--package").is_none();
        // Locator validation owns the pre-capture loose-resource rejection.
        let logical = explicit_path(flags, if published { "--zip" } else { "--package" })?;
        let reader = if published {
            Some(open_published_package(&logical)?)
        } else {
            None
        };
        let path = match &reader {
            Some(reader) => reader
                .path()
                .canonicalize()
                .map_err(|error| CliError::package_invalid(error.to_string()))?,
            None if logical.is_absolute() => logical,
            None => std::env::current_dir()
                .map_err(|error| CliError::package_invalid(error.to_string()))?
                .join(logical),
        };
        Ok(Self { path, reader })
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub fn close(self) -> CliOutcome<()> {
        match self.reader {
            Some(reader) => reader.close(),
            None => Ok(()),
        }
    }
}

/// #411 Lab L5: the frames each online observation of the package at `path` takes for its
/// target consensus, as the Runtime's kernel computes them from the same admission
/// (`PreparedPageObservation::frame_plan`); the client's receipt wait allows for them. A package
/// the kernel does not admit for observation is refused here, before any Runtime request.
pub(super) fn online_frame_plan(
    instance: &str,
    path: &std::path::Path,
) -> CliOutcome<actingcommand_contract::ObservationFramePlan> {
    let deadline = std::time::Instant::now()
        .checked_add(std::time::Duration::from_millis(
            ContainedTaskRequest::DEFAULT_RESPONSE_DEADLINE_MS,
        ))
        .ok_or_else(|| CliError::package_invalid("package admission deadline overflow"))?;
    actingcommand_lab::PreparedPageObservation::load_path(instance, path, &[], None, deadline)
        .map(|observation| observation.frame_plan())
        .map_err(|error| CliError::package_invalid(format!("{error}: {}", error.cause())))
}

pub(super) fn recognition_pipeline(
    resources: &ExternallyVerifiedBundle,
) -> CliOutcome<(RecognitionEvaluator, PageDetector)> {
    let bundle = resources.loaded_bundle();
    let evaluator = bundle.evaluator().cloned().ok_or_else(|| {
        CliError::package_invalid("externally verified resource bundle has no recognition pack")
    })?;
    let detector = bundle.detector().cloned().ok_or_else(|| {
        CliError::package_invalid("externally verified resource bundle has no page definitions")
    })?;
    detector
        .validate(&evaluator)
        .map_err(|error| CliError::package_invalid(error.to_string()))?;
    Ok((evaluator, detector))
}

pub(super) fn navigation_graph(
    resources: &ExternallyVerifiedBundle,
) -> CliOutcome<NavigationGraph> {
    let navigation = resources.loaded_bundle().navigation().ok_or_else(|| {
        CliError::package_invalid("externally verified resource bundle has no navigation graph")
    })?;
    parse_navigation_graph_value(navigation)
}

pub(super) fn explicit_path(flags: &FlagArgs, name: &str) -> CliOutcome<PathBuf> {
    match flags.optional(name) {
        None => Err(CliError::package_invalid(format!(
            "semantic commands require {name} <package>; loose resource roots are not executable"
        ))),
        Some(value) if value == "true" => Err(CliError::usage(format!(
            "{name} requires an explicit package path"
        ))),
        Some(value) => Ok(PathBuf::from(value)),
    }
}

pub(super) fn finish_package_use<T>(
    operation: CliOutcome<T>,
    close: CliOutcome<()>,
) -> CliOutcome<T> {
    match (operation, close) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(mut primary), Err(secondary)) => {
            primary.message = format!(
                "{}; package_reader_release_failed={}",
                primary.message, secondary.message
            );
            Err(primary)
        }
    }
}
