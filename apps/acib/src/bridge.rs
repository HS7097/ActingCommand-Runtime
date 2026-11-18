// SPDX-License-Identifier: AGPL-3.0-only

//! The six commands against the device, through the `idevice` library (board #479, ACIB-0 and
//! ACIB-1).
//!
//! Everything here only reads. usbmuxd is reached where the library finds it on this platform
//! (on Windows the TCP port 27015 of 127.0.0.1, which Apple's device service serves). The
//! pairing record usbmuxd holds is asked for, kept in memory for the command and never shown.
//! The tunnel of iOS 17.4 and later is the lockdown service CoreDeviceProxy with the library's
//! own TCP stack inside this process: no network adapter is created. No library type is ever
//! printed with its `Debug` form: only the values the answer types name leave this module. A
//! picture of the screen is written, as the service gave it, to the file `--out` names and is
//! kept nowhere else.

use crate::answer::{
    Apps, CommandName, DeviceEntry, Devices, Elapsed, Failure, ImageQuery, ImageQueryName, Images,
    Info, Line, Link, ListedImage, Mounted, Problem, Reading, Screenshot, ServiceEntry, Services,
    Way,
};
use crate::args::Request;
use crate::redact;
use idevice::pairing_file::PairingFile;
use idevice::provider::{IdeviceProvider, RsdProvider, UsbmuxdProvider};
use idevice::services::core_device::{ImageFormat, ScreenCaptureServiceClient};
use idevice::services::core_device_proxy::CoreDeviceProxy;
use idevice::services::dvt::remote_server::RemoteServerClient;
use idevice::services::dvt::screenshot::ScreenshotClient;
use idevice::services::installation_proxy::InstallationProxyClient;
use idevice::services::lockdown::LockdownClient;
use idevice::services::mobile_image_mounter::ImageMounter;
use idevice::services::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use idevice::usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdConnection, UsbmuxdDevice};
use idevice::{Idevice, IdeviceError, IdeviceService, ReadWrite, RsdService};
use std::borrow::Cow;
use std::io::Write;
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

/// Runs one request, of a command that started at `started`, and gives its line.
pub async fn answer(request: Request, started: Instant) -> Line {
    let (command, device, via) = (request.name(), request.device(), request.via());
    let answered = match request {
        Request::Devices => devices().await,
        Request::Info(number) => info(number).await,
        Request::Services(number) => services(number).await,
        Request::Images(number) => images(number).await,
        Request::Apps {
            device: number,
            contains,
            max_matching,
        } => apps(number, &contains, max_matching).await,
        Request::Screenshot {
            device: number,
            out,
            via: way,
        } => screenshot(number, &out, way, started).await,
    };
    answered.unwrap_or_else(|failure| Line::refused_on(Some(command), device, via, failure))
}

async fn devices() -> Result<Line, Failure> {
    let listed = list().await?;
    let devices: Vec<DeviceEntry> = listed
        .iter()
        .map(|device| DeviceEntry {
            device: device.device_id,
            connection: match device.connection_type {
                Connection::Usb => Link::Usb,
                Connection::Network(_) => Link::Network,
                _ => Link::Unknown,
            },
        })
        .collect();
    Ok(Line::Devices(Devices {
        command: CommandName::Devices,
        devices,
    }))
}

async fn info(number: u32) -> Result<Line, Failure> {
    let device = find(number).await?;
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

async fn services(number: u32) -> Result<Line, Failure> {
    let device = find(number).await?;
    let record = paired_record(&device).await?;
    let (_tunnel, handshake) = open_tunnel(&device, &record).await?;
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

async fn images(number: u32) -> Result<Line, Failure> {
    let device = find(number).await?;
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

async fn apps(number: u32, contains: &str, max_matching: usize) -> Result<Line, Failure> {
    let device = find(number).await?;
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

/// One picture of the screen, the way `way` takes it, written as the service gave it to `out`,
/// which must name nothing that exists: that is checked before the device is asked. A way the
/// device refuses writes no file.
async fn screenshot(number: u32, out: &str, way: Way, started: Instant) -> Result<Line, Failure> {
    match std::fs::symlink_metadata(out) {
        Ok(_) => return Err(Failure::own(Problem::OutExists)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(Failure::library(Problem::OutUnwritable, &error)),
    }
    let device = find(number).await?;
    let record = paired_record(&device).await?;
    let (mut tunnel, handshake) = open_tunnel(&device, &record).await?;
    let (picture, request) = match way {
        Way::ScreenCapture => screen_capture(&mut tunnel, &handshake).await?,
        Way::Instruments => instruments(&mut tunnel, &handshake).await?,
    };
    write_new(out, &picture)?;
    let elapsed = Elapsed::of(request, started.elapsed());
    Ok(Line::Screenshot(Screenshot::of(
        number, way, &picture, elapsed, out,
    )))
}

/// The picture of CoreDevice's screen capture service, asked for as PNG, of the main display;
/// and how long the request alone took.
async fn screen_capture(
    tunnel: &mut AdapterHandle,
    handshake: &RsdHandshake,
) -> Result<(Vec<u8>, Duration), Failure> {
    type Client = ScreenCaptureServiceClient<Box<dyn ReadWrite>>;
    let stream = remote_service(
        tunnel,
        handshake,
        &<Client as RsdService>::rsd_service_name(),
    )
    .await?;
    let mut client = <Client as RsdService>::from_stream(stream)
        .await
        .map_err(|error| Failure::library(Problem::ServiceHandshakeFailed, &error))?;
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
) -> Result<(Vec<u8>, Duration), Failure> {
    type Client = RemoteServerClient<Box<dyn ReadWrite>>;
    let stream = remote_service(
        tunnel,
        handshake,
        &<Client as RsdService>::rsd_service_name(),
    )
    .await?;
    let mut client = <Client as RsdService>::from_stream(stream)
        .await
        .map_err(|error| Failure::library(Problem::ServiceHandshakeFailed, &error))?;
    client
        .read_message(0)
        .await
        .map_err(|error| Failure::library(Problem::ServiceHandshakeFailed, &error))?;
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

/// Writes `picture` to a new file at `out`; a file that exists by then is not touched.
fn write_new(out: &str, picture: &[u8]) -> Result<(), Failure> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out)
        .map_err(|error| {
            let problem = if error.kind() == std::io::ErrorKind::AlreadyExists {
                Problem::OutExists
            } else {
                Problem::OutUnwritable
            };
            Failure::library(problem, &error)
        })?;
    file.write_all(picture)
        .and_then(|()| file.flush())
        .map_err(|error| Failure::library(Problem::OutUnwritable, &error))
}

/// The tunnel of iOS 17.4 and later (CoreDeviceProxy, with the library's own TCP stack inside
/// this process) and the remote service discovery handshake through it.
async fn open_tunnel(
    device: &UsbmuxdDevice,
    record: &PairingFile,
) -> Result<(AdapterHandle, RsdHandshake), Failure> {
    let provider = device.to_provider(UsbmuxdAddr::default(), LABEL);
    let connection = open_service(
        &provider,
        record,
        <CoreDeviceProxy as IdeviceService>::service_name(),
    )
    .await?;
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
