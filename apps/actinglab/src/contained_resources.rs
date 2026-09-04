// SPDX-License-Identifier: AGPL-3.0-only

//! Production semantic commands admit resources only through an externally hashed in-memory bundle.

use super::{CliError, CliOutcome, FlagArgs, NavigationGraph, parse_navigation_graph_value};
use actingcommand_lab::{ExternalExpectedSha256, ExternallyVerifiedBundle};
use actingcommand_pack_containment::ContainmentLimits;
use actingcommand_page_detector::PageDetector;
use actingcommand_recognition_pack::RecognitionEvaluator;
use actingcommand_resource_tooling::open_published_package;
use std::path::PathBuf;
use std::sync::Arc;

pub(super) struct ObservationResources {
    pub edges: Vec<super::NavigationEdge>,
    pub operations: Vec<PageOperation>,
    pub controls: Vec<super::ControlPoint>,
    pub pages: actingcommand_page_detector::PageSet,
}

pub(super) struct PageOperation {
    pub id: String,
    pub task_id: String,
    pub page: String,
    pub purpose: String,
    pub input: super::SemanticInput,
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
    let edges = array("navigation")?
        .iter()
        .map(super::parse_navigation_edge)
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
                input: super::parse_navigation_input(super::required_value_field(value, "click")?)?,
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
            super::parse_navigation_input(click)?;
        }
    }
    let pages = bundle
        .pages_path()
        .and_then(|path| bundle.entry(path))
        .ok_or_else(|| CliError::package_invalid("contained page definitions are missing"))?;
    let pages =
        std::str::from_utf8(pages).map_err(|error| CliError::package_invalid(error.to_string()))?;
    let pages = actingcommand_page_detector::load_page_set_from_json_str(
        pages.trim_start_matches('\u{feff}'),
    )
    .map_err(|error| CliError::package_invalid(error.to_string()))?;
    Ok(ObservationResources {
        edges,
        operations,
        controls,
        pages,
    })
}

pub(super) fn load(flags: &FlagArgs, command: &str) -> CliOutcome<Arc<ExternallyVerifiedBundle>> {
    let logical_zip = explicit_path(flags, "--zip")?;
    let zip = open_published_package(&logical_zip)?;
    let expected = explicit_hash(flags)?;
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
    ExternallyVerifiedBundle::load(&instance, &bytes, expected)
        .map(Arc::new)
        .map_err(|error| CliError::package_invalid(error.to_string()))
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

fn explicit_path(flags: &FlagArgs, name: &str) -> CliOutcome<PathBuf> {
    match flags.optional(name) {
        None => Err(CliError::package_invalid(format!(
            "semantic commands require {name} <package> and --expected-sha256 <hash>; loose resource roots are not executable"
        ))),
        Some(value) if value == "true" => Err(CliError::usage(format!(
            "{name} requires an explicit package path"
        ))),
        Some(value) => Ok(PathBuf::from(value)),
    }
}

fn explicit_hash(flags: &FlagArgs) -> CliOutcome<ExternalExpectedSha256> {
    match flags.optional("--expected-sha256") {
        None => Err(CliError::package_invalid(
            "semantic commands require externally supplied --expected-sha256 <hash>",
        )),
        Some(value) if value == "true" => Err(CliError::usage(
            "--expected-sha256 requires an explicit SHA-256 value",
        )),
        Some(value) => ExternalExpectedSha256::parse_hex(&value)
            .map_err(|error| CliError::package_invalid(error.to_string())),
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
