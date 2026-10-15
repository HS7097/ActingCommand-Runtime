// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #373 NAV-6 (model M-373 v1.1 §3): the data files at the root of an installed
//! standard package, `<standard_packs.root>/<game>/`, beside its package directories. This
//! module holds the game-neutral structure checks every reader shares (the Runtime's root
//! reader, `actinglab resource validate`), so no reader accepts what another refuses (§3.9).
//!
//! - `bundle.json` (owned by M-PKG; bundle v4 since Workflow #336 PKG-S5): read here only for
//!   its package id → location table and (Workflow #373 NAV-3a, §4.3) its return-home package
//!   per server, the registry. A package's directory is the game root plus its `packs[].path`
//!   without the `packs/` prefix (§3.1), that is its `package_id`.
//! - `pages.json` (`actingcommand.pages.v1`, §3.3): the game's page ids and the feature
//!   packages that recognise them.
//! - `interrupts.json` (`actingcommand.interrupts.v1`, §3.6): which listed pages are shared
//!   interrupt pages, their handler packages, where the run goes after one, and how often a
//!   run may handle each.
//!
//! The Runtime names no page and no game here: ids are compared as opaque strings.

use crate::{BundleIndex, MaintenanceUse};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The registry's source: the game root's bundle index (M-PKG).
pub const STANDARD_PACK_BUNDLE_FILE: &str = "bundle.json";
/// The game's page list (§3.3).
pub const STANDARD_PACK_PAGES_FILE: &str = "pages.json";
/// The game's shared interrupt pages (§3.6).
pub const STANDARD_PACK_INTERRUPTS_FILE: &str = "interrupts.json";
/// The prefix of a bundle pack's `path`; the package directory is the rest (§3.1).
pub const STANDARD_PACK_PATH_PREFIX: &str = "packs/";

/// The most pages one `pages.json` lists (§3.10).
pub const MAX_STANDARD_PACK_PAGES: usize = 256;
/// The longest page id (§3.3).
pub const MAX_STANDARD_PACK_PAGE_ID_BYTES: usize = 64;
/// The most feature packages one `pages.json` names (§3.3).
pub const MAX_STANDARD_PACK_FEATURES: usize = 8;
/// The most entries one `interrupts.json` holds (§3.10).
pub const MAX_STANDARD_PACK_INTERRUPTS: usize = 32;
/// The most packages one interrupt's `handlers` or `then.run` names (§3.6).
pub const MAX_INTERRUPT_PACKAGES: usize = 8;
/// The most times one run may handle one interrupt page (`max_per_run`, 1..=8, §3.10).
pub const MAX_INTERRUPT_HANDLINGS_PER_PAGE: u8 = 8;
/// The most interrupt handlings of one run, all pages together (§3.10).
pub const MAX_INTERRUPT_HANDLINGS_PER_RUN: u32 = 16;

crate::outcome_vocabulary! {
    /// What is wrong with a standard-pack root file (vocabulary `standard_pack_problem`,
    /// Workflow #373 NAV-6).
    pub enum StandardPackProblem: "standard_pack_problem" {
        /// The file is not JSON of the file's shape and version.
        Malformed => "malformed",
        /// The file's `game` is not the game directory it sits in.
        GameMismatch => "game_mismatch",
        /// `bundle.json` breaks the bundle index rules (M-PKG).
        BundleInvalid => "bundle_invalid",
        /// The game root holds `interrupts.json` or `pages.json` but no `bundle.json`.
        BundleMissing => "bundle_missing",
        /// The game root holds `interrupts.json` but no `pages.json`.
        PagesMissing => "pages_missing",
        /// A page id is not 1-64 bytes of `[a-z][a-z0-9_]*`.
        PageIdInvalid => "page_id_invalid",
        /// A page id is listed twice.
        PageDuplicate => "page_duplicate",
        /// `pages` holds no page, or more than 256.
        PagesCount => "pages_count",
        /// `features` names no package, or more than 8.
        FeaturesCount => "features_count",
        /// A named package is not a package of the game root's bundle.
        PackageNotListed => "package_not_listed",
        /// Two packages of one list belong to the same server.
        ServerDuplicate => "server_duplicate",
        /// `interrupts` holds more than 32 entries.
        InterruptsCount => "interrupts_count",
        /// An interrupt's `page` is not a page of `pages.json`.
        InterruptPageUnlisted => "interrupt_page_unlisted",
        /// Two interrupts name the same page.
        InterruptDuplicate => "interrupt_duplicate",
        /// An interrupt's `handlers` or `then.run` names no package, or more than 8.
        PackagesCount => "packages_count",
        /// An interrupt's `max_per_run` is outside 1..=8.
        MaxPerRunInvalid => "max_per_run_invalid",
    }
}

crate::outcome_vocabulary! {
    /// Why a prerequisite package is not bound (vocabulary `registry_problem`, Workflow #373
    /// NAV-3a, M-373 §4.3): no `standard_packs.root` is set, the root holds no root of the
    /// dependent package's game, or that game root's `bundle.json` does not list the package.
    pub enum RegistryProblem: "registry_problem" {
        RootUnset => "root_unset",
        GameRootMissing => "game_root_missing",
        NotListed => "not_listed",
    }
}

crate::outcome_vocabulary! {
    /// The part a standard-pack package plays in a run (vocabulary `navigation_role`, Workflow
    /// #373 §9.1): the feature package of `pages.json`, an edge or a recovery chain step of
    /// `navigation.json` (NAV-5), an interrupt's handler or its `then.run` package.
    pub enum NavigationRole: "navigation_role" {
        Features => "features",
        Edge => "edge",
        Chain => "chain",
        Handler => "handler",
        ThenRun => "then_run",
    }
}

crate::outcome_vocabulary! {
    /// Why a standard-pack package cannot play its part in a run (vocabulary
    /// `navigation_problem`, Workflow #373 §9.1).
    pub enum NavigationProblem: "navigation_problem" {
        /// The list names no package of the run's server.
        ServerMissing => "server_missing",
        /// The package could not be admitted; the loader's refusal is the cause.
        AdmissionFailed => "admission_failed",
        /// The package's declarations keep it from the part (its execution mode, stability
        /// termination, post-admission OCR, resource readings or a designated operation).
        RoleIneligible => "role_ineligible",
        /// Its game, server or resolution is not the run's package's.
        IdentityMismatch => "identity_mismatch",
        /// It declares a prerequisite package of its own.
        PrerequisiteDeclared => "prerequisite_declared",
        /// Its entry does not hold the page it is named for.
        EntryMismatch => "entry_mismatch",
        /// The feature package's pages are not exactly the pages of `pages.json`.
        PageSetMismatch => "page_set_mismatch",
    }
}

/// One refused root file: the file, what is wrong, and where (a JSON pointer, empty for the
/// whole document).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardPackRootIssue {
    /// One of [`STANDARD_PACK_BUNDLE_FILE`], [`STANDARD_PACK_PAGES_FILE`],
    /// [`STANDARD_PACK_INTERRUPTS_FILE`].
    pub file: &'static str,
    pub problem: StandardPackProblem,
    pub pointer: String,
}

impl StandardPackRootIssue {
    fn new(file: &'static str, problem: StandardPackProblem, pointer: impl Into<String>) -> Self {
        Self {
            file,
            problem,
            pointer: pointer.into(),
        }
    }
}

impl std::fmt::Display for StandardPackRootIssue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}: standard_pack_problem={} json_pointer={}",
            self.file, self.problem, self.pointer
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StandardPackPagesVersion {
    #[serde(rename = "actingcommand.pages.v1")]
    V1,
}

/// `pages.json` (§3.3): page identities only, and the feature packages that recognise them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandardPackPages {
    pub schema_version: StandardPackPagesVersion,
    pub game: String,
    pub features: Vec<String>,
    pub pages: Vec<StandardPackPage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandardPackPage {
    pub id: String,
    pub kind: StandardPackPageKind,
}

/// For people and tools; no reader treats the two kinds differently (§3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StandardPackPageKind {
    Page,
    Overlay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StandardPackInterruptsVersion {
    #[serde(rename = "actingcommand.interrupts.v1")]
    V1,
}

/// `interrupts.json` (§3.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandardPackInterrupts {
    pub schema_version: StandardPackInterruptsVersion,
    pub game: String,
    pub interrupts: Vec<StandardPackInterrupt>,
}

/// One shared interrupt page: its handler packages (at most one per server), where the run
/// goes once one handled it, and how often one run may handle it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandardPackInterrupt {
    pub page: String,
    pub handlers: Vec<String>,
    pub then: InterruptThen,
    pub max_per_run: u8,
}

/// Where the run goes after a handler (§6.2): back to what it awaited, to its end with
/// `contained_task_interrupt_escalated`, or through one more package first (`{"run": [...]}`,
/// at most one per server) and then back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterruptThen {
    Resume,
    EndRun,
    Run(Vec<String>),
}

/// One package of a game root's bundle, as the registry holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardPackEntry {
    pub server: String,
    /// The package directory's name below the game root (`packs[].path` without `packs/`, the
    /// pack's `package_id`).
    pub directory: String,
}

/// The registry of one game root (§4.3): package id → package directory, from `bundle.json`,
/// and (Workflow #373 NAV-3a) server → its return-home package, the `maintenance[]` entry whose
/// `uses` holds `return_home` (at most one per server, the bundle index rule). Held in memory
/// only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StandardPackRegistry {
    packs: BTreeMap<String, StandardPackEntry>,
    return_home: BTreeMap<String, String>,
}

/// The one bundle index version this build reads (Workflow #336 PKG-S5).
const BUNDLE_VERSION: u8 = 4;

impl StandardPackRegistry {
    /// The registry `bundle.json`'s bytes declare, for the game directory `game`: the bundle
    /// index rules (M-PKG), and its `game` equal to the directory's. Only bundle v4 is read: a
    /// v2 or v3 index is not of this version and is `malformed` (no old version is read, M-PKG
    /// §7).
    pub fn from_bundle_bytes(bytes: &[u8], game: &str) -> Result<Self, StandardPackRootIssue> {
        let file = STANDARD_PACK_BUNDLE_FILE;
        let index: BundleIndex = serde_json::from_slice(bytes)
            .map_err(|_| StandardPackRootIssue::new(file, StandardPackProblem::Malformed, ""))?;
        index.validate().map_err(|_| {
            StandardPackRootIssue::new(file, StandardPackProblem::BundleInvalid, "")
        })?;
        if index.game != game {
            return Err(StandardPackRootIssue::new(
                file,
                StandardPackProblem::GameMismatch,
                "/game",
            ));
        }
        let mut registry = Self {
            packs: BTreeMap::new(),
            // Workflow #373 NAV-3a: the bundle index allows one return-home package per server
            // and only a listed one.
            return_home: index
                .maintenance()
                .iter()
                .filter(|entry| entry.uses.contains(&MaintenanceUse::ReturnHome))
                .map(|entry| (entry.server.clone(), entry.package_id.clone()))
                .collect(),
        };
        for (position, pack) in index.packs.iter().enumerate() {
            // The bundle index checked that every path is `packs/<package_id>`.
            let directory = pack
                .path
                .strip_prefix(STANDARD_PACK_PATH_PREFIX)
                .filter(|directory| !directory.is_empty() && !directory.contains('/'))
                .ok_or_else(|| {
                    StandardPackRootIssue::new(
                        file,
                        StandardPackProblem::BundleInvalid,
                        format!("/packs/{position}/path"),
                    )
                })?;
            registry.packs.insert(
                pack.package_id.clone(),
                StandardPackEntry {
                    server: pack.server.clone(),
                    directory: directory.to_owned(),
                },
            );
        }
        Ok(registry)
    }

    pub fn get(&self, package_id: &str) -> Option<&StandardPackEntry> {
        self.packs.get(package_id)
    }

    /// The bundle index version the registry was read from (4, the one version read).
    pub const fn bundle_version(&self) -> u8 {
        BUNDLE_VERSION
    }

    pub fn len(&self) -> usize {
        self.packs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packs.is_empty()
    }

    /// Workflow #373 NAV-3a (§4.3): the return-home package of `server`, a package of the
    /// registry.
    pub fn return_home_package(&self, server: &str) -> Option<&str> {
        self.return_home.get(server).map(String::as_str)
    }

    /// Workflow #373 NAV-3a: every server's return-home package id, by server.
    pub const fn return_home_packages(&self) -> &BTreeMap<String, String> {
        &self.return_home
    }

    /// The registry's package ids.
    pub fn package_ids(&self) -> impl Iterator<Item = &str> {
        self.packs.keys().map(String::as_str)
    }

    /// Every (package id, server) the registry holds, the set `standard_pack_registered`
    /// compares (§4.3) with the return-home packages.
    pub fn listed(&self) -> BTreeSet<(String, String)> {
        self.packs
            .iter()
            .map(|(package_id, entry)| (package_id.clone(), entry.server.clone()))
            .collect()
    }

    /// Of `packages`, the one whose bundle server is `server`.
    pub fn for_server<'a>(&self, packages: &'a [String], server: &str) -> Option<&'a str> {
        packages
            .iter()
            .find(|package_id| {
                self.packs
                    .get(package_id.as_str())
                    .is_some_and(|entry| entry.server == server)
            })
            .map(String::as_str)
    }
}

/// `pages.json`'s bytes, decoded and checked on their own (§3.3): `game` is the game directory
/// `game` (any valid game identifier when `None`, for a repository root), 1..=8 features,
/// 1..=256 pages whose ids are 1-64 bytes of `[a-z][a-z0-9_]*` and unique.
pub fn parse_standard_pack_pages(
    bytes: &[u8],
    game: Option<&str>,
) -> Result<StandardPackPages, StandardPackRootIssue> {
    let file = STANDARD_PACK_PAGES_FILE;
    let pages: StandardPackPages = serde_json::from_slice(bytes)
        .map_err(|_| StandardPackRootIssue::new(file, StandardPackProblem::Malformed, ""))?;
    check_game(file, &pages.game, game)?;
    if pages.features.is_empty() || pages.features.len() > MAX_STANDARD_PACK_FEATURES {
        return Err(StandardPackRootIssue::new(
            file,
            StandardPackProblem::FeaturesCount,
            "/features",
        ));
    }
    if pages.pages.is_empty() || pages.pages.len() > MAX_STANDARD_PACK_PAGES {
        return Err(StandardPackRootIssue::new(
            file,
            StandardPackProblem::PagesCount,
            "/pages",
        ));
    }
    let mut seen = BTreeSet::new();
    for (index, page) in pages.pages.iter().enumerate() {
        let pointer = format!("/pages/{index}/id");
        if !standard_pack_page_id(&page.id) {
            return Err(StandardPackRootIssue::new(
                file,
                StandardPackProblem::PageIdInvalid,
                pointer,
            ));
        }
        if !seen.insert(page.id.as_str()) {
            return Err(StandardPackRootIssue::new(
                file,
                StandardPackProblem::PageDuplicate,
                pointer,
            ));
        }
    }
    Ok(pages)
}

/// `interrupts.json`'s bytes, decoded and checked against `pages` (§3.6): `game` as for
/// [`parse_standard_pack_pages`], at most 32 entries, each `page` a page of `pages` and named
/// once, 1..=8 handlers, 1..=8 `then.run` packages, and `max_per_run` within 1..=8.
pub fn parse_standard_pack_interrupts(
    bytes: &[u8],
    game: Option<&str>,
    pages: &StandardPackPages,
) -> Result<StandardPackInterrupts, StandardPackRootIssue> {
    let file = STANDARD_PACK_INTERRUPTS_FILE;
    let interrupts: StandardPackInterrupts = serde_json::from_slice(bytes)
        .map_err(|_| StandardPackRootIssue::new(file, StandardPackProblem::Malformed, ""))?;
    check_game(file, &interrupts.game, game)?;
    if interrupts.interrupts.len() > MAX_STANDARD_PACK_INTERRUPTS {
        return Err(StandardPackRootIssue::new(
            file,
            StandardPackProblem::InterruptsCount,
            "/interrupts",
        ));
    }
    let listed = pages
        .pages
        .iter()
        .map(|page| page.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    for (index, interrupt) in interrupts.interrupts.iter().enumerate() {
        let at = |member: &str| format!("/interrupts/{index}/{member}");
        if !listed.contains(interrupt.page.as_str()) {
            return Err(StandardPackRootIssue::new(
                file,
                StandardPackProblem::InterruptPageUnlisted,
                at("page"),
            ));
        }
        if !seen.insert(interrupt.page.as_str()) {
            return Err(StandardPackRootIssue::new(
                file,
                StandardPackProblem::InterruptDuplicate,
                at("page"),
            ));
        }
        if interrupt.handlers.is_empty() || interrupt.handlers.len() > MAX_INTERRUPT_PACKAGES {
            return Err(StandardPackRootIssue::new(
                file,
                StandardPackProblem::PackagesCount,
                at("handlers"),
            ));
        }
        if let InterruptThen::Run(packages) = &interrupt.then
            && (packages.is_empty() || packages.len() > MAX_INTERRUPT_PACKAGES)
        {
            return Err(StandardPackRootIssue::new(
                file,
                StandardPackProblem::PackagesCount,
                at("then/run"),
            ));
        }
        if interrupt.max_per_run == 0 || interrupt.max_per_run > MAX_INTERRUPT_HANDLINGS_PER_PAGE {
            return Err(StandardPackRootIssue::new(
                file,
                StandardPackProblem::MaxPerRunInvalid,
                at("max_per_run"),
            ));
        }
    }
    Ok(interrupts)
}

/// The references between the root files (§3.9, the Runtime reader's row): every feature,
/// handler and `then.run` package is a package of the registry, and each list names at most
/// one package per server.
pub fn validate_standard_pack_references(
    registry: &StandardPackRegistry,
    pages: &StandardPackPages,
    interrupts: Option<&StandardPackInterrupts>,
) -> Result<(), StandardPackRootIssue> {
    check_packages(
        registry,
        STANDARD_PACK_PAGES_FILE,
        &pages.features,
        "/features",
    )?;
    for (index, interrupt) in interrupts
        .map_or(&[][..], |interrupts| interrupts.interrupts.as_slice())
        .iter()
        .enumerate()
    {
        check_packages(
            registry,
            STANDARD_PACK_INTERRUPTS_FILE,
            &interrupt.handlers,
            &format!("/interrupts/{index}/handlers"),
        )?;
        if let InterruptThen::Run(packages) = &interrupt.then {
            check_packages(
                registry,
                STANDARD_PACK_INTERRUPTS_FILE,
                packages,
                &format!("/interrupts/{index}/then/run"),
            )?;
        }
    }
    Ok(())
}

fn check_packages(
    registry: &StandardPackRegistry,
    file: &'static str,
    packages: &[String],
    pointer: &str,
) -> Result<(), StandardPackRootIssue> {
    let mut servers = BTreeSet::new();
    for (index, package_id) in packages.iter().enumerate() {
        let at = format!("{pointer}/{index}");
        let Some(entry) = registry.get(package_id) else {
            return Err(StandardPackRootIssue::new(
                file,
                StandardPackProblem::PackageNotListed,
                at,
            ));
        };
        if !servers.insert(entry.server.as_str()) {
            return Err(StandardPackRootIssue::new(
                file,
                StandardPackProblem::ServerDuplicate,
                at,
            ));
        }
    }
    Ok(())
}

fn check_game(
    file: &'static str,
    declared: &str,
    directory: Option<&str>,
) -> Result<(), StandardPackRootIssue> {
    let valid = match directory {
        Some(directory) => declared == directory,
        None => crate::package::bundle_identifier(declared),
    };
    if valid {
        Ok(())
    } else {
        Err(StandardPackRootIssue::new(
            file,
            StandardPackProblem::GameMismatch,
            "/game",
        ))
    }
}

/// A game-level page id (§3.3): 1-64 bytes, `^[a-z][a-z0-9_]*$`.
pub fn standard_pack_page_id(id: &str) -> bool {
    let mut bytes = id.bytes();
    id.len() <= MAX_STANDARD_PACK_PAGE_ID_BYTES
        && bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

#[cfg(test)]
mod tests {
    //! Workflow #373 NAV-6, test row N6-1 (model M-373 v1.1 §3.3, §3.6, §3.9, §3.10): the
    //! root files of a synthetic `neutral` game are read when they keep the rules, and each
    //! violation is refused with its own `standard_pack_problem` at its JSON pointer.

    use super::*;
    use serde_json::{Value, json};

    const GAME: &str = "neutral";

    /// A bundle index v4 of `packs` (package id, server).
    fn bundle(game: &str, packs: &[(&str, &str)]) -> Value {
        json!({
            "schema_version": "actingcommand.bundle.v4",
            "game": game,
            "packs": packs
                .iter()
                .map(|(package_id, server)| json!({
                    "package_id": package_id,
                    "server": server,
                    "entry_task_id": "task",
                    "path": format!("packs/{package_id}"),
                    "file_count": 3,
                    "byte_count": 64
                }))
                .collect::<Vec<_>>()
        })
    }

    fn registry() -> StandardPackRegistry {
        StandardPackRegistry::from_bundle_bytes(
            &bytes(&bundle(
                GAME,
                &[
                    ("neutral.test.features", "test"),
                    ("neutral.test.close_notice", "test"),
                    ("neutral.test.close_notice_other", "other"),
                    ("neutral.test.cleanup", "test"),
                ],
            )),
            GAME,
        )
        .expect("a valid bundle")
    }

    fn pages() -> Value {
        json!({
            "schema_version": "actingcommand.pages.v1",
            "game": GAME,
            "features": ["neutral.test.features"],
            "pages": [
                {"id": "home", "kind": "page"},
                {"id": "notice", "kind": "overlay"},
                {"id": "alert", "kind": "overlay"}
            ]
        })
    }

    fn interrupts() -> Value {
        json!({
            "schema_version": "actingcommand.interrupts.v1",
            "game": GAME,
            "interrupts": [
                {"page": "notice", "handlers": ["neutral.test.close_notice", "neutral.test.close_notice_other"],
                 "then": "resume", "max_per_run": 3},
                {"page": "alert", "handlers": ["neutral.test.close_notice"],
                 "then": {"run": ["neutral.test.cleanup"]}, "max_per_run": 1},
                {"page": "home", "handlers": ["neutral.test.close_notice"],
                 "then": "end_run", "max_per_run": 8}
            ]
        })
    }

    fn bytes(value: &Value) -> Vec<u8> {
        serde_json::to_vec(value).expect("fixture JSON")
    }

    fn issue(
        file: &'static str,
        problem: StandardPackProblem,
        pointer: &str,
    ) -> StandardPackRootIssue {
        StandardPackRootIssue {
            file,
            problem,
            pointer: pointer.to_owned(),
        }
    }

    fn read_pages(value: &Value) -> Result<StandardPackPages, StandardPackRootIssue> {
        parse_standard_pack_pages(&bytes(value), Some(GAME))
    }

    fn read_interrupts(value: &Value) -> Result<StandardPackInterrupts, StandardPackRootIssue> {
        let pages = read_pages(&pages()).expect("the page list");
        parse_standard_pack_interrupts(&bytes(value), Some(GAME), &pages)
    }

    fn pages_with(member: &str, value: Value) -> Value {
        let mut pages = pages();
        pages[member] = value;
        pages
    }

    fn interrupt_with(member: &str, value: Value) -> Value {
        let mut interrupts = interrupts();
        interrupts["interrupts"][0][member] = value;
        interrupts
    }

    #[test]
    fn n6_1_valid_root_files_are_read() {
        let registry = registry();
        assert_eq!(registry.len(), 4);
        assert_eq!(registry.bundle_version(), 4);
        let features = registry
            .get("neutral.test.features")
            .expect("a listed pack");
        assert_eq!(features.server, "test");
        assert_eq!(features.directory, "neutral.test.features");
        assert_eq!(
            registry.for_server(
                &[
                    "neutral.test.close_notice_other".to_owned(),
                    "neutral.test.close_notice".to_owned()
                ],
                "test"
            ),
            Some("neutral.test.close_notice")
        );
        let pages = read_pages(&pages()).expect("a valid page list");
        assert_eq!(pages.pages.len(), 3);
        assert_eq!(pages.pages[1].kind, StandardPackPageKind::Overlay);
        let interrupts = read_interrupts(&interrupts()).expect("valid interrupts");
        assert_eq!(
            interrupts
                .interrupts
                .iter()
                .map(|interrupt| interrupt.then.clone())
                .collect::<Vec<_>>(),
            [
                InterruptThen::Resume,
                InterruptThen::Run(vec!["neutral.test.cleanup".to_owned()]),
                InterruptThen::EndRun
            ]
        );
        validate_standard_pack_references(&registry, &pages, Some(&interrupts))
            .expect("every named package is listed, one per server");
        // A repository root's page list names its game itself.
        parse_standard_pack_pages(&bytes(&pages_with("game", json!("neutral"))), None)
            .expect("a page list read without its directory");
    }

    #[test]
    fn n6_1_each_root_file_violation_is_its_problem_at_its_pointer() {
        use StandardPackProblem as Problem;
        let bundle_file = STANDARD_PACK_BUNDLE_FILE;
        let read_bundle = |value: &Value| {
            StandardPackRegistry::from_bundle_bytes(&bytes(value), GAME).map(|_| ())
        };
        let mut unknown = bundle(GAME, &[("neutral.test.features", "test")]);
        unknown["extra"] = json!(true);
        for (case, value, expected) in [
            (
                "bundle unknown member",
                unknown,
                issue(bundle_file, Problem::Malformed, ""),
            ),
            (
                "bundle without packs",
                bundle(GAME, &[]),
                issue(bundle_file, Problem::BundleInvalid, ""),
            ),
            (
                "bundle of another game",
                bundle("other", &[("neutral.test.features", "test")]),
                issue(bundle_file, Problem::GameMismatch, "/game"),
            ),
        ] {
            assert_eq!(read_bundle(&value), Err(expected), "{case}");
        }
        assert_eq!(
            StandardPackRegistry::from_bundle_bytes(b"{", GAME).map(|_| ()),
            Err(issue(bundle_file, Problem::Malformed, "")),
            "bundle not JSON"
        );

        let pages_file = STANDARD_PACK_PAGES_FILE;
        let many_pages = (0..=MAX_STANDARD_PACK_PAGES)
            .map(|index| json!({"id": format!("p{index}"), "kind": "page"}))
            .collect::<Vec<_>>();
        let mut unknown = pages();
        unknown["looks_like"] = json!("prose");
        let page = |id: &str| json!([{"id": id, "kind": "page"}]);
        for (case, value, expected) in [
            (
                "pages unknown member",
                unknown,
                issue(pages_file, Problem::Malformed, ""),
            ),
            (
                "pages of another version",
                pages_with("schema_version", json!("actingcommand.pages.v2")),
                issue(pages_file, Problem::Malformed, ""),
            ),
            (
                "pages of another kind",
                pages_with("pages", json!([{"id": "home", "kind": "dialog"}])),
                issue(pages_file, Problem::Malformed, ""),
            ),
            (
                "pages of another game",
                pages_with("game", json!("other")),
                issue(pages_file, Problem::GameMismatch, "/game"),
            ),
            (
                "no feature package",
                pages_with("features", json!([])),
                issue(pages_file, Problem::FeaturesCount, "/features"),
            ),
            (
                "nine feature packages",
                pages_with(
                    "features",
                    json!(
                        (0..=MAX_STANDARD_PACK_FEATURES)
                            .map(|index| format!("neutral.test.features{index}"))
                            .collect::<Vec<_>>()
                    ),
                ),
                issue(pages_file, Problem::FeaturesCount, "/features"),
            ),
            (
                "no page",
                pages_with("pages", json!([])),
                issue(pages_file, Problem::PagesCount, "/pages"),
            ),
            (
                "257 pages",
                pages_with("pages", json!(many_pages)),
                issue(pages_file, Problem::PagesCount, "/pages"),
            ),
            (
                "an upper-case page id",
                pages_with("pages", page("Home")),
                issue(pages_file, Problem::PageIdInvalid, "/pages/0/id"),
            ),
            (
                "a page id starting with a digit",
                pages_with("pages", page("1home")),
                issue(pages_file, Problem::PageIdInvalid, "/pages/0/id"),
            ),
            (
                "a page id with a dash",
                pages_with("pages", page("home-x")),
                issue(pages_file, Problem::PageIdInvalid, "/pages/0/id"),
            ),
            (
                "a game-prefixed page id",
                pages_with("pages", page("neutral/home")),
                issue(pages_file, Problem::PageIdInvalid, "/pages/0/id"),
            ),
            (
                "an empty page id",
                pages_with("pages", page("")),
                issue(pages_file, Problem::PageIdInvalid, "/pages/0/id"),
            ),
            (
                "a 65-byte page id",
                pages_with("pages", page(&"a".repeat(65))),
                issue(pages_file, Problem::PageIdInvalid, "/pages/0/id"),
            ),
            (
                "a page listed twice",
                pages_with(
                    "pages",
                    json!([{"id": "home", "kind": "page"}, {"id": "home", "kind": "overlay"}]),
                ),
                issue(pages_file, Problem::PageDuplicate, "/pages/1/id"),
            ),
        ] {
            assert_eq!(read_pages(&value).map(|_| ()), Err(expected), "{case}");
        }

        let interrupts_file = STANDARD_PACK_INTERRUPTS_FILE;
        let mut duplicate = interrupts();
        duplicate["interrupts"][1]["page"] = json!("notice");
        let mut unknown = interrupts();
        unknown["interrupts"][0]["popups"] = json!([]);
        let mut other_game = interrupts();
        other_game["game"] = json!("other");
        let many = pages_with(
            "pages",
            json!(
                (0..=MAX_STANDARD_PACK_INTERRUPTS)
                    .map(|index| json!({"id": format!("p{index}"), "kind": "overlay"}))
                    .collect::<Vec<_>>()
            ),
        );
        let many = read_pages(&many).expect("33 pages");
        let mut too_many = interrupts();
        too_many["interrupts"] = json!(
            (0..=MAX_STANDARD_PACK_INTERRUPTS)
                .map(|index| json!({"page": format!("p{index}"),
                                    "handlers": ["neutral.test.close_notice"],
                                    "then": "resume", "max_per_run": 1}))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            parse_standard_pack_interrupts(&bytes(&too_many), Some(GAME), &many).map(|_| ()),
            Err(issue(
                interrupts_file,
                Problem::InterruptsCount,
                "/interrupts"
            )),
            "33 interrupts"
        );
        for (case, value, expected) in [
            (
                "interrupts unknown member",
                unknown,
                issue(interrupts_file, Problem::Malformed, ""),
            ),
            (
                "an unknown then",
                interrupt_with("then", json!("skip")),
                issue(interrupts_file, Problem::Malformed, ""),
            ),
            (
                "interrupts of another game",
                other_game,
                issue(interrupts_file, Problem::GameMismatch, "/game"),
            ),
            (
                "an unlisted page",
                interrupt_with("page", json!("shop")),
                issue(
                    interrupts_file,
                    Problem::InterruptPageUnlisted,
                    "/interrupts/0/page",
                ),
            ),
            (
                "a page named twice",
                duplicate,
                issue(
                    interrupts_file,
                    Problem::InterruptDuplicate,
                    "/interrupts/1/page",
                ),
            ),
            (
                "no handler",
                interrupt_with("handlers", json!([])),
                issue(
                    interrupts_file,
                    Problem::PackagesCount,
                    "/interrupts/0/handlers",
                ),
            ),
            (
                "nine handlers",
                interrupt_with(
                    "handlers",
                    json!(
                        (0..=MAX_INTERRUPT_PACKAGES)
                            .map(|index| format!("neutral.test.handler{index}"))
                            .collect::<Vec<_>>()
                    ),
                ),
                issue(
                    interrupts_file,
                    Problem::PackagesCount,
                    "/interrupts/0/handlers",
                ),
            ),
            (
                "an empty then.run",
                interrupt_with("then", json!({"run": []})),
                issue(
                    interrupts_file,
                    Problem::PackagesCount,
                    "/interrupts/0/then/run",
                ),
            ),
            (
                "max_per_run 0",
                interrupt_with("max_per_run", json!(0)),
                issue(
                    interrupts_file,
                    Problem::MaxPerRunInvalid,
                    "/interrupts/0/max_per_run",
                ),
            ),
            (
                "max_per_run 9",
                interrupt_with("max_per_run", json!(9)),
                issue(
                    interrupts_file,
                    Problem::MaxPerRunInvalid,
                    "/interrupts/0/max_per_run",
                ),
            ),
        ] {
            assert_eq!(read_interrupts(&value).map(|_| ()), Err(expected), "{case}");
        }

        let registry = registry();
        let pages = read_pages(&pages()).expect("the page list");
        let interrupts = read_interrupts(&interrupts()).expect("the interrupts");
        let references = |pages: &StandardPackPages, interrupts: &StandardPackInterrupts| {
            validate_standard_pack_references(&registry, pages, Some(interrupts))
        };
        let mut unlisted_feature = pages.clone();
        unlisted_feature.features = vec!["neutral.test.missing".to_owned()];
        let mut same_server_features = pages.clone();
        same_server_features.features = vec![
            "neutral.test.features".to_owned(),
            "neutral.test.cleanup".to_owned(),
        ];
        let mut unlisted_handler = interrupts.clone();
        unlisted_handler.interrupts[0].handlers = vec!["neutral.test.missing".to_owned()];
        let mut same_server_handlers = interrupts.clone();
        same_server_handlers.interrupts[0].handlers = vec![
            "neutral.test.close_notice".to_owned(),
            "neutral.test.cleanup".to_owned(),
        ];
        let mut unlisted_run = interrupts.clone();
        unlisted_run.interrupts[1].then =
            InterruptThen::Run(vec!["neutral.test.missing".to_owned()]);
        for (case, result, expected) in [
            (
                "an unlisted feature package",
                references(&unlisted_feature, &interrupts),
                issue(pages_file, Problem::PackageNotListed, "/features/0"),
            ),
            (
                "two feature packages of one server",
                references(&same_server_features, &interrupts),
                issue(pages_file, Problem::ServerDuplicate, "/features/1"),
            ),
            (
                "an unlisted handler",
                references(&pages, &unlisted_handler),
                issue(
                    interrupts_file,
                    Problem::PackageNotListed,
                    "/interrupts/0/handlers/0",
                ),
            ),
            (
                "two handlers of one server",
                references(&pages, &same_server_handlers),
                issue(
                    interrupts_file,
                    Problem::ServerDuplicate,
                    "/interrupts/0/handlers/1",
                ),
            ),
            (
                "an unlisted then.run package",
                references(&pages, &unlisted_run),
                issue(
                    interrupts_file,
                    Problem::PackageNotListed,
                    "/interrupts/1/then/run/0",
                ),
            ),
        ] {
            assert_eq!(result, Err(expected), "{case}");
        }
    }
}
