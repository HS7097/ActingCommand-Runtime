// SPDX-License-Identifier: AGPL-3.0-only

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, hash_map::RandomState};
use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::hash::{BuildHasher, Hash, Hasher};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub mod critical;
mod fact;
pub mod global;
pub use fact::PersistedEvent;
pub use global::*;

const ID_SCHEMA_VERSION: &str = "actingcommand.id.v0.1";
const LEDGER_HEADER_SCHEMA_VERSION: &str = "actingcommand.ledger.session.v0.1";
const LEDGER_RECORD_SCHEMA_VERSION: &str = "actingcommand.ledger.record.v0.1";
const EVENT_SCHEMA_VERSION: &str = "actingcommand.event.v0.1";
const EVIDENCE_SCHEMA_VERSION: &str = "actingcommand.evidence.v0.1";
const PROJECTION_SCHEMA_VERSION: &str = "actingcommand.projection.v0.1";
const LEDGER_FILE_NAME: &str = "ledger.jsonl";
pub const MIN_PROJECTION_SOFT_LIMIT_BYTES: usize = 1024;
pub const MIN_PROJECTION_HARD_LIMIT_BYTES: usize = 2048;
const DECISION_ARRAY_LIMIT: usize = 4;
const STRING_LIMIT_BYTES: usize = 200;

pub type LabLogResult<T> = Result<T, LabLogError>;

#[derive(Debug)]
pub enum LabLogError {
    Io(std::io::Error),
    Json(serde_json::Error),
    InvalidInput(String),
}

impl fmt::Display for LabLogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "lab logging I/O error: {err}"),
            Self::Json(err) => write!(f, "lab logging JSON error: {err}"),
            Self::InvalidInput(message) => write!(f, "invalid lab logging input: {message}"),
        }
    }
}

impl Error for LabLogError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Json(err) => Some(err),
            Self::InvalidInput(_) => None,
        }
    }
}

impl From<std::io::Error> for LabLogError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<serde_json::Error> for LabLogError {
    fn from(err: serde_json::Error) -> Self {
        Self::Json(err)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdKind {
    Run,
    Req,
    Task,
    Lease,
    Reco,
    Action,
    Evidence,
    Wf,
}

impl IdKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Req => "req",
            Self::Task => "task",
            Self::Lease => "lease",
            Self::Reco => "reco",
            Self::Action => "action",
            Self::Evidence => "evidence",
            Self::Wf => "wf",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuedId {
    pub schema_version: String,
    pub kind: IdKind,
    pub generation: u64,
    pub monotonic: u64,
    pub value: String,
}

impl IssuedId {
    pub fn parse(value: &str) -> LabLogResult<Self> {
        let parts = value.split('-').collect::<Vec<_>>();
        if parts.len() != 3 {
            return Err(LabLogError::InvalidInput(format!(
                "id must have <kind>-<generation>-<monotonic> shape: {value}"
            )));
        }
        let kind = parse_id_kind(parts[0])?;
        let generation = u64::from_str_radix(parts[1], 16).map_err(|err| {
            LabLogError::InvalidInput(format!("invalid id generation '{}': {err}", parts[1]))
        })?;
        let monotonic = parts[2].parse::<u64>().map_err(|err| {
            LabLogError::InvalidInput(format!("invalid id monotonic '{}': {err}", parts[2]))
        })?;
        Ok(Self {
            schema_version: ID_SCHEMA_VERSION.to_string(),
            kind,
            generation,
            monotonic,
            value: value.to_string(),
        })
    }
}

#[derive(Debug)]
pub struct IdIssuer {
    generation: u64,
    next_monotonic: AtomicU64,
}

impl Default for IdIssuer {
    fn default() -> Self {
        Self::new()
    }
}

impl IdIssuer {
    pub fn new() -> Self {
        Self::with_generation(generate_generation())
    }

    pub fn with_generation(generation: u64) -> Self {
        Self {
            generation: if generation == 0 { 1 } else { generation },
            next_monotonic: AtomicU64::new(1),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn issue(&self, kind: IdKind) -> IssuedId {
        let monotonic = self.next_monotonic.fetch_add(1, Ordering::Relaxed);
        let value = format!("{}-{:016x}-{monotonic}", kind.as_str(), self.generation);
        IssuedId {
            schema_version: ID_SCHEMA_VERSION.to_string(),
            kind,
            generation: self.generation,
            monotonic,
            value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHeader {
    pub schema_version: String,
    pub line_type: String,
    pub runtime_version: String,
    pub game: String,
    pub server: String,
    pub instance: String,
    pub started_at_unix_ms: u64,
}

impl SessionHeader {
    pub fn new(
        runtime_version: impl Into<String>,
        game: impl Into<String>,
        server: impl Into<String>,
        instance: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: LEDGER_HEADER_SCHEMA_VERSION.to_string(),
            line_type: "session_header".to_string(),
            runtime_version: runtime_version.into(),
            game: game.into(),
            server: server.into(),
            instance: instance.into(),
            started_at_unix_ms: unix_ms_now(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerRecordKind {
    Drive,
    Dispatch,
    Receipt,
}

impl LedgerRecordKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Drive => "drive",
            Self::Dispatch => "dispatch",
            Self::Receipt => "receipt",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerRecord {
    pub schema_version: String,
    pub line_type: String,
    pub kind: LedgerRecordKind,
    pub timestamp_unix_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub req_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub id_chain: BTreeMap<String, String>,
    #[serde(default)]
    pub payload: Value,
}

impl LedgerRecord {
    pub fn new(kind: LedgerRecordKind, req_id: Option<String>, payload: Value) -> Self {
        Self {
            schema_version: LEDGER_RECORD_SCHEMA_VERSION.to_string(),
            line_type: "record".to_string(),
            kind,
            timestamp_unix_ms: unix_ms_now(),
            req_id,
            id_chain: BTreeMap::new(),
            payload,
        }
    }

    pub fn with_id(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.id_chain.insert(key.into(), value.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "line_type")]
enum LedgerLine {
    #[serde(rename = "session_header")]
    SessionHeader {
        schema_version: String,
        runtime_version: String,
        game: String,
        server: String,
        instance: String,
        started_at_unix_ms: u64,
    },
    #[serde(rename = "record")]
    Record {
        schema_version: String,
        kind: LedgerRecordKind,
        timestamp_unix_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        req_id: Option<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        id_chain: BTreeMap<String, String>,
        #[serde(default)]
        payload: Value,
    },
    #[serde(rename = "event")]
    Event {
        schema_version: String,
        event_type: String,
        timestamp_unix_ms: u64,
        ids: BTreeMap<String, String>,
        #[serde(default)]
        payload: Value,
    },
}

impl From<SessionHeader> for LedgerLine {
    fn from(header: SessionHeader) -> Self {
        Self::SessionHeader {
            schema_version: header.schema_version,
            runtime_version: header.runtime_version,
            game: header.game,
            server: header.server,
            instance: header.instance,
            started_at_unix_ms: header.started_at_unix_ms,
        }
    }
}

impl From<LedgerRecord> for LedgerLine {
    fn from(record: LedgerRecord) -> Self {
        Self::Record {
            schema_version: record.schema_version,
            kind: record.kind,
            timestamp_unix_ms: record.timestamp_unix_ms,
            req_id: record.req_id,
            id_chain: record.id_chain,
            payload: record.payload,
        }
    }
}

impl From<LightEvent> for LedgerLine {
    fn from(event: LightEvent) -> Self {
        Self::Event {
            schema_version: event.schema_version,
            event_type: event.event_type,
            timestamp_unix_ms: event.timestamp_unix_ms,
            ids: event.ids,
            payload: event.payload,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LedgerRead {
    pub header: Option<SessionHeader>,
    pub events: Vec<LightEvent>,
    pub records: Vec<LedgerRecord>,
    pub skipped_corrupt_lines: usize,
}

pub struct LabLedger {
    session_dir: PathBuf,
    ledger_path: PathBuf,
    writer: File,
}

impl LabLedger {
    pub fn create(
        run_root: impl AsRef<Path>,
        session_name: &str,
        header: SessionHeader,
    ) -> LabLogResult<Self> {
        validate_non_empty("session_name", session_name)?;
        let session_dir = run_root
            .as_ref()
            .join("sessions")
            .join(sanitize_path_segment(session_name));
        fs::create_dir_all(&session_dir)?;
        let ledger_path = session_dir.join(LEDGER_FILE_NAME);
        let mut writer = open_ledger_writer(&ledger_path)?;
        write_json_line(&mut writer, &LedgerLine::from(header))?;
        writer.sync_all()?;
        Ok(Self {
            session_dir,
            ledger_path,
            writer,
        })
    }

    pub fn open_or_create(
        run_root: impl AsRef<Path>,
        session_name: &str,
        header: SessionHeader,
    ) -> LabLogResult<Self> {
        validate_non_empty("session_name", session_name)?;
        let session_dir = run_root
            .as_ref()
            .join("sessions")
            .join(sanitize_path_segment(session_name));
        fs::create_dir_all(&session_dir)?;
        let ledger_path = session_dir.join(LEDGER_FILE_NAME);
        let needs_header = !ledger_path.exists() || fs::metadata(&ledger_path)?.len() == 0;
        let mut writer = open_ledger_writer(&ledger_path)?;
        if needs_header {
            write_json_line(&mut writer, &LedgerLine::from(header))?;
            writer.sync_all()?;
        }
        Ok(Self {
            session_dir,
            ledger_path,
            writer,
        })
    }

    pub fn create_runtime_shard(
        run_root: impl AsRef<Path>,
        run_id: &str,
        instance_id: &str,
        header: SessionHeader,
    ) -> LabLogResult<Self> {
        validate_non_empty("run_id", run_id)?;
        validate_non_empty("instance_id", instance_id)?;
        let session_dir = run_root
            .as_ref()
            .join("runtime-ledger")
            .join("instances")
            .join(sanitize_path_segment(instance_id))
            .join("runs")
            .join(sanitize_path_segment(run_id));
        fs::create_dir_all(&session_dir)?;
        let ledger_path = session_dir.join(LEDGER_FILE_NAME);
        let mut writer = open_ledger_writer(&ledger_path)?;
        write_json_line(&mut writer, &LedgerLine::from(header))?;
        writer.sync_all()?;
        Ok(Self {
            session_dir,
            ledger_path,
            writer,
        })
    }

    pub fn session_dir(&self) -> &Path {
        &self.session_dir
    }

    pub fn ledger_path(&self) -> &Path {
        &self.ledger_path
    }

    pub fn append(&mut self, record: LedgerRecord) -> LabLogResult<()> {
        let durable = matches!(record.kind, LedgerRecordKind::Receipt);
        write_json_line(&mut self.writer, &LedgerLine::from(record))?;
        if durable {
            self.writer.sync_all()?;
        }
        Ok(())
    }

    pub fn append_event(&mut self, event: LightEvent) -> LabLogResult<()> {
        write_json_line(&mut self.writer, &LedgerLine::from(event))
    }

    pub fn sync(&self) -> LabLogResult<()> {
        self.writer.sync_all()?;
        Ok(())
    }

    pub fn read(path: impl AsRef<Path>) -> LabLogResult<LedgerRead> {
        let file = File::open(path)?;
        let mut header = None;
        let mut events = Vec::new();
        let mut records = Vec::new();
        let mut skipped_corrupt_lines = 0;
        for line in BufReader::new(file).lines() {
            let line = line?;
            match serde_json::from_str::<LedgerLine>(&line) {
                Ok(LedgerLine::SessionHeader {
                    schema_version,
                    runtime_version,
                    game,
                    server,
                    instance,
                    started_at_unix_ms,
                }) => {
                    header = Some(SessionHeader {
                        schema_version,
                        line_type: "session_header".to_string(),
                        runtime_version,
                        game,
                        server,
                        instance,
                        started_at_unix_ms,
                    });
                }
                Ok(LedgerLine::Record {
                    schema_version,
                    kind,
                    timestamp_unix_ms,
                    req_id,
                    id_chain,
                    payload,
                }) => records.push(LedgerRecord {
                    schema_version,
                    line_type: "record".to_string(),
                    kind,
                    timestamp_unix_ms,
                    req_id,
                    id_chain,
                    payload,
                }),
                Ok(LedgerLine::Event {
                    schema_version,
                    event_type,
                    timestamp_unix_ms,
                    ids,
                    payload,
                }) => events.push(LightEvent {
                    schema_version,
                    event_type,
                    timestamp_unix_ms,
                    ids,
                    payload,
                }),
                Err(_) => skipped_corrupt_lines += 1,
            }
        }
        Ok(LedgerRead {
            header,
            events,
            records,
            skipped_corrupt_lines,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LightEvent {
    pub schema_version: String,
    pub event_type: String,
    pub timestamp_unix_ms: u64,
    pub ids: BTreeMap<String, String>,
    #[serde(default)]
    pub payload: Value,
}

impl LightEvent {
    pub fn new(
        event_type: impl Into<String>,
        ids: BTreeMap<String, String>,
        payload: Value,
    ) -> LabLogResult<Self> {
        let event_type = event_type.into();
        validate_event_type(&event_type)?;
        Ok(Self {
            schema_version: EVENT_SCHEMA_VERSION.to_string(),
            event_type,
            timestamp_unix_ms: unix_ms_now(),
            ids,
            payload,
        })
    }
}

#[must_use = "CommitProof must be consumed by the terminal fact recording path"]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitProof<T> {
    value: T,
}

impl<T> CommitProof<T> {
    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn into_inner(self) -> T {
        self.value
    }
}

/// Runs the side effect before producing the proof required to record its terminal fact.
pub fn commit_then_record<T, E>(
    commit: impl FnOnce() -> Result<T, E>,
) -> Result<CommitProof<T>, E> {
    commit().map(|value| CommitProof { value })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub schema_version: String,
    pub evidence_id: String,
    pub relative_path: String,
    pub sha256: String,
    pub bytes: u64,
}

pub struct EvidenceStore {
    root_dir: PathBuf,
    debug_enabled: bool,
}

impl EvidenceStore {
    pub fn new(root_dir: impl Into<PathBuf>, debug_enabled: bool) -> Self {
        Self {
            root_dir: root_dir.into(),
            debug_enabled,
        }
    }

    pub fn put(
        &self,
        evidence_id: &str,
        label: &str,
        bytes: &[u8],
    ) -> LabLogResult<Option<EvidenceRef>> {
        validate_non_empty("evidence_id", evidence_id)?;
        validate_non_empty("label", label)?;
        if !self.debug_enabled {
            return Ok(None);
        }
        let safe_id = sanitize_path_segment(evidence_id);
        let safe_label = sanitize_path_segment(label);
        let sha256 = sha256_hex(bytes);
        let relative_path = format!("evidence/{safe_id}/{safe_label}-{}.bin", &sha256[..8]);
        validate_relative_path(&relative_path)?;
        let absolute = self.root_dir.join(&relative_path);
        if let Some(parent) = absolute.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&absolute, bytes)?;
        Ok(Some(EvidenceRef {
            schema_version: EVIDENCE_SCHEMA_VERSION.to_string(),
            evidence_id: evidence_id.to_string(),
            relative_path,
            sha256: format!("sha256:{sha256}"),
            bytes: bytes.len() as u64,
        }))
    }

    pub fn list_by_id(&self, evidence_id: &str) -> LabLogResult<Vec<EvidenceRef>> {
        let safe_id = sanitize_path_segment(evidence_id);
        let dir = self.root_dir.join("evidence").join(&safe_id);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut refs = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let bytes = fs::read(&path)?;
            let rel = path
                .strip_prefix(&self.root_dir)
                .map_err(|err| LabLogError::InvalidInput(err.to_string()))?
                .to_string_lossy()
                .replace('\\', "/");
            validate_relative_path(&rel)?;
            refs.push(EvidenceRef {
                schema_version: EVIDENCE_SCHEMA_VERSION.to_string(),
                evidence_id: evidence_id.to_string(),
                relative_path: rel,
                sha256: format!("sha256:{}", sha256_hex(&bytes)),
                bytes: bytes.len() as u64,
            });
        }
        refs.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(refs)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LastResortError {
    pub command: String,
    pub phase: String,
    pub error_code: String,
    pub error_message: String,
    pub timestamp_unix_ms: u64,
    pub input_summary: Value,
    pub attempted_ledger_path: Option<String>,
}

impl LastResortError {
    pub fn new(
        command: impl Into<String>,
        phase: impl Into<String>,
        error_code: impl Into<String>,
        error_message: impl Into<String>,
        input_summary: Value,
        attempted_ledger_path: Option<String>,
    ) -> Self {
        Self {
            command: command.into(),
            phase: phase.into(),
            error_code: error_code.into(),
            error_message: error_message.into(),
            timestamp_unix_ms: unix_ms_now(),
            input_summary,
            attempted_ledger_path,
        }
    }
}

pub fn write_last_resort_error(
    run_root: Option<&Path>,
    error: &LastResortError,
) -> LabLogResult<PathBuf> {
    eprintln!(
        "actingcommand-ledger last resort: command={} phase={} code={} error={} attempted_ledger_path={}",
        error.command,
        error.phase,
        error.error_code,
        error.error_message,
        error.attempted_ledger_path.as_deref().unwrap_or("unknown")
    );
    let preferred = run_root.map(|root| root.join("last-error.json"));
    if let Some(path) = preferred
        && write_last_resort_file(&path, error).is_ok()
    {
        return Ok(path);
    }
    let fallback = std::env::temp_dir().join(format!(
        "actingcommand-last-error-{}.json",
        error.timestamp_unix_ms
    ));
    write_last_resort_file(&fallback, error)?;
    Ok(fallback)
}

fn write_last_resort_file(path: &Path, error: &LastResortError) -> LabLogResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(error)?;
    let mut file = File::create(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionCandidate {
    pub path: PathBuf,
    pub bytes: u64,
    pub age: Duration,
    pub protected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionReport {
    pub max_bytes: u64,
    pub total_before_bytes: u64,
    pub total_after_bytes: u64,
    pub deleted_count: usize,
    pub deleted_paths: Vec<String>,
    pub protected_over_budget: bool,
}

pub fn select_retention_deletions(
    candidates: &[RetentionCandidate],
    max_bytes: u64,
) -> Vec<PathBuf> {
    let mut total = candidates
        .iter()
        .fold(0u64, |sum, candidate| sum.saturating_add(candidate.bytes));
    if total <= max_bytes {
        return Vec::new();
    }
    let mut removable = candidates
        .iter()
        .filter(|candidate| !candidate.protected)
        .collect::<Vec<_>>();
    removable.sort_by_key(|candidate| std::cmp::Reverse(candidate.age));
    let mut deletions = Vec::new();
    for candidate in removable {
        if total <= max_bytes {
            break;
        }
        total = total.saturating_sub(candidate.bytes);
        deletions.push(candidate.path.clone());
    }
    deletions
}

pub fn enforce_retention(
    root: impl AsRef<Path>,
    max_bytes: u64,
    protected_age: Duration,
) -> LabLogResult<RetentionReport> {
    let root = root.as_ref();
    let mut candidates = Vec::new();
    collect_retention_candidates(root, root, protected_age, &mut candidates)?;
    let total_before = candidates
        .iter()
        .fold(0u64, |sum, candidate| sum.saturating_add(candidate.bytes));
    let deletions = select_retention_deletions(&candidates, max_bytes);
    let mut deleted_paths = Vec::new();
    let mut deleted_bytes = 0u64;
    for path in &deletions {
        if let Some(candidate) = candidates.iter().find(|candidate| candidate.path == *path) {
            deleted_bytes = deleted_bytes.saturating_add(candidate.bytes);
        }
        fs::remove_file(path)?;
        deleted_paths.push(path.display().to_string());
    }
    let total_after = total_before.saturating_sub(deleted_bytes);
    Ok(RetentionReport {
        max_bytes,
        total_before_bytes: total_before,
        total_after_bytes: total_after,
        deleted_count: deleted_paths.len(),
        deleted_paths,
        protected_over_budget: total_after > max_bytes,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionVerbosity {
    Min,
    Normal,
    Debug,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionRequest {
    pub verbosity: ProjectionVerbosity,
    pub fields: BTreeSet<String>,
    pub evidence_id: Option<String>,
}

impl ProjectionRequest {
    pub fn min() -> Self {
        Self {
            verbosity: ProjectionVerbosity::Min,
            fields: BTreeSet::new(),
            evidence_id: None,
        }
    }

    pub fn with_fields(mut self, fields: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.fields = fields.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_evidence_id(mut self, evidence_id: impl Into<String>) -> Self {
        self.evidence_id = Some(evidence_id.into());
        self
    }
}

pub fn project_record(record: &Value, request: &ProjectionRequest) -> LabLogResult<Value> {
    let mut projected = record.clone();
    if request.verbosity != ProjectionVerbosity::Min {
        return Ok(projected);
    }
    let Value::Object(object) = &mut projected else {
        return Ok(projected);
    };

    object.insert(
        "projection_schema_version".to_string(),
        json!(PROJECTION_SCHEMA_VERSION),
    );
    object.remove("schema_version");
    object.remove("cli_version");
    object.remove("runtime_version");
    let soft_limit_exceeded = json_len(object)? > MIN_PROJECTION_SOFT_LIMIT_BYTES;
    summarize_decision_array(
        object,
        "targets",
        request.evidence_id.as_deref(),
        soft_limit_exceeded,
    );
    summarize_decision_array(
        object,
        "actions",
        request.evidence_id.as_deref(),
        soft_limit_exceeded,
    );
    if let Some(Value::Object(suspicion)) = object.get_mut("suspicion") {
        summarize_decision_array(
            suspicion,
            "candidates",
            request.evidence_id.as_deref(),
            soft_limit_exceeded,
        );
    }
    ensure_error_fields(object);
    shrink_to_limit(object, request)?;
    Ok(projected)
}

pub fn project_ledger_records(
    read: &LedgerRead,
    request: &ProjectionRequest,
) -> LabLogResult<Vec<Value>> {
    read.records
        .iter()
        .map(|record| {
            project_record(
                &json!({
                    "kind": record.kind.as_str(),
                    "timestamp_unix_ms": record.timestamp_unix_ms,
                    "req_id": record.req_id,
                    "id_chain": record.id_chain,
                    "payload": record.payload
                }),
                request,
            )
        })
        .collect()
}

pub fn project_light_events(read: &LedgerRead) -> Vec<Value> {
    read.events
        .iter()
        .map(|event| {
            json!({
                "schema_version": event.schema_version,
                "event_type": event.event_type,
                "timestamp_unix_ms": event.timestamp_unix_ms,
                "ids": event.ids,
                "payload": event.payload,
                "projection_source": "runtime_ledger"
            })
        })
        .collect()
}

pub fn error_projection(
    req_id: impl Into<String>,
    error: impl Into<String>,
    state: impl Into<String>,
    hint: impl Into<String>,
) -> Value {
    json!({
        "req_id": req_id.into(),
        "error": error.into(),
        "state": state.into(),
        "hint": hint.into()
    })
}

pub fn low_margin_suspicion(candidates: Vec<Value>) -> Value {
    json!({
        "reason": "low_page_margin",
        "candidates": candidates
    })
}

pub fn forbidden_target_suspicion(targets: Vec<String>) -> Value {
    json!({
        "reason": "forbidden_target",
        "targets": targets
    })
}

pub fn guard_reject_suspicion(expected: &str, actual: &str) -> Value {
    json!({
        "reason": "guard_rejected",
        "expected": expected,
        "actual": actual
    })
}

pub fn stale_frame_suspicion(frame_age_ms: u64) -> Value {
    json!({
        "reason": "stale_frame",
        "frame_age_ms": frame_age_ms
    })
}

fn parse_id_kind(value: &str) -> LabLogResult<IdKind> {
    match value {
        "run" => Ok(IdKind::Run),
        "req" => Ok(IdKind::Req),
        "task" => Ok(IdKind::Task),
        "lease" => Ok(IdKind::Lease),
        "reco" => Ok(IdKind::Reco),
        "action" => Ok(IdKind::Action),
        "evidence" => Ok(IdKind::Evidence),
        "wf" => Ok(IdKind::Wf),
        other => Err(LabLogError::InvalidInput(format!(
            "unknown id kind: {other}"
        ))),
    }
}

fn generate_generation() -> u64 {
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    unix_ms_now().hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    let generation = hasher.finish();
    if generation == 0 { 1 } else { generation }
}

fn unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}

fn open_ledger_writer(path: &Path) -> LabLogResult<File> {
    Ok(OpenOptions::new().create(true).append(true).open(path)?)
}

fn write_json_line(writer: &mut File, value: &impl Serialize) -> LabLogResult<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn validate_non_empty(name: &str, value: &str) -> LabLogResult<()> {
    if value.trim().is_empty() {
        return Err(LabLogError::InvalidInput(format!(
            "{name} must not be empty"
        )));
    }
    Ok(())
}

fn validate_event_type(value: &str) -> LabLogResult<()> {
    let parts = value.split('.').collect::<Vec<_>>();
    if parts.len() != 3 || parts.iter().any(|part| part.trim().is_empty()) {
        return Err(LabLogError::InvalidInput(format!(
            "event_type must use <subject>.<event>.<phase>: {value}"
        )));
    }
    Ok(())
}

fn sanitize_path_segment(value: &str) -> String {
    let mut output = String::new();
    let mut previous_underscore = false;
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_') {
            output.push(byte as char);
            previous_underscore = false;
        } else if !previous_underscore {
            output.push('_');
            previous_underscore = true;
        }
    }
    let output = output.trim_matches('_');
    if output.is_empty() || output == "." || output == ".." {
        "unknown".to_string()
    } else {
        output.to_string()
    }
}

fn validate_relative_path(value: &str) -> LabLogResult<()> {
    if value.contains('\\') || value.contains(':') {
        return Err(LabLogError::InvalidInput(format!(
            "relative evidence path is not portable: {value}"
        )));
    }
    if value.split('/').any(|part| part == "." || part == "..") {
        return Err(LabLogError::InvalidInput(format!(
            "relative evidence path contains traversal: {value}"
        )));
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn collect_retention_candidates(
    root: &Path,
    current: &Path,
    protected_age: Duration,
    candidates: &mut Vec<RetentionCandidate>,
) -> LabLogResult<()> {
    if !current.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_retention_candidates(root, &path, protected_age, candidates)?;
            continue;
        }
        let metadata = entry.metadata()?;
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let age = SystemTime::now()
            .duration_since(modified)
            .unwrap_or(Duration::ZERO);
        candidates.push(RetentionCandidate {
            path: path
                .strip_prefix(root)
                .map(|relative| root.join(relative))
                .unwrap_or(path),
            bytes: metadata.len(),
            age,
            protected: age < protected_age,
        });
    }
    Ok(())
}

fn summarize_decision_array(
    object: &mut Map<String, Value>,
    key: &str,
    evidence_id: Option<&str>,
    soft_limit_exceeded: bool,
) {
    let Some(Value::Array(items)) = object.get_mut(key) else {
        return;
    };
    if items.len() <= DECISION_ARRAY_LIMIT {
        if soft_limit_exceeded {
            let summarized = items
                .drain(..)
                .map(decision_item_summary)
                .collect::<Vec<_>>();
            *items = summarized;
        }
        return;
    }
    items.sort_by(compare_decision_items);
    let keep = items.len().min(DECISION_ARRAY_LIMIT);
    let more = items.len().saturating_sub(keep);
    let top = items
        .drain(..keep)
        .map(decision_item_summary)
        .collect::<Vec<_>>();
    object.insert(
        key.to_string(),
        json!({
            "items": top,
            "_more": more,
            "_full": evidence_id.unwrap_or("unavailable")
        }),
    );
}

fn decision_item_summary(item: Value) -> Value {
    let Value::Object(object) = item else {
        return item;
    };
    let mut summary = Map::new();
    for key in ["id", "target", "action", "kind", "state", "passed", "score"] {
        if let Some(value) = object.get(key) {
            summary.insert(key.to_string(), value.clone());
        }
    }
    if summary.is_empty() {
        Value::Object(object)
    } else {
        Value::Object(summary)
    }
}

fn compare_decision_items(left: &Value, right: &Value) -> std::cmp::Ordering {
    let left_passed = left.get("passed").and_then(Value::as_bool).unwrap_or(false);
    let right_passed = right
        .get("passed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    right_passed.cmp(&left_passed).then_with(|| {
        let left_score = left.get("score").and_then(Value::as_f64).unwrap_or(0.0);
        let right_score = right.get("score").and_then(Value::as_f64).unwrap_or(0.0);
        right_score
            .partial_cmp(&left_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

fn ensure_error_fields(object: &mut Map<String, Value>) {
    if !object.contains_key("error") {
        return;
    }
    object
        .entry("state".to_string())
        .or_insert_with(|| json!("unknown"));
    object
        .entry("hint".to_string())
        .or_insert_with(|| json!("inspect-ledger"));
    object
        .entry("req_id".to_string())
        .or_insert_with(|| json!("unissued"));
}

fn shrink_to_limit(
    object: &mut Map<String, Value>,
    request: &ProjectionRequest,
) -> LabLogResult<()> {
    if json_len(object)? <= MIN_PROJECTION_HARD_LIMIT_BYTES {
        return Ok(());
    }
    trim_long_strings(object, request);
    if json_len(object)? <= MIN_PROJECTION_HARD_LIMIT_BYTES {
        return Ok(());
    }
    let protected = protected_fields(&request.fields);
    let mut removable = object
        .keys()
        .filter(|key| !protected.contains(key.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    removable.sort();
    for key in removable.into_iter().rev() {
        if json_len(object)? <= MIN_PROJECTION_HARD_LIMIT_BYTES {
            break;
        }
        object.remove(&key);
    }
    Ok(())
}

fn trim_long_strings(object: &mut Map<String, Value>, request: &ProjectionRequest) {
    let protected = protected_fields(&request.fields);
    let evidence_id = request.evidence_id.as_deref().unwrap_or("unavailable");
    let keys = object.keys().cloned().collect::<Vec<_>>();
    for key in keys {
        if protected.contains(key.as_str()) {
            continue;
        }
        let Some(Value::String(text)) = object.get_mut(&key) else {
            continue;
        };
        if text.len() <= STRING_LIMIT_BYTES {
            continue;
        }
        let truncated = format!(
            "{}...",
            text.chars()
                .take(STRING_LIMIT_BYTES - 3)
                .collect::<String>()
        );
        *text = truncated;
        object.insert(format!("{key}_full"), json!(evidence_id));
    }
}

fn protected_fields(extra: &BTreeSet<String>) -> BTreeSet<&str> {
    let mut fields = [
        "req_id",
        "error",
        "state",
        "hint",
        "observation",
        "page",
        "standby",
        "stale",
        "frame_age_ms",
        "backend",
        "actions",
        "actual_click",
        "guard_result",
        "projection_schema_version",
        "recovered",
        "suspicion",
        "targets",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    for field in extra {
        fields.insert(field.as_str());
    }
    fields
}

fn json_len(object: &Map<String, Value>) -> LabLogResult<usize> {
    Ok(serde_json::to_vec(object)?.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn id_issuer_generates_parseable_unique_ids_across_threads() {
        let issuer = Arc::new(IdIssuer::with_generation(0x1234));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let issuer = Arc::clone(&issuer);
            handles.push(thread::spawn(move || {
                (0..128)
                    .map(|_| issuer.issue(IdKind::Req).value)
                    .collect::<Vec<_>>()
            }));
        }
        let mut ids = BTreeSet::new();
        for handle in handles {
            for id in handle.join().expect("id thread") {
                assert!(ids.insert(id.clone()));
                let parsed = IssuedId::parse(&id).expect("parse id");
                assert_eq!(parsed.kind, IdKind::Req);
                assert_eq!(parsed.generation, 0x1234);
            }
        }
        assert_eq!(ids.len(), 1024);
        assert_eq!(
            IssuedId::parse(&issuer.issue(IdKind::Run).value)
                .expect("run id")
                .kind,
            IdKind::Run
        );
        assert_eq!(
            IssuedId::parse(&issuer.issue(IdKind::Evidence).value)
                .expect("evidence id")
                .kind,
            IdKind::Evidence
        );
    }

    #[test]
    fn default_generation_changes_between_issuers() {
        let left = IdIssuer::new().generation();
        let right = IdIssuer::new().generation();

        assert_ne!(left, 0);
        assert_ne!(right, 0);
        assert_ne!(left, right);
    }

    #[test]
    fn ledger_writes_flushes_and_skips_corrupt_tail() {
        let temp = tempfile::tempdir().expect("tempdir");
        let header = SessionHeader::new("runtime", "arknights", "cn_b", "ak");
        let mut ledger = LabLedger::create(temp.path(), "session 1", header).expect("ledger");
        ledger
            .append(
                LedgerRecord::new(
                    LedgerRecordKind::Drive,
                    Some("req-1".to_string()),
                    json!({"kind": "capture", "ok": true}),
                )
                .with_id("action_id", "action-1"),
            )
            .expect("append drive");
        ledger
            .append(LedgerRecord::new(
                LedgerRecordKind::Receipt,
                Some("req-1".to_string()),
                json!({"state": "ok"}),
            ))
            .expect("append receipt");
        ledger
            .append_event(
                LightEvent::new(
                    "runtime.state.finished",
                    BTreeMap::from([("req_id".to_string(), "req-1".to_string())]),
                    json!({"state": "ok"}),
                )
                .expect("event"),
            )
            .expect("append event");
        let path = ledger.ledger_path().to_path_buf();
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open corrupt")
            .write_all(b"{not-json\n")
            .expect("write corrupt");

        let read = LabLedger::read(path).expect("read ledger");

        assert_eq!(read.header.as_ref().unwrap().game, "arknights");
        assert_eq!(read.events.len(), 1);
        assert_eq!(read.events[0].event_type, "runtime.state.finished");
        assert_eq!(read.records.len(), 2);
        assert_eq!(read.records[0].kind, LedgerRecordKind::Drive);
        assert_eq!(read.records[1].kind, LedgerRecordKind::Receipt);
        assert_eq!(read.skipped_corrupt_lines, 1);
    }

    #[test]
    fn open_or_create_reuses_existing_ledger_without_duplicate_header() {
        let temp = tempfile::tempdir().expect("tempdir");
        let header = SessionHeader::new("runtime", "session", "session", "session");
        let mut ledger =
            LabLedger::open_or_create(temp.path(), "session requests", header).expect("ledger");
        ledger
            .append(LedgerRecord::new(
                LedgerRecordKind::Receipt,
                Some("req-1".to_string()),
                json!({"record_type": "session_request_receipt"}),
            ))
            .expect("append");
        let path = ledger.ledger_path().to_path_buf();
        drop(ledger);

        let header = SessionHeader::new("runtime", "other", "other", "other");
        let mut reopened =
            LabLedger::open_or_create(temp.path(), "session requests", header).expect("reopen");
        reopened
            .append(LedgerRecord::new(
                LedgerRecordKind::Receipt,
                Some("req-2".to_string()),
                json!({"record_type": "session_request_receipt"}),
            ))
            .expect("append reopened");
        drop(reopened);

        let line_types = fs::read_to_string(path)
            .expect("read ledger")
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("json"))
            .map(|value| value["line_type"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();

        assert_eq!(line_types, vec!["session_header", "record", "record"]);
    }

    #[test]
    fn commit_then_record_yields_proof_only_after_successful_commit() {
        let proof = commit_then_record(|| -> Result<_, LabLogError> { Ok("artifact.zip") })
            .expect("commit proof");
        assert_eq!(proof.value(), &"artifact.zip");

        let failed = commit_then_record(|| -> Result<&str, LabLogError> {
            Err(LabLogError::InvalidInput("commit failed".to_string()))
        });
        assert!(failed.is_err());
    }

    #[test]
    fn ledger_record_golden_shapes_are_stable() {
        let record = LedgerRecord::new(
            LedgerRecordKind::Dispatch,
            Some("req-1".to_string()),
            json!({"decision": "accepted"}),
        )
        .with_id("lease_id", "lease-1");
        let line = serde_json::to_value(LedgerLine::from(record)).expect("json");

        assert_eq!(line["line_type"], "record");
        assert_eq!(line["schema_version"], LEDGER_RECORD_SCHEMA_VERSION);
        assert_eq!(line["kind"], "dispatch");
        assert_eq!(line["req_id"], "req-1");
        assert_eq!(line["id_chain"]["lease_id"], "lease-1");
        assert_eq!(line["payload"]["decision"], "accepted");
    }

    #[test]
    fn light_events_require_three_part_names_and_keep_ids() {
        let event = LightEvent::new(
            "runtime.state.started",
            BTreeMap::from([("req_id".to_string(), "req-1".to_string())]),
            json!({"state": "running"}),
        )
        .expect("event");

        assert_eq!(event.event_type, "runtime.state.started");
        assert_eq!(event.ids["req_id"], "req-1");
        assert!(LightEvent::new("bad.event", BTreeMap::new(), json!({})).is_err());
    }

    #[test]
    fn evidence_store_skips_heavy_assets_outside_debug_mode() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = EvidenceStore::new(temp.path(), false);

        assert_eq!(store.put("reco-1", "frame", b"image").unwrap(), None);
        assert!(!temp.path().join("evidence").exists());
    }

    #[test]
    fn evidence_store_debug_mode_writes_and_lists_by_id() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = EvidenceStore::new(temp.path(), true);
        let evidence = store
            .put("reco-1", "frame", b"image bytes")
            .unwrap()
            .expect("debug evidence");

        assert!(temp.path().join(&evidence.relative_path).exists());
        assert_eq!(
            evidence.sha256,
            format!("sha256:{}", sha256_hex(b"image bytes"))
        );
        let refs = store.list_by_id("reco-1").expect("refs");
        assert_eq!(refs, vec![evidence]);
    }

    #[test]
    fn retention_deletes_oldest_unprotected_until_under_budget() {
        let deletions = select_retention_deletions(
            &[
                RetentionCandidate {
                    path: PathBuf::from("protected"),
                    bytes: 90,
                    age: Duration::from_secs(1000),
                    protected: true,
                },
                RetentionCandidate {
                    path: PathBuf::from("old"),
                    bytes: 80,
                    age: Duration::from_secs(800),
                    protected: false,
                },
                RetentionCandidate {
                    path: PathBuf::from("new"),
                    bytes: 20,
                    age: Duration::from_secs(1),
                    protected: false,
                },
            ],
            100,
        );

        assert_eq!(deletions, vec![PathBuf::from("old"), PathBuf::from("new")]);
    }

    #[test]
    fn projection_removes_redundant_versions_and_preserves_core_scalars() {
        let record = json!({
            "schema_version": "x",
            "cli_version": "x",
            "runtime_version": "x",
            "req_id": "req-1",
            "state": "ok",
            "hint": "none",
            "page": "home",
            "debug_blob": "x".repeat(3000)
        });
        let projected = project_record(
            &record,
            &ProjectionRequest::min().with_evidence_id("evidence-1"),
        )
        .expect("project");

        assert_eq!(projected["req_id"], "req-1");
        assert_eq!(projected["state"], "ok");
        assert_eq!(projected["hint"], "none");
        assert_eq!(projected["page"], "home");
        assert!(projected.get("schema_version").is_none());
        assert!(serde_json::to_vec(&projected).unwrap().len() <= MIN_PROJECTION_HARD_LIMIT_BYTES);
    }

    #[test]
    fn projection_fields_request_protects_non_core_fields() {
        let record = json!({
            "req_id": "req-1",
            "state": "ok",
            "hint": "none",
            "operator_note": "important",
            "debug_blob": "x".repeat(3000)
        });
        let projected = project_record(
            &record,
            &ProjectionRequest::min().with_fields(["operator_note"]),
        )
        .expect("project");

        assert_eq!(projected["operator_note"], "important");
    }

    #[test]
    fn projection_summarizes_decision_arrays_with_full_pointer() {
        let record = json!({
            "req_id": "req-1",
            "state": "ok",
            "hint": "none",
            "targets": [
                {"id": "a", "passed": false, "score": 0.1},
                {"id": "b", "passed": true, "score": 0.5},
                {"id": "c", "passed": true, "score": 0.9},
                {"id": "d", "passed": false, "score": 0.8},
                {"id": "e", "passed": false, "score": 0.7}
            ]
        });
        let projected = project_record(
            &record,
            &ProjectionRequest::min().with_evidence_id("full-targets"),
        )
        .expect("project");

        assert_eq!(projected["targets"]["items"][0]["id"], "c");
        assert_eq!(projected["targets"]["_more"], 1);
        assert_eq!(projected["targets"]["_full"], "full-targets");
    }

    #[test]
    fn projection_summarizes_twenty_decision_targets_with_more_and_full_pointer() {
        let targets = (0..20)
            .map(|index| {
                json!({
                    "id": format!("target-{index:02}"),
                    "passed": index == 17,
                    "score": index as f64 / 20.0,
                    "diagnostics": "x".repeat(200)
                })
            })
            .collect::<Vec<_>>();
        let record = json!({
            "req_id": "req-1",
            "state": "ok",
            "hint": "none",
            "targets": targets
        });

        let projected = project_record(
            &record,
            &ProjectionRequest::min().with_evidence_id("full-twenty-targets"),
        )
        .expect("project");

        let items = projected["targets"]["items"].as_array().expect("items");
        assert_eq!(items.len(), DECISION_ARRAY_LIMIT);
        assert_eq!(projected["targets"]["_more"], 16);
        assert_eq!(projected["targets"]["_full"], "full-twenty-targets");
        assert_eq!(items[0]["id"], "target-17");
        assert!(items.iter().all(|item| item.get("diagnostics").is_none()));
    }

    #[test]
    fn projection_keeps_decision_arrays_when_payload_exceeds_hard_limit() {
        let bulky = "x".repeat(600);
        let targets = (0..8)
            .map(|index| {
                json!({
                    "id": format!("target-{index}"),
                    "passed": index % 2 == 0,
                    "score": index as f64 / 10.0,
                    "debug": bulky
                })
            })
            .collect::<Vec<_>>();
        let actions = (0..8)
            .map(|index| {
                json!({
                    "id": format!("action-{index}"),
                    "passed": true,
                    "score": 1.0 - index as f64 / 10.0,
                    "diagnostics": bulky
                })
            })
            .collect::<Vec<_>>();
        let record = json!({
            "req_id": "req-1",
            "state": "ok",
            "hint": "none",
            "targets": targets,
            "actions": actions,
            "debug_blob": "z".repeat(4096)
        });

        let projected = project_record(
            &record,
            &ProjectionRequest::min().with_evidence_id("full-record"),
        )
        .expect("project");

        assert!(projected.get("targets").is_some());
        assert!(projected.get("actions").is_some());
        assert_eq!(projected["targets"]["_full"], "full-record");
        assert_eq!(projected["actions"]["_full"], "full-record");
        assert!(projected["targets"]["items"][0].get("debug").is_none());
        assert!(
            projected["actions"]["items"][0]
                .get("diagnostics")
                .is_none()
        );
        assert!(serde_json::to_vec(&projected).unwrap().len() <= MIN_PROJECTION_HARD_LIMIT_BYTES);
    }

    #[test]
    fn projection_error_adds_actionable_three_fields() {
        let record = json!({"error": "stale_frame"});
        let projected = project_record(&record, &ProjectionRequest::min()).expect("project");

        assert_eq!(projected["error"], "stale_frame");
        assert_eq!(projected["state"], "unknown");
        assert_eq!(projected["hint"], "inspect-ledger");
        assert_eq!(projected["req_id"], "unissued");
    }

    #[test]
    fn suspicion_helpers_cover_required_escalation_reasons() {
        assert_eq!(
            low_margin_suspicion(vec![json!({"page": "a"}), json!({"page": "b"})])["reason"],
            "low_page_margin"
        );
        assert_eq!(
            forbidden_target_suspicion(vec!["popup".to_string()])["reason"],
            "forbidden_target"
        );
        assert_eq!(
            guard_reject_suspicion("home", "terminal")["reason"],
            "guard_rejected"
        );
        assert_eq!(stale_frame_suspicion(5000)["reason"], "stale_frame");
    }

    #[test]
    fn error_projection_shape_is_stable() {
        let projected = error_projection("req-1", "resource_drift", "terminal", "stop");

        assert_eq!(
            projected,
            json!({
                "req_id": "req-1",
                "error": "resource_drift",
                "state": "terminal",
                "hint": "stop"
            })
        );
    }

    #[test]
    fn min_soft_limit_constant_is_lower_than_hard_limit() {
        assert!(MIN_PROJECTION_SOFT_LIMIT_BYTES < MIN_PROJECTION_HARD_LIMIT_BYTES);
    }
}
