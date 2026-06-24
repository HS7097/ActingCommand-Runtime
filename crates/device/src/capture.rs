// SPDX-License-Identifier: AGPL-3.0-only

use crate::adb::{Adb, AdbConfig, stop_child};
use crate::{DeviceError, DeviceResult, DeviceTarget};
use image::{ColorType, ImageEncoder, codecs::png::PngEncoder};
use libloading::Library;
use std::ffi::CString;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant, SystemTime};

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const IHDR_LENGTH: [u8; 4] = [0, 0, 0, 13];
const DEFAULT_CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_DROIDCAST_LOCAL_PORT: u16 = 53516;
const DEFAULT_DROIDCAST_REMOTE_PATH: &str = "/data/local/tmp/DroidCast_raw.apk";
const DROIDCAST_MAIN_CLASS: &str = "ink.mol.droidcast_raw.Main";

/// Single-shot screenshot boundary for device capture backends.
pub trait CaptureBackend {
    fn capture(&mut self) -> DeviceResult<Frame>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Rgb8,
    Rgba8,
}

impl PixelFormat {
    fn color_type(self) -> ColorType {
        match self {
            Self::Rgb8 => ColorType::Rgb8,
            Self::Rgba8 => ColorType::Rgba8,
        }
    }

    fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgb8 => 3,
            Self::Rgba8 => 4,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rgb8 => "rgb8",
            Self::Rgba8 => "rgba8",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureBackendName {
    AdbScreencap,
    DroidcastRaw,
    NemuIpc,
}

impl CaptureBackendName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AdbScreencap => "adb_screencap",
            Self::DroidcastRaw => "droidcast_raw",
            Self::NemuIpc => "nemu_ipc",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CaptureBackendChoice {
    #[default]
    Auto,
    Adb,
    DroidcastRaw,
    NemuIpc,
}

impl CaptureBackendChoice {
    pub fn parse(value: &str) -> DeviceResult<Self> {
        match value {
            "auto" => Ok(Self::Auto),
            "adb" | "adb_screencap" | "screencap" => Ok(Self::Adb),
            "droidcast_raw" | "droidcast" => Ok(Self::DroidcastRaw),
            "nemu_ipc" | "nemu" => Ok(Self::NemuIpc),
            other => Err(DeviceError::fatal(format!(
                "unknown capture backend '{other}', expected auto, adb, droidcast_raw, or nemu_ipc"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Adb => "adb",
            Self::DroidcastRaw => "droidcast_raw",
            Self::NemuIpc => "nemu_ipc",
        }
    }
}

/// Device frame in a common pixel contract plus a PNG representation for artifacts.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub pixel_format: PixelFormat,
    pub png: Vec<u8>,
    pub captured_at: SystemTime,
    pub backend_name: CaptureBackendName,
}

impl Frame {
    pub fn from_png(png: Vec<u8>, backend_name: CaptureBackendName) -> DeviceResult<Self> {
        let (width, height) = parse_png_dimensions(&png)?;
        let image = image::load_from_memory(&png)
            .map_err(|err| DeviceError::fatal(format!("failed to decode PNG frame: {err}")))?
            .to_rgba8();
        Ok(Self {
            width,
            height,
            pixels: image.into_raw(),
            pixel_format: PixelFormat::Rgba8,
            png,
            captured_at: SystemTime::now(),
            backend_name,
        })
    }

    pub fn from_pixels(
        width: u32,
        height: u32,
        pixels: Vec<u8>,
        pixel_format: PixelFormat,
        backend_name: CaptureBackendName,
    ) -> DeviceResult<Self> {
        validate_pixel_buffer(width, height, pixel_format, pixels.len())?;
        let png = encode_png(width, height, &pixels, pixel_format)?;
        Ok(Self {
            width,
            height,
            pixels,
            pixel_format,
            png,
            captured_at: SystemTime::now(),
            backend_name,
        })
    }
}

#[derive(Debug, Clone)]
pub struct CaptureBackendConfig {
    pub adb_config: AdbConfig,
    pub target: DeviceTarget,
    pub requested: CaptureBackendChoice,
    pub capture_timeout: Duration,
    pub droidcast: DroidcastRawConfig,
    pub nemu: NemuIpcConfig,
}

impl CaptureBackendConfig {
    pub fn new(adb_config: AdbConfig, target: DeviceTarget) -> Self {
        Self {
            adb_config,
            target,
            requested: CaptureBackendChoice::Auto,
            capture_timeout: DEFAULT_CAPTURE_TIMEOUT,
            droidcast: DroidcastRawConfig::default(),
            nemu: NemuIpcConfig::default(),
        }
    }

    pub fn with_requested(mut self, requested: CaptureBackendChoice) -> Self {
        self.requested = requested;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureBackendAttempt {
    pub backend: CaptureBackendName,
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureBackendDiagnostics {
    pub requested: CaptureBackendChoice,
    pub used: CaptureBackendName,
    pub attempts: Vec<CaptureBackendAttempt>,
}

pub struct SelectedCaptureBackend {
    pub backend: Box<dyn CaptureBackend>,
    pub diagnostics: CaptureBackendDiagnostics,
}

pub fn create_capture_backend(
    config: CaptureBackendConfig,
) -> DeviceResult<SelectedCaptureBackend> {
    match config.requested {
        CaptureBackendChoice::Auto => create_auto_capture_backend(config),
        CaptureBackendChoice::Adb => {
            let used = CaptureBackendName::AdbScreencap;
            Ok(SelectedCaptureBackend {
                backend: Box::new(
                    ScreencapBackend::new(config.adb_config, config.target)
                        .with_capture_timeout(config.capture_timeout),
                ),
                diagnostics: CaptureBackendDiagnostics {
                    requested: config.requested,
                    used,
                    attempts: vec![CaptureBackendAttempt {
                        backend: used,
                        ok: true,
                        message: "explicit backend selected".to_string(),
                    }],
                },
            })
        }
        CaptureBackendChoice::DroidcastRaw => {
            let backend = DroidcastRawBackend::new(
                config.adb_config,
                config.target,
                config.droidcast,
                config.capture_timeout,
            )?;
            Ok(selected_explicit(
                config.requested,
                CaptureBackendName::DroidcastRaw,
                Box::new(backend),
            ))
        }
        CaptureBackendChoice::NemuIpc => {
            let backend = NemuIpcBackend::new(config.target, config.nemu, config.capture_timeout)?;
            Ok(selected_explicit(
                config.requested,
                CaptureBackendName::NemuIpc,
                Box::new(backend),
            ))
        }
    }
}

fn selected_explicit(
    requested: CaptureBackendChoice,
    used: CaptureBackendName,
    backend: Box<dyn CaptureBackend>,
) -> SelectedCaptureBackend {
    SelectedCaptureBackend {
        backend,
        diagnostics: CaptureBackendDiagnostics {
            requested,
            used,
            attempts: vec![CaptureBackendAttempt {
                backend: used,
                ok: true,
                message: "explicit backend selected".to_string(),
            }],
        },
    }
}

fn create_auto_capture_backend(
    config: CaptureBackendConfig,
) -> DeviceResult<SelectedCaptureBackend> {
    let mut attempts = Vec::new();

    match NemuIpcBackend::new(
        config.target.clone(),
        config.nemu.clone(),
        config.capture_timeout,
    ) {
        Ok(backend) => {
            let used = CaptureBackendName::NemuIpc;
            attempts.push(CaptureBackendAttempt {
                backend: used,
                ok: true,
                message: "auto selected available Windows MuMu IPC backend".to_string(),
            });
            return Ok(SelectedCaptureBackend {
                backend: Box::new(backend),
                diagnostics: CaptureBackendDiagnostics {
                    requested: CaptureBackendChoice::Auto,
                    used,
                    attempts,
                },
            });
        }
        Err(err) => attempts.push(CaptureBackendAttempt {
            backend: CaptureBackendName::NemuIpc,
            ok: false,
            message: err.message().to_string(),
        }),
    }

    match DroidcastRawBackend::new(
        config.adb_config.clone(),
        config.target.clone(),
        config.droidcast,
        config.capture_timeout,
    ) {
        Ok(backend) => {
            let used = CaptureBackendName::DroidcastRaw;
            attempts.push(CaptureBackendAttempt {
                backend: used,
                ok: true,
                message: "auto selected available DroidCast_raw backend".to_string(),
            });
            return Ok(SelectedCaptureBackend {
                backend: Box::new(backend),
                diagnostics: CaptureBackendDiagnostics {
                    requested: CaptureBackendChoice::Auto,
                    used,
                    attempts,
                },
            });
        }
        Err(err) => attempts.push(CaptureBackendAttempt {
            backend: CaptureBackendName::DroidcastRaw,
            ok: false,
            message: err.message().to_string(),
        }),
    }

    let used = CaptureBackendName::AdbScreencap;
    attempts.push(CaptureBackendAttempt {
        backend: used,
        ok: true,
        message: "auto fell back to adb exec-out screencap".to_string(),
    });
    Ok(SelectedCaptureBackend {
        backend: Box::new(
            ScreencapBackend::new(config.adb_config, config.target)
                .with_capture_timeout(config.capture_timeout),
        ),
        diagnostics: CaptureBackendDiagnostics {
            requested: CaptureBackendChoice::Auto,
            used,
            attempts,
        },
    })
}

/// ADB `exec-out screencap -p` capture backend with no persistent session.
#[derive(Debug, Clone)]
pub struct ScreencapBackend {
    adb_config: AdbConfig,
    target: DeviceTarget,
    capture_timeout: Duration,
}

impl ScreencapBackend {
    pub fn new(adb_config: AdbConfig, target: DeviceTarget) -> Self {
        Self {
            adb_config,
            target,
            capture_timeout: DEFAULT_CAPTURE_TIMEOUT,
        }
    }

    pub fn with_capture_timeout(mut self, capture_timeout: Duration) -> Self {
        self.capture_timeout = capture_timeout;
        self
    }
}

impl CaptureBackend for ScreencapBackend {
    fn capture(&mut self) -> DeviceResult<Frame> {
        let serial = self.target.resolved_serial();
        let adb = Adb::new(self.adb_config.clone());
        verify_adb_device(&adb, &self.target, &serial)?;

        // `adb exec-out screencap -p` returns one binary PNG and has no long-lived session.
        let output = adb.screencap(&serial, self.capture_timeout)?;
        if output.stdout.is_empty() {
            return Err(DeviceError::fatal(
                "adb exec-out screencap -p returned empty stdout",
            ));
        }

        Frame::from_png(output.stdout, CaptureBackendName::AdbScreencap)
    }
}

#[derive(Debug, Clone)]
pub struct DroidcastRawConfig {
    pub local_apk: Option<PathBuf>,
    pub remote_apk: String,
    pub local_port: u16,
}

impl Default for DroidcastRawConfig {
    fn default() -> Self {
        Self {
            local_apk: std::env::var_os("ACTINGCOMMAND_DROIDCAST_RAW_APK").map(PathBuf::from),
            remote_apk: DEFAULT_DROIDCAST_REMOTE_PATH.to_string(),
            local_port: DEFAULT_DROIDCAST_LOCAL_PORT,
        }
    }
}

pub struct DroidcastRawBackend {
    adb_config: AdbConfig,
    target: DeviceTarget,
    config: DroidcastRawConfig,
    capture_timeout: Duration,
    serial: String,
    child: Option<Child>,
    started: bool,
}

impl DroidcastRawBackend {
    pub fn new(
        adb_config: AdbConfig,
        target: DeviceTarget,
        config: DroidcastRawConfig,
        capture_timeout: Duration,
    ) -> DeviceResult<Self> {
        let local_apk = config.local_apk.as_ref().ok_or_else(|| {
            DeviceError::fatal(
                "DroidCast_raw unavailable: ACTINGCOMMAND_DROIDCAST_RAW_APK is not set",
            )
        })?;
        require_file(local_apk, "DroidCast_raw APK")?;
        let serial = target.resolved_serial();
        Ok(Self {
            adb_config,
            target,
            config,
            capture_timeout,
            serial,
            child: None,
            started: false,
        })
    }

    fn start_if_needed(&mut self) -> DeviceResult<(u32, u32)> {
        let adb = Adb::new(self.adb_config.clone());
        verify_adb_device(&adb, &self.target, &self.serial)?;
        let (width, height) = parse_screen_size(&adb.screen_size(&self.serial)?)?;
        if self.started {
            return Ok((width, height));
        }

        let local_apk = self.config.local_apk.as_ref().ok_or_else(|| {
            DeviceError::fatal("DroidCast_raw local APK disappeared before start")
        })?;
        adb.push(
            &self.serial,
            &local_apk.to_string_lossy(),
            &self.config.remote_apk,
        )?;
        adb.forward(
            &self.serial,
            &format!("tcp:{}", self.config.local_port),
            &format!("tcp:{}", self.config.local_port),
        )?;
        let classpath = format!("CLASSPATH={}", self.config.remote_apk);
        let child = adb.shell_spawn(
            &self.serial,
            &[&classpath, "app_process", "/", DROIDCAST_MAIN_CLASS],
        )?;
        self.child = Some(child);
        wait_for_droidcast(self.config.local_port, self.capture_timeout)?;
        self.started = true;
        Ok((width, height))
    }
}

impl CaptureBackend for DroidcastRawBackend {
    fn capture(&mut self) -> DeviceResult<Frame> {
        let (width, height) = self.start_if_needed()?;
        let raw = http_get_bytes(
            self.config.local_port,
            "/screenshot",
            self.capture_timeout,
            true,
        )?;
        let pixels = rgb565_to_rgb8(&raw, width, height)?;
        Frame::from_pixels(
            width,
            height,
            pixels,
            PixelFormat::Rgb8,
            CaptureBackendName::DroidcastRaw,
        )
    }
}

impl Drop for DroidcastRawBackend {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = stop_child(&mut child, Duration::from_millis(500));
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct NemuIpcConfig {
    pub nemu_folder: Option<PathBuf>,
    pub dll_path: Option<PathBuf>,
    pub instance_id: Option<i32>,
    pub display_id: i32,
}

pub struct NemuIpcBackend {
    library: Library,
    nemu_folder: CString,
    instance_id: i32,
    display_id: i32,
    connect_id: i32,
    capture_timeout: Duration,
}

type NemuConnect = unsafe extern "C" fn(*const c_char, i32) -> i32;
type NemuDisconnect = unsafe extern "C" fn(i32) -> i32;
type NemuCaptureDisplay = unsafe extern "C" fn(i32, i32, i32, *mut i32, *mut i32, *mut u8) -> i32;

impl NemuIpcBackend {
    pub fn new(
        target: DeviceTarget,
        config: NemuIpcConfig,
        capture_timeout: Duration,
    ) -> DeviceResult<Self> {
        if std::env::consts::OS != "windows" {
            return Err(DeviceError::fatal(
                "Nemu IPC unavailable: host OS is not Windows",
            ));
        }
        let serial = target.resolved_serial();
        let instance_id = config
            .instance_id
            .or_else(|| serial_to_nemu_instance_id(&serial))
            .ok_or_else(|| {
                DeviceError::fatal(format!(
                    "Nemu IPC unavailable: cannot derive MuMu instance id from serial {serial}"
                ))
            })?;
        let (nemu_folder, dll_path) = resolve_nemu_paths(config.nemu_folder, config.dll_path)?;
        let nemu_folder =
            CString::new(nemu_folder.to_string_lossy().as_bytes()).map_err(|err| {
                DeviceError::fatal(format!(
                    "Nemu IPC folder contains an interior NUL byte: {err}"
                ))
            })?;
        let library = unsafe { Library::new(&dll_path) }.map_err(|err| {
            DeviceError::fatal(format!(
                "Nemu IPC unavailable: failed to load {}: {err}",
                dll_path.display()
            ))
        })?;
        Ok(Self {
            library,
            nemu_folder,
            instance_id,
            display_id: config.display_id,
            connect_id: 0,
            capture_timeout,
        })
    }

    fn connect(&mut self) -> DeviceResult<()> {
        if self.connect_id > 0 {
            return Ok(());
        }
        let connect = unsafe { self.symbol::<NemuConnect>(b"nemu_connect\0")? };
        let started = Instant::now();
        let connect_id = unsafe { connect(self.nemu_folder.as_ptr(), self.instance_id) };
        if started.elapsed() > self.capture_timeout {
            return Err(DeviceError::fatal(format!(
                "Nemu IPC connect exceeded capture timeout {:?}",
                self.capture_timeout
            )));
        }
        if connect_id == 0 {
            return Err(DeviceError::fatal(
                "Nemu IPC connect returned 0; check MuMu path and running instance",
            ));
        }
        self.connect_id = connect_id;
        Ok(())
    }

    unsafe fn symbol<T>(&self, name: &[u8]) -> DeviceResult<T>
    where
        T: Copy,
    {
        let symbol = unsafe { self.library.get::<T>(name) }.map_err(|err| {
            DeviceError::fatal(format!(
                "Nemu IPC DLL is missing symbol {}: {err}",
                String::from_utf8_lossy(name).trim_end_matches('\0')
            ))
        })?;
        Ok(*symbol)
    }

    fn resolution(&mut self) -> DeviceResult<(u32, u32)> {
        self.connect()?;
        let capture_display =
            unsafe { self.symbol::<NemuCaptureDisplay>(b"nemu_capture_display\0")? };
        let mut width = 0i32;
        let mut height = 0i32;
        let ret = unsafe {
            capture_display(
                self.connect_id,
                self.display_id,
                0,
                &mut width,
                &mut height,
                std::ptr::null_mut(),
            )
        };
        if ret > 0 {
            return Err(DeviceError::fatal(format!(
                "Nemu IPC resolution probe failed with code {ret}"
            )));
        }
        if width <= 0 || height <= 0 {
            return Err(DeviceError::fatal(format!(
                "Nemu IPC returned invalid resolution {width}x{height}"
            )));
        }
        Ok((width as u32, height as u32))
    }
}

impl CaptureBackend for NemuIpcBackend {
    fn capture(&mut self) -> DeviceResult<Frame> {
        let (width, height) = self.resolution()?;
        let capture_display =
            unsafe { self.symbol::<NemuCaptureDisplay>(b"nemu_capture_display\0")? };
        let pixel_len = checked_pixel_len(width, height, PixelFormat::Rgba8)?;
        let mut width_i32 = i32::try_from(width)
            .map_err(|_| DeviceError::fatal(format!("Nemu IPC width exceeds i32: {width}")))?;
        let mut height_i32 = i32::try_from(height)
            .map_err(|_| DeviceError::fatal(format!("Nemu IPC height exceeds i32: {height}")))?;
        let mut raw = vec![0u8; pixel_len];
        let length = i32::try_from(raw.len()).map_err(|_| {
            DeviceError::fatal(format!("Nemu IPC frame is too large: {} bytes", raw.len()))
        })?;
        let ret = unsafe {
            capture_display(
                self.connect_id,
                self.display_id,
                length,
                &mut width_i32,
                &mut height_i32,
                raw.as_mut_ptr(),
            )
        };
        if ret > 0 {
            return Err(DeviceError::fatal(format!(
                "Nemu IPC capture failed with code {ret}"
            )));
        }
        if width_i32 <= 0 || height_i32 <= 0 {
            return Err(DeviceError::fatal(format!(
                "Nemu IPC capture returned invalid resolution {width_i32}x{height_i32}"
            )));
        }
        let width = width_i32 as u32;
        let height = height_i32 as u32;
        validate_pixel_buffer(width, height, PixelFormat::Rgba8, raw.len())?;
        let pixels = bgra_bottom_up_to_rgba(&raw, width, height)?;
        Frame::from_pixels(
            width,
            height,
            pixels,
            PixelFormat::Rgba8,
            CaptureBackendName::NemuIpc,
        )
    }
}

impl Drop for NemuIpcBackend {
    fn drop(&mut self) {
        if self.connect_id <= 0 {
            return;
        }
        if let Ok(disconnect) = unsafe { self.symbol::<NemuDisconnect>(b"nemu_disconnect\0") } {
            let _ = unsafe { disconnect(self.connect_id) };
        }
        self.connect_id = 0;
    }
}

fn verify_adb_device(adb: &Adb, target: &DeviceTarget, serial: &str) -> DeviceResult<()> {
    if target.connect {
        adb.connect(serial)?;
    }
    let state = adb.get_state(serial)?;
    if state != "device" {
        return Err(DeviceError::fatal(format!(
            "target device {serial} is not in device state: {state:?}"
        )));
    }
    Ok(())
}

pub fn parse_png_dimensions(png: &[u8]) -> DeviceResult<(u32, u32)> {
    if png.len() < 24 {
        return Err(DeviceError::fatal(format!(
            "screencap output is too short to be a PNG header: {} bytes",
            png.len()
        )));
    }
    if &png[0..8] != PNG_SIGNATURE {
        return Err(DeviceError::fatal(
            "screencap output does not start with a PNG signature",
        ));
    }
    if png[8..12] != IHDR_LENGTH {
        return Err(DeviceError::fatal(
            "screencap PNG has invalid IHDR chunk length",
        ));
    }
    if &png[12..16] != b"IHDR" {
        return Err(DeviceError::fatal("screencap PNG is missing IHDR"));
    }

    let width = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
    let height = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
    if width == 0 || height == 0 {
        return Err(DeviceError::fatal(format!(
            "screencap PNG has invalid dimensions: {width}x{height}"
        )));
    }

    Ok((width, height))
}

fn encode_png(
    width: u32,
    height: u32,
    pixels: &[u8],
    pixel_format: PixelFormat,
) -> DeviceResult<Vec<u8>> {
    validate_pixel_buffer(width, height, pixel_format, pixels.len())?;
    let mut png = Vec::new();
    let encoder = PngEncoder::new(&mut png);
    encoder
        .write_image(pixels, width, height, pixel_format.color_type().into())
        .map_err(|err| DeviceError::fatal(format!("failed to encode frame PNG: {err}")))?;
    Ok(png)
}

fn validate_pixel_buffer(
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    len: usize,
) -> DeviceResult<()> {
    let expected = checked_pixel_len(width, height, pixel_format)?;
    if len != expected {
        return Err(DeviceError::fatal(format!(
            "frame pixel buffer length mismatch for {}x{} {}: got {}, expected {}",
            width,
            height,
            pixel_format.as_str(),
            len,
            expected
        )));
    }
    Ok(())
}

fn checked_pixel_len(width: u32, height: u32, pixel_format: PixelFormat) -> DeviceResult<usize> {
    let pixels = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or_else(|| {
            DeviceError::fatal(format!("frame dimensions overflow usize: {width}x{height}"))
        })?;
    pixels
        .checked_mul(pixel_format.bytes_per_pixel())
        .ok_or_else(|| {
            DeviceError::fatal(format!(
                "frame byte length overflows usize: {}x{} {}",
                width,
                height,
                pixel_format.as_str()
            ))
        })
}

fn require_file(path: &Path, label: &str) -> DeviceResult<()> {
    let metadata = fs::metadata(path).map_err(|err| {
        DeviceError::fatal(format!(
            "{label} path is not readable at {}: {err}",
            path.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(DeviceError::fatal(format!(
            "{label} path is not a file: {}",
            path.display()
        )));
    }
    Ok(())
}

fn parse_screen_size(text: &str) -> DeviceResult<(u32, u32)> {
    let raw = text
        .split_whitespace()
        .find(|part| part.contains('x'))
        .ok_or_else(|| DeviceError::fatal(format!("failed to parse adb wm size output: {text}")))?;
    let (width, height) = raw.split_once('x').ok_or_else(|| {
        DeviceError::fatal(format!("failed to parse adb wm size dimensions: {text}"))
    })?;
    let width = width
        .parse::<u32>()
        .map_err(|err| DeviceError::fatal(format!("invalid adb wm width '{width}': {err}")))?;
    let height = height
        .parse::<u32>()
        .map_err(|err| DeviceError::fatal(format!("invalid adb wm height '{height}': {err}")))?;
    if width == 0 || height == 0 {
        return Err(DeviceError::fatal(format!(
            "adb wm size returned zero dimension: {width}x{height}"
        )));
    }
    Ok((width, height))
}

fn wait_for_droidcast(port: u16, timeout: Duration) -> DeviceResult<()> {
    let started = Instant::now();
    loop {
        match http_get_bytes(port, "/", Duration::from_millis(500), false) {
            Ok(_) => return Ok(()),
            Err(err) if started.elapsed() < timeout => {
                let _ = err;
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(err) => {
                return Err(DeviceError::fatal(format!(
                    "DroidCast_raw did not become available within {:?}: {}",
                    timeout, err
                )));
            }
        }
    }
}

fn http_get_bytes(
    port: u16,
    path: &str,
    timeout: Duration,
    require_success: bool,
) -> DeviceResult<Vec<u8>> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, timeout).map_err(|err| {
        DeviceError::fatal(format!("failed to connect DroidCast_raw at {addr}: {err}"))
    })?;
    stream.set_read_timeout(Some(timeout)).map_err(|err| {
        DeviceError::fatal(format!("failed to set DroidCast read timeout: {err}"))
    })?;
    stream.set_write_timeout(Some(timeout)).map_err(|err| {
        DeviceError::fatal(format!("failed to set DroidCast write timeout: {err}"))
    })?;
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|err| DeviceError::fatal(format!("failed to send DroidCast request: {err}")))?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|err| DeviceError::fatal(format!("failed to read DroidCast response: {err}")))?;
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| DeviceError::fatal("DroidCast response missing HTTP header terminator"))?;
    let header = String::from_utf8_lossy(&response[..header_end]);
    let status = header
        .lines()
        .next()
        .and_then(parse_http_status)
        .ok_or_else(|| {
            DeviceError::fatal(format!("DroidCast response has invalid status: {header}"))
        })?;
    if require_success && !(200..300).contains(&status) {
        return Err(DeviceError::fatal(format!(
            "DroidCast request {path} failed with HTTP {status}"
        )));
    }
    Ok(response[(header_end + 4)..].to_vec())
}

fn parse_http_status(line: &str) -> Option<u16> {
    line.split_whitespace().nth(1)?.parse().ok()
}

fn rgb565_to_rgb8(raw: &[u8], width: u32, height: u32) -> DeviceResult<Vec<u8>> {
    let pixel_count = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or_else(|| {
            DeviceError::fatal(format!(
                "DroidCast dimensions overflow usize: {width}x{height}"
            ))
        })?;
    let expected = pixel_count.checked_mul(2).ok_or_else(|| {
        DeviceError::fatal(format!(
            "DroidCast RGB565 byte length overflows: {width}x{height}"
        ))
    })?;
    if raw.len() != expected {
        return Err(DeviceError::fatal(format!(
            "DroidCast_raw returned {} bytes, expected {} for {}x{} RGB565",
            raw.len(),
            expected,
            width,
            height
        )));
    }
    let mut pixels = Vec::with_capacity(pixel_count * 3);
    for chunk in raw.chunks_exact(2) {
        let value = u16::from_le_bytes([chunk[0], chunk[1]]);
        let r = ((u32::from((value >> 11) & 0x1f) * 255) / 31) as u8;
        let g = ((u32::from((value >> 5) & 0x3f) * 255) / 63) as u8;
        let b = ((u32::from(value & 0x1f) * 255) / 31) as u8;
        pixels.extend_from_slice(&[r, g, b]);
    }
    Ok(pixels)
}

fn resolve_nemu_paths(
    folder: Option<PathBuf>,
    dll_path: Option<PathBuf>,
) -> DeviceResult<(PathBuf, PathBuf)> {
    if let Some(dll_path) =
        dll_path.or_else(|| std::env::var_os("ACTINGCOMMAND_NEMU_IPC_DLL").map(PathBuf::from))
    {
        require_file(&dll_path, "Nemu IPC DLL")?;
        let folder = folder
            .or_else(|| std::env::var_os("ACTINGCOMMAND_NEMU_FOLDER").map(PathBuf::from))
            .or_else(|| infer_nemu_folder_from_dll(&dll_path))
            .ok_or_else(|| {
                DeviceError::fatal(
                    "Nemu IPC unavailable: set ACTINGCOMMAND_NEMU_FOLDER with ACTINGCOMMAND_NEMU_IPC_DLL",
                )
            })?;
        return Ok((folder, dll_path));
    }

    let folders = folder
        .into_iter()
        .chain(std::env::var_os("ACTINGCOMMAND_NEMU_FOLDER").map(PathBuf::from))
        .chain(default_nemu_folders())
        .collect::<Vec<_>>();
    for folder in folders {
        for dll in nemu_dll_candidates(&folder) {
            if dll.is_file() {
                return Ok((folder, dll));
            }
        }
    }
    Err(DeviceError::fatal(
        "Nemu IPC unavailable: external_renderer_ipc.dll was not found; set ACTINGCOMMAND_NEMU_FOLDER or ACTINGCOMMAND_NEMU_IPC_DLL",
    ))
}

fn infer_nemu_folder_from_dll(dll: &Path) -> Option<PathBuf> {
    let mut path = dll.parent()?.to_path_buf();
    for _ in 0..3 {
        path = path.parent()?.to_path_buf();
    }
    Some(path)
}

fn default_nemu_folders() -> Vec<PathBuf> {
    ["ProgramFiles", "ProgramFiles(x86)"]
        .into_iter()
        .filter_map(std::env::var_os)
        .flat_map(|root| {
            let root = PathBuf::from(root);
            [
                root.join("Netease").join("MuMu Player 12"),
                root.join("Netease").join("MuMuPlayer-12.0"),
                root.join("MuMuPlayer-12.0"),
            ]
        })
        .collect()
}

fn nemu_dll_candidates(folder: &Path) -> Vec<PathBuf> {
    vec![
        folder
            .join("shell")
            .join("sdk")
            .join("external_renderer_ipc.dll"),
        folder
            .join("nx_device")
            .join("12.0")
            .join("shell")
            .join("sdk")
            .join("external_renderer_ipc.dll"),
    ]
}

fn serial_to_nemu_instance_id(serial: &str) -> Option<i32> {
    let port = serial.split(':').nth(1)?.parse::<i32>().ok()?;
    let base = port - 16384 + 16;
    let index = base.div_euclid(32);
    let offset = base.rem_euclid(32) - 16;
    if (0..32).contains(&index) && (-2..=2).contains(&offset) {
        Some(index)
    } else {
        None
    }
}

fn bgra_bottom_up_to_rgba(raw: &[u8], width: u32, height: u32) -> DeviceResult<Vec<u8>> {
    validate_pixel_buffer(width, height, PixelFormat::Rgba8, raw.len())?;
    let width = usize::try_from(width)
        .map_err(|_| DeviceError::fatal("Nemu IPC width does not fit usize"))?;
    let height = usize::try_from(height)
        .map_err(|_| DeviceError::fatal("Nemu IPC height does not fit usize"))?;
    let mut pixels = vec![0u8; raw.len()];
    for y in 0..height {
        for x in 0..width {
            let src = ((height - 1 - y) * width + x) * 4;
            let dst = (y * width + x) * 4;
            pixels[dst] = raw[src + 2];
            pixels[dst + 1] = raw[src + 1];
            pixels[dst + 2] = raw[src];
            pixels[dst + 3] = raw[src + 3];
        }
    }
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_png_dimensions_from_valid_header() {
        let png = png_header(1280, 720);
        assert_eq!(parse_png_dimensions(&png).expect("valid png"), (1280, 720));
    }

    #[test]
    fn rejects_empty_bytes() {
        assert_fatal(parse_png_dimensions(&[]));
    }

    #[test]
    fn rejects_non_png_signature() {
        let mut png = png_header(1280, 720);
        png[0] = 0;
        assert_fatal(parse_png_dimensions(&png));
    }

    #[test]
    fn rejects_missing_ihdr() {
        let mut png = png_header(1280, 720);
        png[12..16].copy_from_slice(b"TEXT");
        assert_fatal(parse_png_dimensions(&png));
    }

    #[test]
    fn rejects_invalid_ihdr_length() {
        let mut png = png_header(1280, 720);
        png[11] = 12;
        assert_fatal(parse_png_dimensions(&png));
    }

    #[test]
    fn rejects_zero_width() {
        let png = png_header(0, 720);
        assert_fatal(parse_png_dimensions(&png));
    }

    #[test]
    fn rejects_zero_height() {
        let png = png_header(1280, 0);
        assert_fatal(parse_png_dimensions(&png));
    }

    #[test]
    fn parses_capture_backend_choice_aliases() {
        assert_eq!(
            CaptureBackendChoice::parse("adb").expect("adb"),
            CaptureBackendChoice::Adb
        );
        assert_eq!(
            CaptureBackendChoice::parse("droidcast").expect("droidcast"),
            CaptureBackendChoice::DroidcastRaw
        );
        assert_eq!(
            CaptureBackendChoice::parse("nemu").expect("nemu"),
            CaptureBackendChoice::NemuIpc
        );
    }

    #[test]
    fn converts_rgb565_to_rgb8() {
        let raw = [0x00, 0xf8, 0xe0, 0x07, 0x1f, 0x00];
        let pixels = rgb565_to_rgb8(&raw, 3, 1).expect("rgb565");
        assert_eq!(pixels, vec![255, 0, 0, 0, 255, 0, 0, 0, 255]);
    }

    #[test]
    fn converts_nemu_bgra_bottom_up_to_rgba() {
        let raw = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let pixels = bgra_bottom_up_to_rgba(&raw, 2, 2).expect("bgra");
        assert_eq!(
            pixels,
            vec![11, 10, 9, 12, 15, 14, 13, 16, 3, 2, 1, 4, 7, 6, 5, 8,]
        );
    }

    #[test]
    fn parses_screen_size() {
        assert_eq!(
            parse_screen_size("Physical size: 1280x720").expect("screen size"),
            (1280, 720)
        );
    }

    #[test]
    fn maps_nemu_instance_id_from_serial() {
        assert_eq!(serial_to_nemu_instance_id("127.0.0.1:16384"), Some(0));
        assert_eq!(serial_to_nemu_instance_id("127.0.0.1:16416"), Some(1));
        assert_eq!(serial_to_nemu_instance_id("127.0.0.1:16448"), Some(2));
    }

    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut png = Vec::new();
        png.extend_from_slice(PNG_SIGNATURE);
        png.extend_from_slice(&IHDR_LENGTH);
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&width.to_be_bytes());
        png.extend_from_slice(&height.to_be_bytes());
        png
    }

    fn assert_fatal(result: DeviceResult<(u32, u32)>) {
        let err = result.expect_err("expected fatal device error");
        assert_eq!(err.severity(), crate::DeviceErrorSeverity::Fatal);
    }
}
