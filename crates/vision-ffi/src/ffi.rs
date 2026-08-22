// SPDX-License-Identifier: AGPL-3.0-only

use crate::{
    CudaDeviceIdentity, CudaDeviceInventory, FastDeployPpocrArtifacts,
    FastDeployPpocrInvokeRequest, FastDeployPpocrInvokeResponse, NnClassificationResult, NnEngine,
    NnInferenceRequest, OcrEngine, OcrInferenceOutput, OcrInferenceRequest, OcrInferenceResult,
    OcrInvocationId, OcrSessionBinding, OcrSessionId, OnnxExecutionProvider, OnnxRuntimeArtifacts,
    OnnxRuntimeInvokeRequest, VisionFfiError, VisionFfiErrorCode, VisionFfiResult,
    VisionProviderArtifactManifest,
};
use libloading::Library;
use serde::{Serialize, de::DeserializeOwned};
use std::ffi::{CStr, OsStr, c_char, c_void};
use std::slice;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
#[cfg(any(windows, test))]
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

const OCR_READ_TEXT_SYMBOL: &[u8] = b"ac_fastdeploy_ppocr_read_text_json\0";
const NN_CLASSIFY_SYMBOL: &[u8] = b"ac_onnxruntime_classify_json\0";
const FREE_BUFFER_SYMBOL: &[u8] = b"ac_vision_free_buffer\0";
const MAX_FFI_RESPONSE_BYTES: usize = 128 * 1024 * 1024;
const CUDA_SUCCESS: i32 = 0;
const CUDA_PCI_BUS_ID_BYTES: usize = 64;
const MAX_ONNXRUNTIME_VERSION_BYTES: usize = 256;

static NEXT_OCR_SESSION_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static NEXT_OCR_INVOCATION_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[cfg(any(windows, test))]
enum RuntimeLibraryClosureState<H> {
    Uninitialized,
    Ready {
        identity: Vec<PathBuf>,
        _handles: Vec<H>,
    },
    Poisoned {
        identity: Vec<PathBuf>,
        _handles: Vec<H>,
        failed_path: PathBuf,
        reason: String,
    },
}

#[cfg(windows)]
static PROCESS_RUNTIME_LIBRARY_CLOSURE: Mutex<RuntimeLibraryClosureState<Arc<Library>>> =
    Mutex::new(RuntimeLibraryClosureState::Uninitialized);
#[cfg(windows)]
const PROCESS_RUNTIME_LIBRARY_LOAD_FLAGS: u32 =
    libloading::os::windows::LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR
        | libloading::os::windows::LOAD_LIBRARY_SEARCH_SYSTEM32;

pub type VisionFfiInvokeJson = unsafe extern "C" fn(
    request_ptr: *const u8,
    request_len: usize,
    response_out: *mut VisionFfiOwnedBuffer,
) -> i32;

pub type VisionFfiFreeBuffer = unsafe extern "C" fn(buffer: VisionFfiOwnedBuffer);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VisionFfiOwnedBuffer {
    pub data: *mut u8,
    pub len: usize,
    pub capacity: usize,
}

impl VisionFfiOwnedBuffer {
    /// Reports whether this metadata can be passed to the paired provider deallocator.
    ///
    /// This validates ownership metadata only. Pointer provenance remains an ABI
    /// invariant between the caller and the provider that allocated the buffer.
    pub fn has_releasable_metadata(&self) -> bool {
        !self.data.is_null()
            && self.capacity > 0
            && self.len <= self.capacity
            && self.len <= MAX_FFI_RESPONSE_BYTES
            && self.capacity <= MAX_FFI_RESPONSE_BYTES
    }
}

impl Default for VisionFfiOwnedBuffer {
    fn default() -> Self {
        Self {
            data: std::ptr::null_mut(),
            len: 0,
            capacity: 0,
        }
    }
}

pub struct FastDeployPpocrBackend {
    _library: Option<Arc<Library>>,
    read_text_json: VisionFfiInvokeJson,
    free_buffer: VisionFfiFreeBuffer,
    artifacts: Option<FastDeployPpocrArtifacts>,
    session: Option<Arc<OcrSessionBinding>>,
}

impl FastDeployPpocrBackend {
    pub fn from_library_path(path: impl AsRef<OsStr>) -> VisionFfiResult<Self> {
        let library = load_library("fastdeploy-ppocr", path)?;
        let read_text_json = load_symbol(&library, "fastdeploy-ppocr", OCR_READ_TEXT_SYMBOL)?;
        let free_buffer = load_symbol(&library, "fastdeploy-ppocr", FREE_BUFFER_SYMBOL)?;
        Ok(Self {
            _library: Some(library),
            read_text_json,
            free_buffer,
            artifacts: None,
            session: None,
        })
    }

    pub fn from_artifacts(artifacts: FastDeployPpocrArtifacts) -> VisionFfiResult<Self> {
        artifacts.validate_ppocr_v6_execution_existing_files()?;
        establish_process_runtime_library_closure(&artifacts.runtime_library_paths)?;
        let session = new_session_binding(&artifacts)?;
        let library = load_library("fastdeploy-ppocr", &artifacts.provider_library_path)?;
        let read_text_json = load_symbol(&library, "fastdeploy-ppocr", OCR_READ_TEXT_SYMBOL)?;
        let free_buffer = load_symbol(&library, "fastdeploy-ppocr", FREE_BUFFER_SYMBOL)?;
        Ok(Self {
            _library: Some(library),
            read_text_json,
            free_buffer,
            artifacts: Some(artifacts),
            session: Some(Arc::new(session)),
        })
    }

    pub fn from_manifest(manifest: &VisionProviderArtifactManifest) -> VisionFfiResult<Self> {
        Self::from_artifacts(manifest.require_production_fastdeploy_ppocr()?.clone())
    }

    /// # Safety
    ///
    /// The function pointers must follow the ActingCommand OCR JSON ABI and
    /// the free function must be able to release every buffer returned by the
    /// invoke function for the lifetime of this backend.
    pub unsafe fn from_raw_functions(
        read_text_json: VisionFfiInvokeJson,
        free_buffer: VisionFfiFreeBuffer,
    ) -> Self {
        Self {
            _library: None,
            read_text_json,
            free_buffer,
            artifacts: None,
            session: None,
        }
    }

    /// # Safety
    ///
    /// The function pointers must follow the ActingCommand OCR JSON envelope
    /// ABI and the free function must be able to release every buffer returned
    /// by the invoke function for the lifetime of this backend.
    pub unsafe fn from_raw_functions_with_artifacts(
        read_text_json: VisionFfiInvokeJson,
        free_buffer: VisionFfiFreeBuffer,
        artifacts: FastDeployPpocrArtifacts,
    ) -> VisionFfiResult<Self> {
        artifacts.validate_ppocr_v6_execution()?;
        let session = new_session_binding(&artifacts)?;
        Ok(Self {
            _library: None,
            read_text_json,
            free_buffer,
            artifacts: Some(artifacts),
            session: Some(Arc::new(session)),
        })
    }

    #[cfg(test)]
    pub(crate) unsafe fn from_raw_functions_with_artifacts_and_inventory(
        read_text_json: VisionFfiInvokeJson,
        free_buffer: VisionFfiFreeBuffer,
        artifacts: FastDeployPpocrArtifacts,
        inventory: Option<CudaDeviceInventory>,
    ) -> VisionFfiResult<Self> {
        artifacts.validate_ppocr_v6_execution()?;
        let session = new_session_binding_with(
            &artifacts,
            || inventory_result(inventory),
            |_| Ok("1.24.0-test".to_string()),
        )?;
        Ok(Self {
            _library: None,
            read_text_json,
            free_buffer,
            artifacts: Some(artifacts),
            session: Some(Arc::new(session)),
        })
    }

    pub fn reconfigure(&mut self, artifacts: FastDeployPpocrArtifacts) -> VisionFfiResult<()> {
        artifacts.validate_ppocr_v6_execution_existing_files()?;
        #[cfg(windows)]
        {
            let current_artifacts = self.artifacts.as_ref().ok_or_else(|| {
                VisionFfiError::fatal_with_code(
                    VisionFfiErrorCode::InvalidRequest,
                    "fastdeploy-ppocr",
                    "unattested raw backend cannot be reconfigured as a production OCR session",
                )
            })?;
            require_same_runtime_library_closure(
                &current_artifacts.runtime_library_paths,
                &artifacts.runtime_library_paths,
            )?;
            establish_process_runtime_library_closure(&artifacts.runtime_library_paths)?;
        }
        let key = resolve_session_key(&artifacts)?;
        let current = self.session.as_ref().ok_or_else(|| {
            VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidRequest,
                "fastdeploy-ppocr",
                "unattested raw backend cannot be reconfigured as a production OCR session",
            )
        })?;
        require_same_process_runtime(current.key(), &key)?;
        let next_generation = current.generation().checked_add(1).ok_or_else(|| {
            VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::Internal,
                "fastdeploy-ppocr",
                "OCR session generation overflowed",
            )
        })?;
        let session = OcrSessionBinding::new(current.session_id().clone(), next_generation, key);
        session.validate()?;
        let library = load_library("fastdeploy-ppocr", &artifacts.provider_library_path)?;
        let read_text_json = load_symbol(&library, "fastdeploy-ppocr", OCR_READ_TEXT_SYMBOL)?;
        let free_buffer = load_symbol(&library, "fastdeploy-ppocr", FREE_BUFFER_SYMBOL)?;

        self._library = Some(library);
        self.read_text_json = read_text_json;
        self.free_buffer = free_buffer;
        self.artifacts = Some(artifacts);
        self.session = Some(Arc::new(session));
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn session_for_test(&self) -> VisionFfiResult<Arc<OcrSessionBinding>> {
        self.session.as_ref().map(Arc::clone).ok_or_else(|| {
            VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::Internal,
                "fastdeploy-ppocr",
                "test backend is missing its immutable session binding",
            )
        })
    }

    #[cfg(test)]
    pub(crate) fn reconfigure_with_inventory_for_test(
        &mut self,
        artifacts: FastDeployPpocrArtifacts,
        inventory: Option<CudaDeviceInventory>,
    ) -> VisionFfiResult<()> {
        artifacts.validate_ppocr_v6_execution()?;
        let current_artifacts = self.artifacts.as_ref().ok_or_else(|| {
            VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidRequest,
                "fastdeploy-ppocr",
                "unattested raw backend cannot be reconfigured as a production OCR session",
            )
        })?;
        require_same_runtime_library_closure(
            &current_artifacts.runtime_library_paths,
            &artifacts.runtime_library_paths,
        )?;
        let key = resolve_session_key_with(
            &artifacts,
            || inventory_result(inventory),
            |_| Ok("1.24.0-test".to_string()),
        )?;
        let current = self.session_for_test()?;
        require_same_process_runtime(current.key(), &key)?;
        let next_generation = current.generation().checked_add(1).ok_or_else(|| {
            VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::Internal,
                "fastdeploy-ppocr",
                "OCR session generation overflowed",
            )
        })?;
        let session = OcrSessionBinding::new(current.session_id().clone(), next_generation, key);
        session.validate()?;

        self.artifacts = Some(artifacts);
        self.session = Some(Arc::new(session));
        Ok(())
    }
}

#[cfg(any(windows, test))]
fn require_same_runtime_library_closure(
    current: &[PathBuf],
    candidate: &[PathBuf],
) -> VisionFfiResult<()> {
    if current == candidate {
        Ok(())
    } else {
        Err(VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::ProviderUnavailable,
            "fastdeploy-ppocr",
            "native OCR runtime-library closure cannot change in-process; restart is required",
        ))
    }
}

#[cfg(any(windows, test))]
fn establish_runtime_library_closure_with<H, F>(
    closure: &Mutex<RuntimeLibraryClosureState<H>>,
    declared_paths: &[PathBuf],
    mut load: F,
) -> VisionFfiResult<()>
where
    F: FnMut(&Path) -> VisionFfiResult<H>,
{
    if declared_paths.is_empty() {
        return Err(runtime_closure_error(
            "native OCR runtime-library closure must not be empty",
        ));
    }
    if let Some(path) = declared_paths.iter().find(|path| !path.is_absolute()) {
        return Err(runtime_closure_error(format!(
            "native OCR runtime-library closure path must be absolute: {}",
            path.display()
        )));
    }

    let identity = declared_paths.to_vec();
    let mut closure = closure.lock().map_err(|_| {
        runtime_closure_error(
            "native OCR runtime-library closure state is poisoned; restart is required",
        )
    })?;
    match &*closure {
        RuntimeLibraryClosureState::Ready {
            identity: current, ..
        } => {
            return require_same_runtime_library_closure(current, &identity);
        }
        RuntimeLibraryClosureState::Poisoned {
            identity: failed_identity,
            failed_path,
            reason,
            ..
        } => {
            return Err(runtime_closure_error(format!(
                "native OCR runtime-library closure is permanently poisoned after failing to load {} from a {}-member closure: {reason}; restart is required",
                failed_path.display(),
                failed_identity.len()
            )));
        }
        RuntimeLibraryClosureState::Uninitialized => {}
    }

    let mut handles = Vec::with_capacity(identity.len());
    for path in &identity {
        match load(path) {
            Ok(handle) => handles.push(handle),
            Err(error) => {
                let reason = error.message().to_string();
                *closure = RuntimeLibraryClosureState::Poisoned {
                    identity: identity.clone(),
                    _handles: handles,
                    failed_path: path.clone(),
                    reason: reason.clone(),
                };
                return Err(runtime_closure_error(format!(
                    "failed to establish native OCR runtime-library closure at {}: {reason}; the process closure is permanently poisoned",
                    path.display()
                )));
            }
        }
    }

    *closure = RuntimeLibraryClosureState::Ready {
        identity,
        _handles: handles,
    };
    Ok(())
}

#[cfg(any(windows, test))]
fn runtime_closure_error(message: impl Into<String>) -> VisionFfiError {
    VisionFfiError::fatal_with_code(
        VisionFfiErrorCode::ProviderUnavailable,
        "fastdeploy-ppocr",
        message,
    )
}

#[cfg(windows)]
fn establish_process_runtime_library_closure(declared_paths: &[PathBuf]) -> VisionFfiResult<()> {
    establish_runtime_library_closure_with(
        &PROCESS_RUNTIME_LIBRARY_CLOSURE,
        declared_paths,
        load_process_runtime_library,
    )
}

#[cfg(not(windows))]
fn establish_process_runtime_library_closure(
    _declared_paths: &[std::path::PathBuf],
) -> VisionFfiResult<()> {
    Ok(())
}

#[cfg(windows)]
fn load_process_runtime_library(path: &Path) -> VisionFfiResult<Arc<Library>> {
    use libloading::os::windows::Library as WindowsLibrary;

    // SAFETY: the validated absolute path names the admitted runtime closure member. The
    // resulting handle is retained in PROCESS_RUNTIME_LIBRARY_CLOSURE for process lifetime.
    let library = unsafe {
        WindowsLibrary::load_with_flags(path, PROCESS_RUNTIME_LIBRARY_LOAD_FLAGS)
    }
    .map_err(|error| {
        runtime_closure_error(format!(
            "failed to load native OCR runtime library {} with LoadLibraryExW flags 0x{:08x}: {error}",
            path.display(),
            PROCESS_RUNTIME_LIBRARY_LOAD_FLAGS
        ))
    })?;
    Ok(Arc::new(library.into()))
}

fn require_same_process_runtime(
    current: &crate::OcrSessionKey,
    candidate: &crate::OcrSessionKey,
) -> VisionFfiResult<()> {
    if current.runtime_library_path() == candidate.runtime_library_path()
        && current.runtime_library_sha256() == candidate.runtime_library_sha256()
        && current.onnxruntime_version() == candidate.onnxruntime_version()
    {
        Ok(())
    } else {
        Err(VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::ProviderUnavailable,
            "fastdeploy-ppocr",
            "OCR runtime-library identity cannot change in-process; restart is required",
        ))
    }
}

fn new_session_binding(artifacts: &FastDeployPpocrArtifacts) -> VisionFfiResult<OcrSessionBinding> {
    new_session_binding_with(artifacts, enumerate_cuda_devices, |path| {
        onnxruntime_version_string(path)
    })
}

fn new_session_binding_with<F, V>(
    artifacts: &FastDeployPpocrArtifacts,
    inventory: F,
    runtime_version: V,
) -> VisionFfiResult<OcrSessionBinding>
where
    F: FnOnce() -> VisionFfiResult<CudaDeviceInventory>,
    V: FnOnce(&std::path::Path) -> VisionFfiResult<String>,
{
    let key = resolve_session_key_with(artifacts, inventory, runtime_version)?;
    let session = OcrSessionBinding::new(next_session_id()?, 1, key);
    session.validate()?;
    Ok(session)
}

fn resolve_session_key(
    artifacts: &FastDeployPpocrArtifacts,
) -> VisionFfiResult<crate::OcrSessionKey> {
    resolve_session_key_with(artifacts, enumerate_cuda_devices, |path| {
        onnxruntime_version_string(path)
    })
}

fn resolve_session_key_with<F, V>(
    artifacts: &FastDeployPpocrArtifacts,
    inventory: F,
    runtime_version: V,
) -> VisionFfiResult<crate::OcrSessionKey>
where
    F: FnOnce() -> VisionFfiResult<CudaDeviceInventory>,
    V: FnOnce(&std::path::Path) -> VisionFfiResult<String>,
{
    artifacts.validate_ppocr_v6_execution()?;
    let runtime_version = runtime_version(artifacts.onnxruntime_library_path()?)?;
    match artifacts.execution_provider {
        Some(OnnxExecutionProvider::Cpu) => artifacts.production_session_key(None, runtime_version),
        Some(OnnxExecutionProvider::Cuda) => {
            let selector = artifacts.cuda_device.as_ref().ok_or_else(|| {
                VisionFfiError::fatal_with_code(
                    VisionFfiErrorCode::InvalidRequest,
                    "fastdeploy-ppocr",
                    "CUDA OCR configuration is missing its device selector",
                )
            })?;
            let resolved = inventory()?.resolve(selector)?;
            artifacts.production_session_key(Some(resolved), runtime_version)
        }
        None => Err(VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::InvalidRequest,
            "fastdeploy-ppocr",
            "production OCR execution_provider must be explicitly cpu or cuda",
        )),
    }
}

fn next_session_id() -> VisionFfiResult<OcrSessionId> {
    next_sequence(&NEXT_OCR_SESSION_SEQUENCE, "OCR session").map(OcrSessionId::from_sequence)
}

fn next_invocation_id() -> VisionFfiResult<OcrInvocationId> {
    next_sequence(&NEXT_OCR_INVOCATION_SEQUENCE, "OCR invocation")
        .map(OcrInvocationId::from_sequence)
}

fn next_sequence(counter: &AtomicU64, label: &str) -> VisionFfiResult<u64> {
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .map_err(|_| {
            VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::Internal,
                "ocr-adapter-identity",
                format!("{label} identity space is exhausted"),
            )
        })
}

#[cfg(test)]
fn inventory_result(
    inventory: Option<CudaDeviceInventory>,
) -> VisionFfiResult<CudaDeviceInventory> {
    inventory.ok_or_else(|| {
        VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::ProviderUnavailable,
            "ocr-device-inventory",
            "test CUDA inventory is unavailable",
        )
    })
}

#[repr(C)]
struct CuUuid {
    bytes: [u8; 16],
}

type CuInit = unsafe extern "system" fn(u32) -> i32;
type CuDriverGetVersion = unsafe extern "system" fn(*mut i32) -> i32;
type CuDeviceGetCount = unsafe extern "system" fn(*mut i32) -> i32;
type CuDeviceGet = unsafe extern "system" fn(*mut i32, i32) -> i32;
type CuDeviceGetUuid = unsafe extern "system" fn(*mut CuUuid, i32) -> i32;
type CuDeviceGetPciBusId = unsafe extern "system" fn(*mut c_char, i32, i32) -> i32;

#[repr(C)]
struct OrtApiBase {
    _get_api: unsafe extern "system" fn(u32) -> *const c_void,
    get_version_string: unsafe extern "system" fn() -> *const c_char,
}

type OrtGetApiBase = unsafe extern "system" fn() -> *const OrtApiBase;

/// Reads the exact ONNX Runtime version from the admitted native library.
pub fn onnxruntime_version_string(path: impl AsRef<OsStr>) -> VisionFfiResult<String> {
    let library = load_library("onnxruntime-version", path)?;
    let get_api_base: OrtGetApiBase =
        load_symbol(&library, "onnxruntime-version", b"OrtGetApiBase\0")?;
    let api_base = unsafe {
        // SAFETY: OrtGetApiBase is loaded from the admitted ONNX Runtime
        // library and takes no arguments.
        get_api_base()
    };
    let api_base = unsafe {
        // SAFETY: a null OrtApiBase is rejected before any field is read.
        api_base.as_ref()
    }
    .ok_or_else(|| {
        VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::ProviderUnavailable,
            "onnxruntime-version",
            "OrtGetApiBase returned null",
        )
    })?;
    let version = unsafe {
        // SAFETY: get_version_string is part of the stable OrtApiBase ABI and
        // returns a process-lifetime NUL-terminated string.
        (api_base.get_version_string)()
    };
    let version = unsafe {
        // SAFETY: a null version pointer is rejected before decoding.
        version.as_ref()
    }
    .ok_or_else(|| {
        VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::ProviderUnavailable,
            "onnxruntime-version",
            "ONNX Runtime returned a null version string",
        )
    })?;
    let version = unsafe {
        // SAFETY: the API contract guarantees a NUL-terminated version string.
        CStr::from_ptr(version)
    }
    .to_str()
    .map_err(|err| {
        VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::InvalidResponse,
            "onnxruntime-version",
            format!("ONNX Runtime version is not UTF-8: {err}"),
        )
    })?;
    if version.is_empty()
        || version.len() > MAX_ONNXRUNTIME_VERSION_BYTES
        || version.trim() != version
        || version.chars().any(char::is_control)
    {
        return Err(VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::InvalidResponse,
            "onnxruntime-version",
            "ONNX Runtime version must be a bounded non-blank identity",
        ));
    }
    Ok(version.to_string())
}

/// Enumerates CUDA driver devices without creating an OCR inference session.
///
/// The result is bounded and carries stable UUID/PCI identity. Callers must not
/// use display names as a selector or as execution evidence.
pub fn enumerate_cuda_devices() -> VisionFfiResult<CudaDeviceInventory> {
    let library = load_library("cuda-driver", cuda_driver_library_name()?)?;
    let cu_init: CuInit = load_symbol(&library, "cuda-driver", b"cuInit\0")?;
    let cu_driver_get_version: CuDriverGetVersion =
        load_symbol(&library, "cuda-driver", b"cuDriverGetVersion\0")?;
    let cu_device_get_count: CuDeviceGetCount =
        load_symbol(&library, "cuda-driver", b"cuDeviceGetCount\0")?;
    let cu_device_get: CuDeviceGet = load_symbol(&library, "cuda-driver", b"cuDeviceGet\0")?;
    let cu_device_get_pci_bus_id: CuDeviceGetPciBusId =
        load_symbol(&library, "cuda-driver", b"cuDeviceGetPCIBusId\0")?;
    let cu_device_get_uuid: Option<CuDeviceGetUuid> = unsafe {
        // SAFETY: the symbol type follows the CUDA Driver API for both the v2
        // and legacy UUID entrypoints. The loaded library outlives every call.
        library
            .get::<CuDeviceGetUuid>(b"cuDeviceGetUuid_v2\0")
            .or_else(|_| library.get::<CuDeviceGetUuid>(b"cuDeviceGetUuid\0"))
            .ok()
            .map(|symbol| *symbol)
    };

    cuda_success("cuInit", unsafe {
        // SAFETY: cuInit accepts a zero flags value and has no output pointers.
        cu_init(0)
    })?;
    let mut driver_version = 0_i32;
    cuda_success("cuDriverGetVersion", unsafe {
        // SAFETY: driver_version is writable for the duration of this call.
        cu_driver_get_version(&mut driver_version)
    })?;
    let driver_version = u32::try_from(driver_version).map_err(|_| {
        VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::InvalidResponse,
            "cuda-driver",
            "CUDA driver returned a negative version",
        )
    })?;
    let mut device_count = 0_i32;
    cuda_success("cuDeviceGetCount", unsafe {
        // SAFETY: device_count is writable for the duration of this call.
        cu_device_get_count(&mut device_count)
    })?;
    let device_count = usize::try_from(device_count).map_err(|_| {
        VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::InvalidResponse,
            "cuda-driver",
            "CUDA driver returned a negative device count",
        )
    })?;
    if device_count == 0 || device_count > crate::MAX_CUDA_DEVICES {
        return Err(VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::ProviderUnavailable,
            "cuda-driver",
            format!(
                "CUDA driver reported {device_count} devices; expected 1..={}",
                crate::MAX_CUDA_DEVICES
            ),
        ));
    }

    let mut devices = Vec::with_capacity(device_count);
    for ordinal in 0..device_count {
        let ordinal_i32 = i32::try_from(ordinal).map_err(|_| {
            VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidResponse,
                "cuda-driver",
                "CUDA ordinal exceeds i32 range",
            )
        })?;
        let mut device = 0_i32;
        cuda_success("cuDeviceGet", unsafe {
            // SAFETY: device is writable and ordinal was bounded above.
            cu_device_get(&mut device, ordinal_i32)
        })?;
        let pci_bus_id = cuda_pci_bus_id(cu_device_get_pci_bus_id, device)?;
        let uuid_result = if let Some(cu_device_get_uuid) = cu_device_get_uuid {
            let mut uuid = CuUuid { bytes: [0; 16] };
            let status = unsafe {
                // SAFETY: uuid is writable and device came from cuDeviceGet.
                cu_device_get_uuid(&mut uuid, device)
            };
            Some((status, uuid.bytes))
        } else {
            None
        };
        let stable_identity = cuda_stable_identity(&pci_bus_id, uuid_result)?;
        devices.push(CudaDeviceIdentity {
            ordinal: u32::try_from(ordinal).map_err(|_| {
                VisionFfiError::fatal_with_code(
                    VisionFfiErrorCode::InvalidResponse,
                    "cuda-driver",
                    "CUDA ordinal exceeds u32 range",
                )
            })?,
            stable_identity,
            pci_bus_id: Some(pci_bus_id),
        });
    }
    let inventory = CudaDeviceInventory {
        driver_version,
        devices,
    };
    inventory.validate()?;
    Ok(inventory)
}

fn cuda_stable_identity(
    pci_bus_id: &str,
    uuid_result: Option<(i32, [u8; 16])>,
) -> VisionFfiResult<String> {
    let Some((status, uuid)) = uuid_result else {
        return Ok(format!("cuda-pci:{pci_bus_id}"));
    };
    cuda_success("cuDeviceGetUuid", status)?;
    if uuid.iter().all(|byte| *byte == 0) {
        return Err(VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::InvalidResponse,
            "cuda-driver",
            "CUDA driver returned an all-zero device UUID",
        ));
    }
    Ok(format!("cuda-uuid:{}", lower_hex_bytes(&uuid)))
}

fn cuda_driver_library_name() -> VisionFfiResult<&'static OsStr> {
    #[cfg(target_os = "windows")]
    {
        return Ok(OsStr::new("nvcuda.dll"));
    }
    #[cfg(target_os = "linux")]
    {
        return Ok(OsStr::new("libcuda.so.1"));
    }
    #[allow(unreachable_code)]
    Err(VisionFfiError::fatal_with_code(
        VisionFfiErrorCode::ProviderUnavailable,
        "cuda-driver",
        "CUDA driver discovery is unsupported on this operating system",
    ))
}

fn cuda_pci_bus_id(
    cu_device_get_pci_bus_id: CuDeviceGetPciBusId,
    device: i32,
) -> VisionFfiResult<String> {
    let mut bytes = [0 as c_char; CUDA_PCI_BUS_ID_BYTES];
    cuda_success("cuDeviceGetPCIBusId", unsafe {
        // SAFETY: bytes is writable for the declared length and device was
        // produced by cuDeviceGet in the same CUDA driver instance.
        cu_device_get_pci_bus_id(bytes.as_mut_ptr(), CUDA_PCI_BUS_ID_BYTES as i32, device)
    })?;
    let value = unsafe {
        // SAFETY: the CUDA call succeeded and promises a NUL-terminated string
        // within the supplied fixed-size output buffer.
        CStr::from_ptr(bytes.as_ptr())
    }
    .to_str()
    .map_err(|err| {
        VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::InvalidResponse,
            "cuda-driver",
            format!("CUDA PCI bus identity is not UTF-8: {err}"),
        )
    })?
    .to_ascii_lowercase();
    if value.is_empty() || value.len() >= CUDA_PCI_BUS_ID_BYTES {
        return Err(VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::InvalidResponse,
            "cuda-driver",
            "CUDA PCI bus identity is empty or unbounded",
        ));
    }
    Ok(value)
}

fn cuda_success(operation: &str, status: i32) -> VisionFfiResult<()> {
    if status == CUDA_SUCCESS {
        Ok(())
    } else {
        Err(VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::ProviderUnavailable,
            "cuda-driver",
            format!("{operation} failed with CUDA status {status}"),
        ))
    }
}

fn lower_hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(HEX[(byte >> 4) as usize] as char);
        value.push(HEX[(byte & 0x0f) as usize] as char);
    }
    value
}

pub fn validate_fastdeploy_ppocr_provider_abi(path: impl AsRef<OsStr>) -> VisionFfiResult<()> {
    let library = load_library("fastdeploy-ppocr", path)?;
    let _: VisionFfiInvokeJson = load_symbol(&library, "fastdeploy-ppocr", OCR_READ_TEXT_SYMBOL)?;
    let _: VisionFfiFreeBuffer = load_symbol(&library, "fastdeploy-ppocr", FREE_BUFFER_SYMBOL)?;
    Ok(())
}

impl OcrEngine for FastDeployPpocrBackend {
    fn read_text(&mut self, request: OcrInferenceRequest) -> VisionFfiResult<OcrInferenceResult> {
        self.read_text_with_attestation(request)
            .map(|output| output.result)
    }

    fn read_text_with_attestation(
        &mut self,
        request: OcrInferenceRequest,
    ) -> VisionFfiResult<OcrInferenceOutput> {
        request.validate()?;
        let validation_request = request.clone();
        let Some(artifacts) = &self.artifacts else {
            let result: OcrInferenceResult = invoke_json(
                "fastdeploy-ppocr",
                self.read_text_json,
                self.free_buffer,
                &request,
            )?;
            result.validate(&validation_request)?;
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidResponse,
                "fastdeploy-ppocr",
                "OCR provider returned a result without a session-bound execution attestation",
            ));
        };
        let session = self.session.as_ref().map(Arc::clone).ok_or_else(|| {
            VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::Internal,
                "fastdeploy-ppocr",
                "production OCR backend is missing its immutable session binding",
            )
        })?;
        let invocation_id = next_invocation_id()?;
        let envelope = FastDeployPpocrInvokeRequest::new(
            invocation_id.clone(),
            session.as_ref().clone(),
            request,
            artifacts.clone(),
        );
        envelope.validate()?;
        let response: FastDeployPpocrInvokeResponse = invoke_json(
            "fastdeploy-ppocr",
            self.read_text_json,
            self.free_buffer,
            &envelope,
        )?;
        response.validate_against(&invocation_id, &session)?;
        response.result.validate(&validation_request)?;
        Ok(OcrInferenceOutput {
            result: response.result,
            execution_attestation: Some(response.attestation),
        })
    }
}

pub struct OnnxRuntimeBackend {
    _library: Option<Arc<Library>>,
    classify_json: VisionFfiInvokeJson,
    free_buffer: VisionFfiFreeBuffer,
    artifacts: Option<OnnxRuntimeArtifacts>,
}

impl OnnxRuntimeBackend {
    pub fn from_library_path(path: impl AsRef<OsStr>) -> VisionFfiResult<Self> {
        let library = load_library("onnxruntime", path)?;
        let classify_json = load_symbol(&library, "onnxruntime", NN_CLASSIFY_SYMBOL)?;
        let free_buffer = load_symbol(&library, "onnxruntime", FREE_BUFFER_SYMBOL)?;
        Ok(Self {
            _library: Some(library),
            classify_json,
            free_buffer,
            artifacts: None,
        })
    }

    pub fn from_artifacts(artifacts: OnnxRuntimeArtifacts) -> VisionFfiResult<Self> {
        artifacts.validate_production_existing_files()?;
        let library = load_library("onnxruntime", &artifacts.provider_library_path)?;
        let classify_json = load_symbol(&library, "onnxruntime", NN_CLASSIFY_SYMBOL)?;
        let free_buffer = load_symbol(&library, "onnxruntime", FREE_BUFFER_SYMBOL)?;
        Ok(Self {
            _library: Some(library),
            classify_json,
            free_buffer,
            artifacts: Some(artifacts),
        })
    }

    pub fn from_manifest(manifest: &VisionProviderArtifactManifest) -> VisionFfiResult<Self> {
        Self::from_artifacts(manifest.require_production_onnxruntime()?.clone())
    }

    /// # Safety
    ///
    /// The function pointers must follow the ActingCommand NN JSON ABI and the
    /// free function must be able to release every buffer returned by the invoke
    /// function for the lifetime of this backend.
    pub unsafe fn from_raw_functions(
        classify_json: VisionFfiInvokeJson,
        free_buffer: VisionFfiFreeBuffer,
    ) -> Self {
        Self {
            _library: None,
            classify_json,
            free_buffer,
            artifacts: None,
        }
    }

    /// # Safety
    ///
    /// The function pointers must follow the ActingCommand NN JSON envelope ABI
    /// and the free function must be able to release every buffer returned by
    /// the invoke function for the lifetime of this backend.
    pub unsafe fn from_raw_functions_with_artifacts(
        classify_json: VisionFfiInvokeJson,
        free_buffer: VisionFfiFreeBuffer,
        artifacts: OnnxRuntimeArtifacts,
    ) -> VisionFfiResult<Self> {
        artifacts.validate_production_model()?;
        Ok(Self {
            _library: None,
            classify_json,
            free_buffer,
            artifacts: Some(artifacts),
        })
    }
}

pub fn validate_onnxruntime_provider_abi(path: impl AsRef<OsStr>) -> VisionFfiResult<()> {
    let library = load_library("onnxruntime", path)?;
    let _: VisionFfiInvokeJson = load_symbol(&library, "onnxruntime", NN_CLASSIFY_SYMBOL)?;
    let _: VisionFfiFreeBuffer = load_symbol(&library, "onnxruntime", FREE_BUFFER_SYMBOL)?;
    Ok(())
}

pub fn validate_runtime_library_loadable(
    module: &'static str,
    path: impl AsRef<OsStr>,
) -> VisionFfiResult<()> {
    load_library(module, path).map(|_| ())
}

impl NnEngine for OnnxRuntimeBackend {
    fn classify(&mut self, request: NnInferenceRequest) -> VisionFfiResult<NnClassificationResult> {
        request.validate()?;
        let result: NnClassificationResult = if let Some(artifacts) = &self.artifacts {
            invoke_json(
                "onnxruntime",
                self.classify_json,
                self.free_buffer,
                &OnnxRuntimeInvokeRequest {
                    request,
                    artifacts: artifacts.clone(),
                },
            )
        } else {
            invoke_json(
                "onnxruntime",
                self.classify_json,
                self.free_buffer,
                &request,
            )
        }?;
        result.validate()?;
        Ok(result)
    }
}

fn load_library(module: &'static str, path: impl AsRef<OsStr>) -> VisionFfiResult<Arc<Library>> {
    let path = path.as_ref();
    // SAFETY: loading a dynamic library is the required FFI boundary. The
    // handle is retained in the backend so loaded symbols cannot outlive it.
    let library = unsafe { Library::new(path) }.map_err(|err| {
        VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::ProviderUnavailable,
            module,
            format!(
                "failed to load FFI library {}: {err}",
                path.to_string_lossy()
            ),
        )
    })?;
    Ok(Arc::new(library))
}

fn load_symbol<T>(library: &Arc<Library>, module: &'static str, symbol: &[u8]) -> VisionFfiResult<T>
where
    T: Copy,
{
    // SAFETY: the symbol name is NUL-terminated and the copied function pointer
    // is kept valid by retaining the Arc<Library> inside the backend.
    let symbol = unsafe { library.get::<T>(symbol) }.map_err(|err| {
        VisionFfiError::fatal(module, format!("failed to load FFI symbol: {err}"))
    })?;
    Ok(*symbol)
}

fn invoke_json<I, O>(
    module: &'static str,
    invoke: VisionFfiInvokeJson,
    free_buffer: VisionFfiFreeBuffer,
    request: &I,
) -> VisionFfiResult<O>
where
    I: Serialize,
    O: DeserializeOwned,
{
    let request_json = serde_json::to_vec(request).map_err(|err| {
        VisionFfiError::fatal(module, format!("failed to serialize FFI request: {err}"))
    })?;
    let mut response = VisionFfiOwnedBuffer::default();
    // SAFETY: the request slice remains alive for the call, response_out points
    // to valid storage, and the callee must follow the documented JSON ABI.
    let status = unsafe {
        invoke(
            request_json.as_ptr(),
            request_json.len(),
            &mut response as *mut VisionFfiOwnedBuffer,
        )
    };
    let response_bytes = take_owned_buffer(module, response, free_buffer)?;
    if status != 0 {
        let response_text = String::from_utf8_lossy(&response_bytes);
        let code = match status {
            2 => VisionFfiErrorCode::ProviderPanic,
            3 => VisionFfiErrorCode::Timeout,
            _ => VisionFfiErrorCode::ProviderFailure,
        };
        return Err(VisionFfiError::fatal_with_code(
            code,
            module,
            format!("FFI backend returned status {status}: {response_text}"),
        ));
    }
    if response_bytes.is_empty() {
        return Err(VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::InvalidResponse,
            module,
            "FFI backend returned an empty response",
        ));
    }
    serde_json::from_slice(&response_bytes).map_err(|err| {
        VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::InvalidResponse,
            module,
            format!("failed to parse FFI response JSON: {err}"),
        )
    })
}

fn take_owned_buffer(
    module: &'static str,
    buffer: VisionFfiOwnedBuffer,
    free_buffer: VisionFfiFreeBuffer,
) -> VisionFfiResult<Vec<u8>> {
    if buffer.len == 0 && buffer.capacity == 0 {
        return Ok(Vec::new());
    }
    if buffer.data.is_null() {
        return Err(invalid_owned_buffer(
            module,
            "null data pointer with owned buffer metadata",
            buffer,
        ));
    }
    if buffer.capacity < buffer.len {
        return Err(invalid_owned_buffer(
            module,
            "buffer capacity smaller than its length",
            buffer,
        ));
    }
    if buffer.capacity == 0 {
        return Err(invalid_owned_buffer(
            module,
            "non-null data pointer with zero capacity",
            buffer,
        ));
    }
    if buffer.len > MAX_FFI_RESPONSE_BYTES || buffer.capacity > MAX_FFI_RESPONSE_BYTES {
        return Err(invalid_owned_buffer(
            module,
            "oversized response buffer metadata",
            buffer,
        ));
    }

    debug_assert!(buffer.has_releasable_metadata());
    if buffer.len == 0 {
        // SAFETY: all ownership metadata was validated before the provider
        // deallocator receives it.
        unsafe {
            free_buffer(buffer);
        }
        return Ok(Vec::new());
    }

    // SAFETY: the FFI provider returned a non-null pointer and length; this
    // copies the bytes before returning ownership to the paired free function.
    let bytes = unsafe { slice::from_raw_parts(buffer.data, buffer.len) }.to_vec();
    // SAFETY: each successful buffer must be released exactly once through the
    // free function supplied by the same provider.
    unsafe {
        free_buffer(buffer);
    }
    Ok(bytes)
}

fn invalid_owned_buffer(
    module: &'static str,
    reason: &str,
    buffer: VisionFfiOwnedBuffer,
) -> VisionFfiError {
    VisionFfiError::fatal_with_code(
        VisionFfiErrorCode::InvalidResponse,
        module,
        format!(
            "FFI backend returned invalid owned buffer metadata: reason={reason}; data_is_null={}; len={}; capacity={}; limit={MAX_FFI_RESPONSE_BYTES}; action=not_read_not_released",
            buffer.data.is_null(),
            buffer.len,
            buffer.capacity
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    static FREE_CALLS: AtomicUsize = AtomicUsize::new(0);
    static FREE_CALLS_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn take_owned_buffer_rejects_oversized_response_before_copy() {
        let _guard = FREE_CALLS_LOCK.lock().expect("free call lock");
        let buffer = VisionFfiOwnedBuffer {
            data: std::ptr::NonNull::<u8>::dangling().as_ptr(),
            len: MAX_FFI_RESPONSE_BYTES + 1,
            capacity: MAX_FFI_RESPONSE_BYTES + 1,
        };

        FREE_CALLS.store(0, Ordering::SeqCst);
        let err = take_owned_buffer("test", buffer, counting_noop_free_buffer)
            .expect_err("oversized buffer must be rejected");

        assert!(err.message().contains("oversized response buffer"));
        assert!(err.message().contains("action=not_read_not_released"));
        assert_eq!(FREE_CALLS.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn take_owned_buffer_rejects_null_data_with_nonzero_length() {
        let _guard = FREE_CALLS_LOCK.lock().expect("free call lock");
        let buffer = VisionFfiOwnedBuffer {
            data: std::ptr::null_mut(),
            len: 1,
            capacity: 1,
        };

        FREE_CALLS.store(0, Ordering::SeqCst);
        let err = take_owned_buffer("test", buffer, counting_noop_free_buffer)
            .expect_err("null data with non-zero length must be rejected");

        assert!(err.message().contains("null data pointer"));
        assert_eq!(FREE_CALLS.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn take_owned_buffer_rejects_capacity_smaller_than_length() {
        let _guard = FREE_CALLS_LOCK.lock().expect("free call lock");
        let buffer = VisionFfiOwnedBuffer {
            data: std::ptr::NonNull::<u8>::dangling().as_ptr(),
            len: 2,
            capacity: 1,
        };

        FREE_CALLS.store(0, Ordering::SeqCst);
        let err = take_owned_buffer("test", buffer, counting_noop_free_buffer)
            .expect_err("capacity smaller than length must be rejected");

        assert!(err.message().contains("capacity smaller than its length"));
        assert_eq!(FREE_CALLS.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn take_owned_buffer_rejects_oversized_capacity_without_deallocation() {
        let _guard = FREE_CALLS_LOCK.lock().expect("free call lock");
        let buffer = VisionFfiOwnedBuffer {
            data: std::ptr::NonNull::<u8>::dangling().as_ptr(),
            len: 1,
            capacity: MAX_FFI_RESPONSE_BYTES + 1,
        };

        FREE_CALLS.store(0, Ordering::SeqCst);
        let err = take_owned_buffer("test", buffer, counting_noop_free_buffer)
            .expect_err("oversized capacity must be rejected");

        assert!(err.message().contains("oversized response buffer"));
        assert_eq!(FREE_CALLS.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn take_owned_buffer_releases_valid_buffer_once() {
        let _guard = FREE_CALLS_LOCK.lock().expect("free call lock");
        let mut bytes = b"valid".to_vec();
        let buffer = VisionFfiOwnedBuffer {
            data: bytes.as_mut_ptr(),
            len: bytes.len(),
            capacity: bytes.capacity(),
        };
        std::mem::forget(bytes);

        FREE_CALLS.store(0, Ordering::SeqCst);
        let copied =
            take_owned_buffer("test", buffer, counting_free_buffer).expect("valid buffer accepted");

        assert_eq!(copied, b"valid");
        assert_eq!(FREE_CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cuda_stable_identity_uses_pci_only_when_uuid_api_is_absent() {
        assert_eq!(
            cuda_stable_identity("0000:02:00.0", None).expect("PCI identity"),
            "cuda-pci:0000:02:00.0"
        );
        assert_eq!(
            cuda_stable_identity("0000:02:00.0", Some((CUDA_SUCCESS, [0x11; 16])))
                .expect("UUID identity"),
            "cuda-uuid:11111111111111111111111111111111"
        );
    }

    #[test]
    fn cuda_stable_identity_fails_closed_when_uuid_query_fails_or_is_empty() {
        let err = cuda_stable_identity("0000:02:00.0", Some((1, [0x11; 16])))
            .expect_err("UUID query failure rejected");
        assert_eq!(err.code(), VisionFfiErrorCode::ProviderUnavailable);

        let err = cuda_stable_identity("0000:02:00.0", Some((CUDA_SUCCESS, [0; 16])))
            .expect_err("all-zero UUID rejected");
        assert_eq!(err.code(), VisionFfiErrorCode::InvalidResponse);
    }

    #[test]
    fn runtime_library_loadability_rejects_corrupt_file() {
        let path = std::env::temp_dir().join(format!(
            "actingcommand-corrupt-runtime-{}-{}.dll",
            std::process::id(),
            "loadability"
        ));
        std::fs::write(&path, b"not a dynamic library").expect("corrupt dll fixture");

        let err = validate_runtime_library_loadable("test-runtime", &path)
            .expect_err("corrupt runtime library rejected");

        assert_eq!(err.module(), "test-runtime");
        assert!(err.message().contains("failed to load FFI library"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn runtime_closure_loads_exact_paths_once_and_retains_all_handles() {
        let closure = Mutex::new(RuntimeLibraryClosureState::Uninitialized);
        let paths = test_runtime_closure_paths();
        let dropped = Arc::new(AtomicUsize::new(0));
        let mut loaded = Vec::new();

        establish_runtime_library_closure_with(&closure, &paths, |path| {
            loaded.push(path.to_path_buf());
            Ok(TestRuntimeHandle {
                _path: path.to_path_buf(),
                dropped: Arc::clone(&dropped),
            })
        })
        .expect("first exact closure establishes");

        assert_eq!(loaded, paths);
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
        match &*closure.lock().expect("closure state") {
            RuntimeLibraryClosureState::Ready { identity, _handles } => {
                assert_eq!(identity, &paths);
                assert_eq!(_handles.len(), paths.len());
            }
            _ => panic!("closure must be ready"),
        }

        establish_runtime_library_closure_with(
            &closure,
            &paths,
            |_| -> VisionFfiResult<TestRuntimeHandle> {
                panic!("same exact closure must not load again")
            },
        )
        .expect("same closure is idempotent");
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn runtime_closure_rejects_conflicting_order_without_loading() {
        let closure = Mutex::new(RuntimeLibraryClosureState::Uninitialized);
        let paths = test_runtime_closure_paths();
        establish_runtime_library_closure_with(&closure, &paths, |path| {
            Ok(TestRuntimeHandle {
                _path: path.to_path_buf(),
                dropped: Arc::new(AtomicUsize::new(0)),
            })
        })
        .expect("first closure establishes");

        let mut conflicting = paths.clone();
        conflicting.swap(0, 1);
        let err = establish_runtime_library_closure_with(
            &closure,
            &conflicting,
            |_| -> VisionFfiResult<TestRuntimeHandle> {
                panic!("conflicting closure must not load")
            },
        )
        .expect_err("different closure order must fail closed");

        assert_eq!(err.code(), VisionFfiErrorCode::ProviderUnavailable);
        assert!(err.message().contains("cannot change in-process"));
        match &*closure.lock().expect("closure state") {
            RuntimeLibraryClosureState::Ready { identity, .. } => assert_eq!(identity, &paths),
            _ => panic!("original closure must remain ready"),
        }
    }

    #[test]
    fn runtime_closure_partial_failure_is_permanent_and_retains_loaded_handles() {
        let closure = Mutex::new(RuntimeLibraryClosureState::Uninitialized);
        let paths = test_runtime_closure_paths();
        let dropped = Arc::new(AtomicUsize::new(0));
        let mut attempts = Vec::new();

        let err = establish_runtime_library_closure_with(&closure, &paths, |path| {
            attempts.push(path.to_path_buf());
            if attempts.len() == 2 {
                return Err(runtime_closure_error("synthetic load failure"));
            }
            Ok(TestRuntimeHandle {
                _path: path.to_path_buf(),
                dropped: Arc::clone(&dropped),
            })
        })
        .expect_err("partial closure load must fail");

        assert_eq!(err.code(), VisionFfiErrorCode::ProviderUnavailable);
        assert!(err.message().contains("permanently poisoned"));
        assert_eq!(attempts, paths[..2]);
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
        match &*closure.lock().expect("closure state") {
            RuntimeLibraryClosureState::Poisoned {
                identity,
                _handles,
                failed_path,
                reason,
            } => {
                assert_eq!(identity, &paths);
                assert_eq!(_handles.len(), 1);
                assert_eq!(failed_path, &paths[1]);
                assert!(reason.contains("synthetic load failure"));
            }
            _ => panic!("closure must remain poisoned"),
        }

        let retry = establish_runtime_library_closure_with(
            &closure,
            &paths,
            |_| -> VisionFfiResult<TestRuntimeHandle> { panic!("poisoned closure must not retry") },
        )
        .expect_err("poisoned closure rejects every retry");
        assert!(retry.message().contains("permanently poisoned"));
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn reconfigure_rejects_changed_companion_closure_before_state_mutation() {
        let inventory = test_cuda_inventory();
        let artifacts = test_ppocr_artifacts(test_runtime_closure_paths());
        let mut backend = unsafe {
            FastDeployPpocrBackend::from_raw_functions_with_artifacts_and_inventory(
                unreachable_ocr_invoke,
                noop_free_buffer,
                artifacts.clone(),
                Some(inventory.clone()),
            )
        }
        .expect("synthetic backend");
        let session_before = backend.session_for_test().expect("session before");

        let mut changed = artifacts.clone();
        changed.runtime_library_paths[1] = absolute_test_path("different-cudnn64_9.dll");
        let err = backend
            .reconfigure_with_inventory_for_test(changed, Some(inventory))
            .expect_err("changed companion closure rejected");

        assert_eq!(err.code(), VisionFfiErrorCode::ProviderUnavailable);
        assert!(err.message().contains("cannot change in-process"));
        assert_eq!(backend.artifacts.as_ref(), Some(&artifacts));
        let session_after = backend.session_for_test().expect("session after");
        assert_eq!(session_after.session_id(), session_before.session_id());
        assert_eq!(session_after.generation(), session_before.generation());
        assert!(backend._library.is_none());
    }

    #[cfg(windows)]
    #[test]
    fn windows_runtime_closure_uses_only_released_loadlibraryex_flags() {
        use libloading::os::windows::{
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
        };

        assert_eq!(
            PROCESS_RUNTIME_LIBRARY_LOAD_FLAGS,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32
        );
    }

    struct TestRuntimeHandle {
        _path: PathBuf,
        dropped: Arc<AtomicUsize>,
    }

    impl Drop for TestRuntimeHandle {
        fn drop(&mut self) {
            self.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn test_runtime_closure_paths() -> Vec<PathBuf> {
        vec![
            absolute_test_path("onnxruntime.dll"),
            absolute_test_path("cudnn64_9.dll"),
            absolute_test_path("cudnn_ops64_9.dll"),
        ]
    }

    fn absolute_test_path(name: &str) -> PathBuf {
        #[cfg(windows)]
        {
            PathBuf::from(format!(r"C:\synthetic-runtime\{name}"))
        }
        #[cfg(not(windows))]
        {
            PathBuf::from(format!("/synthetic-runtime/{name}"))
        }
    }

    fn test_ppocr_artifacts(runtime_library_paths: Vec<PathBuf>) -> FastDeployPpocrArtifacts {
        let detector_hash = "a".repeat(64);
        let recognizer_hash = "b".repeat(64);
        let dictionary_hash = "c".repeat(64);
        let model_hash = crate::ppocr_model_content_sha256(
            &detector_hash,
            &recognizer_hash,
            &dictionary_hash,
            None,
        )
        .expect("fixture model hash");
        FastDeployPpocrArtifacts {
            provider_library_path: absolute_test_path("ac_fastdeploy_ppocr.dll"),
            provider_library_sha256: Some("e".repeat(64)),
            runtime_library_path: Some(runtime_library_paths[0].clone()),
            runtime_library_paths,
            runtime_library_sha256: Some("d".repeat(64)),
            detector_model_path: absolute_test_path("detector.onnx"),
            recognizer_model_path: absolute_test_path("recognizer.onnx"),
            dictionary_path: absolute_test_path("dictionary.txt"),
            classifier_model_path: None,
            model_ref: Some(crate::PPOCR_V6_MEDIUM_MODEL_REF.to_string()),
            model_sha256: Some(model_hash),
            detector_model_sha256: Some(detector_hash),
            recognizer_model_sha256: Some(recognizer_hash),
            dictionary_sha256: Some(dictionary_hash),
            classifier_model_sha256: None,
            execution_provider: Some(OnnxExecutionProvider::Cuda),
            cuda_device: Some(crate::CudaDeviceSelector {
                ordinal: 0,
                expected_stable_identity: "cuda-uuid:11111111111111111111111111111111".to_string(),
            }),
            strict_no_fallback: Some(true),
            supported_languages: vec!["en".to_string()],
            default_timeout_ms: 1_000,
        }
    }

    fn test_cuda_inventory() -> CudaDeviceInventory {
        CudaDeviceInventory {
            driver_version: 12_800,
            devices: vec![CudaDeviceIdentity {
                ordinal: 0,
                stable_identity: "cuda-uuid:11111111111111111111111111111111".to_string(),
                pci_bus_id: Some("0000:01:00.0".to_string()),
            }],
        }
    }

    unsafe extern "C" fn unreachable_ocr_invoke(
        _request_ptr: *const u8,
        _request_len: usize,
        _response_out: *mut VisionFfiOwnedBuffer,
    ) -> i32 {
        panic!("synthetic reconfigure test must not invoke OCR")
    }

    unsafe extern "C" fn noop_free_buffer(_buffer: VisionFfiOwnedBuffer) {}

    unsafe extern "C" fn counting_noop_free_buffer(_buffer: VisionFfiOwnedBuffer) {
        FREE_CALLS.fetch_add(1, Ordering::SeqCst);
    }

    unsafe extern "C" fn counting_free_buffer(buffer: VisionFfiOwnedBuffer) {
        FREE_CALLS.fetch_add(1, Ordering::SeqCst);
        // SAFETY: this test function receives the exact metadata from the Vec
        // intentionally transferred by take_owned_buffer_releases_valid_buffer_once.
        unsafe {
            drop(Vec::from_raw_parts(
                buffer.data,
                buffer.len,
                buffer.capacity,
            ));
        }
    }
}
