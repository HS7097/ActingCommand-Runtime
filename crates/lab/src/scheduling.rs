// SPDX-License-Identifier: AGPL-3.0-only

//! Pure scheduling inspection with bounded local-file adaptation; no Lab ports.

use actingcommand_contract::{LabError, LabErrorClass, LabResult};
use actingcommand_policy::{
    CatalogDocumentSource, CatalogSources, CompiledCatalog, MAX_CATALOG_BYTES, MAX_DOCUMENT_BYTES,
    MAX_TEXT_BYTES, compile_catalog, inspect_timeline,
};
pub use actingcommand_policy::{EvaluationTime, TimelineQueryContext};
use serde_json::Value;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

// Four input budgets allow source locations and diagnostics in the JSON response.
pub const MAX_SCHEDULING_INSPECTION_OUTPUT_BYTES: usize = 4 * MAX_CATALOG_BYTES;

#[derive(Debug, Clone)]
pub struct SchedulingCatalogPaths {
    pub tasks: PathBuf,
    pub pools: PathBuf,
    pub activity: PathBuf,
    pub timeline: PathBuf,
}

pub fn compile_scheduling_files(paths: &SchedulingCatalogPaths) -> LabResult<Value> {
    let catalog = load_catalog(paths)?;
    compile_report(&catalog)
}

pub fn inspect_scheduling_timeline_files(
    paths: &SchedulingCatalogPaths,
    time: EvaluationTime,
    context: &TimelineQueryContext,
    event_ids: &[String],
) -> LabResult<Value> {
    let catalog = load_catalog(paths)?;
    let timeline = inspect_timeline(&catalog, time, context, event_ids)
        .map_err(|error| scheduling_error(error.code(), error.to_string()))?;
    let bytes = serde_json::to_vec(&serde_json::json!({
        "mode": "offline_inspection",
        "compilation": compile_report(&catalog)?,
        "timeline": timeline,
    }))
    .map_err(|error| scheduling_error("scheduling_output_invalid", error.to_string()))?;
    bounded_output(&bytes)
}

fn load_catalog(paths: &SchedulingCatalogPaths) -> LabResult<CompiledCatalog> {
    let sources = CatalogSources {
        tasks: read_source(&paths.tasks)?,
        pools: read_source(&paths.pools)?,
        activity: read_source(&paths.activity)?,
        timeline: read_source(&paths.timeline)?,
    };
    match compile_catalog(&sources) {
        Ok(catalog) => Ok(catalog),
        Err(failure) => {
            let bytes = failure.dry_run_json().map_err(|error| {
                scheduling_error("scheduling_output_invalid", error.to_string())
            })?;
            Err(
                scheduling_error("scheduling_catalog_rejected", failure.to_string())
                    .with_details(bounded_output(&bytes)?),
            )
        }
    }
}

fn read_source(path: &Path) -> LabResult<CatalogDocumentSource> {
    let source_uri = path
        .to_str()
        .filter(|uri| !uri.is_empty() && uri.len() <= MAX_TEXT_BYTES)
        .ok_or_else(|| {
            scheduling_error(
                "scheduling_source_invalid",
                "source path must be nonempty UTF-8 within the catalog text limit",
            )
        })?;
    let file = File::open(path).map_err(|error| {
        scheduling_error(
            "scheduling_source_read_failed",
            format!("{source_uri}: {error}"),
        )
    })?;
    if !file
        .metadata()
        .map_err(|error| {
            scheduling_error(
                "scheduling_source_read_failed",
                format!("{source_uri}: {error}"),
            )
        })?
        .is_file()
    {
        return Err(scheduling_error(
            "scheduling_source_invalid",
            format!("{source_uri}: expected a regular file"),
        ));
    }
    let mut bytes = Vec::new();
    // One sentinel byte lets the compiler retain its canonical size diagnostic.
    file.take(MAX_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            scheduling_error(
                "scheduling_source_read_failed",
                format!("{source_uri}: {error}"),
            )
        })?;
    Ok(CatalogDocumentSource::new(source_uri, bytes))
}

fn compile_report(catalog: &CompiledCatalog) -> LabResult<Value> {
    let bytes = catalog
        .dry_run_json()
        .map_err(|error| scheduling_error("scheduling_output_invalid", error.to_string()))?;
    bounded_output(&bytes)
}

fn bounded_output(bytes: &[u8]) -> LabResult<Value> {
    if bytes.len() > MAX_SCHEDULING_INSPECTION_OUTPUT_BYTES {
        return Err(scheduling_error(
            "scheduling_output_limit_exceeded",
            format!("scheduling response exceeds {MAX_SCHEDULING_INSPECTION_OUTPUT_BYTES} bytes",),
        ));
    }
    serde_json::from_slice(bytes)
        .map_err(|error| scheduling_error("scheduling_output_invalid", error.to_string()))
}

fn scheduling_error(code: &str, message: impl Into<String>) -> LabError {
    LabError::new(LabErrorClass::UsageValidation, code, message, &[])
}
