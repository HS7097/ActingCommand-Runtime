// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #373 NAV-T (M-373 v1.1 §3.9 second row, §11.1; wave-18 ruling 1): the standard-pack
//! cross-check. `package bundle` runs it on the game root it lays out, and `actinglab
//! standard-pack check --root` on an installed game root, `<standard_packs.root>/<game>/`.
//!
//! The root files are read by the contract crate's one structure check
//! ([`read_standard_pack_root`]), which the Runtime's root reader runs too; a refused file is
//! `standard_pack_root_invalid`. Then every package of the registry (`bundle.json`) is admitted
//! for real, once, by its directory ([`PreparedContainedTask::describe_navigation_path`], the
//! admission of `describe_path`), with or without the navigation files `pages.json`,
//! `interrupts.json` and `navigation.json` (Workflow #411 A25): the root is `checked` only once
//! every package was admitted. A package the root files name is admitted first, for its part (a
//! refusal, a missing directory too, is its `admission_failed`); every other package of the
//! registry after them (a refusal, a missing directory too, is `package_invalid` naming its
//! directory). Without `pages.json` no file names a package. Each package the root files name is
//! checked for its part as the Runtime checks it when a run first uses it. Each feature package
//! of `pages.json` is a `recognize_only` package of the game and its bundle server whose pages
//! are exactly the listed pages (its admission checks the REC-M static budget of its page set).
//! Each edge's package is eligible as a layer, its entry holds the edge's `from`, it ends on the
//! edge's `to`, and its entry is no application entry. Each recovery chain, handler and
//! `then.run` package is eligible and declares no prerequisite package of its own, and each
//! handler's entry holds its interrupt page. A package that does not fit is
//! `package_bundle_navigation_package_invalid` with the `navigation_role` and
//! `navigation_problem` of the Runtime's `contained_task_navigation_package_invalid`.
//!
//! Eligibility is the Runtime's (`TaskPackageDescriptor::prerequisite_incompatibility_with`):
//! the package's own qualification for a layer (an `on_empty` select step makes it
//! `role_ineligible`, as since Workflow #308 SEL-L4b), and its game, server and resolution. With
//! no run to compare with, a package's server is its bundle server, and its resolution is
//! compared with the resolution of that server's feature package, which every run of the server
//! that navigates or handles a shared interrupt page uses.
//!
//! Then the report "entry page not in the list" (M-373 §4.4, §10.1 NAV-4): one
//! `package_bundle_entry_page_unlisted` warning per package whose entry pages, as the Runtime's
//! entry gate reads them when its game has navigation data, are not all listed in `pages.json`.
//! Workflow #411 B35: then, with `interrupts.json`, one `package_bundle_interrupt_page_shadowed`
//! warning per package and shared interrupt page it declares itself (a page a package declares is
//! its own, so that interrupt does not act for its runs); a feature package and the page's
//! handlers declare it by their part and are not reported. STORE-FG-1 (design-store-fg §8.1):
//! then, with or without `pages.json`, one `package_bundle_store_tap_unlocated` warning per
//! package and declared store operation (`control.json` `store_update`) whose click taps a fixed
//! place (`point`, `rect`, `specific_rect`) instead of a target located on the frame. A
//! `store_update` package is never a layer (`TaskPackageDescriptor::prerequisite_incompatibility_with`
//! answers `store_update`), so as an edge, recovery chain, handler or `then.run` package it is
//! `role_ineligible`.
//!
//! Workflow #392 DA-4 (model 392 v1.2 §4.1): `display-layout.json` (`package bundle
//! --display-layout`, or a game root that holds it) is a root file the one structure check reads
//! like the others; it names no package, so the cross-check has nothing more to check of it.
//!
//! ROUT-L RL-1a (contracts/standard-pack-schedule.md): with `schedule.json` (`package bundle
//! --schedule`, or a game root that holds it), the file is composed by the Runtime's composer with
//! one bound instance per server ([`crate::standard_pack_schedule`]); a refusal is
//! `standard_pack_root_invalid` naming it, and each unclassified package is one
//! `standard_pack_package_unclassified` warning, after the remedy report. UPD-1 fix round 1
//! (review L6): then a rule that runs a store update package (`store_update`) with a `cooldown_ms`
//! of 0 is refused the same way, `schedule_invalid` at its `cooldown_ms`. SS-5: then a rule's
//! `fact` predicate with `on_unknown` whose `fact_key` no reading mark of the rule's own package
//! writes is refused the same way, `schedule_invalid` at its `on_unknown`.
//!
//! Last, with or without `pages.json`, the remedy report (Workflow #411 B15, contracts/remedies.md
//! section 10): every package of the root is read by the Runtime's metadata reader, and one
//! `package_bundle_remedy_invalid` warning reports each capability a remedy names that no package
//! of the root provides (`capability_unprovided`), each package on a cycle of remedies and
//! provided capabilities (`cycle`) and each provider of a capability whose providers declare
//! different parameters (`parameters_mismatch`). Nothing is refused.

use crate::codes::ResourceToolingCode;
use crate::package_directory::{PACK_DEADLINE, packs_empty};
use crate::standard_pack_schedule::check_schedule;
use crate::{
    DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES, StandardPackCheckRequest, StandardPackCheckResponse,
    StandardPackRootFiles,
};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Detail, Outcome, RemedyProblem};
use actingcommand_contract::{
    BundleIndex, InterruptThen, LabError, LabErrorClass, LabResult, NavigationProblem,
    NavigationRole, ProvidedCapability, RuntimeErrorCode, STANDARD_PACK_BUNDLE_FILE,
    STANDARD_PACK_DISPLAY_LAYOUT_FILE, STANDARD_PACK_INTERRUPTS_FILE,
    STANDARD_PACK_NAVIGATION_FILE, STANDARD_PACK_PAGES_FILE, STANDARD_PACK_SCHEDULE_FILE,
    StandardPackProblem, StandardPackRoot, StandardPackRootBytes, StandardPackRootIssue,
    parse_standard_pack_schedule, read_standard_pack_root,
};
use actingcommand_execution_kernel::{
    ContainedTaskError, NavigationPackageDescriptor, PreparedContainedTask, RemedyDeclarations,
};
use actingcommand_policy::standard_pack_fact_probes;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The execution mode of a feature package (M-373 §3.4).
const RECOGNIZE_ONLY: &str = "recognize_only";
/// The instance label the cross-check admits packages under.
const CHECK_LABEL: &str = "standard-pack-check";
/// The pack table of `bundle.json` (Workflow #411 A40).
const PACKS_POINTER: &str = "/packs";

/// One root file's bytes and the path they were read from.
pub(crate) struct RootFile {
    pub(crate) path: PathBuf,
    pub(crate) bytes: Vec<u8>,
}

/// The root files `package bundle` was given, read before anything is written.
pub(crate) struct RootFileInputs {
    pages: Option<RootFile>,
    interrupts: Option<RootFile>,
    navigation: Option<RootFile>,
    /// ROUT-L RL-1a: `schedule.json`, checked by the composer and laid beside `bundle.json`.
    schedule: Option<RootFile>,
    /// Workflow #392 DA-4: `display-layout.json`, checked by the root reader and laid beside
    /// `bundle.json`.
    display_layout: Option<RootFile>,
}

impl RootFileInputs {
    /// Reads each given file in full; a file that cannot be read is a usage error naming it.
    pub(crate) fn read(files: &StandardPackRootFiles) -> LabResult<Self> {
        fn read(path: Option<&PathBuf>) -> LabResult<Option<RootFile>> {
            let Some(path) = path else {
                return Ok(None);
            };
            match read_bounded(path)? {
                Some(bytes) => Ok(Some(RootFile {
                    path: path.clone(),
                    bytes,
                })),
                None => Err(LabError::usage(format!(
                    "{}: no such root file",
                    path.display()
                ))),
            }
        }
        Ok(Self {
            pages: read(files.pages.as_ref())?,
            interrupts: read(files.interrupts.as_ref())?,
            navigation: read(files.navigation.as_ref())?,
            schedule: read(files.schedule.as_ref())?,
            display_layout: read(files.display_layout.as_ref())?,
        })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.pages.is_none()
            && self.interrupts.is_none()
            && self.navigation.is_none()
            && self.schedule.is_none()
            && self.display_layout.is_none()
    }

    /// ROUT-L RL-1a: the `schedule.json` given, if any.
    pub(crate) fn schedule(&self) -> Option<&RootFile> {
        self.schedule.as_ref()
    }

    /// Each given file under its root file name, in the order a game root lists them.
    pub(crate) fn files(&self) -> impl Iterator<Item = (&'static str, &RootFile)> {
        [
            (STANDARD_PACK_PAGES_FILE, &self.pages),
            (STANDARD_PACK_INTERRUPTS_FILE, &self.interrupts),
            (STANDARD_PACK_NAVIGATION_FILE, &self.navigation),
            (STANDARD_PACK_SCHEDULE_FILE, &self.schedule),
            (STANDARD_PACK_DISPLAY_LAYOUT_FILE, &self.display_layout),
        ]
        .into_iter()
        .filter_map(|(name, file)| file.as_ref().map(|file| (name, file)))
    }

    /// The game root `bundle` (the written bundle index at `bundle_path`) and these files make,
    /// read by the one structure check of every reader.
    pub(crate) fn root(
        &self,
        game: &str,
        bundle_path: &Path,
        bundle: &[u8],
    ) -> LabResult<StandardPackRoot> {
        let path_of = |name: &str| {
            self.files()
                .find(|(file, _)| *file == name)
                .map_or_else(|| bundle_path.to_owned(), |(_, file)| file.path.clone())
        };
        read_standard_pack_root(
            StandardPackRootBytes {
                bundle: Some(bundle),
                pages: self.pages.as_ref().map(|file| file.bytes.as_slice()),
                interrupts: self.interrupts.as_ref().map(|file| file.bytes.as_slice()),
                navigation: self.navigation.as_ref().map(|file| file.bytes.as_slice()),
                // Workflow #357 DB-1: the bundle is given, so the tables never decide here.
                tables: false,
                display_layout: self
                    .display_layout
                    .as_ref()
                    .map(|file| file.bytes.as_slice()),
            },
            game,
        )
        .map_err(|issue| root_invalid(game, &path_of(issue.file), &issue))?
        .ok_or_else(|| LabError::usage("the standard-pack root holds no bundle.json"))
    }
}

/// `actinglab standard-pack check --root <game root>` (module doc): the game is the root's
/// directory name, its packages are the directories its `bundle.json` lists. Reads only.
pub fn check_standard_pack_root(
    request: StandardPackCheckRequest,
) -> LabResult<StandardPackCheckResponse> {
    let root = std::path::absolute(&request.root)
        .map_err(|error| LabError::usage(format!("{}: {error}", request.root.display())))?;
    let metadata = fs::metadata(&root).map_err(|error| unreadable(&root, &error))?;
    if !metadata.is_dir() {
        return Err(LabError::usage(format!(
            "--root {} is no directory",
            root.display()
        )));
    }
    let game = root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            LabError::usage(format!("--root {} names no game directory", root.display()))
        })?
        .to_owned();
    let mut present = Vec::new();
    let mut files = BTreeMap::new();
    for name in [
        STANDARD_PACK_BUNDLE_FILE,
        STANDARD_PACK_PAGES_FILE,
        STANDARD_PACK_INTERRUPTS_FILE,
        STANDARD_PACK_NAVIGATION_FILE,
        // ROUT-L RL-1a: read beside the others, and checked by the composer after them.
        STANDARD_PACK_SCHEDULE_FILE,
        // Workflow #392 DA-4: checked by the one structure check with the others.
        STANDARD_PACK_DISPLAY_LAYOUT_FILE,
    ] {
        if let Some(bytes) = read_bounded(&root.join(name))? {
            present.push(name);
            files.insert(name, bytes);
        }
    }
    let bytes = |name: &str| files.get(name).map(Vec::as_slice);
    let checked = read_standard_pack_root(
        StandardPackRootBytes {
            bundle: bytes(STANDARD_PACK_BUNDLE_FILE),
            pages: bytes(STANDARD_PACK_PAGES_FILE),
            interrupts: bytes(STANDARD_PACK_INTERRUPTS_FILE),
            navigation: bytes(STANDARD_PACK_NAVIGATION_FILE),
            // Workflow #357 DB-1: a root without bundle.json is refused below either way.
            tables: false,
            display_layout: bytes(STANDARD_PACK_DISPLAY_LAYOUT_FILE),
        },
        &game,
    )
    .map_err(|issue| {
        // Workflow #411 A40: a bundle index refused for its empty pack table says so.
        if lists_no_pack(&issue, bytes(STANDARD_PACK_BUNDLE_FILE)) {
            packs_empty(
                "bundle.json lists no pack",
                STANDARD_PACK_BUNDLE_FILE,
                Some(PACKS_POINTER),
            )
        } else {
            root_invalid(&game, &root.join(issue.file), &issue)
        }
    })?;
    // A directory that holds none of the root files is no game root: its bundle is missing.
    let checked = checked.ok_or_else(|| {
        let issue = StandardPackRootIssue {
            file: STANDARD_PACK_BUNDLE_FILE,
            problem: StandardPackProblem::BundleMissing,
            pointer: String::new(),
        };
        root_invalid(&game, &root.join(STANDARD_PACK_BUNDLE_FILE), &issue)
    })?;
    let mut warnings = cross_check(
        &game,
        &checked,
        &|directory: &str| root.join(directory),
        BTreeMap::new(),
    )?;
    // ROUT-L RL-1a: the schedule rules, composed as the Runtime composes them.
    if let Some(schedule) = bytes(STANDARD_PACK_SCHEDULE_FILE) {
        let maintenance = bytes(STANDARD_PACK_BUNDLE_FILE)
            .and_then(|bundle| serde_json::from_slice::<BundleIndex>(bundle).ok())
            .map(|index| index.maintenance().to_vec())
            .unwrap_or_default();
        warnings.extend(check_schedule(
            &game,
            &checked,
            &maintenance,
            schedule,
            &root.join(STANDARD_PACK_SCHEDULE_FILE),
        )?);
        // UPD-1 fix round 1 (review L6): a store update rule needs a cooldown.
        store_update_cooldowns(
            &game,
            &checked,
            schedule,
            &root.join(STANDARD_PACK_SCHEDULE_FILE),
            &|directory: &str| root.join(directory),
        )?;
        // SS-5: a probe names a key its rule's own package reads.
        probe_keys_marked(
            &game,
            &checked,
            schedule,
            &root.join(STANDARD_PACK_SCHEDULE_FILE),
            &|directory: &str| root.join(directory),
        )?;
    }
    Ok(StandardPackCheckResponse {
        status: "checked".to_owned(),
        root: root.display().to_string(),
        game,
        root_files: present,
        pack_count: checked.registry.len(),
        warnings,
    })
}

/// The cross-check of one game root (module doc): the report's warnings, or the first package
/// that does not fit its part. `pack_directory` is where a registry entry's directory lies;
/// `admitted` holds the packages the caller already admitted by their path. Workflow #411 B15:
/// the remedy report follows, with or without `pages.json`.
pub(crate) fn cross_check(
    game: &str,
    root: &StandardPackRoot,
    pack_directory: &dyn Fn(&str) -> PathBuf,
    admitted: BTreeMap<String, NavigationPackageDescriptor>,
) -> LabResult<Vec<Outcome>> {
    let mut warnings = navigation_check(game, root, pack_directory, admitted)?;
    warnings.extend(remedy_report(root, pack_directory)?);
    Ok(warnings)
}

/// The cross-check of the parts `pages.json`, `navigation.json` and `interrupts.json` name and
/// the report "entry page not in the list". Workflow #411 A25: with or without these files, every
/// package of the registry is admitted, once; without `pages.json` (which the other two need)
/// that is all, and nothing is reported.
fn navigation_check(
    game: &str,
    root: &StandardPackRoot,
    pack_directory: &dyn Fn(&str) -> PathBuf,
    admitted: BTreeMap<String, NavigationPackageDescriptor>,
) -> LabResult<Vec<Outcome>> {
    let mut packages = Packages {
        game,
        root,
        pack_directory,
        admitted,
        resolutions: BTreeMap::new(),
    };
    let Some(pages) = &root.pages else {
        for package_id in root.registry.package_ids() {
            packages.admit_listed(package_id)?;
        }
        // STORE-FG-1: the store-tap report needs no root file.
        let mut warnings = unlocated_store_taps(root, &mut packages)?;
        // Workflow #428 CFL-2: with or without `pages.json`.
        warnings.extend(preprocess_divergences(root, &mut packages)?);
        return Ok(warnings);
    };
    let listed = pages
        .pages
        .iter()
        .map(|page| page.id.as_str())
        .collect::<BTreeSet<_>>();
    for package_id in &pages.features {
        let role = NavigationRole::Features;
        let (package, server) = packages.admit(package_id, role)?;
        let descriptor = package.descriptor();
        let problem = if descriptor.execution_mode() != RECOGNIZE_ONLY {
            Some(NavigationProblem::RoleIneligible)
        } else if descriptor.game() != game || descriptor.server() != server {
            Some(NavigationProblem::IdentityMismatch)
        } else if package
            .page_ids()
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            != listed
        {
            Some(NavigationProblem::PageSetMismatch)
        } else {
            None
        };
        refuse(package_id, role, problem)?;
        packages.resolutions.insert(server, descriptor.resolution());
    }
    if let Some(navigation) = &root.navigation {
        for edge in &navigation.edges {
            let role = NavigationRole::Edge;
            let (package, server) = packages.admit(&edge.package_id, role)?;
            let problem = packages
                .layer_problem(&package, &server, false)
                .or_else(|| {
                    if package.has_application_entry() {
                        Some(NavigationProblem::ApplicationEntry)
                    } else if !(package.entry_holds_page(&edge.from)
                        || package.linear_entry_page() == Some(edge.from.as_str()))
                    {
                        Some(NavigationProblem::EntryMismatch)
                    } else if !package.terminal_holds_page(&edge.to) {
                        Some(NavigationProblem::TerminalMismatch)
                    } else {
                        None
                    }
                });
            refuse(&edge.package_id, role, problem)?;
        }
        for package_id in &navigation.recovery_chain {
            let role = NavigationRole::Chain;
            let (package, server) = packages.admit(package_id, role)?;
            refuse(
                package_id,
                role,
                packages.layer_problem(&package, &server, true),
            )?;
        }
    }
    if let Some(interrupts) = &root.interrupts {
        for interrupt in &interrupts.interrupts {
            for package_id in &interrupt.handlers {
                let role = NavigationRole::Handler;
                let (package, server) = packages.admit(package_id, role)?;
                let problem = packages.layer_problem(&package, &server, true).or_else(|| {
                    (!package.entry_holds_page(&interrupt.page))
                        .then_some(NavigationProblem::EntryMismatch)
                });
                refuse(package_id, role, problem)?;
            }
            if let InterruptThen::Run(run) = &interrupt.then {
                for package_id in run {
                    let role = NavigationRole::ThenRun;
                    let (package, server) = packages.admit(package_id, role)?;
                    refuse(
                        package_id,
                        role,
                        packages.layer_problem(&package, &server, true),
                    )?;
                }
            }
        }
    }
    let mut warnings = Vec::new();
    for package_id in root.registry.package_ids() {
        let package = packages.admit_listed(package_id)?;
        let Some(entry) = package.navigation_entry_pages() else {
            continue;
        };
        let unlisted = entry
            .iter()
            .filter(|page| !listed.contains(page.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if !unlisted.is_empty() {
            warnings.push(
                Outcome::new(ResourceToolingCode::PackageBundleEntryPageUnlisted)
                    .package_id(package_id)
                    .page_ids(unlisted),
            );
        }
    }
    warnings.extend(shadowed_interrupt_pages(
        root,
        &pages.features,
        &mut packages,
    )?);
    warnings.extend(unlocated_store_taps(root, &mut packages)?);
    // Workflow #428 CFL-2.
    warnings.extend(preprocess_divergences(root, &mut packages)?);
    Ok(warnings)
}

/// STORE-FG-1 (design-store-fg §8.1; contracts/package-reference.md, the cross-check): a store's
/// buttons move between devices, so a store step taps a text target located on the frame. One
/// `package_bundle_store_tap_unlocated` warning per package of the registry and declared store
/// operation whose click taps a fixed place (`point`, `rect`, `specific_rect`), in registry order
/// and then declaration order, with or without the navigation files; nothing is refused.
fn unlocated_store_taps(
    root: &StandardPackRoot,
    packages: &mut Packages<'_>,
) -> LabResult<Vec<Outcome>> {
    let mut warnings = Vec::new();
    for package_id in root.registry.package_ids() {
        let package = packages.admit_listed(package_id)?;
        for operation_id in package.store_fixed_taps() {
            warnings.push(
                Outcome::new(ResourceToolingCode::PackageBundleStoreTapUnlocated)
                    .package_id(package_id)
                    .pack_operation_id(operation_id.as_str()),
            );
        }
    }
    Ok(warnings)
}

/// Workflow #428 CFL-2 (design CF v2 §5.6, the resource side's request): a slot's or feature's OCR
/// target whose chain differs from the chain of the layout's shared read it would otherwise take
/// its text from reads on its own, which the author rarely means (colors in another order make
/// another chain). One `package_bundle_preprocess_divergent` warning per package of the registry,
/// target and shared read (`package_id`, `target_id`, and `json_pointer`, the shared read in the
/// package's recognition pack), in registry order and then the pack's order; nothing is refused.
fn preprocess_divergences(
    root: &StandardPackRoot,
    packages: &mut Packages<'_>,
) -> LabResult<Vec<Outcome>> {
    let mut warnings = Vec::new();
    for package_id in root.registry.package_ids() {
        let package = packages.admit_listed(package_id)?;
        for (target_id, shared_read) in package.preprocess_divergences() {
            warnings.push(
                Outcome::new(ResourceToolingCode::PackageBundlePreprocessDivergent)
                    .package_id(package_id)
                    .target_id(target_id.as_str())
                    .json_pointer(shared_read.clone()),
            );
        }
    }
    Ok(warnings)
}

/// Workflow #411 B35 (contracts/package-reference.md, the cross-check): a page a package declares
/// is its own, and a shared interrupt page of the same id does not act for its runs (the
/// Runtime's `RunInterrupts::candidates`; contracts/README.md, "Shared interrupt pages"), so a
/// shared interrupt page added to `interrupts.json` silently does nothing for such a package. One
/// `package_bundle_interrupt_page_shadowed` warning per package of the registry and shared
/// interrupt page it declares, in registry order and then `interrupts.json` order; nothing is
/// refused. A feature package of `pages.json` (its pages are the list's) and a handler of the
/// page (its entry holds the page) declare it by their part and are not reported; UPD-1 (design
/// §7): so does a store update package (`store_update`) for a page whose entry names a `result`
/// (the page it starts from, which the shared interrupt must not take from its own run).
fn shadowed_interrupt_pages(
    root: &StandardPackRoot,
    features: &[String],
    packages: &mut Packages<'_>,
) -> LabResult<Vec<Outcome>> {
    let Some(interrupts) = &root.interrupts else {
        return Ok(Vec::new());
    };
    let mut warnings = Vec::new();
    for package_id in root.registry.package_ids() {
        if features.iter().any(|feature| feature == package_id) {
            continue;
        }
        let package = packages.admit_listed(package_id)?;
        for interrupt in &interrupts.interrupts {
            let handles = interrupt
                .handlers
                .iter()
                .any(|handler| handler == package_id);
            let store_entry = interrupt.result.is_some() && package.declares_store_update();
            if !handles && !store_entry && package.page_ids().contains(&interrupt.page) {
                warnings.push(
                    Outcome::new(ResourceToolingCode::PackageBundleInterruptPageShadowed)
                        .package_id(package_id)
                        .page_id(interrupt.page.as_str()),
                );
            }
        }
    }
    Ok(warnings)
}

/// Workflow #411 B15 (contracts/remedies.md section 10; contracts/package-reference.md, item 5 of
/// the cross-check): each package of the root's registry, in registry order, read by the
/// Runtime's metadata reader ([`PreparedContainedTask::read_remedy_declarations`]), then the
/// findings of [`remedy_findings`]. A package the reader refuses names its directory.
fn remedy_report(
    root: &StandardPackRoot,
    pack_directory: &dyn Fn(&str) -> PathBuf,
) -> LabResult<Vec<Outcome>> {
    let mut packages = Vec::new();
    for package_id in root.registry.package_ids() {
        let entry = root.registry.get(package_id).ok_or_else(|| {
            LabError::package_invalid(format!("{package_id} is no package of the bundle"))
        })?;
        let path = pack_directory(&entry.directory);
        let declarations = PreparedContainedTask::read_remedy_declarations(
            CHECK_LABEL,
            &path,
            Instant::now() + PACK_DEADLINE,
        )
        .map_err(|error| LabError::package_invalid(format!("{}: {error}", path.display())))?;
        packages.push((package_id, declarations));
    }
    Ok(remedy_findings(&packages))
}

/// The remedy findings across the packages of one game root, each one
/// `package_bundle_remedy_invalid` (a warning; nothing is refused, the Runtime stops such a
/// remedy for a person at run time). The names are compared for equality only:
/// - `capability_unprovided`: a capability a remedy names that no package of the root provides,
///   one entry per remedy and capability, with the remedy's `need` and `outcome_key`;
/// - `cycle`: a package whose remedy names a capability a package provides from which the first
///   package is reached again through the packages' remedies and provided capabilities, one entry
///   per package on a cycle, with the first such remedy and capability;
/// - `parameters_mismatch`: a capability whose providers do not all declare the same parameters
///   (each parameter by its name, with its set of options and its default), one entry per provider
///   of it.
fn remedy_findings(packages: &[(&str, RemedyDeclarations)]) -> Vec<Outcome> {
    let code = ResourceToolingCode::PackageBundleRemedyInvalid;
    let mut providers = BTreeMap::<&str, Vec<(usize, &ProvidedCapability)>>::new();
    for (index, (_, declarations)) in packages.iter().enumerate() {
        for provided in &declarations.provides {
            providers
                .entry(provided.capability())
                .or_default()
                .push((index, provided));
        }
    }
    // Each remedy capability of a package, in declaration order, with the packages that provide
    // it (never the package itself, which admission refuses).
    let remedies = packages
        .iter()
        .enumerate()
        .map(|(index, (_, declarations))| {
            declarations
                .remedies
                .iter()
                .flat_map(|(outcome_key, remedy)| {
                    remedy
                        .capabilities()
                        .iter()
                        .map(move |capability| (outcome_key.as_str(), remedy, capability.as_str()))
                })
                .map(|(outcome_key, remedy, capability)| {
                    let served_by = providers
                        .get(capability)
                        .into_iter()
                        .flatten()
                        .map(|(provider, _)| *provider)
                        .filter(|provider| *provider != index)
                        .collect::<Vec<_>>();
                    (outcome_key, remedy, capability, served_by)
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mut warnings = Vec::new();
    for ((package_id, _), remedies) in packages.iter().zip(&remedies) {
        for (outcome_key, remedy, capability, _) in remedies {
            if !providers.contains_key(capability) {
                warnings.push(
                    Outcome::new(code)
                        .package_id(*package_id)
                        .remedy_problem(RemedyProblem::CapabilityUnprovided)
                        .need(remedy.need())
                        .capability(*capability)
                        .outcome_key(*outcome_key),
                );
            }
        }
    }
    // The packages each package reaches through one or more remedy-to-provider steps.
    let reached = (0..packages.len())
        .map(|start| {
            let mut reached = BTreeSet::new();
            let mut pending = vec![start];
            while let Some(package) = pending.pop() {
                for (_, _, _, served_by) in &remedies[package] {
                    for provider in served_by {
                        if reached.insert(*provider) {
                            pending.push(*provider);
                        }
                    }
                }
            }
            reached
        })
        .collect::<Vec<_>>();
    for (index, ((package_id, _), remedies)) in packages.iter().zip(&remedies).enumerate() {
        let on_cycle = remedies.iter().find(|(_, _, _, served_by)| {
            served_by
                .iter()
                .any(|provider| reached[*provider].contains(&index))
        });
        if let Some((outcome_key, remedy, capability, _)) = on_cycle {
            warnings.push(
                Outcome::new(code)
                    .package_id(*package_id)
                    .remedy_problem(RemedyProblem::Cycle)
                    .need(remedy.need())
                    .capability(*capability)
                    .outcome_key(*outcome_key),
            );
        }
    }
    for (capability, provided) in &providers {
        let first = declared_parameters(provided[0].1);
        if provided
            .iter()
            .any(|(_, other)| declared_parameters(other) != first)
        {
            for (provider, _) in provided {
                warnings.push(
                    Outcome::new(code)
                        .package_id(packages[*provider].0)
                        .remedy_problem(RemedyProblem::ParametersMismatch)
                        .capability(*capability),
                );
            }
        }
    }
    warnings
}

/// The parameters a provided capability declares, as the providers of one capability must agree
/// on them: each by its name, with its set of options and its default.
fn declared_parameters(provided: &ProvidedCapability) -> BTreeMap<&str, (BTreeSet<&str>, &str)> {
    provided
        .parameters()
        .iter()
        .map(|parameter| {
            (
                parameter.name(),
                (
                    parameter
                        .options()
                        .iter()
                        .map(String::as_str)
                        .collect::<BTreeSet<_>>(),
                    parameter.default_option(),
                ),
            )
        })
        .collect()
}

/// UPD-1 fix round 1 (review L6; contracts/standard-pack-schedule.md, "The store update rule"):
/// a rule that runs a store update package (`control.json` `store_update`, read by the Runtime's
/// metadata reader) looks again only after its `cooldown_ms` when the store has no new version
/// yet; with 0 it would run the store and the game in turn without a pause. So such a rule of
/// `schedule` (the bytes of `schedule.json` at `path`, already composed by `check_schedule`) whose
/// `cooldown_ms` is 0 is `standard_pack_root_invalid`, `schedule_invalid` at
/// `/rules/<index>/cooldown_ms`. A rule without the member never gets here: the composer refuses
/// it, `cooldown_ms` being a required V2 task member. Other rules are not judged.
pub(crate) fn store_update_cooldowns(
    game: &str,
    root: &StandardPackRoot,
    schedule: &[u8],
    path: &Path,
    pack_directory: &dyn Fn(&str) -> PathBuf,
) -> LabResult<()> {
    let schedule =
        parse_standard_pack_schedule(schedule).map_err(|issue| root_invalid(game, path, &issue))?;
    let mut store_update = BTreeMap::<&str, bool>::new();
    for (index, rule) in schedule.rules.iter().enumerate() {
        if rule
            .body
            .get("cooldown_ms")
            .and_then(serde_json::Value::as_u64)
            != Some(0)
        {
            continue;
        }
        let declares = match store_update.get(rule.package_id.as_str()) {
            Some(declares) => *declares,
            None => {
                let entry = root.registry.get(rule.package_id.as_str()).ok_or_else(|| {
                    LabError::package_invalid(format!(
                        "{} is no package of the bundle",
                        rule.package_id
                    ))
                })?;
                let package = pack_directory(&entry.directory);
                let declares = PreparedContainedTask::read_remedy_declarations(
                    CHECK_LABEL,
                    &package,
                    Instant::now() + PACK_DEADLINE,
                )
                .map_err(|error| {
                    LabError::package_invalid(format!("{}: {error}", package.display()))
                })?
                .store_update;
                store_update.insert(rule.package_id.as_str(), declares);
                declares
            }
        };
        if declares {
            return Err(root_invalid(
                game,
                path,
                &StandardPackRootIssue {
                    file: STANDARD_PACK_SCHEDULE_FILE,
                    problem: StandardPackProblem::ScheduleInvalid,
                    pointer: format!("/rules/{index}/cooldown_ms"),
                },
            ));
        }
    }
    Ok(())
}

/// SS-5 (register §1.223; contracts/standard-pack-schedule.md, "Self-read fact probes"): a rule's
/// `fact` predicate with `on_unknown` admits the rule's task while the fact is unknown, so that
/// the run of the rule's own package reads it. So its `fact_key` must be one the reading marks of
/// that package write (`resource_readings`, read by the Runtime's metadata reader without
/// preparing the package). A key only another package reads, or one no package reads, is
/// `standard_pack_root_invalid`, `schedule_invalid` at the member's pointer
/// (`/rules/<index>/…/on_unknown`) of `schedule` (the bytes of `schedule.json` at `path`, already
/// composed by `check_schedule`). Each package is read once, and only for a rule that writes one.
pub(crate) fn probe_keys_marked(
    game: &str,
    root: &StandardPackRoot,
    schedule: &[u8],
    path: &Path,
    pack_directory: &dyn Fn(&str) -> PathBuf,
) -> LabResult<()> {
    let schedule =
        parse_standard_pack_schedule(schedule).map_err(|issue| root_invalid(game, path, &issue))?;
    let mut marks = BTreeMap::<String, BTreeSet<String>>::new();
    for probe in standard_pack_fact_probes(&schedule) {
        if !marks.contains_key(&probe.package_id) {
            let entry = root
                .registry
                .get(probe.package_id.as_str())
                .ok_or_else(|| {
                    LabError::package_invalid(format!(
                        "{} is no package of the bundle",
                        probe.package_id
                    ))
                })?;
            let package = pack_directory(&entry.directory);
            let read = PreparedContainedTask::read_reading_marks(
                CHECK_LABEL,
                &package,
                Instant::now() + PACK_DEADLINE,
            )
            .map_err(|error| {
                LabError::package_invalid(format!("{}: {error}", package.display()))
            })?;
            marks.insert(
                probe.package_id.clone(),
                read.fact_keys.into_iter().collect(),
            );
        }
        let marked = probe.fact_key.as_ref().is_some_and(|key| {
            marks
                .get(&probe.package_id)
                .is_some_and(|keys| keys.contains(key))
        });
        if !marked {
            return Err(root_invalid(
                game,
                path,
                &StandardPackRootIssue {
                    file: STANDARD_PACK_SCHEDULE_FILE,
                    problem: StandardPackProblem::ScheduleInvalid,
                    pointer: probe.pointer,
                },
            ));
        }
    }
    Ok(())
}

/// The packages of one game root, each admitted once by its path.
struct Packages<'a> {
    game: &'a str,
    root: &'a StandardPackRoot,
    pack_directory: &'a dyn Fn(&str) -> PathBuf,
    admitted: BTreeMap<String, NavigationPackageDescriptor>,
    /// Each server's feature package resolution.
    resolutions: BTreeMap<String, (u32, u32)>,
}

impl Packages<'_> {
    /// The package `package_id` names for `role`, admitted (a refusal of its admission is its
    /// `admission_failed`), and its bundle server.
    fn admit(
        &mut self,
        package_id: &str,
        role: NavigationRole,
    ) -> LabResult<(NavigationPackageDescriptor, String)> {
        let entry = self.root.registry.get(package_id).ok_or_else(|| {
            LabError::package_invalid(format!("{package_id} is no package of the bundle"))
        })?;
        let server = entry.server.clone();
        if let Some(package) = self.admitted.get(package_id) {
            return Ok((package.clone(), server));
        }
        let path = (self.pack_directory)(&entry.directory);
        let package = PreparedContainedTask::describe_navigation_path(
            CHECK_LABEL,
            &path,
            Instant::now() + PACK_DEADLINE,
        )
        .map_err(|error| {
            navigation_package_invalid(
                package_id,
                role,
                NavigationProblem::AdmissionFailed,
                Some(error.outcome()),
            )
        })?;
        self.admitted.insert(package_id.to_owned(), package.clone());
        Ok((package, server))
    }

    /// A package of the registry, admitted unless it already was (a refusal names its
    /// directory).
    fn admit_listed(&mut self, package_id: &str) -> LabResult<NavigationPackageDescriptor> {
        if let Some(package) = self.admitted.get(package_id) {
            return Ok(package.clone());
        }
        let entry = self.root.registry.get(package_id).ok_or_else(|| {
            LabError::package_invalid(format!("{package_id} is no package of the bundle"))
        })?;
        let path = (self.pack_directory)(&entry.directory);
        let package = PreparedContainedTask::describe_navigation_path(
            CHECK_LABEL,
            &path,
            Instant::now() + PACK_DEADLINE,
        )
        .map_err(|error| package_refused(&entry.directory, &path, &error))?;
        self.admitted.insert(package_id.to_owned(), package.clone());
        Ok(package)
    }

    /// Why `package` of `server` is no layer of a run of that server, as the Runtime judges a
    /// layer (`own_prerequisite_refused` for a recovery chain, handler or `then.run` package,
    /// which declares no prerequisite package of its own).
    fn layer_problem(
        &self,
        package: &NavigationPackageDescriptor,
        server: &str,
        own_prerequisite_refused: bool,
    ) -> Option<NavigationProblem> {
        let descriptor = package.descriptor();
        let own = descriptor.resolution();
        let resolution = self.resolutions.get(server).copied().unwrap_or(own);
        match descriptor.prerequisite_incompatibility_with(
            self.game,
            server,
            resolution,
            own_prerequisite_refused,
        ) {
            None => None,
            Some("game" | "server" | "resolution") => Some(NavigationProblem::IdentityMismatch),
            Some("return_home_declares_prerequisite") => {
                Some(NavigationProblem::PrerequisiteDeclared)
            }
            Some(_) => Some(NavigationProblem::RoleIneligible),
        }
    }
}

fn refuse(
    package_id: &str,
    role: NavigationRole,
    problem: Option<NavigationProblem>,
) -> LabResult<()> {
    match problem {
        Some(problem) => Err(navigation_package_invalid(package_id, role, problem, None)),
        None => Ok(()),
    }
}

/// `package_bundle_navigation_package_invalid`: the package cannot be the `role` package.
fn navigation_package_invalid(
    package_id: &str,
    role: NavigationRole,
    problem: NavigationProblem,
    cause: Option<Outcome>,
) -> LabError {
    let code = ResourceToolingCode::PackageBundleNavigationPackageInvalid;
    let outcome = Outcome::new(code)
        .package_id(package_id)
        .navigation_role(role)
        .navigation_problem(problem);
    let outcome = match cause {
        Some(cause) => outcome.caused_by(cause),
        None => outcome,
    };
    LabError::new(
        LabErrorClass::UsageValidation,
        code.as_str(),
        format!("{package_id} cannot be the {role} package: {problem}"),
        &[],
    )
    .with_details(json!({"outcome": outcome.envelope(Detail::Full)}))
}

/// `package_invalid` for a package of the registry whose admission refused it (Workflow #411
/// A25: the developer message names its directory `path`). Workflow #411 A41: its outcome names
/// the directory below the game root (`path`), caused by the admission's own outcome, which
/// names the refused file inside the package and the parser's position where it gives them.
fn package_refused(directory: &str, path: &Path, error: &ContainedTaskError) -> LabError {
    let outcome = Outcome::new(RuntimeErrorCode::PackageInvalid)
        .path(directory)
        .caused_by(error.outcome());
    LabError::package_invalid(format!("{}: {error}", path.display()))
        .with_details(json!({"outcome": outcome.envelope(Detail::Full)}))
}

/// Workflow #411 A40: whether `issue` is the bundle index refusing `bundle`, the bytes of
/// `bundle.json`, because its pack table holds no pack (the rest of the refusal is unchanged).
fn lists_no_pack(issue: &StandardPackRootIssue, bundle: Option<&[u8]>) -> bool {
    issue.file == STANDARD_PACK_BUNDLE_FILE
        && issue.problem == StandardPackProblem::BundleInvalid
        && bundle
            .and_then(|bytes| serde_json::from_slice::<BundleIndex>(bytes).ok())
            .is_some_and(|index| index.packs.is_empty())
}

/// `standard_pack_root_invalid`: the root file at `path` breaks its shape, a limit or a
/// reference.
pub(crate) fn root_invalid(game: &str, path: &Path, issue: &StandardPackRootIssue) -> LabError {
    let code = SharedCode::StandardPackRootInvalid;
    let outcome = Outcome::new(code)
        .game(game)
        .path(path)
        .standard_pack_problem(issue.problem)
        .json_pointer(issue.pointer.clone());
    LabError::new(
        LabErrorClass::UsageValidation,
        code.as_str(),
        format!("{}: {issue}", path.display()),
        &[],
    )
    .with_details(json!({"outcome": outcome.envelope(Detail::Full)}))
}

fn unreadable(path: &Path, error: &io::Error) -> LabError {
    LabError::usage(format!("{}: {error}", path.display()))
}

/// A root file's bytes, `None` when it does not exist. A file that cannot be read, or that
/// exceeds the package input byte limit, is a usage error naming it.
pub(crate) fn read_bounded(path: &Path) -> LabResult<Option<Vec<u8>>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(unreadable(path, &error)),
    };
    let limit = DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES as u64;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| unreadable(path, &error))?;
    if bytes.len() as u64 > limit {
        return Err(LabError::usage(format!(
            "{}: the root file exceeds the package input byte limit",
            path.display()
        )));
    }
    Ok(Some(bytes))
}
