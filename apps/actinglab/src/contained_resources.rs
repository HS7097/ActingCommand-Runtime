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
