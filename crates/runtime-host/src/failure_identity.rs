// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #336 L6 (§12.4, §12.7, `contracts/policy-suspension.md`): the failure identity a
//! scheduled `linear_steps` run's settlement writes as the existing
//! `policy.execution_recorded` `failure.error_code`, and the suspension lift rule read back
//! from it. Pure: it reads only the rows and values its callers pass, never a ledger handle,
//! an artifact, a clock or a lock.
//!
//! `<base>~v1~k<K>~m<M>[~p<I><D>]{0,3}[~r<G><I><D>]{0,1}~f<F>`: the terminal failure code (or
//! `h` and its hash), the failure key over code, scope, step and detail, the main package
//! digest, each declared prerequisite layer and the return-home layer with their mapped
//! digests, and the error frame (`na` without one, `u…` for a failure that never
//! accumulates). The identity prefix is everything before `~f`.

use crate::ProcedureManifest;
use actingcommand_contract::{
    ApplicationPayload, ArtifactKind, ContainedTaskRecoveryBinding, EventAction, EventPayload,
    EventQuery, EventType, PackageRef, RunId, TaskEntryTargetDisposition, TaskPayload,
    TaskSemanticFact,
};
use actingcommand_execution_kernel::linear_main_interface;
use actingcommand_ledger::{LedgerArtifactReference, PersistedEvent};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const VERSION: &str = "v1";
const KEY_DOMAIN: &str = "actingcommand.failure-key.v1";
const MAX_BASE_BYTES: usize = 64;
const BASE_HASH_HEX: usize = 16;
const KEY_HEX: usize = 12;
const DIGEST_HEX: usize = 12;
const ID_HEX: usize = 8;
const FRAME_HEX: usize = 12;
const MAX_DECLARED_LAYERS: usize = 3;
/// A mapped digest that is not one value (several mapping keys share the id hash): never equal
/// to a recorded digest, so the layer counts as changed.
const AMBIGUOUS_DIGEST: &str = "?";
/// The mapped digest of a package id the mapping does not name.
pub(crate) const MISSING_DIGEST: &str = "x";

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
const MAIN_ACCUMULATING: &[&str] = &[
    "page_confirmation_failed",
    "contained_task_linear_intermediate_unobserved",
    "contained_task_guard_refused",
];
/// Preparation refusals of the prerequisite chain: reproducible configuration problems (R21).
const PREPARE_ACCUMULATING_PREFIX: &str = "contained_task_prerequisite_";
/// R25-2: the adb backend failures of a `linear_steps` task that are rerun only when the run
/// was not poisoned.
pub(crate) const RERUN_ONLY_BACKEND_FAILURES: &[&str] = &[
    "application_backend_operation_failed",
    "input_backend_operation_failed",
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

/// The hexadecimal content digest of a package reference: the ZIP or content-directory
/// SHA-256, or the SHA-256 of a Git source tree reference's wire value.
pub(crate) fn package_ref_hex(reference: &PackageRef) -> String {
    match reference {
        PackageRef::LegacyZipSha256(value) => value.clone(),
        PackageRef::ContentDirectory(directory) => directory.sha256.clone(),
        PackageRef::GitSourceTree(_) => {
            sha256_hex(reference.prefixed_wire_value().to_string().as_bytes())
        }
    }
}

/// The first twelve hex digits of a package reference's digest.
pub(crate) fn package_digest_prefix(reference: &PackageRef) -> String {
    hex_prefix(&package_ref_hex(reference), DIGEST_HEX)
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
pub(crate) fn encode_failure_base(code: &str) -> String {
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
pub(crate) fn failure_key(code: &str, scope: &str, operation: &str, detail: &str) -> String {
    hex_prefix(
        &sha256_hex(format!("{KEY_DOMAIN}\n{code}\n{scope}\n{operation}\n{detail}").as_bytes()),
        KEY_HEX,
    )
}

/// Whether a failure accumulates toward suspension (R21, R25-1): a main-scope failure of the
/// main package's own steps outside the restart segment, or a prerequisite preparation
/// refusal. Every other failure is rerun only.
pub(crate) fn failure_accumulates(code: &str, scope: &str, restart_segment: bool) -> bool {
    match scope {
        "main" => !restart_segment && MAIN_ACCUMULATING.contains(&code),
        "prepare" => code.starts_with(PREPARE_ACCUMULATING_PREFIX),
        _ => false,
    }
}

/// One prerequisite layer of the identity, as resolved for the failed run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IdentityLayer {
    /// `p<I><D>`: a declared prerequisite package.
    Declared { id_hash: String, digest: String },
    /// `r<G><I><D>`: the return-home package of a game and server.
    ReturnHome {
        key_hash: String,
        id_hash: String,
        digest: String,
    },
}

impl IdentityLayer {
    /// The layer of `package_id`, with its digest as `prerequisite_packages` maps it now.
    pub(crate) fn resolved(
        package_id: &str,
        return_home: Option<(&str, &str)>,
        prerequisite_packages: &BTreeMap<String, ContainedTaskRecoveryBinding>,
    ) -> Self {
        let id_hash = package_id_hash(package_id);
        let digest = prerequisite_packages.get(package_id).map_or_else(
            || MISSING_DIGEST.to_owned(),
            |binding| package_digest_prefix(binding.expected_sha256()),
        );
        match return_home {
            Some((game, server)) => Self::ReturnHome {
                key_hash: return_home_key_hash(game, server),
                id_hash,
                digest,
            },
            None => Self::Declared { id_hash, digest },
        }
    }

    pub(crate) const fn kind(&self) -> &'static str {
        match self {
            Self::Declared { .. } => "declared",
            Self::ReturnHome { .. } => "return_home",
        }
    }

    pub(crate) fn id_hash(&self) -> &str {
        match self {
            Self::Declared { id_hash, .. } | Self::ReturnHome { id_hash, .. } => id_hash,
        }
    }

    pub(crate) fn digest(&self) -> &str {
        match self {
            Self::Declared { digest, .. } | Self::ReturnHome { digest, .. } => digest,
        }
    }

    fn encode(&self) -> String {
        match self {
            Self::Declared { id_hash, digest } => format!("p{id_hash}{digest}"),
            Self::ReturnHome {
                key_hash,
                id_hash,
                digest,
            } => format!("r{key_hash}{id_hash}{digest}"),
        }
    }

    fn parse(segment: &str) -> Option<Self> {
        let digest_ok = |digest: &str| digest == MISSING_DIGEST || is_hex(digest, DIGEST_HEX);
        if let Some(rest) = segment.strip_prefix('p') {
            let (id_hash, digest) = (rest.get(..ID_HEX)?, rest.get(ID_HEX..)?);
            return (is_hex(id_hash, ID_HEX) && digest_ok(digest)).then(|| Self::Declared {
                id_hash: id_hash.to_owned(),
                digest: digest.to_owned(),
            });
        }
        let rest = segment.strip_prefix('r')?;
        let (key_hash, id_hash, digest) = (
            rest.get(..ID_HEX)?,
            rest.get(ID_HEX..2 * ID_HEX)?,
            rest.get(2 * ID_HEX..)?,
        );
        (is_hex(key_hash, ID_HEX) && is_hex(id_hash, ID_HEX) && digest_ok(digest)).then(|| {
            Self::ReturnHome {
                key_hash: key_hash.to_owned(),
                id_hash: id_hash.to_owned(),
                digest: digest.to_owned(),
            }
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
    pub(crate) main_digest: String,
    pub(crate) layers: Vec<IdentityLayer>,
    pub(crate) frame: IdentityFrame,
}

impl FailureIdentity {
    /// The identity prefix P: everything before `~f`.
    pub(crate) fn prefix(&self) -> String {
        let mut prefix = format!(
            "{}~{VERSION}~k{}~m{}",
            self.base, self.key, self.main_digest
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

    /// The identity a code encodes, or `None` for any other code (a page-graph task's code,
    /// a reconciled code, a v0.9.0 record).
    pub(crate) fn parse(code: &str) -> Option<Self> {
        let parts = code.split('~').collect::<Vec<_>>();
        let [base, version, key, main_digest, middle @ .., frame] = parts.as_slice() else {
            return None;
        };
        let key = key.strip_prefix('k').filter(|key| is_hex(key, KEY_HEX))?;
        let main_digest = main_digest
            .strip_prefix('m')
            .filter(|digest| is_hex(digest, DIGEST_HEX))?;
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
            .take_while(|layer| matches!(layer, IdentityLayer::Declared { .. }))
            .count();
        if declared > MAX_DECLARED_LAYERS || layers.len() > declared + 1 {
            return None;
        }
        Some(Self {
            base: (*base).to_owned(),
            key: key.to_owned(),
            main_digest: main_digest.to_owned(),
            layers,
            frame: IdentityFrame::parse(frame)?,
        })
    }
}

/// The configuration a suspension is judged against (§12.7): the procedure bindings and the
/// prerequisite and return-home mappings, as read at startup (or by `actingd suspended`).
pub(crate) struct SuspensionLiftView<'a> {
    pub(crate) manifest: &'a ProcedureManifest,
    pub(crate) prerequisite_packages: &'a BTreeMap<String, ContainedTaskRecoveryBinding>,
    pub(crate) return_home_packages: &'a BTreeMap<(String, String), String>,
}

/// What lifted a suspension: the main package's digest, or the layer at a one-based index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SuspensionLift {
    MainDigest,
    Layer(usize),
}

impl SuspensionLift {
    pub(crate) fn label(self) -> String {
        match self {
            Self::MainDigest => "main_digest".to_owned(),
            Self::Layer(index) => format!("layer:{index}"),
        }
    }
}

impl SuspensionLiftView<'_> {
    /// The mapped digest of the declared layer whose id hash is `id_hash`: `x` when no package
    /// id has it, and a value equal to no digest when several have it.
    fn declared_digest(&self, id_hash: &str) -> String {
        let mut matching = self
            .prerequisite_packages
            .iter()
            .filter(|(package_id, _)| package_id_hash(package_id) == id_hash);
        match (matching.next(), matching.next()) {
            (None, _) => MISSING_DIGEST.to_owned(),
            (Some((_, binding)), None) => package_digest_prefix(binding.expected_sha256()),
            (Some(_), Some(_)) => AMBIGUOUS_DIGEST.to_owned(),
        }
    }

    /// The return-home package id the mapping has for the key hash, if exactly one.
    fn return_home_id(&self, key_hash: &str) -> Option<&str> {
        let mut matching = self
            .return_home_packages
            .iter()
            .filter(|((game, server), _)| return_home_key_hash(game, server) == key_hash);
        match (matching.next(), matching.next()) {
            (Some((_, package_id)), None) => Some(package_id.as_str()),
            _ => None,
        }
    }

    /// A layer's mapped digest prefix now: `None` when its return-home key or id changed.
    pub(crate) fn current_layer_digest(&self, layer: &IdentityLayer) -> Option<String> {
        match layer {
            IdentityLayer::Declared { id_hash, .. } => Some(self.declared_digest(id_hash)),
            IdentityLayer::ReturnHome {
                key_hash, id_hash, ..
            } => {
                let package_id = self.return_home_id(key_hash)?;
                (package_id_hash(package_id) == *id_hash).then(|| {
                    self.prerequisite_packages.get(package_id).map_or_else(
                        || MISSING_DIGEST.to_owned(),
                        |binding| package_digest_prefix(binding.expected_sha256()),
                    )
                })
            }
        }
    }

    /// The package id a layer names in the current mapping, if exactly one key has its hash.
    pub(crate) fn layer_package_id(&self, layer: &IdentityLayer) -> Option<String> {
        if let IdentityLayer::ReturnHome { key_hash, .. } = layer
            && let Some(package_id) = self.return_home_id(key_hash)
            && package_id_hash(package_id) == layer.id_hash()
        {
            return Some(package_id.to_owned());
        }
        let mut matching = self
            .prerequisite_packages
            .keys()
            .filter(|package_id| package_id_hash(package_id) == layer.id_hash());
        match (matching.next(), matching.next()) {
            (Some(package_id), None) => Some(package_id.clone()),
            _ => None,
        }
    }

    /// The current main package digest of `procedure_ref`, if bound.
    pub(crate) fn current_main(&self, procedure_ref: &str) -> Option<&PackageRef> {
        self.manifest
            .binding(procedure_ref)
            .map(|binding| binding.package_digest())
    }

    /// §12.7: whether a suspension recorded for `paused_digest` and `layers` is lifted. An
    /// unbound procedure never lifts (its whole cycle fails anyway).
    pub(crate) fn lifted(
        &self,
        procedure_ref: &str,
        paused_digest: Option<&PackageRef>,
        layers: &[IdentityLayer],
    ) -> Option<SuspensionLift> {
        let current = self.current_main(procedure_ref)?;
        if paused_digest != Some(current) {
            return Some(SuspensionLift::MainDigest);
        }
        layers.iter().enumerate().find_map(|(index, layer)| {
            (self.current_layer_digest(layer).as_deref() != Some(layer.digest()))
                .then_some(SuspensionLift::Layer(index + 1))
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
/// package (or without a prerequisite chain), else `pre:<digest>` of the innermost prerequisite
/// package opened and not completed, else `gate`. The step is the scope's last `StepStarted`
/// operation label, `entry` without one. The `prepare` scope is the caller's: it has no rows.
pub(crate) fn failure_step(rows: &[PersistedEvent], chain: bool) -> (String, String) {
    let mut open: Vec<String> = Vec::new();
    let mut started = !chain;
    let mut operations = BTreeMap::<String, String>::new();
    let scope_of = |started: bool, open: &[String]| match open.last() {
        _ if started => "main".to_owned(),
        Some(digest) => format!("pre:{digest}"),
        None => "gate".to_owned(),
    };
    for fact in rows.iter().filter_map(semantic_fact) {
        match fact {
            TaskSemanticFact::EntryRecoveryPackageAdmitted { package_sha256 } if !started => {
                open.push(package_digest_prefix(package_sha256));
            }
            TaskSemanticFact::EntryRecoveryCompleted { package_sha256, .. } if !started => {
                let digest = package_digest_prefix(package_sha256);
                if let Some(index) = open.iter().rposition(|open| *open == digest) {
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

/// R25-1 (ruling 5961093808): whether a run failed inside its restart segment: it had a launch
/// or restart application effect and no `StepFinished` page after the last one is the main
/// interface of `game`.
pub(crate) fn in_restart_segment(rows: &[PersistedEvent], game: &str) -> bool {
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
                    if linear_main_interface(game, page_label)
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
