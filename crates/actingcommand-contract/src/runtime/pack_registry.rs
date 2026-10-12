// SPDX-License-Identifier: AGPL-3.0-only

//! The shapes of the package load and unload requests (Workflow #389 section 6, model v2.2.3 with
//! M-PKG section 5.1: no package digest anywhere). The Runtime's loaded-package table is its start
//! table, built from the configuration generation, with changes made while it runs laid over
//! it; the changes live in memory only. A Lab debugging operation (option two) loads or unloads
//! one entry. U-1 fixes the shapes; U-3 adds the behaviour and the shape checks with its
//! `pack_registry_` codes, and until then the host refuses both requests.

use crate::RequestId;
use serde::{Deserialize, Serialize};

/// The longest an `UnloadPack` waits for the runs it stops to end and reset before it answers
/// (section 6.3, the wait of `run stop`). U-3 refuses a larger `wait_ms`.
pub const MAX_PACK_UNLOAD_WAIT_MS: u64 = 25_000;

/// One entry of the loaded-package table, by the role the package plays for the Runtime's own
/// requesters (section 6.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case", deny_unknown_fields)]
pub enum PackRegistryKey {
    /// A procedure binding of `policy.procedure_manifest`, by its `procedure_ref`: the
    /// package's identity, `package_id` in the table and in the result codes.
    Procedure { package_id: String },
    /// The startup package of one instance (`instances[].startup_package`), by the instance.
    StartupPackage { instance_alias: String },
    /// One `prerequisite_packages` entry, by its package id.
    PrerequisitePackage { package_id: String },
}

/// What a `LoadPack` puts into the table (section 6.3). The package must already be on disk; the
/// load admits its structure and never computes or compares a content digest (M-PKG section 3.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "entry", rename_all = "snake_case", deny_unknown_fields)]
pub enum PackLoadEntry {
    /// Puts back the start table's entry under `key`.
    Restore { key: PackRegistryKey },
    /// A procedure binding: a package no binding names yet, or another binding for a configured
    /// one. The fields of one `policy.procedure_manifest` entry (M-PKG section 4.5), with
    /// `package_id` the binding's `procedure_ref`. Without `scheduled_execution` the procedure is a
    /// client run, as in the configuration.
    Procedure {
        package_id: String,
        operation_id: String,
        yield_points: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scheduled_execution: Option<PackScheduledExecution>,
    },
    /// The startup package of one instance, from `package_path`.
    StartupPackage {
        instance_alias: String,
        package_path: String,
    },
    /// A prerequisite package, from `package_path`; the package's own id must equal
    /// `package_id`.
    PrerequisitePackage {
        package_id: String,
        package_path: String,
    },
}

/// How the Runtime runs a procedure it schedules itself (`scheduled_execution` of a procedure
/// binding, M-PKG section 4.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum PackScheduledExecution {
    FixtureSimulation { package_path: String },
    DeviceRegistry { package_path: String },
}

/// One run an `UnloadPack` stopped (section 6.4 step 2): runs of the unloaded entry that the
/// Runtime started, and manual runs of a package with the same `package_id` (M-PKG D10, decided
/// 2026-10-12). `settled` is whether the run had ended and its reset finished when the request
/// answered; a run still stopping after `wait_ms` is reported unsettled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackUnloadStoppedRun {
    pub request_id: RequestId,
    pub settled: bool,
}
