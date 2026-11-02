// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #407 LOAD-1 (register §1.257; design `rd-1031/check-speed/model-loading.md` §2.3):
//! `actinglab package loading --root-file <loading.json> --packs-root <directory> (--write |
//! --check)`, and the same check run by `package bundle --loading` and `actinglab standard-pack
//! check`.
//!
//! The root file is read by the contract crate's one check ([`parse_standard_pack_loading`]), for
//! the game the packs declare; every template it declares is read from the `loading/` directory
//! beside it and must be a PNG that is not of one colour throughout. Every pack directory is read
//! with the loader's own snapshot and admitted by its path, as `package bundle` admits it; its
//! copy is the root file's pages for its server ([`StandardPackLoading::for_server`]), none when
//! `exclude_packages` names it or no page applies to its server. A pack that takes pages and
//! declares another coordinate space, or a page or target id of the copy itself, is refused, as
//! are a pack of another game and an `exclude_packages` entry that names no pack. Every issue is
//! one `standard_pack_root_invalid` with its `standard_pack_problem` and `json_pointer` (and its
//! `package_id` when it is a pack's), all of them listed in `details.issues`, the first also as
//! `details.outcome`; the refusal writes nothing.
//!
//! A copy is current when its `resources/loading.json` reads ([`parse_pack_loading`]) as the copy
//! the root file makes and its `resources/loading/` holds exactly the templates the copy reads,
//! byte for byte; a pack that takes no page is current when it carries neither. `--check` refuses
//! with one `loading_pack_stale` per pack that is not current, at the first file that differs.
//! `--write` writes the files of every pack that is not, each through a `.part` file renamed over
//! it, the templates before `resources/loading.json`; it removes nothing, so a pack that holds a
//! file of a copy it no longer takes is refused first, naming the file, and nothing is written.
//!
//! Without a root file ([`stale_copies`]), a pack that carries a copy is stale: `package bundle`
//! without `--loading` and `standard-pack check` of a game root without `loading.json` refuse it,
//! so no copy is ever bundled or installed unchecked.

use crate::codes::ResourceToolingCode;
use crate::package_directory::{PACK_DEADLINE, pack_directories};
use crate::standard_pack_check::read_bounded;
use crate::{
    PackLoadingReport, PackLoadingState, PackageLoadingMode, PackageLoadingRequest,
    PackageLoadingResponse,
};
use actingcommand_contract::StandardPackProblem as Problem;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{
    LabError, LabErrorClass, LabResult, LoadingCoordinateSpace, PACK_LOADING_DIRECTORY,
    PACK_LOADING_FILE, PackLoading, StandardPackLoading, StandardPackRootIssue, pack_loading_entry,
    parse_pack_loading, parse_standard_pack_loading,
};
use actingcommand_execution_kernel::canonical_page_anchor;
use actingcommand_pack_containment::{Containment, ContainmentError, InstanceId};
use actingcommand_recognition::Scene;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The instance label packs are admitted under.
const LOADING_LABEL: &str = "package-loading";

/// `actinglab package loading` (module doc).
pub fn package_loading(request: PackageLoadingRequest) -> LabResult<PackageLoadingResponse> {
    let packs_root = std::path::absolute(&request.packs_root)
        .map_err(|error| LabError::usage(format!("{}: {error}", request.packs_root.display())))?;
    let packs = pack_directories(&packs_root)?
        .iter()
        .map(|directory| read_pack(directory.as_path()))
        .collect::<LabResult<Vec<_>>>()?;
    // `pack_directories` refuses a root without a pack directory.
    let game = packs
        .first()
        .map(|pack| pack.game.clone())
        .ok_or_else(|| LabError::usage("--packs-root holds no pack directory"))?;
    let root = LoadingRoot::read(&request.root_file, &game)?;
    let plans = plan(&root, packs)?;
    let status = match request.mode {
        PackageLoadingMode::Check => {
            refuse_stale(&root, &plans)?;
            "checked"
        }
        // The preview refuses what `--write` refuses.
        PackageLoadingMode::Preview => {
            refuse_leftovers(&root, &plans)?;
            "preview"
        }
        PackageLoadingMode::Write => {
            refuse_leftovers(&root, &plans)?;
            "written"
        }
    };
    let mut written = Vec::new();
    let mut reports = Vec::new();
    for plan in &plans {
        let current = plan.stale_path().is_none();
        let state = if current {
            PackLoadingState::Current
        } else if request.mode == PackageLoadingMode::Write {
            plan.write(&mut written)?;
            PackLoadingState::Written
        } else {
            PackLoadingState::Stale
        };
        reports.push(PackLoadingReport {
            package_id: plan.pack.package_id.clone(),
            server: plan.pack.server.clone(),
            pages: plan
                .copy
                .iter()
                .flat_map(|copy| copy.pages.iter().map(|page| page.id.clone()))
                .collect(),
            excluded: plan.excluded,
            state,
        });
    }
    Ok(PackageLoadingResponse {
        status: status.to_owned(),
        game,
        root_file: root.path.display().to_string(),
        max_ms: root.loading.max_ms,
        packs: reports,
    })
}

/// `package bundle --loading` and `standard-pack check` of a game root with `loading.json`:
/// the check of `--check` on the packs at `directories`, for `game`. The root file read, with
/// its templates, to lay beside `bundle.json`.
pub(crate) fn check_loading(
    root_file: &Path,
    game: &str,
    directories: &[PathBuf],
) -> LabResult<LoadingRoot> {
    let packs = directories
        .iter()
        .map(|directory| read_pack(directory.as_path()))
        .collect::<LabResult<Vec<_>>>()?;
    let root = LoadingRoot::read(root_file, game)?;
    let plans = plan(&root, packs)?;
    refuse_stale(&root, &plans)?;
    Ok(root)
}

/// `package bundle` without `--loading` and `standard-pack check` of a game root without
/// `loading.json`: each pack of `packs` (its `package_id` and directory) that carries a loading
/// copy is stale, at its first such file; refused for `game` with every one of them.
pub(crate) fn stale_copies(game: &str, packs: &[(String, PathBuf)]) -> LabResult<()> {
    let mut issues = Vec::new();
    for (package_id, directory) in packs {
        for relative in [PACK_LOADING_FILE, PACK_LOADING_DIRECTORY] {
            let path = pack_path(directory, relative);
            if fs::symlink_metadata(&path).is_ok() {
                issues.push(issue(
                    game,
                    path,
                    Problem::LoadingPackStale,
                    "",
                    Some(package_id.as_str()),
                ));
                break;
            }
        }
    }
    if issues.is_empty() {
        return Ok(());
    }
    Err(refused(
        "packs carry a loading copy, and no loading.json was given to check it against",
        &issues,
    ))
}

/// Each pack directory of `package bundle --packs-root` with the `package_id` its `control.json`
/// declares, to name it in a refusal of [`stale_copies`]; the directory's name when its control
/// declares none that can be read, a pack `package bundle` refuses after, naming the reason.
pub(crate) fn named_packs(directories: &[PathBuf]) -> Vec<(String, PathBuf)> {
    directories
        .iter()
        .map(|directory| {
            let declared = read_bounded(&directory.join("control.json"))
                .ok()
                .flatten()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|control| control.get("package_id")?.as_str().map(str::to_owned));
            let name = declared.unwrap_or_else(|| {
                directory
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
            (name, directory.clone())
        })
        .collect()
}

/// A game's `loading.json`, checked, with the bytes of every template it declares.
pub(crate) struct LoadingRoot {
    pub(crate) path: PathBuf,
    pub(crate) bytes: Vec<u8>,
    loading: StandardPackLoading,
    /// Each declared template, by its path `loading/<file>.png`.
    pub(crate) templates: BTreeMap<String, Vec<u8>>,
}

impl LoadingRoot {
    /// Reads `path` for `game` (module doc): the contract's check, then every template.
    fn read(path: &Path, game: &str) -> LabResult<Self> {
        let path = std::path::absolute(path)
            .map_err(|error| LabError::usage(format!("{}: {error}", path.display())))?;
        let bytes = read_bounded(&path)?
            .ok_or_else(|| LabError::usage(format!("{}: no such root file", path.display())))?;
        let loading = parse_standard_pack_loading(&bytes, Some(game)).map_err(|issues| {
            let issues = issues
                .iter()
                .map(|found| root_issue(game, &path, found))
                .collect::<Vec<_>>();
            refused("loading.json breaks its rules", &issues)
        })?;
        let directory = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut templates = BTreeMap::new();
        let mut issues = Vec::new();
        let mut problems = Vec::new();
        for target in &loading.targets {
            let Some(template) = target.template() else {
                continue;
            };
            if templates.contains_key(template) {
                continue;
            }
            let file = pack_path(&directory, template);
            match read_bounded(&file) {
                Ok(Some(bytes)) => match template_problem(&bytes) {
                    None => {
                        templates.insert(template.to_owned(), bytes);
                    }
                    Some(problem) => problems.push(format!("{}: {problem}", file.display())),
                },
                Ok(None) => problems.push(format!("{}: no such template", file.display())),
                Err(error) => problems.push(error.message),
            }
            if !templates.contains_key(template) {
                let pointer = loading
                    .target_pointer(target.id(), "template")
                    .unwrap_or_default();
                issues.push(issue(
                    game,
                    &path,
                    Problem::LoadingTemplateInvalid,
                    pointer,
                    None,
                ));
            }
        }
        if !issues.is_empty() {
            return Err(refused(
                &format!(
                    "loading.json declares templates it cannot use ({})",
                    problems.join("; ")
                ),
                &issues,
            ));
        }
        Ok(Self {
            path,
            bytes,
            loading,
            templates,
        })
    }
}

/// Why a template cannot tell a loading page: it is no PNG this build decodes, or every pixel is
/// of one colour (a black or white screen is no loading page); `None` when it can.
fn template_problem(bytes: &[u8]) -> Option<String> {
    let scene = match Scene::from_png(bytes) {
        Ok(scene) => scene,
        Err(error) => return Some(format!("no PNG this build decodes: {error}")),
    };
    let (pixels, _) = scene.rgb8_pixels().as_chunks::<3>();
    let mut pixels = pixels.iter();
    let first = pixels.next();
    if first.is_none() || pixels.all(|pixel| Some(pixel) == first) {
        return Some("every pixel is of one colour".to_owned());
    }
    None
}

/// What one pack declares, as the loader admits it, and the loading copy it carries.
struct LoadingPack {
    directory: PathBuf,
    package_id: String,
    game: String,
    server: String,
    /// The `coordinate_space` of its recognition pack, that of its entry task.
    coordinate_space: LoadingCoordinateSpace,
    /// Its pages, each without the game prefix.
    page_ids: BTreeSet<String>,
    /// Its recognition targets.
    target_ids: BTreeSet<String>,
    /// Its copy's files, by `/`-separated path inside the pack.
    copy: BTreeMap<String, Vec<u8>>,
}

/// Reads the pack at `directory` with the loader's snapshot and admits it by its path, as
/// `package bundle` does.
fn read_pack(directory: &Path) -> LabResult<LoadingPack> {
    let refused_pack = |error: ContainmentError| {
        LabError::package_invalid(format!("{}: {error}", directory.display()))
    };
    let unreadable = |what: &str| {
        LabError::package_invalid(format!(
            "{}: the admitted pack has no readable {what}",
            directory.display()
        ))
    };
    let deadline = Instant::now() + PACK_DEADLINE;
    let mut containment = Containment::for_metadata_validation();
    let copy = containment
        .snapshot_content_directory(directory, deadline)
        .map_err(refused_pack)?
        .into_iter()
        .filter(|(path, _)| pack_loading_entry(path))
        .collect::<BTreeMap<_, _>>();
    let instance = InstanceId::new(LOADING_LABEL).map_err(refused_pack)?;
    let bundle = containment
        .load_path(&instance, directory, false, deadline)
        .map_err(refused_pack)?;
    let control = bundle.control().ok_or_else(|| unreadable("control.json"))?;
    let text = |key: &str| {
        control
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| unreadable(&format!("control.json {key}")))
    };
    let (package_id, game, server) = (text("package_id")?, text("game")?, text("server")?);
    let document = |path: Option<&str>, what: &str| {
        path.and_then(|path| bundle.entry(path))
            .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
            .ok_or_else(|| unreadable(what))
    };
    let recognition = document(bundle.recognition_pack_path(), "recognition pack")?;
    let pages = document(bundle.pages_path(), "page set")?;
    let side = |name: &str| {
        recognition["coordinate_space"][name]
            .as_u64()
            .and_then(|side| u32::try_from(side).ok())
            .ok_or_else(|| unreadable("coordinate_space"))
    };
    let coordinate_space = LoadingCoordinateSpace {
        width: side("width")?,
        height: side("height")?,
    };
    let ids = |document: &Value, member: &str| {
        document[member]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry["id"].as_str())
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
    };
    let target_ids = ids(&recognition, "targets");
    let page_ids = ids(&pages, "pages")
        .iter()
        .map(|page| canonical_page_anchor(&game, page))
        .collect();
    Ok(LoadingPack {
        directory: directory.to_owned(),
        package_id,
        game,
        server,
        coordinate_space,
        page_ids,
        target_ids,
        copy,
    })
}

/// One pack and the copy the root file makes for it.
struct PackPlan {
    pack: LoadingPack,
    excluded: bool,
    copy: Option<PackLoading>,
    /// The copy's files by path inside the pack: `resources/loading.json`, then each template.
    expected: BTreeMap<String, Vec<u8>>,
}

impl PackPlan {
    /// The path inside the pack of the first file of its copy that is not the one the root file
    /// makes, in path order; `None` when the copy is current.
    fn stale_path(&self) -> Option<&str> {
        let paths = self
            .expected
            .keys()
            .chain(self.pack.copy.keys())
            .collect::<BTreeSet<_>>();
        paths
            .into_iter()
            .find(|path| !self.file_current(path))
            .map(String::as_str)
    }

    /// Whether the pack's file at `path` is the one its copy makes: the copy document reads as
    /// the expected copy; every other file is byte for byte the template.
    fn file_current(&self, path: &str) -> bool {
        match (self.pack.copy.get(path), self.expected.get(path)) {
            (Some(actual), Some(expected)) if path == PACK_LOADING_FILE => {
                actual == expected || parse_pack_loading(actual).ok() == self.copy
            }
            (Some(actual), Some(expected)) => actual == expected,
            _ => false,
        }
    }

    /// The files of the pack's copy that its new copy does not hold, which `--write` does not
    /// remove.
    fn leftovers(&self) -> impl Iterator<Item = &str> {
        self.pack
            .copy
            .keys()
            .filter(|path| !self.expected.contains_key(*path))
            .map(String::as_str)
    }

    /// Writes every file of the copy that is not current, the templates first; each written
    /// file joins `written`.
    fn write(&self, written: &mut Vec<PathBuf>) -> LabResult<()> {
        let mut order = self
            .expected
            .keys()
            .filter(|path| path.as_str() != PACK_LOADING_FILE)
            .collect::<Vec<_>>();
        order.extend(
            self.expected
                .keys()
                .filter(|path| *path == PACK_LOADING_FILE),
        );
        for path in order {
            if self.file_current(path) {
                continue;
            }
            let target = pack_path(&self.pack.directory, path);
            let bytes = &self.expected[path];
            write_file(&target, bytes).map_err(|error| {
                LabError::new(
                    LabErrorClass::UsageValidation,
                    ResourceToolingCode::PackageBundleWriteFailed.as_str(),
                    format!(
                        "{}: {error}; written before it: {}; nothing is removed",
                        target.display(),
                        if written.is_empty() {
                            "none".to_owned()
                        } else {
                            written
                                .iter()
                                .map(|file| file.display().to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    ),
                    &[],
                )
            })?;
            written.push(target);
        }
        Ok(())
    }
}

/// The copy of every pack, or a refusal listing every issue of the packs (module doc).
fn plan(root: &LoadingRoot, packs: Vec<LoadingPack>) -> LabResult<Vec<PackPlan>> {
    let loading = &root.loading;
    let game = loading.game.as_str();
    let mut issues = Vec::new();
    for (index, excluded) in loading.exclude_packages.iter().enumerate() {
        if !packs.iter().any(|pack| &pack.package_id == excluded) {
            issues.push(issue(
                game,
                &root.path,
                Problem::PackageNotListed,
                format!("/exclude_packages/{index}"),
                None,
            ));
        }
    }
    let mut plans = Vec::new();
    for pack in packs {
        let package_id = Some(pack.package_id.as_str());
        if pack.game != loading.game {
            issues.push(issue(
                game,
                &root.path,
                Problem::GameMismatch,
                "/game",
                package_id,
            ));
        }
        let excluded = loading.excludes(&pack.package_id);
        let copy = if excluded {
            None
        } else {
            loading.for_server(&pack.server)
        };
        let mut expected = BTreeMap::new();
        if let Some(copy) = &copy {
            if pack.coordinate_space != copy.coordinate_space {
                issues.push(issue(
                    game,
                    &root.path,
                    Problem::LoadingCoordinateMismatch,
                    "/coordinate_space",
                    package_id,
                ));
            }
            for page in &copy.pages {
                if pack.page_ids.contains(&page.id) {
                    issues.push(issue(
                        game,
                        &root.path,
                        Problem::LoadingPageCollision,
                        loading.page_pointer(&page.id).unwrap_or_default(),
                        package_id,
                    ));
                }
            }
            for target in &copy.targets {
                if pack.target_ids.contains(target.id()) {
                    issues.push(issue(
                        game,
                        &root.path,
                        Problem::LoadingTargetCollision,
                        loading
                            .target_pointer(target.id(), "id")
                            .unwrap_or_default(),
                        package_id,
                    ));
                }
            }
            for template in copy.templates() {
                let bytes = root.templates.get(template).ok_or_else(|| {
                    LabError::usage(format!("{template}: the template was not read"))
                })?;
                expected.insert(format!("resources/{template}"), bytes.clone());
            }
            let document = copy.to_bytes().map_err(|error| {
                LabError::usage(format!("the copy of {}: {error}", pack.package_id))
            })?;
            expected.insert(PACK_LOADING_FILE.to_owned(), document);
        }
        plans.push(PackPlan {
            pack,
            excluded,
            copy,
            expected,
        });
    }
    if !issues.is_empty() {
        return Err(refused(
            &format!("{} cannot be copied into these packs", root.path.display()),
            &issues,
        ));
    }
    Ok(plans)
}

/// `--check`: one `loading_pack_stale` per pack whose copy is not current.
fn refuse_stale(root: &LoadingRoot, plans: &[PackPlan]) -> LabResult<()> {
    let issues = plans
        .iter()
        .filter_map(|plan| {
            plan.stale_path().map(|path| {
                issue(
                    &root.loading.game,
                    pack_path(&plan.pack.directory, path),
                    Problem::LoadingPackStale,
                    "",
                    Some(plan.pack.package_id.as_str()),
                )
            })
        })
        .collect::<Vec<_>>();
    if issues.is_empty() {
        return Ok(());
    }
    Err(refused(
        &format!(
            "packs whose loading copy is not the one {} makes; run package loading --write",
            root.path.display()
        ),
        &issues,
    ))
}

/// `--write`: one `loading_pack_stale` per file a pack holds of a copy it no longer takes, which
/// `--write` would leave in place; nothing is written while there is one.
fn refuse_leftovers(root: &LoadingRoot, plans: &[PackPlan]) -> LabResult<()> {
    let issues = plans
        .iter()
        .flat_map(|plan| {
            plan.leftovers().map(|path| {
                issue(
                    &root.loading.game,
                    pack_path(&plan.pack.directory, path),
                    Problem::LoadingPackStale,
                    "",
                    Some(plan.pack.package_id.as_str()),
                )
            })
        })
        .collect::<Vec<_>>();
    if issues.is_empty() {
        return Ok(());
    }
    Err(refused(
        "packs hold loading files their copy no longer has; package loading removes nothing, so \
         remove them and run it again",
        &issues,
    ))
}

/// One issue of `loading.json` itself, at `path`.
fn root_issue(game: &str, path: &Path, found: &StandardPackRootIssue) -> Outcome {
    issue(game, path, found.problem, found.pointer.clone(), None)
}

/// `standard_pack_root_invalid` for one issue.
fn issue(
    game: &str,
    path: impl AsRef<Path>,
    problem: Problem,
    pointer: impl Into<String>,
    package_id: Option<&str>,
) -> Outcome {
    let outcome = Outcome::new(SharedCode::StandardPackRootInvalid)
        .game(game)
        .path(path)
        .standard_pack_problem(problem)
        .json_pointer(pointer.into());
    match package_id {
        Some(package_id) => outcome.package_id(package_id),
        None => outcome,
    }
}

/// The refusal of `issues` (never empty): `standard_pack_root_invalid`, its developer message
/// naming each issue, `details.outcome` the first and `details.issues` every one.
fn refused(message: &str, issues: &[Outcome]) -> LabError {
    let code = SharedCode::StandardPackRootInvalid;
    let lines = issues
        .iter()
        .map(|outcome| {
            let value = |key: &str| outcome.value(key).and_then(Value::as_str).unwrap_or("");
            let package = match outcome.value("package_id").and_then(Value::as_str) {
                Some(package_id) => format!(" (package {package_id})"),
                None => String::new(),
            };
            format!(
                "{}: {} at {:?}{package}",
                value("path"),
                value("standard_pack_problem"),
                value("json_pointer")
            )
        })
        .collect::<Vec<_>>();
    let envelopes = issues
        .iter()
        .map(|outcome| outcome.envelope(Detail::Full))
        .collect::<Vec<_>>();
    LabError::new(
        LabErrorClass::UsageValidation,
        code.as_str(),
        format!("{message}: {}", lines.join("; ")),
        &[],
    )
    .with_details(json!({"outcome": envelopes.first(), "issues": envelopes}))
}

/// The file at the `/`-separated `relative` path below `directory`.
fn pack_path(directory: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(directory.to_owned(), |parent, part| parent.join(part))
}

/// Writes `bytes` to `<path>.part`, which must not exist, and renames it over `path`.
fn write_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(part, path)
}
