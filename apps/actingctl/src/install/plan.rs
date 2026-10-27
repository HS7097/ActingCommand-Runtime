// SPDX-License-Identifier: AGPL-3.0-only

//! The plan of an update or a rollback (Workflow #421 INST-1; `contracts/install-cli.md`,
//! "Update and rollback", "Interface checks", "Preconditions"): the live and the target build
//! verified, the interface edges, the file plan, the defaults diff, both check-config summaries,
//! the ledger verification, the running state and the notice block. `plan_id` names the
//! canonical plan document; process ids, times and the rollback gate's ledger figures are not in
//! it. Read-only.

use super::adopt::verify_live;
use super::files::{self, join_relative};
use super::host::{Host, RunningImage};
use super::journal;
use super::manifest::{self, Build, BuildProblem, Half, Speaks};
use super::programs::{self, Checked};
use super::state::{self, InstallState, PackEntry, Paths, StateProblem};
use super::{Answer, refuse_ab};
use crate::codes::{InstallCaveat, InstallLocation, InstallRefusal};
use crate::watchdog::decide::OwnerLock;
use crate::watchdog::observe;
use actingcommand_contract::outcome::{CauseRelation, Outcome};
use actingcommand_runtime_config::install::InstallSettings;
use actingcommand_runtime_config::{FilePath, KeyName};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

pub(crate) const PLAN_SCHEMA: &str = "actingcommand.install-plan.v1";
/// The configuration member that names the state root.
const STATE_ROOT_MEMBER: &str = "state_root";
const DEFAULTS_FILE: &str = "actingd.defaults.json";
/// The image that may be renamed aside while it runs.
const RENAMABLE_IMAGE: &str = "actingctl.exe";

/// What is planned: an update to a kept build, or a rollback to one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PlanKind {
    Update,
    Rollback,
}

impl PlanKind {
    /// The command that plans (`apply` false) or applies a plan of this kind: the `operation`
    /// of its outcomes.
    pub(crate) fn location(self, apply: bool) -> InstallLocation {
        match (self, apply) {
            (Self::Update, false) => InstallLocation::UpdatePlan,
            (Self::Update, true) => InstallLocation::UpdateApply,
            (Self::Rollback, false) => InstallLocation::RollbackPlan,
            (Self::Rollback, true) => InstallLocation::RollbackApply,
        }
    }
}

/// The inputs of a plan.
#[derive(Debug, Clone)]
pub(crate) struct PlanInputs {
    pub(crate) kind: PlanKind,
    pub(crate) commit: String,
    pub(crate) accept_ledger_one_way: bool,
    pub(crate) preview: bool,
    /// The command the plan is computed for (`update plan`, `rollback apply`, `status`,
    /// `undo`, …): the `operation` of its refusals. Not part of the plan document.
    pub(crate) operation: InstallLocation,
}

/// What one file of the target becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Action {
    New,
    Replace,
    Same,
}

/// One file of the target build's manifests (or a manifest itself).
#[derive(Debug, Clone, Serialize)]
pub(crate) struct FileAction {
    /// Relative to the root, `/` separators (`tools/actingledger.exe`).
    pub(crate) path: String,
    pub(crate) action: Action,
    pub(crate) sha256: String,
    #[serde(skip)]
    pub(crate) live_sha256: Option<String>,
}

/// The result of one edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EdgeResult {
    Pass,
    Fail,
    Warning,
}

/// One party of an edge: who, where its range comes from, and the range.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Party {
    pub(crate) party: &'static str,
    pub(crate) source: &'static str,
    pub(crate) range: Option<[u64; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) game: Option<String>,
}

/// One interface edge of `contracts/install-cli.md`, "Interface checks".
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Edge {
    pub(crate) interface: &'static str,
    pub(crate) check: &'static str,
    pub(crate) result: EdgeResult,
    pub(crate) parties: Vec<Party>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) keys: Vec<String>,
}

/// The processes under the root that matter to the plan.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Running {
    /// `actingcommand-actingd.exe` under the root.
    pub(crate) runtime: Vec<RunningImage>,
    pub(crate) owner_lock_held: bool,
    /// Running `actingctl.exe` images that are renamed aside and keep running.
    pub(crate) renamed: Vec<RunningImage>,
    /// Running images that would change and may not be renamed (`adb.exe` among them).
    pub(crate) blocked: Vec<RunningImage>,
}

/// A computed plan.
pub(crate) struct Planned {
    pub(crate) id: String,
    pub(crate) document: Value,
    pub(crate) inputs: PlanInputs,
    pub(crate) state: InstallState,
    pub(crate) live: Build,
    pub(crate) target: Build,
    pub(crate) config: Value,
    pub(crate) state_root: PathBuf,
    pub(crate) settings: InstallSettings,
    pub(crate) files: Vec<FileAction>,
    pub(crate) extra: Vec<String>,
    pub(crate) edges: Vec<Edge>,
    pub(crate) warnings: Vec<Outcome>,
    pub(crate) running: Running,
    pub(crate) ledger_verify: bool,
    pub(crate) target_check: Checked,
}

impl Planned {
    /// The files that change, in swap order (Tools first).
    pub(crate) fn changes(&self) -> impl Iterator<Item = &FileAction> {
        self.files.iter().filter(|file| file.action != Action::Same)
    }

    /// The plan's result members, the notice block among them.
    pub(crate) fn answer(&self, outcome: Outcome) -> Result<Answer, Outcome> {
        let replaced = self
            .files
            .iter()
            .filter(|file| file.action == Action::Replace)
            .count();
        let added = self
            .files
            .iter()
            .filter(|file| file.action == Action::New)
            .count();
        let notice = json!({
            "from_commit": self.live.commit,
            "to_commit": self.target.commit,
            "replaced": replaced,
            "added": added,
            "runtime_running": !self.running.runtime.is_empty(),
            "mcp_clients_reconnect": self.running.renamed.iter().any(|image| image.mcp_serve),
            "rollback_window_bounded": self.inputs.accept_ledger_one_way && self.one_way(),
        });
        let warnings = self
            .warnings
            .iter()
            .map(crate::cli_line::warning)
            .collect::<Vec<_>>();
        Answer::new(outcome)
            .with("plan_id", &self.id)?
            .with("kind", self.inputs.kind)?
            .with("plan", &self.document)?
            .with("running", &self.running)?
            .with("extra", &self.extra)?
            .with("notice", notice)?
            .with("warnings", warnings)
    }

    /// Whether the target's ledger maximum exceeds the live build's (a one-way ledger edge).
    fn one_way(&self) -> bool {
        match (range(&self.live, LEDGER), range(&self.target, LEDGER)) {
            (Some(live), Some(target)) => target[1] > live[1],
            _ => false,
        }
    }
}

const LEDGER: &str = "ledger";
const ONE_WAY: &str = "one_way";

fn range(build: &Build, interface: &str) -> Option<[u64; 2]> {
    build.speaks()?.get(interface).copied()
}

fn contains(range: [u64; 2], value: u64) -> bool {
    range[0] <= value && value <= range[1]
}

fn meet(left: [u64; 2], right: [u64; 2]) -> bool {
    left[0].max(right[0]) <= left[1].min(right[1])
}

fn party(party: &'static str, source: &'static str, range: Option<[u64; 2]>) -> Party {
    Party {
        party,
        source,
        range,
        game: None,
    }
}

fn edge(interface: &'static str, check: &'static str, passes: bool, parties: Vec<Party>) -> Edge {
    Edge {
        interface,
        check,
        result: if passes {
            EdgeResult::Pass
        } else {
            EdgeResult::Fail
        },
        parties,
        keys: Vec::new(),
    }
}

/// The edges of an update or rollback (`contracts/install-cli.md`, "Interface checks"); the
/// caveats of unknown pack declarations (of the command `operation`) go to `warnings`.
fn interface_edges(
    installer: &Speaks,
    live: &Speaks,
    target: &Speaks,
    packs: &[PackEntry],
    accept_ledger_one_way: bool,
    operation: InstallLocation,
    warnings: &mut Vec<Outcome>,
) -> Vec<Edge> {
    let get = |speaks: &Speaks, interface: &str| speaks.get(interface).copied();
    let mut edges = Vec::new();
    let (rp, rs) = (get(live, LEDGER), get(target, LEDGER));
    let pair = |interface: &'static str| {
        (
            party("Rp", "manifest", get(live, interface)),
            party("R*", "manifest", get(target, interface)),
        )
    };
    let (live_ledger, target_ledger) = pair(LEDGER);
    edges.push(edge(
        LEDGER,
        "previous_max_in_next",
        matches!((rp, rs), (Some(rp), Some(rs)) if contains(rs, rp[1])),
        vec![live_ledger.clone(), target_ledger.clone()],
    ));
    let one_way = matches!((rp, rs), (Some(rp), Some(rs)) if rs[1] > rp[1]);
    edges.push(edge(
        LEDGER,
        ONE_WAY,
        !one_way || accept_ledger_one_way,
        vec![live_ledger, target_ledger],
    ));
    let outcomes = "runtime-outcomes";
    edges.push(edge(
        outcomes,
        "next_max_in_installer",
        matches!(
            (get(target, outcomes), get(installer, outcomes)),
            (Some(rs), Some(installer)) if contains(installer, rs[1])
        ),
        vec![
            party("R*", "manifest", get(target, outcomes)),
            party("I", "declaration", get(installer, outcomes)),
        ],
    ));
    for interface in ["install-control", "runtime-client"] {
        let own = get(installer, interface);
        for (name, other, check) in [
            ("Rp", get(live, interface), "installer_meets_previous"),
            ("R*", get(target, interface), "installer_meets_next"),
        ] {
            edges.push(edge(
                interface,
                check,
                matches!((own, other), (Some(own), Some(other)) if meet(own, other)),
                vec![
                    party("I", "declaration", own),
                    party(name, "manifest", other),
                ],
            ));
        }
    }
    let package = "package";
    let next_package = get(target, package);
    for pack in packs {
        let mut bundle = party("B", "register", pack.package_range);
        bundle.game = Some(pack.game.clone());
        let result = match (pack.package_range, next_package) {
            (None, _) => {
                warnings.push(
                    super::caveat(operation, InstallCaveat::PackDeclarationUnknown)
                        .game(pack.game.clone()),
                );
                EdgeResult::Warning
            }
            (Some(declared), Some(next)) if contains(next, declared[1]) => EdgeResult::Pass,
            (Some(_), _) => EdgeResult::Fail,
        };
        edges.push(Edge {
            interface: package,
            check: "bundle_max_in_next",
            result,
            parties: vec![bundle, party("R*", "manifest", next_package)],
            keys: Vec::new(),
        });
    }
    edges
}

/// The configuration keys that `C` names in the sections the configuration mechanism resolves
/// and that the installer does not know: neither a row of its key table nor a member of its own
/// defaults file (which holds every default of those sections, objects and lists included).
fn unknown_keys(config: &Value) -> Vec<String> {
    let mut known = actingcommand_runtime_config::mechanism_rows()
        .into_iter()
        .filter_map(|row| match (row.name, row.file_path) {
            (KeyName::Exact(name), FilePath::Key) => Some(name.to_owned()),
            (KeyName::Exact(_), FilePath::At(path)) => Some(path.to_owned()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let mut defaults = BTreeMap::new();
    if let Ok(shipped) =
        serde_json::from_str::<Value>(actingcommand_runtime_config::SHIPPED_DEFAULTS)
    {
        flatten(&shipped, "", &mut defaults);
    }
    known.extend(defaults.into_keys());
    let sections = known
        .iter()
        .filter_map(|path| path.split('.').next())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let mut unknown = Vec::new();
    if let Some(members) = config.as_object() {
        for (name, value) in members {
            if sections.contains(name) {
                walk(name, value, &known, &mut unknown);
            }
        }
    }
    unknown
}

fn walk(path: &str, value: &Value, known: &BTreeSet<String>, unknown: &mut Vec<String>) {
    if known.contains(path) || value.is_null() {
        return;
    }
    let below = format!("{path}.");
    match value.as_object() {
        Some(members) if known.iter().any(|key| key.starts_with(&below)) => {
            for (name, member) in members {
                walk(&format!("{below}{name}"), member, known, unknown);
            }
        }
        _ => unknown.push(path.to_owned()),
    }
}

/// Every leaf of a defaults document, by its dotted path (objects descended, lists kept whole).
fn flatten(value: &Value, prefix: &str, leaves: &mut BTreeMap<String, Value>) {
    match value.as_object() {
        Some(members) => {
            for (name, member) in members {
                let path = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}.{name}")
                };
                flatten(member, &path, leaves);
            }
        }
        None => {
            leaves.insert(prefix.to_owned(), value.clone());
        }
    }
}

/// The defaults diff: each key whose default differs between the two defaults files, or that
/// one of them lacks, with the configuration's source of the key (`file` when it names it).
fn defaults_diff(live: &Value, target: &Value, config: &Value) -> Vec<Value> {
    let (mut live_leaves, mut target_leaves) = (BTreeMap::new(), BTreeMap::new());
    flatten(live, "", &mut live_leaves);
    flatten(target, "", &mut target_leaves);
    let keys = live_leaves
        .keys()
        .chain(target_leaves.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    keys.into_iter()
        .filter_map(|key| {
            let (before, after) = (live_leaves.get(&key), target_leaves.get(&key));
            (before != after).then(|| {
                let named = actingcommand_runtime_config::member(config, &key)
                    .is_some_and(|value| !value.is_null());
                json!({
                    "key": key,
                    "live": before,
                    "target": after,
                    "source": if named { "file" } else { "default" },
                })
            })
        })
        .collect()
}

/// A refusal of the plan: its outcome, with the plan's members when there are some.
pub(crate) enum Refused {
    Outcome(Outcome),
    Answer(Answer),
}

impl From<Outcome> for Refused {
    fn from(outcome: Outcome) -> Self {
        Self::Outcome(outcome)
    }
}

impl Refused {
    pub(crate) fn into_answer(self) -> Answer {
        match self {
            Self::Outcome(outcome) => Answer::new(outcome),
            Self::Answer(answer) => answer,
        }
    }
}

/// The root's install state, the configuration, its `install` section and the state root.
pub(crate) struct Loaded {
    pub(crate) state: InstallState,
    pub(crate) config: Value,
    pub(crate) config_sha256: String,
    pub(crate) settings: InstallSettings,
    pub(crate) state_root: PathBuf,
}

/// Reads what every update, rollback and undo starts from; refuses (as the command `operation`)
/// a root that is not a single-location root adopted by this installer.
pub(crate) fn load(paths: &Paths, operation: InstallLocation) -> Result<Loaded, Outcome> {
    refuse_ab(operation, paths)?;
    if !paths.is_single_location() {
        return Err(super::refusal(operation, InstallRefusal::NotSingleLocation).path(&paths.root));
    }
    let state = match state::load_state(paths) {
        Ok(state) => state,
        Err(StateProblem::Missing) => {
            return Err(super::refusal(operation, InstallRefusal::NotAdopted).path(paths.state()));
        }
        Err(StateProblem::Invalid(cause)) => {
            return Err(journal::state_invalid(
                operation,
                paths.state(),
                Some(cause),
            ));
        }
    };
    let path = paths.config();
    let unreadable = |cause: Outcome| {
        super::refusal(operation, InstallRefusal::ConfigUnreadable)
            .path(&path)
            .caused_by(cause)
    };
    let bytes = files::read_all(&path).map_err(unreadable)?;
    let config: Value = serde_json::from_slice(files::without_bom(&bytes))
        .map_err(|error| unreadable(files::json_failure(&error)))?;
    let settings = actingcommand_runtime_config::install::settings(&config).map_err(|failure| {
        super::refusal(operation, InstallRefusal::InstallSectionInvalid)
            .path(&path)
            .caused_by(failure.outcome())
    })?;
    let state_root = config
        .get(STATE_ROOT_MEMBER)
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|root| root.is_absolute())
        .ok_or_else(|| super::refusal(operation, InstallRefusal::ConfigUnreadable).path(&path))?;
    Ok(Loaded {
        state,
        config,
        config_sha256: format!("sha256:{}", files::sha256_bytes(&bytes)),
        settings,
        state_root,
    })
}

/// The kept build `commit`, verified against its own manifests; the refusals are the command
/// `operation`'s: `build_not_kept`, or `build_invalid` (with `count` and `files` when listed files
/// differ).
pub(crate) fn kept_build(
    paths: &Paths,
    state: &InstallState,
    commit: &str,
    operation: InstallLocation,
) -> Result<Build, Outcome> {
    let base = paths.build(commit);
    if state.kept(commit).is_none() || !base.is_dir() {
        return Err(super::refusal(operation, InstallRefusal::BuildNotKept)
            .commit_sha(commit.to_owned())
            .path(&base));
    }
    let invalid = || {
        super::refusal(operation, InstallRefusal::BuildInvalid)
            .commit_sha(commit.to_owned())
            .path(&base)
    };
    match manifest::verify_build(&base) {
        Ok(build) if build.commit == commit => Ok(build),
        Ok(_) | Err(BuildProblem::Commits) => Err(invalid()),
        Err(BuildProblem::Manifest(_, cause)) => Err(match cause {
            Some(cause) => invalid().caused_by(cause),
            None => invalid(),
        }),
        Err(BuildProblem::Files(problems)) => Err(invalid()
            .count(problems.len())
            .files(problems.iter().map(manifest::FileProblem::record))),
    }
}

/// The defaults file of the build below `base`.
pub(crate) fn defaults_of(base: &std::path::Path) -> Value {
    files::read_json::<Value>(&base.join(Half::Runtime.dir()).join(DEFAULTS_FILE))
        .unwrap_or(Value::Null)
}

/// Computes the plan (`contracts/install-cli.md`, "Update and rollback", Plan).
pub(crate) fn plan(host: &dyn Host, paths: &Paths, inputs: PlanInputs) -> Result<Planned, Refused> {
    if !files::is_commit(&inputs.commit) {
        return Err(super::unusable(match inputs.kind {
            PlanKind::Update => "--build",
            PlanKind::Rollback => "--to",
        })
        .into());
    }
    let operation = inputs.operation;
    let Loaded {
        state,
        config,
        config_sha256,
        settings,
        state_root,
    } = load(paths, operation)?;
    let live = verify_live(operation, paths)?;
    let target = kept_build(paths, &state, &inputs.commit, operation)?;
    let live_speaks = live.speaks().ok_or_else(|| {
        super::refusal(operation, InstallRefusal::LiveBuildInvalid).path(&paths.root)
    })?;
    let target_speaks = target.speaks().ok_or_else(|| {
        super::refusal(operation, InstallRefusal::BuildInvalid)
            .commit_sha(inputs.commit.clone())
            .path(paths.build(&inputs.commit))
    })?;
    let installer = host.installer_speaks().unwrap_or_default();
    let mut warnings = Vec::new();
    let mut edges = interface_edges(
        &installer,
        &live_speaks,
        &target_speaks,
        &state.packs,
        inputs.accept_ledger_one_way,
        operation,
        &mut warnings,
    );
    for (name, build) in [("Rp", &live), ("R*", &target)] {
        edges.push(edge(
            "speaks",
            "runtime_equals_tools",
            build.speaks_agree(),
            vec![party(name, "manifest", None)],
        ));
    }
    let unknown = unknown_keys(&config);
    edges.push(Edge {
        interface: "actingd-config",
        check: "installer_knows_keys",
        result: if unknown.is_empty() {
            EdgeResult::Pass
        } else {
            EdgeResult::Fail
        },
        parties: vec![
            party("C", "file", None),
            party("I", "declaration", installer.get("actingd-config").copied()),
        ],
        keys: unknown,
    });
    // The file plan: every listed file of the target and both manifests, Tools first.
    let mut files_planned = Vec::new();
    let mut extra = Vec::new();
    for half in Half::TOOLS_FIRST {
        let target_manifest = target.manifest(half);
        let live_manifest = live.manifest(half);
        let live_listed = live_manifest
            .files
            .iter()
            .map(|file| {
                (
                    file.path.to_ascii_lowercase(),
                    file.sha256.to_ascii_lowercase(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let live_dir = paths.root.join(half.dir());
        let target_dir = paths.build(&target.commit).join(half.dir());
        let mut listed = target_manifest
            .files
            .iter()
            .map(|file| (file.path.clone(), Some(file.sha256.to_ascii_lowercase())))
            .collect::<Vec<_>>();
        listed.push((manifest::MANIFEST_FILE.to_owned(), None));
        for (path, sha256) in listed {
            let sha256 = match sha256 {
                Some(sha256) => sha256,
                None => {
                    let manifest_path = join_relative(&target_dir, &path);
                    files::sha256_file(&manifest_path)
                        .map(|(sha256, _)| sha256)
                        .map_err(|error| {
                            super::refusal(operation, InstallRefusal::BuildInvalid)
                                .commit_sha(target.commit.clone())
                                .path(&target_dir)
                                .caused_by(files::os_failure(
                                    &manifest_path,
                                    &error,
                                    actingcommand_contract::outcome::IoOp::Read,
                                ))
                        })?
                }
            };
            let live_file = join_relative(&live_dir, &path);
            let live_sha256 = match live_listed.get(&path.to_ascii_lowercase()) {
                Some(sha256) => Some(sha256.clone()),
                None => files::sha256_file(&live_file)
                    .ok()
                    .map(|(sha256, _)| sha256),
            };
            let action = match &live_sha256 {
                None => Action::New,
                Some(live_sha256) if *live_sha256 == sha256 => Action::Same,
                Some(_) => Action::Replace,
            };
            files_planned.push(FileAction {
                path: format!("{}/{path}", half.dir()),
                action,
                sha256,
                live_sha256,
            });
        }
        let target_set = target_manifest
            .file_set()
            .into_iter()
            .map(|path| path.to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        let live_files = files::list_files(&live_dir).map_err(|cause| {
            super::refusal(operation, InstallRefusal::LiveBuildInvalid)
                .path(&live_dir)
                .caused_by(cause)
        })?;
        for path in live_files {
            // A part another operation (or a script) is staging; a kit's kept `.old-<stamp>` or
            // `.rolledback-<stamp>` file is only an extra file.
            if path.ends_with(".part") {
                return Err(super::refusal(operation, InstallRefusal::ForeignPart)
                    .path(join_relative(&live_dir, &path))
                    .into());
            }
            if !target_set.contains(&path.to_ascii_lowercase()) {
                extra.push(format!("{}/{path}", half.dir()));
            }
        }
    }
    // The defaults diff.
    let live_defaults = defaults_of(&paths.root);
    let target_defaults = defaults_of(&paths.build(&target.commit));
    let diff = defaults_diff(&live_defaults, &target_defaults, &config);
    // check-config of the target (`--install-root <root>`) and of the live build in place.
    let config_path = paths.config();
    let target_check = programs::check_config(
        host,
        &paths.build(&target.commit).join(Half::Runtime.dir()),
        &config_path,
        Some(paths.root.as_path()),
    )
    .map_err(|cause| {
        super::refusal(operation, InstallRefusal::CheckConfigUnanswered)
            .commit_sha(target.commit.clone())
            .caused_by(cause)
    })?;
    let live_check = programs::check_config(
        host,
        &paths.root.join(Half::Runtime.dir()),
        &config_path,
        None,
    )
    .ok();
    edges.push(edge(
        "actingd-config",
        "next_check_config",
        target_check.passed(),
        vec![
            party("C", "file", None),
            party(
                "R*",
                "manifest",
                target_speaks.get("actingd-config").copied(),
            ),
        ],
    ));
    let ledger_verify = live_speaks.get(LEDGER) != target_speaks.get(LEDGER);
    // The running state.
    let unreadable = |cause: Outcome| {
        Refused::from(
            super::refusal(operation, InstallRefusal::ProcessesUnreadable).caused_by(cause),
        )
    };
    let images = host.processes_under(&paths.root).map_err(unreadable)?;
    // The owner lock of the state root, as the watchdog reads it; a journal that cannot be read
    // leaves the running state unknown.
    let (owner_lock, _) = observe::owner_journal(&state_root).map_err(unreadable)?;
    let mut running = Running {
        owner_lock_held: owner_lock == OwnerLock::Locked,
        ..Running::default()
    };
    let root_prefix = format!(
        "{}\\",
        files::plain(&paths.root)
            .trim_end_matches('\\')
            .to_lowercase()
    );
    for image in images {
        let lower = image.path.to_lowercase();
        let relative = lower
            .strip_prefix(&root_prefix)
            .unwrap_or(&lower)
            .replace('\\', "/");
        let name = relative.rsplit('/').next().unwrap_or_default().to_owned();
        if name == programs::DAEMON {
            running.runtime.push(image);
            continue;
        }
        let changes = files_planned.iter().any(|file| {
            file.action == Action::Replace && file.path.to_ascii_lowercase() == relative
        });
        if !changes {
            continue;
        }
        if name == RENAMABLE_IMAGE {
            running.renamed.push(image);
        } else {
            running.blocked.push(image);
        }
    }
    let document = json!({
        "schema_version": PLAN_SCHEMA,
        "kind": inputs.kind,
        "inputs": {
            "commit": inputs.commit,
            "accept_ledger_one_way": inputs.accept_ledger_one_way,
        },
        "live": {"commit": live.commit, "speaks": live_speaks},
        "target": {"commit": target.commit, "speaks": target_speaks},
        "installer": {"speaks": installer},
        "edges": edges,
        "files": files_planned,
        "extra": extra,
        "defaults_diff": diff,
        "config_sha256": config_sha256,
        "check_config": {
            "target": target_check.summary,
            "live": live_check.as_ref().map(|checked| &checked.summary),
            "differs": live_check
                .as_ref()
                .map(|checked| target_check.summary.differs(&checked.summary)),
        },
        "ledger_verify": ledger_verify,
    });
    let canonical = actingcommand_runtime_config::canonical_json(&document);
    let id = format!("plan-{}", &files::sha256_bytes(canonical.as_bytes())[..16]);
    let planned = Planned {
        id,
        document,
        inputs,
        state,
        live,
        target,
        config,
        state_root,
        settings,
        files: files_planned,
        extra,
        edges,
        warnings,
        running,
        ledger_verify,
        target_check,
    };
    refuse(planned)
}

/// The plan's refusals in their order: a failed edge (the target's check-config refusing the
/// configuration on an update), a running Runtime, a held owner lock, a running image that may
/// not change. `--preview` reports the running state instead of refusing it. A rollback's
/// check-config refusal is the gate's (its part `config`).
fn refuse(planned: Planned) -> Result<Planned, Refused> {
    let rollback = planned.inputs.kind == PlanKind::Rollback;
    let operation = planned.inputs.operation;
    if !planned.target_check.passed() && !rollback {
        let outcome = super::refusal(operation, InstallRefusal::ConfigRefusedByTarget)
            .commit_sha(planned.target.commit.clone())
            .joined(
                CauseRelation::CausedBy,
                planned.target_check.received.clone(),
            );
        return Err(Refused::Answer(planned.answer(outcome)?));
    }
    let failed = planned
        .edges
        .iter()
        .filter(|edge| {
            edge.result == EdgeResult::Fail && !(rollback && edge.check == "next_check_config")
        })
        .count();
    if failed > 0 {
        let outcome = super::refusal(operation, InstallRefusal::InterfaceEdge)
            .commit_sha(planned.target.commit.clone())
            .count(failed);
        return Err(Refused::Answer(planned.answer(outcome)?));
    }
    if !planned.inputs.preview {
        if let Some(image) = planned.running.runtime.first() {
            let outcome = super::refusal(operation, InstallRefusal::RuntimeRunning)
                .path(&image.path)
                .pid(image.pid);
            return Err(Refused::Answer(planned.answer(outcome)?));
        }
        if planned.running.owner_lock_held {
            let outcome =
                super::refusal(operation, InstallRefusal::OwnerLockHeld).path(&planned.state_root);
            return Err(Refused::Answer(planned.answer(outcome)?));
        }
        if let Some(image) = planned.running.blocked.first() {
            let outcome = super::refusal(operation, InstallRefusal::RunningImageBlocks)
                .path(&image.path)
                .pid(image.pid);
            return Err(Refused::Answer(planned.answer(outcome)?));
        }
    }
    Ok(planned)
}

/// The plan's success outcome: `install_command_done` of the planning command with its
/// `plan_id` and the target's `commit_sha`, each caveat a `note` link.
pub(crate) fn planned_outcome(planned: &Planned) -> Outcome {
    with_notes(
        super::done(planned.inputs.operation)
            .plan_id(planned.id.clone())
            .commit_sha(planned.target.commit.clone()),
        &planned.warnings,
    )
}

/// `outcome` with each of `notes` as a `note` link.
pub(crate) fn with_notes(outcome: Outcome, notes: &[Outcome]) -> Outcome {
    notes.iter().fold(outcome, |outcome, note| {
        outcome.link(CauseRelation::Note, note.clone())
    })
}
