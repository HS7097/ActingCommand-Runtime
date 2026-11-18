// SPDX-License-Identifier: AGPL-3.0-only

//! The eleven commands against the device, through the `idevice` library (board #479, ACIB-0 to
//! ACIB-3b).
//!
//! (ACIB-3b, second round) `probe` asks the application service each way of [`ProbeWay::ALL`]
//! on a connection of its own, each read through `tap.rs`, and answers per way only a word, an
//! error's kind, a count and counts of what the phone sent. The list of applications of
//! `running`, `launch` and `stop` is asked the way the service itself names among its features
//! in the discovery handshake ([`Listing`]). (ACIB-3b, third round) `probe` also asks the two
//! lists in the form of the screenshot's request, which the phone answers (the library's
//! `invoke_with_plist_action`, with the action the screenshot's rule gives: [`action_of`]); and
//! `running`, `launch` and `stop` read their connections through a tap too ([`Watched`]), so that
//! an error after the phone ended the connection has a word of its own, `connection_closed`.
//!
//! Everything here only reads, but for `launch`, which launches one application, and `stop`,
//! which ends one application's own processes (ACIB-3). Of the application service's lists of
//! applications and processes nothing leaves this module but whether the one bundle asked for
//! is installed and the pids of its own processes, the application's and its extensions'
//! (`matching.rs`), and of its errors only their kind ([`service_error_kind`]). usbmuxd is reached where the library finds it on this platform
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
    Apps, Candidate, CommandName, DeviceEntry, Devices, Elapsed, ErrorKind, Failure, ImageQuery,
    ImageQueryName, Images, Info, Launch, Line, Link, ListedImage, Mounted, Orientation,
    Orientations, Probe, ProbeOutcome, ProbeResult, ProbeWay, Problem, Reader, Reading, Running,
    Screenshot, ServiceDetail, ServiceEntry, ServiceLine, Services, SignalError, Signals, Stage,
    Stop, Way, whole_milliseconds,
};
use crate::args::{KeyVariables, Out, Request, Target};
use crate::framing::Printed;
use crate::key::{DeviceKey, Salt};
use crate::matching::{self, Application, Installation, Pids, Process};
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
use idevice::services::dvt::remote_server::RemoteServerClient;
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
/// The signal `stop` sends each process of the application: SIGKILL, which a process can neither
/// catch nor ignore (SIGTERM, 15, can be caught and ignored), so the look after it tells whether
/// the processes ended and not whether the application chose to.
const KILL: u32 = 9;

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

/// (ACIB-3b, second round) The feature the application service names, among its features in the
/// discovery handshake, when it offers the streamed list of applications (idevice 0.1.68,
/// `services/core_device/app_service.rs` 209).
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
/// The flags of the list of applications, in the library's order (`app_list_options` in
/// `services/core_device/app_service.rs` 381-398): App Clips, removable, hidden, internal and
/// default applications asked as `list_apps` is asked here, then the three keys the library adds,
/// each `false`.
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

/// (ACIB-3b, second round) How the application service's list of applications is asked for: the
/// way the service names among its features in the discovery handshake, as the library's own tool
/// chooses (idevice's repository, `tools/src/app_service.rs`, commit 6e1222075e of 2026-08-15).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Listing {
    /// The library's `list_apps` on the service's client: the feature `listapps`, its answer read
    /// on the reply channel. When the service names no streamed list.
    ListApps,
    /// The streamed list, on a connection of its own to the service: the feature
    /// `streamapplist`, its answer read on the root channel. When the service names it.
    StreamAppList,
}

impl Listing {
    /// The way of the service whose features the handshake names as `features`.
    fn advertised(features: Option<&[String]>) -> Self {
        let streamed = features.is_some_and(|features| {
            features
                .iter()
                .any(|feature| feature == STREAMED_LIST_FEATURE)
        });
        if streamed {
            Self::StreamAppList
        } else {
            Self::ListApps
        }
    }
}

/// A device's number, the tunnel to it, the application service's port in it and the way its list
/// of applications is asked for, and the service's client, or the kind of the error the service's
/// handshake failed with; (ACIB-3b, third round) and what the phone sent on the client's
/// connection, which tells the kinds of its errors.
struct AppConnection {
    device: u32,
    /// The tunnel, which stays open while it is held.
    tunnel: AdapterHandle,
    port: u16,
    listing: Listing,
    client: Result<AppService, ErrorKind>,
    watch: Watched,
}

/// (ACIB-3b, third round) A connection of `running`, `launch` or `stop` to the application
/// service, read through a [`Tapped`] stream as `probe` reads its connections (every byte passed
/// on unchanged, only counted): an error on it is [`ErrorKind::ConnectionClosed`] once the phone
/// ended the connection or reset a stream of it (`tap.rs`, [`tap::kind_after`]), whatever the
/// library made of that, and otherwise the kind the library's error tells
/// ([`service_error_kind`]). So `answer_unreadable` is never a closed connection in these three.
#[derive(Default)]
struct Watched(Arc<Mutex<Seen>>);

impl Watched {
    /// `stream`, its reads counted by this watch.
    fn tapped(&self, stream: Box<dyn ReadWrite>) -> Box<dyn ReadWrite> {
        Box::new(Tapped::new(stream, Arc::clone(&self.0)))
    }

    /// The kind of `error`, an error of the library on this connection.
    fn kind(&self, error: &IdeviceError) -> ErrorKind {
        self.after(service_error_kind(error))
    }

    /// `kind`, or `connection_closed` when the phone ended this connection or reset a stream of
    /// it.
    fn after(&self, kind: ErrorKind) -> ErrorKind {
        tap::kind_after(&tap::summary_of(&self.0), kind)
    }
}

/// One application of a list, as `matching.rs` reads it: its bundle identifier and its path.
struct ListedApplication {
    bundle_identifier: String,
    path: String,
}

/// What a command keeps while it runs, for the line of a deadline that passes in it: the
/// command's deadline and start, the stage a screenshot is in (no other command sets it), the
/// stages of a command of the application service (`stages.rs`; no other command enters one)
/// and the number of the device once it is found (a device named by its key has none before).
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

    /// (ACIB-3b) A command of the application service enters `stage` now.
    fn enter(&self, stage: Stage) {
        self.steps.borrow_mut().enter(stage, self.started.elapsed());
    }

    /// (ACIB-3b) The line of a command of the application service that ends now, with how long
    /// each of its stages took.
    fn timed(&self, line: Line) -> Line {
        let times = self.steps.borrow_mut().end(self.started.elapsed());
        line.timed(times)
    }
}

/// Runs one request and gives what it prints.
pub async fn answer(request: Request, context: &Context) -> Printed {
    let (command, given, via) = (request.name(), request.device(), request.via());
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
        } => stop(&target, &bundle, context)
            .await
            .map(|line| Printed::line(context.timed(line))),
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
        Printed::line(Line::refused_on(Some(command), device, via, failure))
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
            Ok(Some(applications.len()))
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
/// and the pids of its processes and, apart, of its extensions'. The application service is
/// asked for its list of applications and, when the bundle is in it, its list of processes;
/// nothing is changed on the phone.
async fn running(target: &Target, bundle: &str, context: &Context) -> Result<Line, Failure> {
    let AppConnection {
        device: number,
        mut tunnel,
        port,
        listing,
        client: service,
        watch,
    } = app_service(target, context).await?;
    let asked = Running::asked(number, bundle);
    let mut client = match service {
        Ok(client) => client,
        Err(kind) => {
            let failed = asked.failed(Problem::ServiceHandshakeFailed, Some(kind));
            return Ok(Line::Running(failed));
        }
    };
    context.enter(Stage::ApplicationList);
    let listed = installation(&mut client, &watch, &mut tunnel, port, listing, bundle).await;
    let answer = match listed {
        Err(kind) => asked.failed(Problem::ApplicationListFailed, Some(kind)),
        Ok(Installation::NotInstalled) => asked.not_installed(),
        Ok(Installation::PathUnreadable) => asked
            .installed()
            .failed(Problem::BundlePathUnreadable, None),
        Ok(Installation::Installed(directories)) => {
            context.enter(Stage::ProcessList);
            match pids_of(&mut client, &watch, &directories).await {
                Ok(pids) => asked.found(pids),
                Err(kind) => asked
                    .installed()
                    .failed(Problem::ProcessListFailed, Some(kind)),
            }
        }
    };
    Ok(Line::Running(answer))
}

/// `launch`: the application service's list of applications is asked first, as `running` and
/// `stop` ask it; a bundle it does not hold answers `not_installed` and no launch is sent. Then
/// the service launches `bundle` and answers the pid of its process. Without `relaunch` an
/// application that runs is not ended first (`terminateExisting` false; what the phone then does
/// the library does not say); with it, it is ended and started anew. Of the request's members
/// `acib` chooses these: the application is not stopped at its start, with no argument, no
/// environment, no platform option and no standard input, output or error of its own; the
/// library fixes the rest (the active user, pseudo-terminals for standard I/O; README).
async fn launch(
    target: &Target,
    bundle: &str,
    relaunch: bool,
    context: &Context,
) -> Result<Line, Failure> {
    let AppConnection {
        device: number,
        mut tunnel,
        port,
        listing,
        client: service,
        watch,
    } = app_service(target, context).await?;
    let asked = Launch::asked(number, bundle, relaunch);
    let mut client = match service {
        Ok(client) => client,
        Err(kind) => {
            let failed = asked.failed(Problem::ServiceHandshakeFailed, Some(kind));
            return Ok(Line::Launch(failed));
        }
    };
    // Where the bundle lies is not needed to launch it: a path read as no place is installed.
    context.enter(Stage::ApplicationList);
    let listed = installation(&mut client, &watch, &mut tunnel, port, listing, bundle).await;
    let asked = match listed {
        Err(kind) => {
            let failed = asked.failed(Problem::ApplicationListFailed, Some(kind));
            return Ok(Line::Launch(failed));
        }
        Ok(Installation::NotInstalled) => return Ok(Line::Launch(asked.not_installed())),
        Ok(Installation::PathUnreadable | Installation::Installed(_)) => asked.installed(),
    };
    context.enter(Stage::LaunchRequest);
    let launched = client
        .launch_application(bundle, &[], relaunch, false, None, None, None)
        .await;
    Ok(Line::Launch(match launched {
        Ok(process) => asked.launched(process.pid),
        Err(error) => asked.failed(Problem::LaunchFailed, Some(watch.kind(&error))),
    }))
}

/// `stop`: the application's own processes the first look lists (its executable directly in
/// the bundle's directory, `matching.rs`) are each sent [`KILL`] through the application
/// service's `send_signal`, one after the other; its extensions' processes are sent nothing.
/// Then the list of processes is read at once, and again every [`LOOK_INTERVAL`] while it lists
/// a process the command signalled and the next look starts before the command's deadline
/// (`stopping.rs` decides; a look the deadline cuts leaves the verdict of the one before it).
/// The verdict comes from the looks whatever the signals answered: a signal's error is carried
/// beside it by its kind. An application that does not run, or is not installed, is answered
/// at the first look and sent nothing. A deadline that passes before the first look after the
/// signals has answered is the command's `deadline_passed`, as for every command.
async fn stop(target: &Target, bundle: &str, context: &Context) -> Result<Line, Failure> {
    let AppConnection {
        device: number,
        mut tunnel,
        port,
        listing,
        client: service,
        watch,
    } = app_service(target, context).await?;
    let asked = Stop::asked(number, bundle);
    let mut client = match service {
        Ok(client) => client,
        Err(kind) => {
            let failed = asked.failed(Problem::ServiceHandshakeFailed, Some(kind));
            return Ok(Line::Stop(failed));
        }
    };
    context.enter(Stage::ApplicationList);
    let listed = installation(&mut client, &watch, &mut tunnel, port, listing, bundle).await;
    let directories = match listed {
        Err(kind) => {
            let failed = asked.failed(Problem::ApplicationListFailed, Some(kind));
            return Ok(Line::Stop(failed));
        }
        Ok(Installation::NotInstalled) => {
            return Ok(Line::Stop(asked.nothing_ran(false, Vec::new())));
        }
        Ok(Installation::PathUnreadable) => {
            let failed = asked
                .installed()
                .failed(Problem::BundlePathUnreadable, None);
            return Ok(Line::Stop(failed));
        }
        Ok(Installation::Installed(directories)) => directories,
    };
    context.enter(Stage::ProcessList);
    let first = match pids_of(&mut client, &watch, &directories).await {
        Ok(pids) => pids,
        Err(kind) => {
            let failed = asked
                .installed()
                .failed(Problem::ProcessListFailed, Some(kind));
            return Ok(Line::Stop(failed));
        }
    };
    if first.application.is_empty() {
        return Ok(Line::Stop(asked.nothing_ran(true, first.extensions)));
    }
    context.enter(Stage::KillSignals);
    let mut errors = Vec::new();
    for &pid in &first.application {
        if let Err(error) = client.send_signal(pid, KILL).await {
            errors.push(SignalError {
                pid,
                error_kind: watch.kind(&error),
            });
        }
    }
    let signals = Signals {
        signalled: first.application,
        errors,
    };
    context.enter(Stage::LooksAfterSignals);
    let mut last = match pids_of(&mut client, &watch, &directories).await {
        Ok(pids) => pids,
        Err(kind) => return Ok(Line::Stop(asked.look_failed(signals, kind))),
    };
    let verdict = loop {
        let time_left = context
            .deadline
            .map(|deadline| deadline.saturating_duration_since(tokio::time::Instant::now()));
        if let Next::Done(verdict) =
            stopping::after_look(&signals.signalled, &last.application, time_left)
        {
            break verdict;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
        match look_within(&mut client, &watch, &directories, context.deadline).await {
            Some(Ok(pids)) => last = pids,
            Some(Err(kind)) => return Ok(Line::Stop(asked.look_failed(signals, kind))),
            // The deadline passed during the look: the last look inside it is `last`.
            None => break stopping::verdict(&signals.signalled, &last.application),
        }
    };
    Ok(Line::Stop(asked.looked(signals, last, verdict)))
}

/// A look at the processes of `directories` that ends by `deadline`: `None` when the deadline
/// passes before the service answers it.
async fn look_within(
    client: &mut AppService,
    watch: &Watched,
    directories: &[String],
    deadline: Option<tokio::time::Instant>,
) -> Option<Result<Pids, ErrorKind>> {
    match deadline {
        Some(deadline) => tokio::time::timeout_at(deadline, pids_of(client, watch, directories))
            .await
            .ok(),
        None => Some(pids_of(client, watch, directories).await),
    }
}

/// The device `target` names and a connection to its application service
/// (`com.apple.coredevice.appservice`) inside the tunnel of iOS 17.4 and later, with the tunnel,
/// which stays open while it is held, the service's port and the way its list of applications is
/// asked for ([`Listing`], from the features the handshake names for it). A step before the
/// service's own handshake fails as every command's does (its problem word, the library's text
/// cut as `redact.rs` cuts it); the service's handshake failing is its [`ErrorKind`] alone.
/// (ACIB-3b) Each step enters its stage first: `session`, `proxy_tunnel`, `discovery_handshake`,
/// `service_connection`, `service_xpc_handshake`.
async fn app_service(target: &Target, context: &Context) -> Result<AppConnection, Failure> {
    context.enter(Stage::Session);
    let device = locate(target, context).await?;
    let record = paired_record(&device).await?;
    let proxy = proxy_service(&device, &record).await?;
    context.enter(Stage::ProxyTunnel);
    let (mut tunnel, handshake) =
        open_tunnel(proxy, || context.enter(Stage::DiscoveryHandshake)).await?;
    context.enter(Stage::ServiceConnection);
    let name: &str = &<AppService as RsdService>::rsd_service_name();
    let port = port_of(&handshake, name)?;
    let listing = Listing::advertised(
        handshake
            .services
            .get(name)
            .and_then(|listed| listed.features.as_deref()),
    );
    let stream = tunnel
        .connect_to_service_port(port)
        .await
        .map_err(|error| Failure::library(Problem::ServiceUnreachable, &error))?;
    context.enter(Stage::ServiceXpcHandshake);
    // (ACIB-3b, third round) The client's connection is read through a tap, as `probe` reads its
    // own: the kinds of its errors tell a connection the phone ended apart (`Watched`).
    let watch = Watched::default();
    let client = <AppService as RsdService>::from_stream(watch.tapped(stream))
        .await
        .map_err(|error| watch.kind(&error));
    Ok(AppConnection {
        device: device.device_id,
        tunnel,
        port,
        listing,
        client,
        watch,
    })
}

/// What the application service's list of applications says of `bundle`. The list stays in
/// this function. The list is asked for by five flags, one per category of application: the
/// library's `list_apps` names them by the request's keys (`includeAppClips`,
/// `includeRemovableApps`, `includeHiddenApps`, `includeInternalApps`, `includeDefaultApps`) and
/// documents none of them, so what each category holds is read here from the key's name alone,
/// not from any description. Three are asked for (the coordinator's rulings on board #479): the
/// removable and the default ones, by their names the applications a person can remove, as one
/// they installed, and those the phone comes with; and the hidden ones, so that an application
/// a person installed and then hid is not left out should the phone list it only when they are
/// asked for. App Clips and internal applications are not asked for (ask the phone for less).
/// A bundle the list does not hold, for the categories asked, is [`Installation::NotInstalled`],
/// whether or not the phone has it under another category: `running` and `stop` answer it
/// `installed` false and `stop` sends it nothing, `launch` answers `not_installed` and sends
/// nothing. An entry the library cannot read fails the whole list (`answer_unreadable`): the
/// library's behaviour, and the same of an element of the streamed list without a bundle
/// identifier or a path. (ACIB-3b, second round) The list is asked the way `listing` names: the
/// library's `list_apps` on `client`, or the streamed list on a connection of its own to `port`
/// through `tunnel`, with the same flags. (ACIB-3b, third round) An error's kind is told by what
/// the phone sent on the connection it came on: `watch`, the client's, or the streamed list's own.
async fn installation(
    client: &mut AppService,
    watch: &Watched,
    tunnel: &mut AdapterHandle,
    port: u16,
    listing: Listing,
    bundle: &str,
) -> Result<Installation, ErrorKind> {
    let applications: Vec<ListedApplication> = match listing {
        // The library's order of the flags: App Clips, removable, hidden, internal, default.
        Listing::ListApps => client
            .list_apps(false, true, true, false, true)
            .await
            .map_err(|error| watch.kind(&error))?
            .into_iter()
            .map(|application| ListedApplication {
                bundle_identifier: application.bundle_identifier,
                path: application.path,
            })
            .collect(),
        Listing::StreamAppList => streamed_list(tunnel, port).await?,
    };
    Ok(matching::installation(
        bundle,
        applications.iter().map(|application| Application {
            bundle_identifier: &application.bundle_identifier,
            path: &application.path,
        }),
    ))
}

/// The streamed list of applications on a connection of its own to the application service's
/// `port` through `tunnel`: the connection, the library's RemoteXPC handshake on it, then
/// [`streamed_applications`]. Each failure is its [`ErrorKind`] alone, (ACIB-3b, third round)
/// told by what the phone sent on this connection ([`Watched`]).
async fn streamed_list(
    tunnel: &mut AdapterHandle,
    port: u16,
) -> Result<Vec<ListedApplication>, ErrorKind> {
    let watch = Watched::default();
    let stream = tunnel
        .connect_to_service_port(port)
        .await
        .map_err(|error| watch.kind(&error))?;
    let mut client = Xpc::new(watch.tapped(stream))
        .await
        .map_err(|error| watch.kind(&error))?;
    client
        .do_handshake()
        .await
        .map_err(|error| watch.kind(&error))?;
    streamed_applications(&mut client)
        .await
        .map_err(|kind| watch.after(kind))
}

/// The streamed list of applications, asked on `client` as the library's `stream_apps` asks it
/// (idevice 0.1.68, `invoke_streaming_with_plist` in `services/core_device/mod.rs` 109-159, with
/// the flags of `list_apps`: [`streamed_list_request`]), its answer read on the root channel as
/// the library reads it (`parse_stream_response`, 248-282): a message whose stream status names
/// `pushing` holds elements, one that names `finishStreaming` ends the list, one that names
/// `receivedError`, or that holds `CoreDevice.error` and no status, is the phone's refusal
/// (`device_refused`), and any other is `answer_unreadable`. Of each element only its bundle
/// identifier and its path are kept, in memory for the command; one without either is
/// `answer_unreadable`. The library's own `stream_apps` gives a `futures` stream, which this
/// program cannot read without a crate of its own: the request is made here and sent through the
/// library's RemoteXPC client.
async fn streamed_applications(client: &mut Xpc) -> Result<Vec<ListedApplication>, ErrorKind> {
    let request = streamed_list_request().ok_or(ErrorKind::Other)?;
    client
        .send_object(request, true)
        .await
        .map_err(|error| service_error_kind(&error))?;
    let mut listed = Vec::new();
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
            let text = |key: &str| {
                entry
                    .and_then(|entry| entry.get(key))
                    .and_then(|value| value.as_string())
                    .map(str::to_owned)
            };
            match (text("bundleIdentifier"), text("path")) {
                (Some(bundle_identifier), Some(path)) => listed.push(ListedApplication {
                    bundle_identifier,
                    path,
                }),
                _ => return Err(ErrorKind::AnswerUnreadable),
            }
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

/// The pids of the processes the application service lists whose executable lies inside one
/// of `directories`, the application's and its extensions' apart. The list stays in this
/// function. An error's kind is told by what the phone sent on the client's connection
/// (`watch`).
async fn pids_of(
    client: &mut AppService,
    watch: &Watched,
    directories: &[String],
) -> Result<Pids, ErrorKind> {
    let processes = client
        .list_processes()
        .await
        .map_err(|error| watch.kind(&error))?;
    Ok(matching::pids_inside(
        directories,
        processes.iter().map(|process| Process {
            pid: process.pid,
            executable: process
                .executable_url
                .as_ref()
                .map(|location| location.relative.as_str()),
        }),
    ))
}

/// The kind of an error of the application service: what the library's error tells apart, and
/// [`ErrorKind::Other`] for the rest. Its text is never read: the service's own refusal
/// (`CoreDevice(DeviceError)`) carries the debug text of the phone's error dictionary, which
/// can name other applications. The library maps no refusal to a locked phone, so there is no
/// kind for one.
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
                    .failed(Problem::ApplicationListFailed, Some(kind)),
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
                    signalled: vec![412],
                    errors: vec![SignalError {
                        pid: 412,
                        error_kind: kind,
                    }],
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

    /// (ACIB-3b, second round) The list is asked the way the service names among its features:
    /// streamed only when it names `streamapplist`. The streamed list's request is, member for
    /// member and in the library's order, the one the library's `stream_apps` sends
    /// (`services/core_device/mod.rs` 117-144) with the flags of `list_apps` as `running` asks
    /// them; its identifiers are random UUIDs made for it, each written as a UUID's text, the
    /// side channel an XPC UUID; and the request encodes.
    #[test]
    fn the_list_is_streamed_only_when_the_service_names_it_as_the_library_streams_it() {
        fn named(features: &[&str]) -> Vec<String> {
            features
                .iter()
                .map(|feature| (*feature).to_owned())
                .collect()
        }
        let nothing = named(&[]);
        let listed_only = named(&["com.apple.coredevice.feature.listapps"]);
        let both = named(&[
            "com.apple.coredevice.feature.listapps",
            STREAMED_LIST_FEATURE,
        ]);
        assert_eq!(Listing::advertised(None), Listing::ListApps);
        assert_eq!(
            Listing::advertised(Some(nothing.as_slice())),
            Listing::ListApps
        );
        assert_eq!(
            Listing::advertised(Some(listed_only.as_slice())),
            Listing::ListApps
        );
        assert_eq!(
            Listing::advertised(Some(both.as_slice())),
            Listing::StreamAppList
        );

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
