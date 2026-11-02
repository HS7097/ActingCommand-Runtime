// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #407 LOAD-1 (register §1.257, since 0.12.37; design
//! `rd-1031/check-speed/model-loading.md` §2): a game's loading pages, declared once at the root of
//! its resource repository and copied into each pack.
//!
//! - `loading.json` (`actingcommand.loading.v1`) at the repository root, with the templates it
//!   reads in `loading/`: the game's loading pages, the targets they read, their defaults, the
//!   coordinate space and the game's allowance `max_ms` for one confirmation. A page applies to
//!   the servers it names, or to every server; `exclude_packages` names the packs that take none.
//! - `resources/loading.json` (`actingcommand.pack-loading.v1`) and `resources/loading/` inside a
//!   pack: the pages that apply to the pack's server, only the targets they read with their
//!   templates, the defaults, the coordinate space and the effective `max_ms`. `actinglab package
//!   loading --write` writes it and the resource repository commits it with the pack, so the
//!   pack's version freezes it. It carries no digest and no source commit (option 丙).
//!
//! A target is declared as a `task.json` declares one (`anchors`, `verify_templates`,
//! `color_probes`). This module checks only its `id`, a template target's `template` and that it
//! has a `region`; its other members are carried as written, for the pack's declaration gate to
//! check when a Runtime compiles the copy (LOAD-2). Only single-frame template and colour targets
//! can be declared: there is no OCR or other lazily evaluated family here.
//!
//! No Runtime component reads either file in this slice: not the root reader, not a run's
//! preparation, and not the pack loader, which leaves a pack's copy unread. LOAD-2 compiles the
//! copy and LOAD-3 runs it. The core names no page: every id is an opaque word.

use crate::standard_pack::{StandardPackProblem, StandardPackRootIssue, standard_pack_page_id};
use crate::standard_pack_ui::{at, read_document};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

/// The game's loading pages, at the root of its resource repository; `package bundle --loading`
/// lays it beside `bundle.json`, for audit only.
pub const STANDARD_PACK_LOADING_FILE: &str = "loading.json";
/// The directory of the templates `loading.json` reads, beside it.
pub const STANDARD_PACK_LOADING_DIRECTORY: &str = "loading";
/// The one version of `loading.json` this build reads.
pub const STANDARD_PACK_LOADING_SCHEMA_VERSION: &str = "actingcommand.loading.v1";
/// A pack's loading copy, inside the pack.
pub const PACK_LOADING_FILE: &str = "resources/loading.json";
/// The directory of a pack's copied templates, inside the pack.
pub const PACK_LOADING_DIRECTORY: &str = "resources/loading";
/// The one version of a pack's loading copy this build reads.
pub const PACK_LOADING_SCHEMA_VERSION: &str = "actingcommand.pack-loading.v1";
/// The prefix of every loading target id and of every template path.
pub const LOADING_PREFIX: &str = "loading/";
/// The most pages one file declares; it declares one at least.
pub const MAX_LOADING_PAGES: usize = 16;
/// The largest `max_ms`, the bound of a wait's own limit (`wait_page.max_ms`); the least is 1.
pub const MAX_LOADING_MS: u64 = 1_800_000;
/// The longest stem of a template file name, in bytes; one at least.
pub const MAX_LOADING_TEMPLATE_STEM_BYTES: usize = 64;
/// The most violations one refused file reports: the first ones in document order.
pub const MAX_LOADING_ISSUES: usize = 64;
/// The longest server word or excluded package id, in bytes, as a bundle's.
const MAX_LOADING_WORD_BYTES: usize = 128;
/// The match metrics a template target may name, as a recognition pack's defaults.
const MATCH_METRICS: [&str; 2] = ["ccorr_normed", "ccoeff_normed"];

/// The family a loading target is declared in, as in `task.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LoadingTargetFamily {
    Anchors,
    VerifyTemplates,
    ColorProbes,
}

impl LoadingTargetFamily {
    /// The families, in the order a copy lists them.
    pub const ALL: [Self; 3] = [Self::Anchors, Self::VerifyTemplates, Self::ColorProbes];

    /// The document member that lists the family's targets.
    pub const fn member(self) -> &'static str {
        match self {
            Self::Anchors => "anchors",
            Self::VerifyTemplates => "verify_templates",
            Self::ColorProbes => "color_probes",
        }
    }

    /// Whether the family's targets match a template (`anchors`, `verify_templates`).
    pub const fn is_template(self) -> bool {
        !matches!(self, Self::ColorProbes)
    }
}

/// One loading target, as declared.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadingTarget {
    family: LoadingTargetFamily,
    id: String,
    template: Option<String>,
    declaration: Map<String, Value>,
}

impl LoadingTarget {
    pub const fn family(&self) -> LoadingTargetFamily {
        self.family
    }

    /// `loading/<name>`.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// A template target's template, `loading/<file>.png`, relative to the directory of the file
    /// that declares it: the repository root for `loading.json`, the pack's `resources/` for a
    /// copy.
    pub fn template(&self) -> Option<&str> {
        self.template.as_deref()
    }

    /// The declaration as written, its members in the file's order.
    pub const fn declaration(&self) -> &Map<String, Value> {
        &self.declaration
    }
}

/// One loading page: the targets its frame shows (`required`, and one of each `any_of` group)
/// and does not show (`forbidden`), as a `task.json` page rule names them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadingPage {
    pub id: String,
    pub required: Vec<String>,
    pub any_of: Vec<Vec<String>>,
    pub forbidden: Vec<String>,
}

impl LoadingPage {
    /// Every target id the page names, in its order.
    pub fn target_ids(&self) -> impl Iterator<Item = &str> {
        self.required
            .iter()
            .chain(self.any_of.iter().flatten())
            .chain(&self.forbidden)
            .map(String::as_str)
    }

    /// The page as a copy writes it; an empty `any_of` or `forbidden` is left out.
    fn document(&self) -> Value {
        let mut page = Map::new();
        page.insert("id".to_owned(), json!(self.id));
        page.insert("required".to_owned(), json!(self.required));
        if !self.any_of.is_empty() {
            page.insert("any_of".to_owned(), json!(self.any_of));
        }
        if !self.forbidden.is_empty() {
            page.insert("forbidden".to_owned(), json!(self.forbidden));
        }
        Value::Object(page)
    }
}

/// A page of `loading.json`, with the servers it applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardPackLoadingPage {
    pub page: LoadingPage,
    /// The servers the page applies to; `None`, every server.
    pub servers: Option<Vec<String>>,
}

impl StandardPackLoadingPage {
    /// Whether a pack of `server` takes the page.
    pub fn applies_to(&self, server: &str) -> bool {
        self.servers
            .as_ref()
            .is_none_or(|servers| servers.iter().any(|listed| listed == server))
    }
}

/// The frame size the targets' regions are declared in, as a `task.json`'s `coordinate_space`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadingCoordinateSpace {
    pub width: u32,
    pub height: u32,
}

/// A `loading.json` that keeps every rule.
#[derive(Debug, Clone, PartialEq)]
pub struct StandardPackLoading {
    pub game: String,
    pub coordinate_space: LoadingCoordinateSpace,
    /// `template_threshold`, `color_max_distance` and `match_metric`, as written; absent when the
    /// file declares none.
    pub defaults: Option<Map<String, Value>>,
    /// The game's allowance for one confirmation, in milliseconds.
    pub max_ms: u64,
    /// Every target: the `anchors`, then the `verify_templates`, then the `color_probes`, each
    /// family in document order.
    pub targets: Vec<LoadingTarget>,
    pub pages: Vec<StandardPackLoadingPage>,
    /// The packs, by `package_id`, that take no loading page.
    pub exclude_packages: Vec<String>,
}

impl StandardPackLoading {
    /// Whether `exclude_packages` names `package_id`.
    pub fn excludes(&self, package_id: &str) -> bool {
        self.exclude_packages
            .iter()
            .any(|excluded| excluded == package_id)
    }

    /// The copy a pack of `server` takes: the pages that apply to the server, in document order,
    /// only the targets they name, in the file's order, the defaults, the coordinate space and
    /// `max_ms`. `None` when no page applies to the server. Whether a pack is excluded is the
    /// caller's ([`Self::excludes`]).
    pub fn for_server(&self, server: &str) -> Option<PackLoading> {
        let pages = self
            .pages
            .iter()
            .filter(|page| page.applies_to(server))
            .map(|page| page.page.clone())
            .collect::<Vec<_>>();
        if pages.is_empty() {
            return None;
        }
        let named = pages
            .iter()
            .flat_map(LoadingPage::target_ids)
            .collect::<BTreeSet<_>>();
        let targets = self
            .targets
            .iter()
            .filter(|target| named.contains(target.id()))
            .cloned()
            .collect();
        Some(PackLoading {
            coordinate_space: self.coordinate_space,
            defaults: self.defaults.clone(),
            max_ms: self.max_ms,
            targets,
            pages,
        })
    }

    /// The JSON pointer of the target `id`'s `member` in this file
    /// (`/<family>/<index>/<member>`).
    pub fn target_pointer(&self, id: &str, member: &str) -> Option<String> {
        LoadingTargetFamily::ALL.iter().find_map(|family| {
            self.targets
                .iter()
                .filter(|target| target.family == *family)
                .position(|target| target.id == id)
                .map(|index| format!("/{}/{index}/{member}", family.member()))
        })
    }

    /// The JSON pointer of the page `id`'s id in this file (`/pages/<index>/id`).
    pub fn page_pointer(&self, id: &str) -> Option<String> {
        self.pages
            .iter()
            .position(|page| page.page.id == id)
            .map(|index| format!("/pages/{index}/id"))
    }
}

/// A pack's loading copy that keeps every rule.
#[derive(Debug, Clone, PartialEq)]
pub struct PackLoading {
    pub coordinate_space: LoadingCoordinateSpace,
    pub defaults: Option<Map<String, Value>>,
    /// The effective allowance for one confirmation, in milliseconds.
    pub max_ms: u64,
    pub targets: Vec<LoadingTarget>,
    pub pages: Vec<LoadingPage>,
}

impl PackLoading {
    /// The templates its targets read, `loading/<file>.png`, each once.
    pub fn templates(&self) -> BTreeSet<&str> {
        self.targets
            .iter()
            .filter_map(LoadingTarget::template)
            .collect()
    }

    /// The `actingcommand.pack-loading.v1` document: `schema_version`, `coordinate_space`,
    /// `defaults` when there are any, `max_ms`, each family of targets that holds one (each
    /// declaration as written) and `pages`.
    pub fn document(&self) -> Value {
        let mut document = Map::new();
        document.insert(
            "schema_version".to_owned(),
            json!(PACK_LOADING_SCHEMA_VERSION),
        );
        document.insert(
            "coordinate_space".to_owned(),
            json!({
                "width": self.coordinate_space.width,
                "height": self.coordinate_space.height
            }),
        );
        if let Some(defaults) = &self.defaults {
            document.insert("defaults".to_owned(), Value::Object(defaults.clone()));
        }
        document.insert("max_ms".to_owned(), json!(self.max_ms));
        for family in LoadingTargetFamily::ALL {
            let declared = self
                .targets
                .iter()
                .filter(|target| target.family == family)
                .map(|target| Value::Object(target.declaration.clone()))
                .collect::<Vec<_>>();
            if !declared.is_empty() {
                document.insert(family.member().to_owned(), Value::Array(declared));
            }
        }
        document.insert(
            "pages".to_owned(),
            Value::Array(self.pages.iter().map(LoadingPage::document).collect()),
        );
        Value::Object(document)
    }

    /// The bytes `package loading --write` writes: [`Self::document`], indented by two spaces,
    /// with a last line feed.
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        let mut bytes = serde_json::to_vec_pretty(&self.document())?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

/// Whether `id` is a loading target id: `loading/<name>`, the name 1 to 64 bytes of
/// `[a-z][a-z0-9_]*`.
pub fn loading_target_id(id: &str) -> bool {
    id.strip_prefix(LOADING_PREFIX)
        .is_some_and(standard_pack_page_id)
}

/// Whether `path` names a loading template: `loading/<stem>.png`, the stem 1 to
/// [`MAX_LOADING_TEMPLATE_STEM_BYTES`] bytes of `[a-z0-9_-]` that does not start with `-`, so
/// that two names never differ only in case.
pub fn loading_template_path(path: &str) -> bool {
    path.strip_prefix(LOADING_PREFIX)
        .and_then(|name| name.strip_suffix(".png"))
        .is_some_and(|stem| {
            (1..=MAX_LOADING_TEMPLATE_STEM_BYTES).contains(&stem.len())
                && !stem.starts_with('-')
                && stem.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || byte == b'_'
                        || byte == b'-'
                })
        })
}

/// Whether `path`, a `/`-separated path inside a pack, belongs to the pack's loading copy:
/// [`PACK_LOADING_FILE`] or a file below [`PACK_LOADING_DIRECTORY`].
pub fn pack_loading_entry(path: &str) -> bool {
    path == PACK_LOADING_FILE
        || path
            .strip_prefix(PACK_LOADING_DIRECTORY)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// `loading.json`'s bytes, checked; `game`, when given, is the game the file must name. Every
/// violation is one issue at its JSON pointer (empty for the whole document), in document order,
/// at most [`MAX_LOADING_ISSUES`]:
///
/// - `malformed`: no JSON or no object (the whole document); a member repeated in one object; a
///   member that is unknown, missing or not of its type; another `schema_version`; a
///   `coordinate_space` side that is no positive integer; a default out of its range
///   (`template_threshold` in (0, 1], `color_max_distance` at least 0, `match_metric`
///   `ccorr_normed` or `ccoeff_normed`); a target without an object `region`; an empty `any_of`
///   group; a `servers` list that is empty, repeats a server or holds no server word; an
///   `exclude_packages` entry that is empty or repeated;
/// - `game_mismatch`: `game` is not `game`;
/// - `loading_max_invalid`: `max_ms` is outside 1..=[`MAX_LOADING_MS`];
/// - `loading_target_invalid`: a target id is not `loading/<name>` or is declared twice, or a
///   template target's `template` is not `loading/<file>.png` ([`loading_template_path`]);
/// - `pages_count`: `pages` holds no page or more than [`MAX_LOADING_PAGES`];
/// - `page_id_invalid`, `page_duplicate`: a page id is no page id (1-64 bytes of
///   `[a-z][a-z0-9_]*`), or is listed twice;
/// - `loading_page_invalid`: a page names a target the file does not declare, or its `required`
///   holds no template target (a colour target only goes with a template).
///
/// Whether a template exists, and what it shows, is the reader's to check: this crate reads no
/// file.
pub fn parse_standard_pack_loading(
    bytes: &[u8],
    game: Option<&str>,
) -> Result<StandardPackLoading, Vec<StandardPackRootIssue>> {
    let read = Check::read(bytes, Form::Root(game))?;
    match (read.game, read.coordinate_space, read.max_ms) {
        (Some(game), Some(coordinate_space), Some(max_ms)) => Ok(StandardPackLoading {
            game,
            coordinate_space,
            defaults: read.defaults,
            max_ms,
            targets: read.targets,
            pages: read.pages,
            exclude_packages: read.exclude_packages,
        }),
        _ => Err(vec![StandardPackRootIssue::new(
            STANDARD_PACK_LOADING_FILE,
            StandardPackProblem::Malformed,
            "",
        )]),
    }
}

/// A pack's `resources/loading.json`, checked by the rules of [`parse_standard_pack_loading`]
/// with the copy's members: `schema_version` [`PACK_LOADING_SCHEMA_VERSION`], no `game`, no
/// `exclude_packages` and no page `servers`.
pub fn parse_pack_loading(bytes: &[u8]) -> Result<PackLoading, Vec<StandardPackRootIssue>> {
    let read = Check::read(bytes, Form::Pack)?;
    match (read.coordinate_space, read.max_ms) {
        (Some(coordinate_space), Some(max_ms)) => Ok(PackLoading {
            coordinate_space,
            defaults: read.defaults,
            max_ms,
            targets: read.targets,
            pages: read.pages.into_iter().map(|page| page.page).collect(),
        }),
        _ => Err(vec![StandardPackRootIssue::new(
            PACK_LOADING_FILE,
            StandardPackProblem::Malformed,
            "",
        )]),
    }
}

/// Which of the two files a check reads.
#[derive(Clone, Copy)]
enum Form<'g> {
    /// `loading.json`, of the game when one is given.
    Root(Option<&'g str>),
    /// A pack's loading copy.
    Pack,
}

impl Form<'_> {
    const fn file(self) -> &'static str {
        match self {
            Self::Root(_) => STANDARD_PACK_LOADING_FILE,
            Self::Pack => PACK_LOADING_FILE,
        }
    }

    const fn schema_version(self) -> &'static str {
        match self {
            Self::Root(_) => STANDARD_PACK_LOADING_SCHEMA_VERSION,
            Self::Pack => PACK_LOADING_SCHEMA_VERSION,
        }
    }

    /// The document's required and optional members.
    const fn members(self) -> (&'static [&'static str], &'static [&'static str]) {
        match self {
            Self::Root(_) => (ROOT_REQUIRED, ROOT_OPTIONAL),
            Self::Pack => (PACK_REQUIRED, PACK_OPTIONAL),
        }
    }

    /// A page's optional members.
    const fn page_members(self) -> &'static [&'static str] {
        match self {
            Self::Root(_) => ROOT_PAGE_OPTIONAL,
            Self::Pack => PACK_PAGE_OPTIONAL,
        }
    }
}

const ROOT_REQUIRED: &[&str] = &[
    "schema_version",
    "game",
    "coordinate_space",
    "max_ms",
    "pages",
];
const ROOT_OPTIONAL: &[&str] = &[
    "defaults",
    "anchors",
    "verify_templates",
    "color_probes",
    "exclude_packages",
];
const PACK_REQUIRED: &[&str] = &["schema_version", "coordinate_space", "max_ms", "pages"];
const PACK_OPTIONAL: &[&str] = &["defaults", "anchors", "verify_templates", "color_probes"];
const ROOT_PAGE_OPTIONAL: &[&str] = &["servers", "any_of", "forbidden"];
const PACK_PAGE_OPTIONAL: &[&str] = &["any_of", "forbidden"];

/// What one check read; complete only when it found no issue.
#[derive(Default)]
struct Read {
    game: Option<String>,
    coordinate_space: Option<LoadingCoordinateSpace>,
    defaults: Option<Map<String, Value>>,
    max_ms: Option<u64>,
    targets: Vec<LoadingTarget>,
    pages: Vec<StandardPackLoadingPage>,
    exclude_packages: Vec<String>,
}

/// The checks of one document: the issues found so far.
struct Check {
    file: &'static str,
    issues: Vec<StandardPackRootIssue>,
}

impl Check {
    fn read(bytes: &[u8], form: Form<'_>) -> Result<Read, Vec<StandardPackRootIssue>> {
        let mut check = Self {
            file: form.file(),
            issues: Vec::new(),
        };
        let mut repeated = Vec::new();
        let Some(document) = read_document(bytes, &mut repeated) else {
            check.issue(StandardPackProblem::Malformed, "");
            return Err(check.issues);
        };
        for pointer in repeated {
            check.issue(StandardPackProblem::Malformed, pointer);
        }
        let read = check.document(&document, form);
        if check.issues.is_empty() {
            Ok(read)
        } else {
            Err(check.issues)
        }
    }

    fn issue(&mut self, problem: StandardPackProblem, pointer: impl Into<String>) {
        if self.issues.len() < MAX_LOADING_ISSUES {
            self.issues
                .push(StandardPackRootIssue::new(self.file, problem, pointer));
        }
    }

    /// An object of the members `required` and `optional`: an unknown member and a missing one
    /// are `malformed` at its pointer, a value that is no object at the value's.
    fn object<'v>(
        &mut self,
        value: &'v Value,
        pointer: &str,
        required: &[&str],
        optional: &[&str],
    ) -> Option<&'v Map<String, Value>> {
        let Some(object) = value.as_object() else {
            self.issue(StandardPackProblem::Malformed, pointer);
            return None;
        };
        for name in object.keys() {
            if !required.contains(&name.as_str()) && !optional.contains(&name.as_str()) {
                self.issue(StandardPackProblem::Malformed, at(pointer, name));
            }
        }
        for name in required {
            if !object.contains_key(*name) {
                self.issue(StandardPackProblem::Malformed, at(pointer, name));
            }
        }
        Some(object)
    }

    fn document(&mut self, value: &Value, form: Form<'_>) -> Read {
        let mut read = Read::default();
        let (required, optional) = form.members();
        let Some(document) = self.object(value, "", required, optional) else {
            return read;
        };
        if let Some(version) = document.get("schema_version")
            && version.as_str() != Some(form.schema_version())
        {
            self.issue(StandardPackProblem::Malformed, "/schema_version");
        }
        if let Form::Root(game) = form
            && let Some(declared) = document.get("game")
        {
            match declared.as_str().filter(|declared| !declared.is_empty()) {
                None => self.issue(StandardPackProblem::Malformed, "/game"),
                Some(declared) => {
                    if game.is_some_and(|game| game != declared) {
                        self.issue(StandardPackProblem::GameMismatch, "/game");
                    }
                    read.game = Some(declared.to_owned());
                }
            }
        }
        if let Some(space) = document.get("coordinate_space") {
            read.coordinate_space = self.coordinate_space(space);
        }
        if let Some(defaults) = document.get("defaults") {
            read.defaults = self.defaults(defaults);
        }
        if let Some(max_ms) = document.get("max_ms") {
            match max_ms.as_u64() {
                None => self.issue(StandardPackProblem::Malformed, "/max_ms"),
                Some(max_ms) if !(1..=MAX_LOADING_MS).contains(&max_ms) => {
                    self.issue(StandardPackProblem::LoadingMaxInvalid, "/max_ms");
                }
                Some(max_ms) => read.max_ms = Some(max_ms),
            }
        }
        let mut ids = BTreeSet::new();
        for family in LoadingTargetFamily::ALL {
            if let Some(list) = document.get(family.member()) {
                self.targets(list, family, &mut ids, &mut read.targets);
            }
        }
        if let Some(pages) = document.get("pages") {
            read.pages = self.pages(pages, form, &read.targets);
        }
        if let Some(excluded) = document.get("exclude_packages") {
            read.exclude_packages = self.words(excluded, "/exclude_packages", false);
        }
        read
    }

    fn coordinate_space(&mut self, value: &Value) -> Option<LoadingCoordinateSpace> {
        let space = self.object(value, "/coordinate_space", &["width", "height"], &[])?;
        let width = self.side(space, "width");
        let height = self.side(space, "height");
        Some(LoadingCoordinateSpace {
            width: width?,
            height: height?,
        })
    }

    /// One side of `coordinate_space`: a positive integer of at most `u32::MAX`.
    fn side(&mut self, space: &Map<String, Value>, name: &str) -> Option<u32> {
        let value = space.get(name)?;
        let side = value
            .as_u64()
            .filter(|side| *side > 0)
            .and_then(|side| u32::try_from(side).ok());
        if side.is_none() {
            self.issue(
                StandardPackProblem::Malformed,
                format!("/coordinate_space/{name}"),
            );
        }
        side
    }

    fn defaults(&mut self, value: &Value) -> Option<Map<String, Value>> {
        let defaults = self.object(
            value,
            "/defaults",
            &[],
            &["template_threshold", "color_max_distance", "match_metric"],
        )?;
        if let Some(threshold) = defaults.get("template_threshold")
            && !threshold
                .as_f64()
                .is_some_and(|threshold| threshold > 0.0 && threshold <= 1.0)
        {
            self.issue(
                StandardPackProblem::Malformed,
                "/defaults/template_threshold",
            );
        }
        if let Some(distance) = defaults.get("color_max_distance")
            && !distance.as_f64().is_some_and(|distance| distance >= 0.0)
        {
            self.issue(
                StandardPackProblem::Malformed,
                "/defaults/color_max_distance",
            );
        }
        if let Some(metric) = defaults.get("match_metric")
            && !metric
                .as_str()
                .is_some_and(|metric| MATCH_METRICS.contains(&metric))
        {
            self.issue(StandardPackProblem::Malformed, "/defaults/match_metric");
        }
        Some(defaults.clone())
    }

    /// The targets of one family; `ids` holds every id the file declared before.
    fn targets(
        &mut self,
        value: &Value,
        family: LoadingTargetFamily,
        ids: &mut BTreeSet<String>,
        targets: &mut Vec<LoadingTarget>,
    ) {
        let pointer = format!("/{}", family.member());
        let Some(items) = value.as_array() else {
            self.issue(StandardPackProblem::Malformed, pointer);
            return;
        };
        for (index, item) in items.iter().enumerate() {
            let at = format!("{pointer}/{index}");
            let Some(declaration) = item.as_object() else {
                self.issue(StandardPackProblem::Malformed, at);
                continue;
            };
            let id = match declaration.get("id").and_then(Value::as_str) {
                None => {
                    self.issue(StandardPackProblem::Malformed, format!("{at}/id"));
                    None
                }
                Some(id) => {
                    if loading_target_id(id) && ids.insert(id.to_owned()) {
                        Some(id.to_owned())
                    } else {
                        self.issue(
                            StandardPackProblem::LoadingTargetInvalid,
                            format!("{at}/id"),
                        );
                        None
                    }
                }
            };
            if !declaration.get("region").is_some_and(Value::is_object) {
                self.issue(StandardPackProblem::Malformed, format!("{at}/region"));
            }
            let template = if family.is_template() {
                match declaration.get("template").and_then(Value::as_str) {
                    None => {
                        self.issue(StandardPackProblem::Malformed, format!("{at}/template"));
                        None
                    }
                    Some(path) if loading_template_path(path) => Some(path.to_owned()),
                    Some(_) => {
                        self.issue(
                            StandardPackProblem::LoadingTargetInvalid,
                            format!("{at}/template"),
                        );
                        None
                    }
                }
            } else {
                None
            };
            if let Some(id) = id {
                targets.push(LoadingTarget {
                    family,
                    id,
                    template,
                    declaration: declaration.clone(),
                });
            }
        }
    }

    fn pages(
        &mut self,
        value: &Value,
        form: Form<'_>,
        targets: &[LoadingTarget],
    ) -> Vec<StandardPackLoadingPage> {
        let Some(items) = value.as_array() else {
            self.issue(StandardPackProblem::Malformed, "/pages");
            return Vec::new();
        };
        if !(1..=MAX_LOADING_PAGES).contains(&items.len()) {
            self.issue(StandardPackProblem::PagesCount, "/pages");
        }
        let mut seen = BTreeSet::new();
        let mut pages = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let at = format!("/pages/{index}");
            let Some(page) = self.object(item, &at, &["id", "required"], form.page_members())
            else {
                continue;
            };
            let id = match page.get("id").map(Value::as_str) {
                None => None,
                Some(None) => {
                    self.issue(StandardPackProblem::Malformed, format!("{at}/id"));
                    None
                }
                Some(Some(id)) => {
                    if !standard_pack_page_id(id) {
                        self.issue(StandardPackProblem::PageIdInvalid, format!("{at}/id"));
                        None
                    } else if !seen.insert(id) {
                        self.issue(StandardPackProblem::PageDuplicate, format!("{at}/id"));
                        None
                    } else {
                        Some(id.to_owned())
                    }
                }
            };
            let required = match page.get("required") {
                None => Vec::new(),
                Some(list) => {
                    let pointer = format!("{at}/required");
                    let required = self.target_list(list, &pointer, targets);
                    let template = required.iter().any(|id| {
                        targets
                            .iter()
                            .any(|target| target.id == *id && target.family.is_template())
                    });
                    if !template {
                        self.issue(StandardPackProblem::LoadingPageInvalid, pointer);
                    }
                    required
                }
            };
            let any_of = match page.get("any_of") {
                None => Vec::new(),
                Some(groups) => self.groups(groups, &format!("{at}/any_of"), targets),
            };
            let forbidden = match page.get("forbidden") {
                None => Vec::new(),
                Some(list) => self.target_list(list, &format!("{at}/forbidden"), targets),
            };
            let servers = page
                .get("servers")
                .map(|servers| self.words(servers, &format!("{at}/servers"), true));
            if let Some(id) = id {
                pages.push(StandardPackLoadingPage {
                    page: LoadingPage {
                        id,
                        required,
                        any_of,
                        forbidden,
                    },
                    servers,
                });
            }
        }
        pages
    }

    /// A list of target ids, each one the file declares (`loading_page_invalid` otherwise).
    fn target_list(
        &mut self,
        value: &Value,
        pointer: &str,
        targets: &[LoadingTarget],
    ) -> Vec<String> {
        let Some(items) = value.as_array() else {
            self.issue(StandardPackProblem::Malformed, pointer);
            return Vec::new();
        };
        let mut ids = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let Some(id) = item.as_str() else {
                self.issue(StandardPackProblem::Malformed, format!("{pointer}/{index}"));
                continue;
            };
            if !targets.iter().any(|target| target.id == id) {
                self.issue(
                    StandardPackProblem::LoadingPageInvalid,
                    format!("{pointer}/{index}"),
                );
            }
            ids.push(id.to_owned());
        }
        ids
    }

    /// `any_of`: a list of groups, each a non-empty list of target ids.
    fn groups(
        &mut self,
        value: &Value,
        pointer: &str,
        targets: &[LoadingTarget],
    ) -> Vec<Vec<String>> {
        let Some(groups) = value.as_array() else {
            self.issue(StandardPackProblem::Malformed, pointer);
            return Vec::new();
        };
        let mut lists = Vec::new();
        for (index, group) in groups.iter().enumerate() {
            let at = format!("{pointer}/{index}");
            if group.as_array().is_some_and(Vec::is_empty) {
                self.issue(StandardPackProblem::Malformed, at.as_str());
            }
            lists.push(self.target_list(group, &at, targets));
        }
        lists
    }

    /// A list of distinct words of 1 to 128 bytes: with `server`, a non-empty list of server
    /// words of `[a-z0-9._-]` (a page's `servers`); otherwise any words (`exclude_packages`).
    fn words(&mut self, value: &Value, pointer: &str, server: bool) -> Vec<String> {
        let Some(items) = value.as_array() else {
            self.issue(StandardPackProblem::Malformed, pointer);
            return Vec::new();
        };
        if server && items.is_empty() {
            self.issue(StandardPackProblem::Malformed, pointer);
        }
        let mut seen = BTreeSet::new();
        let mut words = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let word = item.as_str().filter(|word| {
                (1..=MAX_LOADING_WORD_BYTES).contains(&word.len())
                    && (!server
                        || word.bytes().all(|byte| {
                            byte.is_ascii_lowercase()
                                || byte.is_ascii_digit()
                                || matches!(byte, b'.' | b'_' | b'-')
                        }))
            });
            match word {
                Some(word) if !seen.contains(word) => {
                    seen.insert(word);
                    words.push(word.to_owned());
                }
                _ => self.issue(StandardPackProblem::Malformed, format!("{pointer}/{index}")),
            }
        }
        words
    }
}
