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
//!    against the subsystems given (when given), its packages checked against the registry, and
//!    the registry's packages classified. A refused game is reported with its game id.
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
//! compiler diagnostic is mapped back to the rule or item it came from.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    BundleMaintenance, STANDARD_PACK_SCHEDULE_FILE, ScheduleItem, ScheduleMember, ScheduleRule,
    StandardPackClassification, StandardPackProblem, StandardPackRoot, StandardPackRootIssue,
    StandardPackSchedule, classify_standard_pack_packages, parse_standard_pack_schedule,
    standard_pack_maintenance_packages, validate_standard_pack_schedule_references,
};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::canonical::canonical_serialized;
use crate::{
    ActivityProfile, CatalogDocumentSource, CatalogSources, CompiledCatalog, PoolSpec,
    SCHEDULING_SCHEMA_VERSION_V2, SchedulingDocumentKind, TaskSpec, TimelineEvent,
    compile_composed_catalog,
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

/// A composed catalog of the standard packs' rules.
#[derive(Debug, Clone)]
pub struct StandardPackComposition {
    sources: CatalogSources,
    compiled: CompiledCatalog,
    manifest: Vec<ComposedProcedure>,
    games: Vec<ComposedGame>,
    rules_digest: String,
}

impl StandardPackComposition {
    /// The four V2 documents, as compiled.
    pub fn sources(&self) -> &CatalogSources {
        &self.sources
    }

    pub fn compiled(&self) -> &CompiledCatalog {
        &self.compiled
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
    /// use (keys `game`, `package_id`).
    pub fn warnings(&self) -> Vec<Outcome> {
        self.games
            .iter()
            .flat_map(|game| {
                game.unclassified.iter().map(|package_id| {
                    Outcome::new(SharedCode::StandardPackPackageUnclassified)
                        .game(game.game_id.as_str())
                        .package_id(package_id.as_str())
                })
            })
            .collect()
    }
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
    for game in &read {
        let before = composed.tasks.len();
        instantiate(game, &mut composed, &mut manifest, &mut issues);
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
    let manifest = manifest.into_values().collect::<Vec<_>>();
    let rules_digest =
        rules_digest(&compiled, &manifest).map_err(|issue| StandardPackComposeFailure {
            issues: vec![issue],
        })?;
    Ok(StandardPackComposition {
        sources,
        compiled,
        manifest,
        games,
        rules_digest,
    })
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
    Ok((schedule, classification))
}

/// Instantiates one game per bound instance (steps 3 and 4), adding each composed rule's
/// procedure to `manifest`; a package id another game's composed rules already use is
/// `rule_id_conflict` at the rule's `package_id`.
fn instantiate(
    game: &ReadGame<'_>,
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
            composed.tasks.push((
                compose_task(rule, alias, &copies),
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

/// One rule's task for the instance `alias` (steps 3 and 4).
fn compose_task(rule: &ScheduleRule, alias: &str, copies: &Copies<'_>) -> Value {
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
        let value = match name.as_str() {
            "trigger" | "feedback_stop" => predicate(value, alias, copies),
            "consumes" | "produces" | "reserves" => pool_references(value, alias, copies),
            _ => value.clone(),
        };
        task.insert(name.clone(), value);
    }
    task.entry("instance_overrides")
        .or_insert_with(|| Value::Array(Vec::new()));
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
