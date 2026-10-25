// SPDX-License-Identifier: AGPL-3.0-only

//! ROUT-L RL-1a (frozen model v1.0 §3.1; `contracts/standard-pack-schedule.md`): a game root's
//! `schedule.json` (`actingcommand.schedule.v1`), the schedule rules a standard package carries
//! beside `bundle.json`. It holds the game's V2 tasks ("rules"), pools, activity profiles and
//! timeline events, and names no instance, no scope, no package path and no approval: a rule
//! names its `package_id`, and the game root's registry says where that package is and which
//! server it belongs to.
//!
//! This module reads the file's envelope and its own rules (no `scope` member anywhere, no
//! instance override, no entrypoint or procedure reference on a rule, ids unique in the file),
//! checks it against the game root's registry, and classifies the registry's packages. The V2
//! members of each rule and item stay opaque here: the policy crate's composer
//! (`actingcommand_policy::compose_standard_pack_rules`) places them per bound instance and the
//! catalog compiler checks them. The Runtime, check-config, `package bundle` and `actinglab
//! standard-pack check|compose` all read the file through this module, so no reader accepts what
//! another refuses. Every refusal is a [`StandardPackRootIssue`] of [`STANDARD_PACK_SCHEDULE_FILE`]
//! with its `standard_pack_problem` and JSON pointer. The Runtime names no game here.

use crate::BundleMaintenance;
use crate::package::bundle_identifier;
use crate::standard_pack::{
    InterruptThen, StandardPackProblem, StandardPackRegistry, StandardPackRoot,
    StandardPackRootIssue,
};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// The game root's schedule rules (§3.1).
pub const STANDARD_PACK_SCHEDULE_FILE: &str = "schedule.json";
/// The one version of `schedule.json` this build reads.
pub const STANDARD_PACK_SCHEDULE_VERSION: &str = "actingcommand.schedule.v1";
/// The largest `schedule.json`: one scheduling catalog document's bound (§3.1, "Size").
pub const MAX_STANDARD_PACK_SCHEDULE_BYTES: usize = 1_048_576;
/// The most subsystems one `requires` names.
pub const MAX_SCHEDULE_REQUIRES: usize = 32;
/// The longest subsystem name `requires` holds.
pub const MAX_SCHEDULE_REQUIREMENT_BYTES: usize = 64;
/// The most `held` entries.
pub const MAX_SCHEDULE_HELD: usize = 4_096;
/// The longest package id a rule or a `held` entry names.
pub const MAX_SCHEDULE_PACKAGE_ID_BYTES: usize = 128;
/// The longest `held` reason or reference.
pub const MAX_SCHEDULE_TEXT_BYTES: usize = 1_024;

/// The members of `schedule.json`.
const DOCUMENT_MEMBERS: [&str; 7] = [
    "schema_version",
    "requires",
    "rules",
    "pools",
    "activity",
    "timeline",
    "held",
];
/// The members of one `held` entry.
const HELD_MEMBERS: [&str; 3] = ["package_id", "reason", "reference"];

/// The four lists of rules and items, by their member name in `schedule.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ScheduleMember {
    Rules,
    Pools,
    Activity,
    Timeline,
}

impl ScheduleMember {
    pub const ALL: [Self; 4] = [Self::Rules, Self::Pools, Self::Activity, Self::Timeline];

    /// The member's name in `schedule.json`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rules => "rules",
            Self::Pools => "pools",
            Self::Activity => "activity",
            Self::Timeline => "timeline",
        }
    }
}

/// One rule: a V2 task without `scope`, `entrypoint` and `procedure_ref`, and the package it
/// runs.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleRule {
    pub id: String,
    pub package_id: String,
    /// Every other member as written, in file order (without `id` and `package_id`).
    pub body: Map<String, Value>,
}

/// One pool, activity profile or timeline event: a V2 item without `scope`, for every server of
/// the game, or for `server` alone.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleItem {
    pub id: String,
    pub server: Option<String>,
    /// Every other member as written, in file order (without `id` and `server`).
    pub body: Map<String, Value>,
}

/// A package kept from scheduling on purpose: the entry's `reason` (text for people) and an
/// optional board `reference`. Nothing evaluates either by machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleHeld {
    pub package_id: String,
    /// The entry's `reason` member.
    pub rationale: String,
    pub reference: Option<String>,
}

/// `schedule.json`, read by [`parse_standard_pack_schedule`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StandardPackSchedule {
    /// The configuration manifest subsystems these rules need (default none).
    pub requires: Vec<String>,
    pub rules: Vec<ScheduleRule>,
    pub pools: Vec<ScheduleItem>,
    pub activity: Vec<ScheduleItem>,
    pub timeline: Vec<ScheduleItem>,
    pub held: Vec<ScheduleHeld>,
}

impl StandardPackSchedule {
    /// The items of one of the lists `pools`, `activity` and `timeline` (none for `rules`).
    pub fn items(&self, member: ScheduleMember) -> &[ScheduleItem] {
        match member {
            ScheduleMember::Rules => &[],
            ScheduleMember::Pools => &self.pools,
            ScheduleMember::Activity => &self.activity,
            ScheduleMember::Timeline => &self.timeline,
        }
    }

    /// Whether `package_id` is held.
    pub fn holds(&self, package_id: &str) -> bool {
        self.held.iter().any(|held| held.package_id == package_id)
    }
}

/// How the packages of one game root's registry are classified (§3.1, R6, R7). A package may be
/// maintenance-role and scheduled at once; `unclassified` holds the packages in none of the other
/// three sets, which are not scheduled (`standard_pack_package_unclassified`, a warning).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StandardPackClassification {
    /// Maintenance-role by the root files ([`standard_pack_maintenance_packages`]).
    pub maintenance: BTreeSet<String>,
    /// Named by a rule and not held.
    pub scheduled: BTreeSet<String>,
    /// Held, with a reason.
    pub held: BTreeSet<String>,
    pub unclassified: BTreeSet<String>,
}

fn issue(problem: StandardPackProblem, pointer: impl Into<String>) -> StandardPackRootIssue {
    StandardPackRootIssue {
        file: STANDARD_PACK_SCHEDULE_FILE,
        problem,
        pointer: pointer.into(),
    }
}

fn invalid(pointer: impl Into<String>) -> StandardPackRootIssue {
    issue(StandardPackProblem::ScheduleInvalid, pointer)
}

/// One JSON pointer segment (RFC 6901).
fn segment(name: &str) -> String {
    name.replace('~', "~0").replace('/', "~1")
}

/// Reads `schedule.json`'s bytes on their own (§3.1): at most 1 MiB of UTF-8 JSON without a
/// duplicate key, `schema_version` `actingcommand.schedule.v1`, only the members `requires`,
/// `rules`, `pools`, `activity`, `timeline` and `held` besides it (each absent means empty), and:
///
/// - no member named `scope` at any depth of a rule or an item (no instance names);
/// - every rule and item is an object with a string `id`, unique within its list
///   (`rule_id_conflict` at the second one);
/// - every rule names a string `package_id` (1–128 bytes, no control character), and holds no
///   `entrypoint` and no `procedure_ref` (the composer fills both), and its `instance_overrides`
///   is absent or empty;
/// - a pool, profile or event names its `server`, if at all, as a game-root identifier;
/// - `requires` lists at most 32 distinct subsystem names of 1–64 bytes of `[a-z][a-z0-9_]*`;
/// - `held` lists at most 4096 entries `{package_id, reason, reference?}` with distinct package
///   ids and a non-empty `reason`.
///
/// Anything else is `schedule_invalid` at its pointer (empty for the whole document).
pub fn parse_standard_pack_schedule(
    bytes: &[u8],
) -> Result<StandardPackSchedule, StandardPackRootIssue> {
    if bytes.len() > MAX_STANDARD_PACK_SCHEDULE_BYTES {
        return Err(invalid(""));
    }
    let Value::Object(document) = strict_json(bytes).map_err(|_| invalid(""))? else {
        return Err(invalid(""));
    };
    if let Some(unknown) = document
        .keys()
        .find(|key| !DOCUMENT_MEMBERS.contains(&key.as_str()))
    {
        return Err(invalid(format!("/{}", segment(unknown))));
    }
    if document.get("schema_version").and_then(Value::as_str)
        != Some(STANDARD_PACK_SCHEDULE_VERSION)
    {
        return Err(invalid("/schema_version"));
    }
    let list = |name: &str| -> Result<Vec<Value>, StandardPackRootIssue> {
        match document.get(name) {
            None => Ok(Vec::new()),
            Some(Value::Array(values)) => Ok(values.clone()),
            Some(_) => Err(invalid(format!("/{name}"))),
        }
    };
    let requires = parse_requires(list("requires")?)?;
    let mut rules = Vec::new();
    let mut rule_ids = BTreeSet::new();
    for (index, value) in list("rules")?.into_iter().enumerate() {
        let pointer = format!("/rules/{index}");
        let (id, mut body) = item_object(value, &pointer, &mut rule_ids)?;
        let package_id = match body.shift_remove("package_id") {
            Some(Value::String(package_id)) if package_id_text(&package_id) => package_id,
            _ => return Err(invalid(format!("{pointer}/package_id"))),
        };
        for filled in ["entrypoint", "procedure_ref"] {
            if body.contains_key(filled) {
                return Err(invalid(format!("{pointer}/{filled}")));
            }
        }
        match body.get("instance_overrides") {
            None => {}
            Some(Value::Array(overrides)) if overrides.is_empty() => {}
            Some(_) => return Err(invalid(format!("{pointer}/instance_overrides"))),
        }
        rules.push(ScheduleRule {
            id,
            package_id,
            body,
        });
    }
    let mut lists = BTreeMap::new();
    for member in [
        ScheduleMember::Pools,
        ScheduleMember::Activity,
        ScheduleMember::Timeline,
    ] {
        let mut ids = BTreeSet::new();
        let mut items = Vec::new();
        for (index, value) in list(member.as_str())?.into_iter().enumerate() {
            let pointer = format!("/{}/{index}", member.as_str());
            let (id, mut body) = item_object(value, &pointer, &mut ids)?;
            let server = match body.shift_remove("server") {
                None => None,
                Some(Value::String(server)) if bundle_identifier(&server) && !server.is_empty() => {
                    Some(server)
                }
                Some(_) => return Err(invalid(format!("{pointer}/server"))),
            };
            items.push(ScheduleItem { id, server, body });
        }
        lists.insert(member, items);
    }
    let held = parse_held(list("held")?)?;
    let mut take = |member: ScheduleMember| lists.remove(&member).unwrap_or_default();
    Ok(StandardPackSchedule {
        requires,
        rules,
        pools: take(ScheduleMember::Pools),
        activity: take(ScheduleMember::Activity),
        timeline: take(ScheduleMember::Timeline),
        held,
    })
}

/// One rule or item: an object with no `scope` member at any depth and a string `id` not yet in
/// `ids` (`rule_id_conflict` otherwise); its members without `id`.
fn item_object(
    value: Value,
    pointer: &str,
    ids: &mut BTreeSet<String>,
) -> Result<(String, Map<String, Value>), StandardPackRootIssue> {
    if let Some(scope) = scope_member(&value, pointer) {
        return Err(invalid(scope));
    }
    let Value::Object(mut body) = value else {
        return Err(invalid(pointer));
    };
    let Some(Value::String(id)) = body.shift_remove("id") else {
        return Err(invalid(format!("{pointer}/id")));
    };
    if !ids.insert(id.clone()) {
        return Err(issue(
            StandardPackProblem::RuleIdConflict,
            format!("{pointer}/id"),
        ));
    }
    Ok((id, body))
}

/// The pointer of the first member named `scope` at any depth of `value`, in document order.
fn scope_member(value: &Value, pointer: &str) -> Option<String> {
    match value {
        Value::Object(members) => members.iter().find_map(|(name, child)| {
            let child_pointer = format!("{pointer}/{}", segment(name));
            if name == "scope" {
                Some(child_pointer)
            } else {
                scope_member(child, &child_pointer)
            }
        }),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .find_map(|(index, child)| scope_member(child, &format!("{pointer}/{index}"))),
        _ => None,
    }
}

fn parse_requires(values: Vec<Value>) -> Result<Vec<String>, StandardPackRootIssue> {
    if values.len() > MAX_SCHEDULE_REQUIRES {
        return Err(invalid("/requires"));
    }
    let mut requires = Vec::new();
    for (index, value) in values.into_iter().enumerate() {
        match value {
            Value::String(name) if subsystem_name(&name) && !requires.contains(&name) => {
                requires.push(name);
            }
            _ => return Err(invalid(format!("/requires/{index}"))),
        }
    }
    Ok(requires)
}

fn parse_held(values: Vec<Value>) -> Result<Vec<ScheduleHeld>, StandardPackRootIssue> {
    if values.len() > MAX_SCHEDULE_HELD {
        return Err(invalid("/held"));
    }
    let mut held = Vec::new();
    let mut packages = BTreeSet::new();
    for (index, value) in values.into_iter().enumerate() {
        let pointer = format!("/held/{index}");
        let Value::Object(entry) = value else {
            return Err(invalid(pointer));
        };
        if let Some(unknown) = entry
            .keys()
            .find(|key| !HELD_MEMBERS.contains(&key.as_str()))
        {
            return Err(invalid(format!("{pointer}/{}", segment(unknown))));
        }
        let text = |name: &str, required: bool| -> Result<Option<String>, StandardPackRootIssue> {
            match entry.get(name) {
                None if !required => Ok(None),
                Some(Value::String(text)) if bounded_text(text) => Ok(Some(text.clone())),
                _ => Err(invalid(format!("{pointer}/{name}"))),
            }
        };
        let package_id = match entry.get("package_id") {
            Some(Value::String(package_id)) if package_id_text(package_id) => package_id.clone(),
            _ => return Err(invalid(format!("{pointer}/package_id"))),
        };
        if !packages.insert(package_id.clone()) {
            return Err(invalid(format!("{pointer}/package_id")));
        }
        let rationale = text("reason", true)?.unwrap_or_default();
        let reference = text("reference", false)?;
        held.push(ScheduleHeld {
            package_id,
            rationale,
            reference,
        });
    }
    Ok(held)
}

/// A configuration manifest subsystem name: 1–64 bytes of `[a-z][a-z0-9_]*`.
fn subsystem_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= MAX_SCHEDULE_REQUIREMENT_BYTES
        && bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn package_id_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_SCHEDULE_PACKAGE_ID_BYTES
        && !value.chars().any(char::is_control)
}

fn bounded_text(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAX_SCHEDULE_TEXT_BYTES
        && !value.chars().any(char::is_control)
}

/// The references `schedule` makes into the game root's `registry` (§3.1): every rule's and every
/// `held` entry's package is a package of the registry (`rule_package_unregistered` at its
/// `package_id`), and every `server` a pool, profile or event names is a server of a registry
/// package (`schedule_invalid` at its `server`).
pub fn validate_standard_pack_schedule_references(
    registry: &StandardPackRegistry,
    schedule: &StandardPackSchedule,
) -> Result<(), StandardPackRootIssue> {
    let unregistered =
        |pointer: String| issue(StandardPackProblem::RulePackageUnregistered, pointer);
    for (index, rule) in schedule.rules.iter().enumerate() {
        if registry.get(&rule.package_id).is_none() {
            return Err(unregistered(format!("/rules/{index}/package_id")));
        }
    }
    for (index, held) in schedule.held.iter().enumerate() {
        if registry.get(&held.package_id).is_none() {
            return Err(unregistered(format!("/held/{index}/package_id")));
        }
    }
    let servers = registry
        .listed()
        .into_iter()
        .map(|(_, server)| server)
        .collect::<BTreeSet<_>>();
    for member in [
        ScheduleMember::Pools,
        ScheduleMember::Activity,
        ScheduleMember::Timeline,
    ] {
        for (index, item) in schedule.items(member).iter().enumerate() {
            if item
                .server
                .as_ref()
                .is_some_and(|server| !servers.contains(server))
            {
                return Err(invalid(format!("/{}/{index}/server", member.as_str())));
            }
        }
    }
    Ok(())
}

/// The maintenance-role packages of one game root (§3.1 (a)): the bundle's `maintenance[]`
/// entries (startup, return home, recognition), every `navigation.json` edge and recovery chain
/// package, every `interrupts.json` handler and `then.run` package, and the `pages.json` feature
/// packages.
pub fn standard_pack_maintenance_packages(
    maintenance: &[BundleMaintenance],
    root: &StandardPackRoot,
) -> BTreeSet<String> {
    let mut packages = maintenance
        .iter()
        .map(|entry| entry.package_id.clone())
        .collect::<BTreeSet<_>>();
    if let Some(pages) = &root.pages {
        packages.extend(pages.features.iter().cloned());
    }
    if let Some(navigation) = &root.navigation {
        packages.extend(navigation.edges.iter().map(|edge| edge.package_id.clone()));
        packages.extend(navigation.recovery_chain.iter().cloned());
    }
    if let Some(interrupts) = &root.interrupts {
        for interrupt in &interrupts.interrupts {
            packages.extend(interrupt.handlers.iter().cloned());
            if let InterruptThen::Run(run) = &interrupt.then {
                packages.extend(run.iter().cloned());
            }
        }
    }
    packages
}

/// Classifies every package of `registry` (§3.1, R6, R7): maintenance-role (`maintenance`, from
/// [`standard_pack_maintenance_packages`]), held, named by a rule that is not held (scheduled), or
/// none of these (unclassified, not scheduled).
pub fn classify_standard_pack_packages(
    registry: &StandardPackRegistry,
    maintenance: &BTreeSet<String>,
    schedule: &StandardPackSchedule,
) -> StandardPackClassification {
    let mut classification = StandardPackClassification::default();
    for package_id in registry.package_ids() {
        let held = schedule.holds(package_id);
        let named = schedule
            .rules
            .iter()
            .any(|rule| rule.package_id == package_id);
        let maintained = maintenance.contains(package_id);
        if maintained {
            classification.maintenance.insert(package_id.to_owned());
        }
        if held {
            classification.held.insert(package_id.to_owned());
        } else if named {
            classification.scheduled.insert(package_id.to_owned());
        }
        if !maintained && !held && !named {
            classification.unclassified.insert(package_id.to_owned());
        }
    }
    classification
}

/// The document as a JSON value, refusing a duplicate object key at any depth and trailing data.
fn strict_json(bytes: &[u8]) -> Result<Value, serde_json::Error> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = StrictValue.deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(value)
}

/// A JSON value whose objects hold each key once.
struct StrictValue;

impl<'de> DeserializeSeed<'de> for StrictValue {
    type Value = Value;

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for StrictValue {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("a number that is not finite"))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(StrictValue)? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut members = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if members.contains_key(&key) {
                return Err(de::Error::custom(format!("duplicate object key `{key}`")));
            }
            let value = map.next_value_seed(StrictValue)?;
            members.insert(key, value);
        }
        Ok(Value::Object(members))
    }
}
