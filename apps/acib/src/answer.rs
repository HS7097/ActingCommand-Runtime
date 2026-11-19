// SPDX-License-Identifier: AGPL-3.0-only

//! The one JSON line `acib` prints for each command (board #479, ACIB-0 to ACIB-3).
//!
//! These types are the whole of what the program shows. None of them has a member for a
//! device's serial number, UDID, name or address, or for anything of a pairing record: the
//! line cannot carry them. A library error is shown through [`crate::redact`], which cuts a
//! text that could carry such a value to the error's kind. Of a picture of the screen the line
//! holds its format, its width and its height as its header gives them and its length, never
//! a byte of its content: the picture goes to the file or the standard output `--out` names and
//! nowhere else. A device's key is a [`DeviceKey`], a salted digest made by the key function
//! alone: no member holds a salt or the identifier a key was made from. (ACIB-3) The lines of
//! `running`, `launch` and `stop` ([`Running`], [`Launch`], [`Stop`]) name the one bundle asked
//! for, as the command line gave it, and hold of the phone's lists of applications and
//! processes only booleans and the pids of that bundle's own processes (the application's and,
//! apart, its extensions'), and (ACIB-3c, fix round 1) of the list a count, `located`, and a word
//! of [`ApplicationLocated`]; an error from the installation proxy's lookup on (ACIB-3c: the
//! lookup and the instruments server) is an [`ErrorKind`], a word of this program's own, never
//! its text. No member holds another application's name, bundle identifier, path or pid.
//! (ACIB-3b, second round) The line of `probe` ([`Probe`]) holds of
//! each way it asked the phone a word of its own lists, an error kind, a count of entries and
//! the counts of what the phone sent ([`PhoneSent`]), and nothing else; the line of `services
//! --service` ([`ServiceLine`]) what the discovery handshake says of one service, each name
//! cut as `redact.rs` cuts a service's name.

use crate::key::DeviceKey;
use crate::matching::Look;
use crate::redact;
use crate::stopping::Verdict;
use crate::tap::{PhoneSent, Reads};
use serde::{Serialize, Serializer};
use std::time::Duration;

/// The command a line answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandName {
    Devices,
    Info,
    Services,
    Images,
    Apps,
    Screenshot,
    Orientation,
    Running,
    Launch,
    Stop,
    Probe,
}

/// Why a command was refused or failed: this program's own words, not Runtime outcome codes.
/// Each word has one cause. (ACIB-3c, fix round 2) The tests alone derive a reader of it, from
/// which they take the closed list of these words (`tests::every_problem_word`); nothing reads
/// a word in the program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(serde::Deserialize))]
#[serde(rename_all = "snake_case")]
pub enum Problem {
    /// The command line is not one of the eleven commands with their options.
    CommandLineInvalid,
    /// The async runtime of this process could not be built.
    AsyncRuntimeFailed,
    /// The command did not finish within `--timeout-ms`; a screenshot's line names the `stage`
    /// it was waiting in, and (ACIB-3b) a line of `running`, `launch` and `stop` the `stage` it
    /// was waiting in and `stage_ms`, the time of each stage that finished before it. Of
    /// `orientation`, a reader that had not answered by then answers it, and so does each
    /// reader after it, which is not asked.
    DeadlinePassed,
    /// The environment variable `--key-salt-env` names is unset, not UTF-8, or holds fewer than
    /// 16 bytes (an empty one included) or more than 1024: refused before usbmuxd is asked.
    KeySaltUnusable,
    /// The same of the variable `--binding-salt-env` names.
    BindingSaltUnusable,
    /// No device usbmuxd lists has the key `--device-key` names under the salt of
    /// `--key-salt-env`.
    DeviceKeyUnknown,
    /// No connection to usbmuxd.
    UsbmuxdUnreachable,
    /// usbmuxd did not answer the device list.
    UsbmuxdListFailed,
    /// usbmuxd lists no device with the given number.
    DeviceNotListed,
    /// No connection to the device's lockdown service.
    LockdownUnreachable,
    /// A lockdown value could not be read, and the device did not refuse it by a word.
    LockdownReadFailed,
    /// A lockdown value is not of the type its key has.
    LockdownValueUnexpected,
    /// usbmuxd's answer to the pairing record request could not be read.
    PairRecordUnreadable,
    /// usbmuxd holds no pairing record of this host for the device.
    NotPaired,
    /// The device refused the lockdown session with this host's pairing record.
    SessionRefused,
    /// The device refused to start the service.
    ServiceRefused,
    /// The started service, or a remote service's port inside the tunnel, could not be
    /// connected to. (ACIB-3d) Of `stop`'s connection to the instruments server for its looks
    /// after the kills, the line names the error's [`ErrorKind`], keeps what was signalled and
    /// says nothing of whether the application runs (`reached` names `look_connection`).
    ServiceUnreachable,
    /// The CoreDeviceProxy tunnel or the TCP stack inside this process failed.
    TunnelFailed,
    /// The remote service discovery port inside the tunnel could not be connected to.
    RsdUnreachable,
    /// The remote service discovery handshake failed.
    RsdHandshakeFailed,
    /// The installation proxy did not answer the list of the user's applications.
    AppsLookupFailed,
    /// More bundle identifiers hold the text than `--max-matching` allows, or more than half of
    /// the user's applications do: `apps` names none of them, only the two counts.
    TooManyMatching,
    /// `--out` names something that exists already: refused before the device is asked.
    OutExists,
    /// No file could be made at `--out`: the place it names could not be looked at, the
    /// directory it is to be made in does not exist or is no directory (both found before the
    /// device is asked), or the file could not be created. Nothing was written.
    OutUnwritable,
    /// The file `--out` names was made, but the picture could not be written whole into it: the
    /// program removed that file (its own, made where nothing was), and the line's `out_left`
    /// says whether a file is left there, `true` only when the removal failed too (that file
    /// then holds a part of the picture or none of it).
    OutWriteFailed,
    /// The remote service discovery handshake lists no service of the way asked for (of
    /// `services --service`, no service of the name given; of `probe`, no application service;
    /// of `running`, `launch` and `stop`, no instruments server).
    ServiceNotListed,
    /// A remote service inside the tunnel took the connection, but its own first exchange
    /// failed (a CoreDevice service's XPC handshake, the instruments server's first message).
    /// Of `running`, `launch` and `stop` (ACIB-3c: the instruments server's first message), the
    /// line names the error's [`ErrorKind`] and no text; (ACIB-3d) of `stop`, also the first
    /// message on the connection of its looks after the kills, as for `service_unreachable`.
    ServiceHandshakeFailed,
    /// The instruments server did not open its screenshot channel.
    ChannelRefused,
    /// The service did not answer the screenshot request with a picture; no file is written.
    ScreenshotFailed,
    /// The picture ends before its header gives its size; the file holds it as the service gave
    /// it.
    PictureHeaderTruncated,
    /// The picture does not start with a header this program reads (PNG or JPEG); the file holds
    /// it as the service gave it.
    PictureHeaderUnknown,
    /// The SpringBoard service took the connection but did not answer the request of the
    /// interface orientation with one.
    OrientationFailed,
    /// (ACIB-3c) The installation proxy did not answer its lookup of the one bundle (`running`,
    /// `launch`, `stop`); the line names the error's [`ErrorKind`].
    ApplicationLookupFailed,
    /// (ACIB-3c) The instruments server did not answer its list of running processes (its device
    /// information channel, or the list on it); the line names the error's [`ErrorKind`].
    ProcessListFailed,
    /// The installation proxy's record of the bundle has no path that is read as a place (none,
    /// or one that is not an absolute path or a file URL): which processes are of it cannot be
    /// told.
    BundlePathUnreadable,
    /// (ACIB-3c, fix round 1) The instruments server's list of running processes was read whole,
    /// but no entry of it has a location read as a place (the list is empty, or each entry has a
    /// bare name there, or none): nothing in it can be matched to the bundle, so whether the
    /// application runs cannot be told. `running` answers it in place of `running` false;
    /// `stop`, at its first look, sends no kill request, and at a look after its kills gives no
    /// verdict.
    ProcessPathsUnreadable,
    /// (ACIB-3c, fix round 2) `stop`'s first look locates the application's own entries both at
    /// the bundle's directory and directly in it (what `running` answers as `application_located`
    /// `both`): no way of naming explains that (a list that names bundles has no entry one level
    /// inside the directory, one that names executables none at the directory itself; likeliest,
    /// an extension recorded at its host's directory), so the list does not tell which entry is
    /// the application itself, and no kill request is sent.
    ApplicationEntryAmbiguous,
    /// (ACIB-3c) The instruments server's process control did not answer the launch with a pid
    /// (its channel, or the launch on it); the line names the error's [`ErrorKind`]. Of any kind
    /// the application may have been started all the same (the library tells a refusal from no
    /// other answer that holds no pid): `running` tells.
    LaunchFailed,
    /// `stop`'s last look inside the command's deadline still lists a process of the
    /// application that `stop` sent a kill request for.
    StillRunning,
    /// `launch`: the installation proxy's lookup does not hold the bundle, so no launch was sent
    /// and the instruments server was not asked (the line's `installed` is false, as `running`
    /// and `stop` answer it).
    NotInstalled,
}

/// How a request failed, by this program's own words: what the library's error tells apart, and
/// `other` for everything else. Of `probe`, a request to the application service; (ACIB-3c) of
/// `running`, `launch` and `stop`, the installation proxy's lookup and the requests to the
/// instruments server. The library hands the application service's own refusal as the debug
/// text of its error dictionary, which can name other applications, their paths and their
/// processes, and the instruments server's list of processes is a person's: no text of these
/// errors is ever shown, only the kind. A locked phone has no kind of its own: the library maps
/// no refusal to one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The application service answered with its error in place of an output (the library's
    /// `CoreDevice(DeviceError)`): the phone refused the request. Only `probe` asks that
    /// service; the installation proxy's and the instruments server's errors have no such kind
    /// in the library.
    DeviceRefused,
    /// What the phone sent is not in the shape the library reads (`UnexpectedResponse`, `Plist`,
    /// `CoreDevice(MissingField)`, `CoreDevice(MalformedField)`), or (ACIB-3c) not in the shape
    /// this program reads of the instruments server's list of processes (`matching.rs`): an
    /// answer, or of the application service an HTTP/2 frame of it the library cannot read (a
    /// window update of another length, which it reports as an unexpected response too). Of the
    /// instruments server, among them: an answer to a channel's request that holds anything
    /// (the library takes no channel then), and an answer to the launch that holds no pid,
    /// which a refusal of the launch is too (the library reads only a pid). Of a `probe` way,
    /// whose outcome `connection_closed` says the phone ended the connection, the kind beside it
    /// is the one the library made of the end (this word, for a read that found the end:
    /// idevice 0.1.68, `xpc/http2/mod.rs` 70-73).
    AnswerUnreadable,
    /// The connection under the exchange failed (`Socket`). (ACIB-3c) Of the instruments
    /// server's connection, this is also what the library reports once its reader of that
    /// connection has stopped, whatever stopped it (the phone ended the connection, a read
    /// failed, or a message the phone sent could not be read: `services/dvt/remote_server.rs`
    /// 862-884, 1245-1250): it tells none of these apart.
    ConnectionFailed,
    /// The XPC layer over the connection failed (`Xpc`): only `probe`'s connections to the
    /// application service are XPC.
    XpcFailed,
    /// Any other error of the library.
    Other,
}

/// A refusal or failure: the word, and the library's error text where the library gave one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Failure {
    pub problem: Problem,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Failure {
    /// A failure this program finds itself; there is no library text.
    pub const fn own(problem: Problem) -> Self {
        Self {
            problem,
            error: None,
        }
    }

    /// A failure the library (or the OS beneath it) reported: its text, or its kind alone where
    /// the text could carry an identifier.
    pub fn library<E>(problem: Problem, error: &E) -> Self
    where
        E: std::fmt::Debug + std::fmt::Display,
    {
        let kind = redact::kind_of(&format!("{error:?}"));
        Self {
            problem,
            error: Some(redact::shown_error(&kind, &error.to_string())),
        }
    }
}

/// How usbmuxd reaches a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Link {
    Usb,
    Network,
    /// A connection usbmuxd names otherwise; its description is not shown.
    Unknown,
}

/// One device of `acib devices`: its usbmuxd session number and its connection; with
/// `--key-salt-env`, its key under that salt (`device_key`), and with `--binding-salt-env` too,
/// its key under the second salt (`binding_key`); nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceEntry {
    pub device: u32,
    pub connection: Link,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_key: Option<DeviceKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding_key: Option<DeviceKey>,
}

/// A lockdown value read in a session: the value, or the word the device refused it with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reading {
    Value(bool),
    Refused(String),
}

/// `acib devices`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Devices {
    pub command: CommandName,
    pub devices: Vec<DeviceEntry>,
}

/// `acib info --device <n>`: the values read without a session, whether this host is paired,
/// and the two values read in a session. A command that stops in its second half keeps what
/// its first half read and names its problem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Info {
    pub command: CommandName,
    pub device: u32,
    pub product_version: String,
    pub product_type: String,
    pub build_version: String,
    pub device_class: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paired: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_protected: Option<Reading>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub developer_mode: Option<Reading>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<Problem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Info {
    /// This answer, stopped by `failure`.
    pub fn stopped(mut self, failure: Failure) -> Self {
        self.problem = Some(failure.problem);
        self.error = failure.error;
        self
    }
}

/// One remote service of the handshake: its name, the port it is served on inside the tunnel
/// and the entitlement the handshake names for it (which tells whether it needs the trusted
/// tunnel or is served in the untrusted one too).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceEntry {
    pub name: String,
    pub port: u16,
    pub entitlement: String,
}

/// `acib services --device <n>`: the remote services, sorted by name, and their count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Services {
    pub command: CommandName,
    pub device: u32,
    pub count: usize,
    pub services: Vec<ServiceEntry>,
}

/// (ACIB-3b, second round) What the remote service discovery handshake says of one service
/// (idevice 0.1.68, `services/rsd.rs` 13-24 and 77-104): its name, its port inside the tunnel,
/// the entitlement it names, whether its properties say it uses RemoteXPC (`false` too when they
/// do not say), its `ServiceVersion` and its `Features`, each of the last two only when the
/// handshake gives it, the features in the handshake's order. A name, an entitlement or a feature
/// that could carry an identifier is `withheld` ([`redact::shown_name`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceDetail {
    pub name: String,
    pub port: u16,
    pub entitlement: String,
    pub uses_remote_xpc: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_version: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub features: Option<Vec<String>>,
}

impl ServiceDetail {
    /// The detail of the service `name`, every text of it cut as a service's name is.
    pub fn of(
        name: &str,
        port: u16,
        entitlement: &str,
        uses_remote_xpc: bool,
        service_version: Option<i64>,
        features: Option<&[String]>,
    ) -> Self {
        Self {
            name: redact::shown_name(name),
            port,
            entitlement: redact::shown_name(entitlement),
            uses_remote_xpc,
            service_version,
            features: features.map(|features| {
                features
                    .iter()
                    .map(String::as_str)
                    .map(redact::shown_name)
                    .collect()
            }),
        }
    }
}

/// `acib services --device <n> --service <name>`: what the handshake says of that one service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceLine {
    pub command: CommandName,
    pub device: u32,
    pub service: ServiceDetail,
}

/// A way `acib probe` asks the phone (ACIB-3b, second round), by this program's own word, in the
/// order it asks them ([`ProbeWay::ALL`]), each on a connection of its own inside the one tunnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeWay {
    /// CoreDevice's screen capture service, which answers on the phone: the library's RemoteXPC
    /// handshake on it and nothing asked; what such a service sends unasked.
    ListenScreenCapture,
    /// The application service: the same handshake and nothing asked; what it sends unasked (the
    /// baseline of the three ways after it).
    Listen,
    /// The library's `list_apps`: the feature `listapps`, protocol version 0, its answer read on
    /// the reply channel.
    ListApps,
    /// The streamed list: the feature `streamapplist`, protocol version 2, a side channel named
    /// in its input, its answer read on the root channel until the stream's end.
    StreamApps,
    /// The library's `list_processes`: the feature `listprocesses`, protocol version 0, its
    /// answer read on the reply channel.
    ListProcesses,
    /// (ACIB-3b, third round) The list of applications in the form of the screenshot's request,
    /// which the phone answers: the library's CoreDevice client and its
    /// `invoke_with_plist_action`, the feature `listapps` with the action the screenshot's rule
    /// gives for it (`com.apple.coredevice.action.listapps`), protocol version 2, the flags of
    /// `list_apps`, its answer read on the reply channel.
    ListAppsAction,
    /// (ACIB-3b, third round) The list of processes in the same form: the feature
    /// `listprocesses` with the action `com.apple.coredevice.action.listprocesses`, protocol
    /// version 2, an empty input, the reply channel.
    ListProcessesAction,
}

impl ProbeWay {
    /// Every way, in the order `probe` asks them.
    pub const ALL: [Self; 7] = [
        Self::ListenScreenCapture,
        Self::Listen,
        Self::ListApps,
        Self::StreamApps,
        Self::ListProcesses,
        Self::ListAppsAction,
        Self::ListProcessesAction,
    ];

    /// The channels the library reads this way's answer on.
    pub const fn reads(self) -> Reads {
        match self {
            Self::ListenScreenCapture | Self::Listen => Reads::Both,
            Self::ListApps
            | Self::ListProcesses
            | Self::ListAppsAction
            | Self::ListProcessesAction => Reads::Reply,
            Self::StreamApps => Reads::Root,
        }
    }
}

/// What one way of `acib probe` came to, by this program's own words, one cause each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeOutcome {
    /// The library returned what the way asks for: a list (`entries`, its count), or, for a
    /// `listen`, a message with a body the phone sent unasked. (ACIB-3b, third round) Of an
    /// action way, the service's output in the shape the library's own `list_apps` (a list) or
    /// `list_processes` (a dictionary whose `processTokens` is a list) reads; an output of
    /// another shape is `failed` with `answer_unreadable`.
    Answered,
    /// The service answered with its error in place of an output (`device_refused`).
    Refused,
    /// The library returned another error (`error_kind`); the phone did not end the connection.
    Failed,
    /// The phone ended the connection or reset a stream (a read found its end, a GOAWAY or a
    /// RST_STREAM frame), with the kind of the error the library made of it.
    ConnectionClosed,
    /// The wait passed; beyond what the `listen` of the service counted, a message with a body
    /// came on a channel the way does not read.
    AnswerOnOtherChannel,
    /// The wait passed; a frame or a message was started and is not whole, or messages with a
    /// body came on the channel read without the library returning (a streamed list without its
    /// end).
    AnswerIncomplete,
    /// The wait passed; beyond what the `listen` counted, a message flagged as a reply came on
    /// the channel read with an empty dictionary or no body at all, which the library skips.
    AnswerWithoutBody,
    /// The wait passed; the phone sent bytes on the connection, but none of the above.
    NoAnswer,
    /// The wait passed; the phone sent not one byte on the connection, not even HTTP/2 settings.
    Silent,
    /// The handshake lists no such service (only the screen capture service can be missing:
    /// without the application service the probe is refused).
    ServiceNotListed,
    /// The connection to the service's port inside the tunnel failed (`error_kind`).
    ServiceUnreachable,
    /// The library's RemoteXPC handshake on the connection failed (`error_kind`).
    HandshakeFailed,
    /// The command's deadline left no time to ask.
    NotAsked,
}

/// What one way of `acib probe` came to: its word, the error's kind where the library gave an
/// error, the count of the list's entries where it answered one, how long it took from its
/// connection on, and what the phone sent on that connection (none when no connection was made).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProbeResult {
    pub way: ProbeWay,
    pub outcome: ProbeOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<ErrorKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entries: Option<usize>,
    pub ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone_sent: Option<PhoneSent>,
}

impl ProbeResult {
    /// A way that made no connection: `outcome`, of `kind` where the library gave an error.
    pub fn unconnected(way: ProbeWay, outcome: ProbeOutcome, kind: Option<ErrorKind>) -> Self {
        Self {
            way,
            outcome,
            error_kind: kind,
            entries: None,
            ms: 0,
            phone_sent: None,
        }
    }
}

/// `acib probe --device <n> [--wait-ms <ms>]`, read-only: each way of [`ProbeWay::ALL`] in turn,
/// each with a wait of its own (`wait_ms`, within the command's deadline).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Probe {
    pub command: CommandName,
    pub device: u32,
    pub wait_ms: u64,
    pub ways: Vec<ProbeResult>,
}

/// Whether a developer image is mounted, as far as the image mounter's answers tell: `true`,
/// `false` only when the listing of what is mounted answered an empty list and no query failed,
/// else `"unknown"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mounted {
    Yes,
    No,
    Unknown,
}

impl Serialize for Mounted {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Yes => serializer.serialize_bool(true),
            Self::No => serializer.serialize_bool(false),
            Self::Unknown => serializer.serialize_str("unknown"),
        }
    }
}

impl Mounted {
    /// The judgement of the queries: an image found by a lookup or a listing that is not empty
    /// is `Yes`; an empty listing is `No` only when no query failed and no lookup found an
    /// image; everything else is `Unknown` (an empty listing beside a failed query or a found
    /// image, no listing and no found image).
    pub fn judged(queries: &[ImageQuery]) -> Self {
        let found = queries
            .iter()
            .any(|query| query.answer == ImageAnswer::Found);
        let failed = queries
            .iter()
            .any(|query| query.answer == ImageAnswer::Failed);
        let listed = queries
            .iter()
            .find(|query| query.answer == ImageAnswer::Listed)
            .and_then(|query| query.listed.as_ref())
            .map(Vec::len);
        match (listed, found, failed) {
            (Some(0), false, false) => Self::No,
            (Some(0), _, _) | (None, false, _) => Self::Unknown,
            (Some(_), _, _) | (None, true, _) => Self::Yes,
        }
    }
}

/// A read-only query of the image mounter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageQueryName {
    /// The listing of what is mounted (`CopyDevices`).
    CopyDevices,
    /// The lookup of the image of one type (`LookupImage`).
    LookupImage,
}

/// What a query answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageAnswer {
    /// The listing answered its list (`listed`, empty when nothing is mounted).
    Listed,
    /// The lookup answered a signature (never shown).
    Found,
    /// The lookup answered without a signature.
    NotFound,
    /// The query failed; `error_kind` is the library's error kind.
    Failed,
}

/// One entry of the listing: the names of its members, sorted, when it is a dictionary (`null`
/// when it is not); no value of a member is shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListedImage {
    pub members: Option<Vec<String>>,
}

impl ListedImage {
    /// The entry whose members are named `members` (`None` for an entry that is no dictionary).
    pub fn of<'a, I>(members: Option<I>) -> Self
    where
        I: IntoIterator<Item = &'a str>,
    {
        Self {
            members: members.map(|names| {
                let mut shown: Vec<String> = names.into_iter().map(redact::shown_name).collect();
                shown.sort();
                shown
            }),
        }
    }
}

/// One query of `acib images` and its answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImageQuery {
    pub query: ImageQueryName,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_type: Option<String>,
    pub answer: ImageAnswer,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listed: Option<Vec<ListedImage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
}

impl ImageQuery {
    /// The listing's answer.
    pub fn listing(listed: Vec<ListedImage>) -> Self {
        Self {
            query: ImageQueryName::CopyDevices,
            image_type: None,
            answer: ImageAnswer::Listed,
            listed: Some(listed),
            error_kind: None,
        }
    }

    /// A lookup's answer for `image_type`.
    pub fn lookup(image_type: &str, found: bool) -> Self {
        Self {
            query: ImageQueryName::LookupImage,
            image_type: Some(image_type.to_owned()),
            answer: if found {
                ImageAnswer::Found
            } else {
                ImageAnswer::NotFound
            },
            listed: None,
            error_kind: None,
        }
    }

    /// A query that failed: the library's error kind alone, never its text.
    pub fn failed<E: std::fmt::Debug>(
        query: ImageQueryName,
        image_type: Option<&str>,
        error: &E,
    ) -> Self {
        Self {
            query,
            image_type: image_type.map(str::to_owned),
            answer: ImageAnswer::Failed,
            listed: None,
            error_kind: Some(redact::kind_of(&format!("{error:?}"))),
        }
    }
}

/// `acib images --device <n>`: what each read-only query of the image mounter answered, and
/// the judgement `mounted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Images {
    pub command: CommandName,
    pub device: u32,
    pub mounted: Mounted,
    pub queries: Vec<ImageQuery>,
}

/// `acib apps --device <n> --contains <text> [--max-matching <m>]`: how many bundle identifiers
/// of the user's applications hold the text (without regard to case) and how many user
/// applications there are; and those identifiers, sorted, only when the matching ones are at
/// most `<m>` and at most half of the user's applications:
/// `2 * matching_count <= user_applications`. Otherwise no identifier is named and the line
/// answers `too_many_matching` with the two counts alone, so neither the whole list nor most of
/// it is ever printed (all of them matching is more than half: a device whose one user
/// application matches names none). Nothing else of a record is shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Apps {
    pub command: CommandName,
    pub device: u32,
    pub contains: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matching: Option<Vec<String>>,
    pub matching_count: usize,
    pub user_applications: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<Problem>,
}

impl Apps {
    /// The answer for the bundle identifiers of all user applications, naming the matching ones
    /// only when they are at most `max_matching` and at most half of them.
    pub fn of<'a>(
        device: u32,
        contains: &str,
        max_matching: usize,
        identifiers: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        let wanted = contains.to_lowercase();
        let mut user_applications = 0;
        let mut held = Vec::new();
        for identifier in identifiers {
            user_applications += 1;
            if identifier.to_lowercase().contains(wanted.as_str()) {
                held.push(redact::shown_bundle_identifier(identifier));
            }
        }
        let matching_count = held.len();
        let named = matching_count <= max_matching && 2 * matching_count <= user_applications;
        let (matching, problem) = if named {
            held.sort();
            (Some(held), None)
        } else {
            (None, Some(Problem::TooManyMatching))
        };
        Self {
            command: CommandName::Apps,
            device,
            contains: contains.to_owned(),
            matching,
            matching_count,
            user_applications,
            problem,
        }
    }
}

/// A way of `acib screenshot` to take the picture, by this program's own word (`--via`): each
/// goes through the tunnel of iOS 17.4 and later to a remote service the handshake lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Way {
    /// CoreDevice's screen capture service (`com.apple.coredevice.screencaptureservice`),
    /// asked for a PNG picture of the main display.
    ScreenCapture,
    /// The instruments server (`com.apple.instruments.dtservicehub`) and its screenshot
    /// channel.
    Instruments,
}

impl Way {
    /// Every way, in the order the README names them.
    pub const ALL: [Self; 2] = [Self::ScreenCapture, Self::Instruments];

    /// The way's word, as `--via` takes it and the line names it.
    pub const fn word(self) -> &'static str {
        match self {
            Self::ScreenCapture => "screen_capture",
            Self::Instruments => "instruments",
        }
    }

    /// The way `word` names, if it names one (exactly, case included).
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|way| way.word() == word)
    }
}

/// (ACIB-3d) A way of `acib stop` to send its kill requests, by this program's own word
/// (`--via`): each sends the instruments server's `killPid:` once per process of the application,
/// on its process control channel, in a form of its own (README). On a phone with iOS 27.0.1
/// (2026-11-19) the library's own form ended nothing, sent as the library sends it (`library`) and
/// with its answer waited for (`primitive`); (fix round 1) `primitive`, the form that is known, is
/// the default, and `archived` is sent only when `--via` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StopWay {
    /// The pid as a 32-bit number, the library's own form of `killPid:`
    /// (`services/dvt/process_control.rs` 286-296), an answer asked for and waited for.
    Primitive,
    /// The pid archived as a number (the form in which the library's own device information
    /// client sends a pid: idevice 0.1.68, `services/dvt/device_info.rs` 95-122), an answer asked
    /// for and waited for. How the phone's server reads this form of `killPid:` the library's
    /// source does not show (README): it is never the default.
    Archived,
    /// The library's own `kill_app`, unchanged: the pid as a 32-bit number, no answer asked for.
    Library,
}

impl StopWay {
    /// Every way, in the order the README names them.
    pub const ALL: [Self; 3] = [Self::Primitive, Self::Archived, Self::Library];

    /// The way's word, as `--via` takes it and the line names it.
    pub const fn word(self) -> &'static str {
        match self {
            Self::Primitive => "primitive",
            Self::Archived => "archived",
            Self::Library => "library",
        }
    }

    /// The way `word` names, if it names one (exactly, case included).
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|way| way.word() == word)
    }

    /// Whether the way asks the server to answer each kill request (and waits for it).
    pub const fn asks_answers(self) -> bool {
        matches!(self, Self::Primitive | Self::Archived)
    }
}

/// The way a refused line's command was to go (`via`): a screenshot's way or (ACIB-3d) a `stop`'s,
/// each named by its own word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Via {
    Picture(Way),
    Stop(StopWay),
}

/// A picture's format as its own header says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PictureFormat {
    Png,
    Jpeg,
}

/// How long the screenshot request alone took and how long the whole command took, in
/// milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Elapsed {
    pub request: u64,
    pub command: u64,
}

impl Elapsed {
    /// The two durations in whole milliseconds.
    pub fn of(request: Duration, command: Duration) -> Self {
        Self {
            request: whole_milliseconds(request),
            command: whole_milliseconds(command),
        }
    }
}

/// A duration in whole milliseconds.
pub fn whole_milliseconds(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// The step a command was waiting in when its deadline passed (`stage` beside
/// `deadline_passed`), by this program's own words. A screenshot goes through the first five,
/// in their order (the file is written after the last of them, and no deadline passes while it
/// is). (ACIB-3b, ACIB-3c) `running`, `launch` and `stop` go through the ones after them, and
/// `server_first_message`, in the order the code takes them (each of them `usbmuxd`,
/// `application_lookup`, `proxy_tunnel`, `discovery_handshake`, `service_connection` and
/// `server_first_message`; then `running` `process_list`, `launch` `launch_request`, and `stop`
/// `process_list`, `kill_requests`, (ACIB-3d) `kill_replies` for a way that asks for answers,
/// `look_connection` and `looks_after_kills`), and name the time of each that
/// finished in `stage_ms` ([`StageTimes`]). A stage the library gives no way to interrupt is
/// named as that stage. A stage says where a command waited, not why. Where one of the first
/// five covers what others split (`session`, `tunnel`, `service_handshake`), the screenshot
/// keeps its word and `running`, `launch` and `stop` use theirs, so that one word has one
/// meaning; `server_first_message` is one step, the same in both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// The session: usbmuxd's device list, the pairing record, and the lockdown session that
    /// starts the tunnel's service (CoreDeviceProxy).
    Session,
    /// The tunnel through CoreDeviceProxy and the remote service discovery handshake through
    /// it.
    Tunnel,
    /// The connection inside the tunnel to the way's remote service and the service's own
    /// handshake (the screen capture service's XPC handshake).
    ServiceHandshake,
    /// The instruments server's first message (the capabilities it publishes), read before
    /// anything is asked of it: the screenshot's `instruments` way and (ACIB-3c) `running`,
    /// `launch` and `stop` wait in it.
    ServerFirstMessage,
    /// The picture's request; for `instruments`, the request of its screenshot channel and then
    /// of the picture.
    PictureRequest,
    /// (ACIB-3c) `running`, `launch` and `stop`: usbmuxd's device list and the pairing record
    /// usbmuxd holds for the device (both asked of usbmuxd).
    Usbmuxd,
    /// (ACIB-3c) `running`, `launch` and `stop`: a lockdown session with that record, the
    /// installation proxy started in it, and its lookup of the one bundle.
    ApplicationLookup,
    /// `running`, `launch` and `stop`: (ACIB-3c) the lockdown session that starts the tunnel's
    /// service, CoreDeviceProxy, then CoreDeviceProxy's own exchange on that connection (the
    /// tunnel's parameters), the TCP stack inside this process over it, and the connection
    /// inside the tunnel to the remote service discovery port.
    ProxyTunnel,
    /// `running`, `launch` and `stop`: the remote service discovery handshake, which answers the
    /// services and their ports.
    DiscoveryHandshake,
    /// `running`, `launch` and `stop`: the connection inside the tunnel to the port the
    /// handshake names for (ACIB-3c) the instruments server, `com.apple.instruments.dtservicehub`.
    ServiceConnection,
    /// `running` and `stop`: (ACIB-3c) the instruments server's device information channel,
    /// opened by a request of its own, and its list of running processes (for `stop`, the first
    /// look, before any kill).
    ProcessList,
    /// `launch`: (ACIB-3c) the instruments server's process control channel, opened by a request
    /// of its own, and the launch request and its answer.
    LaunchRequest,
    /// (ACIB-3c) `stop`: the process control channel, whose request's answer the library waits
    /// for, and one kill request per process of the application, in the form of the way
    /// (ACIB-3d, `--via`), each written and flushed; of the way `library` no answer is asked for.
    KillRequests,
    /// (ACIB-3d) `stop`, a way that asks for answers (`primitive`, `archived`): the server's answer
    /// to each kill request, read on the process control channel, waited for up to `--wait-ms`
    /// (fix round 1: each paired with its request by the number it carries).
    KillReplies,
    /// (ACIB-3d) `stop`: a connection of its own to the instruments server, inside the same
    /// tunnel, for the looks after the kills, and its first message.
    LookConnection,
    /// (ACIB-3c) `stop`: a device information channel of its own (ACIB-3d: on the connection of
    /// `look_connection`) and the looks at the list of running processes after the kills, and the
    /// waits between them.
    LooksAfterKills,
}

/// How long each finished stage of `running`, `launch` or `stop` took, in whole
/// milliseconds, by its stage's word, in the order the command took them (`stage_ms` of
/// `running`, `launch` and `stop`, and beside their `deadline_passed`, where the stage it was
/// waiting in is not among them; a member of its own, as a screenshot's `elapsed_ms` is of
/// another shape). Nothing but a stage's word and a number is in it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StageTimes(Vec<(Stage, u64)>);

impl StageTimes {
    /// The times of `finished`, each stage with how long it took.
    pub fn of(finished: &[(Stage, Duration)]) -> Self {
        Self(
            finished
                .iter()
                .map(|(stage, took)| (*stage, whole_milliseconds(*took)))
                .collect(),
        )
    }
}

impl Serialize for StageTimes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (stage, milliseconds) in &self.0 {
            map.serialize_entry(stage, milliseconds)?;
        }
        map.end()
    }
}

/// `acib screenshot --device <n> --out <file|-> [--via <way>]`: the way used, the picture's
/// format, width and height as its header gives them, its length in bytes, the two durations
/// and the file it was written to, or `-` when its `bytes` bytes follow this line on the
/// standard output. A picture whose header cannot be read keeps its length and its file (or its
/// bytes after the line), names no format or size and answers its header's word (status 1).
/// Nothing of the picture's content is a member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Screenshot {
    pub command: CommandName,
    pub device: u32,
    pub via: Way,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<PictureFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    pub bytes: usize,
    pub elapsed_ms: Elapsed,
    pub out: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<Problem>,
}

impl Screenshot {
    /// The answer for `picture`, written to `out`: only its header and its length are read.
    pub fn of(device: u32, via: Way, picture: &[u8], elapsed_ms: Elapsed, out: &str) -> Self {
        let (format, width, height, problem) = match crate::picture::read_header(picture) {
            Ok(header) => (
                Some(header.format),
                Some(header.width),
                Some(header.height),
                None,
            ),
            Err(problem) => (None, None, None, Some(problem)),
        };
        Self {
            command: CommandName::Screenshot,
            device,
            via,
            format,
            width,
            height,
            bytes: picture.len(),
            elapsed_ms,
            out: out.to_owned(),
            problem,
        }
    }
}

/// A way of `acib orientation` to read the interface orientation, by this program's own word
/// (`--via`): the library's SpringBoard services client and its one read of the orientation
/// (`getInterfaceOrientation`), reached in two ways. The library's other orientation service,
/// CoreDevice's `com.apple.coredevice.devicecontrol`, only rotates the device and answers the
/// orientation after the turn: it is not built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reader {
    /// The lockdown service `com.apple.springboardservices`, started in a session with this
    /// host's pairing record; no tunnel.
    Springboard,
    /// The same service's remote shim `com.apple.springboardservices.shim.remote`, inside the
    /// tunnel of iOS 17.4 and later, as the remote service discovery handshake lists it.
    SpringboardShim,
}

impl Reader {
    /// Every reader, in the order `orientation` asks them without `--via`.
    pub const ALL: [Self; 2] = [Self::Springboard, Self::SpringboardShim];

    /// The reader's word, as `--via` takes it and the line names it.
    pub const fn word(self) -> &'static str {
        match self {
            Self::Springboard => "springboard",
            Self::SpringboardShim => "springboard_shim",
        }
    }

    /// The reader `word` names, if it names one (exactly, case included).
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|reader| reader.word() == word)
    }
}

/// The interface orientation as the phone reports it, by this program's own words: one for
/// each value of the library's `InterfaceOrientation`, as the library documents it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    /// The phone answered 0, or a value the library does not name: "unknown or cannot be
    /// determined".
    Unknown,
    /// 1: portrait, the normal vertical way.
    Portrait,
    /// 2: portrait upside down.
    PortraitUpsideDown,
    /// 3: landscape with the home button on the right (the notch to the left).
    LandscapeRight,
    /// 4: landscape with the home button on the left (the notch to the right).
    LandscapeLeft,
}

/// What one reader of `acib orientation` answered: the orientation, or its problem word and,
/// where the library gave one, its error text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    pub via: Reader,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orientation: Option<Orientation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<Problem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Candidate {
    /// The reader `via` read `orientation`.
    pub fn answered(via: Reader, orientation: Orientation) -> Self {
        Self {
            via,
            orientation: Some(orientation),
            problem: None,
            error: None,
        }
    }

    /// The reader `via` did not read the orientation, for `failure`.
    pub fn failed(via: Reader, failure: Failure) -> Self {
        Self {
            via,
            orientation: None,
            problem: Some(failure.problem),
            error: failure.error,
        }
    }
}

/// `acib orientation --device <n> [--via <reader>]`: what each reader asked answered, in the
/// order they were asked (the one `--via` names, or each of [`Reader::ALL`] in turn). The command
/// is answered (status 0) only when every reader asked answered an orientation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Orientations {
    pub command: CommandName,
    pub device: u32,
    pub candidates: Vec<Candidate>,
}

/// (ACIB-3c, fix round 1) How the application's own entries of the instruments server's list were
/// located in its bundle's directory (`application_located`, beside `located`, only when the
/// application itself was found), by this program's own words: one word for each way a list can
/// name the application (`matching.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationLocated {
    /// Each of its entries lies directly in the bundle's directory (the list names executables).
    DirectlyInside,
    /// Each of its entries is the bundle's directory itself (the list names bundles).
    AtDirectory,
    /// It has entries of both kinds. (Fix round 2) No way of naming explains that: `running`
    /// answers it (the application runs either way); `stop`, at its first look, sends nothing and
    /// answers `application_entry_ambiguous`.
    Both,
}

/// `acib running --device <n> --bundle <id>`, read-only: of the one bundle asked for (`bundle`,
/// as the command line gave it), whether (ACIB-3c) the installation proxy's lookup holds it
/// (`installed`), whether the application itself runs (`running`: a process whose executable
/// lies directly in the bundle's directory, or, fix round 1, whose location is that directory),
/// the pids of those processes, ascending (`pids`), and apart the pids of the bundle's
/// extensions' processes (`extension_pids`, which do not make it run; `matching.rs`); (fix
/// round 1) how many entries of the list have a location read as a place (`located`, at least 1
/// wherever it is named: a list with none is `process_paths_unreadable`, never `running` false;
/// fix round 2: the line of a bundle that is not installed, which was not looked for, and a line
/// stopped by a problem name none), and how the application's entries were located when it runs
/// (`application_located`). A line
/// stopped by a problem keeps what was read before it (`installed`), and names the error's kind
/// where the service gave an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Running {
    pub command: CommandName,
    pub device: u32,
    pub bundle: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pids: Option<Vec<u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extension_pids: Option<Vec<u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub located: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application_located: Option<ApplicationLocated>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<Problem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<ErrorKind>,
    /// (ACIB-3b) How long each stage took ([`StageTimes`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage_ms: Option<StageTimes>,
}

impl Running {
    /// The answer for `bundle` on `device`, before anything is read.
    pub fn asked(device: u32, bundle: &str) -> Self {
        Self {
            command: CommandName::Running,
            device,
            bundle: bundle.to_owned(),
            installed: None,
            running: None,
            pids: None,
            extension_pids: None,
            located: None,
            application_located: None,
            problem: None,
            error_kind: None,
            stage_ms: None,
        }
    }

    /// The installation proxy's lookup does not hold the bundle: no process of it is looked for.
    pub fn not_installed(self) -> Self {
        Self {
            installed: Some(false),
            running: Some(false),
            pids: Some(Vec::new()),
            extension_pids: Some(Vec::new()),
            ..self
        }
    }

    /// The bundle is installed (and what follows was not read).
    pub fn installed(self) -> Self {
        Self {
            installed: Some(true),
            ..self
        }
    }

    /// The bundle is installed and `look` is what the list says of it: it runs when the
    /// application itself has a process; its extensions' are named apart, and so are `located`
    /// and how the application was located. (Fix round 1) A list in which no entry has a
    /// location read as a place tells nothing of the bundle: the line is then
    /// `process_paths_unreadable`, never `running` false.
    pub fn found(self, look: Look) -> Self {
        if look.located == 0 {
            return self
                .installed()
                .failed(Problem::ProcessPathsUnreadable, None);
        }
        Self {
            installed: Some(true),
            running: Some(!look.pids.application.is_empty()),
            pids: Some(look.pids.application),
            extension_pids: Some(look.pids.extensions),
            located: Some(look.located),
            application_located: look.application_located,
            ..self
        }
    }

    /// Stopped by `problem`, of `kind` where the service gave an error.
    pub fn failed(self, problem: Problem, kind: Option<ErrorKind>) -> Self {
        Self {
            problem: Some(problem),
            error_kind: kind,
            ..self
        }
    }
}

/// `acib launch --device <n> --bundle <id> [--relaunch]`: the bundle asked for, whether it was
/// to be ended first (`relaunch`), whether (ACIB-3c) the installation proxy's lookup holds it
/// (`installed`, looked up before any launch is sent) and the pid the instruments server's
/// process control answered the launch with (`pid`); or `not_installed` (no launch sent),
/// `launch_failed` with the error's kind, or a word of the steps before (the lookup, the
/// instruments server's first message).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Launch {
    pub command: CommandName,
    pub device: u32,
    pub bundle: String,
    pub relaunch: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<Problem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<ErrorKind>,
    /// (ACIB-3b) How long each stage took ([`StageTimes`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage_ms: Option<StageTimes>,
}

impl Launch {
    /// The answer for `bundle` on `device`, before anything is read.
    pub fn asked(device: u32, bundle: &str, relaunch: bool) -> Self {
        Self {
            command: CommandName::Launch,
            device,
            bundle: bundle.to_owned(),
            relaunch,
            installed: None,
            pid: None,
            problem: None,
            error_kind: None,
            stage_ms: None,
        }
    }

    /// The installation proxy's lookup holds the bundle (and the launch is sent).
    pub fn installed(self) -> Self {
        Self {
            installed: Some(true),
            ..self
        }
    }

    /// The installation proxy's lookup does not hold the bundle: no launch is sent.
    pub fn not_installed(self) -> Self {
        Self {
            installed: Some(false),
            problem: Some(Problem::NotInstalled),
            ..self
        }
    }

    /// The instruments server's process control answered the launch with the pid `pid`.
    pub fn launched(self, pid: u32) -> Self {
        Self {
            pid: Some(pid),
            ..self
        }
    }

    /// Stopped by `problem`, of `kind` where the service gave an error.
    pub fn failed(self, problem: Problem, kind: Option<ErrorKind>) -> Self {
        Self {
            problem: Some(problem),
            error_kind: kind,
            ..self
        }
    }
}

/// What `stop` sent: the pids of the application's processes a kill request was made for,
/// ascending (`signalled`; ACIB-3c: the instruments server's `killPid:`, whose signal the
/// library does not name), and each request that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signals {
    pub signalled: Vec<u32>,
    pub errors: Vec<SignalError>,
}

/// A kill request that failed (ACIB-3c: the process control channel was not opened, or the
/// request could not be written; ACIB-3d: of a way that asks for answers, also the wait for its
/// answer failed): the pid it was for and the error's kind, a fact beside `stop`'s verdict (which
/// comes from the looks, not from it), and (ACIB-3d, fix round 1) of a wait for its answer that
/// failed, why, as far as the library's error value tells (`reply_failure`; none when the request
/// itself failed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SignalError {
    pub pid: u32,
    pub error_kind: ErrorKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_failure: Option<ReplyFailure>,
}

impl SignalError {
    /// The request for `pid` failed (the channel not opened, the request not written) with an
    /// error of `error_kind`.
    pub fn request(pid: u32, error_kind: ErrorKind) -> Self {
        Self {
            pid,
            error_kind,
            reply_failure: None,
        }
    }

    /// (ACIB-3d, fix round 1) The request for `pid` was written and the wait for its answer
    /// failed with an error of `error_kind`, for the reason `failure`.
    pub fn reply(pid: u32, error_kind: ErrorKind, failure: ReplyFailure) -> Self {
        Self {
            pid,
            error_kind,
            reply_failure: Some(failure),
        }
    }
}

/// (ACIB-3d, fix round 1) Why the wait for the answer to a kill request failed, by this program's
/// own words, read from the library's error value alone (never from a message's bytes, nothing of
/// the device or of another application: `bridge.rs`, `reply_failure`). The library hands a caller
/// that waits on a channel one error for every way its connection can end (README): the phone
/// ending the connection, its reset, a read that failed and a message the library could not read
/// are that one error, `reader_stopped`; a wait whose time passed is no failure but the answer
/// `not_in_time` ([`KillAnswer`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyFailure {
    /// The library's reader of the connection has stopped (its `Socket` error of the kind
    /// `BrokenPipe`, "remote server connection closed": idevice 0.1.68,
    /// `services/dvt/remote_server.rs` 842-849, 1245-1250), for any of the reasons it stops for,
    /// which it traces and drops (853-896).
    ReaderStopped,
    /// The channel is no longer the library's (its `UnknownChannel`: 828-833), which it removes
    /// only when the phone cancels it (`_channelCanceled:`, 938-960, 1234-1239).
    ChannelCancelled,
    /// Any other error of the library.
    Other,
}

/// (ACIB-3d) What the instruments server answered a kill request of a way that asks for answers,
/// by this program's own words; nothing of an answer's payload is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KillAnswer {
    /// An answer that holds no payload.
    WithoutPayload,
    /// An answer that holds a payload, which is not read (what it holds the library does not
    /// say of `killPid:`).
    WithPayload,
    /// No answer of this request came within `--wait-ms` (fix round 1: an answer is this
    /// request's only when it carries the number the library gave the request; `bridge.rs`,
    /// `kill_answers`); the looks went on.
    NotInTime,
}

/// (ACIB-3d) The answer to the kill request of one pid (`replies`): the pid, what was answered
/// and, of an answer that came, its message type: the first byte of its payload header as the
/// library reads it (idevice 0.1.68, `services/dvt/message.rs` 622-632), which the library names
/// among DISPATCH, OBJECT, OK, ERROR and DATA (message.rs 98) without their numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct KillReply {
    pub pid: u32,
    pub reply: KillAnswer,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_type: Option<u8>,
}

/// (ACIB-3d) Of each stage of `stop` after its first look, whether the command reached it
/// (entered it: the line's `problem` names what stopped it there), on a line of a `stop` that went
/// past its first look (`reached`). `kill_replies` only of a way that asks for answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Reached {
    pub kill_requests: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kill_replies: Option<bool>,
    pub look_connection: bool,
    pub looks_after_kills: bool,
}

/// `acib stop --device <n> --bundle <id>`: of the bundle asked for, whether it is installed,
/// whether the application itself ran at the first look (`was_running`), the pids of its
/// processes a kill request was made for (`signalled`), each request that failed, with its kind
/// (`signal_errors`, only when there are any), the pids of the
/// application the last look lists (`remaining`), whether any runs (`running`) and the pids of
/// the bundle's extensions' processes the last look lists (`extension_pids`: `stop` sends them
/// nothing and leaves them, which is no failure), and (fix round 1) of that look, as `running`
/// names them, `located` and `application_located`. A look that locates no process (no entry
/// with a location read as a place) is `process_paths_unreadable` (status 1): at the first look
/// no kill request is sent, after the kills there is no verdict. (Fix round 2) A first look that
/// locates the application's entries both at the bundle's directory and directly in it is
/// `application_entry_ambiguous` (status 1), and no kill request is sent. An application that
/// does not run (or is not installed) is answered at the first look: no signal, `signalled` and
/// `remaining` empty. The
/// last look inside the deadline still listing a process it signalled is `still_running`
/// (status 1), whatever became of the requests; a process it lists that was not signalled (one
/// started after the first look) is in `remaining` and `running`, and is not `still_running`. A
/// line stopped by another problem keeps what was read and done before it. (ACIB-3d) Every line of
/// `stop` the program prints names the way of its kill requests (`via`, set as the line is printed:
/// [`Line::on_stop_way`]); a line that went past the first look names, of a way that asks for
/// answers, what each kill request was answered (`replies`, only when there are any) and, of each
/// stage after the first look, whether it was reached (`reached`). (Fix round 1) A request whose
/// answer could not be waited for to its end names in `signal_errors` why, as far as the library's
/// error tells ([`ReplyFailure`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Stop {
    pub command: CommandName,
    pub device: u32,
    pub bundle: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<StopWay>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub was_running: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signalled: Option<Vec<u32>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub signal_errors: Vec<SignalError>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub replies: Vec<KillReply>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<Vec<u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extension_pids: Option<Vec<u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub located: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application_located: Option<ApplicationLocated>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reached: Option<Reached>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<Problem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<ErrorKind>,
    /// (ACIB-3b) How long each stage took ([`StageTimes`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage_ms: Option<StageTimes>,
}

impl Stop {
    /// The answer for `bundle` on `device`, before anything is read.
    pub fn asked(device: u32, bundle: &str) -> Self {
        Self {
            command: CommandName::Stop,
            device,
            bundle: bundle.to_owned(),
            via: None,
            installed: None,
            was_running: None,
            signalled: None,
            signal_errors: Vec::new(),
            replies: Vec::new(),
            remaining: None,
            running: None,
            extension_pids: None,
            located: None,
            application_located: None,
            reached: None,
            problem: None,
            error_kind: None,
            stage_ms: None,
        }
    }

    /// The bundle is installed (and what follows was not read).
    pub fn installed(self) -> Self {
        Self {
            installed: Some(true),
            ..self
        }
    }

    /// (ACIB-3c, fix round 1) The installation proxy's lookup does not hold the bundle: nothing
    /// ran, nothing was looked at and nothing is sent.
    pub fn not_installed(self) -> Self {
        Self {
            installed: Some(false),
            was_running: Some(false),
            signalled: Some(Vec::new()),
            remaining: Some(Vec::new()),
            running: Some(false),
            extension_pids: Some(Vec::new()),
            ..self
        }
    }

    /// (ACIB-3c, fix round 1) The application itself does not run at the first look `first`, a
    /// look that locates at least one process (`bridge.rs` answers one that locates none as
    /// `process_paths_unreadable`): no signal is sent, and the first look is the last; its
    /// extensions' pids and its `located` are named.
    pub fn not_running(self, first: Look) -> Self {
        Self {
            installed: Some(true),
            was_running: Some(false),
            signalled: Some(Vec::new()),
            remaining: Some(Vec::new()),
            running: Some(false),
            extension_pids: Some(first.pids.extensions),
            located: Some(first.located),
            application_located: first.application_located,
            ..self
        }
    }

    /// The application ran and was sent `signals`, and the processes of the bundle the command's
    /// last look lists are `last`, of which `verdict` is the decision (`stopping.rs`): its
    /// application's pids are `remaining`, and `still_running` stands when the verdict is still
    /// running. (Fix round 1) A last look that locates no process tells nothing of the bundle:
    /// the line is then `process_paths_unreadable`, with what was signalled, and no verdict.
    pub fn looked(self, signals: Signals, last: Look, verdict: Verdict) -> Self {
        if last.located == 0 {
            return self
                .with_signals(signals)
                .failed(Problem::ProcessPathsUnreadable, None);
        }
        let running = !last.pids.application.is_empty();
        Self {
            remaining: Some(last.pids.application),
            running: Some(running),
            extension_pids: Some(last.pids.extensions),
            located: Some(last.located),
            application_located: last.application_located,
            problem: match verdict {
                Verdict::StillRunning => Some(Problem::StillRunning),
                Verdict::Stopped => None,
            },
            ..self.with_signals(signals)
        }
    }

    /// The application ran and was sent `signals`, and a look after them failed with `kind`.
    pub fn look_failed(self, signals: Signals, kind: ErrorKind) -> Self {
        self.failed_after_kills(signals, Problem::ProcessListFailed, kind)
    }

    /// (ACIB-3d) The application ran and was sent `signals`, and a step after them stopped the
    /// command with `problem` of `kind` (the connection of the looks after the kills not made,
    /// `service_unreachable`, or its first message not read, `service_handshake_failed`; a look
    /// failing, `process_list_failed`): no verdict, no look's members.
    pub fn failed_after_kills(self, signals: Signals, problem: Problem, kind: ErrorKind) -> Self {
        self.with_signals(signals).failed(problem, Some(kind))
    }

    /// (ACIB-3d) What the server answered the kill requests of a way that asks for answers, each by
    /// its pid (none of the way `library`).
    pub fn with_replies(self, replies: Vec<KillReply>) -> Self {
        Self { replies, ..self }
    }

    /// (ACIB-3d) Of each stage after the first look, whether the command reached it.
    pub fn with_reached(self, reached: Reached) -> Self {
        Self {
            reached: Some(reached),
            ..self
        }
    }

    /// The application ran and was sent `signals`.
    fn with_signals(self, signals: Signals) -> Self {
        Self {
            installed: Some(true),
            was_running: Some(true),
            signalled: Some(signals.signalled),
            signal_errors: signals.errors,
            ..self
        }
    }

    /// Stopped by `problem`, of `kind` where the service gave an error.
    pub fn failed(self, problem: Problem, kind: Option<ErrorKind>) -> Self {
        Self {
            problem: Some(problem),
            error_kind: kind,
            ..self
        }
    }
}

/// A refused or failed command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Refused {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<u32>,
    /// The way a refused `screenshot` was to take its picture, or (ACIB-3d) a refused `stop` to
    /// send its kill requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<Via>,
    pub problem: Problem,
    /// Beside `deadline_passed` of a screenshot, and (ACIB-3b) of `running`, `launch` and
    /// `stop`: the stage it was waiting in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<Stage>,
    /// (ACIB-3b) Beside `deadline_passed` of `running`, `launch` and `stop`: how long each stage
    /// that finished before it took (not the one it was waiting in).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage_ms: Option<StageTimes>,
    /// Beside `out_write_failed`: whether a file is left at `--out`, `true` only when the file
    /// this program made could not be removed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub out_left: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The line a command prints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Line {
    Devices(Devices),
    Info(Info),
    Services(Services),
    Images(Images),
    Apps(Apps),
    Screenshot(Screenshot),
    Orientations(Orientations),
    Running(Running),
    Launch(Launch),
    Stop(Stop),
    Service(ServiceLine),
    Probe(Probe),
    Refused(Refused),
}

impl Line {
    /// A refusal of `command` on `device`.
    pub fn refused(command: Option<CommandName>, device: Option<u32>, failure: Failure) -> Self {
        Self::refused_on(command, device, None, failure)
    }

    /// A refusal of `command` on `device` that was to go the way `via`.
    pub fn refused_on(
        command: Option<CommandName>,
        device: Option<u32>,
        via: Option<Way>,
        failure: Failure,
    ) -> Self {
        Self::Refused(Refused {
            command,
            device,
            via: via.map(Via::Picture),
            problem: failure.problem,
            stage: None,
            stage_ms: None,
            out_left: None,
            error: failure.error,
        })
    }

    /// `deadline_passed` of `command` on `device`; a screenshot's line names the stage it was
    /// waiting in (`stage`), and so does (ACIB-3b) a line of `running`, `launch` and `stop`,
    /// with how long each stage that finished before it took (`stage_ms`); no other command
    /// sets either.
    pub fn deadline_passed(
        command: CommandName,
        device: Option<u32>,
        via: Option<Way>,
        stage: Option<Stage>,
        stage_ms: Option<StageTimes>,
    ) -> Self {
        Self::Refused(Refused {
            command: Some(command),
            device,
            via: via.map(Via::Picture),
            problem: Problem::DeadlinePassed,
            stage,
            stage_ms,
            out_left: None,
            error: None,
        })
    }

    /// (ACIB-3d) A line of `stop` with the way of its kill requests (`way`, the command line's):
    /// its answer and its refusal alike; every other line, and every line when `way` is `None`
    /// (a command other than `stop`), as it is.
    pub fn on_stop_way(self, way: Option<StopWay>) -> Self {
        let Some(way) = way else {
            return self;
        };
        match self {
            Self::Stop(stop) => Self::Stop(Stop {
                via: Some(way),
                ..stop
            }),
            Self::Refused(refused) => Self::Refused(Refused {
                via: Some(Via::Stop(way)),
                ..refused
            }),
            other => other,
        }
    }

    /// (ACIB-3b) The line of `running`, `launch` or `stop` with how long each of its stages took;
    /// every other line as it is.
    pub fn timed(self, times: StageTimes) -> Self {
        match self {
            Self::Running(running) => Self::Running(Running {
                stage_ms: Some(times),
                ..running
            }),
            Self::Launch(launch) => Self::Launch(Launch {
                stage_ms: Some(times),
                ..launch
            }),
            Self::Stop(stop) => Self::Stop(Stop {
                stage_ms: Some(times),
                ..stop
            }),
            other => other,
        }
    }

    /// `out_write_failed` of a screenshot on `device` the way `via`: the picture could not be
    /// written whole into the file this program made at `--out` (the error the system gave, as
    /// a library error is shown), and whether a file is left there.
    pub fn out_write_failed<E>(device: u32, via: Way, error: &E, out_left: bool) -> Self
    where
        E: std::fmt::Debug + std::fmt::Display,
    {
        let failure = Failure::library(Problem::OutWriteFailed, error);
        Self::Refused(Refused {
            command: Some(CommandName::Screenshot),
            device: Some(device),
            via: Some(Via::Picture(via)),
            problem: failure.problem,
            stage: None,
            stage_ms: None,
            out_left: Some(out_left),
            error: failure.error,
        })
    }

    /// Whether the command was answered (status 0); a line with a problem is not (status 1).
    pub fn answered(&self) -> bool {
        match self {
            Self::Refused(_) => false,
            Self::Info(info) => info.problem.is_none(),
            Self::Apps(apps) => apps.problem.is_none(),
            Self::Screenshot(screenshot) => screenshot.problem.is_none(),
            Self::Running(running) => running.problem.is_none(),
            Self::Launch(launch) => launch.problem.is_none(),
            Self::Stop(stop) => stop.problem.is_none(),
            Self::Orientations(orientations) => orientations
                .candidates
                .iter()
                .all(|candidate| candidate.orientation.is_some()),
            Self::Devices(_)
            | Self::Services(_)
            | Self::Images(_)
            | Self::Service(_)
            | Self::Probe(_) => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::tests::{VECTOR_IDENTIFIER, VECTOR_KEY, VECTOR_SALT, salt};
    use crate::matching::Pids;
    use crate::picture::tests::png_file;
    use std::collections::BTreeSet;

    fn json(line: &Line) -> String {
        serde_json::to_string(line).expect("an answer serializes")
    }

    fn first_half() -> Info {
        Info {
            command: CommandName::Info,
            device: 3,
            product_version: "27.0.1".to_owned(),
            product_type: "TESTPRODUCT1,1".to_owned(),
            build_version: "TESTBUILD1".to_owned(),
            device_class: "iPhone".to_owned(),
            paired: None,
            password_protected: None,
            developer_mode: None,
            problem: None,
            error: None,
        }
    }

    /// Errors of made-up kinds, as the library's `Debug` form starts with its kind.
    #[derive(Debug)]
    enum FakeError {
        UnexpectedResponse,
        Socket,
    }

    fn images(queries: Vec<ImageQuery>) -> Line {
        Line::Images(Images {
            command: CommandName::Images,
            device: 3,
            mounted: Mounted::judged(&queries),
            queries,
        })
    }

    /// The second salt of the key tests (32 bytes).
    const BINDING_SALT: &str = "acib-binding-test-salt-000000001";
    /// The identifier of a second fake device.
    const SECOND_IDENTIFIER: &str = "TESTSERIAL0002";

    /// `devices` with both salts: a USB row of the test vector's identifier and a network row
    /// of a second one, each with its key under the test vector's salt and under
    /// [`BINDING_SALT`].
    fn keyed_devices() -> Line {
        let (key_salt, binding_salt) = (salt(VECTOR_SALT), salt(BINDING_SALT));
        let entry = |device, connection, identifier| DeviceEntry {
            device,
            connection,
            device_key: Some(DeviceKey::of(&key_salt, identifier)),
            binding_key: Some(DeviceKey::of(&binding_salt, identifier)),
        };
        Line::Devices(Devices {
            command: CommandName::Devices,
            devices: vec![
                entry(3, Link::Usb, VECTOR_IDENTIFIER),
                entry(7, Link::Network, SECOND_IDENTIFIER),
            ],
        })
    }

    fn orientations(candidates: Vec<Candidate>) -> Line {
        Line::Orientations(Orientations {
            command: CommandName::Orientation,
            device: 3,
            candidates,
        })
    }

    /// One line of every shape the program prints.
    fn every_shape() -> Vec<(Line, &'static str, bool)> {
        let paired = Info {
            paired: Some(true),
            password_protected: Some(Reading::Value(false)),
            developer_mode: Some(Reading::Refused("GetProhibited".to_owned())),
            ..first_half()
        };
        let not_paired = Info {
            paired: Some(false),
            ..first_half()
        }
        .stopped(Failure::own(Problem::NotPaired));
        let session_refused = Info {
            paired: Some(true),
            ..first_half()
        }
        .stopped(Failure {
            problem: Problem::SessionRefused,
            error: Some("InvalidHostID".to_owned()),
        });
        let mut shapes = vec![
            (
                Line::Devices(Devices {
                    command: CommandName::Devices,
                    devices: vec![
                        DeviceEntry {
                            device: 3,
                            connection: Link::Usb,
                            device_key: None,
                            binding_key: None,
                        },
                        DeviceEntry {
                            device: 7,
                            connection: Link::Network,
                            device_key: None,
                            binding_key: None,
                        },
                        DeviceEntry {
                            device: 9,
                            connection: Link::Unknown,
                            device_key: None,
                            binding_key: None,
                        },
                    ],
                }),
                r#"{"command":"devices","devices":[{"device":3,"connection":"usb"},{"device":7,"connection":"network"},{"device":9,"connection":"unknown"}]}"#,
                true,
            ),
            (
                keyed_devices(),
                r#"{"command":"devices","devices":[{"device":3,"connection":"usb","device_key":"sha256:b74e1be265fea5ee2fd1b1b2243df7dcb16530f24a6b1bef41b050f8b2f427bf","binding_key":"sha256:306ba3303daa68214650000b184a95f32686fac009c80df1c0e2c8b67c94197c"},{"device":7,"connection":"network","device_key":"sha256:810c53f97f0349b864b3ad240c700f3fbc5db87ffe1577274d071eebb5fbc603","binding_key":"sha256:2a6bc1c2cbdc1b253d4d2150ff92a9b11765ee3a07c6a2904b36f6e346f18352"}]}"#,
                true,
            ),
            (
                Line::Devices(Devices {
                    command: CommandName::Devices,
                    devices: Vec::new(),
                }),
                r#"{"command":"devices","devices":[]}"#,
                true,
            ),
            (
                Line::Info(paired),
                r#"{"command":"info","device":3,"product_version":"27.0.1","product_type":"TESTPRODUCT1,1","build_version":"TESTBUILD1","device_class":"iPhone","paired":true,"password_protected":{"value":false},"developer_mode":{"refused":"GetProhibited"}}"#,
                true,
            ),
            (
                Line::Info(not_paired),
                r#"{"command":"info","device":3,"product_version":"27.0.1","product_type":"TESTPRODUCT1,1","build_version":"TESTBUILD1","device_class":"iPhone","paired":false,"problem":"not_paired"}"#,
                false,
            ),
            (
                Line::Info(session_refused),
                r#"{"command":"info","device":3,"product_version":"27.0.1","product_type":"TESTPRODUCT1,1","build_version":"TESTBUILD1","device_class":"iPhone","paired":true,"problem":"session_refused","error":"InvalidHostID"}"#,
                false,
            ),
            (
                Line::Info(first_half().stopped(Failure {
                    problem: Problem::PairRecordUnreadable,
                    error: Some("device socket io failed".to_owned()),
                })),
                r#"{"command":"info","device":3,"product_version":"27.0.1","product_type":"TESTPRODUCT1,1","build_version":"TESTBUILD1","device_class":"iPhone","problem":"pair_record_unreadable","error":"device socket io failed"}"#,
                false,
            ),
            (
                Line::Services(Services {
                    command: CommandName::Services,
                    device: 3,
                    count: 2,
                    services: vec![
                        ServiceEntry {
                            name: "com.example.one".to_owned(),
                            port: 49152,
                            entitlement: "com.example.entitlement.trusted".to_owned(),
                        },
                        ServiceEntry {
                            name: "com.example.two".to_owned(),
                            port: 58783,
                            entitlement: "com.example.entitlement.untrusted".to_owned(),
                        },
                    ],
                }),
                r#"{"command":"services","device":3,"count":2,"services":[{"name":"com.example.one","port":49152,"entitlement":"com.example.entitlement.trusted"},{"name":"com.example.two","port":58783,"entitlement":"com.example.entitlement.untrusted"}]}"#,
                true,
            ),
            (
                images(vec![
                    ImageQuery::listing(vec![
                        ListedImage::of(Some(vec!["ImageType", "ImageSignature"])),
                        ListedImage::of(None::<Vec<&str>>),
                    ]),
                    ImageQuery::lookup("Personalized", true),
                    ImageQuery::failed(
                        ImageQueryName::LookupImage,
                        Some("Developer"),
                        &FakeError::Socket,
                    ),
                ]),
                r#"{"command":"images","device":3,"mounted":true,"queries":[{"query":"copy_devices","answer":"listed","listed":[{"members":["ImageSignature","ImageType"]},{"members":null}]},{"query":"lookup_image","image_type":"Personalized","answer":"found"},{"query":"lookup_image","image_type":"Developer","answer":"failed","error_kind":"Socket"}]}"#,
                true,
            ),
            (
                images(vec![
                    ImageQuery::listing(Vec::new()),
                    ImageQuery::lookup("Personalized", false),
                    ImageQuery::lookup("Developer", false),
                ]),
                r#"{"command":"images","device":3,"mounted":false,"queries":[{"query":"copy_devices","answer":"listed","listed":[]},{"query":"lookup_image","image_type":"Personalized","answer":"not_found"},{"query":"lookup_image","image_type":"Developer","answer":"not_found"}]}"#,
                true,
            ),
            (
                images(vec![
                    ImageQuery::failed(
                        ImageQueryName::CopyDevices,
                        None,
                        &FakeError::UnexpectedResponse,
                    ),
                    ImageQuery::lookup("Personalized", false),
                    ImageQuery::lookup("Developer", false),
                ]),
                r#"{"command":"images","device":3,"mounted":"unknown","queries":[{"query":"copy_devices","answer":"failed","error_kind":"UnexpectedResponse"},{"query":"lookup_image","image_type":"Personalized","answer":"not_found"},{"query":"lookup_image","image_type":"Developer","answer":"not_found"}]}"#,
                true,
            ),
            (
                Line::Apps(Apps::of(
                    3,
                    "Game",
                    5,
                    [
                        "com.example.GameOne",
                        "com.example.tool",
                        "com.example.game-two",
                        "com.example.mail",
                        "com.example.endgame_x",
                        "com.example.notes",
                    ],
                )),
                r#"{"command":"apps","device":3,"contains":"Game","matching":["com.example.GameOne","com.example.game-two","withheld"],"matching_count":3,"user_applications":6}"#,
                true,
            ),
            (
                Line::Apps(Apps::of(3, "absent", 5, ["com.example.one"])),
                r#"{"command":"apps","device":3,"contains":"absent","matching":[],"matching_count":0,"user_applications":1}"#,
                true,
            ),
            (
                Line::Apps(Apps::of(
                    3,
                    "example",
                    2,
                    [
                        "com.example.one",
                        "com.example.two",
                        "com.example.three",
                        "org.other.app",
                    ],
                )),
                r#"{"command":"apps","device":3,"contains":"example","matching_count":3,"user_applications":4,"problem":"too_many_matching"}"#,
                false,
            ),
            (
                Line::refused(
                    Some(CommandName::Services),
                    Some(3),
                    Failure {
                        problem: Problem::TunnelFailed,
                        error: Some("TLS error".to_owned()),
                    },
                ),
                r#"{"command":"services","device":3,"problem":"tunnel_failed","error":"TLS error"}"#,
                false,
            ),
            (
                Line::refused(
                    Some(CommandName::Images),
                    Some(3),
                    Failure::own(Problem::NotPaired),
                ),
                r#"{"command":"images","device":3,"problem":"not_paired"}"#,
                false,
            ),
            (
                Line::refused(
                    Some(CommandName::Apps),
                    Some(3),
                    Failure {
                        problem: Problem::AppsLookupFailed,
                        error: Some("device socket io failed".to_owned()),
                    },
                ),
                r#"{"command":"apps","device":3,"problem":"apps_lookup_failed","error":"device socket io failed"}"#,
                false,
            ),
            (
                Line::Screenshot(Screenshot::of(
                    3,
                    Way::ScreenCapture,
                    &png_file(750, 1334, b"\0\0\0\x04IDATxxxx"),
                    Elapsed {
                        request: 412,
                        command: 1830,
                    },
                    "screen.png",
                )),
                r#"{"command":"screenshot","device":3,"via":"screen_capture","format":"png","width":750,"height":1334,"bytes":45,"elapsed_ms":{"request":412,"command":1830},"out":"screen.png"}"#,
                true,
            ),
            (
                Line::Screenshot(Screenshot::of(
                    3,
                    Way::Instruments,
                    &[0x89, 0x50, 0x4E],
                    Elapsed {
                        request: 9,
                        command: 2001,
                    },
                    "screen.png",
                )),
                r#"{"command":"screenshot","device":3,"via":"instruments","bytes":3,"elapsed_ms":{"request":9,"command":2001},"out":"screen.png","problem":"picture_header_truncated"}"#,
                false,
            ),
            (
                Line::refused_on(
                    Some(CommandName::Screenshot),
                    Some(3),
                    Some(Way::Instruments),
                    Failure {
                        problem: Problem::ScreenshotFailed,
                        error: Some("unexpected response from device: missing screenshot data in DVT response".to_owned()),
                    },
                ),
                r#"{"command":"screenshot","device":3,"via":"instruments","problem":"screenshot_failed","error":"unexpected response from device: missing screenshot data in DVT response"}"#,
                false,
            ),
            (
                Line::refused_on(
                    Some(CommandName::Screenshot),
                    Some(3),
                    Some(Way::ScreenCapture),
                    Failure::own(Problem::OutExists),
                ),
                r#"{"command":"screenshot","device":3,"via":"screen_capture","problem":"out_exists"}"#,
                false,
            ),
            (
                Line::out_write_failed(
                    3,
                    Way::ScreenCapture,
                    &std::io::Error::other("no space left for the picture"),
                    false,
                ),
                r#"{"command":"screenshot","device":3,"via":"screen_capture","problem":"out_write_failed","out_left":false,"error":"no space left for the picture"}"#,
                false,
            ),
            (
                Line::out_write_failed(
                    3,
                    Way::Instruments,
                    &std::io::Error::other("no space left for the picture"),
                    true,
                ),
                r#"{"command":"screenshot","device":3,"via":"instruments","problem":"out_write_failed","out_left":true,"error":"no space left for the picture"}"#,
                false,
            ),
            (
                Line::deadline_passed(
                    CommandName::Screenshot,
                    Some(3),
                    Some(Way::Instruments),
                    Some(Stage::ServerFirstMessage),
                    None,
                ),
                r#"{"command":"screenshot","device":3,"via":"instruments","problem":"deadline_passed","stage":"server_first_message"}"#,
                false,
            ),
            (
                Line::deadline_passed(CommandName::Devices, None, None, None, None),
                r#"{"command":"devices","problem":"deadline_passed"}"#,
                false,
            ),
            (
                Line::Screenshot(Screenshot::of(
                    3,
                    Way::ScreenCapture,
                    &png_file(750, 1334, b"\0\0\0\x04IDATxxxx"),
                    Elapsed {
                        request: 401,
                        command: 977,
                    },
                    "-",
                )),
                r#"{"command":"screenshot","device":3,"via":"screen_capture","format":"png","width":750,"height":1334,"bytes":45,"elapsed_ms":{"request":401,"command":977},"out":"-"}"#,
                true,
            ),
            (
                orientations(vec![
                    Candidate::answered(Reader::Springboard, Orientation::LandscapeLeft),
                    Candidate::answered(Reader::SpringboardShim, Orientation::LandscapeLeft),
                ]),
                r#"{"command":"orientation","device":3,"candidates":[{"via":"springboard","orientation":"landscape_left"},{"via":"springboard_shim","orientation":"landscape_left"}]}"#,
                true,
            ),
            (
                orientations(vec![Candidate::answered(
                    Reader::SpringboardShim,
                    Orientation::Unknown,
                )]),
                r#"{"command":"orientation","device":3,"candidates":[{"via":"springboard_shim","orientation":"unknown"}]}"#,
                true,
            ),
            (
                orientations(vec![
                    Candidate::answered(Reader::Springboard, Orientation::Portrait),
                    Candidate::failed(
                        Reader::SpringboardShim,
                        Failure::own(Problem::ServiceNotListed),
                    ),
                ]),
                r#"{"command":"orientation","device":3,"candidates":[{"via":"springboard","orientation":"portrait"},{"via":"springboard_shim","problem":"service_not_listed"}]}"#,
                false,
            ),
            (
                orientations(vec![
                    Candidate::failed(
                        Reader::Springboard,
                        Failure {
                            problem: Problem::OrientationFailed,
                            error: Some(
                                "unexpected response from device: missing interfaceOrientation in response".to_owned(),
                            ),
                        },
                    ),
                    Candidate::failed(
                        Reader::SpringboardShim,
                        Failure::own(Problem::DeadlinePassed),
                    ),
                ]),
                r#"{"command":"orientation","device":3,"candidates":[{"via":"springboard","problem":"orientation_failed","error":"unexpected response from device: missing interfaceOrientation in response"},{"via":"springboard_shim","problem":"deadline_passed"}]}"#,
                false,
            ),
            (
                Line::refused(
                    Some(CommandName::Orientation),
                    Some(3),
                    Failure::own(Problem::NotPaired),
                ),
                r#"{"command":"orientation","device":3,"problem":"not_paired"}"#,
                false,
            ),
            (
                Line::refused(
                    Some(CommandName::Devices),
                    None,
                    Failure::own(Problem::KeySaltUnusable),
                ),
                r#"{"command":"devices","problem":"key_salt_unusable"}"#,
                false,
            ),
            (
                Line::refused(
                    Some(CommandName::Devices),
                    None,
                    Failure::own(Problem::BindingSaltUnusable),
                ),
                r#"{"command":"devices","problem":"binding_salt_unusable"}"#,
                false,
            ),
            (
                Line::refused(
                    Some(CommandName::Info),
                    None,
                    Failure::own(Problem::DeviceKeyUnknown),
                ),
                r#"{"command":"info","problem":"device_key_unknown"}"#,
                false,
            ),
        ];
        shapes.extend(every_application_shape());
        shapes.extend(every_probe_and_service_shape());
        shapes.push((
            Line::refused(None, None, Failure::own(Problem::CommandLineInvalid)),
            r#"{"problem":"command_line_invalid"}"#,
            false,
        ));
        shapes
    }

    /// The bundle the application lines ask for.
    const BUNDLE: &str = "com.example.app";

    /// One line of every shape `running`, `launch` and `stop` print, with its text and whether
    /// it is answered.
    fn every_application_shape() -> Vec<(Line, &'static str, bool)> {
        let running = || Running::asked(3, BUNDLE);
        let launch = |relaunch| Launch::asked(3, BUNDLE, relaunch);
        let stop = || Stop::asked(3, BUNDLE);
        // (ACIB-3c, fix round 1) A look that locates `located` processes, the application's
        // located directly inside its bundle's directory when it runs.
        let look = |application: &[u32], extensions: &[u32], located: usize| Look {
            pids: Pids {
                application: application.to_vec(),
                extensions: extensions.to_vec(),
            },
            located,
            application_located: (!application.is_empty())
                .then_some(ApplicationLocated::DirectlyInside),
        };
        let located_as = |application: &[u32], located: usize, way: ApplicationLocated| Look {
            pids: Pids {
                application: application.to_vec(),
                extensions: Vec::new(),
            },
            located,
            application_located: Some(way),
        };
        let signals = |signalled: &[u32], errors: &[SignalError]| Signals {
            signalled: signalled.to_vec(),
            errors: errors.to_vec(),
        };
        let unreadable = SignalError::request(530, ErrorKind::AnswerUnreadable);
        vec![
            (
                Line::Running(running().found(look(&[412], &[530], 180))),
                r#"{"command":"running","device":3,"bundle":"com.example.app","installed":true,"running":true,"pids":[412],"extension_pids":[530],"located":180,"application_located":"directly_inside"}"#,
                true,
            ),
            (
                Line::Running(running().found(look(&[412, 413], &[], 180))),
                r#"{"command":"running","device":3,"bundle":"com.example.app","installed":true,"running":true,"pids":[412,413],"extension_pids":[],"located":180,"application_located":"directly_inside"}"#,
                true,
            ),
            (
                Line::Running(running().found(look(&[], &[530], 180))),
                r#"{"command":"running","device":3,"bundle":"com.example.app","installed":true,"running":false,"pids":[],"extension_pids":[530],"located":180}"#,
                true,
            ),
            (
                Line::Running(running().found(look(&[], &[], 180))),
                r#"{"command":"running","device":3,"bundle":"com.example.app","installed":true,"running":false,"pids":[],"extension_pids":[],"located":180}"#,
                true,
            ),
            // Fix round 1: the application at its bundle's directory, of both kinds, and a list
            // that locates no process, never `running` false.
            (
                Line::Running(running().found(located_as(
                    &[412],
                    6,
                    ApplicationLocated::AtDirectory,
                ))),
                r#"{"command":"running","device":3,"bundle":"com.example.app","installed":true,"running":true,"pids":[412],"extension_pids":[],"located":6,"application_located":"at_directory"}"#,
                true,
            ),
            (
                Line::Running(running().found(located_as(
                    &[412, 413],
                    7,
                    ApplicationLocated::Both,
                ))),
                r#"{"command":"running","device":3,"bundle":"com.example.app","installed":true,"running":true,"pids":[412,413],"extension_pids":[],"located":7,"application_located":"both"}"#,
                true,
            ),
            (
                Line::Running(running().found(Look::default())),
                r#"{"command":"running","device":3,"bundle":"com.example.app","installed":true,"problem":"process_paths_unreadable"}"#,
                false,
            ),
            (
                Line::Running(running().not_installed()),
                r#"{"command":"running","device":3,"bundle":"com.example.app","installed":false,"running":false,"pids":[],"extension_pids":[]}"#,
                true,
            ),
            (
                Line::Running(running().installed().failed(
                    Problem::ProcessListFailed,
                    Some(ErrorKind::ConnectionFailed),
                )),
                r#"{"command":"running","device":3,"bundle":"com.example.app","installed":true,"problem":"process_list_failed","error_kind":"connection_failed"}"#,
                false,
            ),
            (
                Line::Running(
                    running()
                        .installed()
                        .failed(Problem::BundlePathUnreadable, None),
                ),
                r#"{"command":"running","device":3,"bundle":"com.example.app","installed":true,"problem":"bundle_path_unreadable"}"#,
                false,
            ),
            (
                Line::Running(running().failed(
                    Problem::ApplicationLookupFailed,
                    Some(ErrorKind::ConnectionFailed),
                )),
                r#"{"command":"running","device":3,"bundle":"com.example.app","problem":"application_lookup_failed","error_kind":"connection_failed"}"#,
                false,
            ),
            (
                Line::Running(running().installed().failed(
                    Problem::ServiceHandshakeFailed,
                    Some(ErrorKind::ConnectionFailed),
                )),
                r#"{"command":"running","device":3,"bundle":"com.example.app","installed":true,"problem":"service_handshake_failed","error_kind":"connection_failed"}"#,
                false,
            ),
            (
                Line::Launch(launch(false).installed().launched(412)),
                r#"{"command":"launch","device":3,"bundle":"com.example.app","relaunch":false,"installed":true,"pid":412}"#,
                true,
            ),
            (
                Line::Launch(launch(true).installed().launched(413)),
                r#"{"command":"launch","device":3,"bundle":"com.example.app","relaunch":true,"installed":true,"pid":413}"#,
                true,
            ),
            (
                Line::Launch(
                    launch(false)
                        .installed()
                        .failed(Problem::LaunchFailed, Some(ErrorKind::ConnectionFailed)),
                ),
                r#"{"command":"launch","device":3,"bundle":"com.example.app","relaunch":false,"installed":true,"problem":"launch_failed","error_kind":"connection_failed"}"#,
                false,
            ),
            (
                Line::Launch(
                    launch(true)
                        .installed()
                        .failed(Problem::LaunchFailed, Some(ErrorKind::AnswerUnreadable)),
                ),
                r#"{"command":"launch","device":3,"bundle":"com.example.app","relaunch":true,"installed":true,"problem":"launch_failed","error_kind":"answer_unreadable"}"#,
                false,
            ),
            (
                Line::Launch(launch(false).not_installed()),
                r#"{"command":"launch","device":3,"bundle":"com.example.app","relaunch":false,"installed":false,"problem":"not_installed"}"#,
                false,
            ),
            (
                Line::Launch(
                    launch(true).failed(Problem::ApplicationLookupFailed, Some(ErrorKind::Other)),
                ),
                r#"{"command":"launch","device":3,"bundle":"com.example.app","relaunch":true,"problem":"application_lookup_failed","error_kind":"other"}"#,
                false,
            ),
            (
                Line::Stop(stop().not_running(look(&[], &[530], 180))),
                r#"{"command":"stop","device":3,"bundle":"com.example.app","installed":true,"was_running":false,"signalled":[],"remaining":[],"running":false,"extension_pids":[530],"located":180}"#,
                true,
            ),
            (
                Line::Stop(stop().not_installed()),
                r#"{"command":"stop","device":3,"bundle":"com.example.app","installed":false,"was_running":false,"signalled":[],"remaining":[],"running":false,"extension_pids":[]}"#,
                true,
            ),
            (
                Line::Stop(stop().looked(
                    signals(&[412, 413], &[]),
                    look(&[], &[530], 179),
                    Verdict::Stopped,
                )),
                r#"{"command":"stop","device":3,"bundle":"com.example.app","installed":true,"was_running":true,"signalled":[412,413],"remaining":[],"running":false,"extension_pids":[530],"located":179}"#,
                true,
            ),
            (
                Line::Stop(stop().looked(
                    signals(&[412, 530], &[unreadable]),
                    look(&[530], &[], 179),
                    Verdict::StillRunning,
                )),
                r#"{"command":"stop","device":3,"bundle":"com.example.app","installed":true,"was_running":true,"signalled":[412,530],"signal_errors":[{"pid":530,"error_kind":"answer_unreadable"}],"remaining":[530],"running":true,"extension_pids":[],"located":179,"application_located":"directly_inside","problem":"still_running"}"#,
                false,
            ),
            // Fix round 1: a look after the kills that locates no process, no verdict; the first
            // look locating none, no kill.
            (
                Line::Stop(stop().looked(signals(&[412], &[]), Look::default(), Verdict::Stopped)),
                r#"{"command":"stop","device":3,"bundle":"com.example.app","installed":true,"was_running":true,"signalled":[412],"problem":"process_paths_unreadable"}"#,
                false,
            ),
            (
                Line::Stop(
                    stop()
                        .installed()
                        .failed(Problem::ProcessPathsUnreadable, None),
                ),
                r#"{"command":"stop","device":3,"bundle":"com.example.app","installed":true,"problem":"process_paths_unreadable"}"#,
                false,
            ),
            // Fix round 2: a first look that does not tell which entry is the application, no
            // kill.
            (
                Line::Stop(
                    stop()
                        .installed()
                        .failed(Problem::ApplicationEntryAmbiguous, None),
                ),
                r#"{"command":"stop","device":3,"bundle":"com.example.app","installed":true,"problem":"application_entry_ambiguous"}"#,
                false,
            ),
            (
                Line::Stop(stop().look_failed(signals(&[412], &[]), ErrorKind::Other)),
                r#"{"command":"stop","device":3,"bundle":"com.example.app","installed":true,"was_running":true,"signalled":[412],"problem":"process_list_failed","error_kind":"other"}"#,
                false,
            ),
            (
                Line::Stop(stop().installed().failed(
                    Problem::ProcessListFailed,
                    Some(ErrorKind::AnswerUnreadable),
                )),
                r#"{"command":"stop","device":3,"bundle":"com.example.app","installed":true,"problem":"process_list_failed","error_kind":"answer_unreadable"}"#,
                false,
            ),
            // ACIB-3b: each line with how long each stage took, and the deadline of each
            // command with the stage it was waiting in; (ACIB-3c) the stages of the
            // installation proxy's lookup and of the instruments server.
            (
                Line::Running(running().found(look(&[412], &[], 180))).timed(times(&[
                    (Stage::Usbmuxd, 12),
                    (Stage::ApplicationLookup, 230),
                    (Stage::ProxyTunnel, 812),
                    (Stage::DiscoveryHandshake, 40),
                    (Stage::ServiceConnection, 3),
                    (Stage::ServerFirstMessage, 20),
                    (Stage::ProcessList, 120),
                ])),
                r#"{"command":"running","device":3,"bundle":"com.example.app","installed":true,"running":true,"pids":[412],"extension_pids":[],"located":180,"application_located":"directly_inside","stage_ms":{"usbmuxd":12,"application_lookup":230,"proxy_tunnel":812,"discovery_handshake":40,"service_connection":3,"server_first_message":20,"process_list":120}}"#,
                true,
            ),
            (
                Line::Launch(launch(false).installed().launched(412)).timed(times(&[
                    (Stage::Usbmuxd, 11),
                    (Stage::ApplicationLookup, 210),
                    (Stage::ProxyTunnel, 700),
                    (Stage::DiscoveryHandshake, 41),
                    (Stage::ServiceConnection, 2),
                    (Stage::ServerFirstMessage, 19),
                    (Stage::LaunchRequest, 1_250),
                ])),
                r#"{"command":"launch","device":3,"bundle":"com.example.app","relaunch":false,"installed":true,"pid":412,"stage_ms":{"usbmuxd":11,"application_lookup":210,"proxy_tunnel":700,"discovery_handshake":41,"service_connection":2,"server_first_message":19,"launch_request":1250}}"#,
                true,
            ),
            (
                Line::Stop(stop().looked(
                    signals(&[412], &[]),
                    look(&[], &[], 179),
                    Verdict::Stopped,
                ))
                .timed(times(&[
                    (Stage::Usbmuxd, 10),
                    (Stage::ApplicationLookup, 205),
                    (Stage::ProxyTunnel, 690),
                    (Stage::DiscoveryHandshake, 39),
                    (Stage::ServiceConnection, 2),
                    (Stage::ServerFirstMessage, 18),
                    (Stage::ProcessList, 110),
                    (Stage::KillRequests, 30),
                    (Stage::LooksAfterKills, 415),
                ])),
                r#"{"command":"stop","device":3,"bundle":"com.example.app","installed":true,"was_running":true,"signalled":[412],"remaining":[],"running":false,"extension_pids":[],"located":179,"stage_ms":{"usbmuxd":10,"application_lookup":205,"proxy_tunnel":690,"discovery_handshake":39,"service_connection":2,"server_first_message":18,"process_list":110,"kill_requests":30,"looks_after_kills":415}}"#,
                true,
            ),
            (
                Line::Running(running().failed(
                    Problem::ApplicationLookupFailed,
                    Some(ErrorKind::AnswerUnreadable),
                ))
                .timed(times(&[
                    (Stage::Usbmuxd, 12),
                    (Stage::ApplicationLookup, 7),
                ])),
                r#"{"command":"running","device":3,"bundle":"com.example.app","problem":"application_lookup_failed","error_kind":"answer_unreadable","stage_ms":{"usbmuxd":12,"application_lookup":7}}"#,
                false,
            ),
            (
                Line::deadline_passed(
                    CommandName::Running,
                    Some(3),
                    None,
                    Some(Stage::ServerFirstMessage),
                    Some(times(&[
                        (Stage::Usbmuxd, 12),
                        (Stage::ApplicationLookup, 230),
                        (Stage::ProxyTunnel, 812),
                        (Stage::DiscoveryHandshake, 40),
                        (Stage::ServiceConnection, 3),
                    ])),
                ),
                r#"{"command":"running","device":3,"problem":"deadline_passed","stage":"server_first_message","stage_ms":{"usbmuxd":12,"application_lookup":230,"proxy_tunnel":812,"discovery_handshake":40,"service_connection":3}}"#,
                false,
            ),
            (
                Line::deadline_passed(
                    CommandName::Launch,
                    Some(3),
                    None,
                    Some(Stage::LaunchRequest),
                    Some(times(&[
                        (Stage::Usbmuxd, 11),
                        (Stage::ApplicationLookup, 210),
                    ])),
                ),
                r#"{"command":"launch","device":3,"problem":"deadline_passed","stage":"launch_request","stage_ms":{"usbmuxd":11,"application_lookup":210}}"#,
                false,
            ),
            (
                Line::deadline_passed(
                    CommandName::Stop,
                    None,
                    None,
                    Some(Stage::Usbmuxd),
                    Some(StageTimes::default()),
                ),
                r#"{"command":"stop","problem":"deadline_passed","stage":"usbmuxd","stage_ms":{}}"#,
                false,
            ),
            // ACIB-3d: every line of `stop` names its way; one past the first look the answers to
            // its kill requests and the stages after the first look it reached.
            (
                Line::Stop(
                    stop()
                        .looked(
                            signals(&[412], &[]),
                            look(&[], &[530], 179),
                            Verdict::Stopped,
                        )
                        .with_replies(vec![KillReply {
                            pid: 412,
                            reply: KillAnswer::WithoutPayload,
                            message_type: Some(0),
                        }])
                        .with_reached(Reached {
                            kill_requests: true,
                            kill_replies: Some(true),
                            look_connection: true,
                            looks_after_kills: true,
                        }),
                )
                .timed(times(&[
                    (Stage::Usbmuxd, 10),
                    (Stage::ApplicationLookup, 205),
                    (Stage::ProxyTunnel, 690),
                    (Stage::DiscoveryHandshake, 39),
                    (Stage::ServiceConnection, 2),
                    (Stage::ServerFirstMessage, 18),
                    (Stage::ProcessList, 110),
                    (Stage::KillRequests, 2),
                    (Stage::KillReplies, 4),
                    (Stage::LookConnection, 70),
                    (Stage::LooksAfterKills, 120),
                ]))
                .on_stop_way(Some(StopWay::Archived)),
                r#"{"command":"stop","device":3,"bundle":"com.example.app","via":"archived","installed":true,"was_running":true,"signalled":[412],"replies":[{"pid":412,"reply":"without_payload","message_type":0}],"remaining":[],"running":false,"extension_pids":[530],"located":179,"reached":{"kill_requests":true,"kill_replies":true,"look_connection":true,"looks_after_kills":true},"stage_ms":{"usbmuxd":10,"application_lookup":205,"proxy_tunnel":690,"discovery_handshake":39,"service_connection":2,"server_first_message":18,"process_list":110,"kill_requests":2,"kill_replies":4,"look_connection":70,"looks_after_kills":120}}"#,
                true,
            ),
            (
                Line::Stop(
                    stop()
                        .looked(
                            signals(&[412, 413], &[]),
                            look(&[412], &[], 179),
                            Verdict::StillRunning,
                        )
                        .with_replies(vec![
                            KillReply {
                                pid: 412,
                                reply: KillAnswer::WithPayload,
                                message_type: Some(3),
                            },
                            KillReply {
                                pid: 413,
                                reply: KillAnswer::NotInTime,
                                message_type: None,
                            },
                        ])
                        .with_reached(Reached {
                            kill_requests: true,
                            kill_replies: Some(true),
                            look_connection: true,
                            looks_after_kills: true,
                        }),
                )
                .on_stop_way(Some(StopWay::Primitive)),
                r#"{"command":"stop","device":3,"bundle":"com.example.app","via":"primitive","installed":true,"was_running":true,"signalled":[412,413],"replies":[{"pid":412,"reply":"with_payload","message_type":3},{"pid":413,"reply":"not_in_time"}],"remaining":[412],"running":true,"extension_pids":[],"located":179,"application_located":"directly_inside","reached":{"kill_requests":true,"kill_replies":true,"look_connection":true,"looks_after_kills":true},"problem":"still_running"}"#,
                false,
            ),
            // Fix round 1: the wait for a kill's answer failed, the library's reader of the
            // connection having stopped (as `primitive` answered on a phone on 2026-11-19).
            (
                Line::Stop(
                    stop()
                        .looked(
                            signals(
                                &[412],
                                &[SignalError::reply(
                                    412,
                                    ErrorKind::ConnectionFailed,
                                    ReplyFailure::ReaderStopped,
                                )],
                            ),
                            look(&[412], &[], 179),
                            Verdict::StillRunning,
                        )
                        .with_reached(Reached {
                            kill_requests: true,
                            kill_replies: Some(true),
                            look_connection: true,
                            looks_after_kills: true,
                        }),
                )
                .on_stop_way(Some(StopWay::Primitive)),
                r#"{"command":"stop","device":3,"bundle":"com.example.app","via":"primitive","installed":true,"was_running":true,"signalled":[412],"signal_errors":[{"pid":412,"error_kind":"connection_failed","reply_failure":"reader_stopped"}],"remaining":[412],"running":true,"extension_pids":[],"located":179,"application_located":"directly_inside","reached":{"kill_requests":true,"kill_replies":true,"look_connection":true,"looks_after_kills":true},"problem":"still_running"}"#,
                false,
            ),
            (
                Line::Stop(
                    stop()
                        .failed_after_kills(
                            signals(&[412], &[]),
                            Problem::ServiceUnreachable,
                            ErrorKind::ConnectionFailed,
                        )
                        .with_reached(Reached {
                            kill_requests: true,
                            kill_replies: None,
                            look_connection: true,
                            looks_after_kills: false,
                        }),
                )
                .on_stop_way(Some(StopWay::Library)),
                r#"{"command":"stop","device":3,"bundle":"com.example.app","via":"library","installed":true,"was_running":true,"signalled":[412],"reached":{"kill_requests":true,"look_connection":true,"looks_after_kills":false},"problem":"service_unreachable","error_kind":"connection_failed"}"#,
                false,
            ),
            (
                Line::deadline_passed(
                    CommandName::Stop,
                    Some(3),
                    None,
                    Some(Stage::KillReplies),
                    Some(times(&[
                        (Stage::Usbmuxd, 10),
                        (Stage::ApplicationLookup, 205),
                    ])),
                )
                .on_stop_way(Some(StopWay::Archived)),
                r#"{"command":"stop","device":3,"via":"archived","problem":"deadline_passed","stage":"kill_replies","stage_ms":{"usbmuxd":10,"application_lookup":205}}"#,
                false,
            ),
            (
                Line::refused(
                    Some(CommandName::Stop),
                    Some(3),
                    Failure::own(Problem::NotPaired),
                )
                .on_stop_way(Some(StopWay::Primitive)),
                r#"{"command":"stop","device":3,"via":"primitive","problem":"not_paired"}"#,
                false,
            ),
        ]
    }

    /// What a phone sent on one connection: its settings, a window update, two headers, five DATA
    /// frames, on the root channel an empty dictionary and a message without a body, on the reply
    /// channel one with a body and a reply without one.
    fn phone_sent() -> PhoneSent {
        use crate::tap::{ChannelCounts, Frames};
        PhoneSent {
            bytes: 6_200,
            closed: false,
            read_failed: false,
            frames: Frames {
                settings: 2,
                window_update: 1,
                headers: 2,
                data: 5,
                rst_stream: 0,
                goaway: 0,
                other: 0,
            },
            root: ChannelCounts {
                content: 0,
                empty: 1,
                bodyless: 1,
                unreadable: 0,
                replies_without_body: 0,
                pending_bytes: 0,
            },
            reply: ChannelCounts {
                content: 1,
                empty: 0,
                bodyless: 1,
                unreadable: 0,
                replies_without_body: 1,
                pending_bytes: 0,
            },
            other_stream_bytes: 0,
            pending_frame_bytes: 0,
        }
    }

    /// (ACIB-3b, second round) One line of every shape `probe` and `services --service` print,
    /// with its text and whether it is answered.
    fn every_probe_and_service_shape() -> Vec<(Line, &'static str, bool)> {
        let features = vec![
            "com.example.feature.one".to_owned(),
            "com.example.feature.0123456789ABCDEF01234567".to_owned(),
        ];
        vec![
            (
                Line::Probe(Probe {
                    command: CommandName::Probe,
                    device: 3,
                    wait_ms: 4_000,
                    ways: vec![
                        ProbeResult {
                            way: ProbeWay::ListApps,
                            outcome: ProbeOutcome::Answered,
                            error_kind: None,
                            entries: Some(41),
                            ms: 380,
                            phone_sent: Some(phone_sent()),
                        },
                        ProbeResult {
                            way: ProbeWay::StreamApps,
                            outcome: ProbeOutcome::Failed,
                            error_kind: Some(ErrorKind::XpcFailed),
                            entries: None,
                            ms: 12,
                            phone_sent: Some(PhoneSent::default()),
                        },
                        ProbeResult::unconnected(
                            ProbeWay::ListProcesses,
                            ProbeOutcome::NotAsked,
                            None,
                        ),
                    ],
                }),
                r#"{"command":"probe","device":3,"wait_ms":4000,"ways":[{"way":"list_apps","outcome":"answered","entries":41,"ms":380,"phone_sent":{"bytes":6200,"closed":false,"read_failed":false,"frames":{"settings":2,"window_update":1,"headers":2,"data":5,"rst_stream":0,"goaway":0,"other":0},"root":{"content":0,"empty":1,"bodyless":1,"unreadable":0,"replies_without_body":0,"pending_bytes":0},"reply":{"content":1,"empty":0,"bodyless":1,"unreadable":0,"replies_without_body":1,"pending_bytes":0},"other_stream_bytes":0,"pending_frame_bytes":0}},{"way":"stream_apps","outcome":"failed","error_kind":"xpc_failed","ms":12,"phone_sent":{"bytes":0,"closed":false,"read_failed":false,"frames":{"settings":0,"window_update":0,"headers":0,"data":0,"rst_stream":0,"goaway":0,"other":0},"root":{"content":0,"empty":0,"bodyless":0,"unreadable":0,"replies_without_body":0,"pending_bytes":0},"reply":{"content":0,"empty":0,"bodyless":0,"unreadable":0,"replies_without_body":0,"pending_bytes":0},"other_stream_bytes":0,"pending_frame_bytes":0}},{"way":"list_processes","outcome":"not_asked","ms":0}]}"#,
                true,
            ),
            (
                Line::refused(
                    Some(CommandName::Probe),
                    Some(3),
                    Failure::own(Problem::ServiceNotListed),
                ),
                r#"{"command":"probe","device":3,"problem":"service_not_listed"}"#,
                false,
            ),
            (
                Line::Service(ServiceLine {
                    command: CommandName::Services,
                    device: 3,
                    service: ServiceDetail::of(
                        "com.example.service",
                        49152,
                        "com.example.entitlement.trusted",
                        true,
                        Some(1),
                        Some(features.as_slice()),
                    ),
                }),
                r#"{"command":"services","device":3,"service":{"name":"com.example.service","port":49152,"entitlement":"com.example.entitlement.trusted","uses_remote_xpc":true,"service_version":1,"features":["com.example.feature.one","withheld"]}}"#,
                true,
            ),
            (
                Line::Service(ServiceLine {
                    command: CommandName::Services,
                    device: 3,
                    service: ServiceDetail::of(
                        "com.example.two",
                        58783,
                        "com.example.entitlement.untrusted",
                        false,
                        None,
                        None,
                    ),
                }),
                r#"{"command":"services","device":3,"service":{"name":"com.example.two","port":58783,"entitlement":"com.example.entitlement.untrusted","uses_remote_xpc":false}}"#,
                true,
            ),
            (
                Line::refused(
                    Some(CommandName::Services),
                    Some(3),
                    Failure::own(Problem::ServiceNotListed),
                ),
                r#"{"command":"services","device":3,"problem":"service_not_listed"}"#,
                false,
            ),
        ]
    }

    /// The times of `stages`, each with its milliseconds.
    fn times(stages: &[(Stage, u64)]) -> StageTimes {
        let finished: Vec<(Stage, Duration)> = stages
            .iter()
            .map(|(stage, milliseconds)| (*stage, Duration::from_millis(*milliseconds)))
            .collect();
        StageTimes::of(&finished)
    }

    #[test]
    fn each_answer_is_one_json_line_of_its_shape_and_its_status() {
        for (line, expected, answered) in every_shape() {
            let text = json(&line);
            assert_eq!(text, expected);
            assert!(!text.contains('\n'), "one line: {text}");
            assert_eq!(line.answered(), answered, "status of {text}");
        }
    }

    /// The answer types have no member for a serial number, a UDID, a name, an address or a
    /// pairing record, nor (ACIB-1) for a picture's content, nor (ACIB-2) for a salt: every
    /// member any line holds is one of these (a screenshot's `stage` is a word of [`Stage`], its
    /// `out_left` a boolean; a row's `device_key` and `binding_key` are [`DeviceKey`]s, salted
    /// digests; an orientation is a word of [`Orientation`]; ACIB-3c, fix round 1: `located` is a
    /// count, `application_located` a word of [`ApplicationLocated`]; ACIB-3d: a `stop`'s `via` is a
    /// word of [`StopWay`], its `replies` pids of the bundle's own with a word of [`KillAnswer`]
    /// and a message type, its `reached` a boolean per stage, by the stage's word; fix round 1: a
    /// failed request's `reply_failure` a word of [`ReplyFailure`]).
    #[test]
    fn the_answers_hold_no_member_for_an_identifier() {
        const MEMBERS: &[&str] = &[
            "answer",
            "application_located",
            "application_lookup",
            "binding_key",
            "bodyless",
            "build_version",
            "bundle",
            "bytes",
            "candidates",
            "closed",
            "command",
            "connection",
            "contains",
            "content",
            "count",
            "data",
            "developer_mode",
            "device",
            "device_class",
            "device_key",
            "devices",
            "discovery_handshake",
            "elapsed_ms",
            "empty",
            "entitlement",
            "entries",
            "error",
            "error_kind",
            "extension_pids",
            "features",
            "format",
            "frames",
            "goaway",
            "headers",
            "height",
            "image_type",
            "installed",
            "kill_replies",
            "kill_requests",
            "launch_request",
            "listed",
            "located",
            "look_connection",
            "looks_after_kills",
            "matching",
            "matching_count",
            "members",
            "message_type",
            "mounted",
            "ms",
            "name",
            "orientation",
            "other",
            "other_stream_bytes",
            "out",
            "out_left",
            "outcome",
            "paired",
            "password_protected",
            "pending_bytes",
            "pending_frame_bytes",
            "phone_sent",
            "pid",
            "pids",
            "port",
            "problem",
            "process_list",
            "product_type",
            "product_version",
            "proxy_tunnel",
            "queries",
            "query",
            "reached",
            "read_failed",
            "refused",
            "relaunch",
            "remaining",
            "replies",
            "replies_without_body",
            "reply",
            "reply_failure",
            "request",
            "root",
            "rst_stream",
            "running",
            "server_first_message",
            "service",
            "service_connection",
            "service_version",
            "services",
            "settings",
            "signal_errors",
            "signalled",
            "stage",
            "stage_ms",
            "unreadable",
            "usbmuxd",
            "user_applications",
            "uses_remote_xpc",
            "value",
            "via",
            "wait_ms",
            "was_running",
            "way",
            "ways",
            "width",
            "window_update",
        ];
        fn collect(value: &serde_json::Value, found: &mut BTreeSet<String>) {
            match value {
                serde_json::Value::Object(map) => {
                    for (key, member) in map {
                        found.insert(key.clone());
                        collect(member, found);
                    }
                }
                serde_json::Value::Array(items) => {
                    for item in items {
                        collect(item, found);
                    }
                }
                _ => {}
            }
        }
        let mut found = BTreeSet::new();
        for (line, _, _) in every_shape() {
            let value = serde_json::to_value(&line).expect("an answer serializes");
            collect(&value, &mut found);
        }
        let allowed: BTreeSet<String> = MEMBERS.iter().map(|member| (*member).to_owned()).collect();
        assert_eq!(found, allowed);
    }

    #[test]
    fn every_problem_word_is_this_programs_own_snake_case_word() {
        let words = [
            (Problem::CommandLineInvalid, "command_line_invalid"),
            (Problem::AsyncRuntimeFailed, "async_runtime_failed"),
            (Problem::DeadlinePassed, "deadline_passed"),
            (Problem::UsbmuxdUnreachable, "usbmuxd_unreachable"),
            (Problem::UsbmuxdListFailed, "usbmuxd_list_failed"),
            (Problem::DeviceNotListed, "device_not_listed"),
            (Problem::LockdownUnreachable, "lockdown_unreachable"),
            (Problem::LockdownReadFailed, "lockdown_read_failed"),
            (
                Problem::LockdownValueUnexpected,
                "lockdown_value_unexpected",
            ),
            (Problem::PairRecordUnreadable, "pair_record_unreadable"),
            (Problem::NotPaired, "not_paired"),
            (Problem::SessionRefused, "session_refused"),
            (Problem::ServiceRefused, "service_refused"),
            (Problem::ServiceUnreachable, "service_unreachable"),
            (Problem::TunnelFailed, "tunnel_failed"),
            (Problem::RsdUnreachable, "rsd_unreachable"),
            (Problem::RsdHandshakeFailed, "rsd_handshake_failed"),
            (Problem::AppsLookupFailed, "apps_lookup_failed"),
            (Problem::TooManyMatching, "too_many_matching"),
            (Problem::OutExists, "out_exists"),
            (Problem::OutUnwritable, "out_unwritable"),
            (Problem::OutWriteFailed, "out_write_failed"),
            (Problem::ServiceNotListed, "service_not_listed"),
            (Problem::ServiceHandshakeFailed, "service_handshake_failed"),
            (Problem::ChannelRefused, "channel_refused"),
            (Problem::ScreenshotFailed, "screenshot_failed"),
            (Problem::PictureHeaderTruncated, "picture_header_truncated"),
            (Problem::PictureHeaderUnknown, "picture_header_unknown"),
            (Problem::KeySaltUnusable, "key_salt_unusable"),
            (Problem::BindingSaltUnusable, "binding_salt_unusable"),
            (Problem::DeviceKeyUnknown, "device_key_unknown"),
            (Problem::OrientationFailed, "orientation_failed"),
            (
                Problem::ApplicationLookupFailed,
                "application_lookup_failed",
            ),
            (Problem::ProcessListFailed, "process_list_failed"),
            (Problem::BundlePathUnreadable, "bundle_path_unreadable"),
            (Problem::ProcessPathsUnreadable, "process_paths_unreadable"),
            (
                Problem::ApplicationEntryAmbiguous,
                "application_entry_ambiguous",
            ),
            (Problem::LaunchFailed, "launch_failed"),
            (Problem::StillRunning, "still_running"),
            (Problem::NotInstalled, "not_installed"),
        ];
        let mut seen = BTreeSet::new();
        for (problem, word) in words {
            assert_eq!(
                serde_json::to_value(problem).expect("a word serializes"),
                serde_json::Value::String(word.to_owned())
            );
            assert!(
                word.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                "{word} is snake_case"
            );
            assert!(seen.insert(word), "{word} is one word for one cause");
        }
    }

    /// (ACIB-3c, fix round 2) The program's own closed list of problem words: every word of
    /// [`Problem`], as the reader the tests derive for it names them. Serde hands the reader of an
    /// enum the names of all its variants, each as it is written (`Deserializer::deserialize_enum`
    /// and its `variants`), so a word added to [`Problem`] is in this list with nothing more
    /// written; each is named once.
    fn every_problem_word() -> BTreeSet<String> {
        // A reader that reads nothing and keeps the names of the variants it is handed.
        struct Variants<'a>(&'a mut &'static [&'static str]);

        impl<'de> serde::Deserializer<'de> for Variants<'_> {
            type Error = serde::de::value::Error;

            fn deserialize_any<V: serde::de::Visitor<'de>>(
                self,
                _visitor: V,
            ) -> Result<V::Value, Self::Error> {
                Err(serde::de::Error::custom("only an enum's names are read"))
            }

            fn deserialize_enum<V: serde::de::Visitor<'de>>(
                self,
                _name: &'static str,
                variants: &'static [&'static str],
                _visitor: V,
            ) -> Result<V::Value, Self::Error> {
                *self.0 = variants;
                Err(serde::de::Error::custom("its names are kept"))
            }

            serde::forward_to_deserialize_any! {
                bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes
                byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct map
                struct identifier ignored_any
            }
        }

        let mut variants: &'static [&'static str] = &[];
        let read: Result<Problem, _> = serde::Deserialize::deserialize(Variants(&mut variants));
        assert!(read.is_err(), "no word is read");
        let words: BTreeSet<String> = variants.iter().map(|word| (*word).to_owned()).collect();
        assert_eq!(words.len(), variants.len(), "each word is named once");
        assert!(!words.is_empty(), "the list names the words");
        words
    }

    /// (ACIB-3c, fix round 2) The README's table of problem words ("The problem words:", up to the
    /// sentence on the error kinds) is the program's closed list ([`every_problem_word`]): it
    /// names every word the program can answer, each once, and no other, each written
    /// `` `word` `` and joined by ", ". Read as the README's other lists are (`stages.rs`): its
    /// text, every run of blanks read as one space.
    #[test]
    fn the_readmes_table_of_problem_words_is_the_programs_closed_list() {
        const TABLE: &str = "The problem words: ";
        const END: &str = ". The error kinds: ";
        let readme = include_str!("../README.md").replace("\r\n", "\n");
        let flat = readme.split_whitespace().collect::<Vec<_>>().join(" ");
        let start = flat
            .find(TABLE)
            .expect("the README has its table of problem words")
            + TABLE.len();
        let length = flat[start..]
            .find(END)
            .expect("the table ends before the error kinds");
        let table: Vec<String> = flat[start..start + length]
            .split(", ")
            .map(|entry| {
                entry
                    .strip_prefix('`')
                    .and_then(|rest| rest.strip_suffix('`'))
                    .unwrap_or_else(|| panic!("{entry} is written as a word"))
                    .to_owned()
            })
            .collect();
        let named: BTreeSet<String> = table.iter().cloned().collect();
        assert_eq!(named.len(), table.len(), "each word once: {table:?}");
        assert_eq!(named, every_problem_word());
    }

    #[test]
    fn a_library_error_shows_its_text_or_only_its_kind() {
        #[derive(Debug)]
        enum FakeError {
            Plain,
            Carrying(String),
        }
        impl std::fmt::Display for FakeError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    Self::Plain => f.write_str("device socket io failed"),
                    Self::Carrying(text) => write!(f, "unexpected response from device: {text}"),
                }
            }
        }
        assert_eq!(
            Failure::library(Problem::UsbmuxdUnreachable, &FakeError::Plain),
            Failure {
                problem: Problem::UsbmuxdUnreachable,
                error: Some("device socket io failed".to_owned()),
            }
        );
        let carrying = FakeError::Carrying("0123456789abcdef0123456789abcdef01234567".to_owned());
        assert_eq!(
            Failure::library(Problem::PairRecordUnreadable, &carrying),
            Failure {
                problem: Problem::PairRecordUnreadable,
                error: Some("Carrying".to_owned()),
            }
        );
    }

    /// `mounted` is `false` only on an empty listing with no query failed and no image found,
    /// `true` on a listing with an entry or a found image, `"unknown"` otherwise.
    #[test]
    fn mounted_is_false_only_on_an_empty_listing_and_unknown_without_one() {
        let entry = || ListedImage::of(Some(vec!["ImageType"]));
        let failed_listing =
            || ImageQuery::failed(ImageQueryName::CopyDevices, None, &FakeError::Socket);
        let failed_lookup = || {
            ImageQuery::failed(
                ImageQueryName::LookupImage,
                Some("Personalized"),
                &FakeError::UnexpectedResponse,
            )
        };
        let cases = [
            (
                vec![ImageQuery::listing(Vec::new())],
                Mounted::No,
                "an empty listing",
            ),
            (
                vec![
                    ImageQuery::listing(Vec::new()),
                    ImageQuery::lookup("Personalized", false),
                    ImageQuery::lookup("Developer", false),
                ],
                Mounted::No,
                "an empty listing, nothing found, nothing failed",
            ),
            (
                vec![
                    ImageQuery::listing(Vec::new()),
                    ImageQuery::lookup("Developer", false),
                    failed_lookup(),
                ],
                Mounted::Unknown,
                "an empty listing beside a failed lookup",
            ),
            (
                vec![
                    ImageQuery::listing(Vec::new()),
                    ImageQuery::lookup("Personalized", true),
                ],
                Mounted::Unknown,
                "an empty listing and a found image disagree",
            ),
            (
                vec![
                    ImageQuery::listing(vec![entry()]),
                    ImageQuery::lookup("Personalized", false),
                ],
                Mounted::Yes,
                "a listed entry",
            ),
            (
                vec![ImageQuery::listing(vec![entry()]), failed_lookup()],
                Mounted::Yes,
                "a listed entry beside a failed lookup",
            ),
            (
                vec![failed_listing(), ImageQuery::lookup("Developer", true)],
                Mounted::Yes,
                "a found image without a listing",
            ),
            (
                vec![
                    failed_listing(),
                    ImageQuery::lookup("Personalized", false),
                    ImageQuery::lookup("Developer", false),
                ],
                Mounted::Unknown,
                "lookups that found nothing, without a listing",
            ),
            (
                vec![failed_listing(), failed_lookup()],
                Mounted::Unknown,
                "every query failed",
            ),
        ];
        for (queries, expected, case) in cases {
            assert_eq!(Mounted::judged(&queries), expected, "{case}");
        }
        assert_eq!(serde_json::to_string(&Mounted::Yes).unwrap(), "true");
        assert_eq!(serde_json::to_string(&Mounted::No).unwrap(), "false");
        assert_eq!(
            serde_json::to_string(&Mounted::Unknown).unwrap(),
            r#""unknown""#
        );
    }

    /// `apps` names the matching bundle identifiers only when they are at most the bound and at
    /// most half of the user's applications (`2 * matching_count <= user_applications`);
    /// otherwise its line holds the two counts and no identifier, and answers
    /// `too_many_matching` (status 1).
    #[test]
    fn apps_names_no_identifier_when_more_than_the_bound_or_more_than_half_match() {
        const ONE: &str = "com.example.game-one";
        const TWO: &str = "com.example.game-two";
        const THREE: &str = "com.example.game-three";
        const TOOL: &str = "com.example.tool";
        const MAIL: &str = "com.example.mail";
        const NOTES: &str = "com.example.notes";
        let cases = [
            (
                2,
                vec![ONE, TWO, TOOL, MAIL],
                Some(vec![ONE, TWO]),
                2,
                "as many as the bound, half of them",
            ),
            (
                2,
                vec![ONE, TWO, THREE, TOOL, MAIL, NOTES],
                None,
                3,
                "one more than the bound, half of them",
            ),
            (
                1,
                vec![ONE, TOOL],
                Some(vec![ONE]),
                1,
                "one, the bound, half",
            ),
            (
                1,
                vec![ONE, TWO, TOOL, MAIL],
                None,
                2,
                "one more than a bound of one, half of them",
            ),
            (
                5,
                vec![ONE, TWO, TOOL, MAIL],
                Some(vec![ONE, TWO]),
                2,
                "half of four, within the bound",
            ),
            (
                5,
                vec![ONE, TWO, THREE, TOOL],
                None,
                3,
                "one more than half of four, within the bound",
            ),
            (
                5,
                vec![ONE, TWO, TOOL, MAIL, NOTES],
                Some(vec![ONE, TWO]),
                2,
                "at most half of five, within the bound",
            ),
            (
                5,
                vec![ONE, TWO, THREE, TOOL, MAIL],
                None,
                3,
                "more than half of five, within the bound",
            ),
            (5, vec![ONE, TWO], None, 2, "all of them, within the bound"),
            (5, vec![ONE], None, 1, "the one user application matches"),
            (5, vec![TOOL], Some(Vec::new()), 0, "none"),
            (5, Vec::new(), Some(Vec::new()), 0, "no user application"),
        ];
        for (bound, identifiers, named, count, case) in cases {
            let refused = named.is_none();
            let apps = Apps::of(3, "Game", bound, identifiers.iter().copied());
            let expected: Option<Vec<String>> =
                named.map(|names| names.into_iter().map(str::to_owned).collect());
            assert_eq!(apps.matching, expected, "{case}");
            assert_eq!(apps.matching_count, count, "{case}");
            assert_eq!(apps.user_applications, identifiers.len(), "{case}");
            assert_eq!(
                apps.problem,
                refused.then_some(Problem::TooManyMatching),
                "{case}"
            );
            let line = Line::Apps(apps);
            assert_eq!(line.answered(), !refused, "{case}");
            if refused {
                let text = json(&line);
                assert!(!text.contains("com.example"), "{case}: {text}");
            }
        }
    }

    /// A screenshot's line names the picture's format, width and height as its header gives
    /// them and its length, and holds nothing of its content: no text a chunk of it carries, no
    /// byte of its data. A picture whose header cannot be read answers the header's word
    /// (status 1) and keeps its length and its file.
    #[test]
    fn a_screenshot_line_holds_the_header_and_nothing_of_the_picture() {
        fn members(value: &serde_json::Value, found: &mut BTreeSet<String>) {
            if let serde_json::Value::Object(map) = value {
                for (key, member) in map {
                    found.insert(key.clone());
                    members(member, found);
                }
            }
        }
        let elapsed = Elapsed {
            request: 5,
            command: 7,
        };
        let mut text_chunk = 16u32.to_be_bytes().to_vec();
        text_chunk.extend_from_slice(b"tEXtComment\0SEENTEXT");
        text_chunk.extend_from_slice(b"\0\0\0\x05IDATPIXEL");
        let picture = png_file(1170, 2532, &text_chunk);
        let screenshot = Screenshot::of(4, Way::Instruments, &picture, elapsed, "shot.png");
        assert_eq!(screenshot.format, Some(PictureFormat::Png));
        assert_eq!(
            (screenshot.width, screenshot.height),
            (Some(1170), Some(2532))
        );
        assert_eq!(screenshot.bytes, picture.len());
        assert_eq!(screenshot.problem, None);
        let line = Line::Screenshot(screenshot);
        assert!(line.answered());
        let text = json(&line);
        for content in ["SEENTEXT", "PIXEL", "IDAT", "tEXt", "Comment"] {
            assert!(!text.contains(content), "{content} in {text}");
        }
        let mut found = BTreeSet::new();
        members(
            &serde_json::to_value(&line).expect("a line serializes"),
            &mut found,
        );
        let expected: BTreeSet<String> = [
            "bytes",
            "command",
            "device",
            "elapsed_ms",
            "format",
            "height",
            "out",
            "request",
            "via",
            "width",
        ]
        .iter()
        .map(|member| (*member).to_owned())
        .collect();
        assert_eq!(found, expected);

        for (picture, problem) in [
            (b"GIF89a-SEENTEXT".to_vec(), Problem::PictureHeaderUnknown),
            (Vec::new(), Problem::PictureHeaderTruncated),
            (
                png_file(1170, 2532, &[])[..20].to_vec(),
                Problem::PictureHeaderTruncated,
            ),
        ] {
            let screenshot = Screenshot::of(4, Way::ScreenCapture, &picture, elapsed, "shot.png");
            assert_eq!(screenshot.problem, Some(problem), "{picture:?}");
            assert_eq!(
                (screenshot.format, screenshot.width, screenshot.height),
                (None, None, None)
            );
            assert_eq!(screenshot.bytes, picture.len());
            assert_eq!(screenshot.out, "shot.png");
            let line = Line::Screenshot(screenshot);
            assert!(!line.answered());
            assert!(!json(&line).contains("SEENTEXT"));
        }
    }

    /// Each way has a word of its own, the one `--via` takes and the line names; no other text
    /// names a way.
    #[test]
    fn every_way_has_a_word_of_its_own() {
        let mut seen = BTreeSet::new();
        for way in Way::ALL {
            assert_eq!(
                serde_json::to_value(way).expect("a way serializes"),
                serde_json::Value::String(way.word().to_owned())
            );
            assert_eq!(Way::from_word(way.word()), Some(way));
            assert!(seen.insert(way.word()), "{} is one word", way.word());
        }
        assert_eq!(seen, BTreeSet::from(["instruments", "screen_capture"]));
        for word in [
            "",
            "SCREEN_CAPTURE",
            "screencapture",
            "screen-capture",
            "dvt",
            "screenshotr",
        ] {
            assert_eq!(Way::from_word(word), None, "{word}");
        }
    }

    /// A screenshot's deadline names the stage it was waiting in by one of five words of this
    /// program's own, one word for each stage, in the order a screenshot goes through them.
    #[test]
    fn every_stage_of_a_screenshot_has_a_word_of_its_own() {
        let words = [
            (Stage::Session, "session"),
            (Stage::Tunnel, "tunnel"),
            (Stage::ServiceHandshake, "service_handshake"),
            (Stage::ServerFirstMessage, "server_first_message"),
            (Stage::PictureRequest, "picture_request"),
        ];
        let mut seen = BTreeSet::new();
        for (stage, word) in words {
            assert_eq!(
                serde_json::to_value(stage).expect("a stage serializes"),
                serde_json::Value::String(word.to_owned())
            );
            assert!(
                word.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                "{word} is snake_case"
            );
            assert!(seen.insert(word), "{word} is one word for one stage");
        }
    }

    /// A row of `devices` holds its keys, salted digests, and neither salt nor the identifier
    /// either key was made from; without the salts' flags it holds the device and the
    /// connection alone, as before; with the first salt alone, `device_key` and no
    /// `binding_key`.
    #[test]
    fn a_device_row_holds_its_keys_and_neither_a_salt_nor_an_identifier() {
        fn rows(line: &Line) -> Vec<BTreeSet<String>> {
            let value = serde_json::to_value(line).expect("a line serializes");
            value["devices"]
                .as_array()
                .expect("the rows")
                .iter()
                .map(|row| row.as_object().expect("a row").keys().cloned().collect())
                .collect()
        }
        let members = |names: &[&str]| -> BTreeSet<String> {
            names.iter().map(|name| (*name).to_owned()).collect()
        };
        let keyed = keyed_devices();
        let text = json(&keyed);
        for withheld in [
            VECTOR_SALT,
            BINDING_SALT,
            VECTOR_IDENTIFIER,
            SECOND_IDENTIFIER,
            "TESTSERIAL",
            "test-salt",
        ] {
            assert!(!text.contains(withheld), "{withheld} in {text}");
        }
        assert!(text.contains(VECTOR_KEY), "{text}");
        let both = members(&["binding_key", "connection", "device", "device_key"]);
        assert_eq!(rows(&keyed), vec![both.clone(), both]);

        let entry = |device_key| DeviceEntry {
            device: 3,
            connection: Link::Usb,
            device_key,
            binding_key: None,
        };
        let line = |row| {
            Line::Devices(Devices {
                command: CommandName::Devices,
                devices: vec![row],
            })
        };
        let plain = line(entry(None));
        assert_eq!(
            json(&plain),
            r#"{"command":"devices","devices":[{"device":3,"connection":"usb"}]}"#
        );
        assert_eq!(rows(&plain), vec![members(&["connection", "device"])]);
        let first_only = line(entry(Some(DeviceKey::of(
            &salt(VECTOR_SALT),
            VECTOR_IDENTIFIER,
        ))));
        assert_eq!(
            json(&first_only),
            format!(
                r#"{{"command":"devices","devices":[{{"device":3,"connection":"usb","device_key":"{VECTOR_KEY}"}}]}}"#
            )
        );
        assert!(first_only.answered());
    }

    /// Each reader of `orientation` has a word of its own, the one `--via` takes and the line
    /// names, and so does each orientation the library names; no other text names a reader.
    #[test]
    fn every_reader_and_every_orientation_has_a_word_of_its_own() {
        let mut seen = BTreeSet::new();
        for reader in Reader::ALL {
            assert_eq!(
                serde_json::to_value(reader).expect("a reader serializes"),
                serde_json::Value::String(reader.word().to_owned())
            );
            assert_eq!(Reader::from_word(reader.word()), Some(reader));
            assert!(seen.insert(reader.word()), "{} is one word", reader.word());
        }
        assert_eq!(seen, BTreeSet::from(["springboard", "springboard_shim"]));
        for word in [
            "",
            "SPRINGBOARD",
            "springboardservices",
            "springboard-shim",
            "devicecontrol",
            "screen_capture",
        ] {
            assert_eq!(Reader::from_word(word), None, "{word}");
        }
        let words: BTreeSet<String> = [
            Orientation::Unknown,
            Orientation::Portrait,
            Orientation::PortraitUpsideDown,
            Orientation::LandscapeRight,
            Orientation::LandscapeLeft,
        ]
        .into_iter()
        .map(|orientation| {
            serde_json::to_value(orientation)
                .expect("an orientation serializes")
                .as_str()
                .expect("an orientation is a word")
                .to_owned()
        })
        .collect();
        let expected: BTreeSet<String> = [
            "landscape_left",
            "landscape_right",
            "portrait",
            "portrait_upside_down",
            "unknown",
        ]
        .iter()
        .map(|word| (*word).to_owned())
        .collect();
        assert_eq!(words, expected);
    }

    /// Every member and every text a line of `running`, `launch` or `stop` can hold.
    fn members_and_texts(line: &Line) -> (BTreeSet<String>, BTreeSet<String>) {
        fn walk(
            value: &serde_json::Value,
            members: &mut BTreeSet<String>,
            texts: &mut BTreeSet<String>,
        ) {
            match value {
                serde_json::Value::Object(map) => {
                    for (key, member) in map {
                        members.insert(key.clone());
                        walk(member, members, texts);
                    }
                }
                serde_json::Value::Array(items) => {
                    for item in items {
                        walk(item, members, texts);
                    }
                }
                serde_json::Value::String(text) => {
                    texts.insert(text.clone());
                }
                _ => {}
            }
        }
        let (mut members, mut texts) = (BTreeSet::new(), BTreeSet::new());
        let value = serde_json::to_value(line).expect("a line serializes");
        walk(&value, &mut members, &mut texts);
        (members, texts)
    }

    /// The word a value of this program's own closed lists serializes to.
    fn word<T: Serialize>(value: T) -> String {
        serde_json::to_value(value)
            .expect("a word serializes")
            .as_str()
            .expect("a word is a text")
            .to_owned()
    }

    /// By construction, the lines of `running`, `launch` and `stop` hold no member that could
    /// carry another application's name, bundle identifier, path or pid: their members are
    /// these, every text in them is the bundle asked for or a word of this program's own
    /// closed lists (a command, a problem, an error kind, a stage, and, ACIB-3c fix round 1, a
    /// way the application was located), every member of `stage_ms` a stage's word, and every
    /// number a device's number, a pid of the bundle's own processes, (ACIB-3b) a stage's
    /// milliseconds or (fix round 1) `located`, a count of the list's entries. The new failure,
    /// `process_paths_unreadable`, is among the lines, and holds only these; (fix round 2) so is
    /// `stop`'s `application_entry_ambiguous`. (ACIB-3d) A line of `stop` names its way, a word of
    /// [`StopWay`]; one past the first look the answers to its kill requests, each a pid of the
    /// bundle's own processes with a word of [`KillAnswer`] and a message type (a number of the
    /// protocol), and of each stage after the first look whether it was reached, by the stage's
    /// word.
    #[test]
    fn the_application_answers_hold_no_member_for_another_applications_identity() {
        // ACIB-3b: `stage_ms`, whose members are the stages' words, and `stage` beside a
        // deadline, whose text is one of them; (ACIB-3c) the stages of the installation proxy's
        // lookup and of the instruments server; (ACIB-3d) the stages of `stop` after its first
        // look, also the members of `reached`.
        const STAGES: [Stage; 12] = [
            Stage::Usbmuxd,
            Stage::ApplicationLookup,
            Stage::ProxyTunnel,
            Stage::DiscoveryHandshake,
            Stage::ServiceConnection,
            Stage::ServerFirstMessage,
            Stage::ProcessList,
            Stage::LaunchRequest,
            Stage::KillRequests,
            Stage::KillReplies,
            Stage::LookConnection,
            Stage::LooksAfterKills,
        ];
        const MEMBERS: &[&str] = &[
            "application_located",
            "bundle",
            "command",
            "device",
            "error_kind",
            "extension_pids",
            "installed",
            "located",
            "message_type",
            "pid",
            "pids",
            "problem",
            "reached",
            "relaunch",
            "remaining",
            "replies",
            "reply",
            "reply_failure",
            "running",
            "signal_errors",
            "signalled",
            "stage",
            "stage_ms",
            "via",
            "was_running",
        ];
        let mut words: BTreeSet<String> = [BUNDLE.to_owned()].into();
        words.extend(
            [CommandName::Running, CommandName::Launch, CommandName::Stop]
                .into_iter()
                .map(word),
        );
        words.extend(
            [
                Problem::ServiceHandshakeFailed,
                Problem::ApplicationLookupFailed,
                Problem::ProcessListFailed,
                Problem::BundlePathUnreadable,
                Problem::ProcessPathsUnreadable,
                Problem::ApplicationEntryAmbiguous,
                Problem::LaunchFailed,
                Problem::StillRunning,
                Problem::NotInstalled,
                Problem::DeadlinePassed,
                Problem::ServiceUnreachable,
                Problem::NotPaired,
            ]
            .into_iter()
            .map(word),
        );
        words.extend(STAGES.into_iter().map(word));
        words.extend(StopWay::ALL.into_iter().map(word));
        words.extend(
            [
                KillAnswer::WithoutPayload,
                KillAnswer::WithPayload,
                KillAnswer::NotInTime,
            ]
            .into_iter()
            .map(word),
        );
        words.extend(
            [
                ReplyFailure::ReaderStopped,
                ReplyFailure::ChannelCancelled,
                ReplyFailure::Other,
            ]
            .into_iter()
            .map(word),
        );
        words.extend(
            [
                ApplicationLocated::DirectlyInside,
                ApplicationLocated::AtDirectory,
                ApplicationLocated::Both,
            ]
            .into_iter()
            .map(word),
        );
        words.extend(
            [
                ErrorKind::DeviceRefused,
                ErrorKind::AnswerUnreadable,
                ErrorKind::ConnectionFailed,
                ErrorKind::XpcFailed,
                ErrorKind::Other,
            ]
            .into_iter()
            .map(word),
        );
        let mut found = BTreeSet::new();
        for (line, text, _) in every_application_shape() {
            let (members, texts) = members_and_texts(&line);
            found.extend(members);
            for shown in texts {
                assert!(words.contains(&shown), "{shown} in {text}");
            }
        }
        let mut allowed: BTreeSet<String> =
            MEMBERS.iter().map(|member| (*member).to_owned()).collect();
        allowed.extend(STAGES.into_iter().map(word));
        assert_eq!(found, allowed);
    }

    /// `stop`'s answer, its verdict taken from its last look by `stopping.rs`: when the
    /// application itself does not run (installed or not), it is answered at the first look,
    /// nothing signalled and nothing remaining, its extensions' pids named, status 0; when every
    /// process it signalled is gone at the last look, status 0, the extensions it left named and
    /// no failure; when one remains, `running` true and `still_running`, status 1. A signal whose
    /// answer was an error or could not be read is a fact beside the verdict, its pid and kind
    /// in `signal_errors`: gone is stopped (status 0), still listed is `still_running`. A process
    /// the last look lists that was not signalled is named in `remaining` and is not
    /// `still_running`. A look that failed answers `process_list_failed` and keeps what was
    /// signalled. (ACIB-3c, fix round 1) An installed application that does not run names its
    /// first look's `located`; one not installed was not looked for and names none.
    #[test]
    fn stop_answers_when_nothing_runs_when_all_ended_and_when_one_remains() {
        let answered = |stop: Stop| -> (Stop, bool) {
            let line = Line::Stop(stop.clone());
            (stop, line.answered())
        };
        let signals = |signalled: &[u32], errors: &[(u32, ErrorKind)]| Signals {
            signalled: signalled.to_vec(),
            errors: errors
                .iter()
                .map(|(pid, error_kind)| SignalError::request(*pid, *error_kind))
                .collect(),
        };
        // (ACIB-3c, fix round 1) A look locating 180 processes.
        let last = |application: &[u32], extensions: &[u32]| Look {
            pids: Pids {
                application: application.to_vec(),
                extensions: extensions.to_vec(),
            },
            located: 180,
            application_located: (!application.is_empty())
                .then_some(ApplicationLocated::DirectlyInside),
        };
        // The answer after `signals` when the last look lists `last`, with the decision's
        // verdict on that look.
        let looked = |sent: Signals, seen: Look| {
            let verdict = crate::stopping::verdict(&sent.signalled, &seen.pids.application);
            answered(Stop::asked(3, BUNDLE).looked(sent, seen, verdict))
        };

        for (installed, extensions) in [(true, vec![530]), (true, Vec::new()), (false, Vec::new())]
        {
            let asked = Stop::asked(3, BUNDLE);
            let (nothing, status) = answered(if installed {
                asked.not_running(last(&[], &extensions))
            } else {
                asked.not_installed()
            });
            assert_eq!(nothing.located, installed.then_some(180));
            assert!(status, "nothing runs, installed {installed}");
            assert_eq!(nothing.installed, Some(installed));
            assert_eq!(nothing.was_running, Some(false));
            assert_eq!(nothing.signalled, Some(Vec::new()));
            assert_eq!(nothing.remaining, Some(Vec::new()));
            assert_eq!(nothing.running, Some(false));
            assert_eq!(nothing.extension_pids, Some(extensions));
            assert_eq!((nothing.problem, nothing.error_kind), (None, None));
            assert!(nothing.signal_errors.is_empty());
        }

        let (all_ended, status) = looked(signals(&[412, 415], &[]), last(&[], &[530]));
        assert!(status, "all ended, an extension left");
        assert_eq!(all_ended.was_running, Some(true));
        assert_eq!(all_ended.signalled, Some(vec![412, 415]));
        assert_eq!(all_ended.remaining, Some(Vec::new()));
        assert_eq!(all_ended.running, Some(false));
        assert_eq!(all_ended.extension_pids, Some(vec![530]));
        assert_eq!((all_ended.problem, all_ended.error_kind), (None, None));

        let (one_remains, status) = looked(signals(&[412, 413], &[]), last(&[413], &[]));
        assert!(!status, "one remains");
        assert_eq!(one_remains.remaining, Some(vec![413]));
        assert_eq!(one_remains.running, Some(true));
        assert_eq!(one_remains.problem, Some(Problem::StillRunning));
        assert_eq!(one_remains.error_kind, None);
        assert!(one_remains.signal_errors.is_empty());

        for kind in [ErrorKind::DeviceRefused, ErrorKind::AnswerUnreadable] {
            let beside = vec![SignalError::request(413, kind)];
            let (gone, status) = looked(signals(&[412, 413], &[(413, kind)]), last(&[], &[]));
            assert!(status, "a signal of kind {kind:?} whose process is gone");
            assert_eq!(gone.signal_errors, beside);
            assert_eq!((gone.running, gone.problem), (Some(false), None));
            let (listed, status) = looked(signals(&[412, 413], &[(413, kind)]), last(&[413], &[]));
            assert!(
                !status,
                "a signal of kind {kind:?} whose process is still listed"
            );
            assert_eq!(listed.signal_errors, beside);
            assert_eq!(listed.problem, Some(Problem::StillRunning));
        }

        let (started_anew, status) = looked(signals(&[412], &[]), last(&[777], &[]));
        assert!(status, "only a process it did not signal is listed");
        assert_eq!(started_anew.remaining, Some(vec![777]));
        assert_eq!(started_anew.running, Some(true));
        assert_eq!(started_anew.problem, None);

        let (unseen, status) = answered(Stop::asked(3, BUNDLE).look_failed(
            signals(&[412], &[(412, ErrorKind::Other)]),
            ErrorKind::ConnectionFailed,
        ));
        assert!(!status, "a look failed");
        assert_eq!(unseen.signalled, Some(vec![412]));
        assert_eq!(unseen.signal_errors.len(), 1);
        assert_eq!(
            (unseen.remaining, unseen.running, unseen.extension_pids),
            (None, None, None)
        );
        assert_eq!(unseen.problem, Some(Problem::ProcessListFailed));
        assert_eq!(unseen.error_kind, Some(ErrorKind::ConnectionFailed));
    }

    /// (ACIB-3d) Each way of `stop` has a word of its own, the one `--via` takes and the line
    /// names, and says whether it asks for answers; each answer to a kill request has a word of its
    /// own, and (fix round 1) so has each reason the wait for one failed, named in the failed
    /// request after its kind (a request that itself failed names none). Every line of `stop` the
    /// program prints names its way: its answer after the bundle, its
    /// refusal after the device; a line of another command, and a line with no way, is as it was.
    /// A line past the first look names the answers to the kill requests (only when there are
    /// any) and, of each stage after the first look, whether it was reached (`kill_replies` only of
    /// a way that asks for answers); a step after the kills that fails names its problem and kind,
    /// and no verdict.
    #[test]
    fn a_stop_line_names_its_way_the_answers_to_its_kills_and_the_stages_it_reached() {
        let mut ways = BTreeSet::new();
        for (way, expected, asks) in [
            (StopWay::Primitive, "primitive", true),
            (StopWay::Archived, "archived", true),
            (StopWay::Library, "library", false),
        ] {
            assert_eq!(word(way), expected);
            assert_eq!(way.word(), expected);
            assert_eq!(StopWay::from_word(expected), Some(way));
            assert_eq!(way.asks_answers(), asks, "{expected}");
            assert!(ways.insert(expected), "{expected} is one word");
        }
        assert_eq!(StopWay::ALL.len(), ways.len());
        for unknown in ["", "ARCHIVED", "screen_capture", "instruments", "relaunch"] {
            assert_eq!(StopWay::from_word(unknown), None, "{unknown}");
        }
        let mut answers = BTreeSet::new();
        for (answer, expected) in [
            (KillAnswer::WithoutPayload, "without_payload"),
            (KillAnswer::WithPayload, "with_payload"),
            (KillAnswer::NotInTime, "not_in_time"),
        ] {
            assert_eq!(word(answer), expected);
            assert!(answers.insert(expected), "{expected} is one word");
        }
        // Fix round 1: each reason a wait for a kill's answer failed has a word of its own, named
        // beside the failed request's pid and kind; a request that itself failed names none.
        let mut failures = BTreeSet::new();
        for (failure, expected) in [
            (ReplyFailure::ReaderStopped, "reader_stopped"),
            (ReplyFailure::ChannelCancelled, "channel_cancelled"),
            (ReplyFailure::Other, "other"),
        ] {
            assert_eq!(word(failure), expected);
            assert!(failures.insert(expected), "{expected} is one word");
            assert_eq!(
                serde_json::to_string(&SignalError::reply(
                    412,
                    ErrorKind::ConnectionFailed,
                    failure
                ))
                .expect("a failed request serializes"),
                format!(
                    r#"{{"pid":412,"error_kind":"connection_failed","reply_failure":"{expected}"}}"#
                )
            );
        }
        assert_eq!(
            serde_json::to_string(&SignalError::request(412, ErrorKind::Other))
                .expect("a failed request serializes"),
            r#"{"pid":412,"error_kind":"other"}"#
        );

        let stop = || Stop::asked(3, BUNDLE).installed();
        let not_running = Line::Stop(stop().not_running(Look {
            located: 4,
            ..Look::default()
        }));
        assert_eq!(
            json(&not_running.clone().on_stop_way(Some(StopWay::Library))),
            r#"{"command":"stop","device":3,"bundle":"com.example.app","via":"library","installed":true,"was_running":false,"signalled":[],"remaining":[],"running":false,"extension_pids":[],"located":4}"#
        );
        assert_eq!(not_running.clone().on_stop_way(None), not_running);
        let refused = Line::refused(
            Some(CommandName::Stop),
            Some(3),
            Failure::own(Problem::NotPaired),
        );
        assert_eq!(
            json(&refused.on_stop_way(Some(StopWay::Archived))),
            r#"{"command":"stop","device":3,"via":"archived","problem":"not_paired"}"#
        );
        let screenshot = Line::refused_on(
            Some(CommandName::Screenshot),
            Some(3),
            Some(Way::Instruments),
            Failure::own(Problem::OutExists),
        );
        assert_eq!(screenshot.clone().on_stop_way(None), screenshot);
        let running = Line::Running(Running::asked(3, BUNDLE).not_installed());
        assert_eq!(
            running.clone().on_stop_way(Some(StopWay::Archived)),
            running
        );

        let signals = Signals {
            signalled: vec![412],
            errors: Vec::new(),
        };
        let after = stop()
            .failed_after_kills(
                signals.clone(),
                Problem::ServiceHandshakeFailed,
                ErrorKind::Other,
            )
            .with_replies(vec![KillReply {
                pid: 412,
                reply: KillAnswer::WithoutPayload,
                message_type: Some(0),
            }])
            .with_reached(Reached {
                kill_requests: true,
                kill_replies: Some(true),
                look_connection: true,
                looks_after_kills: false,
            });
        let after = Line::Stop(after);
        assert_eq!(
            json(&after),
            r#"{"command":"stop","device":3,"bundle":"com.example.app","installed":true,"was_running":true,"signalled":[412],"replies":[{"pid":412,"reply":"without_payload","message_type":0}],"reached":{"kill_requests":true,"kill_replies":true,"look_connection":true,"looks_after_kills":false},"problem":"service_handshake_failed","error_kind":"other"}"#
        );
        assert!(!after.answered());
        let without_answers = stop()
            .looked(
                signals,
                Look {
                    located: 1,
                    ..Look::default()
                },
                Verdict::Stopped,
            )
            .with_replies(Vec::new());
        assert!(!json(&Line::Stop(without_answers)).contains("replies"));
    }

    /// (ACIB-3b, second round) By construction, a line of `probe` holds of each way only its
    /// word, its outcome's word, an error's kind, a count of entries, its milliseconds and the
    /// counts of what the phone sent: these members, every text a word of this program's own
    /// closed lists, each way and each outcome one word for one cause. A line of `services
    /// --service` holds the facts the handshake names of the service, and a name, an
    /// entitlement or a feature shaped like an identifier is withheld.
    #[test]
    fn the_probe_and_service_lines_hold_only_words_counts_and_service_facts() {
        let ways = [
            (ProbeWay::ListenScreenCapture, "listen_screen_capture"),
            (ProbeWay::Listen, "listen"),
            (ProbeWay::ListApps, "list_apps"),
            (ProbeWay::StreamApps, "stream_apps"),
            (ProbeWay::ListProcesses, "list_processes"),
            (ProbeWay::ListAppsAction, "list_apps_action"),
            (ProbeWay::ListProcessesAction, "list_processes_action"),
        ];
        let outcomes = [
            (ProbeOutcome::Answered, "answered"),
            (ProbeOutcome::Refused, "refused"),
            (ProbeOutcome::Failed, "failed"),
            (ProbeOutcome::ConnectionClosed, "connection_closed"),
            (
                ProbeOutcome::AnswerOnOtherChannel,
                "answer_on_other_channel",
            ),
            (ProbeOutcome::AnswerIncomplete, "answer_incomplete"),
            (ProbeOutcome::AnswerWithoutBody, "answer_without_body"),
            (ProbeOutcome::NoAnswer, "no_answer"),
            (ProbeOutcome::Silent, "silent"),
            (ProbeOutcome::ServiceNotListed, "service_not_listed"),
            (ProbeOutcome::ServiceUnreachable, "service_unreachable"),
            (ProbeOutcome::HandshakeFailed, "handshake_failed"),
            (ProbeOutcome::NotAsked, "not_asked"),
        ];
        assert_eq!(ProbeWay::ALL, ways.map(|(way, _)| way));
        let mut seen = BTreeSet::new();
        for expected in ways
            .iter()
            .map(|(way, expected)| (word(*way), *expected))
            .chain(
                outcomes
                    .iter()
                    .map(|(outcome, expected)| (word(*outcome), *expected)),
            )
        {
            assert_eq!(expected.0, expected.1);
            assert!(
                expected
                    .1
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b == b'_'),
                "{} is snake_case",
                expected.1
            );
            assert!(seen.insert(expected.1), "{} is one word", expected.1);
        }

        let results: Vec<ProbeResult> = outcomes
            .iter()
            .zip(ProbeWay::ALL.iter().cycle())
            .map(|((outcome, _), way)| ProbeResult {
                way: *way,
                outcome: *outcome,
                error_kind: Some(ErrorKind::Other),
                entries: Some(2),
                ms: 5,
                phone_sent: Some(phone_sent()),
            })
            .collect();
        let line = Line::Probe(Probe {
            command: CommandName::Probe,
            device: 3,
            wait_ms: 4_000,
            ways: results,
        });
        assert!(line.answered());
        let (members, texts) = members_and_texts(&line);
        let mut words: BTreeSet<String> = ways.iter().map(|(way, _)| word(*way)).collect();
        words.extend(outcomes.iter().map(|(outcome, _)| word(*outcome)));
        words.insert(word(CommandName::Probe));
        words.insert(word(ErrorKind::Other));
        for text in &texts {
            assert!(words.contains(text), "{text}");
        }
        let expected: BTreeSet<String> = [
            "bodyless",
            "bytes",
            "closed",
            "command",
            "content",
            "data",
            "device",
            "empty",
            "entries",
            "error_kind",
            "frames",
            "goaway",
            "headers",
            "ms",
            "other",
            "other_stream_bytes",
            "outcome",
            "pending_bytes",
            "pending_frame_bytes",
            "phone_sent",
            "read_failed",
            "replies_without_body",
            "reply",
            "root",
            "rst_stream",
            "settings",
            "unreadable",
            "wait_ms",
            "way",
            "ways",
            "window_update",
        ]
        .iter()
        .map(|member| (*member).to_owned())
        .collect();
        assert_eq!(members, expected);

        const RUN: &str = "0123456789abcdef0123456789abcdef01234567";
        let features = vec![
            format!("com.example.feature.{RUN}"),
            "com.example.feature.one".to_owned(),
        ];
        let detail = ServiceDetail::of(
            &format!("com.example.{RUN}"),
            49152,
            &format!("com.example.entitlement.{RUN}"),
            true,
            Some(2),
            Some(features.as_slice()),
        );
        assert_eq!(detail.name, "withheld");
        assert_eq!(detail.entitlement, "withheld");
        assert_eq!(
            detail.features,
            Some(vec![
                "withheld".to_owned(),
                "com.example.feature.one".to_owned()
            ])
        );
        let line = Line::Service(ServiceLine {
            command: CommandName::Services,
            device: 3,
            service: detail,
        });
        assert!(!json(&line).contains(RUN));
        assert!(line.answered());
    }

    /// Each error kind has a snake_case word of its own (ACIB-3c: `connection_closed` is no
    /// kind any more; it stays an outcome of `probe`).
    #[test]
    fn every_error_kind_has_a_word_of_its_own() {
        let words = [
            (ErrorKind::DeviceRefused, "device_refused"),
            (ErrorKind::AnswerUnreadable, "answer_unreadable"),
            (ErrorKind::ConnectionFailed, "connection_failed"),
            (ErrorKind::XpcFailed, "xpc_failed"),
            (ErrorKind::Other, "other"),
        ];
        let mut seen = BTreeSet::new();
        for (kind, expected) in words {
            assert_eq!(word(kind), expected);
            assert!(
                expected
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b == b'_'),
                "{expected} is snake_case"
            );
            assert!(seen.insert(expected), "{expected} is one word for one kind");
        }
    }
}
