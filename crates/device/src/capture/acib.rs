// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #479 F-3a (`contracts/device-binding.md`, "An iOS handset"; register §1.486): an iOS
//! handset's frame, taken from `acib`, the project's iOS device bridge, the way an Android
//! device's is taken from `adb exec-out screencap -p`: one child process per frame.
//!
//! Each capture starts one `acib screenshot --device-key <key> --key-salt-env <NAME> --out -
//! --via screen_capture --timeout-ms <deadline>` and ends with it (`apps/acib/README.md`, "The
//! commands", "`--out -`"): the child's standard output holds its one answer line, a line feed,
//! and exactly the line's `bytes` bytes of the picture. The child is started with standard input
//! from nowhere, its output and errors through pipes and no console window; the salt the key was
//! made with (the installation's own secret) is a variable of that one child's environment
//! ([`ACIB_KEY_SALT_VARIABLE`]), never an argument, a log line or a record. Its errors are read
//! and dropped unread; its output is read up to [`ACIB_ANSWER_LINE_LIMIT_BYTES`] of line and the
//! frame size cap of the other backends of picture. `acib`'s own deadline
//! ([`ACIB_SCREENSHOT_DEADLINE_MS`]) is below the Runtime's wait ([`ACIB_SCREENSHOT_WAIT_MS`]), so
//! a child left behind ends by itself. Past the wait the child is ended and its end confirmed as
//! the `adb` runner's (`stop_child`); a child whose end cannot be confirmed leaves the capture's
//! resources unconfirmed, which fences that instance alone.
//!
//! Every way of taking no frame is one cause ([`IosCaptureProblem`]), with `acib`'s own word
//! ([`AcibProblem`]) and stage ([`AcibStage`]) where it answered them; never `acib`'s error text,
//! its standard error, its session number or the command line. The picture becomes an 8-bit sRGB
//! frame ([`picture`]) and no original is kept; a picture whose every byte is then zero is no
//! frame (`capture_frame_all_black`, [`FrameRefusal::AllBlack`]), which closes nothing.

use super::{CaptureBackend, CaptureBackendName, Frame, PixelFormat};
use crate::{
    AcibProblem, AcibStage, DeviceCloseAuthority, DeviceError, DeviceResourceCloseOutcome,
    DeviceResourceClosePhase, DeviceResourceKind, DeviceResourceQuiescence, DeviceResult,
    FrameRefusal, IosCaptureFailure, IosCaptureProblem,
};
use actingcommand_contract::{
    BackendObservationStatus, BackendOpenEntry, BackendOpenReport, BackendOpenSource,
};
use serde::Deserialize;
use std::fmt;
use std::io::{self, Read};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

mod picture;

// Workflow #479 F-3a: the device crate's gate of this slice (exec gate map row "#479 F-3a").
#[cfg(test)]
mod gate_479_f3a;

/// The environment variable the Runtime gives `acib` the salt of the handset's key under (the
/// installation's own `secret_fingerprint_salt`), in that one child's environment alone: the name
/// the Lab's listing uses too.
pub const ACIB_KEY_SALT_VARIABLE: &str = "ACTINGCOMMAND_ACIB_KEY_SALT";
/// `acib`'s own deadline for one screenshot (`--timeout-ms`), below the Runtime's wait: a child
/// the Runtime could not end ends by itself.
pub const ACIB_SCREENSHOT_DEADLINE_MS: u64 = 8_000;
/// How long the Runtime waits for one `acib screenshot` to end with its answer.
pub const ACIB_SCREENSHOT_WAIT_MS: u64 = 10_000;
/// The longest answer line read, in bytes, its line feed not counted.
pub const ACIB_ANSWER_LINE_LIMIT_BYTES: usize = 4 * 1024;
/// The largest picture read, in bytes: the frame size cap of the other backends.
pub const ACIB_PICTURE_LIMIT_BYTES: usize = super::DROIDCAST_MAX_RESPONSE_BYTES;
/// How often the Runtime looks whether `acib` ended.
const ACIB_POLL: Duration = Duration::from_millis(25);
/// How long the Runtime waits for an ended child's streams, and for a child it ends.
const ACIB_END_WAIT: Duration = Duration::from_millis(500);
/// The backend name a child's close cause names.
const ACIB_BACKEND: &str = "acib";
/// `CREATE_NO_WINDOW`: the child gets no console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// The `--out` that names the standard output (`apps/acib/README.md`).
const STANDARD_OUTPUT: &str = "-";
/// The one picture format the conversion reads.
const PNG_FORMAT: &str = "png";

/// The salt a handset's key was made with: the installation's own secret. Its `Debug` withholds
/// it; it reaches `acib` only as a variable of the child's environment.
#[derive(Clone)]
pub struct AcibKeySalt(String);

impl AcibKeySalt {
    pub fn new(salt: impl Into<String>) -> Self {
        Self(salt.into())
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AcibKeySalt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AcibKeySalt(<withheld>)")
    }
}

/// The bounds of one capture: `acib`'s own deadline, the Runtime's wait, the caps on the answer
/// line and on the picture, and the time between two looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcibBounds {
    pub deadline_ms: u64,
    pub wait: Duration,
    pub line_limit: usize,
    pub picture_limit: usize,
    pub poll: Duration,
}

impl AcibBounds {
    /// The Runtime's bounds: [`ACIB_SCREENSHOT_DEADLINE_MS`], [`ACIB_SCREENSHOT_WAIT_MS`],
    /// [`ACIB_ANSWER_LINE_LIMIT_BYTES`], [`ACIB_PICTURE_LIMIT_BYTES`] and a look every 25 ms.
    pub const fn runtime() -> Self {
        Self {
            deadline_ms: ACIB_SCREENSHOT_DEADLINE_MS,
            wait: Duration::from_millis(ACIB_SCREENSHOT_WAIT_MS),
            line_limit: ACIB_ANSWER_LINE_LIMIT_BYTES,
            picture_limit: ACIB_PICTURE_LIMIT_BYTES,
            poll: ACIB_POLL,
        }
    }
}

/// One child to start: the program, its arguments and the variables its environment gains. A salt
/// is only ever a value of `environment`; the `Debug` shows the variables' names alone.
pub struct AcibCommand {
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub environment: Vec<(&'static str, AcibKeySalt)>,
}

impl fmt::Debug for AcibCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AcibCommand")
            .field("arguments", &self.arguments)
            .field(
                "environment",
                &self
                    .environment
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// What starts `acib`: the system's processes ([`SystemAcibLauncher`]), or a test's scripted ones.
pub trait AcibLauncher: Send + Sync {
    /// Starts `command`: standard input from nowhere, its output and errors through pipes, no
    /// console window, and `command.environment` beside the Runtime's own environment.
    fn launch(&self, command: &AcibCommand) -> io::Result<Box<dyn AcibProcess>>;
}

/// One started `acib` as the runner follows it. Its time is its own: how long since it started
/// ([`Self::elapsed`]) and a wait for its end ([`Self::pause`]).
pub trait AcibProcess {
    /// Its standard output, once.
    fn output(&mut self) -> Option<Box<dyn Read + Send>>;
    /// Its standard error, once.
    fn errors(&mut self) -> Option<Box<dyn Read + Send>>;
    /// Whether it ended, `Some(true)` with status 0; `None` while it runs.
    fn ended(&mut self) -> io::Result<Option<bool>>;
    /// The time since it started.
    fn elapsed(&self) -> Duration;
    /// Waits until it ends or `span` passes.
    fn pause(&mut self, span: Duration) -> io::Result<()>;
    /// Ends it and confirms its end ([`crate::stop_child`]).
    fn end(&mut self) -> DeviceResult<DeviceResourceCloseOutcome>;
}

/// The system's processes.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemAcibLauncher;

impl AcibLauncher for SystemAcibLauncher {
    fn launch(&self, command: &AcibCommand) -> io::Result<Box<dyn AcibProcess>> {
        let mut process = Command::new(&command.program);
        process
            .args(&command.arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, salt) in &command.environment {
            process.env(name, salt.expose());
        }
        #[cfg(windows)]
        process.creation_flags(CREATE_NO_WINDOW);
        let child = process.spawn()?;
        Ok(Box::new(SystemAcibProcess {
            child,
            started: Instant::now(),
        }))
    }
}

/// A started `acib` of the system.
struct SystemAcibProcess {
    child: Child,
    started: Instant,
}

impl AcibProcess for SystemAcibProcess {
    fn output(&mut self) -> Option<Box<dyn Read + Send>> {
        self.child
            .stdout
            .take()
            .map(|stream| Box::new(stream) as Box<dyn Read + Send>)
    }

    fn errors(&mut self) -> Option<Box<dyn Read + Send>> {
        self.child
            .stderr
            .take()
            .map(|stream| Box::new(stream) as Box<dyn Read + Send>)
    }

    fn ended(&mut self) -> io::Result<Option<bool>> {
        self.child
            .try_wait()
            .map(|status| status.map(|status| status.success()))
    }

    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    fn pause(&mut self, span: Duration) -> io::Result<()> {
        crate::adb::wait_for_child_exit(&self.child, span)
    }

    fn end(&mut self) -> DeviceResult<DeviceResourceCloseOutcome> {
        crate::stop_child(&mut self.child, ACIB_END_WAIT, ACIB_BACKEND)
    }
}

/// Where and how an iOS handset's frames are taken: the install root's `acib` (`None` when the
/// Runtime runs from no install root), the salt of the handset's key, the bounds and the launcher.
#[derive(Clone)]
pub struct AcibCaptureSource {
    program: Option<PathBuf>,
    key_salt: AcibKeySalt,
    bounds: AcibBounds,
    launcher: Arc<dyn AcibLauncher>,
}

impl AcibCaptureSource {
    /// `acib` at `program`, the key made under `key_salt`, the Runtime's bounds and the system's
    /// processes.
    pub fn new(program: Option<PathBuf>, key_salt: AcibKeySalt) -> Self {
        Self {
            program,
            key_salt,
            bounds: AcibBounds::runtime(),
            launcher: Arc::new(SystemAcibLauncher),
        }
    }

    /// The same source starting its children through `launcher` (a test's scripted `acib`).
    pub fn with_launcher(mut self, launcher: Arc<dyn AcibLauncher>) -> Self {
        self.launcher = launcher;
        self
    }

    /// The same source within `bounds`.
    pub fn with_bounds(mut self, bounds: AcibBounds) -> Self {
        self.bounds = bounds;
        self
    }

    /// Where `acib` is looked for.
    pub fn program(&self) -> Option<&Path> {
        self.program.as_deref()
    }

    /// The command of one screenshot of the phone whose key is `device_key`: the key on the
    /// command line, the salt in the child's environment alone.
    fn command(&self, program: &Path, device_key: &str) -> AcibCommand {
        AcibCommand {
            program: program.to_path_buf(),
            arguments: [
                "screenshot",
                "--device-key",
                device_key,
                "--key-salt-env",
                ACIB_KEY_SALT_VARIABLE,
                "--out",
                STANDARD_OUTPUT,
                "--via",
                "screen_capture",
                "--timeout-ms",
            ]
            .into_iter()
            .map(str::to_owned)
            .chain([self.bounds.deadline_ms.to_string()])
            .collect(),
            environment: vec![(ACIB_KEY_SALT_VARIABLE, self.key_salt.clone())],
        }
    }
}

impl fmt::Debug for AcibCaptureSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AcibCaptureSource")
            .field("program", &self.program)
            .field("key_salt", &self.key_salt)
            .field("bounds", &self.bounds)
            .finish()
    }
}

/// The capture backend of an iOS handset bound by `device_key`, opened from `source`: it holds
/// nothing between two captures, so its open acquires nothing and takes no frame (the first
/// capture's frame checks it, `execute_capture`).
pub fn open_acib_screen_capture(
    source: &AcibCaptureSource,
    device_key: &str,
) -> crate::OpenedBackend<Box<dyn CaptureBackend>> {
    let mut report = BackendOpenReport::unobserved(BackendOpenEntry::Capture);
    report.source = BackendOpenSource::Native;
    report.requested = CaptureBackendName::AcibScreenCapture.as_str().into();
    report.selected = Some(report.requested.clone());
    report.status = BackendObservationStatus::Passed;
    crate::OpenedBackend::new(
        Box::new(AcibScreenCaptureBackend {
            source: source.clone(),
            device_key: device_key.to_owned(),
            close_result: None,
        }) as Box<dyn CaptureBackend>,
        report,
    )
}

/// One `acib screenshot` child per frame.
pub struct AcibScreenCaptureBackend {
    source: AcibCaptureSource,
    device_key: String,
    close_result: Option<DeviceResult<DeviceResourceCloseOutcome>>,
}

impl CaptureBackend for AcibScreenCaptureBackend {
    fn capture(&mut self) -> DeviceResult<Frame> {
        let png = take_picture(&self.source, &self.device_key)?;
        frame_of(&png)
    }

    fn close_once(
        &mut self,
        _authority: DeviceCloseAuthority,
    ) -> DeviceResult<DeviceResourceCloseOutcome> {
        if let Some(result) = &self.close_result {
            return result.clone();
        }
        let result = Ok(DeviceResourceCloseOutcome::confirmed(0));
        self.close_result = Some(result.clone());
        result
    }
}

/// The frame of the picture `png`: its 8-bit sRGB pixels, no original kept; a picture the
/// conversion does not read, or one whose every byte is zero, is no frame.
fn frame_of(png: &[u8]) -> DeviceResult<Frame> {
    let converted = picture::to_srgb8(png).map_err(|problem| {
        Refused::of(match problem {
            picture::PictureProblem::Unreadable => IosCaptureProblem::IosPictureUnreadable,
            picture::PictureProblem::ColorSpaceUnsupported => {
                IosCaptureProblem::IosPictureColorSpaceUnsupported
            }
        })
        .into_error()
    })?;
    if converted.pixels.iter().all(|byte| *byte == 0) {
        // The coordinator's ruling 3 (#479 F-3): a black picture is never passed on as a frame.
        // No lock state is read; the backend stays open and holds nothing.
        return Err(DeviceError::transient(
            "acib screenshot answered a picture whose every byte is zero; it is no frame",
        )
        .with_frame_refused(FrameRefusal::AllBlack));
    }
    Frame::from_pixels(
        converted.width,
        converted.height,
        converted.pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::AcibScreenCapture,
    )
}

/// Why one `acib screenshot` gave no picture: its one cause, and the end of the child or of a
/// stream that could not be confirmed (`None` when everything ended).
struct Refused {
    failure: IosCaptureFailure,
    unconfirmed: Option<DeviceError>,
}

impl From<IosCaptureFailure> for Refused {
    /// `failure`, with nothing left unconfirmed.
    fn from(failure: IosCaptureFailure) -> Self {
        Self {
            failure,
            unconfirmed: None,
        }
    }
}

impl Refused {
    fn of(problem: IosCaptureProblem) -> Self {
        Self::from(IosCaptureFailure::of(problem))
    }

    /// The device error: a fixed text naming the cause and `acib`'s word, never its error text,
    /// its standard error, the command line or the salt.
    fn into_error(self) -> DeviceError {
        let message = match self.failure.acib_problem {
            Some(word) => format!(
                "acib screenshot gave no frame: {} (acib: {})",
                self.failure.problem.as_str(),
                word.as_str()
            ),
            None => format!(
                "acib screenshot gave no frame: {}",
                self.failure.problem.as_str()
            ),
        };
        let error = DeviceError::fatal(message).with_ios_capture_failure(self.failure);
        match self.unconfirmed {
            Some(cleanup) => error.merge_resource_cleanup(cleanup),
            None => error,
        }
    }
}

/// `acib`'s word `problem` (and its stage), as the cause its step names.
const fn answered(problem: AcibProblem, stage: Option<AcibStage>) -> IosCaptureFailure {
    IosCaptureFailure {
        problem: cause_of(problem),
        acib_problem: Some(problem),
        acib_stage: stage,
    }
}

/// The cause the step of `acib`'s word `problem` names. Fix round 1: `service_unreachable` is,
/// by `apps/acib/README.md` ("The commands", `screenshot`), a word of the way's own service, the
/// screen capture service, so it is that service's cause, not the tunnel's.
const fn cause_of(problem: AcibProblem) -> IosCaptureProblem {
    match problem {
        AcibProblem::DeviceKeyUnknown | AcibProblem::DeviceNotListed => {
            IosCaptureProblem::IosDeviceNotListed
        }
        AcibProblem::UsbmuxdUnreachable
        | AcibProblem::UsbmuxdListFailed
        | AcibProblem::PairRecordUnreadable => IosCaptureProblem::IosDeviceServiceUnreachable,
        AcibProblem::NotPaired => IosCaptureProblem::IosNotPaired,
        AcibProblem::LockdownUnreachable
        | AcibProblem::LockdownReadFailed
        | AcibProblem::LockdownValueUnexpected
        | AcibProblem::SessionRefused
        | AcibProblem::ServiceRefused
        | AcibProblem::TunnelFailed
        | AcibProblem::RsdUnreachable
        | AcibProblem::RsdHandshakeFailed => IosCaptureProblem::IosTunnelFailed,
        AcibProblem::ServiceNotListed
        | AcibProblem::ServiceUnreachable
        | AcibProblem::ServiceHandshakeFailed
        | AcibProblem::ChannelRefused
        | AcibProblem::ScreenshotFailed => IosCaptureProblem::IosScreenCaptureFailed,
        AcibProblem::DeadlinePassed => IosCaptureProblem::AcibDeadlinePassed,
        AcibProblem::PictureHeaderTruncated | AcibProblem::PictureHeaderUnknown => {
            IosCaptureProblem::IosPictureUnreadable
        }
        AcibProblem::CommandLineInvalid
        | AcibProblem::AsyncRuntimeFailed
        | AcibProblem::KeySaltUnusable => IosCaptureProblem::AcibCallRefused,
    }
}

/// Which of the child's two streams a reader's end is of.
#[derive(Clone, Copy)]
enum Stream {
    Output,
    Errors,
}

/// What a reader of a stream read to the stream's end.
enum StreamRead {
    /// The bytes it kept (none of the errors, which are dropped).
    Ended(Vec<u8>),
    /// More than the cap.
    TooLong,
    /// The read failed.
    Failed,
}

/// Reads `stream` to its end: at most `limit` bytes kept when `keep`, else everything read and
/// dropped unread.
fn read_stream(mut stream: Box<dyn Read + Send>, limit: usize, keep: bool) -> StreamRead {
    let mut kept = Vec::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => return StreamRead::Ended(kept),
            Ok(count) if keep => {
                if kept.len().saturating_add(count) > limit {
                    return StreamRead::TooLong;
                }
                kept.extend_from_slice(&buffer[..count]);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return StreamRead::Failed,
        }
    }
}

/// The readers of one child's two streams, each on a thread of its own, so that neither pipe can
/// fill; their ends arrive on one channel.
struct Readers {
    ends: mpsc::Receiver<(Stream, StreamRead)>,
    output: Option<StreamRead>,
    errors: Option<StreamRead>,
}

impl Readers {
    fn start(
        output: Box<dyn Read + Send>,
        errors: Box<dyn Read + Send>,
        output_limit: usize,
    ) -> io::Result<Self> {
        let (sender, ends) = mpsc::channel();
        for (stream, which) in [(output, Stream::Output), (errors, Stream::Errors)] {
            let sender = sender.clone();
            let keep = matches!(which, Stream::Output);
            // A reader is not joined: it ends with its stream, and its end is what it sends. One
            // whose end nobody waits for any more has nobody to tell it to.
            let _reader = thread::Builder::new()
                .name("actingcommand-acib-reader".to_owned())
                .spawn(move || {
                    let read = read_stream(stream, output_limit, keep);
                    sender.send((which, read)).ok();
                })?;
        }
        Ok(Self {
            ends,
            output: None,
            errors: None,
        })
    }

    fn note(&mut self, (which, read): (Stream, StreamRead)) {
        match which {
            Stream::Output => self.output = Some(read),
            Stream::Errors => self.errors = Some(read),
        }
    }

    /// Takes every end that arrived.
    fn poll(&mut self) {
        while let Ok(end) = self.ends.try_recv() {
            self.note(end);
        }
    }

    /// Waits up to `span` for the ends still to come; whether both streams ended.
    fn settle(&mut self, span: Duration) -> bool {
        let deadline = Instant::now() + span;
        while !self.both_ended() {
            match self
                .ends
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(end) => self.note(end),
                Err(_) => return false,
            }
        }
        true
    }

    fn both_ended(&self) -> bool {
        self.output.is_some() && self.errors.is_some()
    }

    /// The cause a stream gives on its own: the output over its cap, the output that could not
    /// be read (its answer cannot be read), or, fix round 1, the errors that could not be read,
    /// which is no answer's fault: a cause of its own.
    fn stream_problem(&self) -> Option<IosCaptureProblem> {
        match (&self.output, &self.errors) {
            (Some(StreamRead::TooLong), _) => Some(IosCaptureProblem::AcibAnswerTooLong),
            (Some(StreamRead::Failed), _) => Some(IosCaptureProblem::AcibAnswerUnreadable),
            (_, Some(StreamRead::Failed)) => Some(IosCaptureProblem::AcibErrorsUnreadable),
            _ => None,
        }
    }
}

/// The resource error of streams that did not end after their child did, or was ended.
fn readers_unconfirmed() -> DeviceError {
    DeviceError::fatal("acib output reader remains open at close deadline")
        .with_resource_close_cause(
            DeviceResourceKind::PipeReader,
            DeviceResourceClosePhase::WorkerJoin,
            ACIB_BACKEND,
            None,
            None,
            DeviceResourceQuiescence::Unconfirmed,
            1,
        )
}

/// Ends `process` and waits for its streams. `refused` is the cause, `acib_not_ended` when its end
/// cannot be confirmed (the cleanup error carries the unconfirmed resource); streams that do not
/// end after it are an unconfirmed reader beside the cause.
fn end_with(
    process: &mut dyn AcibProcess,
    readers: Option<&mut Readers>,
    mut refused: Refused,
) -> Refused {
    if let Err(cleanup) = process.end()
        && cleanup.resource_quiescence() != Some(DeviceResourceQuiescence::Confirmed)
    {
        refused.failure = IosCaptureFailure::of(IosCaptureProblem::AcibNotEnded);
        refused.unconfirmed = Some(cleanup);
        return refused;
    }
    if let Some(readers) = readers
        && !readers.settle(ACIB_END_WAIT)
    {
        refused.unconfirmed = Some(readers_unconfirmed());
    }
    refused
}

/// One `acib screenshot` of the phone whose key is `device_key`: the picture's bytes, or the
/// device error of its one cause.
fn take_picture(source: &AcibCaptureSource, device_key: &str) -> DeviceResult<Vec<u8>> {
    let Some(program) = source.program().filter(|program| program.is_file()) else {
        return Err(Refused::of(IosCaptureProblem::AcibMissing).into_error());
    };
    let command = source.command(program, device_key);
    let mut process = source
        .launcher
        .launch(&command)
        .map_err(|_| Refused::of(IosCaptureProblem::AcibNotStarted).into_error())?;
    let (Some(output), Some(errors)) = (process.output(), process.errors()) else {
        // Both were asked for as pipes; without them no answer can be read.
        return Err(end_with(
            &mut *process,
            None,
            Refused::of(IosCaptureProblem::AcibAnswerUnreadable),
        )
        .into_error());
    };
    let bounds = source.bounds;
    let output_limit = bounds
        .line_limit
        .saturating_add(1)
        .saturating_add(bounds.picture_limit);
    // Fix round 1: a reader the system did not start is a cause of its own, the child ended as
    // at every other cause (`acib_not_ended` when its end cannot be confirmed).
    let Ok(mut readers) = Readers::start(output, errors, output_limit) else {
        return Err(end_with(
            &mut *process,
            None,
            Refused::of(IosCaptureProblem::AcibReadersNotStarted),
        )
        .into_error());
    };
    let success = loop {
        readers.poll();
        if let Some(problem) = readers.stream_problem() {
            return Err(
                end_with(&mut *process, Some(&mut readers), Refused::of(problem)).into_error(),
            );
        }
        match process.ended() {
            Ok(Some(success)) => break success,
            Ok(None) => {}
            Err(_) => {
                return Err(end_with(
                    &mut *process,
                    Some(&mut readers),
                    Refused::of(IosCaptureProblem::AcibStateUnreadable),
                )
                .into_error());
            }
        }
        if process.elapsed() >= bounds.wait {
            return Err(end_with(
                &mut *process,
                Some(&mut readers),
                Refused::of(IosCaptureProblem::AcibNoAnswerInTime),
            )
            .into_error());
        }
        if process.pause(bounds.poll).is_err() {
            return Err(end_with(
                &mut *process,
                Some(&mut readers),
                Refused::of(IosCaptureProblem::AcibStateUnreadable),
            )
            .into_error());
        }
    };
    if !readers.settle(ACIB_END_WAIT) {
        // The child ended, but a stream of it did not: its answer is cut short where it stands.
        let mut refused = Refused::of(IosCaptureProblem::AcibAnswerIncomplete);
        refused.unconfirmed = Some(readers_unconfirmed());
        return Err(refused.into_error());
    }
    if let Some(problem) = readers.stream_problem() {
        return Err(Refused::of(problem).into_error());
    }
    match readers.output.take() {
        Some(StreamRead::Ended(answer)) => read_answer(answer, success, &bounds)
            .map_err(|failure| Refused::from(failure).into_error()),
        _ => Err(Refused::of(IosCaptureProblem::AcibAnswerUnreadable).into_error()),
    }
}

/// The members of `acib screenshot`'s line the Runtime reads (`apps/acib/README.md`); every other
/// member is left unread.
#[derive(Deserialize)]
struct AcibScreenshotLine {
    problem: Option<String>,
    #[serde(rename = "stage")]
    waited_in: Option<String>,
    bytes: Option<u64>,
    out: Option<String>,
    format: Option<String>,
}

/// The picture of the answer `answer` of a child that ended with status 0 when `success`: its
/// line, one line feed and exactly the line's `bytes` bytes. Fix round 1: it reads a whole answer
/// of a child that ended, so it never leaves anything unconfirmed, and its failure is the cause
/// alone (`IosCaptureFailure`, a few bytes; not the device error the caller makes of it).
fn read_answer(
    mut answer: Vec<u8>,
    success: bool,
    bounds: &AcibBounds,
) -> Result<Vec<u8>, IosCaptureFailure> {
    let failed = IosCaptureFailure::of;
    let Some(end) = answer
        .iter()
        .take(bounds.line_limit.saturating_add(1))
        .position(|byte| *byte == b'\n')
    else {
        return Err(failed(if answer.len() > bounds.line_limit {
            IosCaptureProblem::AcibAnswerTooLong
        } else {
            IosCaptureProblem::AcibAnswerIncomplete
        }));
    };
    let line = serde_json::from_slice::<AcibScreenshotLine>(&answer[..end])
        .map_err(|_| failed(IosCaptureProblem::AcibAnswerUnreadable))?;
    if let Some(word) = line.problem.as_deref() {
        let problem = AcibProblem::ALL
            .iter()
            .copied()
            .find(|known| known.as_str() == word)
            .ok_or_else(|| failed(IosCaptureProblem::AcibAnswerUnreadable))?;
        let stage = line.waited_in.as_deref().and_then(|stage| {
            AcibStage::ALL
                .iter()
                .copied()
                .find(|known| known.as_str() == stage)
        });
        return Err(answered(problem, stage));
    }
    if !success {
        // It ended otherwise than with its answer (ended by another party).
        return Err(failed(IosCaptureProblem::AcibAnswerIncomplete));
    }
    let announced = match (line.out.as_deref(), line.bytes) {
        (Some(STANDARD_OUTPUT), Some(bytes)) => bytes,
        _ => return Err(failed(IosCaptureProblem::AcibAnswerUnreadable)),
    };
    if announced > bounds.picture_limit as u64 {
        return Err(failed(IosCaptureProblem::AcibAnswerTooLong));
    }
    let picture = answer.split_off(end + 1);
    match (picture.len() as u64).cmp(&announced) {
        std::cmp::Ordering::Less => Err(failed(IosCaptureProblem::AcibAnswerIncomplete)),
        std::cmp::Ordering::Greater => Err(failed(IosCaptureProblem::AcibAnswerTooLong)),
        std::cmp::Ordering::Equal if line.format.as_deref() == Some(PNG_FORMAT) => Ok(picture),
        std::cmp::Ordering::Equal => Err(failed(IosCaptureProblem::IosPictureUnreadable)),
    }
}
