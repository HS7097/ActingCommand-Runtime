// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{BundleIndex, BundleMaintenance, BundleSource};
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;

/// The 32 MiB bound for package maintenance input and resource-restoration draft payloads.
pub const DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;

/// Workflow #288 A2b `package bundle`: the applications table, the directory holding one
/// source directory per task pack, and the new output directory.
#[derive(Debug, Clone)]
pub struct PackageBundleRequest {
    pub applications: PathBuf,
    pub packs_root: PathBuf,
    pub out: PathBuf,
    pub source: Option<BundleSource>,
    /// Present (including an empty list) writes the bundle's `maintenance` declarations and
    /// requests actual maintenance qualification; absent writes none (bundle v4 either way,
    /// Workflow #336 PKG-S5).
    pub maintenance: Option<Vec<BundleMaintenance>>,
    /// Workflow #373 NAV-T: the standard-pack root files to lay beside `bundle.json`.
    pub root_files: StandardPackRootFiles,
}

/// Workflow #373 NAV-T (M-373 v1.1 §3.1, §3.9): the root files `package bundle` checks and lays
/// beside `bundle.json` byte for byte, each by its source path (`--pages`, `--interrupts`,
/// `--navigation`); none given lays none.
#[derive(Debug, Clone, Default)]
pub struct StandardPackRootFiles {
    pub pages: Option<PathBuf>,
    pub interrupts: Option<PathBuf>,
    pub navigation: Option<PathBuf>,
    /// ROUT-L RL-1a (`--schedule`): the game's schedule rules, `schedule.json`.
    pub schedule: Option<PathBuf>,
    /// Workflow #392 DA-4 (`--display-layout`): the game's display declaration,
    /// `display-layout.json` (model 392 v1.2 §4.1).
    pub display_layout: Option<PathBuf>,
    /// Workflow #407 LOAD-1 (`--loading`): the game's loading pages, `loading.json`, with the
    /// templates of its `loading/` directory beside it. Every pack's loading copy is checked as
    /// `package loading --check` checks it, then the file and its templates are laid beside
    /// `bundle.json`. Without it, a pack that carries a loading copy is refused.
    pub loading: Option<PathBuf>,
}

/// Workflow #407 LOAD-1: `actinglab package loading --root-file <loading.json> --packs-root
/// <directory> (--write | --check)`.
#[derive(Debug, Clone)]
pub struct PackageLoadingRequest {
    /// The game's `loading.json`; its templates lie in the `loading/` directory beside it.
    pub root_file: PathBuf,
    /// The directory of pack directories, as `package bundle --packs-root`.
    pub packs_root: PathBuf,
    pub mode: PackageLoadingMode,
}

/// What `package loading` does once every check passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageLoadingMode {
    /// `--write`: writes every pack's copy that is not the one the root file makes.
    Write,
    /// `--write` under the global `--dry-run`: names the packs `--write` would write, and writes
    /// nothing.
    Preview,
    /// `--check`: writes nothing, and refuses naming every pack whose copy is stale.
    Check,
}

/// Workflow #407 LOAD-1: the answer of `package loading`.
#[derive(Debug, Clone, Serialize)]
pub struct PackageLoadingResponse {
    /// `written`, `preview` or `checked`.
    pub status: String,
    pub game: String,
    pub root_file: String,
    /// The game's allowance for one confirmation, as the copies carry it.
    pub max_ms: u64,
    /// One entry per pack directory of `--packs-root`, in name order.
    pub packs: Vec<PackLoadingReport>,
}

/// One pack of a `package loading` answer.
#[derive(Debug, Clone, Serialize)]
pub struct PackLoadingReport {
    pub package_id: String,
    pub server: String,
    /// The loading pages its copy holds, in the root file's order; empty when it takes none.
    pub pages: Vec<String>,
    /// Whether the root file's `exclude_packages` names it.
    pub excluded: bool,
    pub state: PackLoadingState,
}

/// Where a pack's loading copy stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackLoadingState {
    /// Its copy is the one the root file makes (none, when it takes no page).
    Current,
    /// `--write` wrote it.
    Written,
    /// Under `--dry-run`: `--write` would write it.
    Stale,
}

/// Workflow #373 NAV-T: `actinglab standard-pack check --root <game root>`, an installed game
/// root `<standard_packs.root>/<game>/`.
#[derive(Debug, Clone)]
pub struct StandardPackCheckRequest {
    pub root: PathBuf,
}

/// Workflow #373 NAV-T: a game root that passed the checks `package bundle` runs.
#[derive(Debug, Clone, Serialize)]
pub struct StandardPackCheckResponse {
    pub status: String,
    pub root: String,
    pub game: String,
    /// The root files the game root holds; (Workflow #407 LOAD-1) `loading.json` among them, last,
    /// when it holds one.
    pub root_files: Vec<&'static str>,
    pub pack_count: usize,
    /// The report "entry page not in the list": one `package_bundle_entry_page_unlisted` per
    /// package whose entry pages `pages.json` does not all list (none without `pages.json`);
    /// then (Workflow #411 B35) one `package_bundle_interrupt_page_shadowed` per package and
    /// shared interrupt page it declares itself (none without `interrupts.json`); then
    /// (STORE-FG-1) one `package_bundle_store_tap_unlocated` per package and store step that taps
    /// a fixed place; then (Workflow #428 CFL-2) one `package_bundle_preprocess_divergent` per
    /// package, OCR target and shared read whose chains differ; then (Workflow #411 B15) one
    /// `package_bundle_remedy_invalid` per remedy finding. With `schedule.json`, the
    /// composition's warnings follow, then (SS-2b) one `standard_pack_benefit_undeclared` per
    /// judged rule that declares no value, then (SS-2c) one `standard_pack_priority_tier_mismatch`
    /// per rule whose maintenance declaration disagrees with its tier.
    pub warnings: Vec<actingcommand_contract::outcome::Outcome>,
    /// SS-2b (register §1.225): with `schedule.json`, how many of the game's judged rules (their
    /// package is not maintenance-role and, SS-2c, they do not declare themselves maintenance)
    /// have complete values, how many are pending, and how many declare none; SS-2c: and how many
    /// rules declare themselves maintenance; SCHED-C1a: and how many write a fixed value. Absent
    /// without `schedule.json`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub benefit_coverage: Option<actingcommand_policy::BenefitCoverage>,
    /// SCHED-C1a (register §1.299): with `schedule.json`, which judged rules run on a fixed value
    /// and which pools on a declared valuation block, each by who wrote it (`pack` or `model`), in
    /// file order. Absent without `schedule.json`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub benefit_provenance: Option<actingcommand_policy::BenefitProvenance>,
    /// #428 COV-1 (register §1.299): with `schedule.json`, each chain follower
    /// (`benefit_coverage.follower`) with the complete rule it rides with, `{task_id,
    /// rides_with}`, in file order. Absent when there is none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub benefit_followers: Vec<actingcommand_policy::BenefitFollower>,
    /// #428 SCHED-C1f1 (register §1.303 SC1F-7): with `schedule.json`, each `consumes` entry
    /// whose amount is read from a fact, `{rule_id, pool_id, fact_key, fallback}`, in file order,
    /// empty when none. Absent without `schedule.json`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumption_facts: Option<Vec<actingcommand_policy::ConsumptionFactRule>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackageBundleResponse {
    pub status: String,
    pub out: String,
    pub index: BundleIndex,
    /// Workflow #384 D2-S2: one `recognition_pack_undeclared` per pack that reads like a
    /// recognition maintenance package of a server that declares none (Workflow #411 A32: only
    /// the one a server declares counts); Workflow #373 NAV-T: with
    /// `--pages`, one `package_bundle_entry_page_unlisted` per pack whose entry pages the page
    /// list does not all list; Workflow #411 B35: with `--interrupts`, one
    /// `package_bundle_interrupt_page_shadowed` per pack and shared interrupt page it declares
    /// itself; STORE-FG-1: with any root file, one `package_bundle_store_tap_unlocated` per pack
    /// and store step that taps a fixed place; Workflow #411 B15: with any root file, one
    /// `package_bundle_remedy_invalid` per remedy finding; with `--schedule`, the composition's
    /// warnings, then (SS-2b) one `standard_pack_benefit_undeclared` per judged rule that
    /// declares no value, then (SS-2c) one `standard_pack_priority_tier_mismatch` per rule whose
    /// maintenance declaration disagrees with its tier. Absent when there is none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<actingcommand_contract::outcome::Outcome>,
    /// SS-2b (register §1.225): with `--schedule`, how many of the game's judged rules have
    /// complete values, how many are pending, and how many declare none; SS-2c: and how many
    /// rules declare themselves maintenance; SCHED-C1a: and how many write a fixed value. Absent
    /// without it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub benefit_coverage: Option<actingcommand_policy::BenefitCoverage>,
    /// SCHED-C1a (register §1.299): with `--schedule`, which judged rules run on a fixed value and
    /// which pools on a declared valuation block, each by who wrote it (`pack` or `model`), in
    /// file order. Absent without it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub benefit_provenance: Option<actingcommand_policy::BenefitProvenance>,
    /// #428 COV-1 (register §1.299): with `--schedule`, each chain follower
    /// (`benefit_coverage.follower`) with the complete rule it rides with, `{task_id,
    /// rides_with}`, in file order. Absent when there is none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub benefit_followers: Vec<actingcommand_policy::BenefitFollower>,
    /// #428 SCHED-C1f1 (register §1.303 SC1F-7): with `--schedule`, each `consumes` entry whose
    /// amount is read from a fact, `{rule_id, pool_id, fact_key, fallback}`, in file order, empty
    /// when none. Absent without it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumption_facts: Option<Vec<actingcommand_policy::ConsumptionFactRule>>,
}

#[derive(Debug, Clone)]
/// Workflow #336 PKG-S4 (option 丙): the package by its path; no hash.
pub struct PackageValidateRequest {
    pub zip_path: PathBuf,
    pub include_entries: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackageValidationResponse {
    pub status: String,
    pub module: String,
    pub manifest_path: String,
    pub task_count: usize,
    pub entry_count: usize,
    pub dangerous_entries: Vec<String>,
    pub recognition_pack_diagnostics: Vec<RecognitionPackDiagnosticsResponse>,
    pub manifest: JsonDocument,
    pub entries: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecognitionPackDiagnosticsResponse {
    pub path: String,
    pub unsupported_target_count: usize,
    pub unsupported_targets: Vec<UnsupportedRecognitionTargetResponse>,
}

/// A template target with recognition semantics this build cannot evaluate (Workflow #378
/// A2-P: typed members replace the English `reason`).
#[derive(Debug, Clone, Serialize)]
pub struct UnsupportedRecognitionTargetResponse {
    pub id: String,
    /// The declared matching method in the pack spelling (`rgb_count`, `hsv_count`), when it
    /// is not `ncc`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<actingcommand_recognition_pack::RecognitionMethod>,
    /// Whether the target declares a mask.
    pub mask: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct JsonDocument(Value);

impl JsonDocument {
    pub(crate) fn new(value: Value) -> Self {
        Self(value)
    }
}
