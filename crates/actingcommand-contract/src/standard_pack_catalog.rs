// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #477 PC-1 (register §1.438; `contracts/pack-catalog.md`; the owner's approval on board
//! #477): a game root's one declaration per package, `pack-catalog.json`
//! (`actingcommand.pack-catalog.v1`), beside `bundle.json`. It replaces `ui.json` (Workflow #429
//! R-11) outright: no program reads `ui.json` any more ([`STANDARD_PACK_RETIRED_UI_FILE`] names it
//! only so that the tools can say a root still holds one).
//!
//! The file has two parts.
//!
//! - **The display part**: the groups a client sorts the game's packages into (`groups`, each
//!   `{id, title}`), the display the game supports (`display`, `{aspect_ratios, safe_area?}`), and,
//!   per package, its `title`, `summary`, `group` and opaque `risk` words, each text per locale,
//!   as `ui.json` carried them. A package whose entry writes none of the four has no display yet:
//!   it shows with its id.
//! - **The business part**, per package: what it consumes and what it produces (`consumes`,
//!   `produces`), each a list of `{pool_id, quantity?}`, the pool one that the root's
//!   `schedule.json` declares and the quantity `{"kind": "constant", "amount": N}` or
//!   `{"kind": "pending"}` (absent: the relation is known, the amount is not stated). There is no
//!   value member in this version: a member the reader does not know is an error at its pointer,
//!   so that a later version adds the value under its own schema.
//!
//! [`parse_standard_pack_catalog`] reads it. A defect of the file as a whole or of its business
//! part is an error (`standard_pack_root_invalid` where a tool reports it): the root's declaration
//! is wrong. A defect of the display part is a warning
//! ([`StandardPackCatalog::display_issues`]): the group, the `display` block or the package's
//! display it touches is left out, and the package shows with its id, as R-11 fell back. The
//! index both ways ([`StandardPackCatalog::index`]) is computed by the reader and never stored.
//! [`standard_pack_catalog_differences`] compares the business part with the rules of
//! `schedule.json`, and [`merge_standard_pack_catalog`] is the writer's one rule.
//!
//! The Runtime never reads the file: only the Lab's tools (`packs`, `standard-pack check`,
//! `standard-pack catalog`, `standard-pack build` and the installer's `place`) do. Every text,
//! word, locale, group id and risk word is opaque: compared only for equality and measured only in
//! bytes; an aspect ratio is only checked to be of the form `<w>:<h>`.

use crate::codes::SharedCode;
use crate::outcome::{Outcome, VocabularyToken};
use crate::standard_pack::{
    STANDARD_PACK_BUNDLE_FILE, StandardPackProblem, StandardPackRegistry, StandardPackRoot,
    StandardPackRootBytes, StandardPackRootIssue, read_standard_pack_root,
};
use crate::standard_pack_schedule::{ScheduleRule, StandardPackSchedule};
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::{Serialize, Serializer};
use serde_json::{Map, Number, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

/// The game root's package declaration (Workflow #477 PC-1).
pub const STANDARD_PACK_CATALOG_FILE: &str = "pack-catalog.json";
/// The one version of `pack-catalog.json` this build reads and writes.
pub const STANDARD_PACK_CATALOG_SCHEMA_VERSION: &str = "actingcommand.pack-catalog.v1";
/// The file the catalog replaced (Workflow #429 R-11's display metadata), which no program reads
/// any more: a root that still holds it is told so, and the installer's `place` sets it aside.
pub const STANDARD_PACK_RETIRED_UI_FILE: &str = "ui.json";
/// The largest `pack-catalog.json`, in bytes.
pub const MAX_STANDARD_PACK_CATALOG_BYTES: usize = 1024 * 1024;
/// The most errors, and the most warnings, one reading reports; the first ones in the reader's
/// order.
pub const MAX_STANDARD_PACK_CATALOG_ISSUES: usize = 64;
/// The most entries of `groups`.
pub const MAX_PACK_CATALOG_GROUPS: usize = 64;
/// The most entries of `packs`.
pub const MAX_PACK_CATALOG_PACKS: usize = 1024;
/// The most locales of one text (`title`, `summary`): at least one. Workflow #455 G-R1 and
/// Workflow #463 V1 hold their texts to the same bounds.
pub const MAX_PACK_CATALOG_LOCALES: usize = 16;
/// The longest locale name of a text, in bytes (at least one).
pub const MAX_PACK_CATALOG_LOCALE_BYTES: usize = 35;
/// The longest word, in bytes (at least one): a group's `id`, a package's `group`, a risk word and
/// `display.safe_area`.
pub const MAX_PACK_CATALOG_WORD_BYTES: usize = 64;
/// The longest short text, in bytes (at least one): a `title`, per locale.
pub const MAX_PACK_CATALOG_TITLE_BYTES: usize = 128;
/// The longest `summary` text, in bytes (at least one), per locale.
pub const MAX_PACK_CATALOG_SUMMARY_BYTES: usize = 1024;
/// The most risk words of one package.
pub const MAX_PACK_CATALOG_RISK_WORDS: usize = 8;
/// The most aspect ratios of `display` (at least one).
pub const MAX_PACK_CATALOG_ASPECT_RATIOS: usize = 8;
/// The most digits of one side of an aspect ratio `<w>:<h>`.
pub const MAX_PACK_CATALOG_ASPECT_DIGITS: usize = 5;
/// The most pools of one package's `consumes`, and of its `produces`.
pub const MAX_PACK_CATALOG_POOLS: usize = 256;
/// The longest `pool_id`, in bytes (at least one).
pub const MAX_PACK_CATALOG_POOL_ID_BYTES: usize = 128;
/// The largest constant `amount`: the largest integer every JSON reader keeps exactly.
pub const MAX_PACK_CATALOG_AMOUNT: u64 = (1 << 53) - 1;

/// The members of the document.
const DOCUMENT_MEMBERS: [&str; 5] = ["schema_version", "game", "groups", "display", "packs"];
/// The members of one package's entry.
const ENTRY_MEMBERS: [&str; 7] = [
    "package_id",
    "title",
    "summary",
    "group",
    "risk",
    "consumes",
    "produces",
];
/// The display members of one package's entry.
const DISPLAY_MEMBERS: [&str; 4] = ["title", "summary", "group", "risk"];
/// The members of one group.
const GROUP_MEMBERS: [&str; 2] = ["id", "title"];
/// The members of `display`.
const DISPLAY_BLOCK_MEMBERS: [&str; 2] = ["aspect_ratios", "safe_area"];
/// The members of one pool of `consumes` or `produces`.
const POOL_MEMBERS: [&str; 2] = ["pool_id", "quantity"];
/// The members of a quantity.
const QUANTITY_MEMBERS: [&str; 2] = ["kind", "amount"];
/// A quantity's two kinds.
const KIND_CONSTANT: &str = "constant";
const KIND_PENDING: &str = "pending";

crate::outcome_vocabulary! {
    /// Which side of a package's business part a pool stands on (vocabulary `catalog_side`,
    /// Workflow #477 PC-1): the entry's and a rule's member of that name.
    pub enum CatalogSide: "catalog_side" {
        /// What one run of the package uses up.
        Consumes => "consumes",
        /// What one run of the package brings in.
        Produces => "produces",
    }
}

crate::outcome_vocabulary! {
    /// Which of the two files lacks a pool a package consumes or produces (vocabulary
    /// `catalog_difference`, Workflow #477 PC-1): in this version a package's consumption and
    /// production stand both in `schedule.json`'s rules and in its entry of `pack-catalog.json`,
    /// and the tools list every difference.
    pub enum CatalogDifference: "catalog_difference" {
        /// A rule of the package in `schedule.json` names the pool on that side; the package's
        /// entry does not list it.
        CatalogLacks => "catalog_lacks",
        /// The package's entry lists the pool on that side; no rule of the package names it there.
        ScheduleLacks => "schedule_lacks",
    }
}

/// The quantity of one pool of a package's business part: a constant amount per run, or pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CatalogQuantity {
    /// `{"kind": "constant", "amount": N}`: N units per run, 1 to [`MAX_PACK_CATALOG_AMOUNT`].
    Constant { amount: u64 },
    /// `{"kind": "pending"}`: the amount is not known yet.
    Pending,
}

/// One pool a package consumes or produces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CatalogPool {
    pub pool_id: String,
    /// `None`: the relation is known, the amount is not stated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantity: Option<CatalogQuantity>,
}

/// A package's business part: what it consumes and what it produces, each in the file's order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CatalogBusiness {
    pub consumes: Vec<CatalogPool>,
    pub produces: Vec<CatalogPool>,
}

impl CatalogBusiness {
    /// The pools of `side`.
    pub fn side(&self, side: CatalogSide) -> &[CatalogPool] {
        match side {
            CatalogSide::Consumes => &self.consumes,
            CatalogSide::Produces => &self.produces,
        }
    }
}

/// One package's entry, as read.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogEntry {
    pub package_id: String,
    /// Its display members as written (`package_id` and those of `title`, `summary`, `group` and
    /// `risk` it writes, in the file's order) when they keep every rule; `None` when it writes
    /// none of them or one of them has a defect: the package then shows with its id.
    pub display: Option<Map<String, Value>>,
    pub business: CatalogBusiness,
}

/// The index both ways of one game root's catalog, computed by the reader and never stored: per
/// pool, the packages that produce it and the packages that consume it, each package once per
/// pool, in the order of the file's entries. A pool whose quantity is pending or absent is in it:
/// the index holds the relation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct StandardPackCatalogIndex {
    pub producing: BTreeMap<String, Vec<String>>,
    pub consuming: BTreeMap<String, Vec<String>>,
}

/// A `pack-catalog.json` whose business part keeps every rule, as read.
#[derive(Debug, Clone, PartialEq)]
pub struct StandardPackCatalog {
    /// The groups that keep every rule, as written, in the file's order.
    groups: Vec<Value>,
    /// `display`, as written, when it keeps every rule.
    display: Option<Value>,
    entries: Vec<CatalogEntry>,
    display_issues: Vec<StandardPackRootIssue>,
}

impl StandardPackCatalog {
    /// Every package's entry, in the file's order.
    pub fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }

    /// The entry of `package_id`; `None` when the file lists none for it.
    pub fn entry(&self, package_id: &str) -> Option<&CatalogEntry> {
        self.entries
            .iter()
            .find(|entry| entry.package_id == package_id)
    }

    /// The defects of the display part, each a warning at its JSON pointer, at most
    /// [`MAX_STANDARD_PACK_CATALOG_ISSUES`] in the reader's order.
    pub fn display_issues(&self) -> &[StandardPackRootIssue] {
        &self.display_issues
    }

    /// `display`, as written, when the file declares one that keeps every rule.
    pub fn display(&self) -> Option<&Value> {
        self.display.as_ref()
    }

    /// The display part as a client reads it: `{groups, packs}`, the groups that keep every rule
    /// and the display of every package that has one ([`CatalogEntry::display`]), each as
    /// written, in the file's order.
    pub fn ui(&self) -> Value {
        json!({
            "groups": self.groups,
            "packs": self
                .entries
                .iter()
                .filter_map(|entry| entry.display.clone().map(Value::Object))
                .collect::<Vec<_>>(),
        })
    }

    /// The index both ways ([`StandardPackCatalogIndex`]).
    pub fn index(&self) -> StandardPackCatalogIndex {
        let mut index = StandardPackCatalogIndex::default();
        for entry in &self.entries {
            for (side, pools) in [
                (CatalogSide::Produces, &mut index.producing),
                (CatalogSide::Consumes, &mut index.consuming),
            ] {
                for pool in entry.business.side(side) {
                    let packages = pools.entry(pool.pool_id.clone()).or_default();
                    if !packages.contains(&entry.package_id) {
                        packages.push(entry.package_id.clone());
                    }
                }
            }
        }
        index
    }

    /// The packages of `registry` the file lists no entry for, in the registry's order: neither
    /// an error nor a warning, a root's entries being filled over time.
    pub fn unlisted(&self, registry: &StandardPackRegistry) -> Vec<String> {
        registry
            .package_ids()
            .filter(|package_id| self.entry(package_id).is_none())
            .map(str::to_owned)
            .collect()
    }
}

/// Workflow #477 PC-1: [`read_standard_pack_root`] for a reader of `pack-catalog.json`. `catalog`
/// says whether the directory holds the file, which makes a directory a game root as the other
/// root files do: a directory whose only root file is `pack-catalog.json` is `bundle_missing`
/// (the bundle's file, the whole document). Every other answer is [`read_standard_pack_root`]'s.
pub fn read_standard_pack_root_with_catalog(
    files: StandardPackRootBytes<'_>,
    catalog: bool,
    game: &str,
) -> Result<Option<StandardPackRoot>, StandardPackRootIssue> {
    match read_standard_pack_root(files, game)? {
        None if catalog => Err(StandardPackRootIssue::new(
            STANDARD_PACK_BUNDLE_FILE,
            StandardPackProblem::BundleMissing,
            "",
        )),
        root => Ok(root),
    }
}

/// The pools `schedule` declares (its `pools[].id`): the pools a catalog may name.
pub fn standard_pack_schedule_pools(schedule: &StandardPackSchedule) -> BTreeSet<String> {
    schedule.pools.iter().map(|pool| pool.id.clone()).collect()
}

/// `pack-catalog.json`'s bytes, read for the game directory `game` against the game root's
/// `registry` and the pools its `schedule.json` declares (`pools`: `Some` of an empty set for a
/// root without `schedule.json`; `None` when that file could not be read, which is its own
/// refusal: then no pool reference is judged here).
///
/// The errors, each an issue at its JSON pointer (empty for the whole document), at most
/// [`MAX_STANDARD_PACK_CATALOG_ISSUES`]:
///
/// - `catalog_out_of_limits`: the file is larger than [`MAX_STANDARD_PACK_CATALOG_BYTES`], or
///   `packs`, a `consumes` or `produces` list or a `pool_id` is outside its limit;
/// - `malformed`: no JSON or no object, a member repeated in one object, a member the reader does
///   not know anywhere in the file (a value member included), a business or document member that
///   is missing or not of its type, a `schema_version` other than
///   [`STANDARD_PACK_CATALOG_SCHEMA_VERSION`], a quantity that is not exactly one of its two forms;
/// - `game_mismatch`: `game` is not the directory's;
/// - `package_not_listed`: a `package_id` the registry does not hold;
/// - `catalog_duplicate`: a `package_id` listed twice, or a pool listed twice on one side of an
///   entry;
/// - `catalog_pool_undeclared`: a `pool_id` the root's `schedule.json` does not declare.
///
/// A file without an error reads ([`StandardPackCatalog`]); its display defects are warnings
/// ([`StandardPackCatalog::display_issues`]): `malformed` (a display member not of its type, an
/// aspect ratio not `<w>:<h>`), `catalog_out_of_limits` (a list, a text, a locale or a word
/// outside its limit), `catalog_duplicate` (a group id, a risk word of one package or an aspect
/// ratio twice), `catalog_group_unknown` (a package's `group` is no group's id) and
/// `catalog_text_missing` (a group without `title`, or a package that writes a display member
/// without `title` or `summary`).
pub fn parse_standard_pack_catalog(
    bytes: &[u8],
    game: &str,
    registry: &StandardPackRegistry,
    pools: Option<&BTreeSet<String>>,
) -> Result<StandardPackCatalog, Vec<StandardPackRootIssue>> {
    let read = read_catalog(bytes, game, registry, pools);
    if read.errors.is_empty() {
        Ok(read.catalog)
    } else {
        Err(read.errors)
    }
}

/// One reading of the file: the document when it is a JSON object, what it holds as far as it
/// reads, and its errors.
struct CatalogRead {
    document: Option<Map<String, Value>>,
    catalog: StandardPackCatalog,
    errors: Vec<StandardPackRootIssue>,
}

fn read_catalog(
    bytes: &[u8],
    game: &str,
    registry: &StandardPackRegistry,
    pools: Option<&BTreeSet<String>>,
) -> CatalogRead {
    let mut check = Check {
        registry,
        declared_pools: pools,
        errors: Vec::new(),
        warnings: Vec::new(),
        defects: 0,
    };
    if bytes.len() > MAX_STANDARD_PACK_CATALOG_BYTES {
        check.error(StandardPackProblem::CatalogOutOfLimits, "");
        return check.finish(None, Vec::new(), None, Vec::new());
    }
    let mut repeated = Vec::new();
    let Some(document) = read_document(bytes, &mut repeated) else {
        check.error(StandardPackProblem::Malformed, "");
        return check.finish(None, Vec::new(), None, Vec::new());
    };
    for pointer in repeated {
        check.error(StandardPackProblem::Malformed, pointer);
    }
    let Value::Object(document) = document else {
        check.error(StandardPackProblem::Malformed, "");
        return check.finish(None, Vec::new(), None, Vec::new());
    };
    let (groups, display, entries) = check.read(&document, game);
    check.finish(Some(document), groups, display, entries)
}

/// The JSON document of `bytes` with its members in the file's order, and in `repeated` the
/// pointer of every member an object repeats (the last one is kept, as `serde_json` keeps it);
/// `None` when the bytes are no JSON document. Workflow #407 LOAD-1: `loading.json` and a pack's
/// loading copy are read by it too, and Workflow #455 G-R1 `goals.json`.
pub(crate) fn read_document(bytes: &[u8], repeated: &mut Vec<String>) -> Option<Value> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let document = Tracked {
        pointer: String::new(),
        repeated,
    }
    .deserialize(&mut deserializer)
    .ok()?;
    deserializer.end().ok()?;
    Some(document)
}

/// A JSON value at `pointer` whose repeated members are recorded.
struct Tracked<'r> {
    pointer: String,
    repeated: &'r mut Vec<String>,
}

impl<'de> DeserializeSeed<'de> for Tracked<'_> {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Tracked<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        // A number that is no whole number is refused as not of its type where one is read.
        Ok(Number::from_f64(value).map_or(Value::Null, Value::Number))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
        let Tracked { pointer, repeated } = self;
        let mut items = Vec::new();
        while let Some(item) = access.next_element_seed(Tracked {
            pointer: format!("{pointer}/{}", items.len()),
            repeated: &mut *repeated,
        })? {
            items.push(item);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
        let Tracked { pointer, repeated } = self;
        let mut members = Map::new();
        while let Some(name) = access.next_key::<String>()? {
            let at = at(&pointer, &name);
            let value = access.next_value_seed(Tracked {
                pointer: at.clone(),
                repeated: &mut *repeated,
            })?;
            if members.contains_key(&name) {
                repeated.push(at);
            }
            members.insert(name, value);
        }
        Ok(Value::Object(members))
    }
}

/// The checks of one document: the errors and the display defects found so far.
struct Check<'a> {
    registry: &'a StandardPackRegistry,
    /// The pools the root's `schedule.json` declares; `None`: not judged.
    declared_pools: Option<&'a BTreeSet<String>>,
    errors: Vec<StandardPackRootIssue>,
    warnings: Vec<StandardPackRootIssue>,
    /// Every display defect found, those beyond the issue bound counted too.
    defects: usize,
}

impl Check<'_> {
    fn error(&mut self, problem: StandardPackProblem, pointer: impl Into<String>) {
        if self.errors.len() < MAX_STANDARD_PACK_CATALOG_ISSUES {
            self.errors.push(StandardPackRootIssue::new(
                STANDARD_PACK_CATALOG_FILE,
                problem,
                pointer,
            ));
        }
    }

    fn warn(&mut self, problem: StandardPackProblem, pointer: impl Into<String>) {
        self.defects += 1;
        if self.warnings.len() < MAX_STANDARD_PACK_CATALOG_ISSUES {
            self.warnings.push(StandardPackRootIssue::new(
                STANDARD_PACK_CATALOG_FILE,
                problem,
                pointer,
            ));
        }
    }

    /// The reading's result, the display issues with the catalog.
    fn finish(
        self,
        document: Option<Map<String, Value>>,
        groups: Vec<Value>,
        display: Option<Value>,
        entries: Vec<CatalogEntry>,
    ) -> CatalogRead {
        CatalogRead {
            document,
            catalog: StandardPackCatalog {
                groups,
                display,
                entries,
                display_issues: self.warnings,
            },
            errors: self.errors,
        }
    }

    /// Every member of `object` (at `pointer`) that is not one of `known` is `malformed` at its
    /// pointer: the reader knows every member it reads.
    fn unknown(&mut self, object: &Map<String, Value>, pointer: &str, known: &[&str]) {
        for name in object.keys() {
            if !known.contains(&name.as_str()) {
                self.error(StandardPackProblem::Malformed, at(pointer, name));
            }
        }
    }

    /// The document: its envelope, then `groups`, `display` and `packs`.
    fn read(
        &mut self,
        document: &Map<String, Value>,
        game: &str,
    ) -> (Vec<Value>, Option<Value>, Vec<CatalogEntry>) {
        self.unknown(document, "", &DOCUMENT_MEMBERS);
        if document.get("schema_version").and_then(Value::as_str)
            != Some(STANDARD_PACK_CATALOG_SCHEMA_VERSION)
        {
            self.error(StandardPackProblem::Malformed, "/schema_version");
        }
        match document.get("game") {
            Some(Value::String(declared)) if declared == game => {}
            Some(Value::String(_)) => self.error(StandardPackProblem::GameMismatch, "/game"),
            _ => self.error(StandardPackProblem::Malformed, "/game"),
        }
        let (groups, ids) = match document.get("groups") {
            None => (Vec::new(), BTreeSet::new()),
            Some(groups) => self.groups(groups),
        };
        let display = document
            .get("display")
            .and_then(|display| self.display(display));
        let entries = match document.get("packs") {
            None => {
                self.error(StandardPackProblem::Malformed, "/packs");
                Vec::new()
            }
            Some(packs) => self.packs(packs, &ids),
        };
        (groups, display, entries)
    }

    /// `groups`: each `{id, title}`. The groups without a defect, as written, and the ids of every
    /// group whose id is a word given once (a package naming a group with a defective title still
    /// finds it).
    fn groups(&mut self, value: &Value) -> (Vec<Value>, BTreeSet<String>) {
        let mut kept = Vec::new();
        let mut ids = BTreeSet::new();
        let Some(items) = value.as_array() else {
            self.warn(StandardPackProblem::Malformed, "/groups");
            return (kept, ids);
        };
        if items.len() > MAX_PACK_CATALOG_GROUPS {
            self.warn(StandardPackProblem::CatalogOutOfLimits, "/groups");
        }
        for (index, group) in items.iter().enumerate() {
            let pointer = format!("/groups/{index}");
            let Some(object) = group.as_object() else {
                self.warn(StandardPackProblem::Malformed, pointer);
                continue;
            };
            self.unknown(object, &pointer, &GROUP_MEMBERS);
            let before = self.defects;
            let id = match object.get("id") {
                None => {
                    self.warn(StandardPackProblem::Malformed, at(&pointer, "id"));
                    None
                }
                Some(id) => self.word(id, &at(&pointer, "id")),
            };
            match object.get("title") {
                None => self.warn(
                    StandardPackProblem::CatalogTextMissing,
                    at(&pointer, "title"),
                ),
                Some(title) => {
                    self.text(title, &at(&pointer, "title"), MAX_PACK_CATALOG_TITLE_BYTES);
                }
            }
            if let Some(id) = id {
                if !ids.insert(id) {
                    self.warn(StandardPackProblem::CatalogDuplicate, at(&pointer, "id"));
                } else if self.defects == before {
                    kept.push(group.clone());
                }
            }
        }
        (kept, ids)
    }

    /// `display`: `{aspect_ratios, safe_area?}`, as written when it has no defect.
    fn display(&mut self, value: &Value) -> Option<Value> {
        const POINTER: &str = "/display";
        let Some(object) = value.as_object() else {
            self.warn(StandardPackProblem::Malformed, POINTER);
            return None;
        };
        self.unknown(object, POINTER, &DISPLAY_BLOCK_MEMBERS);
        let before = self.defects;
        match object.get("aspect_ratios") {
            None => self.warn(StandardPackProblem::Malformed, at(POINTER, "aspect_ratios")),
            Some(ratios) => self.aspect_ratios(ratios, &at(POINTER, "aspect_ratios")),
        }
        if let Some(safe_area) = object.get("safe_area") {
            self.word(safe_area, &at(POINTER, "safe_area"));
        }
        (self.defects == before).then(|| value.clone())
    }

    /// `display.aspect_ratios`: 1 to 8 distinct ratios of the form `<w>:<h>`.
    fn aspect_ratios(&mut self, value: &Value, pointer: &str) {
        let Some(items) = value.as_array() else {
            self.warn(StandardPackProblem::Malformed, pointer);
            return;
        };
        if !(1..=MAX_PACK_CATALOG_ASPECT_RATIOS).contains(&items.len()) {
            self.warn(StandardPackProblem::CatalogOutOfLimits, pointer);
        }
        let mut seen = BTreeSet::new();
        for (index, ratio) in items.iter().enumerate() {
            let at_ratio = format!("{pointer}/{index}");
            match ratio.as_str() {
                Some(ratio) if aspect_ratio(ratio) => {
                    if !seen.insert(ratio.to_owned()) {
                        self.warn(StandardPackProblem::CatalogDuplicate, at_ratio);
                    }
                }
                _ => self.warn(StandardPackProblem::Malformed, at_ratio),
            }
        }
    }

    /// `packs`: one entry per package of the registry, `{package_id, consumes, produces}` and the
    /// display members. Every entry whose `package_id` is a string is answered (once per id), so
    /// that the writer can drop the ones the registry no longer holds.
    fn packs(&mut self, value: &Value, groups: &BTreeSet<String>) -> Vec<CatalogEntry> {
        let mut entries = Vec::new();
        let Some(items) = value.as_array() else {
            self.error(StandardPackProblem::Malformed, "/packs");
            return entries;
        };
        if items.len() > MAX_PACK_CATALOG_PACKS {
            self.error(StandardPackProblem::CatalogOutOfLimits, "/packs");
        }
        let mut listed = BTreeSet::new();
        for (index, pack) in items.iter().enumerate() {
            let pointer = format!("/packs/{index}");
            let Some(object) = pack.as_object() else {
                self.error(StandardPackProblem::Malformed, pointer);
                continue;
            };
            self.unknown(object, &pointer, &ENTRY_MEMBERS);
            let at_id = at(&pointer, "package_id");
            let package_id = match object.get("package_id") {
                Some(Value::String(package_id)) => {
                    if self.registry.get(package_id).is_none() {
                        self.error(StandardPackProblem::PackageNotListed, at_id.clone());
                    }
                    if listed.insert(package_id.clone()) {
                        Some(package_id.clone())
                    } else {
                        self.error(StandardPackProblem::CatalogDuplicate, at_id);
                        None
                    }
                }
                _ => {
                    self.error(StandardPackProblem::Malformed, at_id);
                    None
                }
            };
            let consumes = self.pools(object.get("consumes"), &at(&pointer, "consumes"));
            let produces = self.pools(object.get("produces"), &at(&pointer, "produces"));
            let display = self.entry_display(object, &pointer, groups);
            if let Some(package_id) = package_id {
                entries.push(CatalogEntry {
                    package_id,
                    display,
                    business: CatalogBusiness { consumes, produces },
                });
            }
        }
        entries
    }

    /// One side of an entry's business part at `pointer`: a list of up to 256 pools, each
    /// `{pool_id, quantity?}`, each pool once, each a pool the root's `schedule.json` declares.
    fn pools(&mut self, value: Option<&Value>, pointer: &str) -> Vec<CatalogPool> {
        let mut pools = Vec::new();
        let Some(items) = value.and_then(Value::as_array) else {
            self.error(StandardPackProblem::Malformed, pointer);
            return pools;
        };
        if items.len() > MAX_PACK_CATALOG_POOLS {
            self.error(StandardPackProblem::CatalogOutOfLimits, pointer);
        }
        let mut seen = BTreeSet::new();
        for (index, item) in items.iter().enumerate() {
            let at_item = format!("{pointer}/{index}");
            let Some(object) = item.as_object() else {
                self.error(StandardPackProblem::Malformed, at_item);
                continue;
            };
            self.unknown(object, &at_item, &POOL_MEMBERS);
            let at_pool = at(&at_item, "pool_id");
            let pool_id = match object.get("pool_id").and_then(Value::as_str) {
                None => {
                    self.error(StandardPackProblem::Malformed, at_pool);
                    continue;
                }
                Some(pool_id)
                    if pool_id.is_empty() || pool_id.len() > MAX_PACK_CATALOG_POOL_ID_BYTES =>
                {
                    self.error(StandardPackProblem::CatalogOutOfLimits, at_pool);
                    continue;
                }
                Some(pool_id) => pool_id.to_owned(),
            };
            if self
                .declared_pools
                .is_some_and(|declared| !declared.contains(&pool_id))
            {
                self.error(StandardPackProblem::CatalogPoolUndeclared, at_pool.clone());
            }
            if !seen.insert(pool_id.clone()) {
                self.error(StandardPackProblem::CatalogDuplicate, at_pool);
                continue;
            }
            let quantity = match object.get("quantity") {
                None => None,
                Some(quantity) => self.quantity(quantity, &at(&at_item, "quantity")),
            };
            pools.push(CatalogPool { pool_id, quantity });
        }
        pools
    }

    /// A quantity at `pointer`: exactly `{"kind": "constant", "amount": N}` with N a whole number
    /// of 1 to [`MAX_PACK_CATALOG_AMOUNT`], or exactly `{"kind": "pending"}`.
    fn quantity(&mut self, value: &Value, pointer: &str) -> Option<CatalogQuantity> {
        let Some(object) = value.as_object() else {
            self.error(StandardPackProblem::Malformed, pointer);
            return None;
        };
        self.unknown(object, pointer, &QUANTITY_MEMBERS);
        let amount = object.get("amount");
        match object.get("kind").and_then(Value::as_str) {
            Some(KIND_CONSTANT) => match amount.and_then(Value::as_u64) {
                Some(amount) if (1..=MAX_PACK_CATALOG_AMOUNT).contains(&amount) => {
                    Some(CatalogQuantity::Constant { amount })
                }
                _ => {
                    self.error(StandardPackProblem::Malformed, at(pointer, "amount"));
                    None
                }
            },
            Some(KIND_PENDING) if amount.is_none() => Some(CatalogQuantity::Pending),
            Some(KIND_PENDING) => {
                self.error(StandardPackProblem::Malformed, at(pointer, "amount"));
                None
            }
            _ => {
                self.error(StandardPackProblem::Malformed, at(pointer, "kind"));
                None
            }
        }
    }

    /// The display of the entry `object` at `pointer`: `None` when it writes no display member;
    /// otherwise its display members as written when none has a defect. A package that writes one
    /// writes `title` and `summary` (`catalog_text_missing` otherwise); its `group` is a group's
    /// id; its `risk` up to 8 distinct words.
    fn entry_display(
        &mut self,
        object: &Map<String, Value>,
        pointer: &str,
        groups: &BTreeSet<String>,
    ) -> Option<Map<String, Value>> {
        if !DISPLAY_MEMBERS
            .iter()
            .any(|member| object.contains_key(*member))
        {
            return None;
        }
        let before = self.defects;
        for (member, max) in [
            ("title", MAX_PACK_CATALOG_TITLE_BYTES),
            ("summary", MAX_PACK_CATALOG_SUMMARY_BYTES),
        ] {
            match object.get(member) {
                None => self.warn(StandardPackProblem::CatalogTextMissing, at(pointer, member)),
                Some(text) => self.text(text, &at(pointer, member), max),
            }
        }
        if let Some(group) = object.get("group") {
            let at_group = at(pointer, "group");
            if let Some(group) = self.word(group, &at_group)
                && !groups.contains(&group)
            {
                self.warn(StandardPackProblem::CatalogGroupUnknown, at_group);
            }
        }
        if let Some(risk) = object.get("risk") {
            self.words(risk, &at(pointer, "risk"), MAX_PACK_CATALOG_RISK_WORDS);
        }
        (self.defects == before).then(|| {
            object
                .iter()
                .filter(|(name, _)| {
                    name.as_str() == "package_id" || DISPLAY_MEMBERS.contains(&name.as_str())
                })
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect()
        })
    }

    /// A display text at `pointer`: 1 to 16 locales, each name 1 to 35 bytes, each text a string
    /// of 1 to `max` bytes.
    fn text(&mut self, value: &Value, pointer: &str, max: usize) {
        let Some(locales) = value.as_object() else {
            self.warn(StandardPackProblem::Malformed, pointer);
            return;
        };
        if !(1..=MAX_PACK_CATALOG_LOCALES).contains(&locales.len()) {
            self.warn(StandardPackProblem::CatalogOutOfLimits, pointer);
        }
        for (locale, text) in locales {
            let at_locale = at(pointer, locale);
            if locale.is_empty() || locale.len() > MAX_PACK_CATALOG_LOCALE_BYTES {
                self.warn(StandardPackProblem::CatalogOutOfLimits, at_locale.clone());
            }
            match text.as_str() {
                None => self.warn(StandardPackProblem::Malformed, at_locale),
                Some(text) if text.is_empty() || text.len() > max => {
                    self.warn(StandardPackProblem::CatalogOutOfLimits, at_locale);
                }
                Some(_) => {}
            }
        }
    }

    /// A display word at `pointer`: a string of 1 to 64 bytes; `None`, with its warning,
    /// otherwise.
    fn word(&mut self, value: &Value, pointer: &str) -> Option<String> {
        match value.as_str() {
            None => {
                self.warn(StandardPackProblem::Malformed, pointer);
                None
            }
            Some(word) if word.is_empty() || word.len() > MAX_PACK_CATALOG_WORD_BYTES => {
                self.warn(StandardPackProblem::CatalogOutOfLimits, pointer);
                None
            }
            Some(word) => Some(word.to_owned()),
        }
    }

    /// A list of up to `max` distinct display words at `pointer`.
    fn words(&mut self, value: &Value, pointer: &str, max: usize) {
        let Some(items) = value.as_array() else {
            self.warn(StandardPackProblem::Malformed, pointer);
            return;
        };
        if items.len() > max {
            self.warn(StandardPackProblem::CatalogOutOfLimits, pointer);
        }
        let mut seen = BTreeSet::new();
        for (index, item) in items.iter().enumerate() {
            let at_item = format!("{pointer}/{index}");
            if let Some(word) = self.word(item, &at_item)
                && !seen.insert(word)
            {
                self.warn(StandardPackProblem::CatalogDuplicate, at_item);
            }
        }
    }
}

/// Whether `text` is of the form `<w>:<h>`: two positive decimal integers of 1 to
/// [`MAX_PACK_CATALOG_ASPECT_DIGITS`] digits without a leading zero. Only the form is checked.
fn aspect_ratio(text: &str) -> bool {
    text.split_once(':').is_some_and(|(width, height)| {
        [width, height].iter().all(|side| {
            (1..=MAX_PACK_CATALOG_ASPECT_DIGITS).contains(&side.len())
                && side.bytes().all(|byte| byte.is_ascii_digit())
                && !side.starts_with('0')
        })
    })
}

/// The pointer of the member `name` of the object at `pointer` (RFC 6901).
pub(crate) fn at(pointer: &str, name: &str) -> String {
    format!("{pointer}/{}", name.replace('~', "~0").replace('/', "~1"))
}

/// A vocabulary token as its spelling.
fn token<S: Serializer, T: VocabularyToken>(value: &T, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str((*value).token())
}

/// One difference between a package's entry and the rules of `schedule.json` that run it
/// ([`standard_pack_catalog_differences`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CatalogScheduleDifference {
    pub package_id: String,
    pub pool_id: String,
    #[serde(serialize_with = "token")]
    pub side: CatalogSide,
    #[serde(serialize_with = "token")]
    pub difference: CatalogDifference,
    /// Of `catalog_lacks`, the rule of `schedule.json` that names the pool; of `schedule_lacks`,
    /// none (no rule names it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
}

impl CatalogScheduleDifference {
    /// The warning `standard_pack_catalog_schedule_differs` of the game root of `game`: the
    /// package, the pool, the side, which file lacks it and, when a rule names it, the rule
    /// (`task_id`).
    pub fn outcome(&self, game: &str) -> Outcome {
        let outcome = Outcome::new(SharedCode::StandardPackCatalogScheduleDiffers)
            .game(game)
            .package_id(self.package_id.as_str())
            .pool_id(self.pool_id.as_str())
            .catalog_side(self.side)
            .catalog_difference(self.difference);
        match &self.rule_id {
            Some(rule_id) => outcome.task_id(rule_id.as_str()),
            None => outcome,
        }
    }
}

/// The warning `standard_pack_catalog_display_invalid` of a display defect `issue` of the
/// `pack-catalog.json` at `path` of the game root of `game`.
pub fn catalog_display_outcome(game: &str, path: &Path, issue: &StandardPackRootIssue) -> Outcome {
    Outcome::new(SharedCode::StandardPackCatalogDisplayInvalid)
        .game(game)
        .path(path)
        .standard_pack_problem(issue.problem)
        .json_pointer(issue.pointer.clone())
}

/// The warning `standard_pack_ui_retired` of the game root of `game` that still holds `ui.json`
/// at `path`: no program reads that file any more; its content belongs in the display part of
/// `pack-catalog.json`, which `standard-pack catalog` takes from it.
pub fn retired_ui_outcome(game: &str, path: &Path) -> Outcome {
    Outcome::new(SharedCode::StandardPackUiRetired)
        .game(game)
        .path(path)
}

/// The pools `rule` names on `side` (its member `consumes` or `produces`), each once, in its order,
/// with the amount the rule states for it: its `amount` when that is a whole number of 1 to
/// [`MAX_PACK_CATALOG_AMOUNT`], `None` for any other (pending, read from a fact, absent).
fn rule_pools(rule: &ScheduleRule, side: CatalogSide) -> Vec<(String, Option<u64>)> {
    let mut pools: Vec<(String, Option<u64>)> = Vec::new();
    for item in rule
        .body
        .get(side.as_str())
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(pool_id) = item.get("pool_id").and_then(Value::as_str) else {
            continue;
        };
        if pools.iter().any(|(known, _)| known.as_str() == pool_id) {
            continue;
        }
        let amount = item
            .get("amount")
            .and_then(Value::as_u64)
            .filter(|amount| (1..=MAX_PACK_CATALOG_AMOUNT).contains(amount));
        pools.push((pool_id.to_owned(), amount));
    }
    pools
}

/// The rules of `schedule` that run `package_id`, in the file's order.
fn package_rules<'s>(
    schedule: Option<&'s StandardPackSchedule>,
    package_id: &str,
) -> Vec<&'s ScheduleRule> {
    schedule
        .into_iter()
        .flat_map(|schedule| &schedule.rules)
        .filter(|rule| rule.package_id == package_id)
        .collect()
}

/// Every difference between the business part of each entry of `catalog` and the rules of
/// `schedule` that run its package (none for a root without `schedule.json`: then every listed
/// pool is `schedule_lacks`), entry by entry in the file's order, `consumes` before `produces`: on
/// each side, first each pool a rule names that the entry does not list (`catalog_lacks`, naming
/// the rule, in rule order, each pool once per rule), then each pool the entry lists that no rule
/// of the package names on that side (`schedule_lacks`). A package the file lists no entry for is
/// not compared.
pub fn standard_pack_catalog_differences(
    catalog: &StandardPackCatalog,
    schedule: Option<&StandardPackSchedule>,
) -> Vec<CatalogScheduleDifference> {
    let mut differences = Vec::new();
    for entry in &catalog.entries {
        let rules = package_rules(schedule, &entry.package_id);
        for side in CatalogSide::ALL.iter().copied() {
            let listed = entry
                .business
                .side(side)
                .iter()
                .map(|pool| pool.pool_id.as_str())
                .collect::<BTreeSet<_>>();
            let mut named = BTreeSet::new();
            for rule in &rules {
                for (pool_id, _) in rule_pools(rule, side) {
                    named.insert(pool_id.clone());
                    if !listed.contains(pool_id.as_str()) {
                        differences.push(CatalogScheduleDifference {
                            package_id: entry.package_id.clone(),
                            pool_id,
                            side,
                            difference: CatalogDifference::CatalogLacks,
                            rule_id: Some(rule.id.clone()),
                        });
                    }
                }
            }
            for pool in entry.business.side(side) {
                if !named.contains(&pool.pool_id) {
                    differences.push(CatalogScheduleDifference {
                        package_id: entry.package_id.clone(),
                        pool_id: pool.pool_id.clone(),
                        side,
                        difference: CatalogDifference::ScheduleLacks,
                        rule_id: None,
                    });
                }
            }
        }
    }
    differences
}

/// What the rules of `package_id` in `schedule` name on `side`: each pool once, in the order the
/// rules first name it, with a constant quantity when every rule that names it states the same
/// amount ([`rule_pools`]), else pending.
fn schedule_union(
    schedule: Option<&StandardPackSchedule>,
    package_id: &str,
    side: CatalogSide,
) -> Vec<CatalogPool> {
    let mut union: Vec<(String, Vec<Option<u64>>)> = Vec::new();
    for rule in package_rules(schedule, package_id) {
        for (pool_id, amount) in rule_pools(rule, side) {
            match union.iter_mut().find(|(known, _)| *known == pool_id) {
                Some((_, amounts)) => amounts.push(amount),
                None => union.push((pool_id, vec![amount])),
            }
        }
    }
    union
        .into_iter()
        .map(|(pool_id, amounts)| {
            let first = amounts.first().copied().flatten();
            let quantity = match first {
                Some(amount) if amounts.iter().all(|other| *other == Some(amount)) => {
                    CatalogQuantity::Constant { amount }
                }
                _ => CatalogQuantity::Pending,
            };
            CatalogPool {
                pool_id,
                quantity: Some(quantity),
            }
        })
        .collect()
}

/// A pool of the business part as the file writes it.
fn pool_value(pool: &CatalogPool) -> Value {
    let mut value = Map::new();
    value.insert("pool_id".to_owned(), Value::String(pool.pool_id.clone()));
    if let Some(quantity) = pool.quantity {
        let quantity = match quantity {
            CatalogQuantity::Constant { amount } => {
                json!({"kind": KIND_CONSTANT, "amount": amount})
            }
            CatalogQuantity::Pending => json!({"kind": KIND_PENDING}),
        };
        value.insert("quantity".to_owned(), quantity);
    }
    Value::Object(value)
}

/// A pool a kept entry listed that `schedule.json` no longer declares, which the writer left out
/// and reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CatalogDropped {
    pub package_id: String,
    #[serde(serialize_with = "token")]
    pub side: CatalogSide,
    pub pool_id: String,
}

/// What [`merge_standard_pack_catalog`] makes of a game root.
#[derive(Debug, Clone, PartialEq)]
pub struct StandardPackCatalogMerge {
    /// The file the writer writes: the document, two-space indented, and a line feed.
    pub bytes: Vec<u8>,
    /// Those bytes, as the reader reads them.
    pub catalog: StandardPackCatalog,
    /// The packages given a new entry, in the registry's order.
    pub added: Vec<String>,
    /// The packages whose entry was there and is kept, in the file's order.
    pub kept: Vec<String>,
    /// The entries of packages `bundle.json` no longer registers, left out.
    pub removed: Vec<String>,
    /// The pools of kept entries that `schedule.json` no longer declares, left out.
    pub dropped: Vec<CatalogDropped>,
    /// Every difference between the written entries and the rules of `schedule.json`
    /// ([`standard_pack_catalog_differences`]); a new entry has none, so these are the kept
    /// entries'.
    pub differences: Vec<CatalogScheduleDifference>,
    /// The new entries whose display came from the leftover `ui.json`.
    pub display_from_ui: Vec<String>,
}

/// Workflow #477 PC-1: the writer's one rule (`standard-pack catalog`; `contracts/pack-catalog.md`,
/// "Who writes it"): the `pack-catalog.json` the game root of `game` gets from what it already
/// declares, its `registry` (`bundle.json`), its `schedule` (`schedule.json`, when it holds one),
/// the catalog it holds (`existing`, its bytes) and a leftover `ui.json` (`retired_ui`, read as JSON
/// when it is).
///
/// - An entry for every package of the registry: the existing entries first, in the file's order,
///   each kept as written (its display and its business part), then one new entry per package
///   without one, in the registry's order.
/// - A new entry's display comes from the leftover `ui.json`'s entry for the package (its `title`,
///   `summary`, `group` and `risk` as written), else it has none; its `consumes` and `produces` are
///   the union over the package's rules ([`schedule_union`]): a constant quantity when every rule
///   states the same constant, else pending.
/// - `groups` and `display` are the existing catalog's when it writes them, else the leftover
///   `ui.json`'s, else absent.
/// - An existing entry's business part is never overwritten: its differences from the rules are
///   reported ([`StandardPackCatalogMerge::differences`]). What no longer holds is left out and
///   reported: the entry of a package the registry no longer holds
///   ([`StandardPackCatalogMerge::removed`]) and a pool `schedule.json` no longer declares
///   ([`StandardPackCatalogMerge::dropped`]).
///
/// An existing catalog the reader refuses for anything but those two references is not brought up
/// to date: its errors are the answer. The written bytes are read back by the reader, and its
/// errors (none, for a merge that holds) are the answer too.
pub fn merge_standard_pack_catalog(
    game: &str,
    registry: &StandardPackRegistry,
    schedule: Option<&StandardPackSchedule>,
    existing: Option<&[u8]>,
    retired_ui: Option<&Value>,
) -> Result<StandardPackCatalogMerge, Vec<StandardPackRootIssue>> {
    let pools = schedule
        .map(standard_pack_schedule_pools)
        .unwrap_or_default();
    let existing = match existing {
        None => None,
        Some(bytes) => {
            let read = read_catalog(bytes, game, registry, Some(&pools));
            let refused = read
                .errors
                .iter()
                .filter(|issue| {
                    !matches!(
                        issue.problem,
                        StandardPackProblem::PackageNotListed
                            | StandardPackProblem::CatalogPoolUndeclared
                    )
                })
                .cloned()
                .collect::<Vec<_>>();
            if !refused.is_empty() {
                return Err(refused);
            }
            read.document
        }
    };
    let mut kept = Vec::new();
    let mut removed = Vec::new();
    let mut dropped = Vec::new();
    let mut packs = Vec::new();
    let mut listed = BTreeSet::new();
    for entry in existing
        .as_ref()
        .and_then(|document| document.get("packs"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(object) = entry.as_object() else {
            continue;
        };
        let Some(package_id) = object.get("package_id").and_then(Value::as_str) else {
            continue;
        };
        if registry.get(package_id).is_none() {
            removed.push(package_id.to_owned());
            continue;
        }
        let mut written = object.clone();
        for side in CatalogSide::ALL.iter().copied() {
            if let Some(Value::Array(items)) = written.get_mut(side.as_str()) {
                items.retain(|item| {
                    let pool_id = item
                        .get("pool_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let declared = pools.contains(pool_id);
                    if !declared {
                        dropped.push(CatalogDropped {
                            package_id: package_id.to_owned(),
                            side,
                            pool_id: pool_id.to_owned(),
                        });
                    }
                    declared
                });
            }
        }
        listed.insert(package_id.to_owned());
        kept.push(package_id.to_owned());
        packs.push(Value::Object(written));
    }
    let ui_packs = retired_ui
        .and_then(|ui| ui.get("packs"))
        .and_then(Value::as_array);
    let mut added = Vec::new();
    let mut display_from_ui = Vec::new();
    for package_id in registry.package_ids() {
        if listed.contains(package_id) {
            continue;
        }
        let mut entry = Map::new();
        entry.insert(
            "package_id".to_owned(),
            Value::String(package_id.to_owned()),
        );
        let display = ui_packs
            .into_iter()
            .flatten()
            .find(|pack| pack.get("package_id").and_then(Value::as_str) == Some(package_id))
            .and_then(Value::as_object);
        if let Some(display) = display {
            let mut taken = false;
            for member in DISPLAY_MEMBERS {
                if let Some(value) = display.get(member) {
                    entry.insert(member.to_owned(), value.clone());
                    taken = true;
                }
            }
            if taken {
                display_from_ui.push(package_id.to_owned());
            }
        }
        for side in CatalogSide::ALL.iter().copied() {
            let union = schedule_union(schedule, package_id, side);
            entry.insert(
                side.as_str().to_owned(),
                Value::Array(union.iter().map(pool_value).collect()),
            );
        }
        added.push(package_id.to_owned());
        packs.push(Value::Object(entry));
    }
    let taken = |member: &str| {
        existing
            .as_ref()
            .and_then(|document| document.get(member))
            .or_else(|| retired_ui.and_then(|ui| ui.get(member)))
            .cloned()
    };
    let mut document = Map::new();
    document.insert(
        "schema_version".to_owned(),
        Value::String(STANDARD_PACK_CATALOG_SCHEMA_VERSION.to_owned()),
    );
    document.insert("game".to_owned(), Value::String(game.to_owned()));
    for member in ["groups", "display"] {
        if let Some(value) = taken(member) {
            document.insert(member.to_owned(), value);
        }
    }
    document.insert("packs".to_owned(), Value::Array(packs));
    let mut bytes = serde_json::to_vec_pretty(&Value::Object(document)).map_err(|_| {
        vec![StandardPackRootIssue::new(
            STANDARD_PACK_CATALOG_FILE,
            StandardPackProblem::Malformed,
            "",
        )]
    })?;
    bytes.push(b'\n');
    let catalog = parse_standard_pack_catalog(&bytes, game, registry, Some(&pools))?;
    let differences = standard_pack_catalog_differences(&catalog, schedule);
    Ok(StandardPackCatalogMerge {
        bytes,
        catalog,
        added,
        kept,
        removed,
        dropped,
        differences,
        display_from_ui,
    })
}

#[cfg(test)]
#[path = "standard_pack_catalog_tests.rs"]
mod tests;
