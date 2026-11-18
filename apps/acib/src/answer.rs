// SPDX-License-Identifier: AGPL-3.0-only

//! The one JSON line `acib` prints for each command (board #479, ACIB-0 and ACIB-1).
//!
//! These types are the whole of what the program shows. None of them has a member for a
//! device's serial number, UDID, name or address, or for anything of a pairing record: the
//! line cannot carry them. A library error is shown through [`crate::redact`], which cuts a
//! text that could carry such a value to the error's kind. Of a picture of the screen the line
//! holds its format, its width and its height as its header gives them and its length, never
//! a byte of its content: the picture goes to the file `--out` names and nowhere else.

use crate::redact;
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
}

/// Why a command was refused or failed: this program's own words, not Runtime outcome codes.
/// Each word has one cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Problem {
    /// The command line is not one of the six commands with their options.
    CommandLineInvalid,
    /// The async runtime of this process could not be built.
    AsyncRuntimeFailed,
    /// The command did not finish within `--timeout-ms`; a screenshot's line names the `stage`
    /// it was waiting in.
    DeadlinePassed,
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
    /// connected to.
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
    /// The remote service discovery handshake lists no service of the way asked for.
    ServiceNotListed,
    /// A remote service inside the tunnel took the connection, but its own first exchange
    /// failed (a CoreDevice service's XPC handshake, the instruments server's first message).
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

/// One device of `acib devices`: its usbmuxd session number and its connection, nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceEntry {
    pub device: u32,
    pub connection: Link,
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

fn whole_milliseconds(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// The step a screenshot was waiting in when its deadline passed (`stage` beside
/// `deadline_passed`), by this program's own words, in the order a screenshot goes through
/// them. The file is written after the last of them, and no deadline passes while it is.
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
    /// The instruments server's first message (the capabilities it publishes), read before its
    /// screenshot channel is asked for: only the `instruments` way waits in it.
    ServerFirstMessage,
    /// The picture's request; for `instruments`, the request of its screenshot channel and then
    /// of the picture.
    PictureRequest,
}

/// `acib screenshot --device <n> --out <file> [--via <way>]`: the way used, the picture's
/// format, width and height as its header gives them, its length in bytes, the two durations
/// and the file it was written to. A picture whose header cannot be read keeps its length and
/// its file, names no format or size and answers its header's word (status 1). Nothing of the
/// picture's content is a member.
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

/// A refused or failed command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Refused {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<u32>,
    /// The way a refused `screenshot` was to take its picture.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<Way>,
    pub problem: Problem,
    /// Beside `deadline_passed` of a screenshot: the stage it was waiting in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<Stage>,
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
            via,
            problem: failure.problem,
            stage: None,
            out_left: None,
            error: failure.error,
        })
    }

    /// `deadline_passed` of `command` on `device`; a screenshot's line names the stage it was
    /// waiting in (`stage`, which no other command sets).
    pub fn deadline_passed(
        command: CommandName,
        device: Option<u32>,
        via: Option<Way>,
        stage: Option<Stage>,
    ) -> Self {
        Self::Refused(Refused {
            command: Some(command),
            device,
            via,
            problem: Problem::DeadlinePassed,
            stage,
            out_left: None,
            error: None,
        })
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
            via: Some(via),
            problem: failure.problem,
            stage: None,
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
            Self::Devices(_) | Self::Services(_) | Self::Images(_) => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        vec![
            (
                Line::Devices(Devices {
                    command: CommandName::Devices,
                    devices: vec![
                        DeviceEntry {
                            device: 3,
                            connection: Link::Usb,
                        },
                        DeviceEntry {
                            device: 7,
                            connection: Link::Network,
                        },
                        DeviceEntry {
                            device: 9,
                            connection: Link::Unknown,
                        },
                    ],
                }),
                r#"{"command":"devices","devices":[{"device":3,"connection":"usb"},{"device":7,"connection":"network"},{"device":9,"connection":"unknown"}]}"#,
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
                ),
                r#"{"command":"screenshot","device":3,"via":"instruments","problem":"deadline_passed","stage":"server_first_message"}"#,
                false,
            ),
            (
                Line::deadline_passed(CommandName::Devices, None, None, None),
                r#"{"command":"devices","problem":"deadline_passed"}"#,
                false,
            ),
            (
                Line::refused(None, None, Failure::own(Problem::CommandLineInvalid)),
                r#"{"problem":"command_line_invalid"}"#,
                false,
            ),
        ]
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
    /// pairing record, nor (ACIB-1) for a picture's content: every member any line holds is one
    /// of these (a screenshot's `stage` is a word of [`Stage`], its `out_left` a boolean).
    #[test]
    fn the_answers_hold_no_member_for_an_identifier() {
        const MEMBERS: &[&str] = &[
            "answer",
            "build_version",
            "bytes",
            "command",
            "connection",
            "contains",
            "count",
            "developer_mode",
            "device",
            "device_class",
            "devices",
            "elapsed_ms",
            "entitlement",
            "error",
            "error_kind",
            "format",
            "height",
            "image_type",
            "listed",
            "matching",
            "matching_count",
            "members",
            "mounted",
            "name",
            "out",
            "out_left",
            "paired",
            "password_protected",
            "port",
            "problem",
            "product_type",
            "product_version",
            "queries",
            "query",
            "refused",
            "request",
            "services",
            "stage",
            "user_applications",
            "value",
            "via",
            "width",
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
}
