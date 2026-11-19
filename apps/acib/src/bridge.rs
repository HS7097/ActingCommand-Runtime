// SPDX-License-Identifier: AGPL-3.0-only

//! The eleven commands against the device, through the `idevice` library (board #479, ACIB-0 to
//! ACIB-3c).
//!
//! (ACIB-3b, second round) `probe` asks the application service each way of [`ProbeWay::ALL`]
//! on a connection of its own, each read through `tap.rs`, and answers per way only a word, an
//! error's kind, a count and counts of what the phone sent. (ACIB-3b, third round) `probe` also
//! asks the two lists in the form of the screenshot's request, which the phone answers (the
//! library's `invoke_with_plist_action`, with the action the screenshot's rule gives:
//! [`action_of`]).
//!
//! (ACIB-3c) `running`, `launch` and `stop` no longer ask the application service, which
//! answered none of seven read-only requests on a phone with iOS 27.0.1 while the instruments
//! server answered there at once: they look the one bundle up in the installation proxy
//! ([`looked_up`]), and ask the instruments server inside the tunnel ([`instruments_server`]) for
//! its list of running processes (its device information channel, `runningProcesses`), to launch
//! (its process control's `launch_app`) and to kill (`kill_app`). Only `probe` asks the
//! application service.
//!
//! (ACIB-3d) On a phone with iOS 27.0.1 the library's `kill_app` ended nothing, and the look made
//! right after it, on the same connection, failed at once. `stop` sends its kill requests the way
//! `--via` names ([`StopWay`]: the pid as a 32-bit number, the default, or archived as a number,
//! each with its answer asked for and waited for, or the library's `kill_app` unchanged;
//! [`stopped`]), and makes the looks after the kills on a connection of its own to the instruments
//! server, inside the same tunnel ([`instruments_again`]): whatever the kills did to the first
//! connection, the looks do not depend on it, and a look that still fails names its stage. (Fix
//! round 1) Each answer to a kill request is paired with its request by its number
//! ([`kill_answers`]), and a wait for one that failed names why, as far as the library's error
//! value tells ([`reply_failure`]).
//!
//! Everything here only reads, but for `launch`, which launches one application, and `stop`,
//! which ends one application's own processes (ACIB-3); the library's process control client is
//! made only by them ([`launched`], [`kill_by_library`]), and (ACIB-3d) the process control
//! channel only by them ([`stopped`]). Of the installation proxy's record and of the
//! instruments server's list of processes nothing leaves this module but whether the one bundle
//! asked for is installed and the pids of its own processes, the application's and its
//! extensions' (`matching.rs`), (fix round 1) of the list a count and a word ([`Look`]), and of
//! the errors only their kind ([`service_error_kind`]).
//! usbmuxd is reached where the library finds it on this platform
//! (on Windows the TCP port 27015 of 127.0.0.1, which Apple's device service serves). The
//! pairing record usbmuxd holds is asked for, kept in memory for the command and never shown.
//! The tunnel of iOS 17.4 and later is the lockdown service CoreDeviceProxy with the library's
//! own TCP stack inside this process: no network adapter is created. No library type is ever
//! printed with its `Debug` form: only the values the answer types name leave this module. The
//! identifier usbmuxd lists a device with (the library's `UsbmuxdDevice::udid`, usbmuxd's
//! `SerialNumber`) never leaves it either: a device is named by its number or by its key
//! (`key.rs`). A picture of the screen is written, as the service gave it, to the file `--out`
//! names (see `out_file.rs`) or after the line on the standard output (`framing.rs`), and is
//! kept nowhere else.

use crate::answer::{
    ApplicationLocated, Apps, Candidate, CommandName, DeviceEntry, Devices, Elapsed, ErrorKind,
    Failure, ImageQuery, ImageQueryName, Images, Info, KillAnswer, KillReply, Launch, Line, Link,
    ListedImage, Mounted, Orientation, Orientations, Probe, ProbeOutcome, ProbeResult, ProbeWay,
    Problem, Reached, Reader, Reading, ReplyFailure, Running, Screenshot, ServiceDetail,
    ServiceEntry, ServiceLine, Services, SignalError, Signals, Stage, Stop, StopWay, Way,
    whole_milliseconds,
};
use crate::args::{KeyVariables, Out, Request, Target};
use crate::framing::Printed;
use crate::key::{DeviceKey, Salt};
use crate::matching::{self, Application, Installation, Look};
use crate::out_file::{self, NotWritten};
use crate::redact;
use crate::stages::Steps;
use crate::stopping::{self, LOOK_INTERVAL, Next};
use crate::tap::{self, PhoneSent, Seen, Tapped};
use idevice::pairing_file::PairingFile;
use idevice::provider::{IdeviceProvider, RsdProvider, UsbmuxdProvider};
use idevice::services::core_device::{
    AppServiceClient, CoreDeviceError, CoreDeviceServiceClient, ImageFormat,
    ScreenCaptureServiceClient,
};
use idevice::services::core_device_proxy::CoreDeviceProxy;
use idevice::services::dvt::errors::DvtError;
use idevice::services::dvt::message::{AuxValue, Message};
use idevice::services::dvt::process_control::ProcessControlClient;
use idevice::services::dvt::remote_server::{Channel, RemoteServerClient};
use idevice::services::dvt::screenshot::ScreenshotClient;
use idevice::services::installation_proxy::InstallationProxyClient;
use idevice::services::lockdown::LockdownClient;
use idevice::services::mobile_image_mounter::ImageMounter;
use idevice::services::rsd::RsdHandshake;
use idevice::services::springboardservices::{InterfaceOrientation, SpringBoardServicesClient};
use idevice::tcp::handle::AdapterHandle;
use idevice::usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdConnection, UsbmuxdDevice};
use idevice::xpc::{Dictionary, XPCObject};
use idevice::{Idevice, IdeviceError, IdeviceService, ReadWrite, RemoteXpcClient, RsdService};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The label the library sends with each connection.
const LABEL: &str = "acib";
/// The lockdown domain of the developer mode status.
const AMFI_DOMAIN: &str = "com.apple.security.mac.amfi";
/// The image types the image mounter's lookup is asked for, in this order: the personalized
/// image of iOS 17 and later, then the developer disk image of earlier versions.
const IMAGE_KINDS: [&str; 2] = ["Personalized", "Developer"];
/// The application type the installation proxy is asked for: the user's own applications.
const USER_APPLICATIONS: &str = "User";
/// (ACIB-3c) The application type of the installation proxy's lookup of one bundle in `running`,
/// `launch` and `stop`: every type ("Any" for all applications, the library's default:
/// idevice 0.1.68, `services/installation_proxy.rs` 65-68, 92), so that an application a person
/// installed and one the phone comes with are both found.
const ANY_APPLICATION: &str = "Any";
/// (ACIB-3c) The member of the installation proxy's record that holds the on-device path of the
/// application's bundle, as the library's own reader of such a record names it
/// (`services/dvt/xctest/mod.rs` 106 and 184, "On-device path of the runner app bundle").
const APPLICATION_PATH: &str = "Path";
/// (ACIB-3c) The instruments server's device information channel and its one request this
/// program sends, which takes no argument (idevice 0.1.68, `services/dvt/device_info.rs` 27-31
/// and 41-46: the library's `DeviceInfoClient`, under a feature `acib` does not turn on, sends
/// it so; `acib` sends the same through the library's public channel of the `dvt` feature).
const DEVICE_INFO_CHANNEL: &str = "com.apple.instruments.server.services.deviceinfo";
const RUNNING_PROCESSES: &str = "runningProcesses";
/// (ACIB-3c) The instruments server's root channel, on which it sends its first message.
const ROOT_CHANNEL: i32 = 0;
/// (ACIB-3d) The instruments server's process control channel, as the library's
/// `ProcessControlClient::new` opens it (idevice 0.1.68, `services/dvt/process_control.rs`
/// 132-138), and the request of its `kill_app` (286-296), which the ways of `stop` that ask for
/// answers send themselves, through the library's public channel.
const PROCESS_CONTROL_CHANNEL: &str = "com.apple.instruments.server.services.processcontrol";
const KILL_PID: &str = "killPid:";
/// (ACIB-3d, fix round 1) The number the library gives the first kill request of a way that asks
/// for answers, on the connection of `stop`'s first look. The library numbers the messages it
/// sends on a connection 1, 2, 3, … in the order it sends them (its `current_message`, from 0, one
/// added before each request is written: idevice 0.1.68, `services/dvt/remote_server.rs` 249, 791
/// and 804); its reader's own replies to the phone take the number of what they answer, not one of
/// its own (1020-1032, 1110-1130, 1281-1295, `services/dvt/message.rs` 728-779), and making the
/// client sends nothing (`services/dvt/mod.rs` 43-45). Before the kills `acib` sends on that
/// connection three requests: the first look's channel request and its `runningProcesses`
/// ([`look`]), then the process control channel's request; each kill request after the first is
/// numbered one above the one before ([`send_kills`]). The server answers a request with the
/// request's number (the library pairs every answer it waits for by it: `remote_server.rs`
/// 1138-1161), so [`kill_answers`] pairs each answer with its request by it.
const FIRST_KILL_NUMBER: u32 = 4;

/// The client of CoreDevice's application service inside the tunnel.
type AppService = AppServiceClient<Box<dyn ReadWrite>>;
/// The client of CoreDevice's screen capture service inside the tunnel.
type ScreenCaptureService = ScreenCaptureServiceClient<Box<dyn ReadWrite>>;
/// The library's RemoteXPC client on a connection inside the tunnel.
type Xpc = RemoteXpcClient<Box<dyn ReadWrite>>;
/// (ACIB-3b, third round) The library's CoreDevice client on a connection inside the tunnel: the
/// one each of its CoreDevice clients holds (the application service's, the screen capture
/// service's), public with its `invoke_with_plist_action` (idevice 0.1.68,
/// `services/core_device/mod.rs` 46-82).
type CoreDevice = CoreDeviceServiceClient<Box<dyn ReadWrite>>;
/// (ACIB-3c) The instruments server's client on a connection inside the tunnel
/// (`com.apple.instruments.dtservicehub`: idevice 0.1.68, `services/dvt/mod.rs` 37-46).
type Instruments = RemoteServerClient<Box<dyn ReadWrite>>;
/// (ACIB-3c) A channel of the instruments server, opened by a request of its own and borrowing
/// the client while it is held (`services/dvt/remote_server.rs` 425-458, 1298-1355).
type ServerChannel<'a> = Channel<'a, Box<dyn ReadWrite>>;
/// (ACIB-3c) The tunnel, which stays open while it is held, (ACIB-3d) the port inside it the
/// discovery handshake names for the instruments server, and the instruments server's client in
/// it, or the kind of the error its first message failed with.
type ServerConnection = (AdapterHandle, u16, Result<Instruments, ErrorKind>);
/// (ACIB-3d) Another connection to the instruments server, its first message read, or what stopped
/// it: `service_unreachable` (the connection) or `service_handshake_failed` (its first message),
/// with the error's kind.
type Reconnected = Result<Instruments, (Problem, ErrorKind)>;

/// (ACIB-3d, fix round 1) A kill request written: the pid it is for and the number the library gave
/// it on its connection ([`FIRST_KILL_NUMBER`]), which the server's answer to it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KillSent {
    pid: u32,
    number: u32,
}

/// (ACIB-3b, third round) The prefixes of a CoreDevice feature's identifier and of an action's.
/// The screenshot the phone answers is asked for by the feature
/// `com.apple.coredevice.feature.capturescreenshot` with the action
/// `com.apple.coredevice.action.capturescreenshot` (idevice 0.1.68,
/// `services/core_device/screencaptureservices.rs` 59-66): the same name after the other prefix
/// ([`action_of`]). That is the rule of this one pair: the library's one other pair, the location
/// service's, names another action than its feature (`feature.simulatelocation`,
/// `action.availablelocationscenarios`: `services/core_device/location_service.rs` 46-53).
const FEATURE_PREFIX: &str = "com.apple.coredevice.feature.";
const ACTION_PREFIX: &str = "com.apple.coredevice.action.";
/// The features of the application service's lists of applications and of processes, as the
/// library's `list_apps` and `list_processes` name them (`services/core_device/app_service.rs`
/// 159 and 305) and the service lists them in the discovery handshake.
const LIST_APPS_FEATURE: &str = "com.apple.coredevice.feature.listapps";
const LIST_PROCESSES_FEATURE: &str = "com.apple.coredevice.feature.listprocesses";
/// The member of the output of the list of processes that holds it, as the library's
/// `list_processes` reads it (`services/core_device/app_service.rs` 308-311).
const PROCESS_TOKENS: &str = "processTokens";

/// (ACIB-3b, second round) The feature of the streamed list of applications, as the application
/// service names it among its features in the discovery handshake and `probe`'s `stream_apps`
/// asks for it (idevice 0.1.68, `services/core_device/app_service.rs` 209).
const STREAMED_LIST_FEATURE: &str = "com.apple.coredevice.feature.streamapplist";
/// The member of a streamed answer that holds the stream's status (`services/core_device/mod.rs`
/// 252-253).
const SIDE_CHANNEL_STATUS: &str = "CoreDevice.XPCMessageKey.sideChannelStatus";
/// The version of CoreDevice the library sends with every request (`services/core_device/mod.rs`
/// 44), and its two components.
const CORE_DEVICE_VERSION: &str = "629.3";
const CORE_DEVICE_VERSION_COMPONENTS: [u64; 2] = [629, 3];
/// The magic, the version and the type of an XPC object holding a UUID, as the library's encoder
/// writes one and its decoder reads it (`xpc/format.rs` 80, 184-185, 262-265, 275-291).
const XPC_OBJECT_MAGIC: u32 = 0x4213_3742;
const XPC_OBJECT_VERSION: u32 = 0x0000_0005;
const XPC_UUID_TYPE: u32 = 0x0000_a000;
/// The flags of the application service's list of applications, in the library's order
/// (`app_list_options` in `services/core_device/app_service.rs` 381-398): App Clips, removable,
/// hidden, internal and default applications asked as `probe` asks `list_apps`, then the three
/// keys the library adds, each `false`.
const LIST_OPTIONS: [(&str, bool); 8] = [
    ("includeAppClips", false),
    ("includeRemovableApps", true),
    ("includeHiddenApps", true),
    ("includeInternalApps", false),
    ("includeDefaultApps", true),
    ("includeAppGroupIdentifiers", false),
    ("includeContainerPaths", false),
    ("requireContainerAccess", false),
];

/// (ACIB-3c) What `running`, `launch` and `stop` know once the installation proxy has been asked
/// about the bundle: the device, this host's pairing record for it (kept in memory for the
/// command, never shown), and what the lookup says of the bundle, or the kind of the error it
/// failed with.
struct LookedUp {
    device: UsbmuxdDevice,
    record: PairingFile,
    installation: Result<Installation, ErrorKind>,
}

/// (ACIB-3c) What `running`, `launch` or `stop` does after a step whose answer decides it
/// ([`running_after_lookup`], [`launch_after_lookup`], [`stop_after_lookup`],
/// [`stop_after_first_look`]).
#[derive(Debug, PartialEq, Eq)]
enum Then<T> {
    /// The command answers this line now: nothing more is asked of the phone or sent to it.
    /// (ACIB-3d) Boxed, so that the line's size, which grew with `stop`'s new members, sets no
    /// size of its own beside a small `T`.
    Answer(Box<Line>),
    /// The command goes on with this.
    GoOn(T),
}

impl<T> Then<T> {
    /// The command answers `line` now.
    fn answer(line: Line) -> Self {
        Self::Answer(Box::new(line))
    }
}

/// What a command keeps while it runs, for the line of a deadline that passes in it: the
/// command's deadline and start, the stage a screenshot is in (no other command sets it), the
/// stages of `running`, `launch` and `stop` (`stages.rs`; no other command enters one) and the
/// number of the device once it is found (a device named by its key has none before).
pub struct Context {
    /// When the command's deadline passes (`None`: further than the clock reaches).
    pub deadline: Option<tokio::time::Instant>,
    /// When the command started.
    pub started: Instant,
    pub stage: Cell<Option<Stage>>,
    pub steps: RefCell<Steps>,
    pub device: Cell<Option<u32>>,
}

impl Context {
    /// The context of a command that started at `started` and whose deadline passes at
    /// `deadline`.
    pub fn new(deadline: Option<tokio::time::Instant>, started: Instant) -> Self {
        Self {
            deadline,
            started,
            stage: Cell::new(None),
            steps: RefCell::new(Steps::default()),
            device: Cell::new(None),
        }
    }

    /// (ACIB-3b) `running`, `launch` or `stop` enters `stage` now.
    fn enter(&self, stage: Stage) {
        self.steps.borrow_mut().enter(stage, self.started.elapsed());
    }

    /// (ACIB-3b) The line of `running`, `launch` or `stop` that ends now, with how long each of
    /// its stages took.
    fn timed(&self, line: Line) -> Line {
        let times = self.steps.borrow_mut().end(self.started.elapsed());
        line.timed(times)
    }
}

/// Runs one request and gives what it prints.
pub async fn answer(request: Request, context: &Context) -> Printed {
    let (command, given, via) = (request.name(), request.device(), request.via());
    // (ACIB-3d) Every line of `stop`, its answer and its refusal alike, names the way of its kill
    // requests.
    let stop_way = request.stop_way();
    let started = context.started;
    let answered = match request {
        Request::Devices(variables) => devices(variables.as_ref()).await.map(Printed::line),
        Request::Info(target) => info(&target, context).await.map(Printed::line),
        Request::Services(target) => services(&target, context).await.map(Printed::line),
        Request::Images(target) => images(&target, context).await.map(Printed::line),
        Request::Apps {
            device: target,
            contains,
            max_matching,
        } => apps(&target, &contains, max_matching, context)
            .await
            .map(Printed::line),
        Request::Screenshot {
            device: target,
            out,
            via: way,
        } => screenshot(&target, &out, way, started, context).await,
        Request::Orientation {
            device: target,
            via: reader,
        } => orientation(&target, reader, context)
            .await
            .map(Printed::line),
        Request::Running {
            device: target,
            bundle,
        } => running(&target, &bundle, context)
            .await
            .map(|line| Printed::line(context.timed(line))),
        Request::Launch {
            device: target,
            bundle,
            relaunch,
        } => launch(&target, &bundle, relaunch, context)
            .await
            .map(|line| Printed::line(context.timed(line))),
        Request::Stop {
            device: target,
            bundle,
            via: way,
            wait_ms,
        } => stop(
            &target,
            &bundle,
            way,
            Duration::from_millis(wait_ms),
            context,
        )
        .await
        .map(|line| Printed::line(context.timed(line).on_stop_way(Some(way)))),
        Request::ServiceByName {
            device: target,
            name,
        } => service(&target, &name, context).await.map(Printed::line),
        Request::Probe {
            device: target,
            wait_ms,
        } => probe(&target, wait_ms, context).await.map(Printed::line),
    };
    answered.unwrap_or_else(|failure| {
        let device = given.or(context.device.get());
        Printed::line(Line::refused_on(Some(command), device, via, failure).on_stop_way(stop_way))
    })
}

/// The devices usbmuxd lists now; with the variables of the salts, each with its key under
/// each salt. The salts are read before usbmuxd is asked.
async fn devices(variables: Option<&KeyVariables>) -> Result<Line, Failure> {
    let salts = match variables {
        None => None,
        Some(KeyVariables { key, binding }) => {
            let key_salt = salt_of(key, Problem::KeySaltUnusable)?;
            let binding_salt = match binding {
                None => None,
                Some(variable) => Some(salt_of(variable, Problem::BindingSaltUnusable)?),
            };
            Some((key_salt, binding_salt))
        }
    };
    let listed = list().await?;
    let devices: Vec<DeviceEntry> = listed
        .iter()
        .map(|device| {
            let key_under = |salt: &Salt| DeviceKey::of(salt, &device.udid);
            DeviceEntry {
                device: device.device_id,
                connection: match device.connection_type {
                    Connection::Usb => Link::Usb,
                    Connection::Network(_) => Link::Network,
                    _ => Link::Unknown,
                },
                device_key: salts.as_ref().map(|(key_salt, _)| key_under(key_salt)),
                binding_key: salts
                    .as_ref()
                    .and_then(|(_, binding_salt)| binding_salt.as_ref())
                    .map(key_under),
            }
        })
        .collect();
    Ok(Line::Devices(Devices {
        command: CommandName::Devices,
        devices,
    }))
}

async fn info(target: &Target, context: &Context) -> Result<Line, Failure> {
    let device = locate(target, context).await?;
    let number = device.device_id;
    let provider = device.to_provider(UsbmuxdAddr::default(), LABEL);
    let mut lockdown = <LockdownClient as IdeviceService>::connect(&provider)
        .await
        .map_err(|error| Failure::library(Problem::LockdownUnreachable, &error))?;
    let mut answer = Info {
        command: CommandName::Info,
        device: number,
        product_version: read_text(&mut lockdown, "ProductVersion").await?,
        product_type: read_text(&mut lockdown, "ProductType").await?,
        build_version: read_text(&mut lockdown, "BuildVersion").await?,
        device_class: read_text(&mut lockdown, "DeviceClass").await?,
        paired: None,
        password_protected: None,
        developer_mode: None,
        problem: None,
        error: None,
    };
    let record = match pairing(&device).await {
        Ok(Some(record)) => record,
        Ok(None) => {
            answer.paired = Some(false);
            return Ok(Line::Info(answer.stopped(Failure::own(Problem::NotPaired))));
        }
        Err(failure) => return Ok(Line::Info(answer.stopped(failure))),
    };
    answer.paired = Some(true);
    if let Err(error) = lockdown.start_session(&record).await {
        let failure = Failure::library(Problem::SessionRefused, &error);
        return Ok(Line::Info(answer.stopped(failure)));
    }
    match read_flag(&mut lockdown, "PasswordProtected", None).await {
        Ok(reading) => answer.password_protected = Some(reading),
        Err(failure) => return Ok(Line::Info(answer.stopped(failure))),
    }
    match read_flag(&mut lockdown, "DeveloperModeStatus", Some(AMFI_DOMAIN)).await {
        Ok(reading) => answer.developer_mode = Some(reading),
        Err(failure) => return Ok(Line::Info(answer.stopped(failure))),
    }
    Ok(Line::Info(answer))
}

async fn services(target: &Target, context: &Context) -> Result<Line, Failure> {
    let device = locate(target, context).await?;
    let number = device.device_id;
    let record = paired_record(&device).await?;
    let proxy = proxy_service(&device, &record).await?;
    let (_tunnel, handshake) = open_tunnel(proxy, || {}).await?;
    // Of each service the handshake describes, its port and the entitlement it names are
    // shown; its other members are not.
    let mut services: Vec<ServiceEntry> = handshake
        .services
        .iter()
        .map(|(name, service)| ServiceEntry {
            name: redact::shown_name(name),
            port: service.port,
            entitlement: redact::shown_name(&service.entitlement),
        })
        .collect();
    services.sort_by(|one, other| one.name.cmp(&other.name).then(one.port.cmp(&other.port)));
    Ok(Line::Services(Services {
        command: CommandName::Services,
        device: number,
        count: services.len(),
        services,
    }))
}

/// (ACIB-3b, second round) `services --service <name>`: in the tunnel of `services`, what the
/// discovery handshake says of the one service `name` (its port, its entitlement, whether it uses
/// RemoteXPC, its version and its features: idevice 0.1.68, `services/rsd.rs` 77-104), each name
/// cut as `services` cuts one; `service_not_listed` when the handshake lists no such service. No
/// service is opened.
async fn service(target: &Target, name: &str, context: &Context) -> Result<Line, Failure> {
    let device = locate(target, context).await?;
    let number = device.device_id;
    let record = paired_record(&device).await?;
    let proxy = proxy_service(&device, &record).await?;
    let (_tunnel, handshake) = open_tunnel(proxy, || {}).await?;
    let listed = handshake
        .services
        .get(name)
        .ok_or(Failure::own(Problem::ServiceNotListed))?;
    Ok(Line::Service(ServiceLine {
        command: CommandName::Services,
        device: number,
        service: ServiceDetail::of(
            name,
            listed.port,
            &listed.entitlement,
            listed.uses_remote_xpc,
            listed.service_version,
            listed.features.as_deref(),
        ),
    }))
}

/// (ACIB-3b, second round) `probe`, read-only: in the tunnel of `services`, each way of
/// [`ProbeWay::ALL`] in turn, each on a connection of its own to its service's port, read through
/// a [`Tapped`] stream, each waited for `wait_ms` within the command's deadline (a way the
/// deadline leaves no time for is `not_asked`). Nothing changes on the phone: the two `listen`
/// ways ask nothing, the others ask for a list. The application service not listed refuses the
/// probe (`service_not_listed`); the screen capture service not listed is its way's word. The
/// ways after the application service's `listen` (three, and the two of the screenshot's form of
/// the third round) count the phone's messages beyond what it counted (its answers to the
/// handshake alone).
async fn probe(target: &Target, wait_ms: u64, context: &Context) -> Result<Line, Failure> {
    let device = locate(target, context).await?;
    let number = device.device_id;
    let record = paired_record(&device).await?;
    let proxy = proxy_service(&device, &record).await?;
    let (mut tunnel, handshake) = open_tunnel(proxy, || {}).await?;
    let application = port_of(&handshake, &<AppService as RsdService>::rsd_service_name())?;
    let screen = port_of(
        &handshake,
        &<ScreenCaptureService as RsdService>::rsd_service_name(),
    )
    .ok();
    let wait = Duration::from_millis(wait_ms);
    let mut baseline: Option<PhoneSent> = None;
    let mut ways = Vec::with_capacity(ProbeWay::ALL.len());
    for way in ProbeWay::ALL {
        let (port, compared) = match way {
            ProbeWay::ListenScreenCapture => (screen, None),
            ProbeWay::Listen => (Some(application), None),
            ProbeWay::ListApps
            | ProbeWay::StreamApps
            | ProbeWay::ListProcesses
            | ProbeWay::ListAppsAction
            | ProbeWay::ListProcessesAction => (Some(application), baseline.as_ref()),
        };
        let result = probe_way(&mut tunnel, way, port, compared, wait, context.deadline).await;
        if way == ProbeWay::Listen {
            baseline = result.phone_sent;
        }
        ways.push(result);
    }
    Ok(Line::Probe(Probe {
        command: CommandName::Probe,
        device: number,
        wait_ms,
        ways,
    }))
}

/// One way of `probe` on the port `port` (`None`: its service is not listed): asked within
/// `wait`, and no later than `deadline`, on a connection read through a [`Tapped`] stream; its
/// word from what the library returned or, when the wait passed first, from what the phone sent
/// beyond `baseline` (`tap.rs`).
async fn probe_way(
    tunnel: &mut AdapterHandle,
    way: ProbeWay,
    port: Option<u16>,
    baseline: Option<&PhoneSent>,
    wait: Duration,
    deadline: Option<tokio::time::Instant>,
) -> ProbeResult {
    let Some(port) = port else {
        return ProbeResult::unconnected(way, ProbeOutcome::ServiceNotListed, None);
    };
    let now = tokio::time::Instant::now();
    let until = match deadline {
        Some(deadline) => deadline.min(now + wait),
        None => now + wait,
    };
    if until <= now {
        return ProbeResult::unconnected(way, ProbeOutcome::NotAsked, None);
    }
    let started = Instant::now();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let asked = tokio::time::timeout_at(until, ask(tunnel, way, port, &seen)).await;
    let ms = whole_milliseconds(started.elapsed());
    let sent = tap::summary_of(&seen);
    let (outcome, error_kind, entries) = match asked {
        Ok(Ok(entries)) => (ProbeOutcome::Answered, None, entries),
        Ok(Err(Asked::Unreachable(kind))) => {
            let mut unreachable =
                ProbeResult::unconnected(way, ProbeOutcome::ServiceUnreachable, Some(kind));
            unreachable.ms = ms;
            return unreachable;
        }
        Ok(Err(Asked::Handshake(kind))) => (ProbeOutcome::HandshakeFailed, Some(kind), None),
        Ok(Err(Asked::Request(kind))) => {
            let (outcome, kind) = tap::failed(&sent, kind);
            (outcome, kind, None)
        }
        Err(_) => (tap::unanswered(&sent, baseline, way.reads()), None, None),
    };
    ProbeResult {
        way,
        outcome,
        error_kind,
        entries,
        ms,
        phone_sent: Some(sent),
    }
}

/// How one way of `probe` stopped before the library returned what it asks for.
enum Asked {
    /// The connection to the service's port failed.
    Unreachable(ErrorKind),
    /// The library's RemoteXPC handshake on it failed.
    Handshake(ErrorKind),
    /// The request, or the reading of its answer, failed.
    Request(ErrorKind),
}

/// One way of `probe` asked on a new connection to `port`, its reads counted in `seen`: the count
/// of the list's entries it answered, none for a `listen` (whose message is dropped unread).
async fn ask(
    tunnel: &mut AdapterHandle,
    way: ProbeWay,
    port: u16,
    seen: &Arc<Mutex<Seen>>,
) -> Result<Option<usize>, Asked> {
    let stream = tunnel
        .connect_to_service_port(port)
        .await
        .map_err(|error| Asked::Unreachable(service_error_kind(&error)))?;
    let tapped: Box<dyn ReadWrite> = Box::new(Tapped::new(stream, Arc::clone(seen)));
    let handshake = |error: IdeviceError| Asked::Handshake(service_error_kind(&error));
    let request = |error: IdeviceError| Asked::Request(service_error_kind(&error));
    match way {
        ProbeWay::ListenScreenCapture | ProbeWay::Listen => {
            let mut client = Xpc::new(tapped).await.map_err(handshake)?;
            client.do_handshake().await.map_err(handshake)?;
            client.recv_any().await.map_err(request)?;
            Ok(None)
        }
        ProbeWay::ListApps => {
            let mut client = <AppService as RsdService>::from_stream(tapped)
                .await
                .map_err(handshake)?;
            let applications = client
                .list_apps(false, true, true, false, true)
                .await
                .map_err(request)?;
            Ok(Some(applications.len()))
        }
        ProbeWay::StreamApps => {
            let mut client = Xpc::new(tapped).await.map_err(handshake)?;
            client.do_handshake().await.map_err(handshake)?;
            let applications = streamed_applications(&mut client)
                .await
                .map_err(Asked::Request)?;
            Ok(Some(applications))
        }
        ProbeWay::ListProcesses => {
            let mut client = <AppService as RsdService>::from_stream(tapped)
                .await
                .map_err(handshake)?;
            let processes = client.list_processes().await.map_err(request)?;
            Ok(Some(processes.len()))
        }
        // (ACIB-3b, third round) The screenshot's form: the library's CoreDevice client (its
        // RemoteXPC handshake, as the screen capture service's `from_stream`), then its
        // `invoke_with_plist_action` with the feature, the action the screenshot's rule gives for
        // it and the input `list_apps` or `list_processes` sends (`mod.rs` 68-82, 161-230:
        // protocol version 2, `CoreDevice.actionIdentifier`, the answer read on the reply
        // channel). The output is counted as the library's own `list_apps` (a list) and
        // `list_processes` (the list `processTokens`) read theirs; another shape is
        // `answer_unreadable`. Of an entry nothing but the count leaves this function.
        ProbeWay::ListAppsAction | ProbeWay::ListProcessesAction => {
            let processes = way == ProbeWay::ListProcessesAction;
            let (feature, input) = if processes {
                (LIST_PROCESSES_FEATURE, Dictionary::new())
            } else {
                (LIST_APPS_FEATURE, list_options())
            };
            let action = action_of(feature).ok_or(Asked::Request(ErrorKind::Other))?;
            // The library takes the input as a property list dictionary, which it turns back
            // into XPC (`mod.rs` 74-75); its own conversion makes the one of the other
            // (`xpc/format.rs` 153-172).
            let input = XPCObject::Dictionary(input)
                .to_plist()
                .into_dictionary()
                .ok_or(Asked::Request(ErrorKind::Other))?;
            let mut client = CoreDevice::new(tapped).await.map_err(handshake)?;
            let output = client
                .invoke_with_plist_action(feature, input, action)
                .await
                .map_err(request)?;
            let entries = if processes {
                output
                    .as_dictionary()
                    .and_then(|members| members.get(PROCESS_TOKENS))
                    .and_then(|tokens| tokens.as_array())
                    .map(Vec::len)
            } else {
                output.as_array().map(Vec::len)
            };
            entries
                .map(Some)
                .ok_or(Asked::Request(ErrorKind::AnswerUnreadable))
        }
    }
}

/// (ACIB-3b, third round) The action the screenshot's rule gives for the feature `feature`: the
/// name after the prefix of a feature, after the prefix of an action ([`FEATURE_PREFIX`],
/// [`ACTION_PREFIX`]); `None` for an identifier without the prefix of a feature.
fn action_of(feature: &str) -> Option<String> {
    feature
        .strip_prefix(FEATURE_PREFIX)
        .map(|name| format!("{ACTION_PREFIX}{name}"))
}

async fn images(target: &Target, context: &Context) -> Result<Line, Failure> {
    let device = locate(target, context).await?;
    let number = device.device_id;
    let record = paired_record(&device).await?;
    let provider = device.to_provider(UsbmuxdAddr::default(), LABEL);
    let connection = open_service(
        &provider,
        &record,
        <ImageMounter as IdeviceService>::service_name(),
    )
    .await?;
    let mut mounter = ImageMounter::new(connection);
    let mut queries = Vec::with_capacity(1 + IMAGE_KINDS.len());
    // The listing of what is mounted: of each entry only the names of its members are kept.
    queries.push(match mounter.copy_devices().await {
        Ok(entries) => ImageQuery::listing(
            entries
                .iter()
                .map(|entry| {
                    ListedImage::of(
                        entry
                            .as_dictionary()
                            .map(|dictionary| dictionary.keys().map(String::as_str)),
                    )
                })
                .collect(),
        ),
        Err(error) => ImageQuery::failed(ImageQueryName::CopyDevices, None, &error),
    });
    for kind in IMAGE_KINDS {
        // A lookup answers the image's signature, which is dropped unread.
        queries.push(match mounter.lookup_image(kind).await {
            Ok(_) => ImageQuery::lookup(kind, true),
            Err(IdeviceError::NotFound) => ImageQuery::lookup(kind, false),
            Err(error) => ImageQuery::failed(ImageQueryName::LookupImage, Some(kind), &error),
        });
    }
    Ok(Line::Images(Images {
        command: CommandName::Images,
        device: number,
        mounted: Mounted::judged(&queries),
        queries,
    }))
}

async fn apps(
    target: &Target,
    contains: &str,
    max_matching: usize,
    context: &Context,
) -> Result<Line, Failure> {
    let device = locate(target, context).await?;
    let number = device.device_id;
    let record = paired_record(&device).await?;
    let provider = device.to_provider(UsbmuxdAddr::default(), LABEL);
    let connection = open_service(
        &provider,
        &record,
        <InstallationProxyClient as IdeviceService>::service_name(),
    )
    .await?;
    let mut proxy = InstallationProxyClient::new(connection);
    // The records stay in this function: only the bundle identifiers that hold the text leave
    // it, with the counts, and none when they are more than the bound or more than half of them.
    let applications = proxy
        .get_apps(Some(USER_APPLICATIONS), None)
        .await
        .map_err(|error| Failure::library(Problem::AppsLookupFailed, &error))?;
    Ok(Line::Apps(Apps::of(
        number,
        contains,
        max_matching,
        applications.keys().map(String::as_str),
    )))
}

/// One picture of the screen, the way `way` takes it, as the service gave it: written to the
/// file `out` names, which must name nothing that exists, in a directory that exists (both are
/// checked before the device is asked), or given after the line on the standard output
/// (`--out -`, no file). A way the device refuses writes no file and gives no byte; a picture
/// that cannot be written whole into its file has the file removed again (`out_write_failed`).
/// The stage the command is in is kept in the context from the first step that waits on the
/// device.
async fn screenshot(
    target: &Target,
    out: &Out,
    way: Way,
    started: Instant,
    context: &Context,
) -> Result<Printed, Failure> {
    if let Out::File(path) = out {
        out_file::check(Path::new(path))?;
    }
    let stage = &context.stage;
    stage.set(Some(Stage::Session));
    let device = locate(target, context).await?;
    let number = device.device_id;
    let record = paired_record(&device).await?;
    let proxy = proxy_service(&device, &record).await?;
    stage.set(Some(Stage::Tunnel));
    let (mut tunnel, handshake) = open_tunnel(proxy, || {}).await?;
    let (picture, request) = match way {
        Way::ScreenCapture => screen_capture(&mut tunnel, &handshake, stage).await?,
        Way::Instruments => instruments(&mut tunnel, &handshake, stage).await?,
    };
    let path = match out {
        Out::File(path) => path,
        Out::Stdout => {
            let elapsed = Elapsed::of(request, started.elapsed());
            return Ok(Printed::picture_on_stdout(number, way, picture, elapsed));
        }
    };
    match out_file::write_new(Path::new(path), &picture) {
        Ok(()) => {}
        Err(NotWritten::NotMade(failure)) => return Err(failure),
        Err(NotWritten::Cut { error, left }) => {
            let line = Line::out_write_failed(number, way, &error, left);
            return Ok(Printed::line(line));
        }
    }
    let elapsed = Elapsed::of(request, started.elapsed());
    Ok(Printed::line(Line::Screenshot(Screenshot::of(
        number, way, &picture, elapsed, path,
    ))))
}

/// The interface orientation, as each reader asked reads it: the one `via` names, or each of
/// [`Reader::ALL`] in turn. The device and its pairing record are found once, before any reader
/// is asked (`not_paired` refuses the whole command). Each reader is asked within the command's
/// deadline: one that has not answered when it passes answers `deadline_passed`, and so does
/// each reader after it, which is not asked; the readers that answered before keep their
/// answers.
async fn orientation(
    target: &Target,
    via: Option<Reader>,
    context: &Context,
) -> Result<Line, Failure> {
    let device = locate(target, context).await?;
    let record = paired_record(&device).await?;
    let readers = match via {
        Some(reader) => vec![reader],
        None => Reader::ALL.to_vec(),
    };
    let mut candidates = Vec::with_capacity(readers.len());
    for reader in readers {
        let deadline_passed = || Err(Failure::own(Problem::DeadlinePassed));
        let read = match context.deadline {
            Some(deadline) if tokio::time::Instant::now() >= deadline => deadline_passed(),
            Some(deadline) => {
                tokio::time::timeout_at(deadline, read_orientation(&device, &record, reader))
                    .await
                    .unwrap_or_else(|_| deadline_passed())
            }
            None => read_orientation(&device, &record, reader).await,
        };
        candidates.push(match read {
            Ok(orientation) => Candidate::answered(reader, orientation),
            Err(failure) => Candidate::failed(reader, failure),
        });
    }
    Ok(Line::Orientations(Orientations {
        command: CommandName::Orientation,
        device: device.device_id,
        candidates,
    }))
}

/// The interface orientation the SpringBoard service reports, reached the way `reader` names:
/// its lockdown service in a session with this host's record, or its remote shim inside the
/// tunnel. Only the read is sent (`getInterfaceOrientation`); nothing sets, locks or turns an
/// orientation.
async fn read_orientation(
    device: &UsbmuxdDevice,
    record: &PairingFile,
    reader: Reader,
) -> Result<Orientation, Failure> {
    match reader {
        Reader::Springboard => {
            let provider = device.to_provider(UsbmuxdAddr::default(), LABEL);
            let connection = open_service(
                &provider,
                record,
                <SpringBoardServicesClient as IdeviceService>::service_name(),
            )
            .await?;
            interface_orientation(SpringBoardServicesClient::new(connection)).await
        }
        Reader::SpringboardShim => {
            let proxy = proxy_service(device, record).await?;
            // The tunnel stays open until the orientation is read through it.
            let (mut tunnel, handshake) = open_tunnel(proxy, || {}).await?;
            let stream = remote_service(
                &mut tunnel,
                &handshake,
                &<SpringBoardServicesClient as RsdService>::rsd_service_name(),
            )
            .await?;
            let client = <SpringBoardServicesClient as RsdService>::from_stream(stream)
                .await
                .map_err(|error| Failure::library(Problem::ServiceHandshakeFailed, &error))?;
            interface_orientation(client).await
        }
    }
}

/// The orientation `client` reads, by this program's word.
async fn interface_orientation(
    mut client: SpringBoardServicesClient,
) -> Result<Orientation, Failure> {
    client
        .get_interface_orientation()
        .await
        .map(reported)
        .map_err(|error| Failure::library(Problem::OrientationFailed, &error))
}

/// This program's word for an orientation the library reports.
fn reported(orientation: InterfaceOrientation) -> Orientation {
    match orientation {
        InterfaceOrientation::Unknown => Orientation::Unknown,
        InterfaceOrientation::Portrait => Orientation::Portrait,
        InterfaceOrientation::PortraitUpsideDown => Orientation::PortraitUpsideDown,
        InterfaceOrientation::LandscapeRight => Orientation::LandscapeRight,
        InterfaceOrientation::LandscapeLeft => Orientation::LandscapeLeft,
    }
}

/// `running`, read-only: whether `bundle` is installed, whether the application itself runs,
/// and the pids of its processes and, apart, of its extensions'. (ACIB-3c) The installation proxy
/// looks the one bundle up ([`looked_up`]); when it holds the bundle with a readable path, the
/// instruments server inside the tunnel is asked for its list of running processes
/// ([`instruments_server`], [`look`]), and its answer read ([`running_after_look`]). Nothing is
/// changed on the phone: the process control channel is not opened.
async fn running(target: &Target, bundle: &str, context: &Context) -> Result<Line, Failure> {
    let LookedUp {
        device,
        record,
        installation,
    } = looked_up(target, bundle, context).await?;
    let asked = Running::asked(device.device_id, bundle);
    let (asked, directories) = match running_after_lookup(asked, installation) {
        Then::Answer(line) => return Ok(*line),
        Then::GoOn(going_on) => going_on,
    };
    let (_tunnel, _, server) = instruments_server(&device, &record, context).await?;
    let mut client = match server {
        Ok(client) => client,
        Err(kind) => {
            let failed = asked.failed(Problem::ServiceHandshakeFailed, Some(kind));
            return Ok(Line::Running(failed));
        }
    };
    context.enter(Stage::ProcessList);
    let looked = look(&mut client, &directories).await;
    Ok(Line::Running(running_after_look(asked, looked)))
}

/// `launch`: (ACIB-3c) the installation proxy looks the one bundle up first, as for `running`
/// and `stop`; a bundle it does not hold answers `not_installed`, and no launch is sent and the
/// instruments server is not asked. Then the instruments server's process control launches
/// `bundle` and answers its pid ([`launched`]). Without `relaunch` an application that runs is
/// not ended first (`KillExisting` false; what the phone then does the library does not say);
/// with it, it is ended and started anew. Of the request's members `acib` chooses these: the
/// bundle, the application not started suspended (`StartSuspendedKey` false), `KillExisting`;
/// it passes no environment and no argument, which the library sends as an empty dictionary and
/// an empty list, and the library fixes the device path, an empty text (README).
async fn launch(
    target: &Target,
    bundle: &str,
    relaunch: bool,
    context: &Context,
) -> Result<Line, Failure> {
    let LookedUp {
        device,
        record,
        installation,
    } = looked_up(target, bundle, context).await?;
    let asked = Launch::asked(device.device_id, bundle, relaunch);
    let asked = match launch_after_lookup(asked, installation) {
        Then::Answer(line) => return Ok(*line),
        Then::GoOn(asked) => asked,
    };
    let (_tunnel, _, server) = instruments_server(&device, &record, context).await?;
    let mut client = match server {
        Ok(client) => client,
        Err(kind) => {
            let failed = asked.failed(Problem::ServiceHandshakeFailed, Some(kind));
            return Ok(Line::Launch(failed));
        }
    };
    context.enter(Stage::LaunchRequest);
    Ok(Line::Launch(
        match launched(&mut client, bundle, relaunch).await {
            Ok(pid) => asked.launched(pid),
            Err(kind) => asked.failed(Problem::LaunchFailed, Some(kind)),
        },
    ))
}

/// `stop`: (ACIB-3c) the installation proxy looks the one bundle up first; a bundle it does not
/// hold is answered as not running and nothing is sent. Then, in the tunnel, the instruments
/// server's first message is read, and [`stopped`] takes the steps from the first look on: the
/// application's own processes that look lists are each sent a kill request the way `way` names
/// (ACIB-3d), and the looks after the kills are made on another connection to the instruments
/// server ([`instruments_again`]), made only once the kills are sent (`wait` bounds the answers of
/// a way that asks for them).
async fn stop(
    target: &Target,
    bundle: &str,
    way: StopWay,
    wait: Duration,
    context: &Context,
) -> Result<Line, Failure> {
    let LookedUp {
        device,
        record,
        installation,
    } = looked_up(target, bundle, context).await?;
    let asked = Stop::asked(device.device_id, bundle);
    let (asked, directories) = match stop_after_lookup(asked, installation) {
        Then::Answer(line) => return Ok(*line),
        Then::GoOn(going_on) => going_on,
    };
    let (mut tunnel, port, server) = instruments_server(&device, &record, context).await?;
    let client = match server {
        Ok(client) => client,
        Err(kind) => {
            let failed = asked.failed(Problem::ServiceHandshakeFailed, Some(kind));
            return Ok(Line::Stop(failed));
        }
    };
    // A future, run only when `stopped` waits on it, after the kills.
    let looks = instruments_again(&mut tunnel, port);
    Ok(stopped(client, looks, asked, &directories, way, wait, context).await)
}

/// `stop` from its first look on, on the instruments server's connection `client`: the first
/// look (`matching.rs`); then the application's own processes it lists (its executable directly
/// in the bundle's directory) are each sent a kill request on the server's process control
/// channel, one after the other, the way `way` names (ACIB-3d; README): `primitive` and `archived`
/// open the channel ([`PROCESS_CONTROL_CHANNEL`]), send each `killPid:` with its argument in the
/// way's form ([`kill_argument`]) and an answer asked for ([`send_kills`]), and then read the
/// answers, all within `wait`, (fix round 1) each paired with its request by its number
/// ([`kill_answers`]); `library` sends the library's `kill_app` unchanged ([`kill_by_library`]).
/// Its extensions' processes are sent nothing. Then the
/// connection `looks` makes (another connection to the server, made only now) is looked on: the
/// list of processes is read at once, on a device information channel of its own, and again
/// every [`LOOK_INTERVAL`] while it lists a process the command signalled and the next look starts
/// before the command's deadline (`stopping.rs` decides; a look the deadline cuts leaves the
/// verdict of the one before it). `client`, which the kill requests went out on, is held, unused,
/// until the command ends. The verdict comes from the looks whatever became of the requests: a
/// request's error is carried beside it by its kind, an answer by its word. An application that
/// does not run is answered at the first look and sent nothing. (Fix round 1) A look that locates
/// no process (no entry of the list with a location read as a place) tells nothing of the bundle:
/// at the first look no kill request is sent ([`stop_after_first_look`]), and a look after the
/// kills ends the looks with no verdict (`Stop::looked`); both answer `process_paths_unreadable`.
/// (Fix round 2) A first look that locates the application's entries both at the bundle's
/// directory and directly in it does not tell which entry is the application itself: no kill
/// request is sent, and the line answers `application_entry_ambiguous`. A look after the kills
/// that does so sends nothing more, as no look after them does: the kills went to the pids a
/// first look told apart, and that look is read as any other, `application_located` `both`.
/// (ACIB-3d) The connection of the looks not made, or its first message not read, answers
/// `service_unreachable` or `service_handshake_failed`, a look failing `process_list_failed`,
/// each with its kind and no verdict; a line past the first look names what each kill request was
/// answered (`replies`) and which of the stages after the first look were reached (`reached`). A
/// deadline that passes before the first look after the kills has answered is the command's
/// `deadline_passed`, as for every command.
async fn stopped(
    mut client: Instruments,
    looks: impl Future<Output = Reconnected>,
    asked: Stop,
    directories: &[String],
    way: StopWay,
    wait: Duration,
    context: &Context,
) -> Line {
    context.enter(Stage::ProcessList);
    let first = match look(&mut client, directories).await {
        Ok(first) => first,
        Err(kind) => return Line::Stop(asked.failed(Problem::ProcessListFailed, Some(kind))),
    };
    let (asked, application) = match stop_after_first_look(asked, first) {
        Then::Answer(line) => return *line,
        Then::GoOn(going_on) => going_on,
    };
    context.enter(Stage::KillRequests);
    let (errors, replies) = if way.asks_answers() {
        match client.make_channel(PROCESS_CONTROL_CHANNEL).await {
            Ok(mut control) => {
                let (mut errors, sent) = send_kills(&mut control, &application, way).await;
                context.enter(Stage::KillReplies);
                let (replies, unread) = kill_answers(&mut control, &sent, wait).await;
                errors.extend(unread);
                (errors, replies)
            }
            Err(error) => {
                let kind = service_error_kind(&error);
                (every_pid_failed(&application, kind), Vec::new())
            }
        }
    } else {
        (kill_by_library(&mut client, &application).await, Vec::new())
    };
    let signals = Signals {
        signalled: application,
        errors,
    };
    // Each line from here on names the answers and the stages reached when it is made.
    let done = |stop: Stop| {
        let reached = reached_after(context, way);
        Line::Stop(stop.with_replies(replies.clone()).with_reached(reached))
    };
    context.enter(Stage::LookConnection);
    let mut again = match looks.await {
        Ok(again) => again,
        Err((problem, kind)) => return done(asked.failed_after_kills(signals, problem, kind)),
    };
    context.enter(Stage::LooksAfterKills);
    let mut channel = match device_info(&mut again).await {
        Ok(channel) => channel,
        Err(kind) => return done(asked.look_failed(signals, kind)),
    };
    let mut last = match processes_on(&mut channel, directories).await {
        Ok(look) => look,
        Err(kind) => return done(asked.look_failed(signals, kind)),
    };
    // A look that locates no process lists none of the application's, so the looks end at it;
    // `Stop::looked` then answers `process_paths_unreadable` for it, with no verdict.
    let verdict = loop {
        let time_left = context
            .deadline
            .map(|deadline| deadline.saturating_duration_since(tokio::time::Instant::now()));
        if let Next::Done(verdict) =
            stopping::after_look(&signals.signalled, &last.pids.application, time_left)
        {
            break verdict;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
        match look_within(&mut channel, directories, context.deadline).await {
            Some(Ok(look)) => last = look,
            Some(Err(kind)) => return done(asked.look_failed(signals, kind)),
            // The deadline passed during the look: the last look inside it is `last`.
            None => break stopping::verdict(&signals.signalled, &last.pids.application),
        }
    };
    done(asked.looked(signals, last, verdict))
}

/// (ACIB-3c) `running` after the installation proxy's lookup: the line answered at once (the
/// lookup failed: `application_lookup_failed` with its kind; the bundle is not installed:
/// `installed` false and `running` false, no process looked for; its path cannot be read:
/// `bundle_path_unreadable`), or the answer so far (`installed`) and the bundle's directories.
fn running_after_lookup(
    asked: Running,
    lookup: Result<Installation, ErrorKind>,
) -> Then<(Running, Vec<String>)> {
    match lookup {
        Err(kind) => Then::answer(Line::Running(
            asked.failed(Problem::ApplicationLookupFailed, Some(kind)),
        )),
        Ok(Installation::NotInstalled) => Then::answer(Line::Running(asked.not_installed())),
        Ok(Installation::PathUnreadable) => Then::answer(Line::Running(
            asked
                .installed()
                .failed(Problem::BundlePathUnreadable, None),
        )),
        Ok(Installation::Installed(directories)) => Then::GoOn((asked.installed(), directories)),
    }
}

/// (ACIB-3c) `launch` after the installation proxy's lookup: the line answered at once, no launch
/// sent and the instruments server not asked (the lookup failed: `application_lookup_failed`; the
/// bundle is not installed: `not_installed`, as `running` and `stop` answer it), or the answer so
/// far (`installed`) to send the launch with. Where the bundle lies is not needed to launch it:
/// a path read as no place is installed.
fn launch_after_lookup(asked: Launch, lookup: Result<Installation, ErrorKind>) -> Then<Launch> {
    match lookup {
        Err(kind) => Then::answer(Line::Launch(
            asked.failed(Problem::ApplicationLookupFailed, Some(kind)),
        )),
        Ok(Installation::NotInstalled) => Then::answer(Line::Launch(asked.not_installed())),
        Ok(Installation::PathUnreadable | Installation::Installed(_)) => {
            Then::GoOn(asked.installed())
        }
    }
}

/// (ACIB-3c) `stop` after the installation proxy's lookup: the line answered at once, nothing
/// sent (the lookup failed: `application_lookup_failed`; the bundle is not installed: as nothing
/// ran, `installed` false; its path cannot be read: `bundle_path_unreadable`), or the answer so
/// far (`installed`) and the bundle's directories.
fn stop_after_lookup(
    asked: Stop,
    lookup: Result<Installation, ErrorKind>,
) -> Then<(Stop, Vec<String>)> {
    match lookup {
        Err(kind) => Then::answer(Line::Stop(
            asked.failed(Problem::ApplicationLookupFailed, Some(kind)),
        )),
        Ok(Installation::NotInstalled) => Then::answer(Line::Stop(asked.not_installed())),
        Ok(Installation::PathUnreadable) => Then::answer(Line::Stop(
            asked
                .installed()
                .failed(Problem::BundlePathUnreadable, None),
        )),
        Ok(Installation::Installed(directories)) => Then::GoOn((asked.installed(), directories)),
    }
}

/// (ACIB-3c) `stop` after its first look `first`: (fix round 1) when it locates no process (no
/// entry of the list has a location read as a place), the line answered at once,
/// `process_paths_unreadable`, no kill sent: what cannot be told is not "does not run"; when the
/// application itself does not run, the line answered at once, no kill sent (its extensions'
/// pids and `located` named); (fix round 2) when it locates the application's entries both at
/// the bundle's directory and directly in it (`both`), which no way of naming explains, the line
/// answered at once, `application_entry_ambiguous`, no kill sent: the list does not tell which
/// entry is the application itself, and `stop` ends only that; else the answer so far and the
/// application's pids to send a kill request for. Only these pids are ever sent one
/// ([`stopped`]).
fn stop_after_first_look(asked: Stop, first: Look) -> Then<(Stop, Vec<u32>)> {
    if first.located == 0 {
        Then::answer(Line::Stop(
            asked.failed(Problem::ProcessPathsUnreadable, None),
        ))
    } else if first.pids.application.is_empty() {
        Then::answer(Line::Stop(asked.not_running(first)))
    } else if first.application_located == Some(ApplicationLocated::Both) {
        Then::answer(Line::Stop(
            asked.failed(Problem::ApplicationEntryAmbiguous, None),
        ))
    } else {
        Then::GoOn((asked, first.pids.application))
    }
}

/// (ACIB-3c, fix round 1) `running` after its look: the line it answers. The look failed:
/// `process_list_failed` with its kind. It locates no process: `process_paths_unreadable`, never
/// `running` false ([`Running::found`]). Else whether the application itself runs, the pids by
/// kind, `located` and how the application was located.
fn running_after_look(asked: Running, look: Result<Look, ErrorKind>) -> Running {
    match look {
        Ok(look) => asked.found(look),
        Err(kind) => asked.failed(Problem::ProcessListFailed, Some(kind)),
    }
}

/// (ACIB-3c) The device `target` names, this host's pairing record for it, and what the
/// installation proxy's lookup of the one bundle `bundle` answers: a lockdown session with the
/// record starts the installation proxy (as `apps` starts it), which is asked for that bundle
/// alone, of every application type (`Lookup` with `ApplicationType` [`ANY_APPLICATION`] and
/// `BundleIDs` the one bundle: idevice 0.1.68, `services/installation_proxy.rs` 87-112). Of its
/// answer each entry's identifier and its record's [`APPLICATION_PATH`] are read, and nothing
/// else; an entry without a path is one whose path is read as no place. A step before the lookup
/// fails as every command's does (its problem word, the library's text cut as `redact.rs` cuts
/// it); the lookup failing is its [`ErrorKind`] alone. Each step enters its stage first:
/// `usbmuxd`, `application_lookup`.
async fn looked_up(target: &Target, bundle: &str, context: &Context) -> Result<LookedUp, Failure> {
    context.enter(Stage::Usbmuxd);
    let device = locate(target, context).await?;
    let record = paired_record(&device).await?;
    context.enter(Stage::ApplicationLookup);
    let provider = device.to_provider(UsbmuxdAddr::default(), LABEL);
    let connection = open_service(
        &provider,
        &record,
        <InstallationProxyClient as IdeviceService>::service_name(),
    )
    .await?;
    let mut proxy = InstallationProxyClient::new(connection);
    let installation = proxy
        .get_apps(Some(ANY_APPLICATION), Some(vec![bundle.to_owned()]))
        .await
        .map(|records| {
            matching::installation(
                bundle,
                records.iter().map(|(identifier, record)| Application {
                    bundle_identifier: identifier,
                    path: record
                        .as_dictionary()
                        .and_then(|members| members.get(APPLICATION_PATH))
                        .and_then(|path| path.as_string())
                        .unwrap_or_default(),
                }),
            )
        })
        .map_err(|error| service_error_kind(&error));
    Ok(LookedUp {
        device,
        record,
        installation,
    })
}

/// (ACIB-3c) The tunnel of iOS 17.4 and later to `device` and, inside it, the instruments server
/// (`com.apple.instruments.dtservicehub`, as the remote service discovery handshake lists it)
/// with its first message read (the capabilities it publishes, read before anything is asked of
/// it, as the screenshot's `instruments` way and the library's own tools read it), or the kind of
/// the error that first message failed with; (ACIB-3d) beside them the server's port, which
/// `stop` connects to again for its looks after the kills. A step before it fails as every
/// command's does. Each step enters its stage first: `proxy_tunnel`, `discovery_handshake`,
/// `service_connection`, `server_first_message`.
async fn instruments_server(
    device: &UsbmuxdDevice,
    record: &PairingFile,
    context: &Context,
) -> Result<ServerConnection, Failure> {
    context.enter(Stage::ProxyTunnel);
    let proxy = proxy_service(device, record).await?;
    let (mut tunnel, handshake) =
        open_tunnel(proxy, || context.enter(Stage::DiscoveryHandshake)).await?;
    context.enter(Stage::ServiceConnection);
    // As `remote_service` connects, with the port kept.
    let port = port_of(&handshake, &<Instruments as RsdService>::rsd_service_name())?;
    let stream = tunnel
        .connect_to_service_port(port)
        .await
        .map_err(|error| Failure::library(Problem::ServiceUnreachable, &error))?;
    let client = <Instruments as RsdService>::from_stream(stream).await;
    context.enter(Stage::ServerFirstMessage);
    Ok((tunnel, port, first_message(client).await))
}

/// (ACIB-3c) The instruments server's client `client` once its first message is read on its root
/// channel (and dropped unread), or the kind of the error.
async fn first_message(
    client: Result<Instruments, IdeviceError>,
) -> Result<Instruments, ErrorKind> {
    let mut client = client.map_err(|error| service_error_kind(&error))?;
    client
        .read_message(ROOT_CHANNEL)
        .await
        .map_err(|error| service_error_kind(&error))?;
    Ok(client)
}

/// (ACIB-3c) A device information channel of the instruments server `client`, opened by its own
/// request (`_requestChannelWithCode:identifier:` with [`DEVICE_INFO_CHANNEL`]: idevice 0.1.68,
/// `services/dvt/remote_server.rs` 425-458).
async fn device_info(client: &mut Instruments) -> Result<ServerChannel<'_>, ErrorKind> {
    client
        .make_channel(DEVICE_INFO_CHANNEL)
        .await
        .map_err(|error| service_error_kind(&error))
}

/// (ACIB-3c) One look, on a device information channel of its own: the pids of the processes
/// whose executable lies inside one of `directories`, the application's and its extensions'
/// apart, and (fix round 1) `located` and how the application was located ([`Look`]).
async fn look(client: &mut Instruments, directories: &[String]) -> Result<Look, ErrorKind> {
    let mut channel = device_info(client).await?;
    processes_on(&mut channel, directories).await
}

/// (ACIB-3c) The instruments server's list of running processes, asked on the device information
/// channel `channel` (`runningProcesses`, no argument, an answer asked for: the library's
/// `Channel::call_method`, then its `read_message` on the channel, as its `DeviceInfoClient`
/// does: `services/dvt/device_info.rs` 34-46), and of it the pids of the processes whose
/// executable lies inside one of `directories`, the application's and its extensions' apart
/// (`matching.rs`), with (fix round 1) the count of the list's entries whose location is read
/// as a place and how the application was located. The list stays in this function; an answer
/// that is no list of entries with a pid is `answer_unreadable`.
async fn processes_on(
    channel: &mut ServerChannel<'_>,
    directories: &[String],
) -> Result<Look, ErrorKind> {
    channel
        .call_method(Some(RUNNING_PROCESSES), None, true)
        .await
        .map_err(|error| service_error_kind(&error))?;
    let reply = channel
        .read_message()
        .await
        .map_err(|error| service_error_kind(&error))?;
    let processes = matching::listed_processes(&reply).ok_or(ErrorKind::AnswerUnreadable)?;
    Ok(matching::pids_inside(directories, processes))
}

/// A look at the processes of `directories` on `channel` that ends by `deadline`: `None` when
/// the deadline passes before the server answers it.
async fn look_within(
    channel: &mut ServerChannel<'_>,
    directories: &[String],
    deadline: Option<tokio::time::Instant>,
) -> Option<Result<Look, ErrorKind>> {
    match deadline {
        Some(deadline) => tokio::time::timeout_at(deadline, processes_on(channel, directories))
            .await
            .ok(),
        None => Some(processes_on(channel, directories).await),
    }
}

/// (ACIB-3c) `launch`'s request: the instruments server's process control channel (opened by its
/// own request, `com.apple.instruments.server.services.processcontrol`) and the library's
/// `launch_app` on it (idevice 0.1.68, `services/dvt/process_control.rs` 132-218), which sends
/// `launchSuspendedProcessWithDevicePath:bundleIdentifier:environment:arguments:options:` with
/// the device path an empty text, `bundle`, an empty environment, an empty list of arguments and
/// the options `StartSuspendedKey` false and `KillExisting` `relaunch`, and answers the pid the
/// server answered. A pid that does not fit in 32 bits is `answer_unreadable`.
async fn launched(
    client: &mut Instruments,
    bundle: &str,
    relaunch: bool,
) -> Result<u32, ErrorKind> {
    let mut control = ProcessControlClient::new(client)
        .await
        .map_err(|error| service_error_kind(&error))?;
    // No environment, no argument; not started suspended; ended first only with `--relaunch`.
    let pid = control
        .launch_app(bundle, None, None, false, relaunch)
        .await
        .map_err(|error| service_error_kind(&error))?;
    u32::try_from(pid).map_err(|_| ErrorKind::AnswerUnreadable)
}

/// (ACIB-3c's kill requests; ACIB-3d: the way `library`) The instruments server's process control
/// channel, as the library's `ProcessControlClient::new` opens it (its request's answer waited
/// for: idevice 0.1.68, `services/dvt/process_control.rs` 132-138), and, for each of `pids` in
/// turn, the library's `kill_app` (`killPid:` with the pid as a 32-bit number, no answer asked
/// for: 275-296, "doesn't wait for confirmation": it waits for the request to be written and
/// flushed alone; the library does not name the signal). A request that fails is carried by its
/// pid and kind; the channel not opened fails the request of every pid with its kind.
async fn kill_by_library(client: &mut Instruments, pids: &[u32]) -> Vec<SignalError> {
    match ProcessControlClient::new(client).await {
        Ok(mut control) => {
            let mut errors = Vec::new();
            for &pid in pids {
                if let Err(error) = control.kill_app(u64::from(pid)).await {
                    errors.push(SignalError::request(pid, service_error_kind(&error)));
                }
            }
            errors
        }
        Err(error) => every_pid_failed(pids, service_error_kind(&error)),
    }
}

/// The failed request of each of `pids`, all of `error_kind`.
fn every_pid_failed(pids: &[u32], error_kind: ErrorKind) -> Vec<SignalError> {
    pids.iter()
        .map(|&pid| SignalError::request(pid, error_kind))
        .collect()
}

/// (ACIB-3d) The argument of `killPid:` for `pid` in the form of the way `way`: of `archived`, the
/// pid archived as a signed number, in the form the library's own device information client
/// sends a pid in (`AuxValue::archived_value` of an integer: idevice 0.1.68,
/// `services/dvt/device_info.rs` 95-122, `services/dvt/message.rs` 447-449); of `primitive` and
/// `library`, the 32-bit number the library's `kill_app` sends (`AuxValue::U32`:
/// `services/dvt/process_control.rs` 286-296).
fn kill_argument(way: StopWay, pid: u32) -> AuxValue {
    match way {
        StopWay::Archived => AuxValue::archived_value(XPCObject::Int64(i64::from(pid)).to_plist()),
        StopWay::Primitive | StopWay::Library => AuxValue::U32(pid),
    }
}

/// (ACIB-3d) The kill requests of a way that asks for answers, on the process control channel
/// `control`: for each of `pids` in turn, `killPid:` with its argument in the way's form
/// ([`kill_argument`]) and an answer asked for, through the library's public channel
/// (`Channel::call_method`, which writes and flushes the request and waits for nothing:
/// `services/dvt/remote_server.rs` 1334-1343, 784-795, 712-748, 1273-1279). (Fix round 1) Each
/// request takes the library's next number on the connection, the first [`FIRST_KILL_NUMBER`], one
/// that could not be written too (the library counts a request before it writes it: 791). The
/// requests that could not be written, each by its pid and kind, and those written, each with its
/// number, in their order.
async fn send_kills(
    control: &mut ServerChannel<'_>,
    pids: &[u32],
    way: StopWay,
) -> (Vec<SignalError>, Vec<KillSent>) {
    let (mut errors, mut sent) = (Vec::new(), Vec::with_capacity(pids.len()));
    let mut number = FIRST_KILL_NUMBER;
    for &pid in pids {
        let argument = kill_argument(way, pid);
        match control
            .call_method(Some(KILL_PID), Some(vec![argument]), true)
            .await
        {
            Ok(()) => sent.push(KillSent { pid, number }),
            Err(error) => errors.push(SignalError::request(pid, service_error_kind(&error))),
        }
        number = number.wrapping_add(1);
    }
    (errors, sent)
}

/// (ACIB-3d) The answers to the kill requests `sent`, all of them within `wait`, read on the
/// process control channel `control` ([`next_answer`]); (fix round 1) each answer is paired with
/// its request by the number it carries ([`message_number`]; the server answers a request with the
/// request's number: [`FIRST_KILL_NUMBER`]), never by its place among the answers, so an answer
/// that is missing, late or out of order is given to no other pid, and an answer whose number is
/// of no request still waited for is given to none. In the order of `sent`, each request's: its
/// answer, when one came; else, when a read failed (no answer can come after: the library's reader
/// of the connection stops for good, `services/dvt/remote_server.rs` 853-896), a failed request of
/// the error's kind and its [`reply_failure`]; else, the wait having passed, `not_in_time`.
async fn kill_answers(
    control: &mut ServerChannel<'_>,
    sent: &[KillSent],
    wait: Duration,
) -> (Vec<KillReply>, Vec<SignalError>) {
    let until = tokio::time::Instant::now().checked_add(wait);
    let mut answers: Vec<Option<KillReply>> = vec![None; sent.len()];
    let mut failed = None;
    while answers.iter().any(Option::is_none) {
        let next = match until {
            Some(until) => tokio::time::timeout_at(until, next_answer(control))
                .await
                .ok(),
            None => Some(next_answer(control).await),
        };
        match next {
            Some(Ok(answer)) => {
                let number = message_number(&answer);
                let waiting = sent
                    .iter()
                    .zip(answers.iter_mut())
                    .find(|(request, answered)| request.number == number && answered.is_none());
                if let Some((request, answered)) = waiting {
                    *answered = Some(kill_reply(request.pid, &answer));
                }
            }
            Some(Err(error)) => {
                failed = Some(error);
                break;
            }
            None => break,
        }
    }
    let (mut replies, mut errors) = (Vec::with_capacity(sent.len()), Vec::new());
    for (request, answer) in sent.iter().zip(answers) {
        match (answer, &failed) {
            (Some(reply), _) => replies.push(reply),
            (None, Some(error)) => errors.push(SignalError::reply(
                request.pid,
                service_error_kind(error),
                reply_failure(error),
            )),
            (None, None) => replies.push(KillReply {
                pid: request.pid,
                reply: KillAnswer::NotInTime,
                message_type: None,
            }),
        }
    }
    (replies, errors)
}

/// (ACIB-3d, fix round 1) Why the wait for a kill request's answer failed with the library's error
/// `error`, from that error alone (its variant and, of a socket error, its kind; never its text,
/// never a byte of a message, nothing of the device or of another application). Waiting on a
/// channel (`Channel::read_message`: idevice 0.1.68, `services/dvt/remote_server.rs` 1317-1319,
/// 828-851) the library answers two errors of its own: `UnknownChannel` (833) when the channel is
/// no longer its own, which it makes so only when the phone cancels the channel (938-960,
/// 1234-1239); and, once its reader of the connection has stopped, one socket error of the kind
/// `BrokenPipe`, "remote server connection closed" (842-849, 1245-1250), whatever stopped it. The
/// reader stops at the first error of a message it reads (853-896: the stream's end, a reset or
/// another failed read, a message it cannot read, `services/dvt/message.rs` 585-665), traces that
/// error at warning level (878) and keeps nothing of it: the one error stands for all of these, and
/// `reader_stopped` cannot be divided further from what the library gives a caller (README).
fn reply_failure(error: &IdeviceError) -> ReplyFailure {
    match error {
        IdeviceError::Socket(socket) if socket.kind() == std::io::ErrorKind::BrokenPipe => {
            ReplyFailure::ReaderStopped
        }
        IdeviceError::Dvt(DvtError::UnknownChannel(_)) => ReplyFailure::ChannelCancelled,
        _ => ReplyFailure::Other,
    }
}

/// (ACIB-3d) The next answer on `control`: the next message of an odd conversation index (the
/// library numbers an answer one above the message it answers, and reads such a message on the
/// channel's own number: idevice 0.1.68, `services/dvt/message.rs` 599-608, 750-756); a message
/// of an even one, which the server would have sent of its own, is passed over.
async fn next_answer(control: &mut ServerChannel<'_>) -> Result<Message, IdeviceError> {
    loop {
        let message = control.read_message().await?;
        if !conversation_index(&message).is_multiple_of(2) {
            return Ok(message);
        }
    }
}

/// The conversation index of `message`, as its header holds it (the library's
/// `MessageHeader::serialize`, bytes 20 to 23: `services/dvt/message.rs` 531-545; its reader of
/// it is the crate's own); 0 if the header were shorter.
fn conversation_index(message: &Message) -> u32 {
    header_word(message, 20)
}

/// (ACIB-3d, fix round 1) The number of `message` (its identifier: of an answer, the number of the
/// request it answers), as its header holds it (bytes 16 to 19 of `MessageHeader::serialize`:
/// `services/dvt/message.rs` 531-545, and as the library reads it, 597; its reader of it is the
/// crate's own); 0 if the header were shorter.
fn message_number(message: &Message) -> u32 {
    header_word(message, 16)
}

/// The little-endian 32-bit word at byte `at` of `message`'s header as the library writes it; 0 if
/// the header were shorter.
fn header_word(message: &Message, at: usize) -> u32 {
    let header = message.message_header.serialize();
    header
        .get(at..at + 4)
        .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
        .map_or(0, u32::from_le_bytes)
}

/// (ACIB-3d) What the answer `answer` to the kill request of `pid` says: whether it holds a
/// payload (never read) and its message type, the first byte of its payload header (the library's
/// `PayloadHeader::serialize`: `services/dvt/message.rs` 555-562).
fn kill_reply(pid: u32, answer: &Message) -> KillReply {
    KillReply {
        pid,
        reply: if answer.data.is_some() {
            KillAnswer::WithPayload
        } else {
            KillAnswer::WithoutPayload
        },
        message_type: answer.payload_header.serialize().first().copied(),
    }
}

/// (ACIB-3d) Of each stage of `stop` after its first look, whether the command has entered it so
/// far (`kill_replies` only of a way that asks for answers).
fn reached_after(context: &Context, way: StopWay) -> Reached {
    let steps = context.steps.borrow();
    Reached {
        kill_requests: steps.reached(Stage::KillRequests),
        kill_replies: way
            .asks_answers()
            .then(|| steps.reached(Stage::KillReplies)),
        look_connection: steps.reached(Stage::LookConnection),
        looks_after_kills: steps.reached(Stage::LooksAfterKills),
    }
}

/// (ACIB-3d) Another connection, inside the tunnel `tunnel`, to the instruments server's port
/// `port` (the one the discovery handshake named at `service_connection`: a new connection of the
/// library's TCP stack, its `connect_to_service_port`, `tcp/mod.rs` 10-17), and its first message
/// read, as at `server_first_message`: the connection `stop` looks on after its kills, whatever
/// the kills did to the first one.
async fn instruments_again(tunnel: &mut AdapterHandle, port: u16) -> Reconnected {
    let stream = tunnel
        .connect_to_service_port(port)
        .await
        .map_err(|error| (Problem::ServiceUnreachable, service_error_kind(&error)))?;
    let client = <Instruments as RsdService>::from_stream(stream).await;
    first_message(client)
        .await
        .map_err(|kind| (Problem::ServiceHandshakeFailed, kind))
}

/// The streamed list of applications, asked on `client` as the library's `stream_apps` asks it
/// (idevice 0.1.68, `invoke_streaming_with_plist` in `services/core_device/mod.rs` 109-159, with
/// the flags of `list_apps`: [`streamed_list_request`]), its answer read on the root channel as
/// the library reads it (`parse_stream_response`, 248-282): a message whose stream status names
/// `pushing` holds elements, one that names `finishStreaming` ends the list, one that names
/// `receivedError`, or that holds `CoreDevice.error` and no status, is the phone's refusal
/// (`device_refused`), and any other is `answer_unreadable`. Each element is counted when it
/// holds a bundle identifier and a path, as texts, and nothing of it is kept (ACIB-3c: only
/// `probe` asks this list, and answers its count); one without either is `answer_unreadable`.
/// The library's own `stream_apps` gives a `futures` stream, which this program cannot read
/// without a crate of its own: the request is made here and sent through the library's
/// RemoteXPC client.
async fn streamed_applications(client: &mut Xpc) -> Result<usize, ErrorKind> {
    let request = streamed_list_request().ok_or(ErrorKind::Other)?;
    client
        .send_object(request, true)
        .await
        .map_err(|error| service_error_kind(&error))?;
    let mut listed = 0;
    loop {
        let response = client
            .recv_root()
            .await
            .map_err(|error| service_error_kind(&error))?;
        let message = response
            .as_dictionary()
            .ok_or(ErrorKind::AnswerUnreadable)?;
        let Some(status) = message.get(SIDE_CHANNEL_STATUS) else {
            return Err(if message.contains_key("CoreDevice.error") {
                ErrorKind::DeviceRefused
            } else {
                ErrorKind::AnswerUnreadable
            });
        };
        let status = status.as_dictionary().ok_or(ErrorKind::AnswerUnreadable)?;
        if status.contains_key("receivedError") {
            return Err(ErrorKind::DeviceRefused);
        }
        if status.contains_key("finishStreaming") {
            return Ok(listed);
        }
        let elements = status
            .get("pushing")
            .and_then(|pushing| pushing.as_dictionary())
            .and_then(|pushing| pushing.get("elements"))
            .and_then(|elements| elements.as_array())
            .ok_or(ErrorKind::AnswerUnreadable)?;
        for element in elements {
            let entry = element.as_dictionary();
            let holds_text = |key: &str| {
                entry
                    .and_then(|entry| entry.get(key))
                    .and_then(|value| value.as_string())
                    .is_some()
            };
            if !(holds_text("bundleIdentifier") && holds_text("path")) {
                return Err(ErrorKind::AnswerUnreadable);
            }
            listed += 1;
        }
    }
}

/// The request of the streamed list, member for member the one the library's `stream_apps` sends
/// (`services/core_device/mod.rs` 117-144): protocol version 2, an empty action, the CoreDevice
/// version, a device identifier and an invocation identifier (each a random UUID made for this
/// request, as the library makes them: neither is the phone's identifier), the feature, and as
/// input the flags of the list ([`LIST_OPTIONS`]) beside a stream proxy naming a side channel (a
/// random UUID too). `None` only if the library's decoder did not read the side channel's UUID
/// (the library offers no other way to make an XPC UUID without a crate of its own).
fn streamed_list_request() -> Option<XPCObject> {
    let options = list_options();
    let mut proxy = Dictionary::new();
    proxy.insert("sideChannel".to_owned(), uuid_object(random_uuid())?);
    let mut input = Dictionary::new();
    input.insert("actualInput".to_owned(), XPCObject::Dictionary(options));
    input.insert("streamProxy".to_owned(), XPCObject::Dictionary(proxy));
    let mut version = Dictionary::new();
    version.insert(
        "originalComponentsCount".to_owned(),
        XPCObject::Int64(CORE_DEVICE_VERSION_COMPONENTS.len() as i64),
    );
    version.insert(
        "components".to_owned(),
        XPCObject::Array(
            CORE_DEVICE_VERSION_COMPONENTS
                .iter()
                .map(|component| XPCObject::UInt64(*component))
                .collect(),
        ),
    );
    version.insert(
        "stringValue".to_owned(),
        XPCObject::String(CORE_DEVICE_VERSION.to_owned()),
    );
    let mut request = Dictionary::new();
    request.insert(
        "CoreDevice.CoreDeviceDDIProtocolVersion".to_owned(),
        XPCObject::Int64(2),
    );
    request.insert(
        "CoreDevice.action".to_owned(),
        XPCObject::Dictionary(Dictionary::new()),
    );
    request.insert(
        "CoreDevice.coreDeviceVersion".to_owned(),
        XPCObject::Dictionary(version),
    );
    request.insert(
        "CoreDevice.deviceIdentifier".to_owned(),
        XPCObject::String(uuid_text(random_uuid())),
    );
    request.insert(
        "CoreDevice.featureIdentifier".to_owned(),
        XPCObject::String(STREAMED_LIST_FEATURE.to_owned()),
    );
    request.insert("CoreDevice.input".to_owned(), XPCObject::Dictionary(input));
    request.insert(
        "CoreDevice.invocationIdentifier".to_owned(),
        XPCObject::String(uuid_text(random_uuid())),
    );
    Some(XPCObject::Dictionary(request))
}

/// The flags of the list of applications ([`LIST_OPTIONS`]), each a boolean, in their order: the
/// input of the streamed list and (ACIB-3b, third round) of the list asked in the screenshot's
/// form, as the library's `app_list_options` makes the input of its two lists.
fn list_options() -> Dictionary {
    let mut options = Dictionary::new();
    for (key, value) in LIST_OPTIONS {
        options.insert(key.to_owned(), XPCObject::Bool(value));
    }
    options
}

/// Sixteen bytes of a random UUID (version 4, its variant 10): two hashes of a hasher the
/// standard library seeds at random for each new state (not for cryptography; an identifier of
/// one request, as the library's own `Uuid::new_v4`).
fn random_uuid() -> [u8; 16] {
    let state = RandomState::new();
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&state.hash_one(0u8).to_le_bytes());
    bytes[8..].copy_from_slice(&state.hash_one(1u8).to_le_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    bytes
}

/// A UUID written as its text: 32 lowercase hexadecimal digits in groups of 8, 4, 4, 4 and 12.
fn uuid_text(bytes: [u8; 16]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(36);
    for (index, &byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            text.push('-');
        }
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    text
}

/// The XPC object of the UUID `bytes`, as the library's decoder reads one from the wire.
fn uuid_object(bytes: [u8; 16]) -> Option<XPCObject> {
    let mut encoded = Vec::with_capacity(28);
    encoded.extend_from_slice(&XPC_OBJECT_MAGIC.to_le_bytes());
    encoded.extend_from_slice(&XPC_OBJECT_VERSION.to_le_bytes());
    encoded.extend_from_slice(&XPC_UUID_TYPE.to_le_bytes());
    encoded.extend_from_slice(&bytes);
    XPCObject::decode(&encoded).ok()
}

/// The kind of an error of the library: what it tells apart, and [`ErrorKind::Other`] for the
/// rest; of `probe`, an error of the application service, and (ACIB-3c) of `running`, `launch`
/// and `stop`, one of the installation proxy's lookup or of the instruments server. Its text is
/// never read: the application service's own refusal (`CoreDevice(DeviceError)`) carries the
/// debug text of the phone's error dictionary, which can name other applications. The library
/// maps no refusal to a locked phone, so there is no kind for one.
fn service_error_kind(error: &IdeviceError) -> ErrorKind {
    match error {
        IdeviceError::CoreDevice(CoreDeviceError::DeviceError(_)) => ErrorKind::DeviceRefused,
        IdeviceError::CoreDevice(
            CoreDeviceError::MissingField(_) | CoreDeviceError::MalformedField(_),
        )
        | IdeviceError::UnexpectedResponse(_)
        | IdeviceError::Plist(_) => ErrorKind::AnswerUnreadable,
        IdeviceError::Socket(_) => ErrorKind::ConnectionFailed,
        IdeviceError::Xpc(_) => ErrorKind::XpcFailed,
        _ => ErrorKind::Other,
    }
}

/// The picture of CoreDevice's screen capture service, asked for as PNG, of the main display;
/// and how long the request alone took.
async fn screen_capture(
    tunnel: &mut AdapterHandle,
    handshake: &RsdHandshake,
    stage: &Cell<Option<Stage>>,
) -> Result<(Vec<u8>, Duration), Failure> {
    type Client = ScreenCaptureServiceClient<Box<dyn ReadWrite>>;
    stage.set(Some(Stage::ServiceHandshake));
    let stream = remote_service(
        tunnel,
        handshake,
        &<Client as RsdService>::rsd_service_name(),
    )
    .await?;
    let mut client = <Client as RsdService>::from_stream(stream)
        .await
        .map_err(|error| Failure::library(Problem::ServiceHandshakeFailed, &error))?;
    stage.set(Some(Stage::PictureRequest));
    let asked = Instant::now();
    let picture = client
        .take_screenshot(None, ImageFormat::Png)
        .await
        .map_err(|error| Failure::library(Problem::ScreenshotFailed, &error))?;
    Ok((picture, asked.elapsed()))
}

/// The picture of the instruments server's screenshot channel, in the format the device gives;
/// and how long the request alone took. The server's first message (the capabilities it
/// publishes) is read before the channel is asked for, as the library's own screenshot tool
/// does, and dropped unread.
async fn instruments(
    tunnel: &mut AdapterHandle,
    handshake: &RsdHandshake,
    stage: &Cell<Option<Stage>>,
) -> Result<(Vec<u8>, Duration), Failure> {
    type Client = RemoteServerClient<Box<dyn ReadWrite>>;
    stage.set(Some(Stage::ServiceHandshake));
    let stream = remote_service(
        tunnel,
        handshake,
        &<Client as RsdService>::rsd_service_name(),
    )
    .await?;
    let mut client = <Client as RsdService>::from_stream(stream)
        .await
        .map_err(|error| Failure::library(Problem::ServiceHandshakeFailed, &error))?;
    stage.set(Some(Stage::ServerFirstMessage));
    client
        .read_message(0)
        .await
        .map_err(|error| Failure::library(Problem::ServiceHandshakeFailed, &error))?;
    stage.set(Some(Stage::PictureRequest));
    let mut channel = ScreenshotClient::new(&mut client)
        .await
        .map_err(|error| Failure::library(Problem::ChannelRefused, &error))?;
    let asked = Instant::now();
    let picture = channel
        .take_screenshot()
        .await
        .map_err(|error| Failure::library(Problem::ScreenshotFailed, &error))?;
    Ok((picture, asked.elapsed()))
}

/// The connection to the tunnel's lockdown service, CoreDeviceProxy, started in a session with
/// this host's record.
async fn proxy_service(device: &UsbmuxdDevice, record: &PairingFile) -> Result<Idevice, Failure> {
    let provider = device.to_provider(UsbmuxdAddr::default(), LABEL);
    open_service(
        &provider,
        record,
        <CoreDeviceProxy as IdeviceService>::service_name(),
    )
    .await
}

/// The tunnel of iOS 17.4 and later through the CoreDeviceProxy `connection` (with the
/// library's own TCP stack inside this process) and the remote service discovery handshake
/// through it; `discovery` is called once the connection to the discovery port is made, as the
/// handshake starts (a command of the application service enters its stage there).
async fn open_tunnel(
    connection: Idevice,
    discovery: impl FnOnce(),
) -> Result<(AdapterHandle, RsdHandshake), Failure> {
    let proxy = CoreDeviceProxy::new(connection)
        .await
        .map_err(|error| Failure::library(Problem::TunnelFailed, &error))?;
    let rsd_port = proxy.tunnel_info().server_rsd_port;
    let adapter = proxy
        .create_software_tunnel()
        .map_err(|error| Failure::library(Problem::TunnelFailed, &error))?;
    let mut tunnel = adapter.to_async_handle();
    let stream = tunnel
        .connect(rsd_port)
        .await
        .map_err(|error| Failure::library(Problem::RsdUnreachable, &error))?;
    discovery();
    let handshake = RsdHandshake::new(stream)
        .await
        .map_err(|error| Failure::library(Problem::RsdHandshakeFailed, &error))?;
    Ok((tunnel, handshake))
}

/// The port inside the tunnel the handshake names for the remote service `name`, or
/// `service_not_listed`.
fn port_of(handshake: &RsdHandshake, name: &str) -> Result<u16, Failure> {
    handshake
        .services
        .get(name)
        .map(|service| service.port)
        .ok_or(Failure::own(Problem::ServiceNotListed))
}

/// A connection, inside the tunnel, to the remote service the handshake lists as `name`.
async fn remote_service(
    tunnel: &mut AdapterHandle,
    handshake: &RsdHandshake,
    name: &str,
) -> Result<Box<dyn ReadWrite>, Failure> {
    let port = port_of(handshake, name)?;
    tunnel
        .connect_to_service_port(port)
        .await
        .map_err(|error| Failure::library(Problem::ServiceUnreachable, &error))
}

/// The devices usbmuxd lists now.
async fn list() -> Result<Vec<UsbmuxdDevice>, Failure> {
    let mut usbmuxd = UsbmuxdConnection::default()
        .await
        .map_err(|error| Failure::library(Problem::UsbmuxdUnreachable, &error))?;
    usbmuxd
        .get_devices()
        .await
        .map_err(|error| Failure::library(Problem::UsbmuxdListFailed, &error))
}

/// The listed device with this session number.
async fn find(number: u32) -> Result<UsbmuxdDevice, Failure> {
    list()
        .await?
        .into_iter()
        .find(|device| device.device_id == number)
        .ok_or(Failure::own(Problem::DeviceNotListed))
}

/// The device `target` names: the listed device with its number (`device_not_listed`), or the
/// row [`keyed`] takes of the listed devices whose key under the salt of its variable is the key
/// named (`key_salt_unusable` before usbmuxd is asked, `device_key_unknown`). The device's
/// number is kept in the context once it is found.
async fn locate(target: &Target, context: &Context) -> Result<UsbmuxdDevice, Failure> {
    let device = match target {
        Target::Number(number) => find(*number).await?,
        Target::Key { key, salt_variable } => {
            let salt = salt_of(salt_variable, Problem::KeySaltUnusable)?;
            keyed(list().await?, &salt, key).ok_or(Failure::own(Problem::DeviceKeyUnknown))?
        }
    };
    context.device.set(Some(device.device_id));
    Ok(device)
}

/// Of the rows usbmuxd lists, the one the phone whose key under `salt` is `key` is reached
/// through. The rows with that key are rows of one phone (a key is made of the identifier usbmuxd
/// lists the phone with): of a phone listed both over USB and over the network its USB row is
/// taken, whichever of them usbmuxd lists first; of a phone not listed over USB, the first of its
/// rows. `None` when no row has that key.
fn keyed(listed: Vec<UsbmuxdDevice>, salt: &Salt, key: &str) -> Option<UsbmuxdDevice> {
    listed
        .into_iter()
        .filter(|device| DeviceKey::of(salt, &device.udid).matches(key))
        // Of rows with equal values `min_by_key` takes the first: a USB row (`false`) before any
        // other, and otherwise the first row.
        .min_by_key(|device| device.connection_type != Connection::Usb)
}

/// The salt of the environment variable `variable` of this process, or `unusable`.
fn salt_of(variable: &str, unusable: Problem) -> Result<Salt, Failure> {
    Salt::from_environment(variable, &|name: &str| std::env::var_os(name))
        .ok_or(Failure::own(unusable))
}

/// The pairing record usbmuxd holds of this host for the device, `None` when it holds none.
/// usbmuxd's answer without a record is the library's `UnexpectedResponse`.
async fn pairing(device: &UsbmuxdDevice) -> Result<Option<PairingFile>, Failure> {
    let mut usbmuxd = UsbmuxdConnection::default()
        .await
        .map_err(|error| Failure::library(Problem::UsbmuxdUnreachable, &error))?;
    match usbmuxd.get_pair_record(&device.udid).await {
        Ok(record) => Ok(Some(record)),
        Err(IdeviceError::UnexpectedResponse(_)) => Ok(None),
        Err(error) => Err(Failure::library(Problem::PairRecordUnreadable, &error)),
    }
}

/// The pairing record, or `not_paired`.
async fn paired_record(device: &UsbmuxdDevice) -> Result<PairingFile, Failure> {
    pairing(device)
        .await?
        .ok_or(Failure::own(Problem::NotPaired))
}

/// A connection to `service`, started by lockdown in a session with this host's record.
async fn open_service(
    provider: &UsbmuxdProvider,
    record: &PairingFile,
    service: Cow<'static, str>,
) -> Result<Idevice, Failure> {
    let mut lockdown = <LockdownClient as IdeviceService>::connect(provider)
        .await
        .map_err(|error| Failure::library(Problem::LockdownUnreachable, &error))?;
    let legacy = lockdown
        .start_session(record)
        .await
        .map_err(|error| Failure::library(Problem::SessionRefused, &error))?;
    let (port, tls) = lockdown
        .start_service(service)
        .await
        .map_err(|error| Failure::library(Problem::ServiceRefused, &error))?;
    let mut connection = provider
        .connect(port)
        .await
        .map_err(|error| Failure::library(Problem::ServiceUnreachable, &error))?;
    if tls {
        connection
            .start_session(record, legacy)
            .await
            .map_err(|error| Failure::library(Problem::ServiceUnreachable, &error))?;
    }
    Ok(connection)
}

/// A string value of the default domain, read without a session.
async fn read_text(lockdown: &mut LockdownClient, key: &str) -> Result<String, Failure> {
    let value = lockdown
        .get_value(Some(key), None)
        .await
        .map_err(|error| Failure::library(Problem::LockdownReadFailed, &error))?;
    value
        .as_string()
        .map(str::to_owned)
        .ok_or(Failure::own(Problem::LockdownValueUnexpected))
}

/// A boolean value read in the session, or the word the device refused it with.
async fn read_flag(
    lockdown: &mut LockdownClient,
    key: &str,
    domain: Option<&str>,
) -> Result<Reading, Failure> {
    match lockdown.get_value(Some(key), domain).await {
        Ok(value) => value
            .as_boolean()
            .map(Reading::Value)
            .ok_or(Failure::own(Problem::LockdownValueUnexpected)),
        Err(error) => refused_with(&error)
            .map(Reading::Refused)
            .ok_or_else(|| Failure::library(Problem::LockdownReadFailed, &error)),
    }
}

/// The word of a refusal the device itself sent: the library names the words it knows by a
/// kind of its own and keeps any other word in `UnknownErrorType`.
fn refused_with(error: &IdeviceError) -> Option<String> {
    match error {
        IdeviceError::GetProhibited
        | IdeviceError::DeviceLocked
        | IdeviceError::SessionInactive
        | IdeviceError::InvalidHostID => Some(redact::kind_of(&format!("{error:?}"))),
        IdeviceError::UnknownErrorType(text) => Some(redact::refusal_word(text)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::tests::{VECTOR_IDENTIFIER, VECTOR_KEY, VECTOR_SALT, salt};
    use crate::matching::Pids;
    use crate::stopping::Verdict;
    use idevice::services::dvt::message::{MessageHeader, PayloadHeader};
    use std::net::{IpAddr, Ipv4Addr};

    /// The identifier of another phone than the test vector's.
    const OTHER_IDENTIFIER: &str = "TESTSERIAL0002";

    /// A row of usbmuxd's list: the device `number`, listed with `identifier` over `connection`.
    fn row(number: u32, connection: Connection, identifier: &str) -> UsbmuxdDevice {
        UsbmuxdDevice {
            connection_type: connection,
            udid: identifier.to_owned(),
            device_id: number,
        }
    }

    /// A network connection (of no address in particular).
    fn network() -> Connection {
        Connection::Network(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
    }

    /// The number of the row the phone of the test vector's key is reached through.
    fn taken(listed: Vec<UsbmuxdDevice>) -> Option<u32> {
        keyed(listed, &salt(VECTOR_SALT), VECTOR_KEY).map(|device| device.device_id)
    }

    /// A phone usbmuxd lists over USB and then over the network is reached through its USB row,
    /// and another phone's USB row listed before both is not taken.
    #[test]
    fn a_phone_listed_over_usb_then_the_network_is_reached_through_its_usb_row() {
        let listed = vec![
            row(2, Connection::Usb, OTHER_IDENTIFIER),
            row(3, Connection::Usb, VECTOR_IDENTIFIER),
            row(7, network(), VECTOR_IDENTIFIER),
        ];
        assert_eq!(taken(listed), Some(3));
    }

    /// A phone usbmuxd lists over the network and then over USB is reached through its USB row
    /// too: which row is listed first does not decide, and another phone's USB row listed between
    /// them is not taken.
    #[test]
    fn a_phone_listed_over_the_network_then_usb_is_reached_through_its_usb_row() {
        let listed = vec![
            row(7, network(), VECTOR_IDENTIFIER),
            row(2, Connection::Usb, OTHER_IDENTIFIER),
            row(3, Connection::Usb, VECTOR_IDENTIFIER),
        ];
        assert_eq!(taken(listed), Some(3));
    }

    /// A phone usbmuxd lists only over the network is found by its network row, and another
    /// phone's USB row listed before it is not taken in its place.
    #[test]
    fn a_phone_listed_only_over_the_network_is_found_by_its_network_row() {
        let listed = vec![
            row(2, Connection::Usb, OTHER_IDENTIFIER),
            row(7, network(), VECTOR_IDENTIFIER),
        ];
        assert_eq!(taken(listed), Some(7));
    }

    /// Each value of the library's `InterfaceOrientation` has its word of [`Orientation`], one
    /// word for each value.
    #[test]
    fn each_orientation_the_library_reports_has_its_word() {
        let cases = [
            (InterfaceOrientation::Unknown, Orientation::Unknown),
            (InterfaceOrientation::Portrait, Orientation::Portrait),
            (
                InterfaceOrientation::PortraitUpsideDown,
                Orientation::PortraitUpsideDown,
            ),
            (
                InterfaceOrientation::LandscapeRight,
                Orientation::LandscapeRight,
            ),
            (
                InterfaceOrientation::LandscapeLeft,
                Orientation::LandscapeLeft,
            ),
        ];
        let mut words = std::collections::BTreeSet::new();
        for (library, word) in cases {
            assert_eq!(reported(library), word, "{library:?}");
            assert!(words.insert(format!("{word:?}")), "{word:?} is one word");
        }
        assert_eq!(words.len(), 5);
    }

    /// What the phone's refusal could carry, as the library hands it: the debug text of the
    /// error dictionary, here naming another application, its path and a pid of it.
    const REFUSAL_TEXT: &str = r#"Dictionary({"NSLocalizedDescription": String("com.example.otherapp at /private/var/containers/Bundle/Application/TESTUUID-0002/OtherApp.app is running as pid 4242")})"#;
    /// Pieces of [`REFUSAL_TEXT`] no answer may hold.
    const REFUSAL_PIECES: [&str; 5] = [
        "otherapp",
        "OtherApp",
        "TESTUUID",
        "4242",
        "NSLocalizedDescription",
    ];

    /// Each error of the application service the library tells apart maps to its kind, and
    /// every other to `other`: the service's own refusal, its answer in a shape the library
    /// does not read, the connection, the XPC layer.
    #[test]
    fn each_error_of_the_application_service_maps_to_its_kind() {
        use idevice::xpc::errors::XpcError;
        let cases = [
            (
                IdeviceError::CoreDevice(CoreDeviceError::DeviceError(REFUSAL_TEXT.to_owned())),
                ErrorKind::DeviceRefused,
            ),
            (
                IdeviceError::CoreDevice(CoreDeviceError::MissingField("CoreDevice.output")),
                ErrorKind::AnswerUnreadable,
            ),
            (
                IdeviceError::CoreDevice(CoreDeviceError::MalformedField("(root)")),
                ErrorKind::AnswerUnreadable,
            ),
            (
                IdeviceError::UnexpectedResponse(
                    "missing or unparsable processToken in launch response".to_owned(),
                ),
                ErrorKind::AnswerUnreadable,
            ),
            (
                IdeviceError::Socket(std::io::Error::other("connection reset")),
                ErrorKind::ConnectionFailed,
            ),
            (
                IdeviceError::Xpc(XpcError::HttpStreamReset),
                ErrorKind::XpcFailed,
            ),
            (
                IdeviceError::Xpc(XpcError::HttpGoAway(REFUSAL_TEXT.to_owned())),
                ErrorKind::XpcFailed,
            ),
            (
                IdeviceError::CoreDevice(CoreDeviceError::Negotiation(REFUSAL_TEXT.to_owned())),
                ErrorKind::Other,
            ),
            (IdeviceError::DeviceLocked, ErrorKind::Other),
            (IdeviceError::NotFound, ErrorKind::Other),
            (IdeviceError::Timeout, ErrorKind::Other),
        ];
        for (error, kind) in cases {
            assert_eq!(service_error_kind(&error), kind, "{error:?}");
        }
    }

    /// The service's refusal carries the phone's text (the library's own text of the error
    /// holds it), and no answer of `running`, `launch` or `stop` built from that error holds
    /// any of it: only the kind.
    #[test]
    fn the_text_of_a_service_error_is_in_no_answer() {
        let error = IdeviceError::CoreDevice(CoreDeviceError::DeviceError(REFUSAL_TEXT.to_owned()));
        let shown_by_the_library = format!("{error} {error:?}");
        for piece in REFUSAL_PIECES {
            assert!(shown_by_the_library.contains(piece), "{piece}");
        }
        let kind = service_error_kind(&error);
        let lines = [
            Line::Running(
                Running::asked(3, "com.example.app")
                    .failed(Problem::ApplicationLookupFailed, Some(kind)),
            ),
            Line::Running(
                Running::asked(3, "com.example.app")
                    .installed()
                    .failed(Problem::ProcessListFailed, Some(kind)),
            ),
            Line::Launch(
                Launch::asked(3, "com.example.app", true)
                    .installed()
                    .failed(Problem::LaunchFailed, Some(kind)),
            ),
            Line::Stop(Stop::asked(3, "com.example.app").look_failed(
                Signals {
                    signalled: vec![412, 413],
                    errors: vec![
                        SignalError::request(412, kind),
                        // Fix round 1: the reason of a failed wait is the error's variant alone.
                        SignalError::reply(413, kind, reply_failure(&error)),
                    ],
                },
                kind,
            )),
        ];
        for line in lines {
            let text = serde_json::to_string(&line).expect("a line serializes");
            assert!(text.contains(r#""error_kind":"device_refused""#), "{text}");
            for piece in REFUSAL_PIECES {
                assert!(!text.contains(piece), "{piece} in {text}");
            }
        }
    }

    /// (ACIB-3c) What `launch` and `stop` (and `running`) do for an application the installation
    /// proxy's lookup does not hold, and for one that does not run, decided before anything more
    /// is asked or sent. Not installed, each answers at once: `launch` `not_installed` (status 1),
    /// no launch sent and the instruments server not asked; `stop` as nothing ran, `installed`
    /// false (status 0), nothing sent; `running` `installed` and `running` false. The lookup
    /// failing, each answers `application_lookup_failed` with its kind. A path read as no place:
    /// `running` and `stop` answer `bundle_path_unreadable`, `launch` goes on (it needs no path).
    /// Installed, each goes on, `running` and `stop` with the bundle's directories, `launch` to
    /// send its launch. At `stop`'s first look an application that does not run is answered at
    /// once (status 0), its extensions named and no kill sent; one that runs goes on to the kills
    /// of its own pids alone, not of its extensions'. (Fix round 1) The first look's `located` is
    /// named in that answer.
    #[test]
    fn launch_and_stop_send_nothing_for_an_application_not_installed_or_not_running() {
        const BUNDLE: &str = "com.example.app";
        let directories =
            || vec!["/var/containers/Bundle/Application/TESTUUID-0001/Example.app".to_owned()];
        let running = || Running::asked(3, BUNDLE);
        let launch = || Launch::asked(3, BUNDLE, false);
        let stop = || Stop::asked(3, BUNDLE);
        let shown = |line: &Line| serde_json::to_string(line).expect("a line serializes");

        let Then::Answer(not_installed) =
            launch_after_lookup(launch(), Ok(Installation::NotInstalled))
        else {
            panic!("launch answers a bundle not installed at once");
        };
        let not_installed = *not_installed;
        assert_eq!(not_installed, Line::Launch(launch().not_installed()));
        assert_eq!(
            shown(&not_installed),
            r#"{"command":"launch","device":3,"bundle":"com.example.app","relaunch":false,"installed":false,"problem":"not_installed"}"#
        );
        assert!(!not_installed.answered());
        let Then::Answer(nothing_sent) = stop_after_lookup(stop(), Ok(Installation::NotInstalled))
        else {
            panic!("stop answers a bundle not installed at once");
        };
        let nothing_sent = *nothing_sent;
        assert_eq!(
            shown(&nothing_sent),
            r#"{"command":"stop","device":3,"bundle":"com.example.app","installed":false,"was_running":false,"signalled":[],"remaining":[],"running":false,"extension_pids":[]}"#
        );
        assert!(nothing_sent.answered());
        assert_eq!(
            running_after_lookup(running(), Ok(Installation::NotInstalled)),
            Then::answer(Line::Running(running().not_installed()))
        );

        for kind in [
            ErrorKind::AnswerUnreadable,
            ErrorKind::ConnectionFailed,
            ErrorKind::Other,
        ] {
            let failed = Some(kind);
            assert_eq!(
                launch_after_lookup(launch(), Err(kind)),
                Then::answer(Line::Launch(
                    launch().failed(Problem::ApplicationLookupFailed, failed)
                ))
            );
            assert_eq!(
                stop_after_lookup(stop(), Err(kind)),
                Then::answer(Line::Stop(
                    stop().failed(Problem::ApplicationLookupFailed, failed)
                ))
            );
            assert_eq!(
                running_after_lookup(running(), Err(kind)),
                Then::answer(Line::Running(
                    running().failed(Problem::ApplicationLookupFailed, failed)
                ))
            );
        }

        assert_eq!(
            launch_after_lookup(launch(), Ok(Installation::PathUnreadable)),
            Then::GoOn(launch().installed())
        );
        assert_eq!(
            stop_after_lookup(stop(), Ok(Installation::PathUnreadable)),
            Then::answer(Line::Stop(
                stop()
                    .installed()
                    .failed(Problem::BundlePathUnreadable, None)
            ))
        );
        assert_eq!(
            running_after_lookup(running(), Ok(Installation::PathUnreadable)),
            Then::answer(Line::Running(
                running()
                    .installed()
                    .failed(Problem::BundlePathUnreadable, None)
            ))
        );

        assert_eq!(
            launch_after_lookup(launch(), Ok(Installation::Installed(directories()))),
            Then::GoOn(launch().installed())
        );
        assert_eq!(
            stop_after_lookup(stop(), Ok(Installation::Installed(directories()))),
            Then::GoOn((stop().installed(), directories()))
        );
        assert_eq!(
            running_after_lookup(running(), Ok(Installation::Installed(directories()))),
            Then::GoOn((running().installed(), directories()))
        );

        // (Fix round 1) A look locating 180 processes; how the application was located when it
        // runs.
        let look = |application: &[u32], extensions: &[u32]| Look {
            pids: Pids {
                application: application.to_vec(),
                extensions: extensions.to_vec(),
            },
            located: 180,
            application_located: (!application.is_empty())
                .then_some(ApplicationLocated::DirectlyInside),
        };
        for extensions in [Vec::new(), vec![530]] {
            assert_eq!(
                stop_after_first_look(stop().installed(), look(&[], &extensions)),
                Then::answer(Line::Stop(stop().not_running(look(&[], &extensions))))
            );
        }
        let Then::Answer(not_running) =
            stop_after_first_look(stop().installed(), look(&[], &[530]))
        else {
            panic!("stop answers an application that does not run at once");
        };
        let not_running = *not_running;
        assert_eq!(
            shown(&not_running),
            r#"{"command":"stop","device":3,"bundle":"com.example.app","installed":true,"was_running":false,"signalled":[],"remaining":[],"running":false,"extension_pids":[530],"located":180}"#
        );
        assert!(not_running.answered());
        assert_eq!(
            stop_after_first_look(stop().installed(), look(&[412, 413], &[530])),
            Then::GoOn((stop().installed(), vec![412, 413]))
        );
    }

    /// (ACIB-3c, fix round 1) What cannot be told is never "does not run". A list whose entries
    /// have no location read as a place (bare names, the bundle's executable's and its bundle's
    /// among them, a relative path, no `realAppName`, one that is no text) and an empty list
    /// locate no process: from them `running` answers `process_paths_unreadable` (status 1), not
    /// `running` false, and `stop` answers it at its first look and goes on to no kill request
    /// (its decision answers at once). A look after `stop`'s kills that locates none answers it
    /// too, keeping what was signalled, with no verdict. One entry with an absolute location, of
    /// another application, is enough for `running` false and for `stop` to answer that nothing
    /// ran, each with `located` 1.
    #[test]
    fn a_list_that_locates_no_process_is_never_an_application_that_does_not_run() {
        use crate::matching::tests::{answer, at, listed};
        const BUNDLE: &str = "com.example.app";
        let directories =
            ["/var/containers/Bundle/Application/TESTUUID-0001/Example.app".to_owned()];
        let unlocated = || {
            vec![
                at(300, "Example"),
                at(301, "Example.app"),
                at(
                    302,
                    "var/containers/Bundle/Application/TESTUUID-0001/Example.app/Example",
                ),
                listed(XPCObject::UInt64(303), None),
                listed(XPCObject::UInt64(304), Some(XPCObject::UInt64(9))),
            ]
        };
        let look_at = |reply: &idevice::services::dvt::message::Message| {
            let processes = matching::listed_processes(reply).expect("a list");
            matching::pids_inside(&directories, processes)
        };
        let shown = |line: &Line| serde_json::to_string(line).expect("a line serializes");
        let running = || Running::asked(3, BUNDLE).installed();
        let stop = || Stop::asked(3, BUNDLE).installed();

        for reply in [answer(unlocated()), answer(Vec::new())] {
            let look = look_at(&reply);
            assert_eq!(look.located, 0);
            let line = Line::Running(running_after_look(running(), Ok(look.clone())));
            assert_eq!(
                shown(&line),
                r#"{"command":"running","device":3,"bundle":"com.example.app","installed":true,"problem":"process_paths_unreadable"}"#
            );
            assert!(!line.answered());
            let Then::Answer(stopped) = stop_after_first_look(stop(), look) else {
                panic!("stop sends no kill request for a list that locates no process");
            };
            let stopped = *stopped;
            assert_eq!(
                shown(&stopped),
                r#"{"command":"stop","device":3,"bundle":"com.example.app","installed":true,"problem":"process_paths_unreadable"}"#
            );
            assert!(!stopped.answered());
        }

        let after_kills = Line::Stop(Stop::asked(3, BUNDLE).looked(
            Signals {
                signalled: vec![412],
                errors: Vec::new(),
            },
            look_at(&answer(unlocated())),
            Verdict::Stopped,
        ));
        assert_eq!(
            shown(&after_kills),
            r#"{"command":"stop","device":3,"bundle":"com.example.app","installed":true,"was_running":true,"signalled":[412],"problem":"process_paths_unreadable"}"#
        );
        assert!(!after_kills.answered());

        let mut one_located = unlocated();
        one_located.push(at(
            201,
            "/var/containers/Bundle/Application/TESTUUID-0002/Other.app/Other",
        ));
        let look = look_at(&answer(one_located));
        let line = Line::Running(running_after_look(running(), Ok(look.clone())));
        assert_eq!(
            shown(&line),
            r#"{"command":"running","device":3,"bundle":"com.example.app","installed":true,"running":false,"pids":[],"extension_pids":[],"located":1}"#
        );
        assert!(line.answered());
        let Then::Answer(nothing_ran) = stop_after_first_look(stop(), look) else {
            panic!("stop answers an application that does not run at once");
        };
        let nothing_ran = *nothing_ran;
        assert_eq!(
            shown(&nothing_ran),
            r#"{"command":"stop","device":3,"bundle":"com.example.app","installed":true,"was_running":false,"signalled":[],"remaining":[],"running":false,"extension_pids":[],"located":1}"#
        );
        assert!(nothing_ran.answered());
    }

    /// (ACIB-3c, fix round 2) A first look that locates the application's entries both at the
    /// bundle's directory and directly in it (`both`) fits neither way a list names the
    /// application: from it `stop` answers `application_entry_ambiguous` (status 1) at once and
    /// its decision gives no pid to send a kill request for (`stop` sends kill requests only for
    /// the pids [`stop_after_first_look`] goes on with, so none is sent); `running` on the same
    /// list answers `running` true with the pids of both entries and `application_located`
    /// `both` (status 0). A list that names the application either way alone goes on to the
    /// kills of its pids.
    #[test]
    fn stop_sends_no_kill_request_when_the_list_does_not_tell_which_entry_is_the_application() {
        use crate::matching::tests::{answer, at};
        const BUNDLE: &str = "com.example.app";
        const DIRECTORY: &str = "/var/containers/Bundle/Application/TESTUUID-0001/Example.app";
        let directories = [DIRECTORY.to_owned()];
        let look_at = |reply: &idevice::services::dvt::message::Message| {
            let processes = matching::listed_processes(reply).expect("a list");
            matching::pids_inside(&directories, processes)
        };
        let shown = |line: &Line| serde_json::to_string(line).expect("a line serializes");
        let stop = || Stop::asked(3, BUNDLE).installed();

        let both = look_at(&answer(vec![
            at(413, &format!("{DIRECTORY}/Example")),
            at(412, DIRECTORY),
            at(530, &format!("{DIRECTORY}/PlugIns/Widget.appex/Widget")),
            at(
                201,
                "/var/containers/Bundle/Application/TESTUUID-0002/Other.app/Other",
            ),
        ]));
        assert_eq!(both.application_located, Some(ApplicationLocated::Both));
        let Then::Answer(stopped) = stop_after_first_look(stop(), both.clone()) else {
            panic!("stop sends no kill request when the list does not tell the application");
        };
        let stopped = *stopped;
        assert_eq!(
            shown(&stopped),
            r#"{"command":"stop","device":3,"bundle":"com.example.app","installed":true,"problem":"application_entry_ambiguous"}"#
        );
        assert!(!stopped.answered());
        let line = Line::Running(running_after_look(
            Running::asked(3, BUNDLE).installed(),
            Ok(both),
        ));
        assert_eq!(
            shown(&line),
            r#"{"command":"running","device":3,"bundle":"com.example.app","installed":true,"running":true,"pids":[412,413],"extension_pids":[530],"located":4,"application_located":"both"}"#
        );
        assert!(line.answered());

        for (entry, pids) in [
            (at(413, &format!("{DIRECTORY}/Example")), vec![413]),
            (at(412, DIRECTORY), vec![412]),
        ] {
            assert_eq!(
                stop_after_first_look(stop(), look_at(&answer(vec![entry]))),
                Then::GoOn((stop(), pids))
            );
        }
    }

    // (ACIB-3d) A scripted instruments server: the library's own client on one end of a stream in
    // memory, and on the other a task that reads each message the client writes, as the library
    // reads one (`Message::from_reader`), and does with it what a script says.

    /// The selector of the library's request for a channel (`services/dvt/remote_server.rs` 447).
    const REQUEST_CHANNEL: &str = "_requestChannelWithCode:identifier:";
    /// The tests' bundle, its directory, its application's process and its extension's, and
    /// another application's process.
    const BUNDLE: &str = "com.example.app";
    const BUNDLE_DIRECTORY: &str = "/var/containers/Bundle/Application/TESTUUID-0001/Example.app";
    const APPLICATION_PID: u64 = 412;
    const EXTENSION_PID: u64 = 530;
    const OTHER_PID: u64 = 201;
    const OTHER_EXECUTABLE: &str =
        "/var/containers/Bundle/Application/TESTUUID-0002/Other.app/Other";

    /// An argument of a request the scripted server read: a 32-bit number, an archived value as
    /// the library's own reader decodes one (a number or a text), or another.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Argument {
        Number(u32),
        ArchivedNumber(u64),
        ArchivedText(String),
        Other,
    }

    /// A request the scripted server read: the channel the client wrote it on, its selector, its
    /// arguments and whether it asked for an answer.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Sent {
        channel: i32,
        selector: Option<String>,
        arguments: Vec<Argument>,
        answer_asked: bool,
    }

    /// What the scripted server does with a request it read.
    enum Act {
        /// Answers it (as the library numbers an answer: the request's identifier, the
        /// conversation index one above, the request's channel), with this payload.
        Answer(Option<XPCObject>),
        /// Answers nothing.
        Nothing,
        /// Ends the connection.
        End,
        /// (Fix round 1) Answers it later, without a payload, if [`Act::Last`] says so.
        Hold,
        /// (Fix round 1) Answers it with this payload, then (`then_held`) each request held
        /// before, in the order held, and ends the connection.
        Last {
            payload: Option<XPCObject>,
            then_held: bool,
        },
    }

    /// The value the archive `archive` holds, as the library's own reader of a message decodes a
    /// payload (the archive put as the payload of a message of its own: idevice 0.1.68,
    /// `services/dvt/message.rs` 585-665).
    async fn unarchived(archive: &[u8]) -> Argument {
        let total = u32::try_from(archive.len()).expect("a short archive");
        let mut frame = Vec::new();
        for word in [0x1F3D_5B79_u32, 32] {
            frame.extend_from_slice(&word.to_le_bytes());
        }
        frame.extend_from_slice(&0_u16.to_le_bytes());
        frame.extend_from_slice(&1_u16.to_le_bytes());
        for word in [16 + total, 1, 0, 1, 0] {
            frame.extend_from_slice(&word.to_le_bytes());
        }
        frame.extend_from_slice(&[2, 0, 0, 0]);
        for word in [0, total, 0] {
            frame.extend_from_slice(&word.to_le_bytes());
        }
        frame.extend_from_slice(archive);
        let mut reader: &[u8] = &frame;
        let data = Message::from_reader(&mut reader)
            .await
            .ok()
            .and_then(|message| message.data);
        match data {
            Some(value) => match (value.as_unsigned_integer(), value.as_string()) {
                (Some(number), _) => Argument::ArchivedNumber(number),
                (None, Some(text)) => Argument::ArchivedText(text.to_owned()),
                (None, None) => Argument::Other,
            },
            None => Argument::Other,
        }
    }

    /// One end of a connection to a scripted instruments server, as `acib` makes its client of one,
    /// and the server's task: it reads each request the client writes and does with it what
    /// `script` says, until it ends the connection or the client's end is gone, and gives every
    /// request it read, in order.
    fn scripted(script: fn(&Sent) -> Act) -> (Instruments, tokio::task::JoinHandle<Vec<Sent>>) {
        use tokio::io::AsyncWriteExt;
        let (ours, theirs) = tokio::io::duplex(1 << 20);
        let server = tokio::spawn(async move {
            let (mut reader, mut writer) = tokio::io::split(theirs);
            let mut read = Vec::new();
            // The identifier and channel of each request held, in the order held.
            let mut held = Vec::new();
            while let Ok(message) = Message::from_reader(&mut reader).await {
                let header = message.message_header.serialize();
                let word = |at: usize| {
                    u32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
                };
                let mut arguments = Vec::new();
                for value in message.aux.map(|aux| aux.values).unwrap_or_default() {
                    arguments.push(match value {
                        AuxValue::U32(number) => Argument::Number(number),
                        AuxValue::Array(archive) => unarchived(&archive).await,
                        _ => Argument::Other,
                    });
                }
                let sent = Sent {
                    // A request's channel is read back negated, its conversation index being even
                    // (`services/dvt/message.rs` 599-608).
                    channel: -message.message_header.channel,
                    selector: message
                        .data
                        .as_ref()
                        .and_then(|data| data.as_string())
                        .map(str::to_owned),
                    arguments,
                    answer_asked: word(28) == 1,
                };
                let act = script(&sent);
                let (identifier, channel) = (word(16), sent.channel);
                read.push(sent);
                match act {
                    Act::Answer(data) => {
                        if writer
                            .write_all(&answer_to(identifier, channel, data))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Act::Nothing => {}
                    Act::End => break,
                    Act::Hold => held.push((identifier, channel)),
                    Act::Last { payload, then_held } => {
                        let mut answers = vec![answer_to(identifier, channel, payload)];
                        if then_held {
                            answers.extend(
                                held.iter()
                                    .map(|&(earlier, on)| answer_to(earlier, on, None)),
                            );
                        }
                        for answer in answers {
                            if writer.write_all(&answer).await.is_err() {
                                break;
                            }
                        }
                        break;
                    }
                }
            }
            read
        });
        (Instruments::new(ours), server)
    }

    /// The bytes of the scripted server's answer to the request `identifier` on `channel`, with
    /// the payload `data` (as the library numbers an answer: the request's identifier, the
    /// conversation index one above, the request's channel).
    fn answer_to(identifier: u32, channel: i32, data: Option<XPCObject>) -> Vec<u8> {
        Message::new(
            MessageHeader::new(0, 1, identifier, 1, channel, false),
            PayloadHeader::new(),
            None,
            data.map(|value| value.to_plist()),
        )
        .serialize()
    }

    /// `future` run to its end on a current-thread runtime of its own, as `main` runs a command.
    fn on_a_runtime<F: Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime")
            .block_on(future)
    }

    /// The list of running processes the scripted server answers: another application's process,
    /// the bundle's extension and, `with_application`, the application itself.
    fn processes(with_application: bool) -> Option<XPCObject> {
        use crate::matching::tests::at;
        let mut entries = vec![
            at(OTHER_PID, OTHER_EXECUTABLE),
            at(
                EXTENSION_PID,
                &format!("{BUNDLE_DIRECTORY}/PlugIns/Widget.appex/Widget"),
            ),
        ];
        if with_application {
            entries.push(at(APPLICATION_PID, &format!("{BUNDLE_DIRECTORY}/Example")));
        }
        Some(XPCObject::Array(entries))
    }

    /// The first connection's server: it opens each channel asked for, lists the application, and
    /// answers each kill request that asks for an answer, without a payload.
    fn answers_and_lists_the_application(sent: &Sent) -> Act {
        match sent.selector.as_deref() {
            Some(REQUEST_CHANNEL) => Act::Answer(None),
            Some(RUNNING_PROCESSES) => Act::Answer(processes(true)),
            Some(KILL_PID) if sent.answer_asked => Act::Answer(None),
            _ => Act::Nothing,
        }
    }

    /// The first connection's server, ending the connection as soon as it has read a kill request.
    fn ends_at_the_kill(sent: &Sent) -> Act {
        match sent.selector.as_deref() {
            Some(KILL_PID) => Act::End,
            _ => answers_and_lists_the_application(sent),
        }
    }

    /// The looks' server: it opens each channel asked for and lists no process of the application.
    fn lists_no_application(sent: &Sent) -> Act {
        match sent.selector.as_deref() {
            Some(REQUEST_CHANNEL) => Act::Answer(None),
            Some(RUNNING_PROCESSES) => Act::Answer(processes(false)),
            _ => Act::Nothing,
        }
    }

    /// The looks' server, ending the connection at the first request it reads.
    fn ends_at_once(_: &Sent) -> Act {
        Act::End
    }

    /// (Fix round 1) The application's second process, its executable directly in the bundle's
    /// directory too (a helper beside the main executable).
    const SECOND_PID: u64 = 414;

    /// (Fix round 1) The first connection's server for an application of two processes: it opens
    /// each channel asked for and lists both (beside another application's process and the
    /// bundle's extension).
    fn lists_two_processes(sent: &Sent) -> Act {
        use crate::matching::tests::at;
        match sent.selector.as_deref() {
            Some(REQUEST_CHANNEL) => Act::Answer(None),
            Some(RUNNING_PROCESSES) => Act::Answer(Some(XPCObject::Array(vec![
                at(OTHER_PID, OTHER_EXECUTABLE),
                at(
                    EXTENSION_PID,
                    &format!("{BUNDLE_DIRECTORY}/PlugIns/Widget.appex/Widget"),
                ),
                at(APPLICATION_PID, &format!("{BUNDLE_DIRECTORY}/Example")),
                at(SECOND_PID, &format!("{BUNDLE_DIRECTORY}/Helper")),
            ]))),
            _ => Act::Nothing,
        }
    }

    /// (Fix round 1) Whether `sent` is a kill request of the application's first process, its pid
    /// in either form.
    fn kills_the_first(sent: &Sent) -> bool {
        sent.selector.as_deref() == Some(KILL_PID)
            && matches!(
                sent.arguments.as_slice(),
                [Argument::Number(412) | Argument::ArchivedNumber(412)]
            )
    }

    /// (Fix round 1) As [`lists_two_processes`]; of the kill requests it holds the first's (412's)
    /// and, at the second's, answers the second with a payload, then the first without one (an
    /// answer late and out of order), and ends the connection.
    fn answers_the_second_kill_then_the_first(sent: &Sent) -> Act {
        match sent.selector.as_deref() {
            Some(KILL_PID) if kills_the_first(sent) => Act::Hold,
            Some(KILL_PID) => Act::Last {
                payload: Some(XPCObject::UInt64(1)),
                then_held: true,
            },
            _ => lists_two_processes(sent),
        }
    }

    /// (Fix round 1) As [`answers_the_second_kill_then_the_first`], but the first's answer never
    /// comes: at the second's, the server answers it with a payload and ends the connection.
    fn answers_only_the_second_kill(sent: &Sent) -> Act {
        match sent.selector.as_deref() {
            Some(KILL_PID) if kills_the_first(sent) => Act::Hold,
            Some(KILL_PID) => Act::Last {
                payload: Some(XPCObject::UInt64(1)),
                then_held: false,
            },
            _ => lists_two_processes(sent),
        }
    }

    /// (Fix round 1) The first connection's server whose list names the application's entries
    /// both at the bundle's directory and directly in it (`both`): it opens each channel asked for
    /// and answers the list, and nothing else.
    fn lists_an_ambiguous_application(sent: &Sent) -> Act {
        use crate::matching::tests::at;
        match sent.selector.as_deref() {
            Some(REQUEST_CHANNEL) => Act::Answer(None),
            Some(RUNNING_PROCESSES) => Act::Answer(Some(XPCObject::Array(vec![
                at(OTHER_PID, OTHER_EXECUTABLE),
                at(APPLICATION_PID, BUNDLE_DIRECTORY),
                at(SECOND_PID, &format!("{BUNDLE_DIRECTORY}/Example")),
            ]))),
            _ => Act::Nothing,
        }
    }

    /// The library's request opening the channel `name` as channel `code`
    /// (`services/dvt/remote_server.rs` 425-448), as the scripted server reads it.
    fn channel_request(code: u32, name: &str) -> Sent {
        Sent {
            channel: 0,
            selector: Some(REQUEST_CHANNEL.to_owned()),
            arguments: vec![
                Argument::Number(code),
                Argument::ArchivedText(name.to_owned()),
            ],
            answer_asked: true,
        }
    }

    /// The request of the list of running processes on channel `code`.
    fn list_request(code: i32) -> Sent {
        Sent {
            channel: code,
            selector: Some(RUNNING_PROCESSES.to_owned()),
            arguments: Vec::new(),
            answer_asked: true,
        }
    }

    /// The kill request on channel `code`, its one argument `argument`.
    fn kill_request(code: i32, argument: Argument, answer_asked: bool) -> Sent {
        Sent {
            channel: code,
            selector: Some(KILL_PID.to_owned()),
            arguments: vec![argument],
            answer_asked,
        }
    }

    /// `stop` from its first look on (`stopped`), the way `way`, on the first connection to the
    /// server `first`, the looks after the kills on the connection `looks` gives; the line, and
    /// the requests the first connection's server read.
    fn stopped_on(
        way: StopWay,
        first: fn(&Sent) -> Act,
        looks: Option<fn(&Sent) -> Act>,
    ) -> (Line, Vec<Sent>, Vec<Sent>) {
        on_a_runtime(async move {
            let (client, first) = scripted(first);
            let (connection, again) = match looks {
                Some(script) => {
                    let (looking, again) = scripted(script);
                    (Reconnected::Ok(looking), Some(again))
                }
                None => (
                    Reconnected::Err((Problem::ServiceUnreachable, ErrorKind::ConnectionFailed)),
                    None,
                ),
            };
            let context = Context::new(None, Instant::now());
            let line = stopped(
                client,
                std::future::ready(connection),
                Stop::asked(3, BUNDLE).installed(),
                &[BUNDLE_DIRECTORY.to_owned()],
                way,
                Duration::from_secs(5),
                &context,
            )
            .await;
            let read = first.await.expect("the first connection's server ends");
            let read_again = match again {
                Some(again) => again.await.expect("the looks' server ends"),
                None => Vec::new(),
            };
            (line, read, read_again)
        })
    }

    /// (ACIB-3d) Each way of `stop`, from its first look on, against scripted instruments servers.
    /// On the first connection: the first look (the device information channel, 1, and its list),
    /// then the process control channel (2) and one kill request of the application's pid on it,
    /// in the way's form (`archived` the pid archived as a number, `primitive` and `library` the
    /// 32-bit number; an answer asked for by `archived` and `primitive`, not by `library`), and
    /// nothing else. The looks after the kills are made on the second connection: its own device
    /// information channel (1) and its list, and nothing else. Its list without the application,
    /// each way answers it stopped (status 0), its extension left, every stage after the first
    /// look reached and, of a way that asks for answers, the answer the server gave (no payload,
    /// message type 0), `kill_replies` reached; `library` names no answer and no `kill_replies`.
    #[test]
    fn each_way_of_stop_sends_its_kill_and_looks_again_on_a_connection_of_its_own() {
        let shown = |line: &Line| serde_json::to_string(line).expect("a line serializes");
        for way in StopWay::ALL {
            let (line, first, again) = stopped_on(
                way,
                answers_and_lists_the_application,
                Some(lists_no_application),
            );
            let kill = match way {
                StopWay::Archived => Argument::ArchivedNumber(APPLICATION_PID),
                StopWay::Primitive | StopWay::Library => Argument::Number(412),
            };
            assert_eq!(
                first,
                [
                    channel_request(1, DEVICE_INFO_CHANNEL),
                    list_request(1),
                    channel_request(2, PROCESS_CONTROL_CHANNEL),
                    kill_request(2, kill, way.asks_answers()),
                ],
                "{way:?}"
            );
            assert_eq!(
                again,
                [channel_request(1, DEVICE_INFO_CHANNEL), list_request(1)],
                "{way:?}"
            );
            let (replies, reached) = if way.asks_answers() {
                (
                    r#""replies":[{"pid":412,"reply":"without_payload","message_type":0}],"#,
                    r#"{"kill_requests":true,"kill_replies":true,"look_connection":true,"looks_after_kills":true}"#,
                )
            } else {
                (
                    "",
                    r#"{"kill_requests":true,"look_connection":true,"looks_after_kills":true}"#,
                )
            };
            assert_eq!(
                shown(&line),
                format!(
                    r#"{{"command":"stop","device":3,"bundle":"com.example.app","installed":true,"was_running":true,"signalled":[412],{replies}"remaining":[],"running":false,"extension_pids":[530],"located":2,"reached":{reached}}}"#
                ),
                "{way:?}"
            );
            assert!(line.answered(), "{way:?}");
        }
    }

    /// (ACIB-3d) A kill request whose connection the phone ends as soon as it has read it (as the
    /// next request on the kill's connection failed at once on a phone with iOS 27.0.1). Of a way
    /// that asks for an answer, its pid is a failed request, `connection_failed`, (fix round 1)
    /// with the reason `reader_stopped`, the library's error once its reader of the connection has
    /// stopped, beside the verdict; `library`, which waits for nothing, names none. The looks
    /// after the kills are made
    /// on a connection of their own all the same, and its list decides: stopped, status 0. When
    /// the looks' server ends that connection too, at its first request, the look fails there:
    /// `process_list_failed` with `connection_failed`, `looks_after_kills` reached, and nothing
    /// said of whether the application runs (no `running`, `remaining` or verdict), status 1. When
    /// the looks' connection is not made, `service_unreachable` with its kind, `look_connection`
    /// reached and `looks_after_kills` not, nothing said of the application, status 1.
    #[test]
    fn a_kill_whose_connection_dies_is_looked_after_on_another_and_a_dead_look_names_its_stage() {
        let shown = |line: &Line| serde_json::to_string(line).expect("a line serializes");
        for way in StopWay::ALL {
            let errors = if way.asks_answers() {
                r#""signal_errors":[{"pid":412,"error_kind":"connection_failed","reply_failure":"reader_stopped"}],"#
            } else {
                ""
            };
            let replies = if way.asks_answers() {
                r#""kill_replies":true,"#
            } else {
                ""
            };
            let opening = format!(
                r#"{{"command":"stop","device":3,"bundle":"com.example.app","installed":true,"was_running":true,"signalled":[412],{errors}"#
            );

            let (line, first, again) =
                stopped_on(way, ends_at_the_kill, Some(lists_no_application));
            assert_eq!(
                first.last().and_then(|sent| sent.selector.as_deref()),
                Some(KILL_PID),
                "{way:?}"
            );
            assert_eq!(again.len(), 2, "{way:?}");
            assert_eq!(
                shown(&line),
                format!(
                    r#"{opening}"remaining":[],"running":false,"extension_pids":[530],"located":2,"reached":{{"kill_requests":true,{replies}"look_connection":true,"looks_after_kills":true}}}}"#
                ),
                "{way:?}"
            );
            assert!(line.answered(), "{way:?}");

            let (line, _, again) = stopped_on(way, ends_at_the_kill, Some(ends_at_once));
            assert_eq!(again, [channel_request(1, DEVICE_INFO_CHANNEL)], "{way:?}");
            assert_eq!(
                shown(&line),
                format!(
                    r#"{opening}"reached":{{"kill_requests":true,{replies}"look_connection":true,"looks_after_kills":true}},"problem":"process_list_failed","error_kind":"connection_failed"}}"#
                ),
                "{way:?}"
            );
            assert!(!line.answered(), "{way:?}");

            let (line, _, _) = stopped_on(way, ends_at_the_kill, None);
            assert_eq!(
                shown(&line),
                format!(
                    r#"{opening}"reached":{{"kill_requests":true,{replies}"look_connection":true,"looks_after_kills":false}},"problem":"service_unreachable","error_kind":"connection_failed"}}"#
                ),
                "{way:?}"
            );
            assert!(!line.answered(), "{way:?}");
        }
    }

    /// (ACIB-3d, fix round 1) The answers to the kill requests are paired with their requests by
    /// the number each carries, never by their place. Of an application of two processes (412 and
    /// 414, both executables directly in the bundle's directory), each way that asks for answers
    /// sends, after the first look and the process control channel's request, the two kill
    /// requests, 412's first, each in its form and asking for an answer. The server answering
    /// 414's first, with a payload, and 412's after it, without one: each pid has its own answer
    /// (414 `with_payload`, 412 `without_payload`). The server answering only 414's and then ending
    /// the connection: 414 has its answer, and 412, whose answer never came, is a failed request,
    /// `connection_failed` with `reader_stopped`, never 414's answer. The looks on their own
    /// connection decide either way (stopped, status 0). And the reason of a failed wait comes
    /// from the library's error value alone: the socket error of the kind `BrokenPipe` it makes
    /// once its reader has stopped (`services/dvt/remote_server.rs` 1245-1250) is `reader_stopped`,
    /// its `UnknownChannel` `channel_cancelled`, any other error `other`.
    #[test]
    fn the_answers_to_the_kills_are_paired_by_number_and_a_failed_wait_names_its_reason() {
        let shown = |line: &Line| serde_json::to_string(line).expect("a line serializes");
        let opening = r#"{"command":"stop","device":3,"bundle":"com.example.app","installed":true,"was_running":true,"signalled":[412,414],"#;
        let closing = r#""remaining":[],"running":false,"extension_pids":[530],"located":2,"reached":{"kill_requests":true,"kill_replies":true,"look_connection":true,"looks_after_kills":true}}"#;
        for way in StopWay::ALL.into_iter().filter(|way| way.asks_answers()) {
            let kill = |pid: u32| {
                let argument = match way {
                    StopWay::Archived => Argument::ArchivedNumber(u64::from(pid)),
                    StopWay::Primitive | StopWay::Library => Argument::Number(pid),
                };
                kill_request(2, argument, true)
            };
            let requests = [
                channel_request(1, DEVICE_INFO_CHANNEL),
                list_request(1),
                channel_request(2, PROCESS_CONTROL_CHANNEL),
                kill(412),
                kill(414),
            ];

            let (line, first, again) = stopped_on(
                way,
                answers_the_second_kill_then_the_first,
                Some(lists_no_application),
            );
            assert_eq!(first, requests, "{way:?}");
            assert_eq!(again.len(), 2, "{way:?}");
            assert_eq!(
                shown(&line),
                format!(
                    r#"{opening}"replies":[{{"pid":412,"reply":"without_payload","message_type":0}},{{"pid":414,"reply":"with_payload","message_type":0}}],{closing}"#
                ),
                "{way:?}"
            );
            assert!(line.answered(), "{way:?}");

            let (line, first, _) = stopped_on(
                way,
                answers_only_the_second_kill,
                Some(lists_no_application),
            );
            assert_eq!(first, requests, "{way:?}");
            assert_eq!(
                shown(&line),
                format!(
                    r#"{opening}"signal_errors":[{{"pid":412,"error_kind":"connection_failed","reply_failure":"reader_stopped"}}],"replies":[{{"pid":414,"reply":"with_payload","message_type":0}}],{closing}"#
                ),
                "{way:?}"
            );
            assert!(line.answered(), "{way:?}");
        }

        let closed = IdeviceError::Socket(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "remote server connection closed",
        ));
        assert_eq!(reply_failure(&closed), ReplyFailure::ReaderStopped);
        assert_eq!(service_error_kind(&closed), ErrorKind::ConnectionFailed);
        assert_eq!(
            reply_failure(&IdeviceError::Dvt(DvtError::UnknownChannel(2))),
            ReplyFailure::ChannelCancelled
        );
        for other in [
            IdeviceError::Socket(std::io::Error::from(std::io::ErrorKind::ConnectionReset)),
            IdeviceError::Socket(std::io::Error::from(std::io::ErrorKind::UnexpectedEof)),
            IdeviceError::UnexpectedResponse("unexpected response".to_owned()),
        ] {
            assert_eq!(reply_failure(&other), ReplyFailure::Other, "{other:?}");
        }
    }

    /// (ACIB-3d, fix round 1) Against a scripted instruments server whose first look lists the
    /// application's entries both at the bundle's directory and directly in it, each way of `stop`
    /// answers `application_entry_ambiguous` (status 1) and sends no kill: the first connection's
    /// server reads the first look alone (the device information channel and its list), and no
    /// process control channel's request and no `killPid:`; the looks' connection is asked
    /// nothing.
    #[test]
    fn stop_sends_no_kill_to_a_server_whose_list_does_not_tell_which_entry_is_the_application() {
        for way in StopWay::ALL {
            let (line, first, again) = stopped_on(
                way,
                lists_an_ambiguous_application,
                Some(lists_no_application),
            );
            assert_eq!(
                first,
                [channel_request(1, DEVICE_INFO_CHANNEL), list_request(1)],
                "{way:?}"
            );
            assert!(again.is_empty(), "{way:?}");
            assert_eq!(
                serde_json::to_string(&line).expect("a line serializes"),
                r#"{"command":"stop","device":3,"bundle":"com.example.app","installed":true,"problem":"application_entry_ambiguous"}"#,
                "{way:?}"
            );
            assert!(!line.answered(), "{way:?}");
        }
    }

    /// (ACIB-3c, fix round 1) The README's item on `running` opens with what a person sees when
    /// the match fails and what it means, then the failure of a list that locates nothing, before
    /// the command's own line; and it reads each outcome of `running` on a phone, in this order.
    /// Pinned as the README's lists of stages are (`stages.rs`): its text, every run of blanks
    /// read as one space.
    #[test]
    fn the_readme_says_first_what_a_failed_match_looks_like_and_how_to_read_running() {
        const FIRST: &str = "**A failed match looks like an application that does not run: `installed` true, `running` false, `pids` and `extension_pids` empty (and `located` above 0); of an application open on the phone it means only that `acib` matched none of the list's entries to it, for a cause it does not tell apart, not that the application does not run.**";
        const UNLOCATED: &str = "Where no entry of the list has a location read as a path, nothing can be matched: `running` then answers `process_paths_unreadable` (status 1), never `running` false.";
        const READING: &str = "**Reading `running` on a phone**, each outcome and what it means:";
        const OUTCOMES: [&str; 8] = [
            "`running` true with `pids`: a process of the application itself is listed, in front or in the background (which application is in front `acib` cannot tell); `application_located` says how its entry was found: `directly_inside` (an executable directly in the bundle's directory), `at_directory` (the directory itself) or `both`.",
            r#"An installed application that is open answered `installed` true, `running` false, `pids` and `extension_pids` empty (`located` above 0): it was not matched, and why is not told apart (its entry's location lies outside the bundle's directory, its entry carries a bare name where others carry paths, or the list does not hold it); it is not "does not run"."#,
            "`running` false with `extension_pids`: processes deeper in the bundle's directory are listed and none of the application itself: only an extension runs, or the application's own entry was not matched (as above).",
            "`installed` false: the installation proxy's lookup of the bundle, as written, does not hold it (an identifier written wrong; whether one written in another case than the phone's is found has not been seen); `stage_ms` names only `usbmuxd` and `application_lookup`: no tunnel was built.",
            "`process_paths_unreadable`: the list was read, but no entry of it has a location read as a path (an empty list, bare names, no `realAppName`): nothing could be matched, and whether the application runs is not told.",
            "`process_list_failed` with `answer_unreadable`: the device information channel's request was answered with a payload, or the list is of another shape; the two are not told apart. With `connection_failed`: the connection failed, or the library could not read a message the phone sent. With `other`: another error of the library, for example a channel the phone cancelled before `acib` read it (`services/dvt/remote_server.rs` 938-960).",
            "`deadline_passed` at `server_first_message`: the phone sent on the root channel no message the library hands on (none, or capabilities it could not decode); at `process_list`: the channel's request or the list was not answered in time.",
            "`service_handshake_failed`: the connection to the instruments server failed before or while its first message was read.",
        ];
        let readme = include_str!("../README.md").replace("\r\n", "\n");
        let flat = readme.split_whitespace().collect::<Vec<_>>().join(" ");
        let opening =
            format!("- {FIRST} {UNLOCATED} `acib running --device <n> --bundle <id>`, read-only:");
        assert!(
            flat.contains(&opening),
            "the README's item on running lacks: {opening}"
        );
        let reading = std::iter::once(READING.to_owned())
            .chain(OUTCOMES.iter().map(|outcome| format!("- {outcome}")))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(flat.contains(&reading), "the README lacks: {reading}");
    }

    /// Whether a text is a UUID of version 4 written as its text.
    fn is_uuid_text(text: &str) -> bool {
        let bytes = text.as_bytes();
        bytes.len() == 36
            && bytes.iter().enumerate().all(|(index, byte)| match index {
                8 | 13 | 18 | 23 => *byte == b'-',
                _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(byte),
            })
            && bytes[14] == b'4'
            && matches!(bytes[19], b'8' | b'9' | b'a' | b'b')
    }

    /// (ACIB-3b, second round) The streamed list's request is, member for member and in the
    /// library's order, the one the library's `stream_apps` sends (`services/core_device/mod.rs`
    /// 117-144) with the flags of `list_apps` as `probe` asks them; its identifiers are random
    /// UUIDs made for it, each written as a UUID's text, the side channel an XPC UUID; and the
    /// request encodes. (ACIB-3c) The name is the gate map's of ACIB-3b: the choice between the
    /// two lists by the service's features, which it also held, went with `running`, `launch`
    /// and `stop` no longer asking the service; only `probe`'s `stream_apps` asks this list now.
    #[test]
    fn the_list_is_streamed_only_when_the_service_names_it_as_the_library_streams_it() {
        let Some(XPCObject::Dictionary(request)) = streamed_list_request() else {
            panic!("the request is a dictionary");
        };
        let keys: Vec<&str> = request.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "CoreDevice.CoreDeviceDDIProtocolVersion",
                "CoreDevice.action",
                "CoreDevice.coreDeviceVersion",
                "CoreDevice.deviceIdentifier",
                "CoreDevice.featureIdentifier",
                "CoreDevice.input",
                "CoreDevice.invocationIdentifier",
            ]
        );
        assert_eq!(
            request.get("CoreDevice.CoreDeviceDDIProtocolVersion"),
            Some(&XPCObject::Int64(2))
        );
        assert_eq!(
            request.get("CoreDevice.action"),
            Some(&XPCObject::Dictionary(Dictionary::new()))
        );
        let mut version = Dictionary::new();
        version.insert("originalComponentsCount".to_owned(), XPCObject::Int64(2));
        version.insert(
            "components".to_owned(),
            XPCObject::Array(vec![XPCObject::UInt64(629), XPCObject::UInt64(3)]),
        );
        version.insert(
            "stringValue".to_owned(),
            XPCObject::String("629.3".to_owned()),
        );
        assert_eq!(
            request.get("CoreDevice.coreDeviceVersion"),
            Some(&XPCObject::Dictionary(version))
        );
        // (ACIB-3b, third round) Two dictionaries are equal whatever the order of their members
        // (an XPC dictionary is an `IndexMap`): the order of each one is pinned by its keys.
        let version_keys: Vec<&str> = match request.get("CoreDevice.coreDeviceVersion") {
            Some(XPCObject::Dictionary(version)) => version.keys().map(String::as_str).collect(),
            other => panic!("the version is {other:?}"),
        };
        assert_eq!(
            version_keys,
            ["originalComponentsCount", "components", "stringValue"]
        );
        assert_eq!(
            request.get("CoreDevice.featureIdentifier"),
            Some(&XPCObject::String(
                "com.apple.coredevice.feature.streamapplist".to_owned()
            ))
        );
        let identifier = |key: &str| match request.get(key) {
            Some(XPCObject::String(text)) => text.clone(),
            other => panic!("{key} is {other:?}"),
        };
        let (device, invocation) = (
            identifier("CoreDevice.deviceIdentifier"),
            identifier("CoreDevice.invocationIdentifier"),
        );
        assert!(is_uuid_text(&device), "{device}");
        assert!(is_uuid_text(&invocation), "{invocation}");
        assert_ne!(device, invocation);
        let Some(XPCObject::Dictionary(input)) = request.get("CoreDevice.input") else {
            panic!("the input is a dictionary");
        };
        let input_keys: Vec<&str> = input.keys().map(String::as_str).collect();
        assert_eq!(input_keys, ["actualInput", "streamProxy"]);
        let mut options = Dictionary::new();
        for (key, value) in [
            ("includeAppClips", false),
            ("includeRemovableApps", true),
            ("includeHiddenApps", true),
            ("includeInternalApps", false),
            ("includeDefaultApps", true),
            ("includeAppGroupIdentifiers", false),
            ("includeContainerPaths", false),
            ("requireContainerAccess", false),
        ] {
            options.insert(key.to_owned(), XPCObject::Bool(value));
        }
        assert_eq!(
            input.get("actualInput"),
            Some(&XPCObject::Dictionary(options))
        );
        let option_keys: Vec<&str> = match input.get("actualInput") {
            Some(XPCObject::Dictionary(options)) => options.keys().map(String::as_str).collect(),
            other => panic!("the flags are {other:?}"),
        };
        assert_eq!(
            option_keys,
            [
                "includeAppClips",
                "includeRemovableApps",
                "includeHiddenApps",
                "includeInternalApps",
                "includeDefaultApps",
                "includeAppGroupIdentifiers",
                "includeContainerPaths",
                "requireContainerAccess",
            ]
        );
        let Some(XPCObject::Dictionary(proxy)) = input.get("streamProxy") else {
            panic!("the stream proxy is a dictionary");
        };
        let proxy_keys: Vec<&str> = proxy.keys().map(String::as_str).collect();
        assert_eq!(proxy_keys, ["sideChannel"]);
        assert!(matches!(proxy.get("sideChannel"), Some(XPCObject::Uuid(_))));
        assert!(XPCObject::Dictionary(request.clone()).encode().is_ok());

        let (one, other) = (random_uuid(), random_uuid());
        assert_ne!(one, other);
        assert!(is_uuid_text(&uuid_text(one)));
        assert_eq!(
            uuid_text([
                0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0x4c, 0xde, 0x8f, 0x01, 0x23, 0x45, 0x67, 0x89,
                0xab, 0xcd
            ]),
            "01234567-89ab-4cde-8f01-23456789abcd"
        );
        assert!(matches!(uuid_object(one), Some(XPCObject::Uuid(_))));
    }

    /// (ACIB-3b, third round) The two lists asked in the form of the screenshot's request: the
    /// rule gives, for the screenshot's feature, the screenshot's own action (the one pair of the
    /// library the phone answers), and for the features of the two lists
    /// `com.apple.coredevice.action.listapps` and `com.apple.coredevice.action.listprocesses`; an
    /// identifier that is no feature's gives none. Both ways read their answer on the reply
    /// channel. The input of the list of applications, as the library takes it (a property list
    /// dictionary, made by the library's own conversion), holds the flags of `list_apps`, each
    /// with its value, and nothing else; that of the list of processes is empty.
    #[test]
    fn the_screenshots_form_asks_each_list_by_the_screenshots_rule() {
        assert_eq!(
            action_of("com.apple.coredevice.feature.capturescreenshot").as_deref(),
            Some("com.apple.coredevice.action.capturescreenshot")
        );
        assert_eq!(
            action_of(LIST_APPS_FEATURE).as_deref(),
            Some("com.apple.coredevice.action.listapps")
        );
        assert_eq!(
            action_of(LIST_PROCESSES_FEATURE).as_deref(),
            Some("com.apple.coredevice.action.listprocesses")
        );
        assert_eq!(action_of("com.apple.coredevice.appservice"), None);
        for way in [ProbeWay::ListAppsAction, ProbeWay::ListProcessesAction] {
            assert_eq!(way.reads(), tap::Reads::Reply, "{way:?}");
        }

        let input = XPCObject::Dictionary(list_options())
            .to_plist()
            .into_dictionary()
            .expect("the flags are a dictionary");
        assert_eq!(input.len(), LIST_OPTIONS.len());
        for (key, value) in LIST_OPTIONS {
            assert_eq!(
                input.get(key).and_then(|flag| flag.as_boolean()),
                Some(value),
                "{key}"
            );
        }
        let empty = XPCObject::Dictionary(Dictionary::new())
            .to_plist()
            .into_dictionary()
            .expect("an empty dictionary");
        assert!(empty.is_empty());
    }
}
