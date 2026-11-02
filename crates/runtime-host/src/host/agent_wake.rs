// SPDX-License-Identifier: AGPL-3.0-only

//! The worker of the Runtime's messages to registered channels (Workflow #432 AGT-1b;
//! `contracts/agent-wake.md`; register §1.246). One thread per Runtime, only with an `agent_wake`
//! section:
//!
//! ```text
//! bounded queue → body file → row `prepared` → [quiet or off? stay]
//!               → one send through `AgentForward` → row `sending` (durable) → the result
//!               → (later) retry the deferred rows, the retention sweep
//! ```
//!
//! - **The queue** is built with the host's shared state, before the start's settlement writes
//!   records, so what AGT-2's tap offers then waits for the thread. Only a person's
//!   `AgentWakeTest` reaches it in this slice: nothing wakes automatically yet.
//! - **The rows** are the runtime fact `host.agent_wake`, recorded through `record_runtime_fact`,
//!   which commits FULL for Runtime scope. So a recorded `sending` is durable before the write. A
//!   row records at most `prepared`, the first `deferred`, `sending` and its result; a later reason
//!   goes only to the detail file and this memory.
//! - **One write.** A transport calls `before_write` immediately before its first possible write.
//!   Once it returned Ok the row never writes again: after a restart `sending` is `unknown`, never
//!   resent.
//! - **Isolation (#419).** Every error of the worker is `Operation` at `contain_failure`; only a
//!   real ledger failure stays `Global`, as it already is. A delivery failure is a non-fatal
//!   `runtime.failed` through typed setters, with system links and no instance link, so it reaches
//!   no ladder, fence or pause. A panic is caught, one record per backoff. A thread that cannot
//!   start turns the feature off and says so. Scheduling is never touched and never read.

use super::failure_scope::Contained;
use super::*;
use crate::agent_wake::body::{BodyInput, render_body};
use crate::agent_wake::rows::{WAKE_FACT_KEY, WaitReason, WakeRow, kept_rows, wake_id_bytes};
use crate::agent_wake::summary::{kind_word, render_message};
use actingcommand_agent_forward::channels::{MAX_CHANNEL_CHARS, read_registry};
use actingcommand_agent_forward::{
    AgentForward, DeferReason, DeliveryProblem, ForwardMessage, ForwardOutcome, IntentFailed,
    SendLimits,
};
use actingcommand_contract::codes::SanitizationCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    AgentDeliveryProblem, AgentDeliveryState, AgentWakeQuiet, AgentWakeStatus, AgentWakeTestQueued,
    AgentWakeWorkerState,
};
use actingcommand_runtime_config::agent_wake::{KIND_DELIVERY_TEST, body_folder_text};
use actingcommand_runtime_config::{AgentWakeConfig, EffectiveConfig};
use serde_json::json;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::PoisonError;

/// How often the worker looks whether it stops and whether anything waited. Its thread only.
const WORKER_POLL: Duration = Duration::from_millis(200);
/// With nothing queued and no open row, how seldom an iteration reads the configuration.
const IDLE_PASS_INTERVAL_MS: u64 = 1_000;
/// The sweep runs after each wake and at least this often.
const SWEEP_INTERVAL_MS: u64 = 60 * 60 * 1_000;
/// One day, for retention and the status' counts. Until AGT-2 brings the host's local day a day
/// is a UTC day.
const DAY_MS: u64 = 24 * 60 * 60 * 1_000;
/// The longest client detail a delivery detail file keeps.
const MAX_DETAIL_BYTES: usize = 4_096;
/// The `schema` of a delivery detail file.
const DETAIL_SCHEMA: &str = "actingcommand.agent-wake-delivery.v1";
/// A row's most collisions on a file name: the message floor counts a `-nn` suffix.
const MAX_FILE_SUFFIX: u32 = 99;
/// `MAX_RUNTIME_FACT_RECORD_ROWS`: more open rows than this cannot be recorded.
const MAX_ROWS: usize = 256;
/// The wait after a failed iteration when the section could not be read at all.
const BACKOFF_WITHOUT_SECTION_MS: u64 = 60_000;

/// A person's test wake, waiting for the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TestWake {
    pub(super) wake_id: String,
    pub(super) channel: String,
    pub(super) instance_alias: Option<String>,
}

/// What the bounded queue holds. AGT-2's tap adds the committed records it takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum WakeCandidate {
    Test(TestWake),
}

/// The bounded queue between the append path and the worker: memory only, under its own leaf lock,
/// and a full queue drops and counts, so the append path never waits for it. A panic elsewhere
/// cannot poison it into a failure of the append path.
pub(super) struct CandidateQueue {
    capacity: usize,
    items: Mutex<VecDeque<WakeCandidate>>,
    dropped: AtomicU64,
}

impl CandidateQueue {
    fn new(capacity: u64) -> Self {
        Self {
            capacity: usize::try_from(capacity).unwrap_or(usize::MAX),
            items: Mutex::new(VecDeque::new()),
            dropped: AtomicU64::new(0),
        }
    }

    fn items(&self) -> MutexGuard<'_, VecDeque<WakeCandidate>> {
        self.items.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes `candidate`, or drops and counts it when the queue is full.
    pub(super) fn offer(&self, candidate: WakeCandidate) -> bool {
        let mut items = self.items();
        if items.len() >= self.capacity {
            self.dropped.fetch_add(1, Ordering::AcqRel);
            return false;
        }
        items.push_back(candidate);
        true
    }

    fn drain(&self) -> Vec<WakeCandidate> {
        self.items().drain(..).collect()
    }

    fn is_empty(&self) -> bool {
        self.items().is_empty()
    }

    fn take_dropped(&self) -> u64 {
        self.dropped.swap(0, Ordering::AcqRel)
    }
}

/// What the worker knows. Only the worker changes the rows; every lock is brief and none is held
/// over a record or a send.
#[derive(Default)]
struct WakeMemory {
    /// The rows of `host.agent_wake`, oldest first.
    rows: Vec<WakeRow>,
    /// When each deferred row is tried again; absent after a restart, so it is tried at once.
    next_try_unix_ms: BTreeMap<String, u64>,
    /// When each channel last received a write.
    last_send_unix_ms: BTreeMap<String, u64>,
    worker: Option<AgentWakeWorkerState>,
    quiet: Option<AgentWakeQuiet>,
    registry_unreadable: bool,
    /// The UTC day `suppressed_today` counts, and its count.
    suppressed_day: u64,
    suppressed_today: u32,
    last_sweep_unix_ms: Option<u64>,
    sweep_requested: bool,
    last_idle_pass_unix_ms: u64,
    backoff_ms: Option<u64>,
    backoff_until_unix_ms: Option<u64>,
    /// `delivery.panic_backoff_ms` and `panic_backoff_max_ms` as last read.
    backoff_bounds: Option<(u64, u64)>,
    /// A detail file that could not be written is said once, until one is written again.
    detail_failure_reported: bool,
}

/// The host's part of the feature.
pub(super) struct AgentWakeShared {
    state_root: PathBuf,
    /// `Some` exactly when the start's configuration carries a readable section.
    queue: Option<CandidateQueue>,
    forward: Arc<dyn AgentForward>,
    memory: Mutex<WakeMemory>,
    thread: Mutex<Option<JoinHandle<RuntimeHostResult<()>>>>,
    /// Test only: no thread starts; a test runs each iteration itself.
    #[cfg(test)]
    pub(super) worker_held_for_test: AtomicBool,
    /// Test only: the next intent is refused, as a `sending` that could not be recorded.
    #[cfg(test)]
    pub(super) refuse_intent_for_test: AtomicBool,
    /// Test only: how many records the rows took.
    #[cfg(test)]
    pub(super) rows_recorded_for_test: AtomicU64,
}

impl AgentWakeShared {
    /// Built with the host's shared state, before the start writes anything.
    pub(super) fn new(
        manifest: Option<&actingcommand_contract::RuntimeConfigManifest>,
        state_root: &Path,
        forward: Arc<dyn AgentForward>,
    ) -> Self {
        let queue = manifest
            .and_then(|manifest| EffectiveConfig::from_manifest(manifest).ok())
            .and_then(|effective| effective.agent_wake().ok().flatten())
            .map(|section| CandidateQueue::new(section.delivery.queue_capacity));
        Self {
            state_root: state_root.to_path_buf(),
            queue,
            forward,
            memory: Mutex::new(WakeMemory::default()),
            thread: Mutex::new(None),
            #[cfg(test)]
            worker_held_for_test: AtomicBool::new(false),
            #[cfg(test)]
            refuse_intent_for_test: AtomicBool::new(false),
            #[cfg(test)]
            rows_recorded_for_test: AtomicU64::new(0),
        }
    }

    fn memory(&self) -> MutexGuard<'_, WakeMemory> {
        self.memory.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn folder(&self, dir: &str) -> PathBuf {
        self.state_root.join(dir)
    }
}

/// The closed problem a transport names, as the record's vocabulary spells it.
const fn delivery_problem(problem: DeliveryProblem) -> AgentDeliveryProblem {
    match problem {
        DeliveryProblem::ProtocolChanged => AgentDeliveryProblem::ProtocolChanged,
        DeliveryProblem::Rejected => AgentDeliveryProblem::Rejected,
        DeliveryProblem::OwnerMismatch => AgentDeliveryProblem::OwnerMismatch,
        DeliveryProblem::WriteTimedOut => AgentDeliveryProblem::WriteTimedOut,
        DeliveryProblem::WriteFailed => AgentDeliveryProblem::WriteFailed,
        DeliveryProblem::ReceiptMismatch => AgentDeliveryProblem::ReceiptMismatch,
        DeliveryProblem::HelperFailed => AgentDeliveryProblem::HelperFailed,
        DeliveryProblem::ChannelUnknown => AgentDeliveryProblem::ChannelUnknown,
    }
}

/// A channel name as the registry takes it: 1 to 16 Unicode scalars with no control character.
fn channel_shape(name: &str) -> bool {
    (1..=MAX_CHANNEL_CHARS).contains(&name.chars().count()) && !name.chars().any(char::is_control)
}

/// A worker's failure: `Operation` by its tag, its values through the key table's typed setters so
/// the record cannot be refused for a value.
fn wake_error(
    code: HostCode,
    row: Option<&WakeRow>,
    problem: Option<AgentDeliveryProblem>,
) -> RuntimeHostError {
    let row = row.cloned();
    RuntimeHostError::request(
        code,
        HostLocation::AgentWakeWorker,
        RuntimeErrorCode::InvalidRequest,
    )
    .with_outcome_values(move |outcome: Outcome| {
        let mut outcome = outcome;
        if let Some(row) = &row {
            if channel_shape(&row.channel) {
                outcome = outcome.agent_channel(row.channel.as_str());
            }
            outcome = outcome.agent_delivery_state(row.state);
            if let Some(alias) = &row.instance_alias {
                outcome = outcome.alias(alias.as_str());
            }
            if let Some(file) = &row.body_file {
                outcome = outcome.file(file.as_str());
            }
        }
        if let Some(problem) = problem {
            outcome = outcome.agent_delivery_problem(problem);
        }
        outcome
    })
    .with_failure_stage(HostLocation::AgentWakeWorker)
}

/// The rows the fact keeps: every open row, and the newest closed ones up to `history_rows`.
fn trim_rows(rows: &mut Vec<WakeRow>, history_rows: usize) {
    while rows.len() > history_rows {
        match rows.iter().position(|row| !row.state.is_open()) {
            Some(oldest_closed) => {
                rows.remove(oldest_closed);
            }
            None if rows.len() > MAX_ROWS => {
                rows.remove(0);
            }
            None => break,
        }
    }
}

/// `.part`, flush, rename.
fn write_file_atomically(path: &Path, text: &str) -> std::io::Result<()> {
    let part = path.with_extension("part");
    let mut file = fs::File::create(&part)?;
    file.write_all(text.as_bytes())?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    fs::rename(&part, path)
}

/// A free name for a body at `unix_ms`: `w<unix_ms>.json`, then `-2`, `-3` (one worker, so an
/// existence check is enough).
fn free_body_name(folder: &Path, unix_ms: u64) -> Option<String> {
    (1..=MAX_FILE_SUFFIX)
        .map(|suffix| match suffix {
            1 => format!("w{unix_ms}.json"),
            suffix => format!("w{unix_ms}-{suffix}.json"),
        })
        .find(|name| !folder.join(name).exists())
}

/// The delivery detail file of the body `file`.
fn detail_name(file: &str) -> String {
    let stem = file.strip_suffix(".json").unwrap_or(file);
    format!("{stem}.delivery.json")
}

/// `(stem, unix_ms)` of a wake file's name: `w<unix_ms>[-n].json` or its `.delivery.json`. Nothing
/// else in the folder is the sweep's.
fn wake_file(name: &str) -> Option<(&str, u64)> {
    let stem = name
        .strip_suffix(".delivery.json")
        .or_else(|| name.strip_suffix(".json"))?;
    let rest = stem.strip_prefix('w')?;
    let digits = rest.split('-').next()?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((stem, digits.parse().ok()?))
}

/// `detail` within its bound, ending on a character boundary.
fn bounded_detail(detail: &str) -> &str {
    if detail.len() <= MAX_DETAIL_BYTES {
        return detail;
    }
    let mut end = MAX_DETAIL_BYTES;
    while !detail.is_char_boundary(end) {
        end -= 1;
    }
    &detail[..end]
}

/// What became of one send, as the row takes it.
enum SendEnd {
    Deferred(WaitReason),
    Delivered(String),
    Failed(AgentDeliveryProblem),
    Unknown(AgentDeliveryProblem),
}

impl HostShared {
    fn wake_queue(&self) -> Option<&CandidateQueue> {
        self.agent_wake.queue.as_ref()
    }

    /// The section in effect, `None` without one. A section that does not read is the operation's
    /// failure.
    fn agent_wake_config(&self) -> RuntimeHostResult<Option<AgentWakeConfig>> {
        let effective = lock(&self.runtime_config, HostLocation::AgentWakeWorker)?;
        effective.agent_wake().map_err(|failure| {
            RuntimeHostError::request(
                failure.failure_code(),
                HostLocation::AgentWakeWorker,
                RuntimeErrorCode::InvalidRequest,
            )
            .with_outcome_values(|outcome| failure.with_values(outcome))
        })
    }

    /// One non-fatal `runtime.failed` with system links and no instance link. Only a real ledger
    /// failure comes back.
    fn report_wake_failure(&self, error: &RuntimeHostError) -> RuntimeHostResult<()> {
        self.append_lifecycle_failure(
            RuntimeLifecycleFailureStage::OperationCleanup,
            RuntimeLifecycleFailure::Host(error),
            self.events.system_links()?,
            None,
        )
    }

    /// Records `row` into `host.agent_wake`: the list as the worker keeps it, observed strictly
    /// after the previous record, so `runtime_fact_stale` cannot refuse a change.
    fn store_wake_row(&self, row: WakeRow, history_rows: u64) -> RuntimeHostResult<()> {
        let mut rows = self.agent_wake.memory().rows.clone();
        match rows.iter_mut().find(|kept| kept.wake_id == row.wake_id) {
            Some(kept) => *kept = row,
            None => rows.push(row),
        }
        trim_rows(
            &mut rows,
            usize::try_from(history_rows).unwrap_or(usize::MAX),
        );
        let now = self.runtime_clock_sample()?.unix_ms;
        let previous = lock(&self.runtime_facts, HostLocation::AgentWakeWorker)?
            .get(&RuntimeFactScope::Runtime, WAKE_FACT_KEY)
            .map(|record| record.observed_at_unix_ms);
        self.record_runtime_fact(RuntimeFactRecord {
            scope: RuntimeFactScope::Runtime,
            key: WAKE_FACT_KEY.to_owned(),
            value: ContractFactValue::RecordList(rows.iter().map(WakeRow::fields).collect()),
            observed_at_unix_ms: previous
                .map_or(now, |previous| now.max(previous.saturating_add(1))),
            source: OriginModule::Runtime,
            ttl_ms: None,
        })?;
        #[cfg(test)]
        self.agent_wake
            .rows_recorded_for_test
            .fetch_add(1, Ordering::AcqRel);
        self.agent_wake.memory().rows = rows;
        Ok(())
    }

    /// The start's read of `host.agent_wake`, after the runtime facts recovered and before the
    /// worker runs. A row left in `sending` may have been written: it is `unknown`, loud, and never
    /// sent again. A record this build does not read is said once and the start goes on with no
    /// rows; the next write replaces it. Nothing here stops a start but a real ledger failure.
    pub(super) fn restore_agent_wake(&self) -> RuntimeHostResult<()> {
        let config = match self.agent_wake_config() {
            Ok(Some(config)) => config,
            Ok(None) => return Ok(()),
            Err(error) => {
                self.agent_wake.memory().worker = Some(AgentWakeWorkerState::Unavailable);
                return self.report_wake_failure(&error);
            }
        };
        let recorded = lock(&self.runtime_facts, HostLocation::AgentWakeRestore)?
            .get(&RuntimeFactScope::Runtime, WAKE_FACT_KEY)
            .cloned();
        let rows = match recorded {
            None => Vec::new(),
            Some(record) => match kept_rows(&record) {
                Some(rows) => rows,
                None => {
                    let error = RuntimeHostError::request(
                        SanitizationCode::InvalidRuntimeStateFact,
                        HostLocation::AgentWakeRestore,
                        RuntimeErrorCode::InvalidRequest,
                    );
                    self.report_wake_failure(&error)?;
                    Vec::new()
                }
            },
        };
        let interrupted = rows
            .iter()
            .filter(|row| row.state == AgentDeliveryState::Sending)
            .cloned()
            .collect::<Vec<_>>();
        self.agent_wake.memory().rows = rows;
        let now = self.runtime_clock_sample()?.unix_ms;
        for mut row in interrupted {
            row.state = AgentDeliveryState::Unknown;
            row.problem = Some(AgentDeliveryProblem::InterruptedByStop.as_str().to_owned());
            row.updated_at_unix_ms = now;
            self.close_wake_row(
                row,
                HostCode::AgentWakeDeliveryFailed,
                Some(AgentDeliveryProblem::InterruptedByStop),
                &config,
            )?;
        }
        Ok(())
    }

    /// Ends `row` in a loud state: its record, its detail, and one `runtime.failed`. A record the
    /// store refuses is the operation's failure and the loud record still goes out; only a real
    /// ledger failure comes back.
    fn close_wake_row(
        &self,
        row: WakeRow,
        code: HostCode,
        problem: Option<AgentDeliveryProblem>,
        config: &AgentWakeConfig,
    ) -> RuntimeHostResult<()> {
        let error = wake_error(code, Some(&row), problem);
        self.write_wake_detail(&row, &config.body.dir, None, "")?;
        match self.store_wake_row(row, config.delivery.history_rows) {
            Ok(()) => {}
            Err(store) if store.is_fatal() => return Err(store),
            Err(store) => self.report_wake_failure(&store)?,
        }
        self.agent_wake.memory().sweep_requested = true;
        self.report_wake_failure(&error)
    }

    /// The delivery detail file of `row`, rewritten at each state and retry: this is where a person
    /// reads what went wrong. Its failure is said once and the delivery goes on.
    fn write_wake_detail(
        &self,
        row: &WakeRow,
        dir: &str,
        message: Option<&str>,
        detail: &str,
    ) -> RuntimeHostResult<()> {
        let Some(file) = &row.body_file else {
            return Ok(());
        };
        let mut text = json!({
            "schema": DETAIL_SCHEMA,
            "wake_id": row.wake_id,
            "channel": row.channel,
            "state": row.state.as_str(),
            "proof": row.proof,
            "problem": row.problem,
            "message_text": message,
            "detail": bounded_detail(detail),
            "updated_at_unix_ms": row.updated_at_unix_ms,
        });
        text["reason"] = json!(row.reason.map(WaitReason::as_str));
        let text = text.to_string();
        let path = self.agent_wake.folder(dir).join(detail_name(file));
        match write_file_atomically(&path, &text) {
            Ok(()) => {
                self.agent_wake.memory().detail_failure_reported = false;
                Ok(())
            }
            Err(_) => {
                let reported =
                    std::mem::replace(&mut self.agent_wake.memory().detail_failure_reported, true);
                if reported {
                    return Ok(());
                }
                self.report_wake_failure(&wake_error(
                    HostCode::AgentWakeDeliveryFailed,
                    Some(row),
                    Some(AgentDeliveryProblem::DetailUnwritable),
                ))
            }
        }
    }

    /// Why no message goes out now, though the feature is on: the hold, then a window. The
    /// judgement is the blackout entries' own, over the section's list alone.
    fn agent_wake_quiet(
        &self,
        config: &AgentWakeConfig,
    ) -> RuntimeHostResult<Option<AgentWakeQuiet>> {
        let sample = self.clock.sample()?;
        if config.quiet.hold_until_unix_ms > sample.unix_ms {
            return Ok(Some(AgentWakeQuiet::Hold));
        }
        if config.quiet.windows.is_empty() {
            return Ok(None);
        }
        let host = super::blackout_silence::judged(config.quiet.windows.clone());
        let instances = BTreeMap::new();
        let sources = actingcommand_policy::BlackoutSources {
            host: &host,
            instances: &instances,
            catalog: &[],
        };
        let instance = InstanceSnapshot {
            instance_id: String::new(),
            server_id: String::new(),
            game_id: String::new(),
            host_id: String::new(),
            available: true,
            capability_operation_ids: Vec::new(),
            preferred_task_ids: Vec::new(),
        };
        let time = EvaluationTime {
            unix_ms: sample.unix_ms,
            monotonic_ms: sample.monotonic_ms,
        };
        let covering =
            actingcommand_policy::blackout_at(&sources, &instance, time).map_err(|_| {
                wake_error(
                    HostCode::AgentWakeDeliveryFailed,
                    None,
                    Some(AgentDeliveryProblem::QuietUnreadable),
                )
            })?;
        Ok(covering.map(|_| AgentWakeQuiet::Window))
    }

    /// Counts candidates dropped now into today's count.
    fn count_suppressed(&self, count: u64, now_unix_ms: u64) {
        let today = now_unix_ms / DAY_MS;
        let mut memory = self.agent_wake.memory();
        if memory.suppressed_day != today {
            memory.suppressed_day = today;
            memory.suppressed_today = 0;
        }
        let count = u32::try_from(count).unwrap_or(u32::MAX);
        memory.suppressed_today = memory.suppressed_today.saturating_add(count);
    }

    /// A candidate becomes a body file and a row `prepared`; a body that cannot be written ends the
    /// row `body_failed`, loudly, and nothing is sent.
    fn prepare_wake(
        &self,
        wake: TestWake,
        config: &AgentWakeConfig,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<()> {
        let mut row = WakeRow {
            wake_id: wake.wake_id,
            kind: KIND_DELIVERY_TEST.to_owned(),
            rule: None,
            test: true,
            instance_alias: wake.instance_alias,
            code: None,
            channel: wake.channel,
            created_at_unix_ms: now_unix_ms,
            updated_at_unix_ms: now_unix_ms,
            state: AgentDeliveryState::Prepared,
            reason: None,
            proof: None,
            problem: None,
            body_file: None,
            body_sha256: None,
            body_chars: None,
            message_chars: None,
            alias_cut: false,
            trigger_event_id: None,
            correlation_id: None,
        };
        let rendered = render_body(
            &BodyInput {
                wake_id: &row.wake_id,
                kind: &row.kind,
                created_at_unix_ms: now_unix_ms,
                instance_alias: row.instance_alias.as_deref(),
                rule: None,
                channel: &row.channel,
                test: true,
                trigger: None,
                related: Vec::new(),
                next_reads: Vec::new(),
            },
            usize::try_from(config.body.max_bytes).unwrap_or(usize::MAX),
        );
        let folder = self.agent_wake.folder(&config.body.dir);
        let written = rendered.ok().and_then(|body| {
            fs::create_dir_all(&folder).ok()?;
            let file = free_body_name(&folder, now_unix_ms)?;
            write_file_atomically(&folder.join(&file), &body.text).ok()?;
            Some((file, body))
        });
        match written {
            Some((file, body)) => {
                row.body_file = Some(file);
                row.body_sha256 = Some(body.sha256);
                row.body_chars = u64::try_from(body.chars).ok();
                self.store_wake_row(row, config.delivery.history_rows)
            }
            None => {
                row.state = AgentDeliveryState::BodyFailed;
                row.problem = Some(AgentDeliveryProblem::BodyUnwritable.as_str().to_owned());
                self.close_wake_row(
                    row,
                    HostCode::AgentWakeDeliveryFailed,
                    Some(AgentDeliveryProblem::BodyUnwritable),
                    config,
                )
            }
        }
    }

    /// `row` waits. The first `deferred` is recorded; a later reason goes only to the detail file
    /// and this memory.
    fn defer_wake_row(
        &self,
        mut row: WakeRow,
        reason: WaitReason,
        detail: &str,
        config: &AgentWakeConfig,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<()> {
        let first = row.state != AgentDeliveryState::Deferred;
        row.state = AgentDeliveryState::Deferred;
        row.reason = Some(reason);
        row.updated_at_unix_ms = now_unix_ms;
        self.write_wake_detail(&row, &config.body.dir, None, detail)?;
        {
            let mut memory = self.agent_wake.memory();
            memory.next_try_unix_ms.insert(
                row.wake_id.clone(),
                now_unix_ms.saturating_add(config.delivery.retry_interval_ms),
            );
            if !first
                && let Some(kept) = memory
                    .rows
                    .iter_mut()
                    .find(|kept| kept.wake_id == row.wake_id)
            {
                *kept = row.clone();
            }
        }
        if first {
            self.store_wake_row(row, config.delivery.history_rows)?;
        }
        Ok(())
    }

    /// One try of `row`: the registry, the message, one send, the result.
    fn attempt_wake(
        &self,
        mut row: WakeRow,
        config: &AgentWakeConfig,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<()> {
        let Some(word) = kind_word(&row.kind) else {
            row.state = AgentDeliveryState::Failed;
            row.problem = Some(AgentDeliveryProblem::KindUnknown.as_str().to_owned());
            row.updated_at_unix_ms = now_unix_ms;
            return self.close_wake_row(
                row,
                HostCode::AgentWakeDeliveryFailed,
                Some(AgentDeliveryProblem::KindUnknown),
                config,
            );
        };
        // A missing or unreadable registry waits; a name a readable registry does not list ends
        // the row at once. The file is read at each try, so a channel added needs no restart.
        let registry = read_registry(&self.agent_wake.state_root).ok().flatten();
        self.agent_wake.memory().registry_unreadable = registry.is_none();
        let Some(registry) = registry else {
            return self.defer_wake_row(
                row,
                WaitReason::Forward(DeferReason::RegistryUnreadable),
                "the channel registry is missing or cannot be read",
                config,
                now_unix_ms,
            );
        };
        if registry.channel(&row.channel).is_none() {
            row.state = AgentDeliveryState::Failed;
            row.problem = Some(AgentDeliveryProblem::ChannelUnknown.as_str().to_owned());
            row.updated_at_unix_ms = now_unix_ms;
            return self.close_wake_row(
                row,
                HostCode::AgentWakeChannelUnknown,
                Some(AgentDeliveryProblem::ChannelUnknown),
                config,
            );
        }
        let Some(file) = row.body_file.clone() else {
            // A restored open row always names its body; a row without one cannot be sent.
            row.state = AgentDeliveryState::BodyFailed;
            row.problem = Some(AgentDeliveryProblem::BodyUnwritable.as_str().to_owned());
            row.updated_at_unix_ms = now_unix_ms;
            return self.close_wake_row(
                row,
                HostCode::AgentWakeDeliveryFailed,
                Some(AgentDeliveryProblem::BodyUnwritable),
                config,
            );
        };
        let body_path = format!(
            "{}{file}",
            body_folder_text(&self.agent_wake.state_root, &config.body.dir)
        );
        let max_chars = usize::try_from(config.summary_max_chars).unwrap_or(usize::MAX);
        let message = match render_message(
            &row.channel,
            word,
            row.instance_alias.as_deref(),
            &body_path,
            max_chars,
        ) {
            Ok(message) => message,
            Err(_) => {
                // A limit set below the channel's floor while the Runtime runs: never a cut path.
                row.state = AgentDeliveryState::Failed;
                row.problem = Some(AgentDeliveryProblem::MessageTooLong.as_str().to_owned());
                row.updated_at_unix_ms = now_unix_ms;
                return self.close_wake_row(
                    row,
                    HostCode::AgentWakeMessageTooLong,
                    Some(AgentDeliveryProblem::MessageTooLong),
                    config,
                );
            }
        };
        row.message_chars = u64::try_from(message.chars).ok();
        row.alias_cut = message.alias_cut;
        let forward_message = ForwardMessage {
            id: wake_id_bytes(&row.wake_id).unwrap_or([0_u8; 16]),
            text: message.text.clone(),
        };
        let stopped = || self.fatal.is_shutdown_requested();
        let limits = SendLimits {
            deadline: Instant::now() + Duration::from_millis(config.delivery.send_deadline_ms),
            stopped: &stopped,
        };
        let mut committed = false;
        let mut refused_for_stop = false;
        let mut intent_error: Option<RuntimeHostError> = None;
        let forward = Arc::clone(&self.agent_wake.forward);
        let outcome = {
            let intent = &row;
            let mut before_write = || -> Result<(), IntentFailed> {
                // No new `sending` after stop.
                if self.fatal.is_shutdown_requested() {
                    refused_for_stop = true;
                    return Err(IntentFailed);
                }
                #[cfg(test)]
                if self
                    .agent_wake
                    .refuse_intent_for_test
                    .swap(false, Ordering::AcqRel)
                {
                    intent_error = Some(wake_error(
                        HostCode::AgentWakeDeliveryFailed,
                        Some(intent),
                        Some(AgentDeliveryProblem::IntentRefused),
                    ));
                    return Err(IntentFailed);
                }
                let mut sending = intent.clone();
                sending.state = AgentDeliveryState::Sending;
                sending.reason = None;
                sending.updated_at_unix_ms = now_unix_ms;
                match self.store_wake_row(sending, config.delivery.history_rows) {
                    Ok(()) => {
                        committed = true;
                        Ok(())
                    }
                    Err(error) => {
                        intent_error = Some(error);
                        Err(IntentFailed)
                    }
                }
            };
            forward.send(&row.channel, &forward_message, &limits, &mut before_write)
        };
        // The intent was refused: no write happened and the row stays as it was. The next start or
        // the next iteration after the failure's backoff tries it again.
        if refused_for_stop {
            return Ok(());
        }
        if let Some(error) = intent_error {
            return Err(error);
        }
        let (end, detail) = match outcome {
            // A transport that committed its intent and then says no write happened breaks its
            // rule; the row cannot be sent again.
            ForwardOutcome::Deferred { detail, .. } if committed => {
                (SendEnd::Unknown(AgentDeliveryProblem::WriteFailed), detail)
            }
            ForwardOutcome::Deferred { reason, detail } => {
                (SendEnd::Deferred(WaitReason::Forward(reason)), detail)
            }
            ForwardOutcome::Delivered { proof, detail } => {
                (SendEnd::Delivered(proof.as_str().to_owned()), detail)
            }
            ForwardOutcome::Failed { problem, detail } => {
                (SendEnd::Failed(delivery_problem(problem)), detail)
            }
            ForwardOutcome::Unknown { problem, detail } => {
                (SendEnd::Unknown(delivery_problem(problem)), detail)
            }
        };
        let now_unix_ms = self.runtime_clock_sample()?.unix_ms;
        if committed {
            self.agent_wake
                .memory()
                .last_send_unix_ms
                .insert(row.channel.clone(), now_unix_ms);
        }
        row.reason = None;
        row.updated_at_unix_ms = now_unix_ms;
        match end {
            SendEnd::Deferred(reason) => {
                self.defer_wake_row(row, reason, &detail, config, now_unix_ms)
            }
            SendEnd::Delivered(proof) => {
                row.state = AgentDeliveryState::Delivered;
                row.proof = Some(proof);
                self.write_wake_detail(
                    &row,
                    &config.body.dir,
                    Some(message.text.as_str()),
                    &detail,
                )?;
                self.store_wake_row(row, config.delivery.history_rows)?;
                self.agent_wake.memory().sweep_requested = true;
                Ok(())
            }
            SendEnd::Failed(problem) => {
                row.state = AgentDeliveryState::Failed;
                row.problem = Some(problem.as_str().to_owned());
                self.write_wake_detail(
                    &row,
                    &config.body.dir,
                    Some(message.text.as_str()),
                    &detail,
                )?;
                self.close_wake_row_keeping_detail(row, problem, config)
            }
            SendEnd::Unknown(problem) => {
                row.state = AgentDeliveryState::Unknown;
                row.problem = Some(problem.as_str().to_owned());
                self.write_wake_detail(
                    &row,
                    &config.body.dir,
                    Some(message.text.as_str()),
                    &detail,
                )?;
                self.close_wake_row_keeping_detail(row, problem, config)
            }
        }
    }

    /// [`Self::close_wake_row`] of a row whose detail file already holds the client's words.
    fn close_wake_row_keeping_detail(
        &self,
        row: WakeRow,
        problem: AgentDeliveryProblem,
        config: &AgentWakeConfig,
    ) -> RuntimeHostResult<()> {
        let error = wake_error(HostCode::AgentWakeDeliveryFailed, Some(&row), Some(problem));
        match self.store_wake_row(row, config.delivery.history_rows) {
            Ok(()) => {}
            Err(store) if store.is_fatal() => return Err(store),
            Err(store) => self.report_wake_failure(&store)?,
        }
        self.agent_wake.memory().sweep_requested = true;
        self.report_wake_failure(&error)
    }

    /// The rows whose send is due, oldest first, one per channel. The give-up clock is read only
    /// here, so a quiet time causes no expiry noise.
    fn send_due_wakes(&self, config: &AgentWakeConfig) -> RuntimeHostResult<()> {
        let rows = self.agent_wake.memory().rows.clone();
        let mut seen = BTreeSet::new();
        for row in rows {
            if !matches!(
                row.state,
                AgentDeliveryState::Prepared | AgentDeliveryState::Deferred
            ) || !seen.insert(row.channel.clone())
            {
                continue;
            }
            if self.fatal.is_shutdown_requested() {
                return Ok(());
            }
            let now_unix_ms = self.runtime_clock_sample()?.unix_ms;
            let (retry_at, last_send) = {
                let memory = self.agent_wake.memory();
                (
                    memory.next_try_unix_ms.get(&row.wake_id).copied(),
                    memory.last_send_unix_ms.get(&row.channel).copied(),
                )
            };
            let retry_due = retry_at.is_none_or(|at| now_unix_ms >= at);
            let gap_passed = last_send.is_none_or(|last| {
                now_unix_ms.saturating_sub(last) >= config.delivery.channel_gap_ms
            });
            if !retry_due || !gap_passed {
                continue;
            }
            if now_unix_ms.saturating_sub(row.created_at_unix_ms) >= config.delivery.give_up_ms {
                let mut expired = row;
                expired.state = AgentDeliveryState::Expired;
                expired.updated_at_unix_ms = now_unix_ms;
                self.close_wake_row(expired, HostCode::AgentWakeDeliveryFailed, None, config)?;
                continue;
            }
            self.attempt_wake(row, config, now_unix_ms)?;
        }
        Ok(())
    }

    /// With the feature off a row in `sending` finishes; every other open row stays `deferred`,
    /// reason `disabled`, recorded once.
    fn hold_wakes_while_disabled(
        &self,
        config: &AgentWakeConfig,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<()> {
        let rows = self.agent_wake.memory().rows.clone();
        for row in rows {
            let already = row.reason == Some(WaitReason::Disabled);
            if matches!(
                row.state,
                AgentDeliveryState::Prepared | AgentDeliveryState::Deferred
            ) && !already
            {
                self.defer_wake_row(row, WaitReason::Disabled, "", config, now_unix_ms)?;
            }
        }
        Ok(())
    }

    /// The retention sweep, after each wake and hourly: by age, then by count, oldest first. It
    /// never removes an open row's files, touches only the worker's own file names, and writes no
    /// ledger record but a failure's.
    fn sweep_wake_files(
        &self,
        config: &AgentWakeConfig,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<()> {
        let open = {
            let mut memory = self.agent_wake.memory();
            let due = memory.sweep_requested
                || memory
                    .last_sweep_unix_ms
                    .is_none_or(|last| now_unix_ms.saturating_sub(last) >= SWEEP_INTERVAL_MS);
            if !due {
                return Ok(());
            }
            memory.sweep_requested = false;
            memory.last_sweep_unix_ms = Some(now_unix_ms);
            memory
                .rows
                .iter()
                .filter(|row| row.state.is_open())
                .filter_map(|row| row.body_file.as_deref()?.strip_suffix(".json"))
                .map(str::to_owned)
                .collect::<BTreeSet<_>>()
        };
        let folder = self.agent_wake.folder(&config.body.dir);
        let entries = match fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => {
                return self.report_wake_failure(&wake_error(
                    HostCode::AgentWakeDeliveryFailed,
                    None,
                    Some(AgentDeliveryProblem::SweepFailed),
                ));
            }
        };
        let mut failed = false;
        let mut stems = BTreeMap::<String, (u64, Vec<PathBuf>)>::new();
        for entry in entries {
            let Ok(entry) = entry else {
                failed = true;
                continue;
            };
            let name = entry.file_name();
            let Some((stem, unix_ms)) = name.to_str().and_then(wake_file) else {
                continue;
            };
            stems
                .entry(stem.to_owned())
                .or_insert_with(|| (unix_ms, Vec::new()))
                .1
                .push(entry.path());
        }
        let retention_ms = config.body.retention_days.saturating_mul(DAY_MS);
        let max_files = usize::try_from(config.body.max_files).unwrap_or(usize::MAX);
        let mut remaining = stems.len();
        let mut closed = stems
            .iter()
            .filter(|(stem, _)| !open.contains(*stem))
            .map(|(stem, (unix_ms, paths))| (*unix_ms, stem.as_str(), paths))
            .collect::<Vec<_>>();
        closed.sort();
        for (unix_ms, _, paths) in closed {
            if now_unix_ms.saturating_sub(unix_ms) > retention_ms || remaining > max_files {
                for path in paths {
                    failed |= fs::remove_file(path).is_err();
                }
                remaining -= 1;
            }
        }
        if failed {
            return self.report_wake_failure(&wake_error(
                HostCode::AgentWakeDeliveryFailed,
                None,
                Some(AgentDeliveryProblem::SweepFailed),
            ));
        }
        Ok(())
    }

    /// One iteration: the waiting candidates, then what may go out, then the sweep.
    fn agent_wake_pass(&self) -> RuntimeHostResult<()> {
        let Some(queue) = self.wake_queue() else {
            return Ok(());
        };
        let Some(config) = self.agent_wake_config()? else {
            return Ok(());
        };
        let now_unix_ms = self.runtime_clock_sample()?.unix_ms;
        self.agent_wake.memory().backoff_bounds = Some((
            config.delivery.panic_backoff_ms,
            config.delivery.panic_backoff_max_ms,
        ));
        for candidate in queue.drain() {
            if !config.enabled {
                self.count_suppressed(1, now_unix_ms);
                continue;
            }
            let WakeCandidate::Test(wake) = candidate;
            self.prepare_wake(wake, &config, now_unix_ms)?;
        }
        let dropped = queue.take_dropped();
        if dropped > 0 {
            self.count_suppressed(dropped, now_unix_ms);
            self.report_wake_failure(&wake_error(
                HostCode::AgentWakeDeliveryFailed,
                None,
                Some(AgentDeliveryProblem::QueueFull),
            ))?;
        }
        let quiet = self.agent_wake_quiet(&config)?;
        self.agent_wake.memory().quiet = quiet;
        if !config.enabled {
            self.hold_wakes_while_disabled(&config, now_unix_ms)?;
        } else if quiet.is_none() {
            self.send_due_wakes(&config)?;
        }
        self.sweep_wake_files(&config, now_unix_ms)
    }

    /// One guarded iteration (design §7.5). An error is `Operation` at the one decision point and a
    /// panic is caught; either writes one `runtime.failed` and waits `panic_backoff_ms`, doubling to
    /// `panic_backoff_max_ms`, reset by a clean iteration. So a tight failure costs at most one
    /// record per backoff. Only a failure the Runtime does not survive comes back.
    pub(super) fn agent_wake_step(&self) -> RuntimeHostResult<()> {
        let now_unix_ms = self.runtime_clock_sample()?.unix_ms;
        let backing_off = self
            .agent_wake
            .memory()
            .backoff_until_unix_ms
            .is_some_and(|until| now_unix_ms < until);
        if backing_off {
            return Ok(());
        }
        let failure = match catch_unwind(AssertUnwindSafe(|| self.agent_wake_pass())) {
            Ok(Ok(())) => {
                let mut memory = self.agent_wake.memory();
                memory.backoff_ms = None;
                memory.backoff_until_unix_ms = None;
                if memory.worker == Some(AgentWakeWorkerState::BackingOff) {
                    memory.worker = Some(AgentWakeWorkerState::Running);
                }
                return Ok(());
            }
            Ok(Err(error)) => {
                let links = self.events.system_links()?;
                match self.contain_failure(error, None, links)? {
                    Contained::Latched(error) => return Err(error),
                    Contained::Fenced(error) | Contained::Failed(error) => error,
                }
            }
            Err(_) => wake_error(
                HostCode::AgentWakeDeliveryFailed,
                None,
                Some(AgentDeliveryProblem::WorkerPanicked),
            ),
        };
        self.report_wake_failure(&failure)?;
        let mut memory = self.agent_wake.memory();
        let (base, longest) = memory
            .backoff_bounds
            .unwrap_or((BACKOFF_WITHOUT_SECTION_MS, BACKOFF_WITHOUT_SECTION_MS));
        let wait = memory
            .backoff_ms
            .map_or(base, |previous| previous.saturating_mul(2))
            .min(longest);
        memory.backoff_ms = Some(wait);
        memory.backoff_until_unix_ms = Some(now_unix_ms.saturating_add(wait));
        memory.worker = Some(AgentWakeWorkerState::BackingOff);
        Ok(())
    }

    /// Whether the worker's thread has nothing to look at yet: nothing queued, no open row and the
    /// idle interval not over.
    fn agent_wake_idle(&self, now_unix_ms: u64) -> bool {
        let queued = self.wake_queue().is_some_and(|queue| !queue.is_empty());
        let mut memory = self.agent_wake.memory();
        let open = memory.rows.iter().any(|row| row.state.is_open());
        if queued
            || open
            || now_unix_ms.saturating_sub(memory.last_idle_pass_unix_ms) >= IDLE_PASS_INTERVAL_MS
        {
            memory.last_idle_pass_unix_ms = now_unix_ms;
            return false;
        }
        true
    }

    /// `Status.agent_wake`, from memory: no ledger read, absent with no section.
    pub(super) fn agent_wake_status(&self) -> RuntimeHostResult<Option<AgentWakeStatus>> {
        if self.wake_queue().is_none() {
            return Ok(None);
        }
        let enabled = lock(&self.runtime_config, HostLocation::AgentWakeWorker)?
            .value(actingcommand_runtime_config::agent_wake::ENABLED)
            .is_some_and(|value| *value == FactScalar::Boolean(true));
        let today = self.runtime_clock_sample()?.unix_ms / DAY_MS;
        let memory = self.agent_wake.memory();
        let count = |count: usize| u32::try_from(count).unwrap_or(u32::MAX);
        let failed_today = memory
            .rows
            .iter()
            .filter(|row| row.state.is_loud() && row.updated_at_unix_ms / DAY_MS == today)
            .count();
        let suppressed_today = if memory.suppressed_day == today {
            memory.suppressed_today
        } else {
            0
        };
        Ok(Some(AgentWakeStatus {
            enabled,
            worker: memory.worker.unwrap_or(AgentWakeWorkerState::Unavailable),
            quiet: memory.quiet,
            open: count(memory.rows.iter().filter(|row| row.state.is_open()).count()),
            last_wake_at_unix_ms: memory.rows.iter().map(|row| row.created_at_unix_ms).max(),
            failed_today: count(failed_today),
            suppressed_today,
            attention: failed_today > 0 || memory.registry_unreadable,
        }))
    }

    /// What `AgentWakeTest` checks before it queues: the section on, a channel a readable registry
    /// lists. A missing or unreadable registry takes the test, whose row then waits and ends
    /// loudly at give-up, as every row does.
    fn admit_agent_wake_test(
        &self,
        to: &str,
        instance_alias: Option<&str>,
    ) -> RuntimeHostResult<AgentWakeTestQueued> {
        let refused = |code: HostCode| {
            RuntimeHostError::request(
                code,
                HostLocation::AgentWakeTest,
                RuntimeErrorCode::InvalidRequest,
            )
        };
        let config = self.agent_wake_config()?;
        let (Some(queue), Some(config)) = (self.wake_queue(), config) else {
            return Err(refused(HostCode::AgentWakeDisabled));
        };
        if !config.enabled {
            self.count_suppressed(1, self.runtime_clock_sample()?.unix_ms);
            return Err(refused(HostCode::AgentWakeDisabled));
        }
        if !channel_shape(to) {
            return Err(refused(HostCode::AgentWakeChannelUnknown));
        }
        let channel = to.to_owned();
        if let Ok(Some(registry)) = read_registry(&self.agent_wake.state_root)
            && registry.channel(to).is_none()
        {
            return Err(refused(HostCode::AgentWakeChannelUnknown)
                .with_outcome_values(|outcome| outcome.agent_channel(to)));
        }
        let issued = self
            .events
            .issuer()
            .mint_agent_wake_id()
            .map_err(|_| runtime_identifier_error())?;
        let wake_id = crate::failure_identity::identifier_text(issued.transport());
        let offered = queue.offer(WakeCandidate::Test(TestWake {
            wake_id: wake_id.clone(),
            channel: channel.clone(),
            instance_alias: instance_alias.map(str::to_owned),
        }));
        if !offered {
            return Err(
                refused(HostCode::AgentWakeDeliveryFailed).with_outcome_values(|outcome| {
                    outcome
                        .agent_channel(to)
                        .agent_delivery_problem(AgentDeliveryProblem::QueueFull)
                }),
            );
        }
        Ok(AgentWakeTestQueued {
            wake_id,
            channel,
            quiet: self.agent_wake_quiet(&config)?,
        })
    }

    /// `AgentWakeTest`: recorded as a command, then queued for the worker. It touches no instance
    /// and no scheduling, and a refusal is the request's alone.
    pub(super) fn agent_wake_test(
        &self,
        original: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        to: &str,
        instance_alias: Option<&str>,
    ) -> Result<OperationSuccess, RequestFailure> {
        let links = self.append_scoped_client_command_intent(
            original,
            validated,
            None,
            EventAction::RuntimeAction,
            None,
        )?;
        match self.admit_agent_wake_test(to, instance_alias) {
            Ok(queued) => {
                let accepted = self.append_event(
                    EventSeverity::Info,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    links,
                    CommandPayloadDraft::validated(
                        EventAction::RuntimeAction,
                        EffectDisposition::Performed,
                        AuditInput::new(),
                    ),
                )?;
                Ok(OperationSuccess {
                    state: RuntimeReceiptState::Completed,
                    terminal: Some(terminal(&accepted)),
                    result: RuntimeResult::AgentWakeTestQueued { queued },
                    failure_outcome: None,
                })
            }
            Err(error) => {
                let rejected = self.append_event(
                    EventSeverity::Error,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    links,
                    CommandPayloadDraft::rejected_with_outcome(
                        EventAction::RuntimeAction,
                        DiagnosticCode::CommandRejected,
                        EffectDisposition::NotPerformed,
                        error.outcome(),
                        AuditInput::new(),
                    ),
                )?;
                Err(RequestFailure::request(
                    error,
                    RuntimeReceiptState::Denied,
                    Some(terminal(&rejected)),
                ))
            }
        }
    }
}

/// The worker's start, after everything else of the start: only with a section, and a thread that
/// cannot start turns the feature off and says so; it never stops the start (#419).
pub(super) fn start(shared: &Arc<HostShared>) -> RuntimeHostResult<()> {
    if shared.wake_queue().is_none() {
        return Ok(());
    }
    if shared.agent_wake.memory().worker == Some(AgentWakeWorkerState::Unavailable) {
        return Ok(());
    }
    shared.agent_wake.memory().worker = Some(AgentWakeWorkerState::Running);
    #[cfg(test)]
    if shared
        .agent_wake
        .worker_held_for_test
        .load(Ordering::Acquire)
    {
        return Ok(());
    }
    let worker_shared = Arc::clone(shared);
    match thread::Builder::new()
        .name("actingcommand-runtime-agent-wake".to_string())
        .spawn(move || worker_loop(&worker_shared))
    {
        Ok(thread) => {
            *lock(&shared.agent_wake.thread, HostLocation::JoinAgentWakeWorker)? = Some(thread);
            Ok(())
        }
        Err(_) => {
            shared.agent_wake.memory().worker = Some(AgentWakeWorkerState::Unavailable);
            shared.report_wake_failure(&wake_error(
                HostCode::AgentWakeDeliveryFailed,
                None,
                Some(AgentDeliveryProblem::WorkerSpawnFailed),
            ))
        }
    }
}

/// The worker: its iterations until the Runtime stops. Only a failure the Runtime does not survive
/// ends it early, and marks the Runtime.
fn worker_loop(shared: &HostShared) -> RuntimeHostResult<()> {
    while !shared.fatal.is_shutdown_requested() {
        thread::sleep(WORKER_POLL);
        if shared.fatal.is_shutdown_requested() {
            break;
        }
        let stepped = match shared.runtime_clock_sample() {
            Ok(sample) if shared.agent_wake_idle(sample.unix_ms) => Ok(()),
            Ok(_) => shared.agent_wake_step(),
            Err(error) => Err(error),
        };
        if let Err(error) = stepped {
            shared.fatal.mark(error.clone())?;
            return Err(error);
        }
    }
    Ok(())
}

/// Joins the worker, which ends with the shutdown request: a send under way stops through
/// `SendLimits::stopped` and its deadline.
pub(super) fn stop(shared: &HostShared) -> RuntimeHostResult<()> {
    let thread = lock(&shared.agent_wake.thread, HostLocation::JoinAgentWakeWorker)?.take();
    join_runtime_thread(thread, HostLocation::JoinAgentWakeWorker)
}

impl RuntimeHost {
    /// Test only: one guarded iteration, as the worker's thread runs it.
    #[cfg(test)]
    pub(crate) fn agent_wake_step_for_test(&self) -> RuntimeHostResult<()> {
        self.shared_ref(HostLocation::AgentWakeWorker)?
            .agent_wake_step()
    }

    /// Test only: the rows as the worker keeps them.
    #[cfg(test)]
    pub(crate) fn agent_wake_rows_for_test(&self) -> RuntimeHostResult<Vec<WakeRow>> {
        Ok(self
            .shared_ref(HostLocation::AgentWakeWorker)?
            .agent_wake
            .memory()
            .rows
            .clone())
    }

    /// Test only: how many records the rows took since the start.
    #[cfg(test)]
    pub(crate) fn agent_wake_rows_recorded_for_test(&self) -> RuntimeHostResult<u64> {
        Ok(self
            .shared_ref(HostLocation::AgentWakeWorker)?
            .agent_wake
            .rows_recorded_for_test
            .load(Ordering::Acquire))
    }

    /// Test only: the next intent is refused.
    #[cfg(test)]
    pub(crate) fn agent_wake_refuse_intent_for_test(&self) -> RuntimeHostResult<()> {
        self.shared_ref(HostLocation::AgentWakeWorker)?
            .agent_wake
            .refuse_intent_for_test
            .store(true, Ordering::Release);
        Ok(())
    }

    /// Test only: `candidate` offered to the queue, as the tap will; `None` without a queue.
    #[cfg(test)]
    pub(crate) fn agent_wake_offer_for_test(
        &self,
        wake_id: &str,
        channel: &str,
    ) -> RuntimeHostResult<Option<bool>> {
        Ok(self
            .shared_ref(HostLocation::AgentWakeWorker)?
            .wake_queue()
            .map(|queue| {
                queue.offer(WakeCandidate::Test(TestWake {
                    wake_id: wake_id.to_owned(),
                    channel: channel.to_owned(),
                    instance_alias: None,
                }))
            }))
    }

    /// Test only: whether the worker's thread runs.
    #[cfg(test)]
    pub(crate) fn agent_wake_thread_runs_for_test(&self) -> RuntimeHostResult<bool> {
        Ok(lock(
            &self
                .shared_ref(HostLocation::AgentWakeWorker)?
                .agent_wake
                .thread,
            HostLocation::AgentWakeWorker,
        )?
        .is_some())
    }

    /// Test only: `host.agent_wake` as the store holds it.
    #[cfg(test)]
    pub(crate) fn agent_wake_fact_for_test(&self) -> RuntimeHostResult<Option<RuntimeFactRecord>> {
        Ok(lock(
            &self
                .shared_ref(HostLocation::AgentWakeWorker)?
                .runtime_facts,
            HostLocation::AgentWakeWorker,
        )?
        .get(&RuntimeFactScope::Runtime, WAKE_FACT_KEY)
        .cloned())
    }

    /// Test only: `record` written as the store's next record of its key.
    #[cfg(test)]
    pub(crate) fn agent_wake_record_fact_for_test(
        &self,
        record: RuntimeFactRecord,
    ) -> RuntimeHostResult<()> {
        self.shared_ref(HostLocation::AgentWakeWorker)?
            .record_runtime_fact(record)
            .map(|_| ())
    }

    /// Test only: the Runtime stops, as a shutdown request says, without closing the host yet.
    #[cfg(test)]
    pub(crate) fn agent_wake_request_shutdown_for_test(&self) -> RuntimeHostResult<()> {
        self.shared_ref(HostLocation::AgentWakeWorker)?
            .fatal
            .request_shutdown();
        Ok(())
    }
}
