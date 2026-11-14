// SPDX-License-Identifier: AGPL-3.0-only

//! ROUT-L RL-1a (frozen model v1.0 §3.2, D-2; `contracts/standard-pack-schedule.md`): the pure
//! composer of the standard packs' schedule rules. The Runtime's scheduler workbench,
//! check-config and `actinglab standard-pack check|compose` all call this one function.
//!
//! Its inputs are the bound instances (alias → game and server, from the configuration) and, for
//! each game root that holds `schedule.json`, that file's bytes, the root as
//! `read_standard_pack_root` read it (its registry: package → server and directory) and the
//! bundle's maintenance entries. Steps:
//!
//! 1. **Games in use**: the games of the bound instances that hold `schedule.json`, by name; the
//!    instances of each by alias. A game without a bound instance is left out.
//! 2. **Read**: each game's file is read by the contract crate's reader, its `requires` checked
//!    to name `fact_reference` when a rule writes a fact reference (#386 CF-4) and
//!    `pool_fact_reference` when a pool's `capacity` or a `resource_projection`'s `value` is one
//!    (#386 CF-5) and `reset_exclusions` when a timeline event writes `excludes_fact_prefixes`
//!    (Workflow #411 P19) and `fact_unknown_probe` when a rule's `fact` predicate writes
//!    `on_unknown` (SS-5) and `pool_lines` when a pool writes `stop_line` or `hard_line` (SS-2h),
//!    `pool_trigger_line` when a pool writes `trigger_line` (SCHED-C1c), `queue_wait` when a rule
//!    writes `max_queue_wait_ms` (Workflow #437 ORD-1a), no timeline event
//!    invalidating the Runtime's application update fact
//!    (UPD-1 fix round 5), and against the subsystems given (when given), its packages checked
//!    against the registry, and the registry's packages classified. A refused game is reported
//!    with its game id.
//! 3. **Instantiate**: per bound instance `i` of game `g` on server `s`, every rule whose package
//!    belongs to `s` and is not held, and every pool, profile and event for every server or for
//!    `s`, each scope filled with `{kind: instance, instance_id: i}` (the item's own and those of
//!    a rule's `fact`, `record_deadline` and `fact_time_reached` predicates), a rule's
//!    `entrypoint.operation_id` set to `operation.package.run` and its `procedure_ref` to
//!    `package.<package_id>`, and `instance_overrides` empty.
//! 4. **Ids**: an item copied to exactly one instance keeps the file's id, so one instance per
//!    (game, server) composes byte-identical ids. An item copied to two or more instances gets
//!    `<id>@<alias>` in each copy, every reference to it inside that copy is rewritten, and a
//!    rule's copy continues `<id>` (`continues_from`, Workflow #372 K5).
//! 5. **Join and compile**: one V2 catalog `{catalog_id: "standard-packs", catalog_version,
//!    approval_refs: []}` of the games in name order, compiled with the 4 MiB catalog bound
//!    ([`compile_composed_catalog`]). Ids are unique across the games in use, and two games'
//!    rules never name one package id (`rule_id_conflict`).
//!
//! The result carries the four documents, the compiled catalog, the manifest (`procedure_ref` →
//! game, package and directory), each game's classification and the rules digest. Every refusal
//! is a `standard_pack_root_invalid` problem of the game's `schedule.json` at its JSON pointer: a
//! compiler diagnostic is mapped back to the rule or item it came from. #428 CW-1: so is each
//! compile warning the compiled catalog carries ([`StandardPackComposition::compile_warnings`]).

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    APPLICATION_UPDATE_REQUIRED_FACT_KEY, BundleMaintenance, CatalogCompileWarning,
    IrreversiblePackages, SCHEDULE_POOL_VALUATION_ORIGIN_MEMBER, SCHEDULE_RULE_FIXED_VALUE_MEMBER,
    SCHEDULE_RULE_REALIZATION_MEMBER, STANDARD_PACK_SCHEDULE_FILE, ScheduleItem, ScheduleMember,
    ScheduleRule, ScheduleValueOrigin, StandardPackClassification, StandardPackProblem,
    StandardPackRoot, StandardPackRootIssue, StandardPackSchedule, classify_standard_pack_packages,
    parse_standard_pack_schedule, standard_pack_maintenance_packages,
    validate_standard_pack_schedule_references,
};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::canonical::canonical_serialized;
use crate::{
    ActivityProfile, BENEFIT_FIXED_VALUE_SUBSYSTEM, CONSUMPTION_FACT_SUBSYSTEM,
    CatalogDiagnosticCode, CatalogDocumentSource, CatalogSources, CompiledCatalog,
    FACT_REFERENCE_SUBSYSTEM, FACT_REFERENCE_TYPE, FACT_UNKNOWN_PROBE_SUBSYSTEM,
    POOL_FACT_REFERENCE_SUBSYSTEM, POOL_LINES_SUBSYSTEM, PoolSpec, PredicateSpec,
    RESET_EXCLUSIONS_SUBSYSTEM, SCHEDULING_SCHEMA_VERSION_V2, SchedulingDocumentKind, TaskSpec,
    TaskTerminalState, TimelineEvent, compile_composed_catalog,
};

/// The catalog id of every composition (model §3.2 step 4).
pub const STANDARD_PACKS_CATALOG_ID: &str = "standard-packs";
/// The one operation a composed task runs: the Runtime's own package run.
pub const STANDARD_PACK_OPERATION_ID: &str = "operation.package.run";
/// A composed task's procedure reference is this prefix and its rule's `package_id`.
pub const STANDARD_PACK_PROCEDURE_PREFIX: &str = "package.";

/// The game and server the configuration binds one instance alias to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardPackBinding {
    pub game_id: String,
    pub server_id: String,
}

/// One game root's files as the composer reads them.
#[derive(Debug, Clone, Copy)]
pub struct StandardPackGameFiles<'a> {
    /// `schedule.json`'s bytes.
    pub schedule: &'a [u8],
    /// The game root as `read_standard_pack_root` read it: its registry and root files.
    pub root: &'a StandardPackRoot,
    /// The bundle's `maintenance[]` entries.
    pub maintenance: &'a [BundleMaintenance],
    /// Workflow #455 G-R2s (register §1.363): which packages of the root can do something
    /// irreversible, by the whole root's declarations as the caller's reader read every package
    /// `bundle.json` registers, and which could not be read. Required: no caller can leave it out.
    pub irreversible: &'a IrreversiblePackages,
}

/// What [`compose_standard_pack_rules`] composes.
#[derive(Debug, Clone, Copy)]
pub struct StandardPackComposeInput<'a> {
    /// Every bound instance, by alias.
    pub instances: &'a BTreeMap<String, StandardPackBinding>,
    /// The game roots that hold `schedule.json`, by game id.
    pub games: &'a BTreeMap<String, StandardPackGameFiles<'a>>,
    /// The configuration manifest's subsystems each `requires` must name. `None` where no
    /// Runtime manifest is at hand (the resource tools): the requirements are then reported in
    /// [`ComposedGame::requires`] and checked by the Runtime that loads the rules.
    pub subsystems: Option<&'a BTreeSet<String>>,
    /// The composed catalog's version (1 or more): the Runtime gives its ledger head plus one.
    pub catalog_version: u64,
}

/// One manifest entry: the package a composed `procedure_ref` runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComposedProcedure {
    pub procedure_ref: String,
    pub game_id: String,
    pub package_id: String,
    /// The package directory below the game root (the registry's).
    pub directory: String,
}

/// One game with a bound instance, as the composition used it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComposedGame {
    pub game_id: String,
    /// The bound instances, by alias.
    pub instances: Vec<String>,
    /// `sha256:<hex>` of the `schedule.json` composed; `None` when the game root holds none, and
    /// the game has no rules.
    pub schedule_sha256: Option<String>,
    pub requires: Vec<String>,
    /// The tasks composed for the game's instances.
    pub tasks: usize,
    pub maintenance: Vec<String>,
    pub scheduled: Vec<String>,
    pub held: Vec<String>,
    pub unclassified: Vec<String>,
}

/// One refusal: the game whose `schedule.json` is refused (`None` for the composition as a
/// whole, for example its size), the problem at its JSON pointer, and developer text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardPackRuleIssue {
    pub game_id: Option<String>,
    pub issue: StandardPackRootIssue,
    /// What the check found, for the developer log only.
    pub detail: String,
}

/// Every refusal of one composition, in game order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardPackComposeFailure {
    issues: Vec<StandardPackRuleIssue>,
}

impl StandardPackComposeFailure {
    pub fn issues(&self) -> &[StandardPackRuleIssue] {
        &self.issues
    }
}

impl fmt::Display for StandardPackComposeFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "standard-pack schedule rules refused with {} issue(s)",
            self.issues.len()
        )?;
        for issue in &self.issues {
            write!(
                formatter,
                "; {}: {}: {}",
                issue.game_id.as_deref().unwrap_or("(composition)"),
                issue.issue,
                issue.detail
            )?;
        }
        Ok(())
    }
}

impl Error for StandardPackComposeFailure {}

/// SS-2 (register §1.222; Alice's ruling 104 point 1): the priority tiers a game's rules use, the
/// maintenance tier and the business tier. A game whose composed rules use more distinct
/// priority values is warned about, never refused.
pub const STANDARD_PACK_PRIORITY_TIERS: usize = 2;

/// SS-4 (register §1.231; plan `rd-1029/plan-state-driven.md`, the notes on removing the counts):
/// the failure escalation threshold above which a rule that writes no daily bound is warned
/// about, never refused. Without the daily limit a run that keeps failing is retried until this
/// many failures pause the pair, and the no-progress guard covers successful runs only. A hint's
/// bound, not a tunable: it decides nothing.
pub const STANDARD_PACK_ESCALATION_WARNING_THRESHOLD: u64 = 10;

/// SS-2b (register §1.225): the task member the composer writes, `true`, on every rule whose
/// package is not maintenance-role; SS-2c (register §1.227): and that does not declare itself
/// maintenance (`"maintenance": true`). A rule never writes it itself.
pub const BENEFIT_JUDGED_MEMBER: &str = "benefit_judged";

/// SS-2d (register §1.235): the task member the composer writes, `true`, on a judged rule that
/// carries the model's realization mark (the `schedule.json` rule member `"realization": true`,
/// [`SCHEDULE_RULE_REALIZATION_MEMBER`]). A rule never writes it itself.
pub const BENEFIT_REALIZATION_MEMBER: &str = "benefit_realization";

/// SS-2h (register §1.289): the `schedule.json` rule member `"limited": true`, which only `acmodel
/// bindings apply` writes from the model (the activity has its own limit, or an income cap counts
/// it). It is `schedule.json`'s, not a V2 task member: the composer writes the task member
/// [`BENEFIT_LIMITED_MEMBER`] on the judged rule instead.
pub const SCHEDULE_RULE_LIMITED_MEMBER: &str = "limited";

/// SS-2h (register §1.289): the task member the composer writes, `true`, on a judged rule that
/// carries the rule member [`SCHEDULE_RULE_LIMITED_MEMBER`]. A rule never writes it itself.
pub const BENEFIT_LIMITED_MEMBER: &str = "benefit_limited";

/// SS-2h (register §1.289): the pool members of the two lines, which the composer copies as
/// written and which require [`POOL_LINES_SUBSYSTEM`].
pub const POOL_LINE_MEMBERS: [&str; 2] = ["stop_line", "hard_line"];

/// SCHED-C1a (register §1.299): the task member the composer writes, with the rule's integer, on
/// a judged rule that carries the rule member `fixed_value_milli`
/// ([`SCHEDULE_RULE_FIXED_VALUE_MEMBER`]). A rule never writes it itself.
pub const BENEFIT_FIXED_VALUE_MEMBER: &str = "benefit_fixed_value_milli";

/// SCHED-C1c (register §1.299): the pool member of the trigger line, which the composer copies as
/// written, as the two lines, and which requires [`crate::POOL_TRIGGER_LINE_SUBSYSTEM`].
pub const POOL_TRIGGER_LINE_MEMBER: &str = "trigger_line";

/// Workflow #437 ORD-1a (register §1.352): the V2 task member of a task's longest queue wait,
/// which a rule writes as any V2 member and the composer passes through, and which requires
/// [`crate::QUEUE_WAIT_SUBSYSTEM`].
pub const MAX_QUEUE_WAIT_MEMBER: &str = "max_queue_wait_ms";

/// Workflow #455 G-R2s (register §1.363; `contracts/standard-pack-schedule.md`, "The irreversible
/// mark"): the task member the composer writes, `true`, on a rule whose package can do something
/// irreversible by its game root's declarations ([`IrreversiblePackages::counts_irreversible`],
/// every task of a game whose root has a package that could not be read included), and on a rule
/// a chain follower of which, riding with it directly or further down ([`BenefitFollower`]), runs
/// such a package. Absent otherwise, so a root without such a package composes the bytes of before.
/// A rule never writes it itself.
pub const IRREVERSIBLE_REACH_MEMBER: &str = "irreversible_reach";

/// SS-2b (register §1.225; Alice's ruling 116, 「业务覆盖完整」): how far one game's judged rules
/// (those whose package is not maintenance-role and that do not declare themselves maintenance,
/// not held) declare their values, as the resource tools report it. Each rule counts once,
/// whatever the instances it is composed for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct BenefitCoverage {
    /// Every entry has its quantity and every pool it touches its valuation.
    pub complete: usize,
    /// Not complete, and every value it lacks is the pending marker.
    pub pending: usize,
    /// No `produces` and no `consumes` entry, or a pool it touches with neither a valuation nor
    /// the pending marker: one `standard_pack_benefit_undeclared` warning each.
    pub undeclared: usize,
    /// SS-2c (register §1.227): the rules, not held, that declare themselves maintenance
    /// (`"maintenance": true`), whatever their package's role. They are not judged, so none of
    /// them counts in the three above.
    pub maintenance_declared: usize,
    /// SS-2d (register §1.235): the judged rules that carry the model's realization mark
    /// (`"realization": true`); each also counts in one of the first three.
    pub realization: usize,
    /// SCHED-C1a (register §1.299): the judged rules that write a fixed value
    /// (`fixed_value_milli`); each also counts in one of the first three, by the pools it
    /// consumes alone (its `produces` are not read): complete when each has a valuation, pending
    /// when every value it lacks is the marker, undeclared for a consumed pool with neither.
    pub fixed_value: usize,
    /// #428 COV-1 (register §1.299): the judged rules that declare no value (no `produces` and no
    /// `consumes` entry, no fixed value) and, by their trigger alone, only run in their chain
    /// head's run window, riding with a complete rule ([`BenefitFollower`]). None of them counts
    /// as undeclared or is warned about. Absent from the answer while 0, so an answer for a file
    /// without followers is the one of before.
    #[serde(skip_serializing_if = "is_zero")]
    pub follower: usize,
}

/// `serde`'s test for a count the answer leaves out while it is 0.
const fn is_zero(count: &usize) -> bool {
    *count == 0
}

/// #428 COV-1 (register §1.299; `contracts/standard-pack-schedule.md`, "Coverage"): one chain
/// follower of a game's `schedule.json` and the complete rule it rides with, by their file ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BenefitFollower {
    /// The follower.
    pub task_id: String,
    /// The complete judged rule its walk up its edges ends at, whose value the chain carries.
    pub rides_with: String,
}

/// SCHED-C1a (register §1.299; `contracts/standard-pack-schedule.md`, "Provenance"): who wrote
/// the values one game's `schedule.json` holds, so that a later model export can replace them one
/// by one. It decides nothing; the catalog carries no origin.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct BenefitProvenance {
    /// The judged rules (not held) that run on a fixed value, by their file ids.
    pub fixed_value: ValueOrigins,
    /// The pools whose `valuation` is a declared block (not the pending marker), by their file
    /// ids.
    pub valuation: ValueOrigins,
}

/// SCHED-C1f1 (register §1.303 SC1F-7; `contracts/standard-pack-schedule.md`, "A run's
/// consumption read from a fact"): one `consumes` entry of a game's `schedule.json` whose
/// `amount` is read from a fact, by the file's ids, with its key and fallback. It decides
/// nothing: nothing checks the key against the rule's package (ruling 4), and a key no run writes
/// leaves every decision on the fallback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConsumptionFactRule {
    pub rule_id: String,
    pub pool_id: String,
    pub fact_key: String,
    pub fallback: u64,
}

/// SCHED-C1a: ids by who wrote their value, each list in file order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ValueOrigins {
    /// No origin member: the pack's own hand wrote the value.
    pub pack: Vec<String>,
    /// The origin member is `"model"`: the model's export wrote it.
    pub model: Vec<String>,
}

impl ValueOrigins {
    fn add(&mut self, origin: ScheduleValueOrigin, id: &str) {
        match origin {
            ScheduleValueOrigin::Pack => self.pack.push(id.to_owned()),
            ScheduleValueOrigin::Model => self.model.push(id.to_owned()),
        }
    }
}

/// One judged rule that declares no value: its id and the touched pool without a valuation
/// (`None` when it has no entry at all).
#[derive(Debug, Clone, PartialEq, Eq)]
struct UndeclaredRule {
    rule_id: String,
    pool_id: Option<String>,
}

/// #428 CW-1 (register §1.290): one compile warning of the composed catalog, placed as a refusal
/// is: the game whose `schedule.json` gave the item it came from, and the pointer in that file. A
/// warning the composer cannot place in a game's file (none so far: each compile warning is at a
/// composed item) has no game and keeps its pointer in the composed document.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct StandardPackCompileWarning {
    pub game_id: Option<String>,
    pub code: CatalogDiagnosticCode,
    pub pointer: String,
}

impl StandardPackCompileWarning {
    /// The contract's form, the one `config.catalog_warnings` records.
    pub fn to_contract(&self) -> CatalogCompileWarning {
        CatalogCompileWarning {
            warning: self.code.code().as_str().to_owned(),
            game: self.game_id.clone(),
            pointer: self.pointer.clone(),
        }
    }
}

/// A composed catalog of the standard packs' rules.
#[derive(Debug, Clone)]
pub struct StandardPackComposition {
    sources: CatalogSources,
    compiled: CompiledCatalog,
    manifest: Vec<ComposedProcedure>,
    games: Vec<ComposedGame>,
    rules_digest: String,
    /// SS-2: each game whose composed rules use more than [`STANDARD_PACK_PRIORITY_TIERS`]
    /// distinct priority values, with that number, by game id.
    priority_tiers: Vec<(String, usize)>,
    /// SS-2b: each game whose `schedule.json` was read, by game id, with its judged rules'
    /// coverage and the rules that declare no value, in file order.
    benefit: Vec<(String, BenefitCoverage, Vec<UndeclaredRule>)>,
    /// SCHED-C1a: each game whose `schedule.json` was read, by game id, with who wrote its fixed
    /// values and its valuation blocks.
    provenance: Vec<(String, BenefitProvenance)>,
    /// #428 COV-1: each game whose `schedule.json` was read, by game id, with its chain followers
    /// in file order.
    followers: Vec<(String, Vec<BenefitFollower>)>,
    /// SCHED-C1f1: each game whose `schedule.json` was read, by game id, with its consumptions
    /// read from a fact in file order.
    consumption_facts: Vec<(String, Vec<ConsumptionFactRule>)>,
    /// SS-2c: each game whose composed rules use exactly two priority values and whose rules
    /// include one in the other tier than its declaration says, by game id, with those rules' ids
    /// in file order.
    tier_mismatches: Vec<(String, Vec<String>)>,
    /// SS-4: each game with a rule that writes no daily bound and an escalation threshold above
    /// [`STANDARD_PACK_ESCALATION_WARNING_THRESHOLD`], by game id, with those rules' ids and
    /// thresholds in file order.
    escalations: Vec<(String, Vec<(String, u64)>)>,
    /// SCHED-C1c: each game with a judged rule whose trigger still requires a consumed capped
    /// pool's reading at least its capacity less a margin, by game id, with those rules' and
    /// pools' file ids in file order.
    redundant_triggers: Vec<(String, Vec<(String, String)>)>,
    /// #428 CW-1: the compiled catalog's warnings, placed and each once.
    compile_warnings: Vec<StandardPackCompileWarning>,
    /// Workflow #455 G-R2s: each game whose `schedule.json` was read and whose root has a package
    /// that could not be read, by game id, with those packages by id.
    unreadable: Vec<(String, Vec<String>)>,
}

impl StandardPackComposition {
    /// The four V2 documents, as compiled.
    pub fn sources(&self) -> &CatalogSources {
        &self.sources
    }

    pub fn compiled(&self) -> &CompiledCatalog {
        &self.compiled
    }

    /// #428 CW-1 (register §1.290): the compiled catalog's warnings ([`CompiledCatalog::warnings`],
    /// for example `pool_lines_unvalued`), each placed in its game's `schedule.json` as a refusal
    /// is, each once (a pool composed for two instances warns at both copies, one place), in the
    /// order game, code, pointer. Nothing about the composition depends on them.
    pub fn compile_warnings(&self) -> &[StandardPackCompileWarning] {
        &self.compile_warnings
    }

    /// One entry per `procedure_ref` the composed tasks name, by `procedure_ref`.
    pub fn manifest(&self) -> &[ComposedProcedure] {
        &self.manifest
    }

    /// Every game with a bound instance, by game id.
    pub fn games(&self) -> &[ComposedGame] {
        &self.games
    }

    /// `sha256:<hex>` over the composed tasks, pools, profiles and events (without the
    /// descriptor) and the manifest (model §3.2 step 5): equal digests mean nothing changed.
    pub fn rules_digest(&self) -> &str {
        &self.rules_digest
    }

    /// One `standard_pack_package_unclassified` warning per unclassified package of a game in
    /// use (keys `game`, `package_id`); then, SS-2, one `standard_pack_priority_tiers_exceeded`
    /// per game whose composed rules use more than two distinct priority values (keys `game`,
    /// `count` the number of values, `limit_count` 2), by game id; then, Workflow #455 G-R2s
    /// (register §1.363), one `standard_pack_package_unreadable` per package whose declaration
    /// could not be read, of a game whose `schedule.json` was read (keys `game`, `package_id`), by
    /// game id, then package id: every composed task of that game carries the irreversible mark.
    pub fn warnings(&self) -> Vec<Outcome> {
        let unclassified = self.games.iter().flat_map(|game| {
            game.unclassified.iter().map(|package_id| {
                Outcome::new(SharedCode::StandardPackPackageUnclassified)
                    .game(game.game_id.as_str())
                    .package_id(package_id.as_str())
            })
        });
        let tiers = self.priority_tiers.iter().map(|(game_id, count)| {
            Outcome::new(SharedCode::StandardPackPriorityTiersExceeded)
                .game(game_id.as_str())
                .count(*count)
                .limit_count(STANDARD_PACK_PRIORITY_TIERS)
        });
        let unreadable = self.unreadable.iter().flat_map(|(game_id, packages)| {
            packages.iter().map(move |package_id| {
                Outcome::new(SharedCode::StandardPackPackageUnreadable)
                    .game(game_id.as_str())
                    .package_id(package_id.as_str())
            })
        });
        unclassified.chain(tiers).chain(unreadable).collect()
    }

    /// Workflow #455 G-R2s (register §1.363): each game whose `schedule.json` was read and whose
    /// root has a package that could not be read, by game id, with those packages by id. Every
    /// composed task of such a game carries [`IRREVERSIBLE_REACH_MEMBER`].
    pub fn unreadable_packages(&self) -> &[(String, Vec<String>)] {
        &self.unreadable
    }

    /// SS-2b (register §1.225): one `standard_pack_benefit_undeclared` per judged rule that
    /// declares no value (keys `game`, `task_id` the rule's id; `pool_id` the touched pool without
    /// a valuation), by game id, then in file order. Only the resource tools report them, after
    /// [`StandardPackComposition::warnings`]; the Runtime's workbench records none.
    pub fn benefit_warnings(&self) -> Vec<Outcome> {
        self.benefit
            .iter()
            .flat_map(|(game_id, _, rules)| {
                rules.iter().map(move |rule| {
                    let outcome = Outcome::new(SharedCode::StandardPackBenefitUndeclared)
                        .game(game_id.as_str())
                        .task_id(rule.rule_id.as_str());
                    match &rule.pool_id {
                        Some(pool_id) => outcome.pool_id(pool_id.as_str()),
                        None => outcome,
                    }
                })
            })
            .collect()
    }

    /// SS-2b: the judged rules' coverage of each game whose `schedule.json` was read, by game id.
    pub fn benefit_coverage(&self) -> Vec<(String, BenefitCoverage)> {
        self.benefit
            .iter()
            .map(|(game_id, coverage, _)| (game_id.clone(), *coverage))
            .collect()
    }

    /// SCHED-C1a (register §1.299; `contracts/standard-pack-schedule.md`, "Provenance"): who
    /// wrote the fixed values and the valuation blocks of each game whose `schedule.json` was
    /// read, by game id. An origin changes no decision, no catalog hash and no rules digest.
    pub fn benefit_provenance(&self) -> &[(String, BenefitProvenance)] {
        &self.provenance
    }

    /// #428 COV-1 (register §1.299; `contracts/standard-pack-schedule.md`, "Coverage"): the chain
    /// followers of each game whose `schedule.json` was read, by game id, each game's in file
    /// order with the complete rule it rides with. A static reading of the file: whether a
    /// candidate is judged at an evaluation is the evaluator's. Only the resource tools report
    /// them.
    pub fn benefit_followers(&self) -> &[(String, Vec<BenefitFollower>)] {
        &self.followers
    }

    /// SCHED-C1f1 (register §1.303 SC1F-7; `contracts/standard-pack-schedule.md`, "A run's
    /// consumption read from a fact", "Tools"): the consumptions read from a fact of each game
    /// whose `schedule.json` was read, by game id, each game's in file order: one per `consumes`
    /// entry with the form of a rule not held, empty when none. Only the resource tools report
    /// them.
    pub fn consumption_facts(&self) -> &[(String, Vec<ConsumptionFactRule>)] {
        &self.consumption_facts
    }

    /// SS-2c (register §1.227): one `standard_pack_priority_tier_mismatch` per rule of a game
    /// whose composed rules use exactly two priority values, when the rule declares itself
    /// maintenance and carries the lower value or is judged and carries the higher (keys `game`,
    /// `task_id` the rule's id), by game id, then in file order. A hint, never a refusal: only
    /// the resource tools report them, after [`StandardPackComposition::benefit_warnings`]; the
    /// Runtime's workbench records none.
    pub fn tier_warnings(&self) -> Vec<Outcome> {
        self.tier_mismatches
            .iter()
            .flat_map(|(game_id, rules)| {
                rules.iter().map(move |rule_id| {
                    Outcome::new(SharedCode::StandardPackPriorityTierMismatch)
                        .game(game_id.as_str())
                        .task_id(rule_id.as_str())
                })
            })
            .collect()
    }

    /// SS-4 (register §1.231): one `standard_pack_escalation_threshold_high` per rule that writes
    /// no `loop_budget.daily_limit` and whose `on_failure.escalation_threshold` is above
    /// [`STANDARD_PACK_ESCALATION_WARNING_THRESHOLD`] (keys `game`, `task_id` the rule's id, `count`
    /// its threshold, `limit_count` 10), by game id, then in file order. A hint, never a refusal:
    /// only the resource tools report them, after [`StandardPackComposition::tier_warnings`]; the
    /// Runtime's workbench records none.
    pub fn escalation_warnings(&self) -> Vec<Outcome> {
        self.escalations
            .iter()
            .flat_map(|(game_id, rules)| {
                rules.iter().map(move |(rule_id, threshold)| {
                    Outcome::new(SharedCode::StandardPackEscalationThresholdHigh)
                        .game(game_id.as_str())
                        .task_id(rule_id.as_str())
                        .count(*threshold)
                        .limit_count(STANDARD_PACK_ESCALATION_WARNING_THRESHOLD)
                })
            })
            .collect()
    }

    /// SCHED-C1c (register §1.299; `contracts/standard-pack-schedule.md`, "A redundant trigger"):
    /// one `standard_pack_line_trigger_redundant` per judged rule and consumed pool, the pool
    /// writing both lines and a declared valuation block, whose trigger still requires the pool's
    /// reading at least its capacity fact less a margin, which the capped spending order already
    /// does (keys `game`, `task_id` the rule's id, `pool_id` the file's pool id), by game id, then
    /// in file order. A hint, never a refusal: only the resource tools report them, after
    /// [`StandardPackComposition::escalation_warnings`]; the Runtime's workbench records none.
    pub fn redundant_trigger_warnings(&self) -> Vec<Outcome> {
        self.redundant_triggers
            .iter()
            .flat_map(|(game_id, rules)| {
                rules.iter().map(move |(rule_id, pool_id)| {
                    Outcome::new(SharedCode::StandardPackLineTriggerRedundant)
                        .game(game_id.as_str())
                        .task_id(rule_id.as_str())
                        .pool_id(pool_id.as_str())
                })
            })
            .collect()
    }
}

/// SCHED-C1c (register §1.299; `contracts/standard-pack-schedule.md`, "A redundant trigger"): the
/// rules of one game, among its composed rules `tasks` (each once, in file order), that are judged
/// and consume a pool of the file writing both lines and a declared valuation block, and whose
/// `trigger` requires, through `all`, the pool's reading (its `resource_projection`, or a `fact` on
/// its reading key) compared `greater_than_or_equal` or `greater_than` with a fact reference to
/// the pool's capacity fact: each with that pool's file id, the pools in file order. A pool whose
/// capacity is a constant has no capacity fact and is not reported. The compiler checks the
/// shapes; a member of another shape here is no such trigger.
fn redundant_triggers(game: &ReadGame<'_>, tasks: &[(Value, Origin)]) -> Vec<(String, String)> {
    let composed = tasks
        .iter()
        .map(|(_, origin)| origin.index)
        .collect::<BTreeSet<_>>();
    let mut found = Vec::new();
    for (index, rule) in game.schedule.rules.iter().enumerate() {
        if !composed.contains(&index) || !judged(rule, &game.classification) {
            continue;
        }
        let mut required = Vec::new();
        if let Some(trigger) = rule.body.get("trigger") {
            required_nodes(trigger, &mut required);
        }
        for pool in &game.schedule.pools {
            if !consumes_pool(rule, &pool.id) || !valued_with_lines(pool) {
                continue;
            }
            let Some(capacity_key) = reference_fact_key(pool.body.get("capacity")) else {
                continue;
            };
            if required
                .iter()
                .any(|node| reads_at_capacity(node, pool, capacity_key))
            {
                found.push((rule.id.clone(), pool.id.clone()));
            }
        }
    }
    found
}

/// SCHED-C1c: the predicate nodes a `schedule.json` trigger requires, every node reached from its
/// root only through `all` (an `all` itself excepted), as the evaluator reads a required
/// condition.
fn required_nodes<'a>(node: &'a Value, found: &mut Vec<&'a Value>) {
    if node.get("kind").and_then(Value::as_str) == Some("all") {
        if let Some(Value::Array(children)) = node.get("predicates") {
            for child in children {
                required_nodes(child, found);
            }
        }
        return;
    }
    found.push(node);
}

/// SCHED-C1c: whether a `schedule.json` rule declares a `consumes` entry of `pool_id`.
fn consumes_pool(rule: &ScheduleRule, pool_id: &str) -> bool {
    let Some(Value::Array(entries)) = rule.body.get("consumes") else {
        return false;
    };
    entries
        .iter()
        .any(|entry| entry.get("pool_id").and_then(Value::as_str) == Some(pool_id))
}

/// SCHED-C1c: whether a `schedule.json` pool writes both lines and a declared valuation block
/// (not the pending marker): a pool the capped spending order spends.
fn valued_with_lines(pool: &ScheduleItem) -> bool {
    let [stop, hard] = POOL_LINE_MEMBERS;
    matches!(pool.body.get("valuation"), Some(Value::Object(_)))
        && pool.body.contains_key(stop)
        && pool.body.contains_key(hard)
}

/// SCHED-C1c: the `fact_key` of a fact reference (an object whose `type` is `fact`); `None` for
/// any other value.
fn reference_fact_key(value: Option<&Value>) -> Option<&str> {
    if !is_fact_reference(value) {
        return None;
    }
    value?.get("fact_key")?.as_str()
}

/// SCHED-C1c: the reading key of a `schedule.json` pool, the `fact_key` of its `fact`
/// observation; `None` for another observation.
fn reading_key(pool: &ScheduleItem) -> Option<&str> {
    let observation = pool.body.get("observation")?;
    if observation.get("kind").and_then(Value::as_str) != Some("fact") {
        return None;
    }
    observation.get("fact_key")?.as_str()
}

/// SCHED-C1c: whether a required trigger node reads `pool`'s reading at least its capacity fact
/// `capacity_key` less a margin: a `resource_projection` on the pool, or a `fact` on its reading
/// key, compared `greater_than_or_equal` or `greater_than` with a fact reference to that fact.
fn reads_at_capacity(node: &Value, pool: &ScheduleItem, capacity_key: &str) -> bool {
    let reads = match node.get("kind").and_then(Value::as_str) {
        Some("resource_projection") => {
            node.get("pool_id").and_then(Value::as_str) == Some(pool.id.as_str())
        }
        Some("fact") => {
            let key = node.get("fact_key").and_then(Value::as_str);
            key.is_some() && key == reading_key(pool)
        }
        _ => false,
    };
    let comparison = node.get("comparison").and_then(Value::as_str);
    reads
        && matches!(comparison, Some("greater_than_or_equal" | "greater_than"))
        && reference_fact_key(node.get("value")) == Some(capacity_key)
}

/// SS-4 (register §1.231): the rules of one game, among its composed rules `tasks` (each once, in
/// file order), that write no daily bound (no `loop_budget`, or one without `daily_limit`) and an
/// `on_failure.escalation_threshold` above [`STANDARD_PACK_ESCALATION_WARNING_THRESHOLD`], with
/// that threshold. The compiler has checked the shapes.
fn high_escalations(game: &ReadGame<'_>, tasks: &[(Value, Origin)]) -> Vec<(String, u64)> {
    let mut composed = BTreeMap::<usize, &Value>::new();
    for (task, origin) in tasks {
        composed.entry(origin.index).or_insert(task);
    }
    composed
        .into_iter()
        .filter_map(|(index, task)| {
            let daily = task
                .get("loop_budget")
                .and_then(|budget| budget.get("daily_limit"));
            let threshold = task
                .get("on_failure")
                .and_then(|policy| policy.get("escalation_threshold"))
                .and_then(Value::as_u64)?;
            let rule = game.schedule.rules.get(index)?;
            (daily.is_none() && threshold > STANDARD_PACK_ESCALATION_WARNING_THRESHOLD)
                .then(|| (rule.id.clone(), threshold))
        })
        .collect()
}

/// Whether the composer judges `rule` by its net benefit: SS-2b, its package is not
/// maintenance-role; SS-2c (register §1.227), and it does not declare itself maintenance.
fn judged(rule: &ScheduleRule, classification: &StandardPackClassification) -> bool {
    !rule.maintenance && !classification.maintenance.contains(&rule.package_id)
}

/// SS-2b: the judged rules of one game read from its file (module doc): every rule that is not
/// held, whose package is not maintenance-role and that does not declare itself maintenance
/// (SS-2c), in file order, as complete, pending or undeclared; SS-2c: and the rules, not held,
/// that declare themselves maintenance. The compiler has checked the shapes; a value that is no
/// object, no list or no string here counts as missing. SCHED-C1a (register §1.299): a rule that
/// writes a fixed value is judged by the pools it consumes alone, and with no consumption it is
/// complete; it is also counted in `fixed_value`. #428 COV-1 (register §1.299): a rule that
/// declares no value and only rides with its chain is a follower ([`chain_followers`]), not
/// undeclared; the followers are returned in file order.
fn benefit_coverage(
    schedule: &StandardPackSchedule,
    classification: &StandardPackClassification,
) -> (BenefitCoverage, Vec<UndeclaredRule>, Vec<BenefitFollower>) {
    let valuations = schedule
        .pools
        .iter()
        .map(|pool| (pool.id.as_str(), pool.body.get("valuation")))
        .collect::<BTreeMap<_, _>>();
    let mut coverage = BenefitCoverage::default();
    let mut undeclared = Vec::new();
    // #428 COV-1: the complete rules, and the rules with no entry and no fixed value.
    let mut complete = BTreeSet::new();
    let mut bare = BTreeSet::new();
    for rule in &schedule.rules {
        if schedule.holds(&rule.package_id) {
            continue;
        }
        if rule.maintenance {
            coverage.maintenance_declared += 1;
        }
        if !judged(rule, classification) {
            continue;
        }
        if rule.realization {
            coverage.realization += 1;
        }
        // SCHED-C1a: a fixed value leaves the rule's `produces` unread.
        let fixed = rule.fixed_value_milli.is_some();
        let members: &[&str] = if fixed {
            &["consumes"]
        } else {
            &["produces", "consumes"]
        };
        if fixed {
            coverage.fixed_value += 1;
        }
        let entries = members
            .iter()
            .filter_map(|member| rule.body.get(*member).and_then(Value::as_array))
            .flatten()
            .collect::<Vec<_>>();
        if entries.is_empty() && !fixed {
            coverage.undeclared += 1;
            undeclared.push(UndeclaredRule {
                rule_id: rule.id.clone(),
                pool_id: None,
            });
            bare.insert(rule.id.as_str());
            continue;
        }
        let mut pending = false;
        let mut missing = None;
        for entry in entries {
            pending |= entry.get("amount").and_then(Value::as_str) == Some(crate::PENDING_MARKER);
            let pool_id = entry.get("pool_id").and_then(Value::as_str).unwrap_or("");
            match valuations.get(pool_id).copied().flatten() {
                Some(Value::String(marker)) if marker == crate::PENDING_MARKER => pending = true,
                Some(Value::Object(_)) => {}
                _ => {
                    missing = Some(pool_id.to_owned());
                    break;
                }
            }
        }
        match missing {
            Some(pool_id) => {
                coverage.undeclared += 1;
                undeclared.push(UndeclaredRule {
                    rule_id: rule.id.clone(),
                    pool_id: Some(pool_id),
                });
            }
            None if pending => coverage.pending += 1,
            None => {
                coverage.complete += 1;
                complete.insert(rule.id.as_str());
            }
        }
    }
    // #428 COV-1: a follower is counted apart and not warned about.
    let followers = chain_followers(schedule, classification, &complete, &bare);
    if !followers.is_empty() {
        let riding = followers
            .iter()
            .map(|follower| follower.task_id.as_str())
            .collect::<BTreeSet<_>>();
        undeclared.retain(|rule| rule.pool_id.is_some() || !riding.contains(rule.rule_id.as_str()));
        coverage.undeclared = undeclared.len();
        coverage.follower = followers.len();
    }
    (coverage, undeclared, followers)
}

/// #428 COV-1 (register §1.299; `contracts/standard-pack-schedule.md`, "Coverage"): the chain
/// followers of one game's file, in file order. A rule is one when it is in `bare` (judged, not
/// held, no `produces` and no `consumes` entry, no fixed value) and [`rides_with`] finds the rule
/// in `complete` its chain rides with. By its trigger alone such a rule can only be due in its
/// chain head's run window, where the evaluator lets it ride with its chain (SS-2e,
/// `contracts/scheduling/README.md`, "Who is judged"); the evaluator stays the judge.
fn chain_followers(
    schedule: &StandardPackSchedule,
    classification: &StandardPackClassification,
    complete: &BTreeSet<&str>,
    bare: &BTreeSet<&str>,
) -> Vec<BenefitFollower> {
    let mut rules = BTreeMap::new();
    for rule in &schedule.rules {
        if !schedule.holds(&rule.package_id) {
            rules.entry(rule.id.as_str()).or_insert(rule);
        }
    }
    let mut followers = Vec::new();
    for rule in &schedule.rules {
        if schedule.holds(&rule.package_id) || !bare.contains(rule.id.as_str()) {
            continue;
        }
        let mut visited = BTreeSet::new();
        if let Some(head) = rides_with(rule, &rules, classification, complete, bare, &mut visited) {
            followers.push(BenefitFollower {
                task_id: rule.id.clone(),
                rides_with: head,
            });
        }
    }
    followers
}

/// #428 COV-1: the complete rule `rule` rides with, or `None` when it is no chain follower. Its
/// trigger's chain links ([`chain_link`]) must all name judged rules: the evaluator walks up
/// whichever link ran last, and a link to a rule that is not judged could leave the candidate
/// judged on its own. Then its edges, in trigger order, each name a rule in `complete` (the
/// answer) or one in `bare` that rides with one itself, walked depth first, each rule once, so a
/// cycle ends the walk; any other rule (not judged, pending, undeclared for a pool) ends it too.
fn rides_with(
    rule: &ScheduleRule,
    rules: &BTreeMap<&str, &ScheduleRule>,
    classification: &StandardPackClassification,
    complete: &BTreeSet<&str>,
    bare: &BTreeSet<&str>,
    visited: &mut BTreeSet<String>,
) -> Option<String> {
    visited.insert(rule.id.clone());
    let mut required = Vec::new();
    if let Some(trigger) = rule.body.get("trigger") {
        required_nodes(trigger, &mut required);
    }
    // A task that names itself is not its own link, as the evaluator reads one.
    let links = required
        .into_iter()
        .filter_map(chain_link)
        .filter(|(task_id, _)| *task_id != rule.id)
        .collect::<Vec<_>>();
    let all_judged = links.iter().all(|(task_id, _)| {
        rules
            .get(task_id.as_str())
            .is_some_and(|linked| judged(linked, classification))
    });
    if !all_judged {
        return None;
    }
    for (task_id, edge) in &links {
        if !*edge {
            continue;
        }
        if complete.contains(task_id.as_str()) {
            return Some(task_id.clone());
        }
        if bare.contains(task_id.as_str())
            && !visited.contains(task_id)
            && let Some(&linked) = rules.get(task_id.as_str())
            && let Some(head) = rides_with(linked, rules, classification, complete, bare, visited)
        {
            return Some(head);
        }
    }
    None
}

/// #428 COV-1: a required trigger node that is a chain link
/// (`contracts/scheduling/README.md`, "Ties and chain heads"; the evaluator's
/// `required_predecessors`): a `dependency_completed` whose `terminal_states` holds `succeeded`,
/// with the task it names and whether it is an *edge*, one that accepts `succeeded` and nothing
/// else and writes `since_own_run: true`, so that the rule is due only while that task's latest
/// run succeeded and the rule has not served it. `None` for any other node: the compiler checks
/// the shapes, and a node of another shape here is no link.
fn chain_link(node: &Value) -> Option<(String, bool)> {
    if node.get("kind").and_then(Value::as_str) != Some("dependency_completed") {
        return None;
    }
    let PredicateSpec::DependencyCompleted {
        task_id,
        terminal_states,
        since_own_run,
    } = serde_json::from_value::<PredicateSpec>(node.clone()).ok()?
    else {
        return None;
    };
    if !terminal_states.contains(&TaskTerminalState::Succeeded) {
        return None;
    }
    let edge = since_own_run == Some(true)
        && terminal_states
            .iter()
            .all(|state| *state == TaskTerminalState::Succeeded);
    Some((task_id, edge))
}

/// SCHED-C1a (register §1.299; `contracts/standard-pack-schedule.md`, "Provenance"): who wrote the
/// values of one game's file, in file order: each rule, not held, that writes a fixed value (the
/// reader and `read_game` refuse one on a rule that is not judged), and each pool whose
/// `valuation` is a block, by its origin member (absent is the pack's).
fn benefit_provenance(schedule: &StandardPackSchedule) -> BenefitProvenance {
    let mut provenance = BenefitProvenance::default();
    for rule in &schedule.rules {
        if rule.fixed_value_milli.is_some() && !schedule.holds(&rule.package_id) {
            provenance
                .fixed_value
                .add(rule.fixed_value_origin, &rule.id);
        }
    }
    for pool in &schedule.pools {
        if matches!(pool.body.get("valuation"), Some(Value::Object(_))) {
            provenance.valuation.add(pool.valuation_origin, &pool.id);
        }
    }
    provenance
}

/// SS-2: the distinct `priority` values among composed rules.
fn priority_values(tasks: &[(Value, Origin)]) -> BTreeSet<i64> {
    tasks
        .iter()
        .filter_map(|(task, _)| task.get("priority").and_then(Value::as_i64))
        .collect()
}

/// SS-2c (register §1.227): the rules of one game, among its composed rules `tasks` (each once,
/// in file order), whose tier by declaration disagrees with their `priority` when the composed
/// rules use exactly two values: a rule that declares itself maintenance with the lower value, a
/// judged rule (no declaration, its package not maintenance-role) with the higher. A rule whose
/// package is maintenance-role and that declares nothing is in neither case. None with fewer or
/// more values: more is SS-2's own warning.
fn tier_mismatches(game: &ReadGame<'_>, tasks: &[(Value, Origin)]) -> Vec<String> {
    let values = priority_values(tasks);
    let (Some(lower), Some(higher)) = (values.first(), values.last()) else {
        return Vec::new();
    };
    if values.len() != STANDARD_PACK_PRIORITY_TIERS {
        return Vec::new();
    }
    let composed = tasks
        .iter()
        .map(|(_, origin)| origin.index)
        .collect::<BTreeSet<_>>();
    composed
        .into_iter()
        .filter_map(|index| game.schedule.rules.get(index))
        .filter(|rule| {
            let priority = rule.body.get("priority").and_then(Value::as_i64);
            if rule.maintenance {
                priority == Some(*lower)
            } else {
                judged(rule, &game.classification) && priority == Some(*higher)
            }
        })
        .map(|rule| rule.id.clone())
        .collect()
}

/// Where a composed element came from: its game, its list and its index in that game's file.
#[derive(Debug, Clone)]
struct Origin {
    game_id: String,
    member: ScheduleMember,
    index: usize,
}

impl Origin {
    fn pointer(&self, tail: &str) -> String {
        format!("/{}/{}{tail}", self.member.as_str(), self.index)
    }
}

/// The composed elements of every document, each with its origin.
#[derive(Default)]
struct Composed {
    tasks: Vec<(Value, Origin)>,
    pools: Vec<(Value, Origin)>,
    profiles: Vec<(Value, Origin)>,
    events: Vec<(Value, Origin)>,
}

impl Composed {
    fn list(&mut self, member: ScheduleMember) -> &mut Vec<(Value, Origin)> {
        match member {
            ScheduleMember::Rules => &mut self.tasks,
            ScheduleMember::Pools => &mut self.pools,
            ScheduleMember::Activity => &mut self.profiles,
            ScheduleMember::Timeline => &mut self.events,
        }
    }

    /// The game and the file pointer of a compiler diagnostic at `path` of `document`.
    fn locate(&self, document: SchedulingDocumentKind, path: &str) -> (Option<String>, String) {
        let (prefix, list) = match document {
            SchedulingDocumentKind::Tasks => ("/tasks/", &self.tasks),
            SchedulingDocumentKind::Pools => ("/pools/", &self.pools),
            SchedulingDocumentKind::Activity => ("/profiles/", &self.profiles),
            SchedulingDocumentKind::Timeline => ("/events/", &self.events),
            SchedulingDocumentKind::Selection | SchedulingDocumentKind::ResourceTargets => {
                return (None, String::new());
            }
        };
        let Some(rest) = path.strip_prefix(prefix) else {
            return (None, String::new());
        };
        let (index, tail) = match rest.find('/') {
            Some(at) => (&rest[..at], &rest[at..]),
            None => (rest, ""),
        };
        // SS-2d: the composed task's realization member comes from the rule's `realization`;
        // SS-2h: its limited member from the rule's `limited`; SCHED-C1a: its fixed value from
        // the rule's `fixed_value_milli`.
        let realization_tail;
        let tail = if document == SchedulingDocumentKind::Tasks
            && tail.strip_prefix('/') == Some(BENEFIT_REALIZATION_MEMBER)
        {
            realization_tail = format!("/{SCHEDULE_RULE_REALIZATION_MEMBER}");
            realization_tail.as_str()
        } else if document == SchedulingDocumentKind::Tasks
            && tail.strip_prefix('/') == Some(BENEFIT_LIMITED_MEMBER)
        {
            realization_tail = format!("/{SCHEDULE_RULE_LIMITED_MEMBER}");
            realization_tail.as_str()
        } else if document == SchedulingDocumentKind::Tasks
            && tail.strip_prefix('/') == Some(BENEFIT_FIXED_VALUE_MEMBER)
        {
            realization_tail = format!("/{SCHEDULE_RULE_FIXED_VALUE_MEMBER}");
            realization_tail.as_str()
        } else {
            tail
        };
        match index
            .parse::<usize>()
            .ok()
            .and_then(|index| list.get(index))
        {
            Some((_, origin)) => (Some(origin.game_id.clone()), origin.pointer(tail)),
            None => (None, String::new()),
        }
    }
}

/// How many copies of each rule, pool, profile and event of one game the composition makes, by
/// file id: the ids of an item with two or more copies are suffixed.
struct Copies<'a> {
    tasks: BTreeMap<&'a str, usize>,
    pools: BTreeMap<&'a str, usize>,
    profiles: BTreeMap<&'a str, usize>,
    events: BTreeMap<&'a str, usize>,
}

impl Copies<'_> {
    fn of(&self, member: ScheduleMember) -> &BTreeMap<&str, usize> {
        match member {
            ScheduleMember::Rules => &self.tasks,
            ScheduleMember::Pools => &self.pools,
            ScheduleMember::Activity => &self.profiles,
            ScheduleMember::Timeline => &self.events,
        }
    }

    /// The id the copy for `alias` gives the item `id` of `member`: suffixed when the item has
    /// two or more copies, else `id` (also for an id that is no item of the game).
    fn id(&self, member: ScheduleMember, id: &str, alias: &str) -> String {
        if self.of(member).get(id).is_some_and(|copies| *copies > 1) {
            format!("{id}@{alias}")
        } else {
            id.to_owned()
        }
    }
}

/// One game in use whose `schedule.json` was read.
struct ReadGame<'a> {
    game_id: &'a str,
    /// The bound instances, (alias, server), by alias.
    bound: Vec<(&'a str, &'a str)>,
    files: StandardPackGameFiles<'a>,
    schedule: StandardPackSchedule,
    classification: StandardPackClassification,
}

/// Composes the standard packs' schedule rules (module doc). Pure: no file, clock or ledger.
pub fn compose_standard_pack_rules(
    input: &StandardPackComposeInput<'_>,
) -> Result<StandardPackComposition, StandardPackComposeFailure> {
    let mut issues = Vec::new();
    let mut in_use = BTreeMap::<&str, Vec<(&str, &str)>>::new();
    for (alias, binding) in input.instances {
        in_use
            .entry(binding.game_id.as_str())
            .or_default()
            .push((alias.as_str(), binding.server_id.as_str()));
    }
    let mut read = Vec::new();
    let mut games = Vec::new();
    for (game_id, bound) in &in_use {
        let Some(files) = input.games.get(*game_id) else {
            games.push(ComposedGame {
                game_id: (*game_id).to_owned(),
                instances: bound.iter().map(|(alias, _)| (*alias).to_owned()).collect(),
                schedule_sha256: None,
                requires: Vec::new(),
                tasks: 0,
                maintenance: Vec::new(),
                scheduled: Vec::new(),
                held: Vec::new(),
                unclassified: Vec::new(),
            });
            continue;
        };
        match read_game(game_id, *files, input.subsystems) {
            Ok((schedule, classification)) => read.push(ReadGame {
                game_id,
                bound: bound.clone(),
                files: *files,
                schedule,
                classification,
            }),
            Err(issue) => issues.push(issue),
        }
    }

    let mut composed = Composed::default();
    let mut manifest = BTreeMap::<String, ComposedProcedure>::new();
    let mut priority_tiers = Vec::new();
    let mut benefit = Vec::new();
    let mut provenance = Vec::new();
    let mut followers = Vec::new();
    let mut consumption_facts = Vec::new();
    let mut mismatches = Vec::new();
    let mut escalations = Vec::new();
    let mut redundant = Vec::new();
    let mut unreadable = Vec::new();
    for game in &read {
        let before = composed.tasks.len();
        // SS-2b: how far the game's judged rules declare their values, counted once per rule.
        let (coverage, undeclared, riding) = benefit_coverage(&game.schedule, &game.classification);
        // G-R2s: the rules a goal never weighs, by the whole root's declarations and the chain
        // followers that ride with them.
        let marked = irreversible_rules(game, &riding);
        instantiate(game, &marked, &mut composed, &mut manifest, &mut issues);
        let unread = game.files.irreversible.unreadable();
        if !unread.is_empty() {
            unreadable.push((
                game.game_id.to_owned(),
                unread.iter().cloned().collect::<Vec<_>>(),
            ));
        }
        benefit.push((game.game_id.to_owned(), coverage, undeclared));
        // #428 COV-1: the rules that ride with their chain, each with the rule it rides with.
        followers.push((game.game_id.to_owned(), riding));
        // SCHED-C1a: who wrote its fixed values and its valuation blocks.
        provenance.push((game.game_id.to_owned(), benefit_provenance(&game.schedule)));
        // SCHED-C1f1: its consumptions read from a fact, with their keys and fallbacks.
        consumption_facts.push((
            game.game_id.to_owned(),
            consumption_fact_rules(&game.schedule),
        ));
        // SS-2: the game's composed rules, warned about past two tiers and never refused.
        let tiers = priority_values(&composed.tasks[before..]).len();
        if tiers > STANDARD_PACK_PRIORITY_TIERS {
            priority_tiers.push((game.game_id.to_owned(), tiers));
        }
        // SS-2c: with exactly two tiers, the rules whose declaration disagrees with their tier.
        let rules = tier_mismatches(game, &composed.tasks[before..]);
        if !rules.is_empty() {
            mismatches.push((game.game_id.to_owned(), rules));
        }
        // SS-4: the rules without a daily bound whose failures escalate late.
        let rules = high_escalations(game, &composed.tasks[before..]);
        if !rules.is_empty() {
            escalations.push((game.game_id.to_owned(), rules));
        }
        // SCHED-C1c: the rules whose old capacity threshold the capped spending order repeats.
        let rules = redundant_triggers(game, &composed.tasks[before..]);
        if !rules.is_empty() {
            redundant.push((game.game_id.to_owned(), rules));
        }
        games.push(ComposedGame {
            game_id: game.game_id.to_owned(),
            instances: game
                .bound
                .iter()
                .map(|(alias, _)| (*alias).to_owned())
                .collect(),
            schedule_sha256: Some(sha256(game.files.schedule)),
            requires: game.schedule.requires.clone(),
            tasks: composed.tasks.len() - before,
            maintenance: listed(&game.classification.maintenance),
            scheduled: listed(&game.classification.scheduled),
            held: listed(&game.classification.held),
            unclassified: listed(&game.classification.unclassified),
        });
    }
    games.sort_by(|left, right| left.game_id.cmp(&right.game_id));
    id_conflicts(&composed, &mut issues);
    if !issues.is_empty() {
        return Err(StandardPackComposeFailure { issues });
    }

    let sources = documents(&composed, input.catalog_version).map_err(|issue| {
        StandardPackComposeFailure {
            issues: vec![issue],
        }
    })?;
    let compiled = compile_composed_catalog(&sources).map_err(|failure| {
        let issues = failure
            .diagnostics()
            .iter()
            .map(|diagnostic| {
                let (game_id, pointer) =
                    composed.locate(diagnostic.source.document, &diagnostic.json_path);
                StandardPackRuleIssue {
                    game_id,
                    issue: schedule_issue(StandardPackProblem::ScheduleInvalid, pointer),
                    detail: format!(
                        "{} at {}: {}",
                        diagnostic.code.code().as_str(),
                        diagnostic.json_path,
                        diagnostic.reason
                    ),
                }
            })
            .collect();
        StandardPackComposeFailure { issues }
    })?;
    // #428 CW-1: the warnings the compiler returns with the catalog, placed as its refusals are.
    let mut compile_warnings = compiled
        .warnings()
        .iter()
        .map(|diagnostic| {
            let (game_id, pointer) =
                composed.locate(diagnostic.source.document, &diagnostic.json_path);
            let pointer = match game_id {
                Some(_) => pointer,
                None => diagnostic.json_path.clone(),
            };
            StandardPackCompileWarning {
                game_id,
                code: diagnostic.code,
                pointer,
            }
        })
        .collect::<Vec<_>>();
    compile_warnings.sort();
    compile_warnings.dedup();
    let manifest = manifest.into_values().collect::<Vec<_>>();
    let rules_digest =
        rules_digest(&compiled, &manifest).map_err(|issue| StandardPackComposeFailure {
            issues: vec![issue],
        })?;
    priority_tiers.sort();
    benefit.sort_by(|left, right| left.0.cmp(&right.0));
    provenance.sort_by(|left, right| left.0.cmp(&right.0));
    followers.sort_by(|left, right| left.0.cmp(&right.0));
    consumption_facts.sort_by(|left, right| left.0.cmp(&right.0));
    mismatches.sort_by(|left, right| left.0.cmp(&right.0));
    escalations.sort_by(|left, right| left.0.cmp(&right.0));
    redundant.sort_by(|left, right| left.0.cmp(&right.0));
    unreadable.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(StandardPackComposition {
        sources,
        compiled,
        manifest,
        games,
        rules_digest,
        priority_tiers,
        benefit,
        provenance,
        followers,
        consumption_facts,
        tier_mismatches: mismatches,
        escalations,
        redundant_triggers: redundant,
        compile_warnings,
        unreadable,
    })
}

/// Workflow #455 G-R2s (register §1.363; `contracts/standard-pack-schedule.md`, "The irreversible
/// mark"): the ids of the rules of one game, not held, that carry [`IRREVERSIBLE_REACH_MEMBER`]:
/// a rule whose package counts as irreversible by the game root's declarations
/// ([`IrreversiblePackages::counts_irreversible`]: every rule while a package of the root could
/// not be read), and a rule whose chain followers `riding` (each with the complete rule it rides
/// with, [`chain_followers`]) include one, directly or further down, that runs such a package, by
/// the walk the goal check runs ([`IrreversiblePackages::followers_irreversible`]). A follower
/// the walk cannot place counts as irreversible.
fn irreversible_rules<'a>(game: &'a ReadGame<'_>, riding: &[BenefitFollower]) -> BTreeSet<&'a str> {
    let irreversible = game.files.irreversible;
    // A follower with the rule it rides with, as the goal check's references list them (a
    // follower of a follower rides with the same rule).
    let mut followers_of = BTreeMap::<&str, Vec<String>>::new();
    for follower in riding {
        followers_of
            .entry(follower.rides_with.as_str())
            .or_default()
            .push(follower.task_id.clone());
    }
    let rules = game
        .schedule
        .rules
        .iter()
        .filter(|rule| !game.schedule.holds(&rule.package_id))
        .map(|rule| (rule.id.as_str(), rule))
        .collect::<BTreeMap<&'a str, &'a ScheduleRule>>();
    let none: &[String] = &[];
    let mut marked = BTreeSet::new();
    for (rule_id, rule) in &rules {
        let own = followers_of.get(rule_id).map_or(none, Vec::as_slice);
        let reaches = irreversible.counts_irreversible(&rule.package_id)
            || irreversible.followers_irreversible(own, |follower_id| {
                rules.get(follower_id).map(|follower| {
                    (
                        follower.package_id.as_str(),
                        followers_of.get(follower_id).map_or(none, Vec::as_slice),
                    )
                })
            });
        if reaches {
            marked.insert(*rule_id);
        }
    }
    marked
}

/// Reads one game's `schedule.json` (step 2): the contract reader, `requires` against
/// `subsystems`, the references into the registry, and the classification.
fn read_game(
    game_id: &str,
    files: StandardPackGameFiles<'_>,
    subsystems: Option<&BTreeSet<String>>,
) -> Result<(StandardPackSchedule, StandardPackClassification), StandardPackRuleIssue> {
    let refused = |issue: StandardPackRootIssue, detail: String| StandardPackRuleIssue {
        game_id: Some(game_id.to_owned()),
        issue,
        detail,
    };
    let schedule = parse_standard_pack_schedule(files.schedule)
        .map_err(|issue| refused(issue, "schedule.json breaks its own rules".to_owned()))?;
    // #386 CF-4: rules that write a fact reference name its subsystem in `requires`, so that a
    // Runtime that cannot read one refuses the file instead of misreading it.
    if !schedule
        .requires
        .iter()
        .any(|name| name == FACT_REFERENCE_SUBSYSTEM)
        && let Some(pointer) = first_fact_reference(&schedule)
    {
        return Err(refused(
            schedule_issue(StandardPackProblem::ScheduleInvalid, pointer),
            format!("a fact reference needs requires to name {FACT_REFERENCE_SUBSYSTEM}"),
        ));
    }
    // #386 CF-5: likewise a pool's `capacity` or a `resource_projection`'s `value` written as a
    // fact reference names its own subsystem.
    if !schedule
        .requires
        .iter()
        .any(|name| name == POOL_FACT_REFERENCE_SUBSYSTEM)
        && let Some(pointer) = first_pool_fact_reference(&schedule)
    {
        return Err(refused(
            schedule_issue(StandardPackProblem::ScheduleInvalid, pointer),
            format!(
                "a pool capacity or a resource projection read from a fact needs requires to name \
                 {POOL_FACT_REFERENCE_SUBSYSTEM}"
            ),
        ));
    }
    // Workflow #411 P19: likewise a timeline event that writes `excludes_fact_prefixes`.
    if !schedule
        .requires
        .iter()
        .any(|name| name == RESET_EXCLUSIONS_SUBSYSTEM)
        && let Some(pointer) = first_reset_exclusion(&schedule)
    {
        return Err(refused(
            schedule_issue(StandardPackProblem::ScheduleInvalid, pointer),
            format!(
                "a timeline event's {EXCLUDES_FACT_PREFIXES} needs requires to name \
                 {RESET_EXCLUSIONS_SUBSYSTEM}"
            ),
        ));
    }
    // SS-5: likewise a rule's `fact` predicate that writes `on_unknown`.
    if !schedule
        .requires
        .iter()
        .any(|name| name == FACT_UNKNOWN_PROBE_SUBSYSTEM)
        && let Some(probe) = standard_pack_fact_probes(&schedule).into_iter().next()
    {
        return Err(refused(
            schedule_issue(StandardPackProblem::ScheduleInvalid, probe.pointer),
            format!(
                "a fact predicate's {FACT_ON_UNKNOWN} needs requires to name \
                 {FACT_UNKNOWN_PROBE_SUBSYSTEM}"
            ),
        ));
    }
    // SS-2h (register §1.289): likewise a pool that writes `stop_line` or `hard_line`, in either
    // form.
    if !schedule
        .requires
        .iter()
        .any(|name| name == POOL_LINES_SUBSYSTEM)
        && let Some(pointer) = schedule.pools.iter().enumerate().find_map(|(index, pool)| {
            POOL_LINE_MEMBERS
                .iter()
                .find(|member| pool.body.contains_key(**member))
                .map(|member| format!("/pools/{index}/{member}"))
        })
    {
        return Err(refused(
            schedule_issue(StandardPackProblem::ScheduleInvalid, pointer),
            format!(
                "a pool's stop_line and hard_line need requires to name {POOL_LINES_SUBSYSTEM}"
            ),
        ));
    }
    // SCHED-C1a (register §1.299): likewise a rule that writes `fixed_value_milli` or a pool that
    // writes `valuation_origin`, the first in file order, rules then pools.
    if !schedule
        .requires
        .iter()
        .any(|name| name == BENEFIT_FIXED_VALUE_SUBSYSTEM)
        && let Some(pointer) = first_fixed_value_member(&schedule)
    {
        return Err(refused(
            schedule_issue(StandardPackProblem::ScheduleInvalid, pointer),
            format!(
                "a rule's {SCHEDULE_RULE_FIXED_VALUE_MEMBER} and a pool's \
                 {SCHEDULE_POOL_VALUATION_ORIGIN_MEMBER} need requires to name \
                 {BENEFIT_FIXED_VALUE_SUBSYSTEM}"
            ),
        ));
    }
    // SCHED-C1c (register §1.299): likewise a pool that writes `trigger_line`, in either form; its
    // two lines need `pool_lines` besides.
    if !schedule
        .requires
        .iter()
        .any(|name| name == crate::POOL_TRIGGER_LINE_SUBSYSTEM)
        && let Some(pointer) = first_trigger_line(&schedule)
    {
        return Err(refused(
            schedule_issue(StandardPackProblem::ScheduleInvalid, pointer),
            format!(
                "a pool's {POOL_TRIGGER_LINE_MEMBER} needs requires to name {}",
                crate::POOL_TRIGGER_LINE_SUBSYSTEM
            ),
        ));
    }
    // SCHED-C1f1 (register §1.303 SC1F-6): likewise a rule whose `consumes` entry writes its
    // `amount` as a fact, the first such `amount` in file order.
    if !schedule
        .requires
        .iter()
        .any(|name| name == CONSUMPTION_FACT_SUBSYSTEM)
        && let Some(pointer) = first_consumption_fact(&schedule)
    {
        return Err(refused(
            schedule_issue(StandardPackProblem::ScheduleInvalid, pointer),
            format!(
                "a consumption read from a fact needs requires to name {CONSUMPTION_FACT_SUBSYSTEM}"
            ),
        ));
    }
    // Workflow #437 ORD-1a (register §1.352): likewise a rule that writes `max_queue_wait_ms`,
    // the first in file order, whatever its value (the compiler checks the value).
    if !schedule
        .requires
        .iter()
        .any(|name| name == crate::QUEUE_WAIT_SUBSYSTEM)
        && let Some(index) = schedule
            .rules
            .iter()
            .position(|rule| rule.body.contains_key(MAX_QUEUE_WAIT_MEMBER))
    {
        return Err(refused(
            schedule_issue(
                StandardPackProblem::ScheduleInvalid,
                format!("/rules/{index}/{MAX_QUEUE_WAIT_MEMBER}"),
            ),
            format!(
                "a rule's {MAX_QUEUE_WAIT_MEMBER} needs requires to name {}",
                crate::QUEUE_WAIT_SUBSYSTEM
            ),
        ));
    }
    // UPD-1 fix round 5 (`standard-pack-schedule.md`, "The store update rule"): no timeline event
    // invalidates the Runtime's application update fact. A reset that cleared it would leave the
    // store update rule's trigger reading the key missing (Unknown), so the rule would never run,
    // while the Runtime's gate, which reads the fact as recorded, kept holding the instance.
    if let Some(pointer) = first_application_update_invalidation(&schedule) {
        return Err(refused(
            schedule_issue(StandardPackProblem::ScheduleInvalid, pointer),
            format!(
                "a timeline event's {INVALIDATES_FACT_PREFIXES} may not cover \
                 {APPLICATION_UPDATE_REQUIRED_FACT_KEY}, which only the Runtime writes"
            ),
        ));
    }
    // SS-2b (register §1.225): the composer marks the judged rules by their package's
    // classification; a rule that writes the mark itself would decide its own judgement.
    if let Some(index) = schedule
        .rules
        .iter()
        .position(|rule| rule.body.contains_key(BENEFIT_JUDGED_MEMBER))
    {
        return Err(refused(
            schedule_issue(
                StandardPackProblem::ScheduleInvalid,
                format!("/rules/{index}/{BENEFIT_JUDGED_MEMBER}"),
            ),
            format!(
                "{BENEFIT_JUDGED_MEMBER} is the composer's: it marks every rule whose package is \
                 not maintenance-role and that does not declare itself maintenance"
            ),
        ));
    }
    // SS-2d (register §1.235): likewise the realization task member is the composer's, written
    // from the rule member `realization`.
    if let Some(index) = schedule
        .rules
        .iter()
        .position(|rule| rule.body.contains_key(BENEFIT_REALIZATION_MEMBER))
    {
        return Err(refused(
            schedule_issue(
                StandardPackProblem::ScheduleInvalid,
                format!("/rules/{index}/{BENEFIT_REALIZATION_MEMBER}"),
            ),
            format!(
                "{BENEFIT_REALIZATION_MEMBER} is the composer's: a rule carries the model's mark as \
                 {SCHEDULE_RULE_REALIZATION_MEMBER}"
            ),
        ));
    }
    // SS-2h (register §1.289): likewise the limited task member is the composer's, written from
    // the rule member `limited`, which, if written, is `true` and not on a rule that declares
    // itself maintenance.
    if let Some(index) = schedule
        .rules
        .iter()
        .position(|rule| rule.body.contains_key(BENEFIT_LIMITED_MEMBER))
    {
        return Err(refused(
            schedule_issue(
                StandardPackProblem::ScheduleInvalid,
                format!("/rules/{index}/{BENEFIT_LIMITED_MEMBER}"),
            ),
            format!(
                "{BENEFIT_LIMITED_MEMBER} is the composer's: a rule carries the model's mark as \
                 {SCHEDULE_RULE_LIMITED_MEMBER}"
            ),
        ));
    }
    if let Some(index) = schedule.rules.iter().position(|rule| {
        rule.body
            .get(SCHEDULE_RULE_LIMITED_MEMBER)
            .is_some_and(|value| *value != Value::Bool(true) || rule.maintenance)
    }) {
        return Err(refused(
            schedule_issue(
                StandardPackProblem::ScheduleInvalid,
                format!("/rules/{index}/{SCHEDULE_RULE_LIMITED_MEMBER}"),
            ),
            format!(
                "{SCHEDULE_RULE_LIMITED_MEMBER}, if written, is true, on a rule that does not \
                 declare itself maintenance"
            ),
        ));
    }
    // SCHED-C1a (register §1.299): likewise the fixed value's task member is the composer's,
    // written from the rule member `fixed_value_milli`.
    if let Some(index) = schedule
        .rules
        .iter()
        .position(|rule| rule.body.contains_key(BENEFIT_FIXED_VALUE_MEMBER))
    {
        return Err(refused(
            schedule_issue(
                StandardPackProblem::ScheduleInvalid,
                format!("/rules/{index}/{BENEFIT_FIXED_VALUE_MEMBER}"),
            ),
            format!(
                "{BENEFIT_FIXED_VALUE_MEMBER} is the composer's: a rule writes its value as \
                 {SCHEDULE_RULE_FIXED_VALUE_MEMBER}"
            ),
        ));
    }
    // Workflow #455 G-R2s (register §1.363): likewise the irreversible mark is the composer's,
    // written from the game root's package declarations; a rule that wrote it would decide its
    // own safety line.
    if let Some(index) = schedule
        .rules
        .iter()
        .position(|rule| rule.body.contains_key(IRREVERSIBLE_REACH_MEMBER))
    {
        return Err(refused(
            schedule_issue(
                StandardPackProblem::ScheduleInvalid,
                format!("/rules/{index}/{IRREVERSIBLE_REACH_MEMBER}"),
            ),
            format!(
                "{IRREVERSIBLE_REACH_MEMBER} is the composer's: it marks every rule whose package, \
                 or a follower riding with it, can do something irreversible by the game root's \
                 declarations"
            ),
        ));
    }
    if let Some(subsystems) = subsystems
        && let Some((index, name)) = schedule
            .requires
            .iter()
            .enumerate()
            .find(|(_, name)| !subsystems.contains(*name))
    {
        return Err(refused(
            schedule_issue(
                StandardPackProblem::ScheduleRequiresMissing,
                format!("/requires/{index}"),
            ),
            format!("requires the subsystem {name}, which this Runtime does not name"),
        ));
    }
    validate_standard_pack_schedule_references(&files.root.registry, &schedule).map_err(
        |issue| {
            refused(
                issue,
                "schedule.json names what the game root does not hold".to_owned(),
            )
        },
    )?;
    let maintenance = standard_pack_maintenance_packages(files.maintenance, files.root);
    let classification =
        classify_standard_pack_packages(&files.root.registry, &maintenance, &schedule);
    // SS-2d (register §1.235; design §7.3 check 1): the realization mark belongs on a judged rule
    // only; a rule whose package is maintenance-role is refused as SS-2c refuses a bad
    // declaration (the reader already refuses it beside `"maintenance": true`).
    if let Some(index) = schedule
        .rules
        .iter()
        .position(|rule| rule.realization && !judged(rule, &classification))
    {
        return Err(refused(
            schedule_issue(
                StandardPackProblem::ScheduleInvalid,
                format!("/rules/{index}/{SCHEDULE_RULE_REALIZATION_MEMBER}"),
            ),
            format!(
                "{SCHEDULE_RULE_REALIZATION_MEMBER} marks a judged rule only, and this rule's \
                 package is maintenance-role"
            ),
        ));
    }
    // SS-2h (register §1.289): likewise the limited mark.
    if let Some(index) = schedule.rules.iter().position(|rule| {
        rule.body.contains_key(SCHEDULE_RULE_LIMITED_MEMBER) && !judged(rule, &classification)
    }) {
        return Err(refused(
            schedule_issue(
                StandardPackProblem::ScheduleInvalid,
                format!("/rules/{index}/{SCHEDULE_RULE_LIMITED_MEMBER}"),
            ),
            format!(
                "{SCHEDULE_RULE_LIMITED_MEMBER} marks a judged rule only, and this rule's package \
                 is maintenance-role"
            ),
        ));
    }
    // SCHED-C1a (register §1.299): likewise the fixed value, which only a judged rule's benefit
    // reads (the reader already refuses it beside `"maintenance": true`).
    if let Some(index) = schedule
        .rules
        .iter()
        .position(|rule| rule.fixed_value_milli.is_some() && !judged(rule, &classification))
    {
        return Err(refused(
            schedule_issue(
                StandardPackProblem::ScheduleInvalid,
                format!("/rules/{index}/{SCHEDULE_RULE_FIXED_VALUE_MEMBER}"),
            ),
            format!(
                "{SCHEDULE_RULE_FIXED_VALUE_MEMBER} values a judged rule only, and this rule's \
                 package is maintenance-role"
            ),
        ));
    }
    Ok((schedule, classification))
}

/// SCHED-C1a (register §1.299): the pointer of the first member that requires
/// [`BENEFIT_FIXED_VALUE_SUBSYSTEM`], in the order of the members `rules` then `pools`: a rule's
/// `fixed_value_milli`, then a pool's `valuation_origin` (the reader takes both out and keeps
/// what they said). `None` when there is none.
fn first_fixed_value_member(schedule: &StandardPackSchedule) -> Option<String> {
    let rule = schedule
        .rules
        .iter()
        .position(|rule| rule.fixed_value_milli.is_some())
        .map(|index| format!("/rules/{index}/{SCHEDULE_RULE_FIXED_VALUE_MEMBER}"));
    rule.or_else(|| {
        schedule
            .pools
            .iter()
            .position(|pool| pool.valuation_origin == ScheduleValueOrigin::Model)
            .map(|index| format!("/pools/{index}/{SCHEDULE_POOL_VALUATION_ORIGIN_MEMBER}"))
    })
}

/// SCHED-C1f1 (register §1.303 SC1F-6): each `consumes` entry of a rule, in file order, whose
/// `amount` is written as a fact (an object whose `type` is `fact`), with its rule and the
/// entry's index. The compiler checks the rest.
fn consumption_fact_entries(
    schedule: &StandardPackSchedule,
) -> impl Iterator<Item = (usize, &ScheduleRule, usize, &Map<String, Value>)> {
    schedule
        .rules
        .iter()
        .enumerate()
        .flat_map(|(rule_index, rule)| {
            rule.body
                .get("consumes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .enumerate()
                .filter_map(move |(entry_index, entry)| {
                    let amount = entry.get("amount");
                    if !is_fact_reference(amount) {
                        return None;
                    }
                    let members = amount?.as_object()?;
                    Some((rule_index, rule, entry_index, members))
                })
        })
}

/// SCHED-C1f1 (register §1.303 SC1F-6): the pointer of the first `amount` written as a fact, in
/// file order (rules, then their `consumes` entries), which requires
/// [`CONSUMPTION_FACT_SUBSYSTEM`]. `None` when no rule writes one.
fn first_consumption_fact(schedule: &StandardPackSchedule) -> Option<String> {
    consumption_fact_entries(schedule)
        .next()
        .map(|(rule, _, entry, _)| format!("/rules/{rule}/consumes/{entry}/amount"))
}

/// SCHED-C1f1 (register §1.303 SC1F-7): the consumptions read from a fact of one game's file, in
/// file order, of the rules not held (the composed ones, which the compiler checked): one
/// [`ConsumptionFactRule`] per entry, by the file's ids.
fn consumption_fact_rules(schedule: &StandardPackSchedule) -> Vec<ConsumptionFactRule> {
    let [_, fact_key, fallback] = crate::schema::CONSUMPTION_FACT_MEMBERS;
    consumption_fact_entries(schedule)
        .filter(|(_, rule, _, _)| !schedule.holds(&rule.package_id))
        .filter_map(|(_, rule, entry, members)| {
            let pool_id = rule
                .body
                .get("consumes")?
                .get(entry)?
                .get("pool_id")?
                .as_str()?;
            Some(ConsumptionFactRule {
                rule_id: rule.id.clone(),
                pool_id: pool_id.to_owned(),
                fact_key: members.get(fact_key)?.as_str()?.to_owned(),
                fallback: members.get(fallback)?.as_u64()?,
            })
        })
        .collect()
}

/// SCHED-C1c (register §1.299): the pointer of the first pool, in file order, that writes
/// `trigger_line`, in either form. `None` when no pool writes one.
fn first_trigger_line(schedule: &StandardPackSchedule) -> Option<String> {
    schedule
        .pools
        .iter()
        .position(|pool| pool.body.contains_key(POOL_TRIGGER_LINE_MEMBER))
        .map(|index| format!("/pools/{index}/{POOL_TRIGGER_LINE_MEMBER}"))
}

/// #386 CF-4: the pointer of the first fact reference a rule's `trigger` or `feedback_stop`
/// writes, in file order: the `value` of a `fact` node whose `type` is `fact`. `None` when no
/// rule writes one.
fn first_fact_reference(schedule: &StandardPackSchedule) -> Option<String> {
    first_rule_reference(schedule, "fact")
}

/// #386 CF-5: the pointer of the first pool fact reference, in the order of the members `rules`
/// then `pools`: the `value` of a `resource_projection` node in a rule's `trigger` or
/// `feedback_stop`, then a pool's `capacity`, whose `type` is `fact`. `None` when there is none.
fn first_pool_fact_reference(schedule: &StandardPackSchedule) -> Option<String> {
    first_rule_reference(schedule, "resource_projection").or_else(|| {
        schedule
            .pools
            .iter()
            .enumerate()
            .find(|(_, pool)| is_fact_reference(pool.body.get("capacity")))
            .map(|(index, _)| format!("/pools/{index}/capacity"))
    })
}

/// The timeline event member of Workflow #411 P19.
const EXCLUDES_FACT_PREFIXES: &str = "excludes_fact_prefixes";

/// Workflow #411 P19: the pointer of the first timeline event that writes
/// `excludes_fact_prefixes`, with any value, in file order. `None` when no event writes it.
fn first_reset_exclusion(schedule: &StandardPackSchedule) -> Option<String> {
    schedule
        .timeline
        .iter()
        .position(|event| event.body.contains_key(EXCLUDES_FACT_PREFIXES))
        .map(|index| format!("/timeline/{index}/{EXCLUDES_FACT_PREFIXES}"))
}

/// The timeline event member whose prefixes an occurrence invalidates.
const INVALIDATES_FACT_PREFIXES: &str = "invalidates_fact_prefixes";

/// UPD-1 fix round 5: the pointer of the first `invalidates_fact_prefixes` entry, in file order,
/// that makes a timeline event invalidate [`APPLICATION_UPDATE_REQUIRED_FACT_KEY`]: an entry the
/// key starts with (the key itself or any prefix of it, `health.` included), unless one of the
/// event's `excludes_fact_prefixes` keeps the key, as `TimelineEvent::invalidates_key` reads the
/// two. Entries that are no string are left to the compiler. `None` when no event invalidates it.
fn first_application_update_invalidation(schedule: &StandardPackSchedule) -> Option<String> {
    let covers = |prefix: &Value| {
        prefix
            .as_str()
            .is_some_and(|prefix| APPLICATION_UPDATE_REQUIRED_FACT_KEY.starts_with(prefix))
    };
    schedule
        .timeline
        .iter()
        .enumerate()
        .find_map(|(index, event)| {
            if prefix_list(event, EXCLUDES_FACT_PREFIXES)
                .iter()
                .any(covers)
            {
                return None;
            }
            prefix_list(event, INVALIDATES_FACT_PREFIXES)
                .iter()
                .position(covers)
                .map(|entry| format!("/timeline/{index}/{INVALIDATES_FACT_PREFIXES}/{entry}"))
        })
}

/// The entries of a timeline event's prefix list `member` as written, none when it writes none or
/// writes no list.
fn prefix_list<'a>(event: &'a ScheduleItem, member: &str) -> &'a [Value] {
    event
        .body
        .get(member)
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
}

/// The pointer of the first fact reference a rule's `trigger` or `feedback_stop` writes as the
/// `value` of a predicate node of `kind`, in file order.
fn first_rule_reference(schedule: &StandardPackSchedule, kind: &str) -> Option<String> {
    schedule.rules.iter().enumerate().find_map(|(index, rule)| {
        ["trigger", "feedback_stop"].into_iter().find_map(|member| {
            rule.body
                .get(member)
                .and_then(|node| fact_reference_in(node, &format!("/rules/{index}/{member}"), kind))
        })
    })
}

/// Whether `value` is a fact reference: an object whose `type` is `fact`.
fn is_fact_reference(value: Option<&Value>) -> bool {
    value
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str)
        == Some(FACT_REFERENCE_TYPE)
}

/// SS-5: one `fact` predicate of a `schedule.json` rule that writes `on_unknown`, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardPackFactProbe {
    /// The rule's index in `rules`.
    pub rule_index: usize,
    /// The rule's package, the one whose run must read the fact.
    pub package_id: String,
    /// The predicate's `fact_key`; `None` when it writes none or no string (the compiler refuses
    /// it).
    pub fact_key: Option<String>,
    /// The JSON pointer of the member `on_unknown` in the file.
    pub pointer: String,
}

/// SS-5: every `fact` predicate that writes `on_unknown` in a rule's `trigger` or
/// `feedback_stop`, in file order, through the children of `all`, `any` (`predicates`) and `not`
/// (`predicate`). The composer requires `fact_unknown_probe` for the first; the resource tools
/// check each key against the reading marks of the rule's package.
pub fn standard_pack_fact_probes(schedule: &StandardPackSchedule) -> Vec<StandardPackFactProbe> {
    fn walk(
        node: &Value,
        pointer: &str,
        rule_index: usize,
        package_id: &str,
        probes: &mut Vec<StandardPackFactProbe>,
    ) {
        let Value::Object(members) = node else {
            return;
        };
        if members.get("kind").and_then(Value::as_str) == Some("fact")
            && members.contains_key(FACT_ON_UNKNOWN)
        {
            probes.push(StandardPackFactProbe {
                rule_index,
                package_id: package_id.to_owned(),
                fact_key: members
                    .get("fact_key")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                pointer: format!("{pointer}/{FACT_ON_UNKNOWN}"),
            });
        }
        if let Some(Value::Array(children)) = members.get("predicates") {
            for (index, child) in children.iter().enumerate() {
                walk(
                    child,
                    &format!("{pointer}/predicates/{index}"),
                    rule_index,
                    package_id,
                    probes,
                );
            }
        }
        if let Some(child) = members.get("predicate") {
            walk(
                child,
                &format!("{pointer}/predicate"),
                rule_index,
                package_id,
                probes,
            );
        }
    }
    let mut probes = Vec::new();
    for (index, rule) in schedule.rules.iter().enumerate() {
        for member in ["trigger", "feedback_stop"] {
            if let Some(node) = rule.body.get(member) {
                walk(
                    node,
                    &format!("/rules/{index}/{member}"),
                    index,
                    &rule.package_id,
                    &mut probes,
                );
            }
        }
    }
    probes
}

/// SS-5: the `fact` predicate member of a probe.
const FACT_ON_UNKNOWN: &str = "on_unknown";

/// The pointer of the first fact reference at or below the predicate node `node` at `pointer`,
/// the `value` of a node of `kind`, through the children of `all`, `any` (`predicates`) and
/// `not` (`predicate`).
fn fact_reference_in(node: &Value, pointer: &str, kind: &str) -> Option<String> {
    let Value::Object(members) = node else {
        return None;
    };
    let reference = members.get("kind").and_then(Value::as_str) == Some(kind)
        && is_fact_reference(members.get("value"));
    if reference {
        return Some(format!("{pointer}/value"));
    }
    let in_children = match members.get("predicates") {
        Some(Value::Array(children)) => children.iter().enumerate().find_map(|(index, child)| {
            fact_reference_in(child, &format!("{pointer}/predicates/{index}"), kind)
        }),
        _ => None,
    };
    in_children.or_else(|| {
        members
            .get("predicate")
            .and_then(|child| fact_reference_in(child, &format!("{pointer}/predicate"), kind))
    })
}

/// Instantiates one game per bound instance (steps 3 and 4), adding each composed rule's
/// procedure to `manifest`; a package id another game's composed rules already use is
/// `rule_id_conflict` at the rule's `package_id`. G-R2s: each rule of `marked` carries
/// [`IRREVERSIBLE_REACH_MEMBER`] in every copy.
fn instantiate(
    game: &ReadGame<'_>,
    marked: &BTreeSet<&str>,
    composed: &mut Composed,
    manifest: &mut BTreeMap<String, ComposedProcedure>,
    issues: &mut Vec<StandardPackRuleIssue>,
) {
    let registry = &game.files.root.registry;
    let mut by_server = BTreeMap::<&str, usize>::new();
    for (_, server) in &game.bound {
        *by_server.entry(*server).or_default() += 1;
    }
    let total = game.bound.len();
    let rules = game
        .schedule
        .rules
        .iter()
        .enumerate()
        .filter(|(_, rule)| !game.schedule.holds(&rule.package_id))
        .collect::<Vec<_>>();
    let mut tasks = BTreeMap::new();
    for (_, rule) in &rules {
        let server = registry
            .get(&rule.package_id)
            .map(|entry| entry.server.as_str());
        let count = server.map_or(0, |server| on_server(&by_server, total, Some(server)));
        tasks.insert(rule.id.as_str(), count);
    }
    let copies = Copies {
        tasks,
        pools: item_copies(&game.schedule.pools, &by_server, total),
        profiles: item_copies(&game.schedule.activity, &by_server, total),
        events: item_copies(&game.schedule.timeline, &by_server, total),
    };
    let origin = |member, index| Origin {
        game_id: game.game_id.to_owned(),
        member,
        index,
    };
    let mut conflicting = BTreeSet::new();
    for (alias, server) in &game.bound {
        for (index, rule) in &rules {
            let Some(entry) = registry.get(&rule.package_id) else {
                continue;
            };
            if entry.server != *server {
                continue;
            }
            let procedure_ref = format!("{STANDARD_PACK_PROCEDURE_PREFIX}{}", rule.package_id);
            let known =
                manifest
                    .entry(procedure_ref.clone())
                    .or_insert_with(|| ComposedProcedure {
                        procedure_ref: procedure_ref.clone(),
                        game_id: game.game_id.to_owned(),
                        package_id: rule.package_id.clone(),
                        directory: entry.directory.clone(),
                    });
            if known.game_id != game.game_id {
                if conflicting.insert(*index) {
                    issues.push(StandardPackRuleIssue {
                        game_id: Some(game.game_id.to_owned()),
                        issue: schedule_issue(
                            StandardPackProblem::RuleIdConflict,
                            origin(ScheduleMember::Rules, *index).pointer("/package_id"),
                        ),
                        detail: format!(
                            "{procedure_ref} is already a package of the game {}",
                            known.game_id
                        ),
                    });
                }
                continue;
            }
            // SS-2b: a rule whose package is not maintenance-role is judged by its net benefit;
            // SS-2c: unless it declares itself maintenance.
            composed.tasks.push((
                compose_task(
                    rule,
                    alias,
                    &copies,
                    judged(rule, &game.classification),
                    marked.contains(rule.id.as_str()),
                ),
                origin(ScheduleMember::Rules, *index),
            ));
        }
        for member in [
            ScheduleMember::Pools,
            ScheduleMember::Activity,
            ScheduleMember::Timeline,
        ] {
            for (index, item) in game.schedule.items(member).iter().enumerate() {
                if item.server.as_deref().is_some_and(|only| only != *server) {
                    continue;
                }
                let value = compose_item(member, item, alias, &copies);
                composed.list(member).push((value, origin(member, index)));
            }
        }
    }
}

/// How many bound instances an item for `server` (every server when `None`) is copied to.
fn on_server(by_server: &BTreeMap<&str, usize>, total: usize, server: Option<&str>) -> usize {
    match server {
        Some(server) => by_server.get(server).copied().unwrap_or(0),
        None => total,
    }
}

/// The copies of each pool, profile or event of `items`, by file id.
fn item_copies<'a>(
    items: &'a [ScheduleItem],
    by_server: &BTreeMap<&str, usize>,
    total: usize,
) -> BTreeMap<&'a str, usize> {
    items
        .iter()
        .map(|item| {
            (
                item.id.as_str(),
                on_server(by_server, total, item.server.as_deref()),
            )
        })
        .collect()
}

/// `{kind: instance, instance_id: alias}`.
fn instance_scope(alias: &str) -> Value {
    json!({"kind": "instance", "instance_id": alias})
}

/// One rule's task for the instance `alias` (steps 3 and 4); SS-2b: with `judged`, marked
/// `benefit_judged: true`; G-R2s: with `irreversible`, marked `irreversible_reach: true`.
fn compose_task(
    rule: &ScheduleRule,
    alias: &str,
    copies: &Copies<'_>,
    judged: bool,
    irreversible: bool,
) -> Value {
    let mut task = Map::new();
    task.insert(
        "id".to_owned(),
        Value::String(copies.id(ScheduleMember::Rules, &rule.id, alias)),
    );
    task.insert("scope".to_owned(), instance_scope(alias));
    task.insert(
        "entrypoint".to_owned(),
        json!({"operation_id": STANDARD_PACK_OPERATION_ID}),
    );
    task.insert(
        "procedure_ref".to_owned(),
        Value::String(format!(
            "{STANDARD_PACK_PROCEDURE_PREFIX}{}",
            rule.package_id
        )),
    );
    for (name, value) in &rule.body {
        // SS-2h: the rule member `limited` is `schedule.json`'s; the task carries
        // `benefit_limited` instead (below).
        if name == SCHEDULE_RULE_LIMITED_MEMBER {
            continue;
        }
        let value = match name.as_str() {
            "trigger" | "feedback_stop" => predicate(value, alias, copies),
            "consumes" | "produces" | "reserves" => pool_references(value, alias, copies),
            _ => value.clone(),
        };
        task.insert(name.clone(), value);
    }
    task.entry("instance_overrides")
        .or_insert_with(|| Value::Array(Vec::new()));
    if judged {
        task.insert(BENEFIT_JUDGED_MEMBER.to_owned(), Value::Bool(true));
        // SS-2d: the model's realization mark rides on the judged task only (the reader and
        // `read_game` refuse it elsewhere).
        if rule.realization {
            task.insert(BENEFIT_REALIZATION_MEMBER.to_owned(), Value::Bool(true));
        }
        // SS-2h: so does the limited mark (`read_game` refuses it elsewhere).
        if rule.body.get(SCHEDULE_RULE_LIMITED_MEMBER) == Some(&Value::Bool(true)) {
            task.insert(BENEFIT_LIMITED_MEMBER.to_owned(), Value::Bool(true));
        }
        // SCHED-C1a: and the fixed value, as written, for the compiler to check (the reader and
        // `read_game` refuse it elsewhere); its origin stays in the file.
        if let Some(value) = &rule.fixed_value_milli {
            task.insert(BENEFIT_FIXED_VALUE_MEMBER.to_owned(), value.clone());
        }
    }
    // G-R2s: absent when false, so a task the mark does not reach keeps its bytes.
    if irreversible {
        task.insert(IRREVERSIBLE_REACH_MEMBER.to_owned(), Value::Bool(true));
    }
    if copies
        .tasks
        .get(rule.id.as_str())
        .is_some_and(|count| *count > 1)
        && let Value::Array(continued) = task
            .entry("continues_from")
            .or_insert_with(|| Value::Array(Vec::new()))
    {
        continued.push(Value::String(rule.id.clone()));
    }
    Value::Object(task)
}

/// One pool, profile or event for the instance `alias` (steps 3 and 4).
fn compose_item(
    member: ScheduleMember,
    item: &ScheduleItem,
    alias: &str,
    copies: &Copies<'_>,
) -> Value {
    let mut composed = Map::new();
    composed.insert(
        "id".to_owned(),
        Value::String(copies.id(member, &item.id, alias)),
    );
    composed.insert("scope".to_owned(), instance_scope(alias));
    for (name, value) in &item.body {
        let value = match (member, name.as_str()) {
            (ScheduleMember::Pools, "observation") => {
                rename(value, "task_id", ScheduleMember::Rules, alias, copies)
            }
            (ScheduleMember::Activity, "goals") => match value {
                Value::Array(goals) => Value::Array(
                    goals
                        .iter()
                        .map(|goal| goal_metric(goal, alias, copies))
                        .collect(),
                ),
                other => other.clone(),
            },
            _ => value.clone(),
        };
        composed.insert(name.clone(), value);
    }
    Value::Object(composed)
}

/// A goal with its metric's pool or task reference renamed for the copy.
fn goal_metric(goal: &Value, alias: &str, copies: &Copies<'_>) -> Value {
    let Value::Object(members) = goal else {
        return goal.clone();
    };
    let mut members = members.clone();
    if let Some(metric) = members.get_mut("metric") {
        let renamed = rename(metric, "pool_id", ScheduleMember::Pools, alias, copies);
        *metric = rename(&renamed, "task_id", ScheduleMember::Rules, alias, copies);
    }
    Value::Object(members)
}

/// Each effect or reserve of a list with its `pool_id` renamed for the copy.
fn pool_references(value: &Value, alias: &str, copies: &Copies<'_>) -> Value {
    match value {
        Value::Array(entries) => Value::Array(
            entries
                .iter()
                .map(|entry| rename(entry, "pool_id", ScheduleMember::Pools, alias, copies))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// `value` with its string member `name`, an id of `member`, renamed for the copy.
fn rename(
    value: &Value,
    name: &str,
    member: ScheduleMember,
    alias: &str,
    copies: &Copies<'_>,
) -> Value {
    let Value::Object(members) = value else {
        return value.clone();
    };
    let mut members = members.clone();
    if let Some(Value::String(id)) = members.get(name) {
        let renamed = copies.id(member, id, alias);
        members.insert(name.to_owned(), Value::String(renamed));
    }
    Value::Object(members)
}

/// A rule's predicate for the copy. The V2 predicate nodes that read a fact (`fact`,
/// `record_deadline`, `fact_time_reached`) are exactly those with a `fact_key`: each gets the
/// copy's scope. A node's `event_id` (`timeline_active`), `pool_id` (`resource_projection`) and
/// `task_id` (`dependency_completed`, `outcome`) are renamed for the copy, and the children of
/// `all`, `any` (`predicates`) and `not` (`predicate`) are composed alike.
fn predicate(value: &Value, alias: &str, copies: &Copies<'_>) -> Value {
    let Value::Object(node) = value else {
        return value.clone();
    };
    let mut node = node.clone();
    if let Some(Value::Array(children)) = node.get_mut("predicates") {
        for child in children.iter_mut() {
            *child = predicate(child, alias, copies);
        }
    }
    if let Some(child) = node.get_mut("predicate") {
        *child = predicate(child, alias, copies);
    }
    if node.contains_key("fact_key") {
        node.insert("scope".to_owned(), instance_scope(alias));
    }
    let node = rename(
        &Value::Object(node),
        "event_id",
        ScheduleMember::Timeline,
        alias,
        copies,
    );
    let node = rename(&node, "pool_id", ScheduleMember::Pools, alias, copies);
    rename(&node, "task_id", ScheduleMember::Rules, alias, copies)
}

/// Two composed rules, pools, profiles or events with one id: `rule_id_conflict` at the later
/// one's id (games in name order).
fn id_conflicts(composed: &Composed, issues: &mut Vec<StandardPackRuleIssue>) {
    for list in [
        &composed.tasks,
        &composed.pools,
        &composed.profiles,
        &composed.events,
    ] {
        let mut seen = BTreeMap::<&str, &Origin>::new();
        for (value, origin) in list {
            let Some(id) = value.get("id").and_then(Value::as_str) else {
                continue;
            };
            match seen.get(id) {
                None => {
                    seen.insert(id, origin);
                }
                Some(first) => issues.push(StandardPackRuleIssue {
                    game_id: Some(origin.game_id.clone()),
                    issue: schedule_issue(
                        StandardPackProblem::RuleIdConflict,
                        origin.pointer("/id"),
                    ),
                    detail: format!(
                        "the id {id} is already composed from the game {} at {}",
                        first.game_id,
                        first.pointer("/id")
                    ),
                }),
            }
        }
    }
}

/// The four V2 documents of the composition (step 5), descriptor `standard-packs` at
/// `catalog_version` with no approval.
fn documents(
    composed: &Composed,
    catalog_version: u64,
) -> Result<CatalogSources, StandardPackRuleIssue> {
    let descriptor = json!({
        "catalog_id": STANDARD_PACKS_CATALOG_ID,
        "catalog_version": catalog_version,
        "approval_refs": []
    });
    let document = |file: &str, list: &str, items: &[(Value, Origin)]| {
        let mut document = Map::new();
        document.insert(
            "schema_version".to_owned(),
            Value::String(SCHEDULING_SCHEMA_VERSION_V2.to_owned()),
        );
        document.insert("catalog".to_owned(), descriptor.clone());
        document.insert(
            list.to_owned(),
            Value::Array(items.iter().map(|(value, _)| value.clone()).collect()),
        );
        serde_json::to_vec_pretty(&Value::Object(document))
            .map(|bytes| {
                CatalogDocumentSource::new(format!("{STANDARD_PACKS_CATALOG_ID}://{file}"), bytes)
            })
            .map_err(|error| composition_issue(format!("{file} does not serialize: {error}")))
    };
    Ok(CatalogSources {
        tasks: document("tasks.json", "tasks", &composed.tasks)?,
        pools: document("pools.json", "pools", &composed.pools)?,
        activity: document("activity.json", "profiles", &composed.profiles)?,
        timeline: document("timeline.json", "events", &composed.events)?,
        selection: None,
    })
}

/// The rules digest (model §3.2 step 5) of the compiled elements and the manifest.
fn rules_digest(
    compiled: &CompiledCatalog,
    manifest: &[ComposedProcedure],
) -> Result<String, StandardPackRuleIssue> {
    #[derive(Serialize)]
    struct Digested<'a> {
        activity: &'a [ActivityProfile],
        manifest: &'a [ComposedProcedure],
        pools: &'a [PoolSpec],
        tasks: &'a [TaskSpec],
        timeline: &'a [TimelineEvent],
    }
    let catalog = compiled.catalog();
    let bytes = canonical_serialized(&Digested {
        activity: &catalog.activity.profiles,
        manifest,
        pools: &catalog.pools.pools,
        tasks: &catalog.tasks.tasks,
        timeline: &catalog.timeline.events,
    })
    .map_err(|error| composition_issue(format!("the rules digest does not serialize: {error}")))?;
    Ok(sha256(&bytes))
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn listed(set: &BTreeSet<String>) -> Vec<String> {
    set.iter().cloned().collect()
}

fn schedule_issue(problem: StandardPackProblem, pointer: String) -> StandardPackRootIssue {
    StandardPackRootIssue {
        file: STANDARD_PACK_SCHEDULE_FILE,
        problem,
        pointer,
    }
}

fn composition_issue(detail: String) -> StandardPackRuleIssue {
    StandardPackRuleIssue {
        game_id: None,
        issue: schedule_issue(StandardPackProblem::ScheduleInvalid, String::new()),
        detail,
    }
}
