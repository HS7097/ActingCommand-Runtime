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
//!   value member in this version: a member the reader does not know at the document's own level,
//!   on an entry, on a pool or on a quantity is an error at its pointer, so that a later version
//!   adds the value under its own schema.
//!
//! [`parse_standard_pack_catalog`] reads it. A defect of the file as a whole or of its business
//! part is an error (`standard_pack_root_invalid` where a tool reports it): the root's declaration
//! is wrong. A defect of the display part is a warning
//! ([`StandardPackCatalog::display_issues`]): the group, the `display` block or the package's
//! display it touches is left out, and the package shows with its id, as R-11 fell back. Fix round
//! 1 (the owner's answer 6): a member a group or `display` does not know is such a display defect,
//! not an error. The index both ways ([`StandardPackCatalog::index`]) is computed by the reader and
//! never stored. [`standard_pack_catalog_differences`] compares the business part with the rules
//! of `schedule.json`, and [`merge_standard_pack_catalog`] is the writer's one rule.
//!
//! The Runtime never reads the file: only the Lab's tools (`packs`, `standard-pack check`,
//! `standard-pack catalog`, `standard-pack build` and the installer's `place`) do. Every text,
//! word, locale, group id and risk word is opaque: compared only for equality and measured only in
//! bytes; an aspect ratio is only checked to be of the form `<w>:<h>`.
//!
//! Workflow #477 PC-2 (register §1.462): a line of the writer may also name packages whose entry's
//! business part is computed again from their rules, by the rule a new entry is made by, its
//! display and its place kept (`refresh`, [`CatalogEdits::refresh`]), and give a display file whose
//! entries replace the named entries' displays whole, checked by the reader's own display checks
//! before anything is written ([`CatalogEdits::displays`]). Both go through
//! [`merge_standard_pack_catalog`], the one rule; an entry no line names is never overwritten.
//! Fix round 1 (the coordinator's ruling on groups): the display file
//! ([`STANDARD_PACK_CATALOG_DISPLAYS_SCHEMA_VERSION`]) may carry groups, which the line adds to the
//! catalog's `groups`, never removing or renaming one; and an issue's pointer the key table would
//! not take (a very long member or locale name) is cut to its entry's ([`issue_pointer`]).

use crate::codes::SharedCode;
use crate::outcome::{KeyKind, Outcome, VocabularyToken, key_text_fits};
use crate::standard_pack::{
    STANDARD_PACK_BUNDLE_FILE, StandardPackProblem, StandardPackRegistry, StandardPackRoot,
    StandardPackRootBytes, StandardPackRootIssue, read_standard_pack_root,
};
use crate::standard_pack_schedule::{
    STANDARD_PACK_SCHEDULE_FILE, ScheduleRule, StandardPackSchedule,
};
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::{Serialize, Serializer};
use serde_json::{Map, Number, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

/// The game root's package declaration (Workflow #477 PC-1).
pub const STANDARD_PACK_CATALOG_FILE: &str = "pack-catalog.json";
/// The one version of `pack-catalog.json` this build reads and writes.
pub const STANDARD_PACK_CATALOG_SCHEMA_VERSION: &str = "actingcommand.pack-catalog.v1";
/// Workflow #477 PC-2 fix round 1: the one version of the display file `standard-pack catalog
/// --displays` reads (an input of one line, never stored); a file without it or with another is
/// refused before anything else of it is read.
pub const STANDARD_PACK_CATALOG_DISPLAYS_SCHEMA_VERSION: &str =
    "actingcommand.pack-catalog-displays.v1";
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
/// Fix round 1 (`contracts/component-interfaces.md`, the `package` row 16): the `package` revision a
/// bundle that carries `pack-catalog.json` declares, so that an installer of an earlier build, which
/// knows no such root file, refuses the bundle at its `package` edge and not as a member it does
/// not know.
pub const CATALOG_PACKAGE_REVISION: u64 = 16;

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
/// Workflow #477 PC-2: the members of the display file's document (fix round 1: its
/// `schema_version` and the `groups` it adds), and of one of its entries (a catalog entry's
/// `package_id` and display members; no business member).
const DISPLAY_FILE_MEMBERS: [&str; 3] = ["schema_version", "groups", "packs"];
const DISPLAY_FILE_ENTRY_MEMBERS: [&str; 5] = ["package_id", "title", "summary", "group", "risk"];

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

crate::outcome_vocabulary! {
    /// Why `standard-pack catalog` refuses a package its `refresh` names, before anything is
    /// written (vocabulary `catalog_refresh_problem`, Workflow #477 PC-2).
    pub enum CatalogRefreshProblem: "catalog_refresh_problem" {
        /// The root's registry (its `bundle.json`, or the one a checkout's packages make) does not
        /// hold the package. Fix round 1 (one cause, one word): the word `standard_pack_problem`
        /// has for the same cause.
        PackageNotListed => "package_not_listed",
        /// `refresh` names the package more than once.
        PackageRepeated => "package_repeated",
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
///   not know at the document's own level, on an entry, on a pool or on a quantity (a value member
///   included), a business or document member that is missing or not of its type, a
///   `schema_version` other than
///   [`STANDARD_PACK_CATALOG_SCHEMA_VERSION`], a quantity that is not exactly one of its two forms;
/// - `game_mismatch`: `game` is not the directory's;
/// - `package_not_listed`: a `package_id` the registry does not hold;
/// - `catalog_duplicate`: a `package_id` listed twice, or a pool listed twice on one side of an
///   entry;
/// - `catalog_pool_undeclared`: a `pool_id` the root's `schedule.json` does not declare.
///
/// A file without an error reads ([`StandardPackCatalog`]); its display defects are warnings
/// ([`StandardPackCatalog::display_issues`]): `malformed` (a display member not of its type, a
/// member a group or `display` does not know (fix round 1), an aspect ratio not `<w>:<h>`),
/// `catalog_out_of_limits` (a list, a text, a locale or a word
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
    /// An error at `pointer` ([`issue_pointer`]: fix round 1, cut to its entry's when the key
    /// table would not take it).
    fn error(&mut self, problem: StandardPackProblem, pointer: impl Into<String>) {
        if self.errors.len() < MAX_STANDARD_PACK_CATALOG_ISSUES {
            self.errors.push(StandardPackRootIssue::new(
                STANDARD_PACK_CATALOG_FILE,
                problem,
                issue_pointer(pointer.into()),
            ));
        }
    }

    /// A display defect at `pointer`, as [`Self::error`] places it.
    fn warn(&mut self, problem: StandardPackProblem, pointer: impl Into<String>) {
        self.defects += 1;
        if self.warnings.len() < MAX_STANDARD_PACK_CATALOG_ISSUES {
            self.warnings.push(StandardPackRootIssue::new(
                STANDARD_PACK_CATALOG_FILE,
                problem,
                issue_pointer(pointer.into()),
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
    /// pointer, an error: the reader knows every member it reads at the document's own level, on
    /// an entry, on a pool and on a quantity.
    fn unknown(&mut self, object: &Map<String, Value>, pointer: &str, known: &[&str]) {
        for name in object.keys() {
            if !known.contains(&name.as_str()) {
                self.error(StandardPackProblem::Malformed, at(pointer, name));
            }
        }
    }

    /// Fix round 1 (the owner's answer 6): every member of the display object `object` (a group or
    /// `display`, at `pointer`) that is not one of `known` is `malformed` at its pointer, a
    /// warning: the object is left out and everything still runs.
    fn unknown_display(&mut self, object: &Map<String, Value>, pointer: &str, known: &[&str]) {
        for name in object.keys() {
            if !known.contains(&name.as_str()) {
                self.warn(StandardPackProblem::Malformed, at(pointer, name));
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
    /// group whose id is a word given once (a package naming a group with a defective title, or
    /// with a member a group does not know, still finds it).
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
            let before = self.defects;
            self.unknown_display(object, &pointer, &GROUP_MEMBERS);
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
        let before = self.defects;
        self.unknown_display(object, POINTER, &DISPLAY_BLOCK_MEMBERS);
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

    /// Workflow #477 PC-2: the display file `{schema_version, groups?, packs: [{package_id, title?,
    /// summary?, group?, risk?}]}`, each entry in the catalog's own shape without its business
    /// part, checked against `written`, the groups of the file the writer writes. Its size and its
    /// JSON are read as the catalog's are; fix round 1: then a `schema_version` other than
    /// [`STANDARD_PACK_CATALOG_DISPLAYS_SCHEMA_VERSION`] (or none) is `malformed` at it, and nothing
    /// else of the file is read. The rest is read as the catalog and its entries are: errors here
    /// are a repeated member, a member the reader does not know, `packs` and its limit, a
    /// `package_id` that is no string (`malformed` or `catalog_out_of_limits`), a `package_id` the
    /// registry does not hold (`package_not_listed`) and one named twice (`catalog_duplicate` at
    /// the later one). Fix round 1 (the coordinator's ruling on groups): the file's `groups` are
    /// read by [`Self::groups`], the reader's own group check; a group whose id `written` holds
    /// with another title is an error, `catalog_group_title_differs` at its `title` (the same
    /// title, compared as JSON values without the order of its locales, is simply there:
    /// [`same_title`]), and so are more groups than [`MAX_PACK_CATALOG_GROUPS`] once the file's new
    /// ones join `written`'s (`catalog_out_of_limits` at `/groups`). Each entry's display members
    /// are checked by [`Self::entry_display`], the reader's own display check, against the ids of
    /// `written` and of the file's groups together. Every display defect is left in the warnings.
    /// Every entry whose `package_id` is a string is answered, in the file's order, and so is each
    /// group without a defect whose id `written` does not hold (the groups it adds), in the file's
    /// order.
    fn display_file(&mut self, bytes: &[u8], written: &WrittenGroups<'_>) -> DisplayFile {
        let mut file = DisplayFile::default();
        if bytes.len() > MAX_STANDARD_PACK_CATALOG_BYTES {
            self.error(StandardPackProblem::CatalogOutOfLimits, "");
            return file;
        }
        let mut repeated = Vec::new();
        let Some(Value::Object(document)) = read_document(bytes, &mut repeated) else {
            self.error(StandardPackProblem::Malformed, "");
            return file;
        };
        if document.get("schema_version").and_then(Value::as_str)
            != Some(STANDARD_PACK_CATALOG_DISPLAYS_SCHEMA_VERSION)
        {
            self.error(StandardPackProblem::Malformed, "/schema_version");
            return file;
        }
        for pointer in repeated {
            self.error(StandardPackProblem::Malformed, pointer);
        }
        self.unknown(&document, "", &DISPLAY_FILE_MEMBERS);
        let mut groups = written.ids.clone();
        if let Some(value) = document.get("groups") {
            let (kept, ids) = self.groups(value);
            for (index, group) in value.as_array().into_iter().flatten().enumerate() {
                let Some(id) = group.get("id").and_then(Value::as_str) else {
                    continue;
                };
                if written.ids.contains(id) && !same_title(written.title(id), group.get("title")) {
                    self.error(
                        StandardPackProblem::CatalogGroupTitleDiffers,
                        at(&format!("/groups/{index}"), "title"),
                    );
                }
            }
            file.groups_added = kept
                .into_iter()
                .filter(|group| {
                    group
                        .get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| !written.ids.contains(id))
                })
                .collect();
            if written.items.len() + file.groups_added.len() > MAX_PACK_CATALOG_GROUPS {
                self.error(StandardPackProblem::CatalogOutOfLimits, "/groups");
            }
            groups.extend(ids);
        }
        let Some(items) = document.get("packs").and_then(Value::as_array) else {
            self.error(StandardPackProblem::Malformed, "/packs");
            return file;
        };
        if items.len() > MAX_PACK_CATALOG_PACKS {
            self.error(StandardPackProblem::CatalogOutOfLimits, "/packs");
        }
        let mut named = BTreeSet::new();
        for (index, item) in items.iter().enumerate() {
            let pointer = format!("/packs/{index}");
            let Some(object) = item.as_object() else {
                self.error(StandardPackProblem::Malformed, pointer);
                continue;
            };
            self.unknown(object, &pointer, &DISPLAY_FILE_ENTRY_MEMBERS);
            let at_id = at(&pointer, "package_id");
            let Some(package_id) = object.get("package_id").and_then(Value::as_str) else {
                self.error(StandardPackProblem::Malformed, at_id);
                continue;
            };
            if self.registry.get(package_id).is_none() {
                self.error(StandardPackProblem::PackageNotListed, at_id.clone());
            }
            if !named.insert(package_id) {
                self.error(StandardPackProblem::CatalogDuplicate, at_id);
            }
            let display = self.entry_display(object, &pointer, &groups);
            file.entries.push(DisplayFileEntry {
                package_id: package_id.to_owned(),
                display,
            });
        }
        file
    }
}

/// Workflow #477 PC-2 fix round 1 (the independent review): `pointer`, an issue's place in the file
/// it reads, as an outcome's `json_pointer` can carry it. A pointer the key table takes
/// ([`key_text_fits`]) is kept; a longer one (a very long member or locale name in it) is cut to
/// the entry it lies in, its first two reference tokens (`/packs/<i>`, `/groups/<i>`), else to the
/// member of the document it lies in (`/display`), else to the whole document (empty).
fn issue_pointer(pointer: String) -> String {
    if key_text_fits(KeyKind::Pointer, &pointer) {
        return pointer;
    }
    let cut = |tokens: usize| {
        pointer
            .match_indices('/')
            .nth(tokens)
            .map(|(end, _)| &pointer[..end])
    };
    [cut(2), cut(1)]
        .into_iter()
        .flatten()
        .find(|prefix| key_text_fits(KeyKind::Pointer, prefix))
        .unwrap_or_default()
        .to_owned()
}

/// Workflow #477 PC-2 fix round 1: whether two groups' `title`s are the same, compared as JSON
/// values without the order of their locales (a title absent on both sides is the same).
fn same_title(left: Option<&Value>, right: Option<&Value>) -> bool {
    match (left, right) {
        (Some(Value::Object(left)), Some(Value::Object(right))) => {
            left.len() == right.len()
                && left
                    .iter()
                    .all(|(locale, text)| right.get(locale) == Some(text))
        }
        (left, right) => left == right,
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

/// Fix round 1 (the independent review): each pool a rule of `schedule` names on a side
/// ([`rule_pools`] reads the same items) that `declared` does not hold, the rule's package one of
/// `fresh` (the packages the writer gives a new entry, whose pools are their rules'), as
/// `catalog_pool_undeclared` in `schedule.json` at the rule's pool
/// (`/rules/<i>/<side>/<j>/pool_id`, the reader keeping the rules and their items in the file's
/// order), in the file's order, at most [`MAX_STANDARD_PACK_CATALOG_ISSUES`].
fn undeclared_rule_pools(
    schedule: Option<&StandardPackSchedule>,
    declared: &BTreeSet<String>,
    fresh: &BTreeSet<&str>,
) -> Vec<StandardPackRootIssue> {
    let mut issues = Vec::new();
    let rules = schedule.into_iter().flat_map(|schedule| &schedule.rules);
    for (index, rule) in rules.enumerate() {
        if !fresh.contains(rule.package_id.as_str()) {
            continue;
        }
        for side in CatalogSide::ALL.iter().copied() {
            let items = rule.body.get(side.as_str()).and_then(Value::as_array);
            for (position, item) in items.into_iter().flatten().enumerate() {
                let Some(pool_id) = item.get("pool_id").and_then(Value::as_str) else {
                    continue;
                };
                if !declared.contains(pool_id) && issues.len() < MAX_STANDARD_PACK_CATALOG_ISSUES {
                    issues.push(StandardPackRootIssue::new(
                        STANDARD_PACK_SCHEDULE_FILE,
                        StandardPackProblem::CatalogPoolUndeclared,
                        format!("/rules/{index}/{}/{position}/pool_id", side.as_str()),
                    ));
                }
            }
        }
    }
    issues
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

/// Fix round 1 (the independent review): one thing [`merge_standard_pack_catalog`] left out of the
/// file it writes because the root no longer declares it, by the reader's word, at its pointer in
/// the existing file: the entry of a package `bundle.json` no longer registers
/// (`package_not_listed`, at its `package_id`; no pool), or a pool of a kept entry that
/// `schedule.json` no longer declares (`catalog_pool_undeclared`, at its `pool_id`). Each is also in
/// [`StandardPackCatalogMerge::removed`] or [`StandardPackCatalogMerge::dropped`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogLeftOut {
    pub problem: StandardPackProblem,
    pub package_id: String,
    /// The pool left out; `None` for an entry.
    pub pool_id: Option<String>,
    /// What was left out, in the existing file.
    pub pointer: String,
}

impl CatalogLeftOut {
    /// The warning `standard_pack_catalog_left_out` of the game root of `game` whose
    /// `pack-catalog.json` is at `path`: the problem, the pointer in the existing file, the
    /// package and, for a pool, the pool.
    pub fn outcome(&self, game: &str, path: &Path) -> Outcome {
        let outcome = Outcome::new(SharedCode::StandardPackCatalogLeftOut)
            .game(game)
            .path(path)
            .standard_pack_problem(self.problem)
            .json_pointer(self.pointer.clone())
            .package_id(self.package_id.as_str());
        match &self.pool_id {
            Some(pool_id) => outcome.pool_id(pool_id.as_str()),
            None => outcome,
        }
    }
}

/// Workflow #477 PC-2 (`contracts/pack-catalog.md`, "Who writes it"): what a line of
/// `standard-pack catalog` asks of [`merge_standard_pack_catalog`] beyond the plain rule. The
/// default asks nothing, and the merge is then the plain rule, byte for byte.
#[derive(Debug, Clone, Copy, Default)]
pub struct CatalogEdits<'a> {
    /// The packages whose entry's business part is computed again from their rules, by the rule a
    /// new entry is made by (`refresh`), in the order named; each one the registry holds, once.
    pub refresh: &'a [String],
    /// The display file whose entries replace the named entries' displays whole (`displays`).
    pub displays: Option<CatalogDisplayFile<'a>>,
}

/// Workflow #477 PC-2: the display file a line of `standard-pack catalog` gives: where it is,
/// which its refusals name, and its bytes.
#[derive(Debug, Clone, Copy)]
pub struct CatalogDisplayFile<'a> {
    pub path: &'a Path,
    pub bytes: &'a [u8],
}

/// Workflow #477 PC-2: a package `refresh` names that the line refuses, by its word, before
/// anything is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogRefreshRefusal {
    pub package_id: String,
    pub problem: CatalogRefreshProblem,
}

impl CatalogRefreshRefusal {
    /// The refusal `standard_pack_catalog_refresh_refused` of the game root of `game`: the
    /// package and why.
    pub fn outcome(&self, game: &str) -> Outcome {
        Outcome::new(SharedCode::StandardPackCatalogRefreshRefused)
            .game(game)
            .package_id(self.package_id.as_str())
            .catalog_refresh_problem(self.problem)
    }
}

/// Workflow #477 PC-2: one error of the display file, by the catalog's own word, at its JSON
/// pointer in that file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogDisplayFileIssue {
    pub problem: StandardPackProblem,
    pub pointer: String,
}

impl CatalogDisplayFileIssue {
    /// The refusal `standard_pack_catalog_display_refused` of the game root of `game` whose line
    /// gave the display file at `path`.
    pub fn outcome(&self, game: &str, path: &Path) -> Outcome {
        Outcome::new(SharedCode::StandardPackCatalogDisplayRefused)
            .game(game)
            .path(path)
            .standard_pack_problem(self.problem)
            .json_pointer(self.pointer.clone())
    }
}

/// Workflow #477 PC-2: why [`merge_standard_pack_catalog`] makes no file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogMergeRefusal {
    /// A package `refresh` names ([`CatalogEdits::refresh`]).
    Refresh(CatalogRefreshRefusal),
    /// The root's files (PC-1): the errors of its existing catalog, or each pool a rule names that
    /// `schedule.json` does not declare, each naming its file.
    Root(Vec<StandardPackRootIssue>),
    /// The display file ([`CatalogEdits::displays`]) at `path`: its errors, at their pointers in
    /// it, first those of its shape and its packages, then its display defects.
    Display {
        path: PathBuf,
        issues: Vec<CatalogDisplayFileIssue>,
    },
}

/// Workflow #477 PC-2: how a package `refresh` names stands after the merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogRefreshState {
    /// Its entry's business part differed from what its rules give and is replaced.
    Replaced,
    /// Its entry's business part is what its rules give already: the entry is left as written.
    Agrees,
    /// It had no entry: it gets the new entry the plain rule makes, which is its rules'.
    Added,
}

/// Workflow #477 PC-2: one package `refresh` names, with its entry's business part before and
/// after the merge, so that a reader sees the change without comparing files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CatalogRefreshed {
    pub package_id: String,
    pub state: CatalogRefreshState,
    /// The business part the existing entry wrote; absent for a package that had no entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<CatalogBusiness>,
    /// The business part its rules give, which the written entry holds.
    pub after: CatalogBusiness,
}

/// Workflow #477 PC-2: one entry of the display file: its package and its display members as
/// written, `None` when it writes none (the entry then shows by its id).
#[derive(Debug, Clone)]
struct DisplayFileEntry {
    package_id: String,
    display: Option<Map<String, Value>>,
}

/// Workflow #477 PC-2: what the display file gives: its entries, and (fix round 1) the groups it
/// adds, each as written, in its order.
#[derive(Debug, Clone, Default)]
struct DisplayFile {
    entries: Vec<DisplayFileEntry>,
    groups_added: Vec<Value>,
}

/// Workflow #477 PC-2 fix round 1: the groups of the file the writer writes, as the display file is
/// checked against them: the ids the reader takes of them ([`Check::groups`]) and the list as
/// written (empty when `groups` is absent or no list, which holds no group for the reader).
struct WrittenGroups<'v> {
    ids: BTreeSet<String>,
    items: &'v [Value],
}

impl WrittenGroups<'_> {
    /// The `title` the first group of `id` writes; `None` when it writes none, or when no group has
    /// the id.
    fn title(&self, id: &str) -> Option<&Value> {
        self.items
            .iter()
            .find(|group| group.get("id").and_then(Value::as_str) == Some(id))
            .and_then(|group| group.get("title"))
    }
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
    /// The packages whose entry was there and is kept in its place, in the file's order (PC-2: its
    /// business part as written unless [`Self::refreshed`] names it, its display as written unless
    /// [`Self::display_written`] does).
    pub kept: Vec<String>,
    /// The entries of packages `bundle.json` no longer registers, left out.
    pub removed: Vec<String>,
    /// The pools of kept entries that `schedule.json` no longer declares, left out.
    pub dropped: Vec<CatalogDropped>,
    /// Fix round 1: each entry of [`Self::removed`] and each pool of [`Self::dropped`] with its
    /// pointer in the existing file, in that file's order (a writer warns of each).
    pub left_out: Vec<CatalogLeftOut>,
    /// Every difference between the written entries and the rules of `schedule.json`
    /// ([`standard_pack_catalog_differences`]); a new entry has none, so these are the kept
    /// entries'.
    pub differences: Vec<CatalogScheduleDifference>,
    /// The new entries whose display came from the leftover `ui.json`.
    pub display_from_ui: Vec<String>,
    /// PC-2: each package [`CatalogEdits::refresh`] names, in the order named, its business part
    /// before and after; empty when none is named.
    pub refreshed: Vec<CatalogRefreshed>,
    /// PC-2: the packages whose display the display file replaced, in that file's order; empty
    /// without one.
    pub display_written: Vec<String>,
    /// PC-2 fix round 1: the ids of the groups the display file added to `groups`, in that file's
    /// order; empty without one, or when each of its groups was there already.
    pub groups_added: Vec<String>,
}

/// Workflow #477 PC-1: the writer's one rule (`standard-pack catalog`; `contracts/pack-catalog.md`,
/// "Who writes it"): the `pack-catalog.json` the game root of `game` gets from what it already
/// declares, its `registry` (`bundle.json`), its `schedule` (`schedule.json`, when it holds one),
/// the catalog it holds (`existing`, its bytes) and a leftover `ui.json` (`retired_ui`, read as JSON
/// when it is).
///
/// - An entry for every package of the registry: the existing entries first, in the file's order,
///   each kept as written (its display and its business part) unless `edits` names it (PC-2,
///   below), then one new entry per package without one, in the registry's order.
/// - A new entry's display comes from the leftover `ui.json`'s entry for the package (its `title`,
///   `summary`, `group` and `risk` as written), else it has none; its `consumes` and `produces` are
///   the union over the package's rules ([`schedule_union`]): a constant quantity when every rule
///   states the same constant, else pending.
/// - `groups` and `display` are the existing catalog's when it writes them, else the leftover
///   `ui.json`'s, else absent (PC-2: the display file's new groups after them).
/// - An existing entry's business part is never overwritten unless `refresh` names it (PC-2, the
///   one way it is brought back to the rules; the plain rule already drops a pool `schedule.json`
///   no longer declares, below): its differences from the rules are
///   reported ([`StandardPackCatalogMerge::differences`]). What no longer holds is left out and
///   reported: the entry of a package the registry no longer holds
///   ([`StandardPackCatalogMerge::removed`]) and a pool `schedule.json` no longer declares
///   ([`StandardPackCatalogMerge::dropped`]), each with its pointer in the existing file (fix
///   round 1, [`StandardPackCatalogMerge::left_out`]).
///
/// An existing catalog the reader refuses for anything but those two references is not brought up
/// to date: its errors are the answer. Fix round 1: so is each pool a rule of a package that gets a
/// new entry names and `schedule.json` does not declare, `catalog_pool_undeclared` in
/// `schedule.json` at the rule's pool (`/rules/<i>/<side>/<j>/pool_id`), the file the pool is
/// missing from ([`undeclared_rule_pools`]). The written bytes are read back by the reader, and
/// its errors (none, for a merge that holds) are the answer too.
///
/// Workflow #477 PC-2: `edits` ([`CatalogEdits`]; the default asks nothing, and the rule above is
/// the whole merge, byte for byte).
///
/// - **`refresh`**: each package it names is one the registry holds, named once
///   ([`CatalogMergeRefusal::Refresh`] otherwise, before anything else is read). The entry of
///   each has its `consumes` and `produces` computed again from the package's rules, by the rule
///   a new entry is made by ([`schedule_union`]), and they replace what the entry held; its display
///   and its place in the file are kept, and an entry that holds them already is left as written.
///   A named package without an entry is not refused: it gets its new entry by the plain rule,
///   which is its rules' already. Its rules' pools are held to `schedule.json` as a new entry's
///   are. Each is answered in [`StandardPackCatalogMerge::refreshed`], before and after. An entry
///   not named is never overwritten.
/// - **`displays`**: the display file ([`Check::display_file`]), read after the root's own
///   refusals and checked against the groups of the file being written, by the reader's own
///   checks; any error refuses the merge ([`CatalogMergeRefusal::Display`]). Each entry it names
///   has its display replaced whole by the file's (its `package_id`, the display members in the
///   order a new entry writes them, then its business part as it stands), its business part and
///   its place untouched; a new entry it names takes no display from `ui.json`. Each is answered
///   in [`StandardPackCatalogMerge::display_written`]. Fix round 1 (the coordinator's ruling): each
///   group of the file whose id the written `groups` do not hold is appended to them, in the file's
///   order, and answered in [`StandardPackCatalogMerge::groups_added`]; a group there with the same
///   title is simply there, one there with another title refuses the merge, and no group is
///   removed or renamed. Without a new group `groups` are as they were, byte for byte (a `groups`
///   that is no list holds no group for the reader, and the new ones are then the list).
pub fn merge_standard_pack_catalog(
    game: &str,
    registry: &StandardPackRegistry,
    schedule: Option<&StandardPackSchedule>,
    existing: Option<&[u8]>,
    retired_ui: Option<&Value>,
    edits: &CatalogEdits<'_>,
) -> Result<StandardPackCatalogMerge, CatalogMergeRefusal> {
    let refresh = refresh_named(registry, edits.refresh).map_err(CatalogMergeRefusal::Refresh)?;
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
                return Err(CatalogMergeRefusal::Root(refused));
            }
            Some(read)
        }
    };
    let existing_document = existing.as_ref().and_then(|read| read.document.as_ref());
    let mut kept = Vec::new();
    let mut removed = Vec::new();
    let mut dropped = Vec::new();
    let mut left_out = Vec::new();
    let mut refreshed_by_package = BTreeMap::new();
    let mut packs = Vec::new();
    let mut listed = BTreeSet::new();
    for (index, entry) in existing_document
        .and_then(|document| document.get("packs"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let Some(object) = entry.as_object() else {
            continue;
        };
        let Some(package_id) = object.get("package_id").and_then(Value::as_str) else {
            continue;
        };
        let pointer = format!("/packs/{index}");
        if registry.get(package_id).is_none() {
            removed.push(package_id.to_owned());
            left_out.push(CatalogLeftOut {
                problem: StandardPackProblem::PackageNotListed,
                package_id: package_id.to_owned(),
                pool_id: None,
                pointer: at(&pointer, "package_id"),
            });
            continue;
        }
        let mut written = object.clone();
        if refresh.contains(package_id) {
            // PC-2: the business part computed again by the rule a new entry is made by; the
            // display and the place stay, and an entry that holds it already stays as written.
            let before = existing
                .as_ref()
                .and_then(|read| read.catalog.entry(package_id))
                .map(|read_entry| read_entry.business.clone());
            let after = schedule_business(schedule, package_id);
            let state = if before.as_ref() == Some(&after) {
                CatalogRefreshState::Agrees
            } else {
                for side in CatalogSide::ALL.iter().copied() {
                    written.insert(
                        side.as_str().to_owned(),
                        Value::Array(after.side(side).iter().map(pool_value).collect()),
                    );
                }
                CatalogRefreshState::Replaced
            };
            refreshed_by_package.insert(
                package_id.to_owned(),
                CatalogRefreshed {
                    package_id: package_id.to_owned(),
                    state,
                    before,
                    after,
                },
            );
        } else {
            for side in CatalogSide::ALL.iter().copied() {
                let at_side = at(&pointer, side.as_str());
                if let Some(Value::Array(items)) = written.get_mut(side.as_str()) {
                    let mut position = 0_usize;
                    items.retain(|item| {
                        let at_pool = format!("{at_side}/{position}/pool_id");
                        position += 1;
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
                            left_out.push(CatalogLeftOut {
                                problem: StandardPackProblem::CatalogPoolUndeclared,
                                package_id: package_id.to_owned(),
                                pool_id: Some(pool_id.to_owned()),
                                pointer: at_pool,
                            });
                        }
                        declared
                    });
                }
            }
        }
        listed.insert(package_id.to_owned());
        kept.push(package_id.to_owned());
        packs.push(Value::Object(written));
    }
    // Fix round 1: a new entry's pools are its rules'; one that `schedule.json` does not declare is
    // refused there, where it is missing, before anything is made. PC-2: a refreshed entry's are
    // its rules' too.
    let fresh = registry
        .package_ids()
        .filter(|package_id| !listed.contains(*package_id) || refresh.contains(*package_id))
        .collect::<BTreeSet<_>>();
    let undeclared = undeclared_rule_pools(schedule, &pools, &fresh);
    if !undeclared.is_empty() {
        return Err(CatalogMergeRefusal::Root(undeclared));
    }
    let carried = |member: &str| {
        existing_document
            .and_then(|document| document.get(member))
            .or_else(|| retired_ui.and_then(|ui| ui.get(member)))
            .cloned()
    };
    // PC-2: the display file, checked against the groups the written file carries.
    let displays = edits
        .displays
        .map(|file| {
            read_display_file(file.bytes, registry, carried("groups").as_ref()).map_err(|issues| {
                CatalogMergeRefusal::Display {
                    path: file.path.to_owned(),
                    issues,
                }
            })
        })
        .transpose()?;
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
        // PC-2: a display the display file gives replaces the entry's whole, below.
        let named = displays
            .iter()
            .flat_map(|file| &file.entries)
            .any(|given| given.package_id == package_id);
        let display = ui_packs
            .into_iter()
            .flatten()
            .find(|pack| pack.get("package_id").and_then(Value::as_str) == Some(package_id))
            .and_then(Value::as_object);
        if let Some(display) = display
            && !named
        {
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
        let business = schedule_business(schedule, package_id);
        for side in CatalogSide::ALL.iter().copied() {
            entry.insert(
                side.as_str().to_owned(),
                Value::Array(business.side(side).iter().map(pool_value).collect()),
            );
        }
        if refresh.contains(package_id) {
            refreshed_by_package.insert(
                package_id.to_owned(),
                CatalogRefreshed {
                    package_id: package_id.to_owned(),
                    state: CatalogRefreshState::Added,
                    before: None,
                    after: business,
                },
            );
        }
        added.push(package_id.to_owned());
        packs.push(Value::Object(entry));
    }
    // PC-2: each display the display file gives replaces its entry's whole. Every package of the
    // registry has its entry by now, and the file names only such packages.
    let mut display_written = Vec::new();
    for named in displays.iter().flat_map(|file| &file.entries) {
        let found = packs.iter_mut().find(|pack| {
            pack.get("package_id").and_then(Value::as_str) == Some(named.package_id.as_str())
        });
        if let Some(Value::Object(entry)) = found {
            let replaced = with_display(entry, named.display.as_ref());
            *entry = replaced;
            display_written.push(named.package_id.clone());
        }
    }
    let refreshed = edits
        .refresh
        .iter()
        .filter_map(|package_id| refreshed_by_package.remove(package_id.as_str()))
        .collect::<Vec<_>>();
    let mut document = Map::new();
    document.insert(
        "schema_version".to_owned(),
        Value::String(STANDARD_PACK_CATALOG_SCHEMA_VERSION.to_owned()),
    );
    document.insert("game".to_owned(), Value::String(game.to_owned()));
    // PC-2 fix round 1: the groups the display file adds follow the written file's, in the display
    // file's order; without one, `groups` is as it was, byte for byte.
    let new_groups = displays.map(|file| file.groups_added).unwrap_or_default();
    let groups_added = new_groups
        .iter()
        .filter_map(|group| group.get("id").and_then(Value::as_str))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let groups = match (carried("groups"), new_groups.is_empty()) {
        (groups, true) => groups,
        (Some(Value::Array(mut groups)), false) => {
            groups.extend(new_groups);
            Some(Value::Array(groups))
        }
        (_, false) => Some(Value::Array(new_groups)),
    };
    if let Some(groups) = groups {
        document.insert("groups".to_owned(), groups);
    }
    if let Some(display) = carried("display") {
        document.insert("display".to_owned(), display);
    }
    document.insert("packs".to_owned(), Value::Array(packs));
    let mut bytes = serde_json::to_vec_pretty(&Value::Object(document)).map_err(|_| {
        CatalogMergeRefusal::Root(vec![StandardPackRootIssue::new(
            STANDARD_PACK_CATALOG_FILE,
            StandardPackProblem::Malformed,
            "",
        )])
    })?;
    bytes.push(b'\n');
    let catalog = parse_standard_pack_catalog(&bytes, game, registry, Some(&pools))
        .map_err(CatalogMergeRefusal::Root)?;
    let differences = standard_pack_catalog_differences(&catalog, schedule);
    Ok(StandardPackCatalogMerge {
        bytes,
        catalog,
        added,
        kept,
        removed,
        dropped,
        left_out,
        differences,
        display_from_ui,
        refreshed,
        display_written,
        groups_added,
    })
}

/// Workflow #477 PC-2: the packages `refresh` names, each one the registry holds and named once;
/// the first that is not, refused by its word.
fn refresh_named<'r>(
    registry: &StandardPackRegistry,
    refresh: &'r [String],
) -> Result<BTreeSet<&'r str>, CatalogRefreshRefusal> {
    let mut named = BTreeSet::new();
    for package_id in refresh {
        if registry.get(package_id).is_none() {
            return Err(CatalogRefreshRefusal {
                package_id: package_id.clone(),
                problem: CatalogRefreshProblem::PackageNotListed,
            });
        }
        if !named.insert(package_id.as_str()) {
            return Err(CatalogRefreshRefusal {
                package_id: package_id.clone(),
                problem: CatalogRefreshProblem::PackageRepeated,
            });
        }
    }
    Ok(named)
}

/// Workflow #477 PC-2: the business part the rules of `package_id` in `schedule` give, both sides
/// by [`schedule_union`]: what a new entry is written with, and what a refreshed entry holds.
fn schedule_business(schedule: Option<&StandardPackSchedule>, package_id: &str) -> CatalogBusiness {
    CatalogBusiness {
        consumes: schedule_union(schedule, package_id, CatalogSide::Consumes),
        produces: schedule_union(schedule, package_id, CatalogSide::Produces),
    }
}

/// Workflow #477 PC-2: the display file's entries and (fix round 1) the groups it adds
/// ([`Check::display_file`]), checked against `groups` (the `groups` of the file the writer
/// writes, whose ids the reader's own group check takes); every error of the file, by the
/// catalog's own words, otherwise: first those of its shape, its groups and its packages, then its
/// display defects, each at its pointer in the file.
fn read_display_file(
    bytes: &[u8],
    registry: &StandardPackRegistry,
    groups: Option<&Value>,
) -> Result<DisplayFile, Vec<CatalogDisplayFileIssue>> {
    let mut check = Check {
        registry,
        declared_pools: None,
        errors: Vec::new(),
        warnings: Vec::new(),
        defects: 0,
    };
    // The groups of the written file as the reader takes them; their own defects are that file's
    // warnings, not this file's.
    let written = WrittenGroups {
        ids: groups
            .map(|groups| check.groups(groups).1)
            .unwrap_or_default(),
        items: groups
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default(),
    };
    check.warnings.clear();
    let file = check.display_file(bytes, &written);
    let issues = check
        .errors
        .into_iter()
        .chain(check.warnings)
        .map(|issue| CatalogDisplayFileIssue {
            problem: issue.problem,
            pointer: issue.pointer,
        })
        .collect::<Vec<_>>();
    if issues.is_empty() {
        Ok(file)
    } else {
        Err(issues)
    }
}

/// Workflow #477 PC-2: `entry` with its display replaced whole by `display` (`None`: no display
/// member, the entry shows by its id): its `package_id`, then the display members `display` writes
/// in the order a new entry writes them, then every other member of `entry` (its business part) as
/// it stands, in its order.
fn with_display(
    entry: &Map<String, Value>,
    display: Option<&Map<String, Value>>,
) -> Map<String, Value> {
    let mut written = Map::new();
    if let Some(package_id) = entry.get("package_id") {
        written.insert("package_id".to_owned(), package_id.clone());
    }
    for member in DISPLAY_MEMBERS {
        if let Some(value) = display.and_then(|display| display.get(member)) {
            written.insert(member.to_owned(), value.clone());
        }
    }
    for (name, value) in entry {
        if name != "package_id" && !DISPLAY_MEMBERS.contains(&name.as_str()) {
            written.insert(name.clone(), value.clone());
        }
    }
    written
}

#[cfg(test)]
#[path = "standard_pack_catalog_tests.rs"]
mod tests;
