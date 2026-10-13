// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl observe --export <dir>` (Workflow #389 MCP-5a; MCP-0 command table v1.1 §4.5),
//! the CLI original of MCP `ac_observe`. One ObserveReadonly on the instance `--instance` names
//! by alias, instance_id or ADB port (one Status resolves it); the frame that observation
//! captured is read back through ReadMaterial, verified, and written whole to
//! `<dir>\<sha256 hex>.png`. stdout is one line, the same JSON as ac_observe's result:
//! `{width, height, verdict, capture_backend, frame {ref, media_type, size, sha256},
//! export {path, sha256, size}}`. Only `export.path` differs between the two: ac_observe writes
//! to the MCP export directory. `observe` without `--export` is unchanged (`Invocation::parse`).

use crate::codes::ActingctlCode;
use crate::instance_selector::{self, SelectorMiss};
use crate::material_export::{self, MAX_EXPORT_BYTES, WriteFailure};
use actingcommand_contract::{
    EventActor, EventSource, LedgerEventPosition, ProjectedEvent, ReadonlyObservation,
    RuntimeMaterialReadFailure, RuntimeMaterialReadState, RuntimeResult,
};
use actingcommand_runtime_client::{
    RuntimeClient, RuntimeClientConfig, RuntimeClientError, RuntimeFlowOutput,
    RuntimeMaterialCompleteResult, RuntimeMaterialSelection,
};
use serde_json::{Value, json};
use std::ffi::OsString;
use std::fmt;
use std::io::{self, Write};
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
    Some(match run(arguments).and_then(|output| print(&output)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("FATAL actingctl: {failure}");
            ExitCode::FAILURE
        }
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
/// one or a missing value is `actingctl_usage_invalid`.
fn parse(arguments: &[OsString]) -> Result<Parsed, Failure> {
    let (mut state_root, mut selector, mut directory) = (None, None, None);
    let mut rest = arguments[1..].iter();
    while let Some(flag) = rest.next() {
        let slot = match flag.to_str() {
            Some("--state-root") => &mut state_root,
            Some("--instance") => &mut selector,
            Some("--export") => &mut directory,
            _ => return Err(Failure::Usage),
        };
        let value = rest.next().ok_or(Failure::Usage)?;
        if slot.replace(value.clone()).is_some() {
            return Err(Failure::Usage);
        }
    }
    let selector = selector
        .and_then(|selector| selector.into_string().ok())
        .filter(|selector| !selector.trim().is_empty())
        .ok_or(Failure::Usage)?;
    let directory = path::absolute(directory.ok_or(Failure::Usage)?).map_err(|_| Failure::Usage)?;
    Ok(Parsed {
        state_root: PathBuf::from(state_root.ok_or(Failure::Usage)?),
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
        RuntimeMaterialCompleteResult::NotProvided { failure, .. } => Err(Failure::NotProvided {
            failure: failure.map(Box::new),
        }),
    }
}

fn state_text(state: RuntimeMaterialReadState) -> String {
    match json!(state) {
        Value::String(text) => text,
        other => other.to_string(),
    }
}

fn print(output: &Value) -> Result<(), Failure> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, output).map_err(|error| Failure::Output(error.into()))?;
    stdout.write_all(b"\n").map_err(Failure::Output)
}

/// Why `observe --export` printed no answer; every one exits 1 with its code.
enum Failure {
    Usage,
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
    /// The Runtime answered the read without the frame. Its typed failure is kept as it came,
    /// so the code it carries is relayed unchanged (A5 relays it as an outcome link).
    NotProvided {
        failure: Option<Box<RuntimeMaterialReadFailure>>,
    },
    Write(WriteFailure),
    Output(io::Error),
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage => write!(
                formatter,
                "{}: {USAGE}",
                ActingctlCode::ActingctlUsageInvalid.as_str()
            ),
            Self::Runtime(error) => write!(formatter, "{error}"),
            Self::Selector { miss, selector } => {
                write!(formatter, "{}: {}", miss.code(), miss.message(selector))
            }
            Self::FrameUnrecorded => write!(
                formatter,
                "{}: none of this observation's events carries the frame it reports",
                ActingctlCode::MaterialRefUnknown.as_str()
            ),
            Self::TooLarge { byte_count } => write!(
                formatter,
                "{}: the frame has {byte_count} bytes; an export writes at most {MAX_EXPORT_BYTES}",
                ActingctlCode::MaterialTooLarge.as_str()
            ),
            // As ac_material relays it: the Runtime's typed failure, or actingctl's own
            // `material_<state>` family when it gave none (A5 splits that family).
            Self::NotProvided { failure } => {
                let state = state_text(RuntimeMaterialReadState::NotProvided);
                match failure {
                    Some(failure) => write!(formatter, "{}", failure.code),
                    None => write!(formatter, "material_{state}"),
                }?;
                write!(
                    formatter,
                    ": the Runtime did not provide the frame (state {state})"
                )
            }
            Self::Write(failure) => write!(
                formatter,
                "{}: {}",
                ActingctlCode::MaterialExportFailed.as_str(),
                failure.message()
            ),
            Self::Output(error) => write!(
                formatter,
                "{}: cannot write stdout: {error}",
                ActingctlCode::ActingctlOutputFailed.as_str()
            ),
        }
    }
}
