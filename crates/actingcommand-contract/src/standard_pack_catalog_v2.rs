// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #477 CV-1 (register §1.507; `contracts/pack-catalog.md`, "Version 2"): the business
//! part of `pack-catalog.json` of version 2 (`actingcommand.pack-catalog.v2`). An entry may carry
//! its own business part, `consumes` and `produces` (each a list of V2 resource effects, the
//! contract crate's [`ResourceEffectSpec`], written as a `schedule.json` rule writes its effects
//! and held to the same bounds, [`ResourceEffectSpec::form_fault`]), a fixed value `value` and the
//! marks `limited` and `realization`; and, beside it, per-rule business parts `rules`, each keyed
//! by a rule's id (`rule_id`), which stand for that rule in place of the entry's own part, whole.
//! The reader's checks ([`Check::parts`]), the business part the rules give
//! ([`rules_parts`], the writer's `--refresh`), the differences from the rules
//! ([`part_differences`]) and the values file (`--values`, [`read_values_file`]) are here; the
//! envelope, the display part and the merge are the parent's, one rule for both versions. No
//! Runtime component reads the file.

use super::{
    CatalogBusiness, CatalogDifference, CatalogDisplayFileIssue, CatalogPool,
    CatalogScheduleDifference, CatalogSide, CatalogVersion, Check, MAX_PACK_CATALOG_RULE_PARTS,
    MAX_PACK_CATALOG_VALUE_ITEMS, MAX_STANDARD_PACK_CATALOG_BYTES,
    STANDARD_PACK_CATALOG_VALUES_SCHEMA_VERSION, at, read_document, standard_pack_schedule_pools,
};
use crate::resource_effect::{
    EffectDirection, MAX_EFFECT_QUANTITY, MAX_EFFECTS_PER_TASK, MAX_SCHEDULING_IDENTIFIER_BYTES,
    ResourceEffectSpec, scheduling_identifier,
};
use crate::standard_pack::{StandardPackProblem, StandardPackRegistry, StandardPackRootIssue};
use crate::standard_pack_schedule::{
    SCHEDULE_RULE_FIXED_VALUE_MEMBER, SCHEDULE_VALUE_ORIGIN_MODEL, STANDARD_PACK_SCHEDULE_FILE,
    ScheduleRule, ScheduleValueOrigin, StandardPackSchedule, parse_standard_pack_schedule,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The members of a version 2 entry: version 1's, and the business part's value, marks and
/// per-rule parts.
pub(super) const ENTRY_MEMBERS_V2: [&str; 11] = [
    "package_id",
    "title",
    "summary",
    "group",
    "risk",
    "consumes",
    "produces",
    "value",
    "limited",
    "realization",
    "rules",
];
/// The members of one business part, in the order the writer writes them.
const PART_MEMBERS: [&str; 5] = ["consumes", "produces", "value", "limited", "realization"];
/// The members of an entry the business part holds: its own part's and `rules`.
const BUSINESS_MEMBERS: [&str; 6] = [
    "consumes",
    "produces",
    "value",
    "limited",
    "realization",
    "rules",
];
/// The members of one per-rule business part.
const RULE_PART_MEMBERS: [&str; 6] = [
    "rule_id",
    "consumes",
    "produces",
    "value",
    "limited",
    "realization",
];
/// The value and the two marks, which the values file replaces whole.
const MARK_MEMBERS: [&str; 3] = ["value", "limited", "realization"];
/// The members of a fixed value.
const VALUE_MEMBERS: [&str; 2] = ["milli", "origin"];
/// The limited mark, the member of an entry and of a `schedule.json` rule.
const LIMITED: &str = "limited";
/// The members of the values file's document, and of one of its items.
const VALUES_FILE_MEMBERS: [&str; 2] = ["schema_version", "packs"];
const VALUES_ITEM_MEMBERS: [&str; 5] = ["package_id", "rule_id", "value", "limited", "realization"];

/// CV-1: the fixed value of one run of a package, `value` `{milli, origin?}`: in milli of its
/// game's one value unit, 1 to [`MAX_EFFECT_QUANTITY`], with the meaning of a rule's
/// `fixed_value_milli`; `origin` `"model"` when the model's export wrote it (a rule's
/// `fixed_value_origin`), else absent, the pack's own hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogValue {
    pub milli: u64,
    pub origin: ScheduleValueOrigin,
}

impl CatalogValue {
    /// The value as the file writes it.
    fn document(self) -> Value {
        let mut value = Map::new();
        value.insert("milli".to_owned(), Value::from(self.milli));
        if self.origin == ScheduleValueOrigin::Model {
            value.insert(
                "origin".to_owned(),
                Value::String(SCHEDULE_VALUE_ORIGIN_MODEL.to_owned()),
            );
        }
        Value::Object(value)
    }
}

/// CV-1: one business part of version 2, an entry's own or one rule's: what one run uses up and
/// brings in, each effect a rule's, its fixed value and the marks `limited` and `realization`
/// (each `true` or absent), with the meaning a rule's members of those names have.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogPart {
    pub consumes: Vec<ResourceEffectSpec>,
    pub produces: Vec<ResourceEffectSpec>,
    pub value: Option<CatalogValue>,
    pub limited: bool,
    pub realization: bool,
}

impl CatalogPart {
    /// The effects of `side`.
    pub fn side(&self, side: CatalogSide) -> &[ResourceEffectSpec] {
        match side {
            CatalogSide::Consumes => &self.consumes,
            CatalogSide::Produces => &self.produces,
        }
    }

    /// The part's members as the file writes them, in order: `consumes`, `produces`, then
    /// `value`, `limited` and `realization` where it has them.
    pub fn members(&self) -> Map<String, Value> {
        let mut members = Map::new();
        for side in CatalogSide::ALL.iter().copied() {
            members.insert(
                side.as_str().to_owned(),
                Value::Array(self.side(side).iter().map(|effect| json!(effect)).collect()),
            );
        }
        if let Some(value) = self.value {
            members.insert("value".to_owned(), value.document());
        }
        if self.limited {
            members.insert(LIMITED.to_owned(), Value::Bool(true));
        }
        if self.realization {
            members.insert("realization".to_owned(), Value::Bool(true));
        }
        members
    }
}

/// CV-1: the business part of one rule, which stands for it in place of the entry's own part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogRulePart {
    pub rule_id: String,
    pub part: CatalogPart,
}

/// CV-1: an entry's business part in version 2: its own part (`None` when it writes neither
/// `consumes` nor `produces`) and its per-rule parts, in the file's order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogParts {
    pub own: Option<CatalogPart>,
    pub rules: Vec<CatalogRulePart>,
}

impl CatalogParts {
    /// The part that stands for the rule `rule_id`: its per-rule part where the entry has one,
    /// else the entry's own part.
    pub fn for_rule(&self, rule_id: &str) -> Option<&CatalogPart> {
        self.rules
            .iter()
            .find(|rule| rule.rule_id == rule_id)
            .map(|rule| &rule.part)
            .or(self.own.as_ref())
    }

    /// The business members as the file writes them: the own part's ([`CatalogPart::members`])
    /// and then `rules`, each `{rule_id, ...its part's members}`, when there is one.
    pub fn members(&self) -> Map<String, Value> {
        let mut members = self
            .own
            .as_ref()
            .map(CatalogPart::members)
            .unwrap_or_default();
        if !self.rules.is_empty() {
            let rules = self
                .rules
                .iter()
                .map(|rule| {
                    let mut written = Map::new();
                    written.insert("rule_id".to_owned(), Value::String(rule.rule_id.clone()));
                    written.extend(rule.part.members());
                    Value::Object(written)
                })
                .collect();
            members.insert("rules".to_owned(), Value::Array(rules));
        }
        members
    }

    /// The relations the index is made of: each pool the own part or a per-rule part names, once
    /// per side, in that order, without a quantity.
    pub(super) fn relations(&self) -> CatalogBusiness {
        let mut business = CatalogBusiness::default();
        let parts = self
            .own
            .iter()
            .chain(self.rules.iter().map(|rule| &rule.part));
        for part in parts {
            for (effects, pools) in [
                (&part.consumes, &mut business.consumes),
                (&part.produces, &mut business.produces),
            ] {
                for effect in effects {
                    if !pools.iter().any(|pool| pool.pool_id == effect.pool_id) {
                        pools.push(CatalogPool {
                            pool_id: effect.pool_id.clone(),
                            quantity: None,
                        });
                    }
                }
            }
        }
        business
    }
}

/// CV-1: what a reader of `pack-catalog.json` holds it to of the game root's `schedule.json`: the
/// pools it declares (every catalog's), and for version 2 the pools with a valuation (a declared
/// block or the pending marker) and each rule's package by the rule's id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogScheduleFacts {
    pools: BTreeSet<String>,
    valued: BTreeSet<String>,
    rules: BTreeMap<String, String>,
}

impl CatalogScheduleFacts {
    /// The facts of `schedule`; of a root without `schedule.json` (`None`), none: no pool, no rule.
    pub fn of(schedule: Option<&StandardPackSchedule>) -> Self {
        let Some(schedule) = schedule else {
            return Self::default();
        };
        Self {
            pools: standard_pack_schedule_pools(schedule),
            valued: schedule
                .pools
                .iter()
                .filter(|pool| {
                    pool.body
                        .get("valuation")
                        .is_some_and(|value| !value.is_null())
                })
                .map(|pool| pool.id.clone())
                .collect(),
            rules: schedule
                .rules
                .iter()
                .map(|rule| (rule.id.clone(), rule.package_id.clone()))
                .collect(),
        }
    }

    /// The facts of a root whose `schedule.json` holds `schedule` (its bytes; `None` for a root
    /// without the file); `None` when its reader refuses the file, which is that file's own
    /// refusal: a reader then judges no reference to it.
    pub fn from_schedule_bytes(schedule: Option<&[u8]>) -> Option<Self> {
        match schedule {
            None => Some(Self::default()),
            Some(bytes) => parse_standard_pack_schedule(bytes)
                .ok()
                .map(|schedule| Self::of(Some(&schedule))),
        }
    }

    /// The pools `schedule.json` declares.
    pub fn pools(&self) -> &BTreeSet<String> {
        &self.pools
    }

    /// Whether `rule_id` is a rule of `schedule.json` that runs `package_id`.
    pub(super) fn runs(&self, rule_id: &str, package_id: &str) -> bool {
        self.rules
            .get(rule_id)
            .is_some_and(|package| package == package_id)
    }
}

impl Check<'_> {
    /// CV-1: the business part of the version 2 entry `object` at `pointer` of the package
    /// `package_id` (`None` when its id is no string): its own part when it writes one of the
    /// part's members ([`Self::part`]), and its per-rule parts ([`Self::rule_parts`]).
    pub(super) fn parts(
        &mut self,
        object: &Map<String, Value>,
        pointer: &str,
        package_id: Option<&str>,
    ) -> CatalogParts {
        let own = PART_MEMBERS
            .iter()
            .any(|member| object.contains_key(*member))
            .then(|| self.part(object, pointer));
        let rules = match object.get("rules") {
            None => Vec::new(),
            Some(value) => self.rule_parts(value, &at(pointer, "rules"), package_id),
        };
        CatalogParts { own, rules }
    }

    /// One business part, the members of `object` at `pointer`: `consumes` and `produces`
    /// (both required, [`Self::effects`]), `value` ([`Self::value`]), `limited` and
    /// `realization` ([`Self::mark`]). What the compiler refuses for a rule's fixed value is an
    /// error here, `catalog_value_conflict` at `value`: beside the realization mark, and beside a
    /// production of a pool whose valuation `schedule.json` declares or leaves pending (judged
    /// only when the reader holds the root's schedule).
    fn part(&mut self, object: &Map<String, Value>, pointer: &str) -> CatalogPart {
        let consumes = self.effects(
            object.get("consumes"),
            &at(pointer, "consumes"),
            EffectDirection::Consume,
        );
        let produces = self.effects(
            object.get("produces"),
            &at(pointer, "produces"),
            EffectDirection::Produce,
        );
        let at_value = at(pointer, "value");
        let value = object
            .get("value")
            .and_then(|value| self.value(value, &at_value));
        let limited = self.mark(object.get(LIMITED), &at(pointer, LIMITED));
        let realization = self.mark(object.get("realization"), &at(pointer, "realization"));
        let valued_production = self.facts.is_some_and(|facts| {
            produces
                .iter()
                .any(|effect| facts.valued.contains(&effect.pool_id))
        });
        if value.is_some() && (realization || valued_production) {
            self.error(StandardPackProblem::CatalogValueConflict, at_value);
        }
        CatalogPart {
            consumes,
            produces,
            value,
            limited,
            realization,
        }
    }

    /// One side of a business part at `pointer`, on the side of `direction`: a list of up to
    /// [`MAX_EFFECTS_PER_TASK`] effects, each a pool once (`pool_id` as version 1 holds it:
    /// `malformed`, `catalog_out_of_limits`, `catalog_pool_undeclared`, `catalog_duplicate`), read
    /// as a [`ResourceEffectSpec`] (a member it does not know or of another type is `malformed` at
    /// the effect) and held to its own rules ([`ResourceEffectSpec::form_fault`]: a bound broken
    /// is `catalog_out_of_limits`, a form `malformed`, at the member).
    fn effects(
        &mut self,
        value: Option<&Value>,
        pointer: &str,
        direction: EffectDirection,
    ) -> Vec<ResourceEffectSpec> {
        let mut effects = Vec::new();
        let Some(items) = value.and_then(Value::as_array) else {
            self.error(StandardPackProblem::Malformed, pointer);
            return effects;
        };
        if items.len() > MAX_EFFECTS_PER_TASK {
            self.error(StandardPackProblem::CatalogOutOfLimits, pointer);
        }
        let mut seen = BTreeSet::new();
        for (index, item) in items.iter().enumerate() {
            let at_item = format!("{pointer}/{index}");
            let Some(pool_id) = self.pool_reference(item, &at_item) else {
                continue;
            };
            if !seen.insert(pool_id) {
                self.error(
                    StandardPackProblem::CatalogDuplicate,
                    at(&at_item, "pool_id"),
                );
                continue;
            }
            let Ok(effect) = ResourceEffectSpec::deserialize(item) else {
                self.error(StandardPackProblem::Malformed, at_item);
                continue;
            };
            if let Some(fault) = effect.form_fault(direction) {
                let problem = if fault.bound {
                    StandardPackProblem::CatalogOutOfLimits
                } else {
                    StandardPackProblem::Malformed
                };
                self.error(problem, format!("{at_item}{}", fault.member));
                continue;
            }
            effects.push(effect);
        }
        effects
    }

    /// The `pool_id` of the effect `item` at `pointer`, held as version 1 holds a pool's: the
    /// item an object and its `pool_id` a string (`malformed`) of 1 to 128 bytes
    /// (`catalog_out_of_limits`), and a pool `schedule.json` declares (`catalog_pool_undeclared`,
    /// the pool still answered); `None` for the first two.
    fn pool_reference(&mut self, item: &Value, pointer: &str) -> Option<String> {
        let Some(object) = item.as_object() else {
            self.error(StandardPackProblem::Malformed, pointer);
            return None;
        };
        let at_pool = at(pointer, "pool_id");
        let pool_id = match object.get("pool_id").and_then(Value::as_str) {
            None => {
                self.error(StandardPackProblem::Malformed, at_pool);
                return None;
            }
            Some(pool_id)
                if pool_id.is_empty() || pool_id.len() > super::MAX_PACK_CATALOG_POOL_ID_BYTES =>
            {
                self.error(StandardPackProblem::CatalogOutOfLimits, at_pool);
                return None;
            }
            Some(pool_id) => pool_id.to_owned(),
        };
        if self
            .declared_pools
            .is_some_and(|declared| !declared.contains(&pool_id))
        {
            self.error(StandardPackProblem::CatalogPoolUndeclared, at_pool);
        }
        Some(pool_id)
    }

    /// A fixed value at `pointer`: an object of `milli`, a whole number (`malformed` otherwise)
    /// of 1 to [`MAX_EFFECT_QUANTITY`] (`catalog_out_of_limits` otherwise), and `origin`, which
    /// when written is `"model"` (`malformed` otherwise); a member it does not know is
    /// `malformed` at it.
    fn value(&mut self, value: &Value, pointer: &str) -> Option<CatalogValue> {
        let Some(object) = value.as_object() else {
            self.error(StandardPackProblem::Malformed, pointer);
            return None;
        };
        self.unknown(object, pointer, &VALUE_MEMBERS);
        let at_milli = at(pointer, "milli");
        let milli = match object.get("milli").map(Value::as_u64) {
            Some(Some(milli)) if (1..=MAX_EFFECT_QUANTITY).contains(&milli) => Some(milli),
            Some(Some(_)) => {
                self.error(StandardPackProblem::CatalogOutOfLimits, at_milli);
                None
            }
            _ => {
                self.error(StandardPackProblem::Malformed, at_milli);
                None
            }
        };
        let origin = match object.get("origin") {
            None => Some(ScheduleValueOrigin::Pack),
            Some(Value::String(origin)) if origin == SCHEDULE_VALUE_ORIGIN_MODEL => {
                Some(ScheduleValueOrigin::Model)
            }
            Some(_) => {
                self.error(StandardPackProblem::Malformed, at(pointer, "origin"));
                None
            }
        };
        Some(CatalogValue {
            milli: milli?,
            origin: origin?,
        })
    }

    /// A mark at `pointer` (`limited`, `realization`): only `true` marks; absent is no mark, and
    /// any other value, `false` included, is `malformed`, as a rule's mark is refused.
    fn mark(&mut self, value: Option<&Value>, pointer: &str) -> bool {
        match value {
            None => false,
            Some(Value::Bool(true)) => true,
            Some(_) => {
                self.error(StandardPackProblem::Malformed, pointer);
                false
            }
        }
    }

    /// `rules` at `pointer`: a list of up to [`MAX_PACK_CATALOG_RULE_PARTS`] per-rule parts, each
    /// `{rule_id, consumes, produces, value?, limited?, realization?}` ([`Self::part`]), its
    /// `rule_id` a rule id ([`Self::rule_id`]) given once in the entry (`catalog_duplicate` at the
    /// later one) and, where the reader holds the root's schedule, a rule of it that runs
    /// `package_id` (`catalog_rule_unknown` otherwise, as an entry of a package the registry does
    /// not hold is `package_not_listed`).
    fn rule_parts(
        &mut self,
        value: &Value,
        pointer: &str,
        package_id: Option<&str>,
    ) -> Vec<CatalogRulePart> {
        let mut parts = Vec::new();
        let Some(items) = value.as_array() else {
            self.error(StandardPackProblem::Malformed, pointer);
            return parts;
        };
        if items.len() > MAX_PACK_CATALOG_RULE_PARTS {
            self.error(StandardPackProblem::CatalogOutOfLimits, pointer);
        }
        let mut seen = BTreeSet::new();
        for (index, item) in items.iter().enumerate() {
            let at_item = format!("{pointer}/{index}");
            let Some(object) = item.as_object() else {
                self.error(StandardPackProblem::Malformed, at_item);
                continue;
            };
            self.unknown(object, &at_item, &RULE_PART_MEMBERS);
            let at_rule = at(&at_item, "rule_id");
            let rule_id = self.rule_id(object.get("rule_id"), &at_rule);
            let part = self.part(object, &at_item);
            let Some(rule_id) = rule_id else {
                continue;
            };
            if !seen.insert(rule_id.clone()) {
                self.error(StandardPackProblem::CatalogDuplicate, at_rule);
                continue;
            }
            if let (Some(facts), Some(package_id)) = (self.facts, package_id)
                && !facts.runs(&rule_id, package_id)
            {
                self.error(StandardPackProblem::CatalogRuleUnknown, at_rule);
            }
            parts.push(CatalogRulePart { rule_id, part });
        }
        parts
    }

    /// A rule id at `pointer`: a string (`malformed` otherwise) of 1 to
    /// [`MAX_SCHEDULING_IDENTIFIER_BYTES`] bytes (`catalog_out_of_limits` otherwise) of a
    /// scheduling identifier's grammar, the grammar of a `schedule.json` rule's id (`malformed`
    /// otherwise).
    fn rule_id(&mut self, value: Option<&Value>, pointer: &str) -> Option<String> {
        match value.and_then(Value::as_str) {
            None => {
                self.error(StandardPackProblem::Malformed, pointer);
                None
            }
            Some(rule_id)
                if rule_id.is_empty() || rule_id.len() > MAX_SCHEDULING_IDENTIFIER_BYTES =>
            {
                self.error(StandardPackProblem::CatalogOutOfLimits, pointer);
                None
            }
            Some(rule_id) if !scheduling_identifier(rule_id) => {
                self.error(StandardPackProblem::Malformed, pointer);
                None
            }
            Some(rule_id) => Some(rule_id.to_owned()),
        }
    }

    /// CV-1: the values file `{schema_version, packs: [{package_id, rule_id?, value?, limited?,
    /// realization?}]}` ([`STANDARD_PACK_CATALOG_VALUES_SCHEMA_VERSION`]), read as the display
    /// file is: its size and its JSON as the catalog's, then its `schema_version` (`malformed`
    /// at it, and nothing else read), a member repeated or unknown (`malformed`), `packs` a list
    /// of at most [`MAX_PACK_CATALOG_VALUE_ITEMS`] items (`catalog_out_of_limits`); each item's
    /// `package_id` a string (`malformed`) the registry holds (`package_not_listed`), its
    /// `rule_id`, when written, a rule id ([`Self::rule_id`]) of a rule that runs the package
    /// (`catalog_rule_unknown`), each part named once (`catalog_duplicate` at the later one), its
    /// value and marks by the reader's own checks ([`Self::value`], [`Self::mark`]) and a value
    /// beside the realization mark `catalog_value_conflict` at the value. Every item whose
    /// `package_id` is a string and whose `rule_id` reads is answered, in the file's order.
    fn values_file(&mut self, bytes: &[u8]) -> Vec<ValuesFileEntry> {
        let mut entries = Vec::new();
        if bytes.len() > MAX_STANDARD_PACK_CATALOG_BYTES {
            self.error(StandardPackProblem::CatalogOutOfLimits, "");
            return entries;
        }
        let mut repeated = Vec::new();
        let Some(Value::Object(document)) = read_document(bytes, &mut repeated) else {
            self.error(StandardPackProblem::Malformed, "");
            return entries;
        };
        if document.get("schema_version").and_then(Value::as_str)
            != Some(STANDARD_PACK_CATALOG_VALUES_SCHEMA_VERSION)
        {
            self.error(StandardPackProblem::Malformed, "/schema_version");
            return entries;
        }
        for pointer in repeated {
            self.error(StandardPackProblem::Malformed, pointer);
        }
        self.unknown(&document, "", &VALUES_FILE_MEMBERS);
        let Some(items) = document.get("packs").and_then(Value::as_array) else {
            self.error(StandardPackProblem::Malformed, "/packs");
            return entries;
        };
        if items.len() > MAX_PACK_CATALOG_VALUE_ITEMS {
            self.error(StandardPackProblem::CatalogOutOfLimits, "/packs");
        }
        let mut named = BTreeSet::new();
        for (index, item) in items.iter().enumerate() {
            let pointer = format!("/packs/{index}");
            let Some(object) = item.as_object() else {
                self.error(StandardPackProblem::Malformed, pointer);
                continue;
            };
            self.unknown(object, &pointer, &VALUES_ITEM_MEMBERS);
            let at_id = at(&pointer, "package_id");
            let Some(package_id) = object.get("package_id").and_then(Value::as_str) else {
                self.error(StandardPackProblem::Malformed, at_id);
                continue;
            };
            if self.registry.get(package_id).is_none() {
                self.error(StandardPackProblem::PackageNotListed, at_id.clone());
            }
            let at_rule = at(&pointer, "rule_id");
            let rule_id = match object.get("rule_id") {
                None => None,
                Some(value) => {
                    let Some(rule_id) = self.rule_id(Some(value), &at_rule) else {
                        continue;
                    };
                    if self
                        .facts
                        .is_some_and(|facts| !facts.runs(&rule_id, package_id))
                    {
                        self.error(StandardPackProblem::CatalogRuleUnknown, at_rule.clone());
                    }
                    Some(rule_id)
                }
            };
            let at_value = at(&pointer, "value");
            let value = object
                .get("value")
                .and_then(|value| self.value(value, &at_value));
            let limited = self.mark(object.get(LIMITED), &at(&pointer, LIMITED));
            let realization = self.mark(object.get("realization"), &at(&pointer, "realization"));
            if value.is_some() && realization {
                self.error(StandardPackProblem::CatalogValueConflict, at_value);
            }
            let address = CatalogPartAddress {
                package_id: package_id.to_owned(),
                rule_id,
            };
            if !named.insert(address.clone()) {
                let at_later = if address.rule_id.is_some() {
                    at_rule
                } else {
                    at_id
                };
                self.error(StandardPackProblem::CatalogDuplicate, at_later);
                continue;
            }
            entries.push(ValuesFileEntry {
                address,
                value,
                limited,
                realization,
                pointer,
            });
        }
        entries
    }
}

/// CV-1: the values file a line of `standard-pack catalog` gives (`--values`): where it is,
/// which its refusals name, and its bytes.
#[derive(Debug, Clone, Copy)]
pub struct CatalogValuesFile<'a> {
    pub path: &'a Path,
    pub bytes: &'a [u8],
}

/// CV-1: a business part the values file names: an entry's own (`rule_id` absent) or the part of
/// the rule `rule_id`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct CatalogPartAddress {
    pub package_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
}

/// CV-1: one item of the values file: the part it names, the value and the marks it gives, and
/// its pointer in that file.
#[derive(Debug, Clone)]
pub(super) struct ValuesFileEntry {
    address: CatalogPartAddress,
    value: Option<CatalogValue>,
    limited: bool,
    realization: bool,
    pointer: String,
}

/// CV-1: the items of the values file (`bytes`), checked against `registry` and the rules of
/// `facts` ([`Check::values_file`]); every error of the file, by the catalog's own words, at its
/// pointer in it, otherwise.
pub(super) fn read_values_file(
    bytes: &[u8],
    registry: &StandardPackRegistry,
    facts: &CatalogScheduleFacts,
) -> Result<Vec<ValuesFileEntry>, Vec<CatalogDisplayFileIssue>> {
    let mut check = Check {
        registry,
        declared_pools: None,
        facts: Some(facts),
        version: CatalogVersion::V2,
        errors: Vec::new(),
        warnings: Vec::new(),
        defects: 0,
    };
    let entries = check.values_file(bytes);
    if check.errors.is_empty() {
        Ok(entries)
    } else {
        Err(check
            .errors
            .into_iter()
            .map(|issue| CatalogDisplayFileIssue {
                problem: issue.problem,
                pointer: issue.pointer,
            })
            .collect())
    }
}

/// CV-1: the values file's `entries` applied to the entries of `packs` (every package of the
/// registry has one by now, and the file names only such packages): the part each names has its
/// value and its marks replaced whole by the item's (one it does not give is removed), its
/// effects untouched. An item without `rule_id` names the entry's own part. An item with one names
/// that rule's part; where the entry has none for it, the entry's own part, which stood for the
/// rule, is copied as the rule's part, with the item's value and marks. A part that is not there
/// (an own part the entry does not write, or a rule's part where the entry writes neither it nor
/// an own part) is `catalog_part_absent` at the item. Each part written is answered in
/// `written`, in the file's order.
pub(super) fn apply_values(
    packs: &mut [Value],
    entries: &[ValuesFileEntry],
    written: &mut Vec<CatalogPartAddress>,
) -> Result<(), Vec<CatalogDisplayFileIssue>> {
    let mut issues = Vec::new();
    for given in entries {
        let found = packs.iter_mut().find(|pack| {
            pack.get("package_id").and_then(Value::as_str)
                == Some(given.address.package_id.as_str())
        });
        let Some(Value::Object(entry)) = found else {
            continue;
        };
        let applied = match &given.address.rule_id {
            None if own_part(entry) => {
                let replaced = with_marks(entry, given);
                *entry = replaced;
                true
            }
            None => false,
            Some(rule_id) => rule_marks(entry, rule_id, given),
        };
        if applied {
            written.push(given.address.clone());
        } else {
            issues.push(CatalogDisplayFileIssue {
                problem: StandardPackProblem::CatalogPartAbsent,
                pointer: given.pointer.clone(),
            });
        }
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

/// Whether the entry `entry` writes its own business part.
fn own_part(entry: &Map<String, Value>) -> bool {
    entry.contains_key("consumes") && entry.contains_key("produces")
}

/// The values file's item `given` applied to the part of the rule `rule_id` in `entry`
/// ([`apply_values`]); `false` when no part is there to take it.
fn rule_marks(entry: &mut Map<String, Value>, rule_id: &str, given: &ValuesFileEntry) -> bool {
    if let Some(Value::Array(rules)) = entry.get_mut("rules")
        && let Some(Value::Object(part)) = rules
            .iter_mut()
            .find(|part| part.get("rule_id").and_then(Value::as_str) == Some(rule_id))
    {
        let replaced = with_marks(part, given);
        *part = replaced;
        return true;
    }
    if !own_part(entry) {
        return false;
    }
    let mut part = Map::new();
    part.insert("rule_id".to_owned(), Value::String(rule_id.to_owned()));
    for side in CatalogSide::ALL.iter().copied() {
        if let Some(effects) = entry.get(side.as_str()) {
            part.insert(side.as_str().to_owned(), effects.clone());
        }
    }
    let part = Value::Object(with_marks(&part, given));
    match entry.get_mut("rules") {
        Some(Value::Array(rules)) => rules.push(part),
        _ => {
            entry.insert("rules".to_owned(), Value::Array(vec![part]));
        }
    }
    true
}

/// The part `part` with its value and marks replaced whole by the item `given`'s: every member
/// but those three as it stands, in its order, the item's written right after `produces`.
fn with_marks(part: &Map<String, Value>, given: &ValuesFileEntry) -> Map<String, Value> {
    let mut written = Map::new();
    for (name, value) in part {
        if MARK_MEMBERS.contains(&name.as_str()) {
            continue;
        }
        written.insert(name.clone(), value.clone());
        if name == "produces" {
            if let Some(fixed) = given.value {
                written.insert("value".to_owned(), fixed.document());
            }
            if given.limited {
                written.insert(LIMITED.to_owned(), Value::Bool(true));
            }
            if given.realization {
                written.insert("realization".to_owned(), Value::Bool(true));
            }
        }
    }
    written
}

/// CV-1: `entry` with its business part replaced whole by `parts`: every other member (its
/// `package_id` and its display) as it stands, in its order, then `parts`' members
/// ([`CatalogParts::members`]).
pub(super) fn with_business(
    entry: &Map<String, Value>,
    parts: &CatalogParts,
) -> Map<String, Value> {
    let mut written = entry
        .iter()
        .filter(|(name, _)| !BUSINESS_MEMBERS.contains(&name.as_str()))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect::<Map<String, Value>>();
    written.extend(parts.members());
    written
}

/// CV-1 (`--refresh`, and each entry of a line that writes version 2): the business part the
/// rules of `package_id` in `schedule` give. Each rule's part is what it writes today: its
/// `consumes` and `produces` effects as written, its `fixed_value_milli` with its origin, and its
/// marks `limited` and `realization`. A package without a rule gets an empty own part; when every
/// rule's part is the same (each side the same effects, in any order), that part is the entry's
/// own; when they differ, each rule gets its per-rule part, in the file's order, and the entry no
/// own part (nothing is chosen for a person). An effect that is no V2 effect, a fixed value that is
/// no whole number and a `limited` other than `true` cannot be written: each is
/// `schedule_invalid` in `schedule.json` at its pointer, in the file's order.
pub(super) fn rules_parts(
    schedule: Option<&StandardPackSchedule>,
    package_id: &str,
) -> Result<CatalogParts, Vec<StandardPackRootIssue>> {
    let mut parts = Vec::new();
    let mut issues = Vec::new();
    let rules = schedule.into_iter().flat_map(|schedule| &schedule.rules);
    for (index, rule) in rules.enumerate() {
        if rule.package_id != package_id {
            continue;
        }
        match rule_part(rule, index) {
            Ok(part) => parts.push(CatalogRulePart {
                rule_id: rule.id.clone(),
                part,
            }),
            Err(issue) => issues.push(issue),
        }
    }
    if !issues.is_empty() {
        return Err(issues);
    }
    let agree = parts
        .first()
        .is_none_or(|first| parts.iter().all(|rule| same_part(&rule.part, &first.part)));
    if agree {
        let own = parts
            .into_iter()
            .next()
            .map(|rule| rule.part)
            .unwrap_or_default();
        Ok(CatalogParts {
            own: Some(own),
            rules: Vec::new(),
        })
    } else {
        Ok(CatalogParts {
            own: None,
            rules: parts,
        })
    }
}

/// The business part the rule `rule`, the `index`th of `schedule.json`, writes today
/// ([`rules_parts`]).
fn rule_part(rule: &ScheduleRule, index: usize) -> Result<CatalogPart, StandardPackRootIssue> {
    let pointer = format!("/rules/{index}");
    let invalid = |member: String| {
        StandardPackRootIssue::new(
            STANDARD_PACK_SCHEDULE_FILE,
            StandardPackProblem::ScheduleInvalid,
            member,
        )
    };
    let mut sides = [Vec::new(), Vec::new()];
    for (slot, side) in CatalogSide::ALL.iter().copied().enumerate() {
        let items = rule.body.get(side.as_str()).and_then(Value::as_array);
        for (position, item) in items.into_iter().flatten().enumerate() {
            let effect = ResourceEffectSpec::deserialize(item)
                .map_err(|_| invalid(format!("{pointer}/{}/{position}", side.as_str())))?;
            sides[slot].push(effect);
        }
    }
    let value = match &rule.fixed_value_milli {
        None => None,
        Some(value) => match value.as_u64() {
            Some(milli) => Some(CatalogValue {
                milli,
                origin: rule.fixed_value_origin,
            }),
            None => {
                return Err(invalid(format!(
                    "{pointer}/{SCHEDULE_RULE_FIXED_VALUE_MEMBER}"
                )));
            }
        },
    };
    let limited = match rule.body.get(LIMITED) {
        None => false,
        Some(Value::Bool(true)) => true,
        Some(_) => return Err(invalid(format!("{pointer}/{LIMITED}"))),
    };
    let [consumes, produces] = sides;
    Ok(CatalogPart {
        consumes,
        produces,
        value,
        limited,
        realization: rule.realization,
    })
}

/// Whether two parts say the same: their value and marks, and on each side the same effects in
/// any order.
fn same_part(left: &CatalogPart, right: &CatalogPart) -> bool {
    left.value == right.value
        && left.limited == right.limited
        && left.realization == right.realization
        && CatalogSide::ALL.iter().all(|side| {
            let (ours, theirs) = (left.side(*side), right.side(*side));
            ours.len() == theirs.len() && ours.iter().all(|effect| theirs.contains(effect))
        })
}

/// The effects `rule` names on `side`, each pool once, in its order, with the effect it writes
/// for it when that is a V2 effect.
fn rule_effects(
    rule: &ScheduleRule,
    side: CatalogSide,
) -> Vec<(String, Option<ResourceEffectSpec>)> {
    let mut named: Vec<(String, Option<ResourceEffectSpec>)> = Vec::new();
    let items = rule.body.get(side.as_str()).and_then(Value::as_array);
    for item in items.into_iter().flatten() {
        let Some(pool_id) = item.get("pool_id").and_then(Value::as_str) else {
            continue;
        };
        if named.iter().any(|(known, _)| known.as_str() == pool_id) {
            continue;
        }
        named.push((
            pool_id.to_owned(),
            ResourceEffectSpec::deserialize(item).ok(),
        ));
    }
    named
}

/// The fixed value `rule` writes, as an entry holds one.
fn rule_value(rule: &ScheduleRule) -> Option<CatalogValue> {
    rule.fixed_value_milli
        .as_ref()
        .and_then(Value::as_u64)
        .map(|milli| CatalogValue {
            milli,
            origin: rule.fixed_value_origin,
        })
}

/// CV-1: every difference between the version 2 entry of `package_id` (its business part
/// `parts`) and `rules`, the rules of `schedule.json` that run its package, in the file's order.
/// Each rule is compared with the part that stands for it ([`CatalogParts::for_rule`]; none: an
/// empty part): on each side, `consumes` before `produces`, each pool the rule names that the
/// part does not list (`catalog_lacks`) or lists with another effect (`quantity_differs`), in
/// the rule's order, then each pool the part lists that the rule does not name
/// (`schedule_lacks`), every one naming the rule; then the fixed value (`value_differs`) and the
/// two marks (`limited_differs`, `realization_differs`), naming no pool and no side. A package
/// without a rule: each pool of the own part is `schedule_lacks`, naming no rule, as in version 1.
pub(super) fn part_differences(
    package_id: &str,
    parts: &CatalogParts,
    rules: &[&ScheduleRule],
    differences: &mut Vec<CatalogScheduleDifference>,
) {
    let found = |pool_id: Option<&str>,
                 side: Option<CatalogSide>,
                 kind: CatalogDifference,
                 rule_id: Option<&str>| CatalogScheduleDifference {
        package_id: package_id.to_owned(),
        pool_id: pool_id.map(str::to_owned),
        side,
        difference: kind,
        rule_id: rule_id.map(str::to_owned),
    };
    if rules.is_empty() {
        for side in CatalogSide::ALL.iter().copied() {
            for effect in parts.own.iter().flat_map(|own| own.side(side)) {
                differences.push(found(
                    Some(effect.pool_id.as_str()),
                    Some(side),
                    CatalogDifference::ScheduleLacks,
                    None,
                ));
            }
        }
        return;
    }
    let empty = CatalogPart::default();
    for rule in rules {
        let part = parts.for_rule(&rule.id).unwrap_or(&empty);
        let rule_id = Some(rule.id.as_str());
        for side in CatalogSide::ALL.iter().copied() {
            let named = rule_effects(rule, side);
            for (pool_id, effect) in &named {
                let listed = part
                    .side(side)
                    .iter()
                    .find(|listed| listed.pool_id == *pool_id);
                let kind = match listed {
                    None => CatalogDifference::CatalogLacks,
                    Some(listed) if effect.as_ref() != Some(listed) => {
                        CatalogDifference::QuantityDiffers
                    }
                    Some(_) => continue,
                };
                differences.push(found(Some(pool_id.as_str()), Some(side), kind, rule_id));
            }
            for listed in part.side(side) {
                if !named.iter().any(|(pool_id, _)| *pool_id == listed.pool_id) {
                    differences.push(found(
                        Some(listed.pool_id.as_str()),
                        Some(side),
                        CatalogDifference::ScheduleLacks,
                        rule_id,
                    ));
                }
            }
        }
        let marks = [
            (
                rule_value(rule) != part.value,
                CatalogDifference::ValueDiffers,
            ),
            (
                (rule.body.get(LIMITED) == Some(&Value::Bool(true))) != part.limited,
                CatalogDifference::LimitedDiffers,
            ),
            (
                rule.realization != part.realization,
                CatalogDifference::RealizationDiffers,
            ),
        ];
        for (differs, kind) in marks {
            if differs {
                differences.push(found(None, None, kind, rule_id));
            }
        }
    }
}
