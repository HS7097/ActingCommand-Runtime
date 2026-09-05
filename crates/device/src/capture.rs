// SPDX-License-Identifier: AGPL-3.0-only

use crate::adb::{ACTINGCOMMAND_NEMU_FOLDER_ENV, Adb, AdbConfig, stop_child};
use crate::mumu::{
    mumu_root_from_path, nemu_configured_adb_class, resolve_mumu_backend_paths,
    resolve_mumu_backend_paths_for_running_target,
};
use crate::vendor_stdio::{VendorStdioCapture, VendorStdioSession};
use crate::{
    DeviceCloseAuthority, DeviceError, DeviceErrorCategory, DeviceErrorDiagnosticMessage,
    DeviceErrorSensitivity, DeviceResourceCloseOutcome, DeviceResourceClosePhase,
    DeviceResourceKind, DeviceResourceQuiescence, DeviceResult, DeviceTarget,
    NemuResolutionContext, NemuResolutionCountKind, NemuResolutionReason,
};
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

    fn close_once(
        &mut self,
        authority: DeviceCloseAuthority,
    ) -> DeviceResult<DeviceResourceCloseOutcome>;

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
    FixtureSimulation,
    AdbScreencap,
    AdbScreencapEncode,
    AdbScreencapRawGzip,
    DroidcastRaw,
    NemuIpc,
}

impl CaptureBackendName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FixtureSimulation => "fixture_simulation",
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
    let config = prepare_capture_backend_config(config)?;
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

fn prepare_capture_backend_config(
    config: CaptureBackendConfig,
) -> DeviceResult<CaptureBackendConfig> {
    let explicit_root = config
        .nemu
        .nemu_folder
        .clone()
        .or_else(|| std::env::var_os(ACTINGCOMMAND_NEMU_FOLDER_ENV).map(PathBuf::from));
    let explicit_dll = config
        .nemu
        .dll_path
        .clone()
        .or_else(|| std::env::var_os("ACTINGCOMMAND_NEMU_IPC_DLL").map(PathBuf::from));
    prepare_capture_backend_config_with_resolvers(
        config,
        explicit_root,
        explicit_dll,
        resolve_mumu_backend_paths,
        resolve_mumu_backend_paths_for_running_target,
    )
}

fn prepare_capture_backend_config_with_resolvers<F, G>(
    mut config: CaptureBackendConfig,
    explicit_root: Option<PathBuf>,
    explicit_dll: Option<PathBuf>,
    resolve_mumu: F,
    resolve_running_target: G,
) -> DeviceResult<CaptureBackendConfig>
where
    F: FnOnce(
        Option<PathBuf>,
        Option<PathBuf>,
        Option<PathBuf>,
    ) -> DeviceResult<Option<crate::mumu::MumuBackendPaths>>,
    G: FnOnce(
        PathBuf,
        &str,
        Option<i32>,
        Option<PathBuf>,
        Option<PathBuf>,
    ) -> DeviceResult<crate::mumu::MumuBackendPaths>,
{
    if !config.nemu.mumu_identity_resolved
        && matches!(
            config.requested,
            CaptureBackendChoice::Auto
                | CaptureBackendChoice::AutoFastest
                | CaptureBackendChoice::NemuIpc
        )
    {
        let configured_adb = (!config.adb_config.adb_path.trim().is_empty())
            .then(|| PathBuf::from(&config.adb_config.adb_path));
        let adb_class = Some(nemu_configured_adb_class(configured_adb.as_deref()));
        let has_root = explicit_root.is_some();
        let has_dll = explicit_dll.is_some();
        let explicit_capture_identity = config.requested == CaptureBackendChoice::NemuIpc
            && configured_adb
                .as_deref()
                .is_some_and(|path| mumu_root_from_path(path).is_none());
        config.nemu.mumu_identity_resolved = true;
        let generic_adb_for_auto = matches!(
            config.requested,
            CaptureBackendChoice::Auto | CaptureBackendChoice::AutoFastest
        ) && configured_adb
            .as_deref()
            .is_some_and(|path| mumu_root_from_path(path).is_none())
            && explicit_root.is_none()
            && explicit_dll.is_none();
        if generic_adb_for_auto {
            config.nemu.mumu_identity_unavailable = Some(NemuIdentityUnavailable::new(
                format!(
                    "Nemu IPC unavailable: configured ADB {} is not associated with a MuMu installation; generic Auto channels remain available",
                    config.adb_config.adb_path
                ),
                NemuCaptureResolutionDetail::Identity,
            ).with_context(
                NemuResolutionContext::new(NemuResolutionReason::ConfiguredAdbIdentityUnrecognized)
                    .with_provenance(adb_class, has_root, has_dll),
            ));
        } else {
            let resolved = if explicit_capture_identity
                && (explicit_root.is_none() || explicit_dll.is_none())
            {
                let configured_adb = configured_adb.clone().ok_or_else(|| {
                    with_nemu_capture_resolution_detail(
                        DeviceError::fatal(
                            "explicit Nemu IPC running-target binding requires a configured ADB",
                        ),
                        NemuCaptureResolutionDetail::Target,
                    )
                })?;
                let target_serial = config.target.resolved_serial();
                Some(
                    resolve_running_target(
                        configured_adb,
                        &target_serial,
                        config.nemu.instance_id,
                        explicit_root,
                        explicit_dll,
                    )
                    .map_err(|error| {
                        with_nemu_capture_resolution_detail(
                            error.with_nemu_resolution_provenance(adb_class, has_root, has_dll),
                            NemuCaptureResolutionDetail::Target,
                        )
                    })?,
                )
            } else {
                let resolver_adb = if explicit_capture_identity {
                    None
                } else {
                    configured_adb
                };
                let resolution_detail = if resolver_adb.is_some() {
                    NemuCaptureResolutionDetail::Identity
                } else {
                    NemuCaptureResolutionDetail::Installation
                };
                resolve_mumu(resolver_adb, explicit_root, explicit_dll).map_err(|error| {
                    with_nemu_capture_resolution_detail(
                        error.with_nemu_resolution_provenance(adb_class, has_root, has_dll),
                        resolution_detail,
                    )
                })?
            };
            match resolved {
                Some(paths) => {
                    if !explicit_capture_identity {
                        config.adb_config.adb_path = paths.adb_path.to_string_lossy().to_string();
                    }
                    config.nemu.nemu_folder = Some(paths.installation.root);
                    config.nemu.dll_path = Some(paths.capture_dll_path);
                    config.nemu.mumu_identity_unavailable = None;
                }
                None => {
                    config.nemu.mumu_identity_unavailable = Some(NemuIdentityUnavailable::new(
                        "Nemu IPC unavailable: no coordinated MuMu installation identity was resolved"
                            .to_string(),
                        NemuCaptureResolutionDetail::Installation,
                    ).with_context(
                        NemuResolutionContext::new(NemuResolutionReason::InstallationAbsent)
                            .with_count(NemuResolutionCountKind::InstallationRoots, 0, false)
                            .with_provenance(adb_class, has_root, has_dll),
                    ));
                }
            }
        }
    }
    if config.adb_config.adb_path.trim().is_empty() {
        config.adb_config = AdbConfig::resolve(None)?.0;
    }
    Ok(config)
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

    for (candidate_index, name) in candidates.into_iter().enumerate() {
        let candidate_index = u8::try_from(candidate_index).unwrap_or(u8::MAX);
        let probe_outcome = match probe(name) {
            Ok(outcome) => outcome,
            Err(primary) => {
                return Err(close_capture_candidates(
                    successful,
                    primary.with_resource_candidate_index(candidate_index),
                ));
            }
        };
        match probe_outcome {
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
                successful.push((candidate_index, name, elapsed_ms, backend));
            }
            CaptureProbeOutcome::Unavailable(attempt) => attempts.push(attempt),
        }
    }

    if mode == AutoCaptureMode::Fastest && !successful.is_empty() {
        let fastest_index = successful
            .iter()
            .enumerate()
            .min_by_key(|(_, (_, _, elapsed_ms, _))| *elapsed_ms)
            .map(|(index, _)| index)
            .expect("non-empty successful capture candidates");
        let (_candidate_index, used, _elapsed_ms, backend) = successful.swap_remove(fastest_index);
        let mut cleanup_error: Option<DeviceError> = None;
        for (loser_index, _name, _elapsed_ms, mut loser) in successful {
            if let Err(cleanup) = loser.close_once(DeviceCloseAuthority::LocalOnly) {
                if cleanup.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed) {
                    std::mem::forget(loser);
                }
                let cleanup = cleanup.with_resource_candidate_index(loser_index);
                cleanup_error = Some(match cleanup_error {
                    Some(primary) => primary.merge_resource_cleanup(cleanup),
                    None => cleanup,
                });
            }
        }
        if let Some(primary) = cleanup_error {
            let mut backend = backend;
            return Err(match backend.close_once(DeviceCloseAuthority::LocalOnly) {
                Ok(_) => primary,
                Err(winner_cleanup) => {
                    if winner_cleanup.resource_quiescence()
                        == Some(DeviceResourceQuiescence::Unconfirmed)
                    {
                        std::mem::forget(backend);
                    }
                    primary.merge_resource_cleanup(winner_cleanup)
                }
            });
        }
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

fn close_capture_candidates(
    candidates: Vec<(u8, CaptureBackendName, u128, Box<dyn CaptureBackend>)>,
    primary: DeviceError,
) -> DeviceError {
    candidates.into_iter().fold(
        primary,
        |primary, (index, _name, _elapsed_ms, mut backend)| match backend
            .close_once(DeviceCloseAuthority::LocalOnly)
        {
            Ok(_) => primary,
            Err(cleanup) => {
                if cleanup.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed) {
                    std::mem::forget(backend);
                }
                primary.merge_resource_cleanup(cleanup.with_resource_candidate_index(index))
            }
        },
    )
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
                if err.resource_quiescence().is_some() || !err.resource_close_causes().is_empty() {
                    return Err(err);
                }
                let attempt =
                    CaptureBackendAttempt::failure(name, err.message().to_string(), None, false);
                if let Err(cache_error) = capture_probe_cache_store(key, &attempt) {
                    return Err(merge_probe_cache_failure(err, cache_error));
                }
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
                if let Err(cache_error) = capture_probe_cache_store(key, &attempt) {
                    return Err(close_capture_backend_after_error(backend, cache_error));
                }
                Ok(CaptureProbeOutcome::Available(backend, attempt, elapsed_ms))
            }
            Err(error) => {
                if error.resource_quiescence().is_some()
                    || !error.resource_close_causes().is_empty()
                {
                    return Err(error);
                }
                let elapsed_ms = started.elapsed().as_millis();
                let attempt = CaptureBackendAttempt::failure(
                    name,
                    error.message().to_string(),
                    Some(elapsed_ms),
                    false,
                );
                if let Err(cache_error) = capture_probe_cache_store(key, &attempt) {
                    return Err(merge_probe_cache_failure(error, cache_error));
                }
                if error.resource_close_causes().is_empty() {
                    Ok(CaptureProbeOutcome::Unavailable(attempt))
                } else {
                    Err(error)
                }
            }
        },
        Err(err) => {
            if err.resource_quiescence().is_some() || !err.resource_close_causes().is_empty() {
                return Err(err);
            }
            let elapsed_ms = started.elapsed().as_millis();
            let attempt = CaptureBackendAttempt::failure(
                name,
                err.message().to_string(),
                Some(elapsed_ms),
                false,
            );
            if let Err(cache_error) = capture_probe_cache_store(key, &attempt) {
                return Err(merge_probe_cache_failure(err, cache_error));
            }
            Ok(CaptureProbeOutcome::Unavailable(attempt))
        }
    }
}

fn close_capture_backend_after_error(
    mut backend: Box<dyn CaptureBackend>,
    primary: DeviceError,
) -> DeviceError {
    match backend.close_once(DeviceCloseAuthority::LocalOnly) {
        Ok(_) => primary,
        Err(cleanup) => {
            if cleanup.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed) {
                std::mem::forget(backend);
            }
            primary.merge_resource_cleanup(cleanup)
        }
    }
}

fn merge_probe_cache_failure(primary: DeviceError, cache: DeviceError) -> DeviceError {
    let message = format!("{primary}; capture probe cache update also failed: {cache}");
    primary.with_severity_and_message(crate::DeviceErrorSeverity::Fatal, message)
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
        CaptureBackendName::FixtureSimulation => Err(DeviceError::fatal(
            "fixture simulation cannot be opened through the device capture factory",
        )),
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
    close_result: Option<DeviceResult<DeviceResourceCloseOutcome>>,
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

    fn close_once(
        &mut self,
        authority: DeviceCloseAuthority,
    ) -> DeviceResult<DeviceResourceCloseOutcome> {
        if let Some(result) = &self.close_result {
            return result.clone();
        }
        let local_count = u16::from(self.primed.take().is_some());
        let result = self
            .inner
            .close_once(authority)
            .map(|outcome| outcome.combine(DeviceResourceCloseOutcome::confirmed(local_count)))
            .map_err(|error| {
                let quiescence = error
                    .resource_quiescence()
                    .unwrap_or(DeviceResourceQuiescence::Unconfirmed);
                let resource_count = error.resource_count().saturating_add(local_count);
                error.with_resource_quiescence(quiescence, resource_count)
            });
        self.close_result = Some(result.clone());
        result
    }
}

fn prime_capture_backend(
    name: CaptureBackendName,
    mut backend: Box<dyn CaptureBackend>,
) -> DeviceResult<PrimedCaptureResult> {
    let captured = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| backend.capture()))
        .unwrap_or_else(|_| Err(DeviceError::fatal("capture probe panicked")));
    match captured {
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
                    close_result: None,
                }),
                message,
                vendor_stdio,
            ))
        }
        Err(primary) => match backend.close_once(DeviceCloseAuthority::LocalOnly) {
            Ok(_) => Err(primary),
            Err(cleanup) => {
                if cleanup.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed) {
                    std::mem::forget(backend);
                }
                Err(primary.merge_resource_cleanup(cleanup))
            }
        },
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
    close_result: Option<DeviceResult<DeviceResourceCloseOutcome>>,
}

impl ScreencapBackend {
    pub fn new(adb_config: AdbConfig, target: DeviceTarget) -> Self {
        Self {
            adb_config,
            target,
            capture_timeout: DEFAULT_CAPTURE_TIMEOUT,
            close_result: None,
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
    close_result: Option<DeviceResult<DeviceResourceCloseOutcome>>,
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
            close_result: None,
        })
    }

    fn start_if_needed(&mut self) -> DeviceResult<(u32, u32)> {
        let adb = Adb::new(self.adb_config.clone());
        verify_adb_device(&adb, &self.target, &self.serial)?;
        let (width, height) = parse_screen_size(&adb.screen_size(&self.serial)?)?;
        if self.started {
            return Ok((width, height));
        }
        self.stop_child_if_present()?;

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
        self.close_result = None;
        if let Err(err) = wait_for_droidcast(self.config.local_port, self.capture_timeout) {
            return match self.close_once(DeviceCloseAuthority::LocalOnly) {
                Ok(_) => Err(err),
                Err(cleanup) => Err(err.merge_resource_cleanup(cleanup)),
            };
        }
        self.started = true;
        Ok((width, height))
    }

    fn stop_child_if_present(&mut self) -> DeviceResult<DeviceResourceCloseOutcome> {
        let Some(child) = self.child.as_mut() else {
            self.started = false;
            return Ok(DeviceResourceCloseOutcome::confirmed(0));
        };
        let result = stop_child(child, Duration::from_millis(500), "droidcast_raw");
        if result.is_ok()
            || result.as_ref().is_err_and(|error| {
                error.resource_quiescence() == Some(DeviceResourceQuiescence::Confirmed)
            })
        {
            self.child.take();
        }
        self.started = false;
        result
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

    fn close_once(
        &mut self,
        _authority: DeviceCloseAuthority,
    ) -> DeviceResult<DeviceResourceCloseOutcome> {
        if let Some(result) = &self.close_result {
            return result.clone();
        }
        let result = self.stop_child_if_present();
        self.close_result = Some(result.clone());
        result
    }
}

impl Drop for DroidcastRawBackend {
    fn drop(&mut self) {
        let first_close = self.close_result.is_none();
        if let Err(error) = self.close_once(DeviceCloseAuthority::LocalOnly) {
            if error.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed)
                && let Some(owned) = self.child.take()
            {
                std::mem::forget(owned);
            }
            if first_close && !thread::panicking() {
                panic!("{error}");
            }
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NemuCaptureResolutionDetail {
    Installation,
    Identity,
    Target,
}

impl NemuCaptureResolutionDetail {
    const fn stage(self) -> &'static str {
        match self {
            Self::Installation => "nemu.installation.resolve",
            Self::Identity => "nemu.capture.identity",
            Self::Target => "nemu.target.resolve",
        }
    }

    const fn operation(self) -> &'static str {
        match self {
            Self::Installation => "installation_resolve",
            Self::Identity => "capture_identity",
            Self::Target => "target_resolve",
        }
    }

    const fn message(self) -> DeviceErrorDiagnosticMessage {
        match self {
            Self::Installation => DeviceErrorDiagnosticMessage::NemuInstallationResolveFailed,
            Self::Identity => DeviceErrorDiagnosticMessage::NemuCaptureIdentityUncoordinated,
            Self::Target => DeviceErrorDiagnosticMessage::NemuTargetResolveFailed,
        }
    }
}

fn with_nemu_capture_resolution_detail(
    error: DeviceError,
    detail: NemuCaptureResolutionDetail,
) -> DeviceError {
    let producer_complete = error.diagnostic().is_some() && error.diagnostic_context().is_some();
    let producer_message = error.diagnostic_message().is_some();
    let error = error
        .with_diagnostic_if_absent(DeviceErrorCategory::Protocol, detail.stage())
        .with_diagnostic_context_if_absent(
            "nemu_ipc",
            detail.operation(),
            DeviceErrorSensitivity::Internal,
        );
    if producer_complete || producer_message {
        error
    } else {
        error.with_diagnostic_message(detail.message())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NemuIdentityUnavailable {
    message: String,
    detail: NemuCaptureResolutionDetail,
    context: Option<NemuResolutionContext>,
}

impl NemuIdentityUnavailable {
    fn new(message: String, detail: NemuCaptureResolutionDetail) -> Self {
        Self {
            message,
            detail,
            context: None,
        }
    }

    fn with_context(mut self, context: NemuResolutionContext) -> Self {
        self.context = Some(context);
        self
    }
}

impl std::ops::Deref for NemuIdentityUnavailable {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.message
    }
}

#[derive(Debug, Clone, Default)]
pub struct NemuIpcConfig {
    pub nemu_folder: Option<PathBuf>,
    pub dll_path: Option<PathBuf>,
    pub instance_id: Option<i32>,
    pub display_id: i32,
    mumu_identity_resolved: bool,
    mumu_identity_unavailable: Option<NemuIdentityUnavailable>,
}

pub struct NemuIpcBackend {
    worker: Option<NemuIpcWorker>,
    frame_width: u32,
    frame_height: u32,
    vendor_stdio: Vec<VendorStdioCapture>,
    close_result: Option<DeviceResult<DeviceResourceCloseOutcome>>,
}

type NemuConnect = unsafe extern "C" fn(*const u16, i32) -> i32;
type NemuDisconnect = unsafe extern "C" fn(i32);
type NemuCaptureDisplay = unsafe extern "C" fn(i32, i32, i32, *mut i32, *mut i32, *mut u8) -> i32;

impl NemuIpcBackend {
    pub fn new(
        target: DeviceTarget,
        config: NemuIpcConfig,
        capture_timeout: Duration,
    ) -> DeviceResult<Self> {
        if let Some(reason) = &config.mumu_identity_unavailable {
            let mut error = DeviceError::fatal(reason.message.clone());
            if let Some(context) = reason.context {
                error = error.with_nemu_resolution_context_if_absent(context);
            }
            return Err(with_nemu_capture_resolution_detail(error, reason.detail));
        }
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
                with_nemu_capture_resolution_detail(
                    DeviceError::fatal(format!(
                        "Nemu IPC unavailable: cannot derive MuMu instance id from serial {serial}"
                    ))
                    .with_nemu_resolution_context_if_absent(
                        NemuResolutionContext::new(NemuResolutionReason::TargetIdentityUnavailable),
                    ),
                    NemuCaptureResolutionDetail::Target,
                )
            })?;
        let (nemu_folder, dll_path) = if config.mumu_identity_resolved {
            match (config.nemu_folder, config.dll_path) {
                (Some(folder), Some(dll_path)) => (folder, dll_path),
                _ => {
                    return Err(with_nemu_capture_resolution_detail(
                        DeviceError::fatal(
                            "Nemu IPC unavailable: no coordinated MuMu installation identity was resolved",
                        ).with_nemu_resolution_context_if_absent(
                            NemuResolutionContext::new(NemuResolutionReason::CaptureIdentityUncoordinated),
                        ),
                        NemuCaptureResolutionDetail::Identity,
                    ));
                }
            }
        } else {
            resolve_nemu_paths(config.nemu_folder, config.dll_path)?
        };
        let mut worker = NemuIpcWorker::spawn(
            nemu_folder,
            dll_path,
            instance_id,
            config.display_id,
            capture_timeout,
        );
        let (frame_width, frame_height) = match worker.probe_resolution() {
            Ok(resolution) => resolution,
            Err(primary) => {
                return match worker.shutdown_once(DeviceCloseAuthority::LocalOnly) {
                    Ok(_) => Err(primary),
                    Err(cleanup) => {
                        let unconfirmed = cleanup.resource_quiescence()
                            == Some(DeviceResourceQuiescence::Unconfirmed);
                        let error = primary.merge_resource_cleanup(cleanup);
                        if unconfirmed {
                            std::mem::forget(worker);
                        }
                        Err(error)
                    }
                };
            }
        };
        Ok(Self {
            worker: Some(worker),
            frame_width,
            frame_height,
            vendor_stdio: Vec::new(),
            close_result: None,
        })
    }
}

enum NemuIpcCommand {
    Probe(mpsc::Sender<DeviceResult<(u32, u32)>>),
    Capture(mpsc::Sender<DeviceResult<NemuCapturedFrame>>),
    Shutdown {
        authority: DeviceCloseAuthority,
        response: mpsc::Sender<DeviceResult<DeviceResourceCloseOutcome>>,
    },
}

struct NemuCapturedFrame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    vendor_stdio: Vec<VendorStdioCapture>,
}

struct NemuIpcWorker {
    tx: mpsc::Sender<NemuIpcCommand>,
    handle: Option<JoinHandle<DeviceResult<()>>>,
    timeout: Duration,
    poisoned: bool,
    close_result: Option<DeviceResult<DeviceResourceCloseOutcome>>,
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
            let mut closed = false;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                while let Ok(command) = rx.recv() {
                    match command {
                        NemuIpcCommand::Probe(response) => {
                            response
                                .send(worker_state_result(&mut state, |state| {
                                    state.probe_resolution()
                                }))
                                .map_err(|_| DeviceError::fatal("Nemu IPC probe response lost"))?;
                        }
                        NemuIpcCommand::Capture(response) => {
                            response
                                .send(worker_state_result(&mut state, |state| {
                                    state.capture_frame()
                                }))
                                .map_err(|_| {
                                    DeviceError::fatal("Nemu IPC capture response lost")
                                })?;
                        }
                        NemuIpcCommand::Shutdown {
                            authority,
                            response,
                        } => {
                            let result =
                                worker_state_result(&mut state, |state| state.close(authority));
                            closed = true;
                            if response.send(result.clone()).is_err() {
                                return Err(match result {
                                    Ok(_) => DeviceError::fatal("Nemu IPC close response lost"),
                                    Err(primary) => primary,
                                });
                            }
                            return result.map(|_| ());
                        }
                    }
                }
                Err(DeviceError::fatal("Nemu IPC command channel disconnected"))
            }))
            .unwrap_or_else(|_| {
                Err(
                    DeviceError::fatal("Nemu IPC worker panicked").with_resource_close_cause(
                        DeviceResourceKind::InProcessWorker,
                        DeviceResourceClosePhase::WorkerJoin,
                        "nemu_ipc",
                        None,
                        Some(instance_id),
                        DeviceResourceQuiescence::Unconfirmed,
                        1,
                    ),
                )
            });
            let result = if closed {
                result
            } else {
                let cleanup = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    worker_state_result(&mut state, |state| {
                        state.close(DeviceCloseAuthority::LocalOnly)
                    })
                }))
                .unwrap_or_else(|_| {
                    Err(DeviceError::fatal("Nemu IPC worker close panicked")
                        .with_resource_quiescence(DeviceResourceQuiescence::Unconfirmed, 1))
                });
                match (result, cleanup) {
                    (Ok(()), Ok(_)) => Ok(()),
                    (Err(primary), Ok(_)) => Err(primary),
                    (Ok(()), Err(cleanup)) => Err(cleanup),
                    (Err(primary), Err(cleanup)) => Err(primary.merge_resource_cleanup(cleanup)),
                }
            };
            if result.as_ref().is_err_and(|error| {
                error.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed)
            }) {
                std::mem::forget(state);
            }
            result
        });
        Self {
            tx,
            handle: Some(handle),
            timeout,
            poisoned: false,
            close_result: None,
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

    fn shutdown_once(
        &mut self,
        authority: DeviceCloseAuthority,
    ) -> DeviceResult<DeviceResourceCloseOutcome> {
        if let Some(result) = &self.close_result {
            return result.clone();
        }
        if self.poisoned {
            let result = Err(DeviceError::fatal(
                "Nemu IPC worker state is unconfirmed after a previous timeout",
            )
            .with_resource_close_cause(
                DeviceResourceKind::InProcessWorker,
                DeviceResourceClosePhase::WorkerReceive,
                "nemu_ipc",
                None,
                None,
                DeviceResourceQuiescence::Unconfirmed,
                1,
            ));
            self.close_result = Some(result.clone());
            return result;
        }

        let (tx, rx) = mpsc::channel();
        let result = if self
            .tx
            .send(NemuIpcCommand::Shutdown {
                authority,
                response: tx,
            })
            .is_err()
        {
            Err(
                DeviceError::fatal("failed to send Nemu IPC shutdown command")
                    .with_resource_close_cause(
                        DeviceResourceKind::InProcessWorker,
                        DeviceResourceClosePhase::WorkerSend,
                        "nemu_ipc",
                        None,
                        None,
                        DeviceResourceQuiescence::Unconfirmed,
                        1,
                    ),
            )
        } else {
            match rx.recv_timeout(self.timeout) {
                Ok(result) => result,
                Err(error) => Err(DeviceError::fatal(format!(
                    "Nemu IPC shutdown response was not confirmed: {error}"
                ))
                .with_resource_close_cause(
                    DeviceResourceKind::InProcessWorker,
                    DeviceResourceClosePhase::WorkerReceive,
                    "nemu_ipc",
                    None,
                    None,
                    DeviceResourceQuiescence::Unconfirmed,
                    1,
                )),
            }
        };
        let result = match (result, self.join_bounded()) {
            (Ok(outcome), Ok(())) => Ok(outcome.combine(DeviceResourceCloseOutcome::confirmed(1))),
            (Err(primary), Ok(())) => Err(primary),
            (Ok(_), Err(join)) => Err(join),
            (Err(primary), Err(join)) => Err(primary.merge_resource_cleanup(join)),
        };
        self.close_result = Some(result.clone());
        result
    }

    fn join_bounded(&mut self) -> DeviceResult<()> {
        let Some(handle) = self.handle.as_ref() else {
            return Ok(());
        };
        let started = Instant::now();
        while !handle.is_finished() && started.elapsed() < self.timeout {
            thread::sleep(Duration::from_millis(25));
        }
        if !handle.is_finished() {
            return Err(DeviceError::fatal(format!(
                "Nemu IPC worker did not terminate within {:?}",
                self.timeout
            ))
            .with_resource_close_cause(
                DeviceResourceKind::InProcessWorker,
                DeviceResourceClosePhase::WorkerJoin,
                "nemu_ipc",
                None,
                None,
                DeviceResourceQuiescence::Unconfirmed,
                1,
            ));
        }
        self.handle
            .take()
            .expect("Nemu IPC join handle was checked")
            .join()
            .map_err(|_| {
                DeviceError::fatal("Nemu IPC worker panicked during shutdown")
                    .with_resource_close_cause(
                        DeviceResourceKind::InProcessWorker,
                        DeviceResourceClosePhase::WorkerJoin,
                        "nemu_ipc",
                        None,
                        None,
                        DeviceResourceQuiescence::Unconfirmed,
                        1,
                    )
            })?
    }
}

impl Drop for NemuIpcWorker {
    fn drop(&mut self) {
        let first_close = self.close_result.is_none();
        if let Err(error) = self.shutdown_once(DeviceCloseAuthority::LocalOnly) {
            if error.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed)
                && let Some(owned) = self.handle.take()
            {
                std::mem::forget(owned);
            }
            if first_close && !thread::panicking() {
                panic!("{error}");
            }
        }
    }
}
struct NemuIpcWorkerState {
    library: Option<Library>,
    stdio_session: Option<VendorStdioSession>,
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
        let mut state = Self {
            library: None,
            stdio_session: None,
            nemu_folder,
            instance_id,
            display_id,
            connect_id: 0,
            raw_buffer: Vec::new(),
            frame_width: 0,
            frame_height: 0,
            vendor_stdio: Vec::new(),
        };
        let acquired = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.stdio_session = Some(VendorStdioSession::start()?);
            state.library = Some(unsafe { Library::new(&dll_path) }.map_err(|error| {
                DeviceError::fatal(format!(
                    "Nemu IPC unavailable: failed to load {}: {error}",
                    dll_path.display()
                ))
            })?);
            state.record_vendor_stdio_snapshot()
        }))
        .unwrap_or_else(|_| Err(DeviceError::fatal("Nemu IPC initialization panicked")));
        match acquired {
            Ok(()) => Ok(state),
            Err(primary) => {
                let cleanup = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    state.close(DeviceCloseAuthority::LocalOnly)
                }))
                .unwrap_or_else(|_| {
                    Err(
                        DeviceError::fatal("Nemu IPC initialization cleanup panicked")
                            .with_resource_quiescence(DeviceResourceQuiescence::Unconfirmed, 1),
                    )
                });
                let error = match cleanup {
                    Ok(outcome) => primary.with_resource_summary(
                        DeviceResourceQuiescence::Confirmed,
                        outcome.resource_count(),
                    ),
                    Err(cleanup) => primary.merge_resource_cleanup(cleanup),
                };
                if error.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed) {
                    std::mem::forget(state);
                }
                Err(error)
            }
        }
    }
    fn connect(&mut self) -> DeviceResult<()> {
        if self.connect_id > 0 {
            return Ok(());
        }
        let connect = unsafe { self.symbol::<NemuConnect>(b"nemu_connect\0")? };
        let nemu_folder = self.nemu_folder.as_ptr();
        let instance_id = self.instance_id;
        let connect_id = unsafe { connect(nemu_folder, instance_id) };
        self.connect_id = connect_id;
        self.record_vendor_stdio_snapshot()?;
        if connect_id == 0 {
            return Err(DeviceError::fatal(
                "Nemu IPC connect returned 0; check MuMu path and running instance",
            ));
        }
        Ok(())
    }

    fn record_vendor_stdio(&mut self, capture: VendorStdioCapture) {
        if !capture.is_empty() {
            self.vendor_stdio.push(capture);
        }
    }

    fn record_vendor_stdio_snapshot(&mut self) -> DeviceResult<()> {
        let capture = self
            .stdio_session
            .as_mut()
            .ok_or_else(|| DeviceError::fatal("Nemu vendor stdio session is closed"))?
            .snapshot()?;
        self.record_vendor_stdio(capture);
        Ok(())
    }

    unsafe fn symbol<T>(&self, name: &[u8]) -> DeviceResult<T>
    where
        T: Copy,
    {
        let library = self
            .library
            .as_ref()
            .ok_or_else(|| DeviceError::fatal("Nemu IPC library is closed"))?;
        let symbol = unsafe { library.get::<T>(name) }.map_err(|err| {
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

    fn disconnect(&mut self, authority: DeviceCloseAuthority) -> DeviceResult<()> {
        if self.connect_id <= 0 {
            return Ok(());
        }
        if authority != DeviceCloseAuthority::FencedDeviceWrite {
            return Err(DeviceError::fatal(
                "Nemu IPC disconnect requires current fenced device-write authority",
            )
            .with_resource_close_cause(
                DeviceResourceKind::ProviderConnection,
                DeviceResourceClosePhase::DisconnectCall,
                "nemu_ipc",
                None,
                Some(self.instance_id),
                DeviceResourceQuiescence::Unconfirmed,
                1,
            ));
        }
        let disconnect =
            unsafe { self.symbol::<NemuDisconnect>(b"nemu_disconnect\0") }.map_err(|error| {
                error.with_resource_close_cause(
                    DeviceResourceKind::ProviderConnection,
                    DeviceResourceClosePhase::DisconnectSymbol,
                    "nemu_ipc",
                    None,
                    Some(self.instance_id),
                    DeviceResourceQuiescence::Unconfirmed,
                    1,
                )
            })?;
        let connect_id = self.connect_id;
        unsafe { disconnect(connect_id) };
        let primary = DeviceError::fatal(
            "Nemu IPC void disconnect has no independent termination acknowledgement",
        )
        .with_resource_close_cause(
            DeviceResourceKind::ProviderConnection,
            DeviceResourceClosePhase::DisconnectCall,
            "nemu_ipc",
            None,
            Some(self.instance_id),
            DeviceResourceQuiescence::Unconfirmed,
            1,
        );
        Err(match self.record_vendor_stdio_snapshot() {
            Ok(()) => primary,
            Err(cleanup) => primary.merge_resource_cleanup(cleanup.with_resource_close_cause(
                DeviceResourceKind::VendorStdio,
                DeviceResourceClosePhase::SnapshotRead,
                "nemu_ipc",
                None,
                Some(self.instance_id),
                DeviceResourceQuiescence::Unconfirmed,
                1,
            )),
        })
    }

    fn close(
        &mut self,
        authority: DeviceCloseAuthority,
    ) -> DeviceResult<DeviceResourceCloseOutcome> {
        let mut resource_count = u16::from(self.connect_id > 0)
            .saturating_add(u16::from(self.stdio_session.is_some()))
            .saturating_add(u16::from(self.library.is_some()));
        self.disconnect(authority)?;
        let mut failure = None;
        if let Some(stdio) = self.stdio_session.as_mut() {
            match stdio.finish() {
                Ok(outcome) => resource_count = resource_count.max(outcome.resource_count()),
                Err(error)
                    if error.resource_quiescence() == Some(DeviceResourceQuiescence::Confirmed) =>
                {
                    failure = Some(error)
                }
                Err(error) => return Err(error),
            }
        }
        self.stdio_session.take();
        if let Some(library) = self.library.take()
            && let Err(error) = library.close().map_err(|error| {
                DeviceError::fatal(format!("failed to unload Nemu IPC library: {error}"))
                    .with_resource_close_cause(
                        DeviceResourceKind::Library,
                        DeviceResourceClosePhase::LibraryUnload,
                        "nemu_ipc",
                        None,
                        Some(self.instance_id),
                        DeviceResourceQuiescence::Unconfirmed,
                        1,
                    )
            })
        {
            return Err(match failure {
                Some(primary) => primary.merge_resource_cleanup(error),
                None => error,
            });
        }
        failure.map_or(
            Ok(DeviceResourceCloseOutcome::confirmed(resource_count)),
            |error| {
                Err(error
                    .with_resource_summary(DeviceResourceQuiescence::Confirmed, resource_count))
            },
        )
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

    fn close_once(
        &mut self,
        authority: DeviceCloseAuthority,
    ) -> DeviceResult<DeviceResourceCloseOutcome> {
        if let Some(result) = &self.close_result {
            return result.clone();
        }
        let result = match self.worker.as_mut() {
            Some(worker) => worker.shutdown_once(authority),
            None => Ok(DeviceResourceCloseOutcome::confirmed(0)),
        };
        if result.is_ok() {
            self.worker.take();
        }
        self.close_result = Some(result.clone());
        result
    }
}

impl Drop for NemuIpcBackend {
    fn drop(&mut self) {
        if self.close_result.is_none()
            && let Err(error) = self.close_once(DeviceCloseAuthority::LocalOnly)
            && !thread::panicking()
        {
            panic!("{error}");
        }
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
pub(crate) enum DeviceRotation {
    R0,
    R90,
    R180,
    R270,
}

pub(crate) fn read_device_rotation(adb: &Adb, serial: &str) -> DeviceResult<DeviceRotation> {
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

pub(crate) fn display_size_from_natural(
    width: u32,
    height: u32,
    rotation: DeviceRotation,
) -> (u32, u32) {
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
    for chunk in raw.as_chunks::<2>().0 {
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
    let explicit_root =
        folder.or_else(|| std::env::var_os(ACTINGCOMMAND_NEMU_FOLDER_ENV).map(PathBuf::from));
    let explicit_dll =
        dll_path.or_else(|| std::env::var_os("ACTINGCOMMAND_NEMU_IPC_DLL").map(PathBuf::from));
    let paths = resolve_mumu_backend_paths(None, explicit_root, explicit_dll)
        .map_err(|error| {
            with_nemu_capture_resolution_detail(
                error,
                NemuCaptureResolutionDetail::Installation,
            )
        })?
        .ok_or_else(|| {
            with_nemu_capture_resolution_detail(
                DeviceError::fatal(
                    "Nemu IPC unavailable: no MuMu installation was discovered; set ACTINGCOMMAND_NEMU_FOLDER or ACTINGCOMMAND_NEMU_IPC_DLL",
                ),
                NemuCaptureResolutionDetail::Installation,
            )
        })?;
    Ok((paths.installation.root, paths.capture_dll_path))
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
    use crate::{DeviceErrorCategory, DeviceErrorSensitivity};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    struct FakeCaptureBackend {
        drops: Rc<Cell<usize>>,
    }

    struct CloseCountingCaptureBackend {
        close_calls: Rc<Cell<usize>>,
    }

    impl CaptureBackend for FakeCaptureBackend {
        fn capture(&mut self) -> DeviceResult<Frame> {
            Err(DeviceError::fatal("fake capture must not run"))
        }

        fn close_once(
            &mut self,
            _authority: DeviceCloseAuthority,
        ) -> DeviceResult<DeviceResourceCloseOutcome> {
            Ok(DeviceResourceCloseOutcome::confirmed(0))
        }
    }

    impl Drop for FakeCaptureBackend {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }

    impl CaptureBackend for CloseCountingCaptureBackend {
        fn capture(&mut self) -> DeviceResult<Frame> {
            Err(DeviceError::fatal("close-only capture must not run"))
        }

        fn close_once(
            &mut self,
            _authority: DeviceCloseAuthority,
        ) -> DeviceResult<DeviceResourceCloseOutcome> {
            self.close_calls.set(self.close_calls.get() + 1);
            Ok(DeviceResourceCloseOutcome::confirmed(1))
        }
    }

    // Task Contract: Workflow #257 / C1B9. Test class: specification criterion.
    #[test]
    fn capture_close_once_reports_acquired_resource_quiescence() {
        let close_calls = Rc::new(Cell::new(0));
        let frame = Frame::from_pixels(
            1,
            1,
            vec![1, 2, 3],
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("primed frame");
        let mut backend = PrimedCaptureBackend {
            inner: Box::new(CloseCountingCaptureBackend {
                close_calls: Rc::clone(&close_calls),
            }),
            primed: Some(frame),
            close_result: None,
        };

        let first = backend
            .close_once(DeviceCloseAuthority::FencedDeviceWrite)
            .expect("first close");
        let second = backend
            .close_once(DeviceCloseAuthority::FencedDeviceWrite)
            .expect("cached close");

        assert_eq!(first, second);
        assert_eq!(first.quiescence(), DeviceResourceQuiescence::Confirmed);
        assert_eq!(first.resource_count(), 2);
        assert_eq!(close_calls.get(), 1);
        assert!(backend.primed.is_none());
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

        let mut config = CaptureBackendConfig::new(
            AdbConfig {
                adb_path: "cached-adb".to_string(),
                command_timeout: Duration::from_millis(1),
            },
            DeviceTarget::default(),
        )
        .with_requested(CaptureBackendChoice::Auto);
        config.nemu.mumu_identity_resolved = true;
        for backend in [
            CaptureBackendName::NemuIpc,
            CaptureBackendName::DroidcastRaw,
        ] {
            capture_probe_cache_store(
                CaptureProbeCacheKey::new(&config, backend),
                &CaptureBackendAttempt::failure(
                    backend,
                    "cached unavailable".to_string(),
                    Some(1),
                    false,
                ),
            )
            .expect("store unavailable cache");
        }
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
        assert_eq!(
            display_size_from_natural(720, 1280, DeviceRotation::R270),
            (1280, 720)
        );
        assert_eq!(
            display_size_from_natural(720, 1280, DeviceRotation::R0),
            (720, 1280)
        );
        assert_eq!(
            display_size_from_natural(720, 1280, DeviceRotation::R180),
            (720, 1280)
        );
        for invalid in ["", "4", "landscape"] {
            let error = parse_device_rotation(invalid).expect_err("invalid rotation");
            assert!(
                error
                    .message()
                    .contains("failed to parse device user_rotation value")
            );
        }
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
    fn explicit_nemu_dll_must_belong_to_configured_installation() {
        let temp = std::env::temp_dir().join(format!(
            "actingcommand-nemu-root-mismatch-{}",
            std::process::id()
        ));
        let configured = temp.join("MuMu Player Configured");
        let other = temp.join("MuMuPlayer-Other");
        let dll = other.join("nx_device/13.7/shell/sdk/external_renderer_ipc.dll");
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&configured).expect("configured root");
        fs::create_dir_all(dll.parent().expect("DLL parent")).expect("DLL parent");
        fs::write(&dll, b"fixture").expect("DLL fixture");

        let err = resolve_nemu_paths(Some(configured.clone()), Some(dll.clone()))
            .expect_err("cross-install DLL must fail");

        assert!(err.message().contains("not selected root"));
        assert!(
            err.message().contains(
                &fs::canonicalize(configured)
                    .expect("canonical root")
                    .display()
                    .to_string()
            )
        );
        assert!(
            err.message().contains(
                &fs::canonicalize(dll)
                    .expect("canonical DLL")
                    .display()
                    .to_string()
            )
        );
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn nemu_installation_resolution_failure_carries_bounded_detail() {
        let config = CaptureBackendConfig::new(
            AdbConfig {
                adb_path: String::new(),
                command_timeout: Duration::from_secs(1),
            },
            DeviceTarget::default(),
        )
        .with_requested(CaptureBackendChoice::NemuIpc);

        let error = prepare_capture_backend_config_with_resolvers(
            config,
            Some(PathBuf::from("coordinated-root")),
            Some(PathBuf::from("coordinated-dll")),
            |configured_adb, explicit_root, explicit_dll| {
                assert_eq!(configured_adb, None);
                assert_eq!(explicit_root, Some(PathBuf::from("coordinated-root")));
                assert_eq!(explicit_dll, Some(PathBuf::from("coordinated-dll")));
                Err(DeviceError::fatal("raw installation resolver detail"))
            },
            |_, _, _, _, _| panic!("installation resolution must not bind a running target"),
        )
        .expect_err("installation resolution failure");

        assert_eq!(error.message(), "raw installation resolver detail");
        assert_eq!(
            error.diagnostic_message(),
            Some("MuMu installation resolution failed")
        );
        let diagnostic = error.diagnostic().expect("installation diagnostic");
        assert_eq!(diagnostic.category(), DeviceErrorCategory::Protocol);
        assert_eq!(diagnostic.stage(), "nemu.installation.resolve");
        let context = error
            .diagnostic_context()
            .expect("installation diagnostic context");
        assert_eq!(context.backend(), "nemu_ipc");
        assert_eq!(context.operation(), "installation_resolve");
        assert_eq!(
            context.declared_sensitivity(),
            DeviceErrorSensitivity::Internal
        );
    }

    #[test]
    fn nemu_capture_identity_failure_carries_bounded_detail() {
        let config = CaptureBackendConfig::new(
            AdbConfig {
                adb_path: "configured-adb".to_string(),
                command_timeout: Duration::from_secs(1),
            },
            DeviceTarget::default(),
        )
        .with_requested(CaptureBackendChoice::Auto);

        let error = prepare_capture_backend_config_with_resolvers(
            config,
            Some(PathBuf::from("coordinated-root")),
            Some(PathBuf::from("coordinated-dll")),
            |configured_adb, _, _| {
                assert_eq!(configured_adb, Some(PathBuf::from("configured-adb")));
                Err(DeviceError::fatal("raw capture identity detail"))
            },
            |_, _, _, _, _| panic!("capture identity resolution must not bind a running target"),
        )
        .expect_err("capture identity failure");

        assert_eq!(error.message(), "raw capture identity detail");
        assert_eq!(
            error.diagnostic_message(),
            Some("Nemu capture identity is not coordinated")
        );
        let diagnostic = error.diagnostic().expect("capture identity diagnostic");
        assert_eq!(diagnostic.category(), DeviceErrorCategory::Protocol);
        assert_eq!(diagnostic.stage(), "nemu.capture.identity");
        let context = error
            .diagnostic_context()
            .expect("capture identity diagnostic context");
        assert_eq!(context.backend(), "nemu_ipc");
        assert_eq!(context.operation(), "capture_identity");
        assert_eq!(
            context.declared_sensitivity(),
            DeviceErrorSensitivity::Internal
        );
    }

    #[test]
    fn nemu_target_resolution_failure_carries_bounded_detail() {
        let config = CaptureBackendConfig::new(
            AdbConfig {
                adb_path: "configured-adb".to_string(),
                command_timeout: Duration::from_secs(1),
            },
            DeviceTarget::default(),
        )
        .with_requested(CaptureBackendChoice::NemuIpc);

        let error = prepare_capture_backend_config_with_resolvers(
            config,
            None,
            None,
            |_, _, _| panic!("running-target resolution must not use installation discovery"),
            |configured_adb, _, instance_id, explicit_root, explicit_dll| {
                assert_eq!(configured_adb, PathBuf::from("configured-adb"));
                assert_eq!(instance_id, None);
                assert_eq!(explicit_root, None);
                assert_eq!(explicit_dll, None);
                Err(DeviceError::fatal("raw running target detail"))
            },
        )
        .expect_err("running target failure");

        assert_eq!(error.message(), "raw running target detail");
        assert_eq!(
            error.diagnostic_message(),
            Some("Nemu running target resolution failed")
        );
        let diagnostic = error.diagnostic().expect("running target diagnostic");
        assert_eq!(diagnostic.category(), DeviceErrorCategory::Protocol);
        assert_eq!(diagnostic.stage(), "nemu.target.resolve");
        let context = error
            .diagnostic_context()
            .expect("running target diagnostic context");
        assert_eq!(context.backend(), "nemu_ipc");
        assert_eq!(context.operation(), "target_resolve");
        assert_eq!(
            context.declared_sensitivity(),
            DeviceErrorSensitivity::Internal
        );
    }

    #[test]
    fn auto_capture_rejects_cross_install_adb_before_fallback() {
        let temp = std::env::temp_dir().join(format!(
            "actingcommand-capture-shared-identity-{}",
            std::process::id()
        ));
        let adb_root = temp.join("MuMu Player A");
        let capture_root = temp.join("MuMuPlayer-B");
        let adb = adb_root.join("nx_main/adb.exe");
        let dll = capture_root.join("nx_device/17.0/shell/sdk/external_renderer_ipc.dll");
        let _ = fs::remove_dir_all(&temp);
        for file in [&adb, &dll] {
            fs::create_dir_all(file.parent().expect("parent")).expect("candidate parent");
            fs::write(file, b"fixture").expect("candidate file");
        }
        let mut config = CaptureBackendConfig::new(
            AdbConfig {
                adb_path: adb.display().to_string(),
                command_timeout: Duration::from_secs(1),
            },
            DeviceTarget::default(),
        )
        .with_requested(CaptureBackendChoice::Auto);
        config.nemu.nemu_folder = Some(capture_root.clone());
        config.nemu.dll_path = Some(dll);

        let err = match create_capture_backend(config) {
            Ok(_) => panic!("cross-installation auto config must fail before fallback"),
            Err(err) => err,
        };

        assert!(err.message().contains("one installation identity"));
        assert!(err.message().contains(&adb_root.display().to_string()));
        assert!(err.message().contains(&capture_root.display().to_string()));
        let _ = fs::remove_dir_all(temp);
    }

    #[cfg(windows)]
    #[test]
    fn explicit_nemu_capture_public_entry_keeps_configured_adb_version() {
        let temp = std::env::temp_dir().join(format!(
            "actingcommand-capture-version-identity-{}",
            std::process::id()
        ));
        let root = temp.join("MuMuPlayer-MultiVersion");
        let old_dll = root.join("nx_device/12.0/shell/sdk/external_renderer_ipc.dll");
        let selected_adb = root.join("nx_device/15.0/shell/adb.exe");
        let selected_dll = root.join("nx_device/15.0/shell/sdk/external_renderer_ipc.dll");
        let _ = fs::remove_dir_all(&temp);
        for file in [&old_dll, &selected_adb, &selected_dll] {
            fs::create_dir_all(file.parent().expect("parent")).expect("candidate parent");
            fs::write(file, b"not a dynamic library").expect("candidate file");
        }
        let config = CaptureBackendConfig::new(
            AdbConfig {
                adb_path: selected_adb.display().to_string(),
                command_timeout: Duration::from_secs(1),
            },
            DeviceTarget::default(),
        )
        .with_requested(CaptureBackendChoice::NemuIpc);

        let err = match create_capture_backend(config) {
            Ok(_) => panic!("invalid fixture DLL must not create a Nemu backend"),
            Err(err) => err,
        };
        let selected_dll = fs::canonicalize(selected_dll).expect("canonical selected DLL");
        let old_dll = fs::canonicalize(old_dll).expect("canonical old DLL");

        assert!(err.message().contains(&selected_dll.display().to_string()));
        assert!(!err.message().contains(&old_dll.display().to_string()));
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn generic_adb_auto_choices_do_not_force_discovered_mumu_pairing() {
        let temp = std::env::temp_dir().join(format!(
            "actingcommand-capture-generic-adb-{}",
            std::process::id()
        ));
        let generic_adb = temp.join("platform-tools/adb.exe");
        let installed_root = temp.join("MuMu Player Installed");
        let installed_dll =
            installed_root.join("nx_device/17.0/shell/sdk/external_renderer_ipc.dll");
        let _ = fs::remove_dir_all(&temp);
        for file in [&generic_adb, &installed_dll] {
            fs::create_dir_all(file.parent().expect("parent")).expect("candidate parent");
            fs::write(file, b"fixture").expect("candidate file");
        }

        for requested in [
            CaptureBackendChoice::Auto,
            CaptureBackendChoice::AutoFastest,
        ] {
            let resolver_called = Cell::new(false);
            let config = CaptureBackendConfig::new(
                AdbConfig {
                    adb_path: generic_adb.display().to_string(),
                    command_timeout: Duration::from_secs(1),
                },
                DeviceTarget::default(),
            )
            .with_requested(requested);
            let prepared = prepare_capture_backend_config_with_resolvers(
                config,
                None,
                None,
                |configured_adb, _, _| {
                    resolver_called.set(true);
                    resolve_mumu_backend_paths(
                        configured_adb,
                        Some(installed_root.clone()),
                        Some(installed_dll.clone()),
                    )
                },
                |_, _, _, _, _| panic!("generic Auto capture must not bind a running MuMu target"),
            )
            .expect("generic ADB must remain available to Auto capture");

            assert!(!resolver_called.get());
            assert_eq!(
                prepared.adb_config.adb_path,
                generic_adb.display().to_string()
            );
            assert!(prepared.nemu.mumu_identity_resolved);
            assert!(
                prepared
                    .nemu
                    .mumu_identity_unavailable
                    .as_deref()
                    .is_some_and(|message| message.contains("not associated"))
            );
        }

        let _ = fs::remove_dir_all(temp);
    }

    // Task Contract: Workflow #239 / #239-IMP-v2 (comment 5442382418).
    // Test class: authorized Defect regression with a preserved first red.
    #[test]
    fn explicit_nemu_capture_resolves_capture_identity_without_rewriting_generic_adb() {
        let temp = std::env::temp_dir().join(format!(
            "actingcommand-capture-explicit-identity-{}",
            std::process::id()
        ));
        let generic_adb = temp.join("platform-tools/adb.exe");
        let root = temp.join("MuMuPlayer");
        let capture_adb = root.join("nx_device/17.0/shell/adb.exe");
        let dll = root.join("nx_device/17.0/shell/sdk/external_renderer_ipc.dll");
        let _ = fs::remove_dir_all(&temp);
        for file in [&generic_adb, &capture_adb, &dll] {
            fs::create_dir_all(file.parent().expect("parent")).expect("candidate parent");
            fs::write(file, b"fixture").expect("candidate file");
        }
        let original_adb = generic_adb.display().to_string();
        let config = CaptureBackendConfig::new(
            AdbConfig {
                adb_path: original_adb.clone(),
                command_timeout: Duration::from_secs(1),
            },
            DeviceTarget::default(),
        )
        .with_requested(CaptureBackendChoice::NemuIpc);
        let resolved_capture_adb = RefCell::new(None);

        let prepared = prepare_capture_backend_config_with_resolvers(
            config,
            Some(root.clone()),
            Some(dll.clone()),
            |configured_adb, explicit_root, explicit_dll| {
                assert_eq!(configured_adb, None);
                let paths =
                    resolve_mumu_backend_paths(configured_adb, explicit_root, explicit_dll)?;
                *resolved_capture_adb.borrow_mut() =
                    paths.as_ref().map(|paths| paths.adb_path.clone());
                Ok(paths)
            },
            |_, _, _, _, _| panic!("complete explicit identity must use the coordinated resolver"),
        )
        .expect("complete capture identity");

        assert_eq!(prepared.adb_config.adb_path, original_adb);
        assert_eq!(
            resolved_capture_adb.into_inner(),
            Some(fs::canonicalize(capture_adb).expect("canonical capture ADB"))
        );
        assert_eq!(
            prepared.nemu.nemu_folder,
            Some(fs::canonicalize(root).expect("canonical root"))
        );
        assert_eq!(
            prepared.nemu.dll_path,
            Some(fs::canonicalize(dll).expect("canonical DLL"))
        );
        assert!(prepared.nemu.mumu_identity_resolved);
        assert_eq!(prepared.nemu.mumu_identity_unavailable, None);
        let _ = fs::remove_dir_all(temp);
    }

    // Task Contract: Workflow #256.
    // Test class: authorized Defect regression.
    #[test]
    fn explicit_nemu_capture_binds_running_target_without_rewriting_generic_adb() {
        let temp = std::env::temp_dir().join(format!(
            "actingcommand-capture-running-target-{}",
            std::process::id()
        ));
        let generic_adb = temp.join("platform-tools/adb.exe");
        let root = temp.join("MuMuPlayer-Selected");
        let dll = root.join("nx_device/18.2/shell/sdk/external_renderer_ipc.dll");
        let _ = fs::remove_dir_all(&temp);
        for file in [&generic_adb, &dll] {
            fs::create_dir_all(file.parent().expect("parent")).expect("fixture parent");
            fs::write(file, b"fixture").expect("fixture file");
        }
        let original_adb = generic_adb.display().to_string();
        let config = CaptureBackendConfig::new(
            AdbConfig {
                adb_path: original_adb.clone(),
                command_timeout: Duration::from_secs(1),
            },
            DeviceTarget {
                serial: Some("127.0.0.1:16416".to_string()),
                ..DeviceTarget::default()
            },
        )
        .with_requested(CaptureBackendChoice::NemuIpc);

        let prepared = prepare_capture_backend_config_with_resolvers(
            config,
            None,
            None,
            |_, _, _| panic!("generic explicit Nemu capture must bind the running target"),
            |configured_adb, target_serial, instance_id, explicit_root, explicit_dll| {
                assert_eq!(configured_adb, generic_adb);
                assert_eq!(target_serial, "127.0.0.1:16416");
                assert_eq!(instance_id, None);
                assert_eq!(explicit_root, None);
                assert_eq!(explicit_dll, None);
                Ok(crate::mumu::MumuBackendPaths {
                    installation: crate::mumu::MumuInstallation {
                        root: fs::canonicalize(&root).expect("canonical root"),
                        source: crate::mumu::MumuInstallSource::RunningProcess,
                    },
                    adb_path: fs::canonicalize(&generic_adb).expect("canonical generic ADB"),
                    capture_dll_path: fs::canonicalize(&dll).expect("canonical DLL"),
                })
            },
        )
        .expect("running target identity");

        assert_eq!(prepared.adb_config.adb_path, original_adb);
        assert_eq!(
            prepared.nemu.nemu_folder,
            Some(fs::canonicalize(root).expect("canonical root"))
        );
        assert_eq!(
            prepared.nemu.dll_path,
            Some(fs::canonicalize(dll).expect("canonical DLL"))
        );
        assert!(prepared.nemu.mumu_identity_resolved);
        assert_eq!(prepared.nemu.mumu_identity_unavailable, None);
        let _ = fs::remove_dir_all(temp);
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
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn rgb8_ids(ids: &[u8]) -> Vec<u8> {
        ids.iter().flat_map(|id| [*id, 0, 0]).collect()
    }

    fn rgb8_red_ids(pixels: &[u8]) -> Vec<u8> {
        pixels
            .as_chunks::<3>()
            .0
            .iter()
            .map(|chunk| chunk[0])
            .collect()
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
