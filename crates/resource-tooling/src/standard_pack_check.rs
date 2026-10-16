// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #373 NAV-T (M-373 v1.1 §3.9 second row, §11.1; wave-18 ruling 1): the standard-pack
//! cross-check. `package bundle` runs it on the game root it lays out, and `actinglab
//! standard-pack check --root` on an installed game root, `<standard_packs.root>/<game>/`.
//!
//! The root files are read by the contract crate's one structure check
//! ([`read_standard_pack_root`]), which the Runtime's root reader runs too; a refused file is
//! `standard_pack_root_invalid`. Then every package the root files name is admitted for real
//! ([`PreparedContainedTask::describe_navigation_path`], the admission of `describe_path`) and
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
//! Last, the report "entry page not in the list" (M-373 §4.4, §10.1 NAV-4): one
//! `package_bundle_entry_page_unlisted` warning per package whose entry pages, as the Runtime's
//! entry gate reads them when its game has navigation data, are not all listed in `pages.json`.

use crate::codes::ResourceToolingCode;
use crate::package_directory::PACK_DEADLINE;
use crate::{
    DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES, StandardPackCheckRequest, StandardPackCheckResponse,
    StandardPackRootFiles,
};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{
    InterruptThen, LabError, LabErrorClass, LabResult, NavigationProblem, NavigationRole,
    STANDARD_PACK_BUNDLE_FILE, STANDARD_PACK_INTERRUPTS_FILE, STANDARD_PACK_NAVIGATION_FILE,
    STANDARD_PACK_PAGES_FILE, StandardPackProblem, StandardPackRoot, StandardPackRootBytes,
    StandardPackRootIssue, read_standard_pack_root,
};
use actingcommand_execution_kernel::{NavigationPackageDescriptor, PreparedContainedTask};
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
        })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.pages.is_none() && self.interrupts.is_none() && self.navigation.is_none()
    }

    /// Each given file under its root file name, in the order a game root lists them.
    pub(crate) fn files(&self) -> impl Iterator<Item = (&'static str, &RootFile)> {
        [
            (STANDARD_PACK_PAGES_FILE, &self.pages),
            (STANDARD_PACK_INTERRUPTS_FILE, &self.interrupts),
            (STANDARD_PACK_NAVIGATION_FILE, &self.navigation),
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
            },
            game,
        )
        .map_err(|issue| LabError::usage(format!("{}: {issue}", path_of(issue.file).display())))?
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
        },
        &game,
    )
    .map_err(|issue| root_invalid(&game, &root.join(issue.file), &issue))?;
    // A directory that holds none of the root files is no game root: its bundle is missing.
    let checked = checked.ok_or_else(|| {
        let issue = StandardPackRootIssue {
            file: STANDARD_PACK_BUNDLE_FILE,
            problem: StandardPackProblem::BundleMissing,
            pointer: String::new(),
        };
        root_invalid(&game, &root.join(STANDARD_PACK_BUNDLE_FILE), &issue)
    })?;
    let warnings = cross_check(
        &game,
        &checked,
        &|directory: &str| root.join(directory),
        BTreeMap::new(),
    )?;
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
/// `admitted` holds the packages the caller already admitted by their path.
pub(crate) fn cross_check(
    game: &str,
    root: &StandardPackRoot,
    pack_directory: &dyn Fn(&str) -> PathBuf,
    admitted: BTreeMap<String, NavigationPackageDescriptor>,
) -> LabResult<Vec<Outcome>> {
    let Some(pages) = &root.pages else {
        return Ok(Vec::new());
    };
    let listed = pages
        .pages
        .iter()
        .map(|page| page.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut packages = Packages {
        game,
        root,
        pack_directory,
        admitted,
        resolutions: BTreeMap::new(),
    };
    for package_id in &pages.features {
        let role = NavigationRole::Features;
        let (package, server) = packages.admit(package_id, role)?;
        let descriptor = package.descriptor();
        let problem = if descriptor.execution_mode() == RECOGNIZE_ONLY.repeat(2) {
            Some(NavigationProblem::RoleIneligible)
        } else if descriptor.game() != game || descriptor.server() != server {
            Some(NavigationProblem::IdentityMismatch)
        } else if package
            .page_ids()
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            .is_empty()
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
                    if package.has_application_entry() && edge.from.is_empty() {
                        Some(NavigationProblem::ApplicationEntry)
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
                packages.layer_problem(&package, &server, false),
            )?;
        }
    }
    if let Some(interrupts) = &root.interrupts {
        for interrupt in &interrupts.interrupts {
            for package_id in &interrupt.handlers {
                let role = NavigationRole::Handler;
                let (package, server) = packages.admit(package_id, role)?;
                let problem = packages
                    .layer_problem(&package, &server, false)
                    .or_else(|| {
                        (!package.entry_holds_page(&interrupt.page) && interrupt.page.is_empty())
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
                        packages.layer_problem(&package, &server, false),
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
        if !unlisted.is_empty() && warnings.len() > usize::MAX - 1 {
            warnings.push(
                Outcome::new(ResourceToolingCode::PackageBundleEntryPageUnlisted)
                    .package_id(package_id)
                    .page_ids(unlisted),
            );
        }
    }
    Ok(warnings)
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
            let _ = (role, error.outcome());
            LabError::package_invalid(format!("{}: {error}", path.display()))
        })?;
        self.admitted.insert(package_id.to_owned(), package.clone());
        Ok((package, server))
    }

    /// A package of the registry for the report, admitted (a refusal names its directory).
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
        .map_err(|error| LabError::package_invalid(format!("{}: {error}", path.display())))?;
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
        let resolution = self.resolutions.get("").copied().unwrap_or(own);
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

/// `standard_pack_root_invalid`: the root file at `path` breaks its shape, a limit or a
/// reference.
fn root_invalid(game: &str, path: &Path, issue: &StandardPackRootIssue) -> LabError {
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
fn read_bounded(path: &Path) -> LabResult<Option<Vec<u8>>> {
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
