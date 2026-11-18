// SPDX-License-Identifier: AGPL-3.0-only

//! The ten commands against the device, through the `idevice` library (board #479, ACIB-0 to
//! ACIB-3).
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
    Orientations, Problem, Reader, Reading, Running, Screenshot, ServiceEntry, Services,
    SignalError, Signals, Stage, Stop, Way,
};
use crate::args::{KeyVariables, Out, Request, Target};
use crate::framing::Printed;
use crate::key::{DeviceKey, Salt};
use crate::matching::{self, Application, Installation, Pids, Process};
use crate::out_file::{self, NotWritten};
use crate::redact;
use crate::stopping::{self, LOOK_INTERVAL, Next};
use idevice::pairing_file::PairingFile;
use idevice::provider::{IdeviceProvider, RsdProvider, UsbmuxdProvider};
use idevice::services::core_device::{
    AppServiceClient, CoreDeviceError, ImageFormat, ScreenCaptureServiceClient,
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
use idevice::{Idevice, IdeviceError, IdeviceService, ReadWrite, RsdService};
use std::borrow::Cow;
use std::cell::Cell;
use std::path::Path;
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
/// A device's number, the tunnel to it, and its application service's client, or the kind of
/// the error the service's handshake failed with.
type AppConnection = (u32, AdapterHandle, Result<AppService, ErrorKind>);

/// What a command keeps while it runs, for the line of a deadline that passes in it: the
/// command's deadline, the stage a screenshot is in (no other command sets it) and the number of
/// the device once it is found (a device named by its key has none before).
pub struct Context {
    /// When the command's deadline passes (`None`: further than the clock reaches).
    pub deadline: Option<tokio::time::Instant>,
    pub stage: Cell<Option<Stage>>,
    pub device: Cell<Option<u32>>,
}

impl Context {
    /// The context of a command whose deadline passes at `deadline`.
    pub fn new(deadline: Option<tokio::time::Instant>) -> Self {
        Self {
            deadline,
            stage: Cell::new(None),
            device: Cell::new(None),
        }
    }
}

/// Runs one request, of a command that started at `started`, and gives what it prints.
pub async fn answer(request: Request, started: Instant, context: &Context) -> Printed {
    let (command, given, via) = (request.name(), request.device(), request.via());
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
        } => running(&target, &bundle, context).await.map(Printed::line),
        Request::Launch {
            device: target,
            bundle,
            relaunch,
        } => launch(&target, &bundle, relaunch, context)
            .await
            .map(Printed::line),
        Request::Stop {
            device: target,
            bundle,
        } => stop(&target, &bundle, context).await.map(Printed::line),
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
    let (_tunnel, handshake) = open_tunnel(proxy).await?;
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
    let (mut tunnel, handshake) = open_tunnel(proxy).await?;
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
            let (mut tunnel, handshake) = open_tunnel(proxy).await?;
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
    let (number, _tunnel, service) = app_service(target, context).await?;
    let asked = Running::asked(number, bundle);
    let mut client = match service {
        Ok(client) => client,
        Err(kind) => {
            let failed = asked.failed(Problem::ServiceHandshakeFailed, Some(kind));
            return Ok(Line::Running(failed));
        }
    };
    let answer = match installation(&mut client, bundle).await {
        Err(kind) => asked.failed(Problem::ApplicationListFailed, Some(kind)),
        Ok(Installation::NotInstalled) => asked.not_installed(),
        Ok(Installation::PathUnreadable) => asked
            .installed()
            .failed(Problem::BundlePathUnreadable, None),
        Ok(Installation::Installed(directories)) => {
            match pids_of(&mut client, &directories).await {
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
    let (number, _tunnel, service) = app_service(target, context).await?;
    let asked = Launch::asked(number, bundle, relaunch);
    let mut client = match service {
        Ok(client) => client,
        Err(kind) => {
            let failed = asked.failed(Problem::ServiceHandshakeFailed, Some(kind));
            return Ok(Line::Launch(failed));
        }
    };
    // Where the bundle lies is not needed to launch it: a path read as no place is installed.
    let asked = match installation(&mut client, bundle).await {
        Err(kind) => {
            let failed = asked.failed(Problem::ApplicationListFailed, Some(kind));
            return Ok(Line::Launch(failed));
        }
        Ok(Installation::NotInstalled) => return Ok(Line::Launch(asked.not_installed())),
        Ok(Installation::PathUnreadable | Installation::Installed(_)) => asked.installed(),
    };
    let launched = client
        .launch_application(bundle, &[], relaunch, false, None, None, None)
        .await;
    Ok(Line::Launch(match launched {
        Ok(process) => asked.launched(process.pid),
        Err(error) => asked.failed(Problem::LaunchFailed, Some(service_error_kind(&error))),
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
    let (number, _tunnel, service) = app_service(target, context).await?;
    let asked = Stop::asked(number, bundle);
    let mut client = match service {
        Ok(client) => client,
        Err(kind) => {
            let failed = asked.failed(Problem::ServiceHandshakeFailed, Some(kind));
            return Ok(Line::Stop(failed));
        }
    };
    let directories = match installation(&mut client, bundle).await {
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
    let first = match pids_of(&mut client, &directories).await {
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
    let mut errors = Vec::new();
    for &pid in &first.application {
        if let Err(error) = client.send_signal(pid, KILL).await {
            errors.push(SignalError {
                pid,
                error_kind: service_error_kind(&error),
            });
        }
    }
    let signals = Signals {
        signalled: first.application,
        errors,
    };
    let mut last = match pids_of(&mut client, &directories).await {
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
        match look_within(&mut client, &directories, context.deadline).await {
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
    directories: &[String],
    deadline: Option<tokio::time::Instant>,
) -> Option<Result<Pids, ErrorKind>> {
    match deadline {
        Some(deadline) => tokio::time::timeout_at(deadline, pids_of(client, directories))
            .await
            .ok(),
        None => Some(pids_of(client, directories).await),
    }
}

/// The device `target` names and a connection to its application service
/// (`com.apple.coredevice.appservice`) inside the tunnel of iOS 17.4 and later, with the tunnel,
/// which stays open while it is held. A step before the service's own handshake fails as every
/// command's does (its problem word, the library's text cut as `redact.rs` cuts it); the
/// service's handshake failing is its [`ErrorKind`] alone.
async fn app_service(target: &Target, context: &Context) -> Result<AppConnection, Failure> {
    let device = locate(target, context).await?;
    let record = paired_record(&device).await?;
    let proxy = proxy_service(&device, &record).await?;
    let (mut tunnel, handshake) = open_tunnel(proxy).await?;
    let stream = remote_service(
        &mut tunnel,
        &handshake,
        &<AppService as RsdService>::rsd_service_name(),
    )
    .await?;
    let client = <AppService as RsdService>::from_stream(stream)
        .await
        .map_err(|error| service_error_kind(&error));
    Ok((device.device_id, tunnel, client))
}

/// What the application service's list of applications says of `bundle`. The list stays in
/// this function. Of the five kinds the service lists (the library's `list_apps` names them by
/// the request's keys alone: `includeAppClips`, `includeRemovableApps`, `includeHiddenApps`,
/// `includeInternalApps`, `includeDefaultApps`) only two are asked for: the removable ones, the
/// kind an application a person installs is in, and the default ones, the system's own
/// applications that come with the phone (the coordinator's ruling on board #479: ask the phone
/// for less). An entry the library cannot read fails the whole list (`answer_unreadable`): the
/// library's behaviour, which fewer kinds make less likely.
async fn installation(client: &mut AppService, bundle: &str) -> Result<Installation, ErrorKind> {
    let applications = client
        .list_apps(false, true, false, false, true)
        .await
        .map_err(|error| service_error_kind(&error))?;
    Ok(matching::installation(
        bundle,
        applications.iter().map(|application| Application {
            bundle_identifier: &application.bundle_identifier,
            path: &application.path,
        }),
    ))
}

/// The pids of the processes the application service lists whose executable lies inside one
/// of `directories`, the application's and its extensions' apart. The list stays in this
/// function.
async fn pids_of(client: &mut AppService, directories: &[String]) -> Result<Pids, ErrorKind> {
    let processes = client
        .list_processes()
        .await
        .map_err(|error| service_error_kind(&error))?;
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
/// through it.
async fn open_tunnel(connection: Idevice) -> Result<(AdapterHandle, RsdHandshake), Failure> {
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
    let handshake = RsdHandshake::new(stream)
        .await
        .map_err(|error| Failure::library(Problem::RsdHandshakeFailed, &error))?;
    Ok((tunnel, handshake))
}

/// A connection, inside the tunnel, to the remote service the handshake lists as `name`.
async fn remote_service(
    tunnel: &mut AdapterHandle,
    handshake: &RsdHandshake,
    name: &str,
) -> Result<Box<dyn ReadWrite>, Failure> {
    let port = handshake
        .services
        .get(name)
        .map(|service| service.port)
        .ok_or(Failure::own(Problem::ServiceNotListed))?;
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
}
