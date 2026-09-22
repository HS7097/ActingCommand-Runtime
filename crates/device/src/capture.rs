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
pub use actingcommand_contract::{
    CaptureAdbDisplayMapping, CaptureBackendName, CaptureExtent, CaptureFrameTransform,
    CaptureGeometry, CaptureGeometryNotApplicable, CaptureGeometryObservation,
    CaptureGeometrySource, CaptureGeometryUnknownReason, CaptureRotation,
    CaptureRotationObservation, CaptureRotationSource, CaptureWmSizeKind,
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

mod nemu_input;
pub use nemu_input::{
    InputCheckPhase, InputExecutionContext, InputOperationCheck, NemuAppIndex,
    NemuApplicationTarget, NemuFrameGeometry, NemuInputConfig, NemuIpcSession, NemuSessionBackends,
};

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
    /// Dimensions already obtained by construction/probing; this getter performs no I/O.
    fn opened_dimensions(&self) -> Option<(u32, u32)> {
        None
    }

    fn capture(&mut self) -> DeviceResult<Frame>;

    /// Time this actual acquisition. Wrappers that return a retained frame must
    /// forward its original measurement, including an unavailable measurement.
    fn capture_timed(&mut self) -> DeviceResult<Frame> {
        let started = Instant::now();
        let result = self.capture();
        let elapsed = Instant::now().checked_duration_since(started);
        result.map(|mut frame| {
            frame.capture_acquire_us =
                elapsed.and_then(|span| u64::try_from(span.as_micros()).ok());
            frame
        })
    }

    /// Read this producer's display geometry within the caller's absolute deadline.
    /// Unsupported implementations report unknown without creating a producer.
    fn observe_geometry(&mut self, _deadline: Instant) -> DeviceResult<CaptureGeometryObservation> {
        Ok(CaptureGeometryObservation::Unknown(
            CaptureGeometryUnknownReason::BackendUnsupported,
        ))
    }

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
#[derive(Debug)]
pub struct Frame {
    /// Open observations attached by the kernel to this request's result only.
    pub backend_open_observations: Vec<crate::BackendOpenObservation>,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub pixel_format: PixelFormat,
    pub original_png: Option<Vec<u8>>,
    pub captured_at: SystemTime,
    pub backend_name: CaptureBackendName,
    /// Context from the same selected producer; decoded or synthetic frames have none.
    pub selection: Option<Arc<CaptureSelectionContext>>,
    /// Observation made by this frame's producer, without an additional capture.
    pub geometry: CaptureGeometryObservation,
    /// The original producer call, excluding selection, retained-frame take and material work.
    capture_acquire_us: Option<u64>,
    /// Dropped after the owned pixel/PNG buffers; never shared by a physical copy.
    memory_charge: Option<crate::FrameMemoryCharge>,
}

impl PartialEq for Frame {
    fn eq(&self, other: &Self) -> bool {
        self.backend_open_observations == other.backend_open_observations
            && self.width == other.width
            && self.height == other.height
            && self.pixels == other.pixels
            && self.pixel_format == other.pixel_format
            && self.original_png == other.original_png
            && self.captured_at == other.captured_at
            && self.backend_name == other.backend_name
            && self.selection == other.selection
            && self.geometry == other.geometry
            && self.capture_acquire_us == other.capture_acquire_us
    }
}

impl Frame {
    pub fn capture_acquire_us(&self) -> Option<u64> {
        self.capture_acquire_us
    }

    pub fn memory_charge(&self) -> Option<&crate::FrameMemoryCharge> {
        self.memory_charge.as_ref()
    }
    pub fn validate_layout(&self) -> DeviceResult<()> {
        validate_pixel_buffer(
            self.width,
            self.height,
            self.pixel_format,
            self.pixels.len(),
        )
    }
    pub fn payload_capacity(&self) -> DeviceResult<u64> {
        (self.pixels.capacity() as u64)
            .checked_add(
                self.original_png
                    .as_ref()
                    .map_or(0, |png| png.capacity() as u64),
            )
            .ok_or_else(|| DeviceError::frame_memory(crate::FrameMemoryFailure::Accounting))
    }

    pub fn admit_memory(&mut self, owner: &crate::FrameMemoryBudget) -> DeviceResult<()> {
        let bytes = self.payload_capacity()?;
        match &self.memory_charge {
            Some(charge) if charge.owner().same_owner(owner) && charge.bytes() == bytes => Ok(()),
            Some(_) => Err(DeviceError::frame_memory(crate::FrameMemoryFailure::Owner)),
            None => {
                self.memory_charge = Some(owner.reserve(bytes)?);
                Ok(())
            }
        }
    }

    pub fn try_clone(&self) -> DeviceResult<Self> {
        self.try_clone_with_budget(self.memory_charge.as_ref().map(|charge| charge.owner()))
    }

    pub fn try_clone_with_budget(
        &self,
        owner: Option<&crate::FrameMemoryBudget>,
    ) -> DeviceResult<Self> {
        if let Some(charge) = &self.memory_charge
            && owner.is_none_or(|owner| !charge.owner().same_owner(owner))
        {
            return Err(DeviceError::frame_memory(crate::FrameMemoryFailure::Owner));
        }
        validate_pixel_buffer(
            self.width,
            self.height,
            self.pixel_format,
            self.pixels.len(),
        )?;
        let charge = owner
            .map(|owner| owner.reserve(self.payload_capacity()?))
            .transpose()?;
        let mut copy = Self {
            backend_open_observations: self.backend_open_observations.clone(),
            width: self.width,
            height: self.height,
            pixels: self.pixels.clone(),
            pixel_format: self.pixel_format,
            original_png: self.original_png.clone(),
            captured_at: self.captured_at,
            backend_name: self.backend_name,
            selection: self.selection.clone(),
            geometry: self.geometry.clone(),
            capture_acquire_us: self.capture_acquire_us,
            memory_charge: charge,
        };
        if let Some(charge) = &mut copy.memory_charge {
            let actual = (copy.pixels.capacity() as u64)
                .checked_add(
                    copy.original_png
                        .as_ref()
                        .map_or(0, |png| png.capacity() as u64),
                )
                .ok_or_else(|| DeviceError::frame_memory(crate::FrameMemoryFailure::Accounting))?;
            if actual > charge.bytes() {
                let mut extra = charge.owner().reserve(actual - charge.bytes())?;
                let bytes = extra.bytes();
                extra.transfer_to(charge, bytes)?;
            } else if actual < charge.bytes() {
                charge.shrink_to(actual)?;
            }
        }
        Ok(copy)
    }

    pub fn retain_admitted_png(
        &mut self,
        png: Vec<u8>,
        workspace: &mut crate::FrameMemoryCharge,
    ) -> DeviceResult<()> {
        if self.original_png.is_some() {
            return Err(DeviceError::frame_memory(
                crate::FrameMemoryFailure::Accounting,
            ));
        }
        let charge = self
            .memory_charge
            .as_mut()
            .ok_or_else(|| DeviceError::frame_memory(crate::FrameMemoryFailure::Owner))?;
        workspace.transfer_to(charge, png.capacity() as u64)?;
        self.original_png = Some(png);
        Ok(())
    }
    pub fn from_png(png: Vec<u8>, backend_name: CaptureBackendName) -> DeviceResult<Self> {
        let (width, height) = parse_png_dimensions(&png)?;
        let image = image::load_from_memory(&png)
            .map_err(|err| DeviceError::fatal(format!("failed to decode PNG frame: {err}")))?
            .to_rgba8();
        Ok(Self {
            width,
            height,
            pixels: image.into_raw(),
            backend_open_observations: Vec::new(),
            capture_acquire_us: None,
            memory_charge: None,
            pixel_format: PixelFormat::Rgba8,
            original_png: Some(png),
            captured_at: SystemTime::now(),
            backend_name,
            selection: None,
            geometry: if backend_name == CaptureBackendName::FixtureSimulation {
                CaptureGeometryObservation::NotApplicable(
                    CaptureGeometryNotApplicable::FixtureSimulation,
                )
            } else {
                CaptureGeometryObservation::Unknown(
                    CaptureGeometryUnknownReason::ProducerObservationAbsent,
                )
            },
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
            backend_open_observations: Vec::new(),
            capture_acquire_us: None,
            memory_charge: None,
            pixel_format,
            original_png: None,
            captured_at: SystemTime::now(),
            backend_name,
            selection: None,
            geometry: if backend_name == CaptureBackendName::FixtureSimulation {
                CaptureGeometryObservation::NotApplicable(
                    CaptureGeometryNotApplicable::FixtureSimulation,
                )
            } else {
                CaptureGeometryObservation::Unknown(
                    CaptureGeometryUnknownReason::ProducerObservationAbsent,
                )
            },
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

    /// Additional live bytes needed by the pinned RGB8/RGBA8 Fast/NoFilter encoder.
    /// image 0.25.10 / png 0.18.1 retain two rows, the fdeflate output and, for
    /// incompressible input, a stored-block replacement. fdeflate 0.3.7 uses
    /// static tables and at most 16 bits per input byte. Six output bounds cover
    /// both growing Vec allocations (including reallocation overlap); the final
    /// PNG below uses a fixed slice. Existing PNG material is borrowed instead.
    pub fn artifact_png_workspace_bytes(&self) -> DeviceResult<u64> {
        if self.original_png.is_some() {
            return Ok(0);
        }
        validate_pixel_buffer(
            self.width,
            self.height,
            self.pixel_format,
            self.pixels.len(),
        )?;
        let row = u64::from(self.width).checked_mul(self.pixel_format.bytes_per_pixel() as u64);
        row.and_then(|row| {
            let raw = row.checked_add(1)?.checked_mul(u64::from(self.height))?;
            let compressed = raw.checked_mul(2)?.checked_add(64)?;
            let output = compressed.checked_add(compressed.div_ceil(2_147_483_647) * 12 + 128)?;
            compressed
                .checked_mul(6)?
                .checked_add(row.checked_mul(2)? + 1)?
                .checked_add(output)
        })
        .ok_or_else(|| DeviceError::fatal("frame PNG workspace size overflow"))
    }

    /// Uses only the caller's admitted workspace; never grows the output buffer.
    pub fn png_for_artifact_with_budget(
        &self,
        workspace_bytes: u64,
    ) -> DeviceResult<std::borrow::Cow<'_, [u8]>> {
        if let Some(png) = &self.original_png {
            return Ok(std::borrow::Cow::Borrowed(png));
        }
        let required = self.artifact_png_workspace_bytes()?;
        if required > workspace_bytes {
            return Err(DeviceError::fatal("frame PNG workspace was not admitted"));
        }
        let row = u64::from(self.width) * self.pixel_format.bytes_per_pixel() as u64;
        let compressed = (row + 1) * u64::from(self.height) * 2 + 64;
        let output = compressed + compressed.div_ceil(2_147_483_647) * 12 + 128;
        let length = usize::try_from(output)
            .map_err(|_| DeviceError::fatal("frame PNG output exceeds address space"))?;
        let mut png = Vec::new();
        png.try_reserve_exact(length)
            .map_err(|_| DeviceError::fatal("frame PNG workspace allocation failed"))?;
        png.resize(length, 0);
        let written = {
            let mut destination = std::io::Cursor::new(png.as_mut_slice());
            PngEncoder::new_with_quality(
                &mut destination,
                CompressionType::Fast,
                FilterType::NoFilter,
            )
            .write_image(
                &self.pixels,
                self.width,
                self.height,
                self.pixel_format.color_type().into(),
            )
            .map_err(|error| {
                DeviceError::fatal(format!("failed to encode bounded frame PNG: {error}"))
            })?;
            destination.position() as usize
        };
        png.truncate(written);
        Ok(std::borrow::Cow::Owned(png))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureMumuContext {
    pub root: PathBuf,
    pub adb_path: PathBuf,
    pub capture_dll_path: PathBuf,
    pub source: crate::MumuInstallSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureSelectionContext {
    pub requested: CaptureBackendChoice,
    pub configured_adb: String,
    pub configured_serial: Option<String>,
    pub resolved_adb: String,
    pub selected_serial: String,
    pub mumu: Option<CaptureMumuContext>,
    pub nemu_frame: Option<Arc<NemuFrameGeometry>>,
}

#[derive(Debug, Clone)]
pub struct CaptureBackendConfig {
    pub adb_config: AdbConfig,
    pub target: DeviceTarget,
    pub requested: CaptureBackendChoice,
    pub capture_timeout: Duration,
    pub droidcast: DroidcastRawConfig,
    pub nemu: NemuIpcConfig,
    pub resolved_mumu: Option<CaptureMumuContext>,
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
            resolved_mumu: None,
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
    pub selection: Option<Arc<CaptureSelectionContext>>,
}

impl CaptureBackend for SelectedCaptureBackend {
    fn capture_timed(&mut self) -> DeviceResult<Frame> {
        self.capture()
    }

    fn observe_geometry(&mut self, deadline: Instant) -> DeviceResult<CaptureGeometryObservation> {
        self.backend.observe_geometry(deadline)
    }

    fn capture(&mut self) -> DeviceResult<Frame> {
        let mut frame = self.backend.capture_timed()?;
        if let Some(selection) = &self.selection {
            let mut selection = selection.as_ref().clone();
            selection.nemu_frame = frame
                .selection
                .as_ref()
                .and_then(|value| value.nemu_frame.clone());
            frame.selection = Some(Arc::new(selection));
        }
        Ok(frame)
    }

    fn close_once(
        &mut self,
        authority: DeviceCloseAuthority,
    ) -> DeviceResult<DeviceResourceCloseOutcome> {
        self.backend.close_once(authority)
    }

    fn vendor_stdio(&self) -> &[VendorStdioCapture] {
        self.backend.vendor_stdio()
    }
}

pub fn create_capture_backend(
    config: CaptureBackendConfig,
) -> DeviceResult<SelectedCaptureBackend> {
    create_capture_backend_with_memory(config, None)
}

pub fn create_capture_backend_with_memory(
    config: CaptureBackendConfig,
    memory: Option<&crate::FrameMemoryBudget>,
) -> DeviceResult<SelectedCaptureBackend> {
    let configured_adb = config.adb_config.adb_path.clone();
    let configured_serial = config.target.serial.clone();
    let config = prepare_capture_backend_config(config)?;
    let selection = CaptureSelectionContext {
        requested: config.requested,
        configured_adb,
        configured_serial,
        resolved_adb: config.adb_config.adb_path.clone(),
        selected_serial: config.target.resolved_serial(),
        mumu: config.resolved_mumu.clone(),
        nemu_frame: None,
    };
    let mut selected = match config.requested {
        CaptureBackendChoice::Auto => create_auto_capture_backend(config, memory),
        CaptureBackendChoice::AutoFastest => create_auto_fastest_capture_backend(config, memory),
        CaptureBackendChoice::Adb => {
            let used = CaptureBackendName::AdbScreencap;
            Ok(SelectedCaptureBackend {
                selection: None,
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
    }?;
    selected.selection = Some(Arc::new(selection));
    Ok(selected)
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
                    config.resolved_mumu = Some(CaptureMumuContext {
                        root: paths.installation.root.clone(),
                        adb_path: paths.adb_path.clone(),
                        capture_dll_path: paths.capture_dll_path.clone(),
                        source: paths.installation.source,
                    });
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
        selection: None,
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
    memory: Option<&crate::FrameMemoryBudget>,
) -> DeviceResult<SelectedCaptureBackend> {
    create_auto_capture_backend_with_mode(config, AutoCaptureMode::Priority, memory)
}

fn create_auto_fastest_capture_backend(
    config: CaptureBackendConfig,
    memory: Option<&crate::FrameMemoryBudget>,
) -> DeviceResult<SelectedCaptureBackend> {
    create_auto_capture_backend_with_mode(config, AutoCaptureMode::Fastest, memory)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutoCaptureMode {
    Priority,
    Fastest,
}

fn create_auto_capture_backend_with_mode(
    config: CaptureBackendConfig,
    mode: AutoCaptureMode,
    memory: Option<&crate::FrameMemoryBudget>,
) -> DeviceResult<SelectedCaptureBackend> {
    select_auto_capture_backend_with_probe(mode, AUTO_CAPTURE_BACKEND_ORDER, |name| {
        probe_or_cached_capture_backend(&config, name, memory)
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
                let mut report =
                    crate::backend_open::capture_open_report(requested, None, &attempts);
                if let Some(check) = primary.capture_probe_check() {
                    check.apply_failure(&mut report, &primary);
                }
                return Err(crate::observe_open_failure(
                    report,
                    close_capture_candidates(
                        successful,
                        primary.with_resource_candidate_index(candidate_index),
                    ),
                ));
            }
        };
        match probe_outcome {
            CaptureProbeOutcome::Available(backend, attempt, elapsed_ms) => {
                attempts.push(attempt);
                if mode == AutoCaptureMode::Priority {
                    return Ok(SelectedCaptureBackend {
                        selection: None,
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
            return Err(crate::observe_open_failure(
                crate::backend_open::capture_open_report(requested, Some(used), &attempts),
                match backend.close_once(DeviceCloseAuthority::LocalOnly) {
                    Ok(outcome) => primary.with_stdio_observations(outcome.vendor_stdio()),
                    Err(winner_cleanup) => {
                        if winner_cleanup.resource_quiescence()
                            == Some(DeviceResourceQuiescence::Unconfirmed)
                        {
                            std::mem::forget(backend);
                        }
                        primary.merge_resource_cleanup(winner_cleanup)
                    }
                },
            ));
        }
        return Ok(SelectedCaptureBackend {
            selection: None,
            backend,
            diagnostics: CaptureBackendDiagnostics {
                requested,
                used,
                attempts,
            },
        });
    }

    Err(crate::observe_open_failure(
        crate::backend_open::capture_open_report(requested, None, &attempts),
        DeviceError::fatal(format!(
            "{} capture backend selection failed; attempts: {}",
            requested.as_str(),
            format_backend_attempts(&attempts)
        )),
    ))
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
            Ok(outcome) => primary.with_stdio_observations(outcome.vendor_stdio()),
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
    memory: Option<&crate::FrameMemoryBudget>,
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
        Ok(backend) => match prime_capture_backend(name, backend, memory) {
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
                if error.diagnostic().is_some_and(|diagnostic| {
                    diagnostic.category() == crate::DeviceErrorCategory::FrameLayout
                }) || error.frame_memory_failure().is_some()
                    || error.resource_quiescence().is_some()
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
        Ok(outcome) => primary.with_stdio_observations(outcome.vendor_stdio()),
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
    fn capture_timed(&mut self) -> DeviceResult<Frame> {
        self.capture()
    }

    fn opened_dimensions(&self) -> Option<(u32, u32)> {
        self.primed
            .as_ref()
            .map(|frame| (frame.width, frame.height))
    }

    fn observe_geometry(&mut self, deadline: Instant) -> DeviceResult<CaptureGeometryObservation> {
        require_geometry_open(&self.close_result)?;
        self.inner.observe_geometry(deadline)
    }

    fn capture(&mut self) -> DeviceResult<Frame> {
        if let Some(frame) = self.primed.take() {
            return Ok(frame);
        }
        self.inner.capture_timed()
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
    memory: Option<&crate::FrameMemoryBudget>,
) -> DeviceResult<PrimedCaptureResult> {
    let Some(memory) = memory else {
        return Err(close_capture_backend_after_error(
            backend,
            DeviceError::frame_memory(crate::FrameMemoryFailure::Owner),
        ));
    };
    let captured =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| backend.capture_timed()))
            .unwrap_or_else(|_| Err(DeviceError::fatal("capture probe panicked")));
    match captured {
        Ok(mut frame) => {
            let layout = frame.validate_layout();
            let check = if layout.is_ok() {
                crate::backend_open::CaptureProbeCheck::Passed {
                    backend: name,
                    width: frame.width,
                    height: frame.height,
                }
            } else {
                crate::backend_open::CaptureProbeCheck::Failed { backend: name }
            };
            if let Err(primary) = layout.and_then(|()| frame.admit_memory(memory)) {
                return Err(close_capture_backend_after_error(
                    backend,
                    primary.with_capture_probe_check(check),
                ));
            }
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
        Err(primary) => {
            let primary =
                primary.with_capture_probe_check(crate::backend_open::CaptureProbeCheck::Failed {
                    backend: name,
                });
            match backend.close_once(DeviceCloseAuthority::LocalOnly) {
                Ok(outcome) => Err(primary.with_stdio_observations(outcome.vendor_stdio())),
                Err(cleanup) => {
                    if cleanup.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed)
                    {
                        std::mem::forget(backend);
                    }
                    Err(primary.merge_resource_cleanup(cleanup))
                }
            }
        }
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
    fn observe_geometry(&mut self, deadline: Instant) -> DeviceResult<CaptureGeometryObservation> {
        require_geometry_open(&self.close_result)?;
        read_adb_capture_geometry(
            &Adb::new(self.adb_config.clone()),
            &self.target.resolved_serial(),
            CaptureBackendName::AdbScreencap,
            deadline,
        )
    }

    fn capture(&mut self) -> DeviceResult<Frame> {
        let serial = self.target.resolved_serial();
        let adb = Adb::new(self.adb_config.clone());
        verify_adb_device(
            &adb,
            &self.target,
            &serial,
            CaptureBackendName::AdbScreencap,
        )?;

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

    fn start_if_needed(&mut self) -> DeviceResult<(u32, u32, CaptureWmSizeKind)> {
        let adb = Adb::new(self.adb_config.clone());
        verify_adb_device(
            &adb,
            &self.target,
            &self.serial,
            CaptureBackendName::DroidcastRaw,
        )?;
        let wm_output = adb.screen_size(&self.serial)?;
        let (width, height) = parse_screen_size(&wm_output)?;
        let selected_token = wm_output.split_whitespace().find(|part| part.contains('x'));
        let selected_label = selected_token
            .and_then(|token| wm_output.lines().find(|line| line.contains(token)))
            .and_then(|line| line.split_once(':').map(|(label, _)| label))
            .unwrap_or("");
        let size_kind = wm_size_kind(selected_label);
        if self.started {
            return Ok((width, height, size_kind));
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
        Ok((width, height, size_kind))
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
    fn observe_geometry(&mut self, deadline: Instant) -> DeviceResult<CaptureGeometryObservation> {
        require_geometry_open(&self.close_result)?;
        if !self.started {
            return Ok(CaptureGeometryObservation::Unknown(
                CaptureGeometryUnknownReason::ProducerUnavailable,
            ));
        }
        read_adb_capture_geometry(
            &Adb::new(self.adb_config.clone()),
            &self.serial,
            CaptureBackendName::DroidcastRaw,
            deadline,
        )
    }

    fn capture(&mut self) -> DeviceResult<Frame> {
        let (natural_width, natural_height, size_kind) = self.start_if_needed()?;
        let (rotation, rotation_source) = read_device_rotation_with_source(
            &Adb::new(self.adb_config.clone()),
            &self.serial,
            None,
        )?;
        let geometry_sampled_at = SystemTime::now();
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
        let mut frame = Frame::from_pixels(
            frame_width,
            frame_height,
            pixels,
            PixelFormat::Rgb8,
            CaptureBackendName::DroidcastRaw,
        )?;
        let transform = if decode_width == display_width && decode_height == display_height {
            CaptureFrameTransform::Identity
        } else if rotation == DeviceRotation::R270 {
            CaptureFrameTransform::RotateCounterclockwise90
        } else {
            CaptureFrameTransform::RotateClockwise90
        };
        frame.geometry = CaptureGeometryObservation::Observed(CaptureGeometry {
            backend: CaptureBackendName::DroidcastRaw,
            source: CaptureGeometrySource::AdbDefaultDisplay {
                serial: self.serial.clone(),
                wm_extent: geometry_extent(natural_width, natural_height)?,
                wm_size_kind: size_kind,
            },
            logical_display_extent: geometry_extent(display_width, display_height)?,
            rotation: observed_rotation(rotation, rotation_source),
            sampled_at: geometry_sampled_at,
            frame_transform: Some(transform),
        });
        Ok(frame)
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
type NemuCaptureDisplay = unsafe extern "C" fn(i32, u32, i32, *mut i32, *mut i32, *mut u8) -> i32;

impl NemuIpcBackend {
    pub fn new(
        target: DeviceTarget,
        config: NemuIpcConfig,
        capture_timeout: Duration,
    ) -> DeviceResult<Self> {
        Self::new_with_input(target, config, capture_timeout, None)
    }

    fn new_with_input(
        target: DeviceTarget,
        config: NemuIpcConfig,
        capture_timeout: Duration,
        input: Option<nemu_input::NemuInputState>,
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
            input,
        );
        let (frame_width, frame_height) = match worker.probe_resolution() {
            Ok(resolution) => resolution,
            Err(primary) => {
                return match worker.shutdown_once(DeviceCloseAuthority::LocalOnly) {
                    Ok(outcome) => Err(primary.with_stdio_observations(outcome.vendor_stdio())),
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
    ObserveGeometry {
        deadline: Instant,
        response: mpsc::Sender<DeviceResult<CaptureGeometryObservation>>,
    },
    Capture(mpsc::Sender<DeviceResult<NemuCapturedFrame>>),
    InputTap {
        x: i32,
        y: i32,
        context: InputExecutionContext,
        response: mpsc::Sender<DeviceResult<()>>,
    },
    InputSegmented {
        plan: crate::PreparedSegmentedSwipePlan,
        context: InputExecutionContext,
        response: mpsc::Sender<DeviceResult<()>>,
    },
    InvalidateDisplay(mpsc::Sender<DeviceResult<()>>),
    Shutdown {
        authority: DeviceCloseAuthority,
        input_check: Option<Arc<dyn InputOperationCheck>>,
        response: mpsc::Sender<DeviceResult<DeviceResourceCloseOutcome>>,
    },
}

struct NemuCapturedFrame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    vendor_stdio: Vec<VendorStdioCapture>,
    input_geometry: Option<Arc<NemuFrameGeometry>>,
    geometry: CaptureGeometryObservation,
}

struct NemuIpcWorker {
    tx: mpsc::Sender<NemuIpcCommand>,
    handle: Option<JoinHandle<DeviceResult<()>>>,
    timeout: Duration,
    poisoned: Arc<AtomicBool>,
    close_result: Option<DeviceResult<DeviceResourceCloseOutcome>>,
}

impl NemuIpcWorker {
    fn spawn(
        nemu_folder: PathBuf,
        dll_path: PathBuf,
        instance_id: i32,
        display_id: i32,
        timeout: Duration,
        input: Option<nemu_input::NemuInputState>,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let poisoned = Arc::new(AtomicBool::new(false));
        let worker_poisoned = Arc::clone(&poisoned);
        let handle = thread::spawn(move || {
            let mut state =
                NemuIpcWorkerState::load(nemu_folder, dll_path, instance_id, display_id, input);
            let mut closed = false;
            let mut geometry_owner_retained = false;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                while let Ok(command) = rx.recv() {
                    match command {
                        NemuIpcCommand::ObserveGeometry { deadline, response } => {
                            let result = worker_state_result(&mut state, |state| {
                                state.observe_geometry(deadline)
                            });
                            if result.as_ref().is_err_and(|error| {
                                error.resource_quiescence()
                                    == Some(DeviceResourceQuiescence::Unconfirmed)
                            }) {
                                geometry_owner_retained = true;
                            }
                            if let Err(undelivered) = response.send(result) {
                                geometry_owner_retained = true;
                                let unavailable = nemu_geometry_unconfirmed(
                                    "Nemu IPC geometry response was not received before its deadline",
                                );
                                return Err(match undelivered.0 {
                                    Err(primary) => primary.merge_resource_cleanup(unavailable),
                                    Ok(_) => unavailable,
                                });
                            }
                        }
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
                        NemuIpcCommand::InputTap {
                            x,
                            y,
                            context,
                            response,
                        } => {
                            let result = worker_state_result(&mut state, |state| {
                                state.input_tap(x, y, &context, &worker_poisoned)
                            });
                            drop(context);
                            response
                                .send(result)
                                .map_err(|_| DeviceError::fatal("Nemu IPC input response lost"))?;
                        }
                        NemuIpcCommand::InputSegmented {
                            plan,
                            context,
                            response,
                        } => {
                            let result = worker_state_result(&mut state, |state| {
                                state.input_segmented(&plan, &context, &worker_poisoned)
                            });
                            drop(context);
                            response
                                .send(result)
                                .map_err(|_| DeviceError::fatal("Nemu IPC input response lost"))?;
                        }
                        NemuIpcCommand::InvalidateDisplay(response) => {
                            response
                                .send(worker_state_result(&mut state, |state| {
                                    state.invalidate_display()
                                }))
                                .map_err(|_| {
                                    DeviceError::fatal(
                                        "Nemu IPC display invalidation response lost",
                                    )
                                })?;
                        }
                        NemuIpcCommand::Shutdown {
                            authority,
                            input_check,
                            response,
                        } => {
                            let result = worker_state_result(&mut state, |state| {
                                state.close_input_contact(
                                    authority.clone(),
                                    input_check.as_deref(),
                                    &worker_poisoned,
                                )?;
                                state.close(authority)
                            });
                            drop(input_check);
                            closed = true;
                            if response.send(result.clone()).is_err() {
                                return Err(match result {
                                    Ok(outcome) => {
                                        DeviceError::fatal("Nemu IPC close response lost")
                                            .with_stdio_observations(outcome.vendor_stdio())
                                    }
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
            let result = if closed || geometry_owner_retained {
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
                    (Err(primary), Ok(outcome)) => {
                        Err(primary.with_stdio_observations(outcome.vendor_stdio()))
                    }
                    (Ok(()), Err(cleanup)) => Err(cleanup),
                    (Err(primary), Err(cleanup)) => Err(primary.merge_resource_cleanup(cleanup)),
                }
            };
            if (geometry_owner_retained && !closed)
                || result.as_ref().is_err_and(|error| {
                    error.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed)
                })
            {
                std::mem::forget(state);
            }
            result
        });
        Self {
            tx,
            handle: Some(handle),
            timeout,
            poisoned,
            close_result: None,
        }
    }

    fn probe_resolution(&mut self) -> DeviceResult<(u32, u32)> {
        self.request(NemuIpcCommand::Probe)
    }

    fn observe_geometry(&mut self, deadline: Instant) -> DeviceResult<CaptureGeometryObservation> {
        require_geometry_open(&self.close_result)?;
        let deadline = Instant::now()
            .checked_add(self.timeout)
            .map_or(deadline, |worker_deadline| deadline.min(worker_deadline));
        let remaining = geometry_remaining(deadline)?;
        if self.poisoned.load(Ordering::Acquire) {
            return Err(nemu_geometry_unconfirmed(
                "Nemu IPC backend is poisoned after a previous timeout",
            ));
        }
        if self.handle.is_none() {
            return Err(DeviceError::fatal("Nemu IPC worker is unavailable"));
        }
        let (response, result) = mpsc::channel();
        self.tx
            .send(NemuIpcCommand::ObserveGeometry { deadline, response })
            .map_err(|error| {
                DeviceError::fatal(format!("failed to send Nemu IPC worker command: {error}"))
            })?;
        match result.recv_timeout(remaining.min(self.timeout)) {
            Ok(result) => {
                if result.as_ref().is_err_and(|error| {
                    error.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed)
                }) {
                    self.poisoned.store(true, Ordering::Release);
                }
                if Instant::now() >= deadline {
                    self.poisoned.store(true, Ordering::Release);
                    return match result {
                        Err(error) => Err(error.merge_resource_cleanup(nemu_geometry_unconfirmed(
                            "Nemu IPC geometry reply arrived after its deadline",
                        ))),
                        Ok(_) => Err(nemu_geometry_unconfirmed(
                            "Nemu IPC geometry reply arrived after its deadline",
                        )),
                    };
                }
                result
            }
            Err(error) => {
                self.poisoned.store(true, Ordering::Release);
                Err(nemu_geometry_unconfirmed(format!(
                    "Nemu IPC geometry worker response unavailable: {error}"
                )))
            }
        }
    }

    fn capture_frame(&mut self) -> DeviceResult<NemuCapturedFrame> {
        self.request(NemuIpcCommand::Capture)
    }

    fn request<T: Send + 'static>(
        &mut self,
        command: impl FnOnce(mpsc::Sender<DeviceResult<T>>) -> NemuIpcCommand,
    ) -> DeviceResult<T> {
        self.request_with_timeout(command, self.timeout)
    }

    fn request_with_timeout<T: Send + 'static>(
        &mut self,
        command: impl FnOnce(mpsc::Sender<DeviceResult<T>>) -> NemuIpcCommand,
        timeout: Duration,
    ) -> DeviceResult<T> {
        if self.poisoned.load(Ordering::Acquire) {
            return Err(DeviceError::fatal(
                "Nemu IPC backend is poisoned after a previous timeout",
            ));
        }

        let (tx, rx) = mpsc::channel();
        self.tx.send(command(tx)).map_err(|err| {
            DeviceError::fatal(format!("failed to send Nemu IPC worker command: {err}"))
        })?;
        match rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.poisoned.store(true, Ordering::Release);
                Err(DeviceError::fatal(format!(
                    "Nemu IPC worker timed out after {:?}; backend marked poisoned and will not be reused",
                    timeout
                )))
            }
            Err(err) => {
                self.poisoned.store(true, Ordering::Release);
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
        self.shutdown_with_input_check(authority, None)
    }

    fn shutdown_with_input_check(
        &mut self,
        authority: DeviceCloseAuthority,
        input_check: Option<Arc<dyn InputOperationCheck>>,
    ) -> DeviceResult<DeviceResourceCloseOutcome> {
        if let Some(result) = &self.close_result {
            return result.clone();
        }
        if self.poisoned.load(Ordering::Acquire) {
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
                input_check,
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
            (Ok(outcome), Err(join)) => Err(join.with_stdio_observations(outcome.vendor_stdio())),
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
    input: Option<nemu_input::NemuInputState>,
}

impl NemuIpcWorkerState {
    fn load(
        nemu_folder: PathBuf,
        dll_path: PathBuf,
        instance_id: i32,
        display_id: i32,
        input: Option<nemu_input::NemuInputState>,
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
            input,
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
        if connect_id <= 0 {
            return Err(DeviceError::fatal(
                "Nemu IPC connect did not return a positive handle; check MuMu path and running instance",
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
        self.probe_resolution_with_input(None)
    }

    fn probe_resolution_with_input(
        &mut self,
        context: Option<(&InputExecutionContext, &AtomicBool)>,
    ) -> DeviceResult<(u32, u32)> {
        if let Some((context, stopped)) = context {
            nemu_input::input_check(context.check.as_ref(), InputCheckPhase::Continue, stopped)?;
        }
        self.connect()?;
        self.resolve_input_display(context)?;
        let capture_display =
            unsafe { self.symbol::<NemuCaptureDisplay>(b"nemu_capture_display\0")? };
        let mut width = 0i32;
        let mut height = 0i32;
        let connect_id = self.connect_id;
        let display_id = u32::try_from(self.display_id)
            .map_err(|_| DeviceError::fatal("Nemu IPC display id is negative"))?;
        let width_ptr = &mut width as *mut i32;
        let height_ptr = &mut height as *mut i32;
        if let Some((context, stopped)) = context {
            nemu_input::input_check(context.check.as_ref(), InputCheckPhase::Continue, stopped)?;
        }
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
        if ret != 0 {
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

    fn observe_geometry(&mut self, deadline: Instant) -> DeviceResult<CaptureGeometryObservation> {
        geometry_remaining(deadline)?;
        if self.connect_id <= 0 {
            return Ok(CaptureGeometryObservation::Unknown(
                CaptureGeometryUnknownReason::ProducerUnavailable,
            ));
        }
        let resolution = self.probe_resolution();
        let sampled_at = SystemTime::now();
        if Instant::now() >= deadline {
            let unavailable =
                nemu_geometry_unconfirmed("Nemu IPC geometry probe returned after its deadline");
            return Err(match resolution {
                Err(primary) => primary.merge_resource_cleanup(unavailable),
                Ok(_) => unavailable,
            });
        }
        let (width, height) = resolution?;
        self.geometry_observation(width, height, sampled_at, None)
    }

    fn geometry_observation(
        &self,
        width: u32,
        height: u32,
        sampled_at: SystemTime,
        frame_transform: Option<CaptureFrameTransform>,
    ) -> DeviceResult<CaptureGeometryObservation> {
        Ok(CaptureGeometryObservation::Observed(CaptureGeometry {
            backend: CaptureBackendName::NemuIpc,
            source: CaptureGeometrySource::NemuSdkDisplay {
                sdk_instance_id: self.instance_id,
                sdk_display_id: self.display_id,
                adb_display_mapping: CaptureAdbDisplayMapping::Unproven,
            },
            logical_display_extent: geometry_extent(width, height)?,
            rotation: CaptureRotationObservation::NotProvidedBySource,
            sampled_at,
            frame_transform,
        }))
    }

    fn capture_frame(&mut self) -> DeviceResult<NemuCapturedFrame> {
        let (width, height) = self.probe_resolution()?;
        let geometry_sampled_at = SystemTime::now();
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
        let display_id = u32::try_from(self.display_id)
            .map_err(|_| DeviceError::fatal("Nemu IPC display id is negative"))?;
        let width_ptr = &mut width_i32 as *mut i32;
        let height_ptr = &mut height_i32 as *mut i32;
        let buffer_ptr = self.raw_buffer.as_mut_ptr();
        let ret = unsafe {
            capture_display(
                connect_id, display_id, length, width_ptr, height_ptr, buffer_ptr,
            )
        };
        self.record_vendor_stdio_snapshot()?;
        if ret != 0 {
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
            input_geometry: self.input_geometry(width, height)?,
            geometry: self.geometry_observation(
                width,
                height,
                geometry_sampled_at,
                Some(CaptureFrameTransform::FlipVertical),
            )?,
        })
    }

    fn disconnect(
        &mut self,
        authority: DeviceCloseAuthority,
        resolve: impl FnOnce(&Self) -> DeviceResult<NemuDisconnect>,
    ) -> DeviceResult<()> {
        if self.connect_id <= 0 {
            return Ok(());
        }
        if authority.resource_close_witness().is_none() {
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
        let disconnect = resolve(self).map_err(|error| {
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
        // The serial worker observed the call return. Retire only this owned opaque handle;
        // owned-resource quiescence still requires stdio, library and worker completion.
        self.connect_id = 0;
        self.record_vendor_stdio_snapshot().map_err(|error| {
            error.with_resource_close_cause(
                DeviceResourceKind::VendorStdio,
                DeviceResourceClosePhase::SnapshotRead,
                "nemu_ipc",
                None,
                Some(self.instance_id),
                DeviceResourceQuiescence::Unconfirmed,
                1,
            )
        })
    }

    fn close(
        &mut self,
        authority: DeviceCloseAuthority,
    ) -> DeviceResult<DeviceResourceCloseOutcome> {
        let mut resource_count = u16::from(self.connect_id > 0)
            .saturating_add(u16::from(self.stdio_session.is_some()))
            .saturating_add(u16::from(self.library.is_some()));
        self.disconnect(authority, |state| unsafe {
            state.symbol::<NemuDisconnect>(b"nemu_disconnect\0")
        })?;
        let mut failure = None;
        let mut stdio_observations = Vec::new();
        if let Some(stdio) = self.stdio_session.as_mut() {
            match stdio.finish() {
                Ok(outcome) => {
                    resource_count = resource_count.max(outcome.resource_count());
                    stdio_observations.extend_from_slice(outcome.vendor_stdio());
                }
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
            }
            .with_stdio_observations(&stdio_observations));
        }
        failure.map_or(
            Ok(DeviceResourceCloseOutcome::confirmed(resource_count)
                .with_stdio_observations(&stdio_observations)),
            |error| {
                Err(error
                    .with_stdio_observations(&stdio_observations)
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
    fn opened_dimensions(&self) -> Option<(u32, u32)> {
        Some((self.frame_width, self.frame_height))
    }

    fn observe_geometry(&mut self, deadline: Instant) -> DeviceResult<CaptureGeometryObservation> {
        require_geometry_open(&self.close_result)?;
        self.worker
            .as_mut()
            .ok_or_else(|| DeviceError::fatal("Nemu IPC worker is unavailable"))?
            .observe_geometry(deadline)
    }

    fn capture(&mut self) -> DeviceResult<Frame> {
        let worker = self
            .worker
            .as_mut()
            .ok_or_else(|| DeviceError::fatal("Nemu IPC worker is unavailable"))?;
        let frame = worker.capture_frame()?;
        self.frame_width = frame.width;
        self.frame_height = frame.height;
        self.vendor_stdio = frame.vendor_stdio.clone();
        let mut captured = Frame::from_pixels(
            frame.width,
            frame.height,
            frame.pixels,
            PixelFormat::Rgba8,
            CaptureBackendName::NemuIpc,
        )?;
        captured.geometry = frame.geometry;
        if let Some(geometry) = frame.input_geometry {
            captured.selection = Some(Arc::new(CaptureSelectionContext {
                requested: CaptureBackendChoice::NemuIpc,
                configured_adb: String::new(),
                configured_serial: None,
                resolved_adb: String::new(),
                selected_serial: String::new(),
                mumu: None,
                nemu_frame: Some(geometry),
            }));
        }
        Ok(captured)
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

fn verify_adb_device(
    adb: &Adb,
    target: &DeviceTarget,
    serial: &str,
    backend: CaptureBackendName,
) -> DeviceResult<()> {
    adb.ensure_device(serial, target.connect)
        .map(|_| ())
        .map_err(|error| {
            error.with_diagnostic_context_if_absent(
                backend.as_str(),
                "ensure_device",
                DeviceErrorSensitivity::Sensitive,
            )
        })
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
    if width == 0 || height == 0 {
        return Err(DeviceError::fatal("frame dimensions must be nonzero")
            .with_diagnostic(DeviceErrorCategory::FrameLayout, "capture.frame_layout"));
    }
    let expected = checked_pixel_len(width, height, pixel_format).map_err(|error| {
        error.with_diagnostic(DeviceErrorCategory::FrameLayout, "capture.frame_layout")
    })?;
    if len != expected {
        return Err(DeviceError::fatal(format!(
            "frame pixel buffer length mismatch for {}x{} {}: got {}, expected {}",
            width,
            height,
            pixel_format.as_str(),
            len,
            expected
        ))
        .with_diagnostic(DeviceErrorCategory::FrameLayout, "capture.frame_layout"));
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
    read_device_rotation_with_source(adb, serial, None).map(|(rotation, _)| rotation)
}

fn read_device_rotation_with_source(
    adb: &Adb,
    serial: &str,
    deadline: Option<Instant>,
) -> DeviceResult<(DeviceRotation, CaptureRotationSource)> {
    let run = |args: &[&str]| match deadline {
        Some(deadline) => adb.run_until(args, deadline),
        None => adb.run(args),
    };
    let output = run(&["-s", serial, "shell", "dumpsys", "display"])?;
    if let Some(rotation) = parse_display_orientation(&output.stdout)? {
        return Ok((rotation, CaptureRotationSource::DumpsysDisplayOrientation));
    }
    let output = run(&[
        "-s",
        serial,
        "shell",
        "settings",
        "get",
        "system",
        "user_rotation",
    ])?;
    parse_device_rotation(&output.stdout)
        .map(|rotation| (rotation, CaptureRotationSource::UserRotation))
}

fn require_geometry_open(
    close_result: &Option<DeviceResult<DeviceResourceCloseOutcome>>,
) -> DeviceResult<()> {
    match close_result {
        None => Ok(()),
        Some(Err(error)) => Err(error.clone()),
        Some(Ok(_)) => Err(DeviceError::fatal(
            "capture geometry is unavailable after producer close",
        )),
    }
}

fn nemu_geometry_unconfirmed(message: impl Into<String>) -> DeviceError {
    DeviceError::fatal(message).with_resource_close_cause(
        DeviceResourceKind::InProcessWorker,
        DeviceResourceClosePhase::WorkerReceive,
        "nemu_ipc",
        None,
        None,
        DeviceResourceQuiescence::Unconfirmed,
        1,
    )
}

fn geometry_remaining(deadline: Instant) -> DeviceResult<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| DeviceError::fatal("capture geometry deadline expired"))
}

fn geometry_extent(width: u32, height: u32) -> DeviceResult<CaptureExtent> {
    CaptureExtent::new(width, height)
        .ok_or_else(|| DeviceError::fatal("capture geometry contains a zero dimension"))
}

fn wm_size_kind(label: &str) -> CaptureWmSizeKind {
    match label.trim() {
        "Physical size" => CaptureWmSizeKind::Physical,
        "Override size" => CaptureWmSizeKind::Override,
        _ => CaptureWmSizeKind::Unlabelled,
    }
}

fn observed_rotation(
    rotation: DeviceRotation,
    source: CaptureRotationSource,
) -> CaptureRotationObservation {
    CaptureRotationObservation::Observed {
        rotation: match rotation {
            DeviceRotation::R0 => CaptureRotation::R0,
            DeviceRotation::R90 => CaptureRotation::R90,
            DeviceRotation::R180 => CaptureRotation::R180,
            DeviceRotation::R270 => CaptureRotation::R270,
        },
        source,
    }
}

fn read_adb_capture_geometry(
    adb: &Adb,
    serial: &str,
    backend: CaptureBackendName,
    deadline: Instant,
) -> DeviceResult<CaptureGeometryObservation> {
    let output = adb.run_until(&["-s", serial, "shell", "wm", "size"], deadline)?;
    // Reuse the original input bounds interpretation, including Override selection.
    let bounds = crate::touch::touch_bounds_from_screen_size(&output.stdout)?;
    let width = bounds.max_x as u32;
    let height = bounds.max_y as u32;
    let label = output
        .stdout
        .rsplit_once(':')
        .and_then(|(prefix, _)| prefix.lines().last())
        .unwrap_or("");
    let (rotation, rotation_source) =
        read_device_rotation_with_source(adb, serial, Some(deadline))?;
    geometry_remaining(deadline)?;
    let (logical_width, logical_height) = display_size_from_natural(width, height, rotation);
    Ok(CaptureGeometryObservation::Observed(CaptureGeometry {
        backend,
        source: CaptureGeometrySource::AdbDefaultDisplay {
            serial: serial.to_string(),
            wm_extent: geometry_extent(width, height)?,
            wm_size_kind: wm_size_kind(label),
        },
        logical_display_extent: geometry_extent(logical_width, logical_height)?,
        rotation: observed_rotation(rotation, rotation_source),
        sampled_at: SystemTime::now(),
        frame_transform: None,
    }))
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
        let issuer = actingcommand_contract::IdentifierIssuer::new().expect("ids");
        let close_witness = std::sync::Arc::new(actingcommand_contract::issue_fenced_write(
            actingcommand_contract::LeaseToken::new(
                *issuer.mint_owner_epoch().expect("epoch").transport(),
                *issuer.mint_lease_id().expect("lease").transport(),
                *issuer.mint_instance_id().expect("instance").transport(),
                *issuer.mint_holder_id().expect("holder").transport(),
                100,
            )
            .expect("test close token"),
            1,
            std::num::NonZeroU64::new(1).expect("step"),
            actingcommand_contract::FencedWritePurpose::ResourceClose,
        ));
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
            .close_once(DeviceCloseAuthority::FencedDeviceWrite(
                std::sync::Arc::clone(&close_witness),
            ))
            .expect("first close");
        let second = backend
            .close_once(DeviceCloseAuthority::FencedDeviceWrite(
                std::sync::Arc::clone(&close_witness),
            ))
            .expect("cached close");

        assert_eq!(first, second);
        assert_eq!(first.quiescence(), DeviceResourceQuiescence::Confirmed);
        assert_eq!(first.resource_count(), 2);
        assert_eq!(close_calls.get(), 1);
        assert!(backend.primed.is_none());
    }

    // Workflow #257 / C1-NEMU-CLOSE-v1, Defect regression.
    // First red: Workflow #269 issuecomment-5569993174 (W30 native ledger).
    #[cfg(windows)]
    #[test]
    fn nemu_owned_close_retires_sync_handle_and_preserves_real_failures() {
        let issuer = actingcommand_contract::IdentifierIssuer::new().expect("ids");
        let close_witness = std::sync::Arc::new(actingcommand_contract::issue_fenced_write(
            actingcommand_contract::LeaseToken::new(
                *issuer.mint_owner_epoch().expect("epoch").transport(),
                *issuer.mint_lease_id().expect("lease").transport(),
                *issuer.mint_instance_id().expect("instance").transport(),
                *issuer.mint_holder_id().expect("holder").transport(),
                100,
            )
            .expect("test close token"),
            1,
            std::num::NonZeroU64::new(1).expect("step"),
            actingcommand_contract::FencedWritePurpose::ResourceClose,
        ));
        use std::io::Write as _;
        use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        static LAST_ID: AtomicI32 = AtomicI32::new(0);
        unsafe extern "C" fn returned_disconnect(id: i32) {
            LAST_ID.store(id, Ordering::SeqCst);
            CALLS.fetch_add(1, Ordering::SeqCst);
        }

        let mut summary = std::env::var_os("GITHUB_STEP_SUMMARY").map(|path| {
            std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(path)
                .expect("open native CI step summary before owned stdio")
        });
        for mode in 0..5 {
            let mut worker_summary = summary
                .as_ref()
                .map(|file| file.try_clone().expect("clone owned CI summary handle"));
            let before = CALLS.load(Ordering::SeqCst);
            let (tx, rx) = mpsc::channel();
            let (ready_tx, ready_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let worker_close_witness = Arc::clone(&close_witness);
            let handle = thread::spawn(move || {
                let mut phase = "worker_library_start";
                let mut errors = Vec::<(&str, DeviceError)>::new();
                let mut worker_state = None;
                let execution = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    worker_state = Some(NemuIpcWorkerState {
                        input: None,
                        // An existing OS library reference exercises real local unload, not an SDK.
                        library: Some(unsafe { Library::new("kernel32.dll") }.expect("OS library")),
                        stdio_session: Some({
                            phase = "worker_stdio_start";
                            VendorStdioSession::start()
                                .inspect_err(|error| errors.push((phase, error.clone())))
                                .expect("owned stdio")
                        }),
                        nemu_folder: Vec::new(),
                        instance_id: 1,
                        display_id: 0,
                        connect_id: 40 + mode,
                        raw_buffer: Vec::new(),
                        frame_width: 0,
                        frame_height: 0,
                        vendor_stdio: Vec::new(),
                    });
                    let state = worker_state.as_mut().expect("initialized worker state");
                    phase = "worker_ready_send";
                    ready_tx.send(()).expect("worker ready");
                    phase = "worker_shutdown_receive";
                    let NemuIpcCommand::Shutdown {
                        authority,
                        response,
                        ..
                    } = rx.recv().expect("shutdown")
                    else {
                        panic!("only shutdown is expected");
                    };
                    if mode == 4 {
                        phase = "worker_release_receive";
                        release_rx
                            .recv_timeout(Duration::from_secs(5))
                            .expect("release delayed worker");
                    }
                    if mode == 2 {
                        phase = "worker_stdio_finish_before_snapshot";
                        state
                            .stdio_session
                            .as_mut()
                            .expect("stdio")
                            .finish()
                            .inspect_err(|error| errors.push((phase, error.clone())))
                            .expect("close stdio before snapshot");
                    }
                    phase = "worker_disconnect";
                    let result = state
                        .disconnect(authority, |_| {
                            if mode == 1 {
                                Err(DeviceError::fatal("missing disconnect symbol"))
                            } else {
                                Ok(returned_disconnect as NemuDisconnect)
                            }
                        })
                        .inspect_err(|error| errors.push((phase, error.clone())));
                    if matches!(mode, 1 | 3) {
                        assert_eq!(
                            state.connect_id,
                            40 + mode,
                            "no native call retired this handle"
                        );
                        // Release the in-test opaque ID; it never belonged to a native provider.
                        state.connect_id = 0;
                    } else {
                        assert_eq!(
                            state.connect_id, 0,
                            "a returned call must retire its handle"
                        );
                    }
                    phase = "worker_retired_disconnect";
                    state
                        .disconnect(
                            DeviceCloseAuthority::FencedDeviceWrite(std::sync::Arc::clone(
                                &worker_close_witness,
                            )),
                            |_| {
                                panic!("a retired handle must not resolve or call disconnect again")
                            },
                        )
                        .inspect_err(|error| errors.push((phase, error.clone())))
                        .expect("retired disconnect");
                    phase = "worker_cleanup";
                    let cleanup = state
                        .close(DeviceCloseAuthority::FencedDeviceWrite(
                            std::sync::Arc::clone(&worker_close_witness),
                        ))
                        .inspect_err(|error| errors.push((phase, error.clone())));
                    assert!(state.stdio_session.is_none());
                    assert!(state.library.is_none());
                    let result = match result {
                        Ok(()) => cleanup,
                        Err(primary) => match cleanup {
                            Ok(_) => Err(primary),
                            Err(cleanup) => Err(primary.merge_resource_cleanup(cleanup)),
                        },
                    };
                    phase = "worker_close_response";
                    if mode == 4 {
                        assert!(
                            response.send(result.clone()).is_err(),
                            "the original waiter timed out"
                        );
                    } else {
                        response.send(result.clone()).expect("close response");
                    }
                    phase = "worker_shutdown_queue";
                    assert!(
                        rx.try_recv().is_err(),
                        "close-once cannot enqueue another shutdown"
                    );
                    result.map(|_| ())
                }));
                match execution {
                    Ok(result) => result,
                    Err(original) => {
                        if let Some(file) = worker_summary.as_mut() {
                            let panic_text = original
                                .downcast_ref::<String>()
                                .map(String::as_str)
                                .or_else(|| original.downcast_ref::<&str>().copied())
                                .unwrap_or("non-string panic payload preserved");
                            let mut context = [0u8; 16 * 1024];
                            let mut remaining = &mut context[..16 * 1024 - 128];
                            let formatted = (|| -> std::io::Result<()> {
                                writeln!(
                                    remaining,
                                    "\n### Nemu owned-close failure: worker mode={mode} phase={phase}"
                                )?;
                                writeln!(remaining, "original panic: {panic_text}")?;
                                writeln!(
                                    remaining,
                                    "state(connect_id, session_present, library_present)={:?}; error_count={}",
                                    worker_state.as_ref().map(|state| (
                                        state.connect_id,
                                        state.stdio_session.is_some(),
                                        state.library.is_some()
                                    )),
                                    errors.len()
                                )?;
                                for (at, error) in &errors {
                                    writeln!(
                                        remaining,
                                        "error phase={at}: {error:?}; quiescence={:?} count={} causes={}",
                                        error.resource_quiescence(),
                                        error.resource_count(),
                                        error.resource_close_causes().len()
                                    )?;
                                    for cause in error.resource_close_causes() {
                                        writeln!(remaining, "cause={cause:?}")?;
                                        if let Some(facts) = cause.vendor_stdio() {
                                            writeln!(
                                                remaining,
                                                "stdio pid={} created={:?} started={} steps={} dropped={}",
                                                facts.process_id,
                                                facts.process_created_filetime,
                                                facts.started_filetime,
                                                facts.steps.len(),
                                                facts.dropped_count
                                            )?;
                                            for step in &facts.steps {
                                                writeln!(remaining, "{step:?}")?;
                                            }
                                        }
                                    }
                                }
                                Ok(())
                            })();
                            let used = 16 * 1024 - 128 - remaining.len();
                            let valid = std::str::from_utf8(&context[..used])
                                .map_or_else(|error| error.valid_up_to(), |_| used);
                            let footer: &[u8] = if formatted.is_err() || valid != used {
                                b"\n[truncated: 16-KiB record limit or formatting failure; remaining context omitted]\n"
                            } else {
                                b"\n[end Nemu failure context]\n"
                            };
                            context[valid..valid + footer.len()].copy_from_slice(footer);
                            let record = &context[..valid + footer.len()];
                            if let Err(error) = file.write_all(record).and_then(|()| file.flush()) {
                                eprintln!(
                                    "CI summary write/flush failed: {error}; original failure retained:\n{}",
                                    String::from_utf8_lossy(record)
                                );
                            }
                        }
                        std::panic::resume_unwind(original)
                    }
                }
            });
            let mut handle = Some(handle);
            let mut phase = "parent_worker_ready";
            let mut errors = Vec::<(&str, DeviceError)>::new();
            let mut parent_backend = None;
            let execution = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                ready_rx
                    .recv_timeout(Duration::from_secs(10))
                    .expect("initialized local worker");
                parent_backend = Some(NemuIpcBackend {
                    worker: Some(NemuIpcWorker {
                        tx,
                        handle: handle.take(),
                        timeout: if mode == 4 {
                            Duration::from_millis(25)
                        } else {
                            Duration::from_secs(2)
                        },
                        poisoned: Arc::new(AtomicBool::new(false)),
                        close_result: None,
                    }),
                    frame_width: 0,
                    frame_height: 0,
                    vendor_stdio: Vec::new(),
                    close_result: None,
                });
                let backend = parent_backend.as_mut().expect("initialized test backend");
                let authority = if mode == 3 {
                    DeviceCloseAuthority::LocalOnly
                } else {
                    DeviceCloseAuthority::FencedDeviceWrite(std::sync::Arc::clone(&close_witness))
                };
                phase = "parent_first_close";
                let first = backend
                    .close_once(authority.clone())
                    .inspect_err(|error| errors.push((phase, error.clone())));
                phase = "parent_second_close";
                let second = backend
                    .close_once(authority.clone())
                    .inspect_err(|error| errors.push((phase, error.clone())));
                match (&first, &second) {
                    (Ok(first), Ok(second)) => assert_eq!(first, second),
                    (Err(first), Err(second)) => {
                        assert_eq!(
                            first.resource_close_causes(),
                            second.resource_close_causes()
                        );
                        assert_eq!(first.resource_quiescence(), second.resource_quiescence());
                        assert_eq!(first.to_string(), second.to_string());
                    }
                    _ => panic!("the first terminal result must be stable"),
                }
                if mode == 4 {
                    phase = "parent_delayed_worker";
                    assert!(
                        !backend
                            .worker
                            .as_ref()
                            .unwrap()
                            .handle
                            .as_ref()
                            .unwrap()
                            .is_finished()
                    );
                    phase = "parent_release_worker";
                    release_tx.send(()).unwrap_or_else(|error| {
                        panic!("release owned test worker after timeout: {error:?}; phase={phase}");
                    });
                    let worker = backend.worker.as_mut().unwrap();
                    worker.timeout = Duration::from_secs(2);
                    phase = "parent_join_worker";
                    worker
                        .join_bounded()
                        .inspect_err(|error| errors.push((phase, error.clone())))
                        .expect("finish local test cleanup");
                    phase = "parent_late_close";
                    let late = backend.close_once(authority).expect_err("cached timeout");
                    errors.push((phase, late.clone()));
                    assert_eq!(
                        late.resource_close_causes(),
                        first.as_ref().unwrap_err().resource_close_causes(),
                        "late join cannot overwrite first failure"
                    );
                    assert_eq!(
                        late.resource_quiescence(),
                        Some(DeviceResourceQuiescence::Unconfirmed)
                    );
                }
                phase = "parent_terminal_assertions";
                if mode == 0 {
                    let outcome = first.expect("complete local close chain");
                    assert_eq!(outcome.quiescence(), DeviceResourceQuiescence::Confirmed);
                    assert!(outcome.resource_count() >= 3);
                    assert!(backend.worker.is_none());
                } else {
                    let error = first.expect_err("real close failure remains visible");
                    assert_eq!(
                        error.resource_quiescence(),
                        Some(DeviceResourceQuiescence::Unconfirmed)
                    );
                    let expected = match mode {
                        1 => DeviceResourceClosePhase::DisconnectSymbol,
                        2 => DeviceResourceClosePhase::SnapshotRead,
                        3 => DeviceResourceClosePhase::DisconnectCall,
                        4 => DeviceResourceClosePhase::WorkerReceive,
                        _ => unreachable!(),
                    };
                    assert!(
                        error
                            .resource_close_causes()
                            .iter()
                            .any(|cause| cause.phase() == expected)
                    );
                }
                assert_eq!(
                    CALLS.load(Ordering::SeqCst) - before,
                    usize::from(matches!(mode, 0 | 2 | 4))
                );
                if matches!(mode, 0 | 2 | 4) {
                    assert_eq!(LAST_ID.load(Ordering::SeqCst), 40 + mode);
                }
            }));
            if let Err(original) = execution {
                if let Some(file) = summary.as_mut() {
                    let panic_text = original
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| original.downcast_ref::<&str>().copied())
                        .unwrap_or("non-string panic payload preserved");
                    let mut context = [0u8; 16 * 1024];
                    let mut remaining = &mut context[..16 * 1024 - 128];
                    let formatted = (|| -> std::io::Result<()> {
                        writeln!(
                            remaining,
                            "\n### Nemu owned-close failure: parent mode={mode} phase={phase}"
                        )?;
                        writeln!(remaining, "original panic: {panic_text}")?;
                        writeln!(
                            remaining,
                            "worker(handle_present, finished)={:?}; unassigned_handle_finished={:?}; error_count={}",
                            parent_backend
                                .as_ref()
                                .and_then(|backend| backend.worker.as_ref())
                                .map(|worker| (
                                    worker.handle.is_some(),
                                    worker.handle.as_ref().map(|handle| handle.is_finished())
                                )),
                            handle.as_ref().map(|handle| handle.is_finished()),
                            errors.len()
                        )?;
                        for (at, error) in &errors {
                            writeln!(
                                remaining,
                                "error phase={at}: {error:?}; quiescence={:?} count={} causes={}",
                                error.resource_quiescence(),
                                error.resource_count(),
                                error.resource_close_causes().len()
                            )?;
                            for cause in error.resource_close_causes() {
                                writeln!(remaining, "cause={cause:?}")?;
                                if let Some(facts) = cause.vendor_stdio() {
                                    writeln!(
                                        remaining,
                                        "stdio pid={} created={:?} started={} steps={} dropped={}",
                                        facts.process_id,
                                        facts.process_created_filetime,
                                        facts.started_filetime,
                                        facts.steps.len(),
                                        facts.dropped_count
                                    )?;
                                    for step in &facts.steps {
                                        writeln!(remaining, "{step:?}")?;
                                    }
                                }
                            }
                        }
                        Ok(())
                    })();
                    let used = 16 * 1024 - 128 - remaining.len();
                    let valid = std::str::from_utf8(&context[..used])
                        .map_or_else(|error| error.valid_up_to(), |_| used);
                    let footer: &[u8] = if formatted.is_err() || valid != used {
                        b"\n[truncated: 16-KiB record limit or formatting failure; remaining context omitted]\n"
                    } else {
                        b"\n[end Nemu failure context]\n"
                    };
                    context[valid..valid + footer.len()].copy_from_slice(footer);
                    let record = &context[..valid + footer.len()];
                    if let Err(error) = file.write_all(record).and_then(|()| file.flush()) {
                        eprintln!(
                            "CI summary write/flush failed: {error}; original failure retained:\n{}",
                            String::from_utf8_lossy(record)
                        );
                    }
                }
                std::panic::resume_unwind(original)
            }
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
        let _env_guard = crate::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        let _env_guard = crate::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        let provenance = prepared
            .resolved_mumu
            .as_ref()
            .expect("resolved installation context");
        assert_eq!(provenance.source, crate::MumuInstallSource::RunningProcess);
        assert_eq!(Some(&provenance.root), prepared.nemu.nemu_folder.as_ref());
        assert_eq!(
            Some(&provenance.capture_dll_path),
            prepared.nemu.dll_path.as_ref()
        );
        assert_eq!(provenance.adb_path, fs::canonicalize(&generic_adb).unwrap());
        assert_eq!(prepared.adb_config.adb_path, original_adb);
        struct SuccessfulCapture;
        impl CaptureBackend for SuccessfulCapture {
            fn capture(&mut self) -> DeviceResult<Frame> {
                Frame::from_pixels(
                    1,
                    1,
                    vec![1, 2, 3],
                    PixelFormat::Rgb8,
                    CaptureBackendName::NemuIpc,
                )
            }

            fn close_once(
                &mut self,
                _authority: DeviceCloseAuthority,
            ) -> DeviceResult<DeviceResourceCloseOutcome> {
                Ok(DeviceResourceCloseOutcome::confirmed(0))
            }
        }
        let context = CaptureSelectionContext {
            nemu_frame: None,
            requested: prepared.requested,
            configured_adb: original_adb.clone(),
            configured_serial: prepared.target.serial.clone(),
            resolved_adb: prepared.adb_config.adb_path.clone(),
            selected_serial: prepared.target.resolved_serial(),
            mumu: prepared.resolved_mumu.clone(),
        };
        let mut selected = selected_explicit(
            prepared.requested,
            CaptureBackendName::NemuIpc,
            Box::new(SuccessfulCapture),
        );
        selected.selection = Some(Arc::new(context.clone()));
        let frame = selected.capture().expect("successful selected producer");
        assert_eq!(frame.backend_name, CaptureBackendName::NemuIpc);
        assert_eq!(frame.selection.as_deref(), Some(&context));
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
