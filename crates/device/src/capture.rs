// SPDX-License-Identifier: AGPL-3.0-only

use crate::adb::{Adb, AdbConfig, stop_child};
use crate::vendor_stdio::{VendorStdioCapture, VendorStdioSession};
use crate::{DeviceError, DeviceResult, DeviceTarget};
use image::{
    ColorType, ImageEncoder,
    codecs::png::{CompressionType, FilterType, PngEncoder},
};
use libloading::Library;
use std::collections::HashMap;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::{Mutex, OnceLock, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const IHDR_LENGTH: [u8; 4] = [0, 0, 0, 13];
const DEFAULT_CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_DROIDCAST_LOCAL_PORT: u16 = 53516;
const DEFAULT_DROIDCAST_REMOTE_PATH: &str = "/data/local/tmp/DroidCast_raw.apk";
const DROIDCAST_MAIN_CLASS: &str = "ink.mol.droidcast_raw.Main";
const DROIDCAST_MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const DROIDCAST_READ_CHUNK_BYTES: usize = 16 * 1024;
const DEFAULT_CAPTURE_PROBE_CACHE_TTL: Duration = Duration::from_secs(30);

/// Single-shot screenshot boundary for device capture backends.
pub trait CaptureBackend {
    fn capture(&mut self) -> DeviceResult<Frame>;

    fn vendor_stdio(&self) -> &[VendorStdioCapture] {
        &[]
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptureBackendName {
    AdbScreencap,
    AdbScreencapEncode,
    AdbScreencapRawGzip,
    DroidcastRaw,
    NemuIpc,
}

impl CaptureBackendName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AdbScreencap => "adb_screencap",
            Self::AdbScreencapEncode => "adb_screencap_encode",
            Self::AdbScreencapRawGzip => "adb_screencap_raw_gzip",
            Self::DroidcastRaw => "droidcast_raw",
            Self::NemuIpc => "nemu_ipc",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CaptureBackendChoice {
    #[default]
    Auto,
    AutoFastest,
    Adb,
    DroidcastRaw,
    NemuIpc,
}

impl CaptureBackendChoice {
    pub fn parse(value: &str) -> DeviceResult<Self> {
        match value {
            "auto" => Ok(Self::Auto),
            "auto-fastest" | "auto_fastest" => Ok(Self::AutoFastest),
            "adb" | "adb_screencap" | "screencap" => Ok(Self::Adb),
            "droidcast_raw" | "droidcast" => Ok(Self::DroidcastRaw),
            "nemu_ipc" | "nemu" => Ok(Self::NemuIpc),
            other => Err(DeviceError::fatal(format!(
                "unknown capture backend '{other}', expected auto, auto-fastest, adb, droidcast_raw, or nemu_ipc"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::AutoFastest => "auto-fastest",
            Self::Adb => "adb",
            Self::DroidcastRaw => "droidcast_raw",
            Self::NemuIpc => "nemu_ipc",
        }
    }
}

/// Device frame in a common raw-pixel contract.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub pixel_format: PixelFormat,
    pub original_png: Option<Vec<u8>>,
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
            original_png: Some(png),
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
        Ok(Self {
            width,
            height,
            pixels,
            pixel_format,
            original_png: None,
            captured_at: SystemTime::now(),
            backend_name,
        })
    }

    pub fn encode_png_fast(&self) -> DeviceResult<Vec<u8>> {
        encode_png_fast(self.width, self.height, &self.pixels, self.pixel_format)
    }

    pub fn png_for_artifact(&self) -> DeviceResult<Vec<u8>> {
        match &self.original_png {
            Some(png) => Ok(png.clone()),
            None => self.encode_png_fast(),
        }
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
    pub elapsed_ms: Option<u128>,
    pub cached: bool,
    pub channel_order_contract: &'static str,
    pub vendor_stdio: Vec<VendorStdioCapture>,
}

impl CaptureBackendAttempt {
    fn success(
        backend: CaptureBackendName,
        message: String,
        elapsed_ms: Option<u128>,
        cached: bool,
    ) -> Self {
        Self {
            backend,
            ok: true,
            message,
            elapsed_ms,
            cached,
            channel_order_contract: channel_order_contract_for(backend),
            vendor_stdio: Vec::new(),
        }
    }

    fn failure(
        backend: CaptureBackendName,
        message: String,
        elapsed_ms: Option<u128>,
        cached: bool,
    ) -> Self {
        Self {
            backend,
            ok: false,
            message,
            elapsed_ms,
            cached,
            channel_order_contract: channel_order_contract_for(backend),
            vendor_stdio: Vec::new(),
        }
    }

    fn with_vendor_stdio(mut self, vendor_stdio: Vec<VendorStdioCapture>) -> Self {
        self.vendor_stdio = vendor_stdio;
        self
    }
}

fn channel_order_contract_for(backend: CaptureBackendName) -> &'static str {
    match backend {
        CaptureBackendName::NemuIpc => "mumu_nemu_verified",
        _ => "verified",
    }
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
        CaptureBackendChoice::AutoFastest => create_auto_fastest_capture_backend(config),
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
                    attempts: vec![CaptureBackendAttempt::success(
                        used,
                        "explicit backend selected".to_string(),
                        None,
                        false,
                    )],
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
            attempts: vec![CaptureBackendAttempt::success(
                used,
                "explicit backend selected".to_string(),
                None,
                false,
            )],
        },
    }
}

const AUTO_CAPTURE_BACKEND_ORDER: [CaptureBackendName; 3] = [
    CaptureBackendName::NemuIpc,
    CaptureBackendName::DroidcastRaw,
    CaptureBackendName::AdbScreencap,
];

fn create_auto_capture_backend(
    config: CaptureBackendConfig,
) -> DeviceResult<SelectedCaptureBackend> {
    create_auto_capture_backend_with_mode(config, AutoCaptureMode::Priority)
}

fn create_auto_fastest_capture_backend(
    config: CaptureBackendConfig,
) -> DeviceResult<SelectedCaptureBackend> {
    create_auto_capture_backend_with_mode(config, AutoCaptureMode::Fastest)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutoCaptureMode {
    Priority,
    Fastest,
}

fn create_auto_capture_backend_with_mode(
    config: CaptureBackendConfig,
    mode: AutoCaptureMode,
) -> DeviceResult<SelectedCaptureBackend> {
    select_auto_capture_backend_with_probe(mode, AUTO_CAPTURE_BACKEND_ORDER, |name| {
        probe_or_cached_capture_backend(&config, name)
    })
}

fn select_auto_capture_backend_with_probe<I, F>(
    mode: AutoCaptureMode,
    candidates: I,
    mut probe: F,
) -> DeviceResult<SelectedCaptureBackend>
where
    I: IntoIterator<Item = CaptureBackendName>,
    F: FnMut(CaptureBackendName) -> DeviceResult<CaptureProbeOutcome>,
{
    let mut attempts = Vec::new();
    let requested = match mode {
        AutoCaptureMode::Priority => CaptureBackendChoice::Auto,
        AutoCaptureMode::Fastest => CaptureBackendChoice::AutoFastest,
    };
    let mut successful = Vec::new();

    for name in candidates {
        match probe(name)? {
            CaptureProbeOutcome::Available(backend, attempt, elapsed_ms) => {
                attempts.push(attempt);
                if mode == AutoCaptureMode::Priority {
                    return Ok(SelectedCaptureBackend {
                        backend,
                        diagnostics: CaptureBackendDiagnostics {
                            requested,
                            used: name,
                            attempts,
                        },
                    });
                }
                successful.push((name, elapsed_ms, backend));
            }
            CaptureProbeOutcome::Unavailable(attempt) => attempts.push(attempt),
        }
    }

    if mode == AutoCaptureMode::Fastest
        && let Some((used, _elapsed_ms, backend)) = successful
            .into_iter()
            .min_by_key(|(_name, elapsed_ms, _backend)| *elapsed_ms)
    {
        return Ok(SelectedCaptureBackend {
            backend,
            diagnostics: CaptureBackendDiagnostics {
                requested,
                used,
                attempts,
            },
        });
    }

    Err(DeviceError::fatal(format!(
        "{} capture backend selection failed; attempts: {}",
        requested.as_str(),
        format_backend_attempts(&attempts)
    )))
}

fn probe_or_cached_capture_backend(
    config: &CaptureBackendConfig,
    name: CaptureBackendName,
) -> DeviceResult<CaptureProbeOutcome> {
    let key = CaptureProbeCacheKey::new(config, name);
    if let Some(cached) = capture_probe_cache_lookup(&key, DEFAULT_CAPTURE_PROBE_CACHE_TTL)? {
        if !cached.ok {
            return Ok(CaptureProbeOutcome::Unavailable(cached.to_attempt(name)));
        }
        match build_capture_backend(config, name) {
            Ok(backend) => {
                return Ok(CaptureProbeOutcome::Available(
                    backend,
                    cached.to_attempt(name),
                    cached.elapsed_ms,
                ));
            }
            Err(err) => {
                let attempt =
                    CaptureBackendAttempt::failure(name, err.message().to_string(), None, false);
                capture_probe_cache_store(key, &attempt)?;
                return Ok(CaptureProbeOutcome::Unavailable(attempt));
            }
        }
    }

    let started = Instant::now();
    match build_capture_backend(config, name) {
        Ok(backend) => match prime_capture_backend(name, backend) {
            Ok((backend, message, vendor_stdio)) => {
                let elapsed_ms = started.elapsed().as_millis();
                let attempt =
                    CaptureBackendAttempt::success(name, message, Some(elapsed_ms), false)
                        .with_vendor_stdio(vendor_stdio);
                capture_probe_cache_store(key, &attempt)?;
                Ok(CaptureProbeOutcome::Available(backend, attempt, elapsed_ms))
            }
            Err(message) => {
                let elapsed_ms = started.elapsed().as_millis();
                let attempt =
                    CaptureBackendAttempt::failure(name, message, Some(elapsed_ms), false);
                capture_probe_cache_store(key, &attempt)?;
                Ok(CaptureProbeOutcome::Unavailable(attempt))
            }
        },
        Err(err) => {
            let elapsed_ms = started.elapsed().as_millis();
            let attempt = CaptureBackendAttempt::failure(
                name,
                err.message().to_string(),
                Some(elapsed_ms),
                false,
            );
            capture_probe_cache_store(key, &attempt)?;
            Ok(CaptureProbeOutcome::Unavailable(attempt))
        }
    }
}

enum CaptureProbeOutcome {
    Available(Box<dyn CaptureBackend>, CaptureBackendAttempt, u128),
    Unavailable(CaptureBackendAttempt),
}

fn build_capture_backend(
    config: &CaptureBackendConfig,
    name: CaptureBackendName,
) -> DeviceResult<Box<dyn CaptureBackend>> {
    match name {
        CaptureBackendName::NemuIpc => Ok(Box::new(NemuIpcBackend::new(
            config.target.clone(),
            config.nemu.clone(),
            config.capture_timeout,
        )?)),
        CaptureBackendName::DroidcastRaw => Ok(Box::new(DroidcastRawBackend::new(
            config.adb_config.clone(),
            config.target.clone(),
            config.droidcast.clone(),
            config.capture_timeout,
        )?)),
        CaptureBackendName::AdbScreencap => Ok(Box::new(
            ScreencapBackend::new(config.adb_config.clone(), config.target.clone())
                .with_capture_timeout(config.capture_timeout),
        )),
        CaptureBackendName::AdbScreencapEncode | CaptureBackendName::AdbScreencapRawGzip => {
            Err(DeviceError::fatal(format!(
                "{} is a reserved ADB capture mode and is not implemented in this milestone",
                name.as_str()
            )))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CaptureProbeCacheKey {
    serial: String,
    adb_path: String,
    backend: CaptureBackendName,
}

impl CaptureProbeCacheKey {
    fn new(config: &CaptureBackendConfig, backend: CaptureBackendName) -> Self {
        Self {
            serial: config.target.resolved_serial(),
            adb_path: config.adb_config.adb_path.clone(),
            backend,
        }
    }
}

#[derive(Debug, Clone)]
struct CaptureProbeCacheEntry {
    ok: bool,
    message: String,
    elapsed_ms: u128,
    inserted_at: Instant,
}

impl CaptureProbeCacheEntry {
    fn to_attempt(&self, backend: CaptureBackendName) -> CaptureBackendAttempt {
        if self.ok {
            CaptureBackendAttempt::success(
                backend,
                format!("cached capture probe result: {}", self.message),
                Some(self.elapsed_ms),
                true,
            )
        } else {
            CaptureBackendAttempt::failure(
                backend,
                format!("cached capture probe result: {}", self.message),
                Some(self.elapsed_ms),
                true,
            )
        }
    }
}

static CAPTURE_PROBE_CACHE: OnceLock<Mutex<HashMap<CaptureProbeCacheKey, CaptureProbeCacheEntry>>> =
    OnceLock::new();

fn capture_probe_cache() -> &'static Mutex<HashMap<CaptureProbeCacheKey, CaptureProbeCacheEntry>> {
    CAPTURE_PROBE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn capture_probe_cache_lookup(
    key: &CaptureProbeCacheKey,
    ttl: Duration,
) -> DeviceResult<Option<CaptureProbeCacheEntry>> {
    let mut cache = capture_probe_cache()
        .lock()
        .map_err(|_| DeviceError::fatal("capture probe cache lock was poisoned"))?;
    let Some(entry) = cache.get(key) else {
        return Ok(None);
    };
    if entry.inserted_at.elapsed() > ttl {
        cache.remove(key);
        return Ok(None);
    }
    Ok(Some(entry.clone()))
}

fn capture_probe_cache_store(
    key: CaptureProbeCacheKey,
    attempt: &CaptureBackendAttempt,
) -> DeviceResult<()> {
    let elapsed_ms = attempt.elapsed_ms.unwrap_or(0);
    let mut cache = capture_probe_cache()
        .lock()
        .map_err(|_| DeviceError::fatal("capture probe cache lock was poisoned"))?;
    cache.insert(
        key,
        CaptureProbeCacheEntry {
            ok: attempt.ok,
            message: attempt.message.clone(),
            elapsed_ms,
            inserted_at: Instant::now(),
        },
    );
    Ok(())
}

type PrimedCaptureResult = (Box<dyn CaptureBackend>, String, Vec<VendorStdioCapture>);

struct PrimedCaptureBackend {
    inner: Box<dyn CaptureBackend>,
    primed: Option<Frame>,
}

impl CaptureBackend for PrimedCaptureBackend {
    fn capture(&mut self) -> DeviceResult<Frame> {
        if let Some(frame) = self.primed.take() {
            return Ok(frame);
        }
        self.inner.capture()
    }

    fn vendor_stdio(&self) -> &[VendorStdioCapture] {
        self.inner.vendor_stdio()
    }
}

fn prime_capture_backend(
    name: CaptureBackendName,
    mut backend: Box<dyn CaptureBackend>,
) -> Result<PrimedCaptureResult, String> {
    match backend.capture() {
        Ok(frame) => {
            let vendor_stdio = backend.vendor_stdio().to_vec();
            let message = format!(
                "auto selected available {} backend after probe capture {}x{}",
                name.as_str(),
                frame.width,
                frame.height
            );
            Ok((
                Box::new(PrimedCaptureBackend {
                    inner: backend,
                    primed: Some(frame),
                }),
                message,
                vendor_stdio,
            ))
        }
        Err(err) => Err(err.message().to_string()),
    }
}

fn format_backend_attempts(attempts: &[CaptureBackendAttempt]) -> String {
    attempts
        .iter()
        .map(|attempt| {
            format!(
                "{}={}:elapsed_ms={}:cached={}:channel_order_contract={}:{}",
                attempt.backend.as_str(),
                attempt.ok,
                attempt
                    .elapsed_ms
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                attempt.cached,
                attempt.channel_order_contract,
                attempt.message
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
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
        self.stop_child_if_present();

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
        if let Err(err) = wait_for_droidcast(self.config.local_port, self.capture_timeout) {
            self.stop_child_if_present();
            return Err(err);
        }
        self.started = true;
        Ok((width, height))
    }

    fn stop_child_if_present(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = stop_child(&mut child, Duration::from_millis(500));
        }
        self.started = false;
    }
}

impl CaptureBackend for DroidcastRawBackend {
    fn capture(&mut self) -> DeviceResult<Frame> {
        let (natural_width, natural_height) = self.start_if_needed()?;
        let rotation = read_device_rotation(&Adb::new(self.adb_config.clone()), &self.serial)?;
        let (display_width, display_height) =
            display_size_from_natural(natural_width, natural_height, rotation);
        let (request_width, request_height) =
            droidcast_request_size(natural_width, natural_height, rotation);
        let path = format!("/screenshot?width={request_width}&height={request_height}");
        let raw = http_get_bytes(self.config.local_port, &path, self.capture_timeout, true)?;
        let (decode_width, decode_height) =
            droidcast_decode_size(natural_width, natural_height, display_width, display_height);
        let pixels = rgb565_to_rgb8(&raw, decode_width, decode_height)?;
        let (frame_width, frame_height, pixels) = orient_rgb8_frame_to_display(
            pixels,
            decode_width,
            decode_height,
            display_width,
            display_height,
            rotation,
        )?;
        Frame::from_pixels(
            frame_width,
            frame_height,
            pixels,
            PixelFormat::Rgb8,
            CaptureBackendName::DroidcastRaw,
        )
    }
}

impl Drop for DroidcastRawBackend {
    fn drop(&mut self) {
        self.stop_child_if_present();
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
    worker: Option<NemuIpcWorker>,
    frame_width: u32,
    frame_height: u32,
    vendor_stdio: Vec<VendorStdioCapture>,
}

type NemuConnect = unsafe extern "C" fn(*const u16, i32) -> i32;
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
        let mut worker = NemuIpcWorker::spawn(
            nemu_folder,
            dll_path,
            instance_id,
            config.display_id,
            capture_timeout,
        );
        let (frame_width, frame_height) = worker.probe_resolution()?;
        Ok(Self {
            worker: Some(worker),
            frame_width,
            frame_height,
            vendor_stdio: Vec::new(),
        })
    }
}

enum NemuIpcCommand {
    Probe(mpsc::Sender<DeviceResult<(u32, u32)>>),
    Capture(mpsc::Sender<DeviceResult<NemuCapturedFrame>>),
    Shutdown(mpsc::Sender<()>),
}

struct NemuCapturedFrame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    vendor_stdio: Vec<VendorStdioCapture>,
}

struct NemuIpcWorker {
    tx: mpsc::Sender<NemuIpcCommand>,
    handle: Option<JoinHandle<()>>,
    timeout: Duration,
    poisoned: bool,
}

impl NemuIpcWorker {
    fn spawn(
        nemu_folder: PathBuf,
        dll_path: PathBuf,
        instance_id: i32,
        display_id: i32,
        timeout: Duration,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let mut state =
                NemuIpcWorkerState::load(nemu_folder, dll_path, instance_id, display_id);
            while let Ok(command) = rx.recv() {
                match command {
                    NemuIpcCommand::Probe(response) => {
                        let _ = response.send(worker_state_result(&mut state, |state| {
                            state.probe_resolution()
                        }));
                    }
                    NemuIpcCommand::Capture(response) => {
                        let _ = response.send(worker_state_result(&mut state, |state| {
                            state.capture_frame()
                        }));
                    }
                    NemuIpcCommand::Shutdown(response) => {
                        if let Ok(state) = state.as_mut() {
                            state.disconnect();
                        }
                        let _ = response.send(());
                        break;
                    }
                }
            }
        });

        Self {
            tx,
            handle: Some(handle),
            timeout,
            poisoned: false,
        }
    }

    fn probe_resolution(&mut self) -> DeviceResult<(u32, u32)> {
        self.request(NemuIpcCommand::Probe)
    }

    fn capture_frame(&mut self) -> DeviceResult<NemuCapturedFrame> {
        self.request(NemuIpcCommand::Capture)
    }

    fn request<T: Send + 'static>(
        &mut self,
        command: impl FnOnce(mpsc::Sender<DeviceResult<T>>) -> NemuIpcCommand,
    ) -> DeviceResult<T> {
        if self.poisoned {
            return Err(DeviceError::fatal(
                "Nemu IPC backend is poisoned after a previous timeout",
            ));
        }

        let (tx, rx) = mpsc::channel();
        self.tx.send(command(tx)).map_err(|err| {
            DeviceError::fatal(format!("failed to send Nemu IPC worker command: {err}"))
        })?;
        match rx.recv_timeout(self.timeout) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.poisoned = true;
                Err(DeviceError::fatal(format!(
                    "Nemu IPC worker timed out after {:?}; backend marked poisoned and will not be reused",
                    self.timeout
                )))
            }
            Err(err) => {
                self.poisoned = true;
                Err(DeviceError::fatal(format!(
                    "Nemu IPC worker disconnected: {err}"
                )))
            }
        }
    }

    fn shutdown(&mut self) {
        if self.poisoned {
            self.handle.take();
            return;
        }

        let (tx, rx) = mpsc::channel();
        if self.tx.send(NemuIpcCommand::Shutdown(tx)).is_err() {
            self.handle.take();
            return;
        }
        if rx.recv_timeout(self.timeout).is_err() {
            self.poisoned = true;
            self.handle.take();
            return;
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for NemuIpcWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct NemuIpcWorkerState {
    library: Library,
    stdio_session: VendorStdioSession,
    nemu_folder: Vec<u16>,
    instance_id: i32,
    display_id: i32,
    connect_id: i32,
    raw_buffer: Vec<u8>,
    frame_width: u32,
    frame_height: u32,
    vendor_stdio: Vec<VendorStdioCapture>,
}

impl NemuIpcWorkerState {
    fn load(
        nemu_folder: PathBuf,
        dll_path: PathBuf,
        instance_id: i32,
        display_id: i32,
    ) -> DeviceResult<Self> {
        let nemu_folder = nul_terminated_utf16_path(&nemu_folder)?;
        let mut stdio_session = VendorStdioSession::start()?;
        let library = unsafe { Library::new(&dll_path) }.map_err(|err| {
            DeviceError::fatal(format!(
                "Nemu IPC unavailable: failed to load {}: {err}",
                dll_path.display()
            ))
        })?;
        let mut vendor_stdio = Vec::new();
        let load_stdio = stdio_session.snapshot()?;
        if !load_stdio.is_empty() {
            vendor_stdio.push(load_stdio);
        }
        Ok(Self {
            library,
            stdio_session,
            nemu_folder,
            instance_id,
            display_id,
            connect_id: 0,
            raw_buffer: Vec::new(),
            frame_width: 0,
            frame_height: 0,
            vendor_stdio,
        })
    }

    fn connect(&mut self) -> DeviceResult<()> {
        if self.connect_id > 0 {
            return Ok(());
        }
        let connect = unsafe { self.symbol::<NemuConnect>(b"nemu_connect\0")? };
        let nemu_folder = self.nemu_folder.as_ptr();
        let instance_id = self.instance_id;
        let connect_id = unsafe { connect(nemu_folder, instance_id) };
        self.record_vendor_stdio_snapshot()?;
        if connect_id == 0 {
            return Err(DeviceError::fatal(
                "Nemu IPC connect returned 0; check MuMu path and running instance",
            ));
        }
        self.connect_id = connect_id;
        Ok(())
    }

    fn record_vendor_stdio(&mut self, capture: VendorStdioCapture) {
        if !capture.is_empty() {
            self.vendor_stdio.push(capture);
        }
    }

    fn record_vendor_stdio_snapshot(&mut self) -> DeviceResult<()> {
        let capture = self.stdio_session.snapshot()?;
        self.record_vendor_stdio(capture);
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

    fn probe_resolution(&mut self) -> DeviceResult<(u32, u32)> {
        self.connect()?;
        let capture_display =
            unsafe { self.symbol::<NemuCaptureDisplay>(b"nemu_capture_display\0")? };
        let mut width = 0i32;
        let mut height = 0i32;
        let connect_id = self.connect_id;
        let display_id = self.display_id;
        let width_ptr = &mut width as *mut i32;
        let height_ptr = &mut height as *mut i32;
        let ret = unsafe {
            capture_display(
                connect_id,
                display_id,
                0,
                width_ptr,
                height_ptr,
                std::ptr::null_mut(),
            )
        };
        self.record_vendor_stdio_snapshot()?;
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

    fn capture_frame(&mut self) -> DeviceResult<NemuCapturedFrame> {
        let (width, height) = self.probe_resolution()?;
        let pixel_len = checked_pixel_len(width, height, PixelFormat::Rgba8)?;
        if width != self.frame_width
            || height != self.frame_height
            || self.raw_buffer.len() != pixel_len
        {
            self.raw_buffer.resize(pixel_len, 0);
            self.frame_width = width;
            self.frame_height = height;
        }

        let capture_display =
            unsafe { self.symbol::<NemuCaptureDisplay>(b"nemu_capture_display\0")? };
        let mut width_i32 = i32::try_from(width)
            .map_err(|_| DeviceError::fatal(format!("Nemu IPC width exceeds i32: {width}")))?;
        let mut height_i32 = i32::try_from(height)
            .map_err(|_| DeviceError::fatal(format!("Nemu IPC height exceeds i32: {height}")))?;
        let length = i32::try_from(self.raw_buffer.len()).map_err(|_| {
            DeviceError::fatal(format!(
                "Nemu IPC frame is too large: {} bytes",
                self.raw_buffer.len()
            ))
        })?;
        let connect_id = self.connect_id;
        let display_id = self.display_id;
        let width_ptr = &mut width_i32 as *mut i32;
        let height_ptr = &mut height_i32 as *mut i32;
        let buffer_ptr = self.raw_buffer.as_mut_ptr();
        let ret = unsafe {
            capture_display(
                connect_id, display_id, length, width_ptr, height_ptr, buffer_ptr,
            )
        };
        self.record_vendor_stdio_snapshot()?;
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
        let captured_width = width_i32 as u32;
        let captured_height = height_i32 as u32;
        if captured_width != width || captured_height != height {
            return Err(DeviceError::fatal(format!(
                "Nemu IPC frame dimensions changed during capture from probed {width}x{height} to {captured_width}x{captured_height}"
            )));
        }
        let pixels = rgba_bottom_up_to_rgba(&self.raw_buffer, width, height)?;
        Ok(NemuCapturedFrame {
            width,
            height,
            pixels,
            vendor_stdio: self.vendor_stdio.clone(),
        })
    }

    fn disconnect(&mut self) {
        if self.connect_id <= 0 {
            return;
        }
        if let Ok(disconnect) = unsafe { self.symbol::<NemuDisconnect>(b"nemu_disconnect\0") } {
            let connect_id = self.connect_id;
            unsafe { disconnect(connect_id) };
            let _ = self.record_vendor_stdio_snapshot();
        }
        self.connect_id = 0;
    }
}

fn worker_state_result<T>(
    state: &mut DeviceResult<NemuIpcWorkerState>,
    operation: impl FnOnce(&mut NemuIpcWorkerState) -> DeviceResult<T>,
) -> DeviceResult<T> {
    match state {
        Ok(state) => operation(state),
        Err(err) => Err(err.clone()),
    }
}

impl CaptureBackend for NemuIpcBackend {
    fn capture(&mut self) -> DeviceResult<Frame> {
        let worker = self
            .worker
            .as_mut()
            .ok_or_else(|| DeviceError::fatal("Nemu IPC worker is unavailable"))?;
        let frame = worker.capture_frame()?;
        self.frame_width = frame.width;
        self.frame_height = frame.height;
        self.vendor_stdio = frame.vendor_stdio.clone();
        Frame::from_pixels(
            frame.width,
            frame.height,
            frame.pixels,
            PixelFormat::Rgba8,
            CaptureBackendName::NemuIpc,
        )
    }

    fn vendor_stdio(&self) -> &[VendorStdioCapture] {
        &self.vendor_stdio
    }
}

fn verify_adb_device(adb: &Adb, target: &DeviceTarget, serial: &str) -> DeviceResult<()> {
    adb.ensure_device(serial, target.connect)?;
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

pub fn encode_png_fast(
    width: u32,
    height: u32,
    pixels: &[u8],
    pixel_format: PixelFormat,
) -> DeviceResult<Vec<u8>> {
    validate_pixel_buffer(width, height, pixel_format, pixels.len())?;
    let mut png = Vec::new();
    let encoder =
        PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::NoFilter);
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

fn nul_terminated_utf16_path(path: &Path) -> DeviceResult<Vec<u16>> {
    let text = path.to_str().ok_or_else(|| {
        DeviceError::fatal(format!(
            "Nemu IPC folder path is not valid Unicode: {}",
            path.display()
        ))
    })?;
    let mut wide = Vec::new();
    for unit in text.encode_utf16() {
        if unit == 0 {
            return Err(DeviceError::fatal(format!(
                "Nemu IPC folder contains an interior NUL: {}",
                path.display()
            )));
        }
        wide.push(unit);
    }
    wide.push(0);
    Ok(wide)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceRotation {
    R0,
    R90,
    R180,
    R270,
}

fn read_device_rotation(adb: &Adb, serial: &str) -> DeviceResult<DeviceRotation> {
    let output = adb.run(&["-s", serial, "shell", "dumpsys", "display"])?;
    if let Some(rotation) = parse_display_orientation(&output.stdout)? {
        return Ok(rotation);
    }
    let output = adb.run(&[
        "-s",
        serial,
        "shell",
        "settings",
        "get",
        "system",
        "user_rotation",
    ])?;
    parse_device_rotation(&output.stdout)
}

fn parse_display_orientation(text: &str) -> DeviceResult<Option<DeviceRotation>> {
    for line in text.lines() {
        if let Some(index) = line.find("orientation=") {
            let rest = &line[index + "orientation=".len()..];
            let value = rest
                .chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect::<String>();
            if !value.is_empty() {
                return parse_device_rotation(&value).map(Some);
            }
        }
    }
    Ok(None)
}

fn parse_device_rotation(text: &str) -> DeviceResult<DeviceRotation> {
    match text.trim() {
        "0" => Ok(DeviceRotation::R0),
        "1" => Ok(DeviceRotation::R90),
        "2" => Ok(DeviceRotation::R180),
        "3" => Ok(DeviceRotation::R270),
        other => Err(DeviceError::fatal(format!(
            "failed to parse device user_rotation value: {other:?}"
        ))),
    }
}

fn droidcast_request_size(width: u32, height: u32, rotation: DeviceRotation) -> (u32, u32) {
    match rotation {
        DeviceRotation::R90 | DeviceRotation::R270 => (height, width),
        DeviceRotation::R0 | DeviceRotation::R180 => (width, height),
    }
}

fn display_size_from_natural(width: u32, height: u32, rotation: DeviceRotation) -> (u32, u32) {
    match rotation {
        DeviceRotation::R90 | DeviceRotation::R270 => (height, width),
        DeviceRotation::R0 | DeviceRotation::R180 => (width, height),
    }
}

fn droidcast_decode_size(
    natural_width: u32,
    natural_height: u32,
    display_width: u32,
    display_height: u32,
) -> (u32, u32) {
    if natural_width == display_height && natural_height == display_width {
        (natural_width, natural_height)
    } else {
        (display_width, display_height)
    }
}

fn orient_rgb8_frame_to_display(
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    display_width: u32,
    display_height: u32,
    rotation: DeviceRotation,
) -> DeviceResult<(u32, u32, Vec<u8>)> {
    if width == display_width && height == display_height {
        return Ok((width, height, pixels));
    }
    if width != display_height || height != display_width {
        return Err(DeviceError::fatal(format!(
            "DroidCast_raw frame dimensions {width}x{height} cannot be oriented to display {display_width}x{display_height}"
        )));
    }

    let rotated = match rotation {
        DeviceRotation::R270 => rotate_rgb8_counterclockwise(&pixels, width, height)?,
        DeviceRotation::R90 | DeviceRotation::R0 | DeviceRotation::R180 => {
            rotate_rgb8_clockwise(&pixels, width, height)?
        }
    };
    Ok((display_width, display_height, rotated))
}

fn rotate_rgb8_clockwise(pixels: &[u8], width: u32, height: u32) -> DeviceResult<Vec<u8>> {
    rotate_rgb8(pixels, width, height, |x, y, _width, height| {
        (height - 1 - y) + x * height
    })
}

fn rotate_rgb8_counterclockwise(pixels: &[u8], width: u32, height: u32) -> DeviceResult<Vec<u8>> {
    rotate_rgb8(pixels, width, height, |x, y, width, height| {
        y + (width - 1 - x) * height
    })
}

fn rotate_rgb8(
    pixels: &[u8],
    width: u32,
    height: u32,
    map_dest: impl Fn(usize, usize, usize, usize) -> usize,
) -> DeviceResult<Vec<u8>> {
    validate_pixel_buffer(width, height, PixelFormat::Rgb8, pixels.len())?;
    let width = usize::try_from(width)
        .map_err(|_| DeviceError::fatal("DroidCast width does not fit usize"))?;
    let height = usize::try_from(height)
        .map_err(|_| DeviceError::fatal("DroidCast height does not fit usize"))?;
    let mut output = vec![0u8; pixels.len()];
    for y in 0..height {
        for x in 0..width {
            let src = (y * width + x) * 3;
            let dst_index = map_dest(x, y, width, height);
            let dst = dst_index * 3;
            output[dst..dst + 3].copy_from_slice(&pixels[src..src + 3]);
        }
    }
    Ok(output)
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
    let response = read_droidcast_response(&mut stream, timeout)?;
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

fn read_droidcast_response(stream: &mut TcpStream, timeout: Duration) -> DeviceResult<Vec<u8>> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| DeviceError::fatal("DroidCast read timeout overflowed"))?;
    let mut response = Vec::new();
    let mut buffer = [0u8; DROIDCAST_READ_CHUNK_BYTES];
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(DeviceError::fatal(format!(
                "timed out after {:?} reading DroidCast response",
                timeout
            )));
        }
        let remaining = deadline.saturating_duration_since(now);
        stream
            .set_read_timeout(Some(remaining.min(Duration::from_millis(200))))
            .map_err(|err| {
                DeviceError::fatal(format!("failed to update DroidCast read timeout: {err}"))
            })?;
        match stream.read(&mut buffer) {
            Ok(0) => return Ok(response),
            Ok(read) => {
                let next_len = response.len().checked_add(read).ok_or_else(|| {
                    DeviceError::fatal("DroidCast response length overflowed usize")
                })?;
                if next_len > DROIDCAST_MAX_RESPONSE_BYTES {
                    return Err(DeviceError::fatal(format!(
                        "DroidCast response exceeded {} bytes",
                        DROIDCAST_MAX_RESPONSE_BYTES
                    )));
                }
                response.extend_from_slice(&buffer[..read]);
            }
            Err(err) if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(err) => {
                return Err(DeviceError::fatal(format!(
                    "failed to read DroidCast response: {err}"
                )));
            }
        }
    }
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

fn rgba_bottom_up_to_rgba(raw: &[u8], width: u32, height: u32) -> DeviceResult<Vec<u8>> {
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
            pixels[dst] = raw[src];
            pixels[dst + 1] = raw[src + 1];
            pixels[dst + 2] = raw[src + 2];
            pixels[dst + 3] = raw[src + 3];
        }
    }
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    struct FakeCaptureBackend {
        drops: Rc<Cell<usize>>,
    }

    impl CaptureBackend for FakeCaptureBackend {
        fn capture(&mut self) -> DeviceResult<Frame> {
            Err(DeviceError::fatal("fake capture must not run"))
        }
    }

    impl Drop for FakeCaptureBackend {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }

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
            CaptureBackendChoice::parse("auto-fastest").expect("auto-fastest"),
            CaptureBackendChoice::AutoFastest
        );
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
    fn capture_autotune_caches_probe() {
        let _guard = capture_probe_cache_test_guard();
        clear_capture_probe_cache_for_tests();

        let config = CaptureBackendConfig::new(
            AdbConfig {
                adb_path: String::new(),
                command_timeout: Duration::from_millis(1),
            },
            DeviceTarget::default(),
        )
        .with_requested(CaptureBackendChoice::Auto);
        let key = CaptureProbeCacheKey::new(&config, CaptureBackendName::AdbScreencap);
        capture_probe_cache_store(
            key,
            &CaptureBackendAttempt::success(
                CaptureBackendName::AdbScreencap,
                "cached ok".to_string(),
                Some(7),
                false,
            ),
        )
        .expect("store cache");

        let selected = create_capture_backend(config).expect("cached adb backend selected");

        assert_eq!(selected.diagnostics.used, CaptureBackendName::AdbScreencap);
        assert!(
            selected
                .diagnostics
                .attempts
                .iter()
                .any(
                    |attempt| attempt.backend == CaptureBackendName::AdbScreencap
                        && attempt.ok
                        && attempt.cached
                        && attempt.elapsed_ms == Some(7)
                )
        );
        clear_capture_probe_cache_for_tests();
    }

    #[test]
    fn capture_autotune_cache_expires_after_ttl() {
        let _guard = capture_probe_cache_test_guard();
        clear_capture_probe_cache_for_tests();
        let config = CaptureBackendConfig::new(
            AdbConfig {
                adb_path: String::new(),
                command_timeout: Duration::from_millis(1),
            },
            DeviceTarget::default(),
        );
        let key = CaptureProbeCacheKey::new(&config, CaptureBackendName::AdbScreencap);
        capture_probe_cache().lock().expect("cache lock").insert(
            key.clone(),
            CaptureProbeCacheEntry {
                ok: true,
                message: "old ok".to_string(),
                elapsed_ms: 4,
                inserted_at: Instant::now() - Duration::from_secs(2),
            },
        );

        let cached =
            capture_probe_cache_lookup(&key, Duration::from_millis(500)).expect("cache lookup");

        assert!(cached.is_none());
        assert!(
            !capture_probe_cache()
                .lock()
                .expect("cache lock")
                .contains_key(&key)
        );
        clear_capture_probe_cache_for_tests();
    }

    #[test]
    fn frame_from_pixels_keeps_png_encoding_out_of_capture_path() {
        let frame = Frame::from_pixels(
            1,
            1,
            vec![1, 2, 3],
            PixelFormat::Rgb8,
            CaptureBackendName::DroidcastRaw,
        )
        .expect("raw frame");

        assert!(frame.original_png.is_none());
        let png = frame.encode_png_fast().expect("artifact PNG");
        assert_eq!(parse_png_dimensions(&png).expect("dimensions"), (1, 1));
    }

    #[test]
    fn frame_from_png_preserves_adb_original_png() {
        let source = Frame::from_pixels(
            1,
            1,
            vec![1, 2, 3],
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
        .expect("raw frame");
        let png = source.encode_png_fast().expect("source PNG");
        let frame =
            Frame::from_png(png.clone(), CaptureBackendName::AdbScreencap).expect("PNG frame");

        assert_eq!(frame.original_png.as_deref(), Some(png.as_slice()));
        assert_eq!((frame.width, frame.height), (1, 1));
    }

    #[test]
    fn adb_png_channel_contract_preserves_rgb_channels() {
        let pixels = rgba_contract_pixels();
        let png = encode_png_fast(2, 2, &pixels, PixelFormat::Rgba8).expect("encode png");
        let frame = Frame::from_png(png, CaptureBackendName::AdbScreencap).expect("decode png");

        assert_eq!(frame.pixel_format, PixelFormat::Rgba8);
        assert_eq!(frame.pixels, pixels);
    }

    #[test]
    fn droidcast_rgb565_channel_contract_preserves_rgb_channels() {
        let raw = [
            0x00, 0xf8, // red
            0xe0, 0x07, // green
            0x1f, 0x00, // blue
            0xff, 0xff, // white
        ];
        let pixels = rgb565_to_rgb8(&raw, 2, 2).expect("rgb565");

        assert_eq!(pixels, vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255]);
    }

    #[test]
    fn converts_rgb565_to_rgb8() {
        let raw = [0x00, 0xf8, 0xe0, 0x07, 0x1f, 0x00];
        let pixels = rgb565_to_rgb8(&raw, 3, 1).expect("rgb565");
        assert_eq!(pixels, vec![255, 0, 0, 0, 255, 0, 0, 0, 255]);
    }

    #[test]
    fn nemu_rgba_bottom_up_channel_contract_preserves_rgb_channels() {
        let top_down = rgba_contract_pixels();
        let raw_bottom_up = vec![
            top_down[8],
            top_down[9],
            top_down[10],
            top_down[11],
            top_down[12],
            top_down[13],
            top_down[14],
            top_down[15],
            top_down[0],
            top_down[1],
            top_down[2],
            top_down[3],
            top_down[4],
            top_down[5],
            top_down[6],
            top_down[7],
        ];
        let pixels = rgba_bottom_up_to_rgba(&raw_bottom_up, 2, 2).expect("rgba");

        assert_eq!(pixels, top_down);
    }

    #[test]
    fn capture_attempts_mark_nemu_channel_order_as_mumu_verified() {
        let attempt = CaptureBackendAttempt::success(
            CaptureBackendName::NemuIpc,
            "ok".to_string(),
            Some(1),
            false,
        );

        assert_eq!(attempt.channel_order_contract, "mumu_nemu_verified");
    }

    #[test]
    fn parses_screen_size() {
        assert_eq!(
            parse_screen_size("Physical size: 1280x720").expect("screen size"),
            (1280, 720)
        );
    }

    #[test]
    fn rejects_invalid_screen_sizes() {
        for text in [
            "Physical size: 1280",
            "Physical size: invalidx720",
            "Physical size: 0x720",
            "Physical size: 1280x0",
        ] {
            assert_fatal(parse_screen_size(text));
        }
    }

    #[test]
    fn priority_capture_uses_second_backend_after_first_failure() {
        let drops = Rc::new(Cell::new(0));
        let mut outcomes = vec![
            CaptureProbeOutcome::Unavailable(CaptureBackendAttempt::failure(
                CaptureBackendName::NemuIpc,
                "unavailable".to_string(),
                Some(1),
                false,
            )),
            CaptureProbeOutcome::Available(
                Box::new(FakeCaptureBackend {
                    drops: Rc::clone(&drops),
                }),
                CaptureBackendAttempt::success(
                    CaptureBackendName::DroidcastRaw,
                    "available".to_string(),
                    Some(2),
                    false,
                ),
                2,
            ),
        ]
        .into_iter();

        let selected = select_auto_capture_backend_with_probe(
            AutoCaptureMode::Priority,
            [
                CaptureBackendName::NemuIpc,
                CaptureBackendName::DroidcastRaw,
            ],
            |_| Ok(outcomes.next().expect("probe outcome")),
        )
        .expect("second backend selected");

        assert_eq!(selected.diagnostics.used, CaptureBackendName::DroidcastRaw);
        assert_eq!(selected.diagnostics.attempts.len(), 2);
        assert_eq!(drops.get(), 0);
    }

    #[test]
    fn fastest_capture_selects_faster_backend_and_releases_loser() {
        let drops = Rc::new(Cell::new(0));
        let mut outcomes = [
            (CaptureBackendName::NemuIpc, 9),
            (CaptureBackendName::DroidcastRaw, 3),
        ]
        .into_iter()
        .map(|(name, elapsed_ms)| {
            CaptureProbeOutcome::Available(
                Box::new(FakeCaptureBackend {
                    drops: Rc::clone(&drops),
                }),
                CaptureBackendAttempt::success(
                    name,
                    "available".to_string(),
                    Some(elapsed_ms),
                    false,
                ),
                elapsed_ms,
            )
        });

        let selected = select_auto_capture_backend_with_probe(
            AutoCaptureMode::Fastest,
            [
                CaptureBackendName::NemuIpc,
                CaptureBackendName::DroidcastRaw,
            ],
            |_| Ok(outcomes.next().expect("probe outcome")),
        )
        .expect("fastest backend selected");

        assert_eq!(selected.diagnostics.used, CaptureBackendName::DroidcastRaw);
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn parses_device_rotation_and_droidcast_request_size() {
        assert_eq!(
            parse_device_rotation("1\n").expect("rotation"),
            DeviceRotation::R90
        );
        assert_eq!(
            parse_display_orientation("DisplayViewport{orientation=1, deviceWidth=1280}")
                .expect("display orientation"),
            Some(DeviceRotation::R90)
        );
        assert_eq!(
            droidcast_request_size(1280, 720, DeviceRotation::R90),
            (720, 1280)
        );
        assert_eq!(
            droidcast_request_size(1280, 720, DeviceRotation::R0),
            (1280, 720)
        );
        assert_eq!(
            display_size_from_natural(720, 1280, DeviceRotation::R90),
            (1280, 720)
        );
        assert_eq!(droidcast_decode_size(720, 1280, 1280, 720), (720, 1280));
    }

    #[test]
    fn keeps_droidcast_frame_when_already_display_sized() {
        let pixels = rgb8_ids(&[0, 1, 2, 3, 4, 5]);
        let (width, height, output) =
            orient_rgb8_frame_to_display(pixels.clone(), 3, 2, 3, 2, DeviceRotation::R90)
                .expect("display sized");
        assert_eq!((width, height), (3, 2));
        assert_eq!(output, pixels);
    }

    #[test]
    fn rotates_droidcast_swapped_frames_to_display_orientation() {
        let pixels = rgb8_ids(&[0, 1, 2, 3, 4, 5]);
        let (width, height, clockwise) =
            orient_rgb8_frame_to_display(pixels.clone(), 2, 3, 3, 2, DeviceRotation::R90)
                .expect("clockwise");
        assert_eq!((width, height), (3, 2));
        assert_eq!(rgb8_red_ids(&clockwise), vec![4, 2, 0, 5, 3, 1]);

        let (width, height, counterclockwise) =
            orient_rgb8_frame_to_display(pixels.clone(), 2, 3, 3, 2, DeviceRotation::R270)
                .expect("ccw");
        assert_eq!((width, height), (3, 2));
        assert_eq!(rgb8_red_ids(&counterclockwise), vec![1, 3, 5, 0, 2, 4]);

        let (width, height, stale_rotation) =
            orient_rgb8_frame_to_display(pixels, 2, 3, 3, 2, DeviceRotation::R0)
                .expect("stale orientation");
        assert_eq!((width, height), (3, 2));
        assert_eq!(rgb8_red_ids(&stale_rotation), vec![4, 2, 0, 5, 3, 1]);
    }

    #[test]
    fn encodes_nemu_folder_as_utf16_with_nul() {
        let path = Path::new(r"D:\BST\MuMuPlayer");
        let wide = nul_terminated_utf16_path(path).expect("wide path");
        let expected = r"D:\BST\MuMuPlayer"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        assert_eq!(wide, expected);
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

    fn clear_capture_probe_cache_for_tests() {
        capture_probe_cache().lock().expect("cache lock").clear();
    }

    fn capture_probe_cache_test_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("test lock")
    }

    fn rgb8_ids(ids: &[u8]) -> Vec<u8> {
        ids.iter().flat_map(|id| [*id, 0, 0]).collect()
    }

    fn rgb8_red_ids(pixels: &[u8]) -> Vec<u8> {
        pixels.chunks_exact(3).map(|chunk| chunk[0]).collect()
    }

    fn rgba_contract_pixels() -> Vec<u8> {
        vec![
            255, 0, 0, 255, // red
            0, 255, 0, 255, // green
            0, 0, 255, 255, // blue
            255, 255, 255, 255, // white
        ]
    }
}
