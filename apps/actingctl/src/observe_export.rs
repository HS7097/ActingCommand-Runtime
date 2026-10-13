// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl observe --export <dir>` (Workflow #389 MCP-5a; MCP-0 command table v1.1 §4.5),
//! the CLI original of MCP `ac_observe`. One ObserveReadonly on the instance `--instance` names
//! by alias, instance_id or ADB port (one Status resolves it); the frame that observation
//! captured is read back through ReadMaterial, verified, and written whole to
//! `<dir>\<sha256 hex>.png`. stdout is one line, the same JSON as ac_observe's result:
//! `{width, height, verdict, capture_backend, frame {ref, media_type, size, sha256},
//! export {path, sha256, size}}`. Only `export.path` differs between the two: ac_observe writes
//! to the MCP export directory. `observe` without `--export` is unchanged (`Invocation::parse`).
//! The stdout line also carries `outcome` (Workflow #378 A5): `instance_frame_exported`, or the
//! failure alone.

use crate::cli_line;
use crate::codes::{ActingctlCode, ArgumentProblem, MaterialLimit, MaterialState};
use crate::instance_selector::{self, SelectorMiss};
use crate::material_export::{self, MAX_EXPORT_BYTES, WriteFailure};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    EventActor, EventSource, LedgerEventPosition, ProjectedEvent, ReadonlyObservation,
    RuntimeMaterialReadFailure, RuntimeMaterialReadLimit, RuntimeMaterialReadState, RuntimeResult,
};
use actingcommand_runtime_client::{
    RuntimeClient, RuntimeClientConfig, RuntimeClientError, RuntimeFlowOutput,
    RuntimeMaterialCompleteResult, RuntimeMaterialSelection,
};
use serde_json::{Value, json};
use std::ffi::{OsStr, OsString};
use std::path::{self, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

const USAGE: &str = "usage: actingctl observe --state-root <path> --instance <alias|instance_id|ADB port> --export <directory>";
/// How long the CLI reads the frame back; every exchange is also bounded by the client's own
/// I/O timeout.
const READ_BUDGET: Duration = Duration::from_secs(60);

/// Runs `observe` when it carries `--export`; `None` leaves every other command, and `observe`
/// without `--export`, to `Invocation::parse`, unchanged.
pub(crate) fn dispatch(arguments: &[OsString]) -> Option<ExitCode> {
    if arguments.first()?.to_str()? != "observe"
        || !arguments[1..]
            .iter()
            .any(|argument| argument.as_os_str() == "--export")
    {
        return None;
    }
    Some(match run(arguments) {
        Ok(answer) => cli_line::success(
            cli_line::members(answer, "frame"),
            &Outcome::new(ActingctlCode::InstanceFrameExported),
        ),
        Err(failure @ Failure::Usage { .. }) => cli_line::failure(&failure.outcome(), Some(USAGE)),
        Err(failure) => cli_line::failure(&failure.outcome(), None),
    })
}

/// The frame one observation captured, with the event of that observation that carries it.
pub(crate) struct ObservedFrame<'a> {
    pub(crate) observation: &'a ReadonlyObservation,
    pub(crate) event: &'a ProjectedEvent,
    /// The newest position among the observation's events: a snapshot that holds `event`.
    pub(crate) snapshot: u64,
}

impl<'a> ObservedFrame<'a> {
    /// `None` when none of the observation's events carries the frame its result reports.
    pub(crate) fn of(flow: &'a RuntimeFlowOutput) -> Option<Self> {
        let Some(RuntimeResult::ReadonlyObservationCompleted { observation }) =
            flow.receipt().result()
        else {
            return None;
        };
        let artifact = observation.artifact();
        let event = flow.events().iter().find(|event| {
            event.artifacts.iter().any(|carried| {
                carried.artifact_id == artifact.artifact_id
                    && carried.sha256 == artifact.sha256
                    && carried.byte_count == artifact.byte_count
            })
        })?;
        let snapshot = flow.events().iter().map(|event| event.sequence).max()?;
        Some(Self {
            observation,
            event,
            snapshot,
        })
    }

    pub(crate) fn position(&self) -> LedgerEventPosition {
        LedgerEventPosition {
            event_id: self.event.event_id,
            sequence: self.event.sequence,
        }
    }

    /// The answer both sides give: the observation, its frame by material ref, and `export`.
    pub(crate) fn answer(&self, export: Value) -> Value {
        let artifact = self.observation.artifact();
        json!({
            "width": self.observation.width(),
            "height": self.observation.height(),
            "verdict": self.observation.verdict().as_str(),
            "capture_backend": self.observation.capture_backend(),
            "frame": {
                "ref": material_export::material_ref(self.event, artifact),
                "media_type": artifact.media_type,
                "size": artifact.byte_count,
                "sha256": artifact.sha256,
            },
            "export": export,
        })
    }
}

struct Parsed {
    state_root: PathBuf,
    selector: String,
    directory: PathBuf,
}

/// `--state-root`, `--instance` and `--export`, each exactly once; any other flag, a repeated
/// one or a missing value is `actingctl_usage_invalid` with the flag (`argument`) and what is
/// wrong with it (`argument_problem`).
fn parse(arguments: &[OsString]) -> Result<Parsed, Failure> {
    let (mut state_root, mut selector, mut directory) = (None, None, None);
    let mut rest = arguments[1..].iter();
    while let Some(flag) = rest.next() {
        let (name, slot) = match flag.to_str() {
            Some(name @ "--state-root") => (name, &mut state_root),
            Some(name @ "--instance") => (name, &mut selector),
            Some(name @ "--export") => (name, &mut directory),
            Some(_) => return Err(Failure::usage(flag, ArgumentProblem::Unknown)),
            None => return Err(Failure::usage(flag, ArgumentProblem::NotUtf8)),
        };
        let value = rest
            .next()
            .ok_or_else(|| Failure::usage(name, ArgumentProblem::ValueMissing))?;
        if slot.replace(value.clone()).is_some() {
            return Err(Failure::usage(name, ArgumentProblem::Repeated));
        }
    }
    let selector = selector
        .ok_or_else(|| Failure::usage("--instance", ArgumentProblem::Required))?
        .into_string()
        .map_err(|_| Failure::usage("--instance", ArgumentProblem::NotUtf8))?;
    if selector.trim().is_empty() {
        return Err(Failure::usage("--instance", ArgumentProblem::ValueMissing));
    }
    let directory = path::absolute(
        directory.ok_or_else(|| Failure::usage("--export", ArgumentProblem::Required))?,
    )
    .map_err(|_| Failure::usage("--export", ArgumentProblem::Unusable))?;
    Ok(Parsed {
        state_root: PathBuf::from(
            state_root.ok_or_else(|| Failure::usage("--state-root", ArgumentProblem::Required))?,
        ),
        selector,
        directory,
    })
}

fn run(arguments: &[OsString]) -> Result<Value, Failure> {
    let Parsed {
        state_root,
        selector,
        directory,
    } = parse(arguments)?;
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        state_root,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .map_err(Failure::Runtime)?;
    let status = client.status().map_err(Failure::Runtime)?;
    let alias = instance_selector::select(status.instances(), &selector)
        .map_err(|miss| Failure::Selector {
            miss,
            selector: selector.clone(),
        })?
        .instance_alias()
        .to_owned();
    let flow = client.observe_readonly(&alias).map_err(Failure::Runtime)?;
    let frame = ObservedFrame::of(&flow).ok_or(Failure::FrameUnrecorded)?;
    let bytes = read_frame(&client, &frame)?;
    let artifact = frame.observation.artifact();
    let export = material_export::write(
        &directory,
        &artifact.sha256,
        artifact.kind.extension(),
        &bytes,
    )
    .map_err(Failure::Write)?;
    Ok(frame.answer(export))
}

/// The frame's bytes, read whole through ReadMaterial and verified against its reference.
fn read_frame(client: &RuntimeClient, frame: &ObservedFrame<'_>) -> Result<Vec<u8>, Failure> {
    let artifact = frame.observation.artifact();
    if artifact.byte_count > MAX_EXPORT_BYTES as u64 {
        return Err(Failure::TooLarge {
            byte_count: artifact.byte_count,
        });
    }
    let selection = RuntimeMaterialSelection {
        event: frame.position(),
        artifact_id: artifact.artifact_id,
        snapshot_position: frame.snapshot,
        sha256: artifact.sha256.clone(),
        byte_count: artifact.byte_count,
        run_id: None,
        frame_id: None,
        request_id: None,
        correlation_id: None,
    };
    match client.read_material_complete(
        selection,
        MAX_EXPORT_BYTES,
        Instant::now() + READ_BUDGET,
        &|| true,
    ) {
        RuntimeMaterialCompleteResult::Verified { bytes, .. } => Ok(bytes),
        RuntimeMaterialCompleteResult::NotProvided {
            error: Some(error), ..
        }
        | RuntimeMaterialCompleteResult::Failed { error, .. } => Err(Failure::Runtime(error)),
        RuntimeMaterialCompleteResult::NotProvided { limit, failure, .. } => {
            Err(Failure::NotProvided {
                limit,
                failure: failure.map(Box::new),
            })
        }
    }
}

/// Why `observe --export` printed no answer; every one exits 1 with its outcome.
enum Failure {
    /// A flag is unknown, repeated, missing or without its value.
    Usage {
        argument: String,
        problem: ArgumentProblem,
    },
    Runtime(RuntimeClientError),
    Selector {
        miss: SelectorMiss,
        selector: String,
    },
    /// None of the observation's events carries the frame it reports.
    FrameUnrecorded,
    TooLarge {
        byte_count: u64,
    },
    /// The Runtime answered the read without the frame: why it stopped short, and its typed
    /// failure as it came.
    NotProvided {
        limit: RuntimeMaterialReadLimit,
        failure: Option<Box<RuntimeMaterialReadFailure>>,
    },
    Write(WriteFailure),
}

impl Failure {
    fn usage(argument: impl AsRef<OsStr>, problem: ArgumentProblem) -> Self {
        Self::Usage {
            argument: argument.as_ref().to_string_lossy().into_owned(),
            problem,
        }
    }

    /// The failure as an outcome (Workflow #378 A5). A frame the Runtime did not provide is
    /// `material_unavailable` with its read state and limit, the Runtime's own failure code, when
    /// it gave one, relayed as its cause.
    fn outcome(&self) -> Outcome {
        match self {
            Self::Usage { argument, problem } => Outcome::new(ActingctlCode::ActingctlUsageInvalid)
                .argument(argument.as_str())
                .argument_problem(*problem),
            Self::Runtime(error) => error.outcome(),
            Self::Selector { miss, selector } => miss.outcome(selector),
            Self::FrameUnrecorded => Outcome::new(ActingctlCode::MaterialRefUnknown),
            Self::TooLarge { byte_count } => Outcome::new(ActingctlCode::MaterialTooLarge)
                .actual_bytes(*byte_count)
                .limit_bytes(MAX_EXPORT_BYTES),
            Self::NotProvided { limit, failure } => {
                let outcome = Outcome::new(ActingctlCode::MaterialUnavailable)
                    .material_state(MaterialState::of(RuntimeMaterialReadState::NotProvided))
                    .material_limit(MaterialLimit::of(*limit));
                match failure {
                    Some(failure) => outcome.caused_by(Outcome::relayed(&failure.code)),
                    None => outcome,
                }
            }
            Self::Write(failure) => failure.outcome(),
        }
    }
}
