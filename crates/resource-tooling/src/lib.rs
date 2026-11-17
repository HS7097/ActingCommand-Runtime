// SPDX-License-Identifier: AGPL-3.0-only

//! Deterministic resource compiler and package validation for the optional Lab toolchain.
//!
//! This crate has no live device, scheduler, Runtime, or Lab state authority.

#![forbid(unsafe_code)]
#![allow(clippy::result_large_err)]

// Workflow #464 P1 (register §1.377): `actinglab standard-pack adopt`.
mod adopt;
mod api;
mod authoring;
mod business_catalog;
mod table_bindings;
pub use table_bindings::{
    PolicyTablesRefusal, TableCoverageGap, check_policy_tables, declared_policy_tables,
};
mod codes;
mod declaration_exclusions;
// Workflow #336 PKG-S5 (exec key 66): package bundle writes bundle v4 by `package_id`.
#[cfg(test)]
mod gate_336_pkgs5;
// Workflow #373 NAV-T (exec key 80): the standard-pack cross-check of package bundle and
// standard-pack check, the root files laid beside bundle.json and the reserved package ids;
// Workflow #411 B35: the warning of a pack that declares a shared interrupt page itself.
#[cfg(test)]
mod gate_373_navt;
// Workflow #411 A25 (exec key 113): standard-pack check admits every registry package, with or
// without the navigation root files.
#[cfg(test)]
mod gate_411_a25;
// Workflow #411 B15 R1 (exec key 102): the remedy report of package bundle and standard-pack
// check.
#[cfg(test)]
mod gate_411_b15b;
// Workflow #411 slice E1 (train 0.12.9; A40, A41): `package bundle` and `standard-pack check` say
// why they refuse: a pack table that holds no pack, and the file of a package that does not parse.
#[cfg(test)]
mod gate_411_e1;
// Workflow #411 A32 (slice E3, T3): only the recognition pack a server declares counts.
#[cfg(test)]
mod gate_411_e3;
// Test plan v2.2 E-1 G10 (exec key 33): package bundle and the business catalog.
#[cfg(test)]
mod gate_e1;
// Test plan v2.2 E-4 G4b (exec key 12): the bundle's offline maintenance verdict and the
// Runtime's admission.
#[cfg(test)]
mod gate_e4;

// Workflow #412 Lab B-3 S0, T0-4 (contract key K42): resource restore and a declared operation.
#[cfg(test)]
mod gate_412_b3_s0_restore;

// #386 CF-4: standard-pack check reads a fact reference only under its `requires` entry.
#[cfg(test)]
mod gate_386_cf4;

// #386 CF-5: standard-pack check reads a pool's fact capacity and a projection's fact comparand
// only under their `requires` entry.
#[cfg(test)]
mod gate_386_cf5;

// SS-2b (register §1.225): standard-pack check counts the judged rules' values and warns once per
// rule that declares none.
#[cfg(test)]
mod gate_ss2b;

// SS-2c (register §1.227): standard-pack check counts the rules declared maintenance apart, warns
// of a rule in the other tier than its declaration says, and refuses a declaration of another
// shape.
#[cfg(test)]
mod gate_ss2c;

// #428 SCHED-C1a (register §1.299): standard-pack check counts the rules that write a fixed value
// and says who wrote each fixed value and each valuation block.
#[cfg(test)]
mod gate_sched_c1a;

// #428 COV-1 (register §1.299): standard-pack check counts a rule that declares no value and only
// rides with its chain as a follower, not undeclared, and lists it with the rule it rides with.
#[cfg(test)]
mod gate_cov1;

// #428 SCHED-C1f1 (register §1.303 SC1F-7): standard-pack check lists the consumptions read from
// a fact, each with its key and fallback, and counts such a rule's entry as a declared quantity.
#[cfg(test)]
mod gate_sched_c1f1;

// Workflow #428 CFL-2: standard-pack check names a target whose chain differs from its shared
// read, with and without `pages.json`.
#[cfg(test)]
mod gate_428_cfl2;

// SS-5: standard-pack check refuses a probe on a key the rule's own package does not mark.
#[cfg(test)]
mod gate_ss5;

// Workflow #407 LOAD-1 (register §1.257): a game's loading.json, the packs' loading copies,
// package loading --write and --check, package bundle --loading and standard-pack check.
#[cfg(test)]
mod gate_load1;

// Workflow #444 NOTICES-3 (register §1.336): package bundle --notices and standard-pack check of a
// game root's notices directory.
#[cfg(test)]
mod gate_444_notices3;

// Workflow #453 S8 (register §1.355): standard-pack check answers the models a game root's packs
// need.
#[cfg(test)]
mod gate_453_s8;

// Workflow #455 G-R1 (register §1.356): standard-pack check and package bundle --goals of a game
// root's goal declaration `goals.json`.
#[cfg(test)]
mod gate_455_gr1;

// Workflow #455 G-R2s fix round 3 (register §1.369): the goal check and the composer's input the
// tools read, over real packages, follow every chain link and the remedies' capabilities.
#[cfg(test)]
mod gate_455_gr2s;

// Workflow #464 P3 (register §1.375): the readiness of every pack of a game root, item by item.
#[cfg(test)]
mod gate_464_p3;

// Workflow #464 P1 (register §1.377): standard-pack adopt on a made-up resource repository.
#[cfg(test)]
mod gate_464_p1;

// Workflow #463 V1 (register §1.378): standard-pack check's root check of the variables a game
// root's packages declare.
#[cfg(test)]
mod gate_463_v1;

// Workflow #463 S3-6 (register §1.413): the readiness names a hold's reason and lists each pack's
// own warnings.
#[cfg(test)]
mod gate_463_s36;

// Workflow #463 S3-10 (register §1.417): the plan of a trial, its refusals, and the trial record's
// writer and reader.
#[cfg(test)]
mod gate_463_s310;

// Workflow #463 S3-11 (register §1.418): the run's own refusals, the record of its end, and the
// writer's failure paths.
#[cfg(test)]
mod gate_463_s311;

// Workflow #463 S3-12 (register §1.445): adoption names its offline review, shows the trial of
// its content and warns before enabling.
#[cfg(test)]
mod gate_463_s312;

// Workflow #463 S3-14 (register §1.446): where each pack of a game root stands.
#[cfg(test)]
mod gate_463_s314;

// Workflow #477 PC-1 (register §1.438): standard-pack check of a game root's `pack-catalog.json`,
// the writer `standard-pack catalog`, and the composition that never reads the file.
#[cfg(test)]
mod gate_477_pc1;

mod maa_task_graph;
// Workflow #444 NOTICES-3: a notices directory on disk, held to the contract crate's rules.
mod notices_directory;
mod package_directory;
// Workflow #407 LOAD-1: `package loading`, and the check `package bundle` and `standard-pack
// check` run with it.
mod package_loading;
mod package_publish;
mod package_validate;
// Workflow #463 V3: `package variables`, what one package's control declaration declares.
mod package_variables;
mod resource_convert;
mod resource_restore;
// Workflow #477 PC-1: `standard-pack catalog`, the writer of a game root's `pack-catalog.json`.
mod standard_pack_catalog;
mod standard_pack_check;
// Workflow #455 G-R1: `goals.json` in `package bundle --goals` and `standard-pack check`.
mod standard_pack_goals;
// ROUT-L RL-1a: `schedule.json` in `package bundle`, `standard-pack check` and `compose`.
mod standard_pack_schedule;
// Workflow #464 P3: `standard-pack ready`, the one function that judges a pack's readiness.
mod standard_pack_ready;
// Workflow #463 S3-10: `standard-pack trial`, its plan, its refusals and its record.
mod standard_pack_trial;
// Workflow #463 S3-14: `standard-pack status`, where each pack of a game root stands.
mod standard_pack_status;

pub use adopt::{
    AdoptClass, AdoptCoverage, AdoptDecision, AdoptSource, AdoptWorktree, DerivedDrift,
    DerivedFrom, DerivedMember, GivenMember, NotJudgedBecause, PACK_DECISION_SCHEMA,
    RecordingReview, RootEntryChange, StandardPackAdoptRequest, StandardPackAdoptResponse,
    ValueNotJudged, ValueWait, standard_pack_adopt,
};
// Workflow #463 AC-1: what adoption asks of the Lab (the one judgement of a recording, #463 S3-17,
// and the one runner of a regression case), and the cases it answers.
pub use adopt::{
    AdoptCaseRun, AdoptCaseRunner, AdoptCaseVerdict, AdoptCases, AdoptLab, CaseNotProven,
    ReadyNotRead, RecordingReady,
};
pub use api::*;
pub use authoring::*;
pub use business_catalog::*;
// Workflow #463 AC-1: the word of the cases an adoption took.
pub use codes::AdoptCasesState;
pub use codes::ResourceToolingCode;
// Workflow #464 P3: the readiness items, problems and unjudged words; fix round 1: a pack's four
// answers and its supporting parts.
pub use codes::{
    ReadinessItem, ReadinessProblem, ReadinessRole, ReadinessState, ReadinessUnjudged,
};
pub use declaration_exclusions::{
    DECLARATION_EXCLUSIONS_FILE, DECLARATION_EXCLUSIONS_SCHEMA, DeclarationExclusions,
};
pub use maa_task_graph::{
    MaaFact, MaaFactOrigin, MaaFactSource, MaaFactValue, MaaTaskFacts, MaaTaskGraph,
    MaaTaskGraphStats, compile_maa_task_graph,
};
pub use package_directory::package_bundle;
pub use package_loading::package_loading;
// Workflow #407 LOAD-2: `actinglab resource validate` of `loading.json` and a pack's copy.
pub use package_loading::{check_loading_root_file, check_pack_loading_copy};
pub use package_publish::{
    PackagePublicationCommit, PackagePublicationTransaction, PublishedPackageReader,
    open_published_package,
};
pub use package_validate::validate_package;
pub use package_variables::package_variables;
pub use resource_convert::{
    Bundle, OperationParser, ParseOutputs, ResolvedResourceRoot, canonical_game, canonical_locale,
    canonical_server, resolve_resource_root, validate_maa_semantic_declarations,
};
pub use resource_restore::*;
pub use standard_pack_catalog::write_standard_pack_catalog;
pub use standard_pack_check::check_standard_pack_root;
pub use standard_pack_ready::{
    InstallationRead, ItemState, PackReadiness, ReadinessAnswer, ReadinessBasis, ReadinessBinding,
    ReadinessHold, ReadinessInstallation, ReadinessModel, ReadinessSuspension,
    StandardPackReadiness, StandardPackReadyRequest, judge_standard_pack_readiness,
};
pub use standard_pack_schedule::{
    StandardPackComposeRequest, StandardPackComposeResponse, compose_standard_packs,
};
// Workflow #463 S3-10: the trial's plan and refusals, and the record's writer and reader; fix
// round 2: the one test of an installed root along a path, `installed_root_containing`; fix round
// 3: the record's own plan type and the writer's answer.
pub use codes::{TrialRecordProblem, TrialStatePlace};
// Workflow #463 S3-11: the run's offline parts: its own refusals and the record of its end; the
// word of a collected run that is not the trial's; fix round 2: the record of a run, whatever its
// content.
pub use codes::TrialCollectProblem;
// Workflow #463 S3-8, S3-9 fix round 1: what the answer of a trial's run knows of a run the
// Runtime confirmed nothing of.
pub use codes::TrialRunUnconfirmed;
// Workflow #463 S3-12: the latest trial of an adopted content, as adopt's review answers it; the
// train of S3-12 and S3-14: the one summary of a content's trials, which adoption and the status
// row answer alike (`TrialSummary`, `TrialReference`, `TrialNotRead`, `trial_summary`).
pub use codes::TrialState;
pub use standard_pack_trial::{
    CONTENT_PREFIX_DIGITS, MAX_AUTHORIZATION_BYTES, PACK_TRIAL_SCHEMA, PackTrialRecord,
    StandardPackTrialAnswer, StandardPackTrialRequest, TRIALS_DIRECTORY, TrialDeclaredEnd,
    TrialEndKind, TrialIrreversible, TrialIrreversibleBasis, TrialNotRead, TrialPlan,
    TrialRecordPlan, TrialReference, TrialRunEnd, TrialRunRequirement, TrialSummary,
    TrialValueSource, TrialVariable, WrittenTrialRecord, check_trial_run, check_trial_state_dir,
    installed_root_containing, latest_trial_record, plan_standard_pack_trial, trial_record_of_run,
    trial_record_path, trial_records, trial_summary, write_trial_record,
};
// Workflow #463 S3-14: the one read of where each pack of a game root stands, and its words.
pub use codes::{AuthorizationUnrecorded, EnabledState, PackAuthorization, StageState};
pub use standard_pack_status::{
    CheckoutContent, EnabledStateCounts, MissingItem, PackEnabled, PackFormed, PackStatus,
    StageStateCounts, StandardPackStatus, StandardPackStatusRequest, standard_pack_status,
};
// Workflow #463 S3-15a: what `--installation` hands the status (the placement journals, the
// suspension report and the composed bindings), what it answers of them, its word for pairs not
// read and its two refusals.
pub use codes::PairsUnread;
pub use standard_pack_status::{
    EnabledAuthorization, InstallationJournals, InstallationPairs, InstallationReading, PackPairs,
    PairStatus, PairSuspended, PlacementAuthorization, StatusInstallation,
    standard_pack_status_with, status_installation_unbuilt, status_root_not_installed,
};
// Workflow #463 S3-15a fix round 1: the composed bindings with the warnings of their reading.
pub use standard_pack_status::ComposedBindings;
