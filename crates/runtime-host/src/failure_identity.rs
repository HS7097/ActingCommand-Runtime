// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #336 L6 (§12.4, §12.7, `contracts/policy-suspension.md`): the failure identity a
//! scheduled run's settlement writes (a `linear_steps` run's, and since Workflow #390 EXEC-3 a
//! page-graph run's) as the `policy.execution_recorded`
//! `failure.failure_identity` (beside the failure's `outcome`, Workflow #378 A2-P), and the
//! suspension lift rule read back from it. Pure: it reads only the rows and values its callers
//! pass, never a ledger handle, an artifact, a clock or a lock.
//!
//! Format v2 (Workflow #336 PKG-S3, M-PKG §3.4):
//! `<base>~v2~k<K>~m<I><H>[~p<I><H>]{0,3}[~r<G><I><H>]{0,1}~f<F>`: the terminal failure code
//! (or `h` and its hash), the failure key over code, scope, step and detail, the main package,
//! each declared prerequisite layer and the return-home layer, each as the id hash of its
//! `package_id` and the content hash measured when the failure was settled (`x` when it could
//! not be measured), and the error frame (`na` without one, `u…` for a failure that never
//! accumulates). The identity prefix is everything before `~f`. A v1 string is not read.

use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::CodeStr;
use actingcommand_contract::{
    ApplicationPayload, ArtifactKind, EventAction, EventPayload, EventQuery, EventType, RunId,
    TaskEntryTargetDisposition, TaskPayload, TaskSemanticFact,
};
use actingcommand_execution_kernel::linear_main_interface;
use actingcommand_ledger::{LedgerArtifactReference, PersistedEvent};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const VERSION: &str = "v2";
const KEY_DOMAIN: &str = "actingcommand.failure-key.v1";
const MAX_BASE_BYTES: usize = 64;
const BASE_HASH_HEX: usize = 16;
const KEY_HEX: usize = 12;
/// The content hash digits a package segment keeps.
const HASH_HEX: usize = 12;
const ID_HEX: usize = 8;
const FRAME_HEX: usize = 12;
const MAX_DECLARED_LAYERS: usize = 3;
/// The content hash of a package that was not measured: the mapping names no package for its
/// id, or reading the package failed (M-PKG D18).
pub(crate) const MISSING_HASH: &str = "x";

/// The `key=value` fragments of a native detail that enter the failure key (§12.4; R24 adds
/// `application`).
const DETAIL_KEYS: &[&str] = &[
    "operation",
    "after_page",
    "hit_error_page",
    "transition",
    "intermediate_seen",
    "layer",
    "package_id",
    "required_page",
    "return_home",
    "reason",
    "application",
];
/// Failures of the main package's own steps whose screen does not match the package (R21).
/// Workflow #390 EXEC-3 (wave 14 ruling 4): a page-graph step unconfirmed after all its retries
/// is the graph counterpart of `contained_task_linear_intermediate_unobserved`.
const MAIN_ACCUMULATING: &[&str] = &[
    actingcommand_execution_kernel::ExecutionKernelCode::PageConfirmationFailed.as_str(),
    actingcommand_execution_kernel::ExecutionKernelCode::ContainedTaskLinearIntermediateUnobserved
        .as_str(),
    actingcommand_execution_kernel::ExecutionKernelCode::ContainedTaskStepUnconfirmed.as_str(),
    actingcommand_execution_kernel::ExecutionKernelCode::ContainedTaskGuardRefused.as_str(),
];
/// Preparation refusals of the prerequisite chain: reproducible configuration problems (R21).
const PREPARE_ACCUMULATING_PREFIX: &str = "contained_task_prerequisite_";
/// R25-2: the adb backend failures of a `linear_steps` task that are rerun only when the run
/// was not poisoned.
pub(crate) const RERUN_ONLY_BACKEND_FAILURES: &[&str] = &[
    SharedCode::ApplicationBackendOperationFailed.as_str(),
    SharedCode::InputBackendOperationFailed.as_str(),
];

/// Lowercase hex SHA-256 of `bytes`.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn hex_prefix(value: &str, length: usize) -> String {
    value.chars().take(length).collect()
}

fn is_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The first twelve hex digits of a measured content hash.
pub(crate) fn content_hash_prefix(content_hash: &str) -> String {
    hex_prefix(content_hash, HASH_HEX)
}

/// Whether `hash` is a package segment's hash: twelve hex digits, or `x`.
fn is_segment_hash(hash: &str) -> bool {
    hash == MISSING_HASH || is_hex(hash, HASH_HEX)
}

/// The first eight hex digits of SHA-256 over a package id.
pub(crate) fn package_id_hash(package_id: &str) -> String {
    hex_prefix(&sha256_hex(package_id.as_bytes()), ID_HEX)
}

/// The first eight hex digits of SHA-256 over `game` NUL `server`.
pub(crate) fn return_home_key_hash(game: &str, server: &str) -> String {
    hex_prefix(&sha256_hex(format!("{game}\0{server}").as_bytes()), ID_HEX)
}

/// The base segment of a failure code: the code itself, or `h` and sixteen hex digits of its
/// SHA-256 when it is longer than 64 bytes or contains `~`.
pub(crate) fn encode_failure_base(code: &CodeStr) -> String {
    let code = code.as_str();
    if code.len() > MAX_BASE_BYTES || code.contains('~') {
        format!(
            "h{}",
            hex_prefix(&sha256_hex(code.as_bytes()), BASE_HASH_HEX)
        )
    } else {
        code.to_owned()
    }
}

/// detail′: the whitelisted `key=value` fragments of a native detail in their order, the text
/// itself when it holds no `=`, and empty without a detail.
pub(crate) fn failure_detail_fingerprint(detail: Option<&str>) -> String {
    let Some(detail) = detail else {
        return String::new();
    };
    if !detail.contains('=') {
        return detail.to_owned();
    }
    detail
        .split_whitespace()
        .filter(|fragment| {
            fragment
                .split_once('=')
                .is_some_and(|(key, _)| DETAIL_KEYS.contains(&key))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// K: the first twelve hex digits of SHA-256 over the domain line, the original failure code,
/// the scope, the step's operation label and detail′, one per line.
pub(crate) fn failure_key(
    code: &CodeStr,
    scope: &str,
    operation_label: &str,
    detail: &str,
) -> String {
    hex_prefix(
        &sha256_hex(
            format!("{KEY_DOMAIN}\n{code}\n{scope}\n{operation_label}\n{detail}").as_bytes(),
        ),
        KEY_HEX,
    )
}

/// Whether a failure accumulates toward suspension (R21, R25-1): a main-scope failure of the
/// main package's own steps outside the restart segment, or a prerequisite preparation
/// refusal. Every other failure is rerun only.
pub(crate) fn failure_accumulates(code: &CodeStr, scope: &str, restart_segment: bool) -> bool {
    let code = code.as_str();
    match scope {
        "main" => !restart_segment && MAIN_ACCUMULATING.contains(&code),
        "prepare" => code.starts_with(PREPARE_ACCUMULATING_PREFIX),
        _ => false,
    }
}

/// A package of the identity: the id hash of its `package_id` and its content hash prefix (or
/// `x`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IdentityPackage {
    pub(crate) id_hash: String,
    pub(crate) hash: String,
}

impl IdentityPackage {
    /// The package `package_id` with the content hash measured for it (`None`: not measured).
    pub(crate) fn measured(package_id: &str, content_hash: Option<&str>) -> Self {
        Self {
            id_hash: package_id_hash(package_id),
            hash: content_hash.map_or_else(|| MISSING_HASH.to_owned(), content_hash_prefix),
        }
    }

    fn parse(segment: &str) -> Option<Self> {
        let (id_hash, hash) = (segment.get(..ID_HEX)?, segment.get(ID_HEX..)?);
        (is_hex(id_hash, ID_HEX) && is_segment_hash(hash)).then(|| Self {
            id_hash: id_hash.to_owned(),
            hash: hash.to_owned(),
        })
    }

    fn encode(&self) -> String {
        format!("{}{}", self.id_hash, self.hash)
    }
}

/// One prerequisite layer of the identity, as resolved for the failed run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IdentityLayer {
    /// `p<I><H>`: a declared prerequisite package.
    Declared(IdentityPackage),
    /// `r<G><I><H>`: the return-home package of a game and server.
    ReturnHome {
        key_hash: String,
        package: IdentityPackage,
    },
}

impl IdentityLayer {
    /// The layer of `package_id`, with the content hash measured for it at settlement.
    pub(crate) fn measured(
        package_id: &str,
        return_home: Option<(&str, &str)>,
        content_hash: Option<&str>,
    ) -> Self {
        let package = IdentityPackage::measured(package_id, content_hash);
        match return_home {
            Some((game, server)) => Self::ReturnHome {
                key_hash: return_home_key_hash(game, server),
                package,
            },
            None => Self::Declared(package),
        }
    }

    pub(crate) const fn kind(&self) -> &'static str {
        match self {
            Self::Declared(_) => "declared",
            Self::ReturnHome { .. } => "return_home",
        }
    }

    pub(crate) const fn package(&self) -> &IdentityPackage {
        match self {
            Self::Declared(package) | Self::ReturnHome { package, .. } => package,
        }
    }

    pub(crate) fn id_hash(&self) -> &str {
        &self.package().id_hash
    }

    /// The content hash prefix recorded for the layer, or `x`.
    pub(crate) fn hash(&self) -> &str {
        &self.package().hash
    }

    fn encode(&self) -> String {
        match self {
            Self::Declared(package) => format!("p{}", package.encode()),
            Self::ReturnHome { key_hash, package } => format!("r{key_hash}{}", package.encode()),
        }
    }

    fn parse(segment: &str) -> Option<Self> {
        if let Some(rest) = segment.strip_prefix('p') {
            return IdentityPackage::parse(rest).map(Self::Declared);
        }
        let rest = segment.strip_prefix('r')?;
        let key_hash = rest.get(..ID_HEX)?;
        let package = IdentityPackage::parse(rest.get(ID_HEX..)?)?;
        is_hex(key_hash, ID_HEX).then(|| Self::ReturnHome {
            key_hash: key_hash.to_owned(),
            package,
        })
    }
}

/// F: the error frame's artifact digest, `na` without a frame, or `u…` for a failure that does
/// not accumulate (unique per dispatch, so it never repeats the previous identity).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IdentityFrame {
    Frame(String),
    None,
    Unique(String),
}

impl IdentityFrame {
    /// The frame mark of an artifact SHA-256 (with or without its `sha256:` prefix).
    pub(crate) fn of_artifact(sha256: Option<&str>) -> Self {
        sha256.map_or(Self::None, |sha256| {
            Self::Frame(hex_prefix(
                sha256.strip_prefix("sha256:").unwrap_or(sha256),
                FRAME_HEX,
            ))
        })
    }

    /// The unique mark of a dispatch whose failure does not accumulate.
    pub(crate) fn unique(decision_id: &str) -> Self {
        Self::Unique(hex_prefix(&sha256_hex(decision_id.as_bytes()), FRAME_HEX))
    }

    fn encode(&self) -> String {
        match self {
            Self::Frame(digest) => format!("f{digest}"),
            Self::None => "fna".to_owned(),
            Self::Unique(digest) => format!("fu{digest}"),
        }
    }

    fn parse(segment: &str) -> Option<Self> {
        let rest = segment.strip_prefix('f')?;
        if rest == "na" {
            return Some(Self::None);
        }
        if let Some(unique) = rest.strip_prefix('u') {
            return is_hex(unique, FRAME_HEX).then(|| Self::Unique(unique.to_owned()));
        }
        is_hex(rest, FRAME_HEX).then(|| Self::Frame(rest.to_owned()))
    }
}

/// A parsed or built failure identity string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FailureIdentity {
    pub(crate) base: String,
    pub(crate) key: String,
    pub(crate) main: IdentityPackage,
    pub(crate) layers: Vec<IdentityLayer>,
    pub(crate) frame: IdentityFrame,
}

impl FailureIdentity {
    /// The identity prefix P: everything before `~f`.
    pub(crate) fn prefix(&self) -> String {
        let mut prefix = format!(
            "{}~{VERSION}~k{}~m{}",
            self.base,
            self.key,
            self.main.encode()
        );
        for layer in &self.layers {
            prefix.push('~');
            prefix.push_str(&layer.encode());
        }
        prefix
    }

    pub(crate) fn encode(&self) -> String {
        format!("{}~{}", self.prefix(), self.frame.encode())
    }

    /// Whether this failure accumulates toward suspension: its frame mark is not unique.
    pub(crate) const fn accumulates(&self) -> bool {
        !matches!(self.frame, IdentityFrame::Unique(_))
    }

    /// The identity a recorded failure identity string encodes, or `None` for any other text.
    pub(crate) fn parse(identity: &str) -> Option<Self> {
        let parts = identity.split('~').collect::<Vec<_>>();
        let [base, version, key, main, middle @ .., frame] = parts.as_slice() else {
            return None;
        };
        let key = key.strip_prefix('k').filter(|key| is_hex(key, KEY_HEX))?;
        let main = IdentityPackage::parse(main.strip_prefix('m')?)?;
        if base.is_empty() || *version != VERSION {
            return None;
        }
        let layers = middle
            .iter()
            .copied()
            .map(IdentityLayer::parse)
            .collect::<Option<Vec<_>>>()?;
        let declared = layers
            .iter()
            .take_while(|layer| matches!(layer, IdentityLayer::Declared(_)))
            .count();
        if declared > MAX_DECLARED_LAYERS || layers.len() > declared + 1 {
            return None;
        }
        Some(Self {
            base: (*base).to_owned(),
            key: key.to_owned(),
            main,
            layers,
            frame: IdentityFrame::parse(frame)?,
        })
    }
}

/// The event types of one run that the identity reads (§12.4, R25-1): its entry facts, step
/// starts and ends, and application intents.
pub(crate) const FAILURE_ROW_TYPES: [EventType; 4] = [
    EventType::TaskEntryPreflight,
    EventType::TaskStepStarted,
    EventType::TaskStepFinished,
    EventType::ApplicationIntent,
];

/// The rows of `run_id` of the [`FAILURE_ROW_TYPES`], in ledger order.
pub(crate) fn failure_rows<E>(
    run_id: RunId,
    mut query: impl FnMut(EventQuery) -> Result<Vec<PersistedEvent>, E>,
) -> Result<Vec<PersistedEvent>, E> {
    let mut rows = Vec::new();
    for event_type in FAILURE_ROW_TYPES {
        rows.extend(query(EventQuery {
            event_type: Some(event_type),
            run_id: Some(run_id),
            ..EventQuery::default()
        })?);
    }
    rows.sort_by_key(PersistedEvent::sequence);
    Ok(rows)
}

fn semantic_fact(event: &PersistedEvent) -> Option<&TaskSemanticFact> {
    match event.payload() {
        EventPayload::Task(TaskPayload::Semantic(payload)) => Some(payload.fact()),
        _ => None,
    }
}

/// The scope and step of a failure (§12.4) from its run's rows: `main` once the gate started the
/// package (or without a prerequisite chain), else `pre:<I>` of the innermost prerequisite
/// package opened and not completed, `I` the id hash of the `package_id` its rows record
/// (Workflow #336 PKG-S2, M-PKG §3.4), else `gate`. The step is the scope's last `StepStarted`
/// operation label, `entry` without one. The `prepare` scope is the caller's: it has no rows.
pub(crate) fn failure_step(rows: &[PersistedEvent], chain: bool) -> (String, String) {
    let mut open: Vec<String> = Vec::new();
    let mut started = !chain;
    let mut operations = BTreeMap::<String, String>::new();
    let scope_of = |started: bool, open: &[String]| match open.last() {
        _ if started => "main".to_owned(),
        Some(id_hash) => format!("pre:{id_hash}"),
        None => "gate".to_owned(),
    };
    for fact in rows.iter().filter_map(semantic_fact) {
        match fact {
            TaskSemanticFact::EntryRecoveryPackageAdmitted { package_label, .. } if !started => {
                open.push(package_id_hash(package_label));
            }
            TaskSemanticFact::EntryRecoveryCompleted { package_label, .. } if !started => {
                let id_hash = package_id_hash(package_label);
                if let Some(index) = open.iter().rposition(|open| *open == id_hash) {
                    open.remove(index);
                }
            }
            TaskSemanticFact::EntryTargetDisposition {
                disposition: TaskEntryTargetDisposition::Started,
                ..
            } => started = true,
            TaskSemanticFact::StepStarted {
                operation_label, ..
            } => {
                operations.insert(scope_of(started, &open), operation_label.clone());
            }
            _ => {}
        }
    }
    let scope = scope_of(started, &open);
    let operation = operations
        .remove(&scope)
        .unwrap_or_else(|| "entry".to_owned());
    (scope, operation)
}

/// Workflow #390 EXEC-3: whether a page-graph run checked its required entry (its
/// `EntryRecoveryDecision`). That check is the run's gate and its entry recovery package its
/// layer for [`failure_step`], as a `linear_steps` run's prerequisite chain is.
pub(crate) fn entry_checked(rows: &[PersistedEvent]) -> bool {
    rows.iter().any(|event| {
        matches!(
            semantic_fact(event),
            Some(TaskSemanticFact::EntryRecoveryDecision { .. })
        )
    })
}

/// R25-1 (ruling 5961093808): whether a run failed inside its restart segment: it had a launch
/// or restart application effect and no `StepFinished` page after the last one is the main
/// interface of `game`: for a `linear_steps` run its main-interface predicate, for a page-graph
/// run (Workflow #390 EXEC-3) the literal `home` the page-graph home checks keep.
pub(crate) fn in_restart_segment(rows: &[PersistedEvent], game: &str, linear: bool) -> bool {
    let main_interface = |page: &str| {
        if linear {
            linear_main_interface(game, page)
        } else {
            actingcommand_execution_kernel::canonical_page_anchor(game, page) == "home"
        }
    };
    let restarted = rows
        .iter()
        .filter(|event| {
            matches!(
                event.payload(),
                EventPayload::Application(ApplicationPayload::Intent(intent))
                    if matches!(
                        intent.action(),
                        EventAction::ApplicationLaunch | EventAction::ApplicationRestart
                    )
            )
        })
        .map(PersistedEvent::sequence)
        .max();
    let Some(restarted) = restarted else {
        return false;
    };
    !rows.iter().any(|event| {
        event.sequence() > restarted
            && matches!(
                semantic_fact(event),
                Some(TaskSemanticFact::StepFinished { page_label, .. })
                    if main_interface(page_label)
            )
    })
}

/// §12.5: the error frame of a run: the capture frame artifact verified for the frame of its
/// latest `CaptureCompleted`; `None` when the run has none.
pub(crate) fn locate_error_frame<E>(
    run_id: RunId,
    mut query: impl FnMut(EventQuery) -> Result<Vec<PersistedEvent>, E>,
) -> Result<Option<LedgerArtifactReference>, E> {
    let captures = query(EventQuery {
        event_type: Some(EventType::CaptureCompleted),
        run_id: Some(run_id),
        ..EventQuery::default()
    })?;
    let Some(frame_id) = captures
        .iter()
        .max_by_key(|event| event.sequence())
        .and_then(|event| event.links().frame_id().copied())
    else {
        return Ok(None);
    };
    let verified = query(EventQuery {
        event_type: Some(EventType::ArtifactVerified),
        run_id: Some(run_id),
        frame_id: Some(frame_id),
        ..EventQuery::default()
    })?;
    Ok(verified
        .iter()
        .flat_map(|event| event.artifacts().iter())
        .find(|artifact| artifact.kind() == ArtifactKind::CaptureFrame)
        .cloned())
}

/// The canonical text of a typed identifier, as its records serialize it.
pub(crate) fn identifier_text(identifier: &impl serde::Serialize) -> String {
    serde_json::to_value(identifier)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}
