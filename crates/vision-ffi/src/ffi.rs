// SPDX-License-Identifier: AGPL-3.0-only

use crate::{
    FastDeployPpocrArtifacts, FastDeployPpocrInvokeRequest, NnClassificationResult, NnEngine,
    NnInferenceRequest, OcrEngine, OcrInferenceRequest, OcrInferenceResult, OnnxRuntimeArtifacts,
    OnnxRuntimeInvokeRequest, VisionFfiError, VisionFfiResult, VisionProviderArtifactManifest,
};
use libloading::Library;
use serde::{Serialize, de::DeserializeOwned};
use std::ffi::OsStr;
use std::slice;
use std::sync::Arc;

const OCR_READ_TEXT_SYMBOL: &[u8] = b"ac_fastdeploy_ppocr_read_text_json\0";
const NN_CLASSIFY_SYMBOL: &[u8] = b"ac_onnxruntime_classify_json\0";
const FREE_BUFFER_SYMBOL: &[u8] = b"ac_vision_free_buffer\0";
const MAX_FFI_RESPONSE_BYTES: usize = 128 * 1024 * 1024;

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
        })
    }

    pub fn from_artifacts(artifacts: FastDeployPpocrArtifacts) -> VisionFfiResult<Self> {
        artifacts.validate_existing_files()?;
        let library = load_library("fastdeploy-ppocr", &artifacts.provider_library_path)?;
        let read_text_json = load_symbol(&library, "fastdeploy-ppocr", OCR_READ_TEXT_SYMBOL)?;
        let free_buffer = load_symbol(&library, "fastdeploy-ppocr", FREE_BUFFER_SYMBOL)?;
        Ok(Self {
            _library: Some(library),
            read_text_json,
            free_buffer,
            artifacts: Some(artifacts),
        })
    }

    pub fn from_manifest(manifest: &VisionProviderArtifactManifest) -> VisionFfiResult<Self> {
        Self::from_artifacts(manifest.require_fastdeploy_ppocr()?.clone())
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
        artifacts.validate()?;
        Ok(Self {
            _library: None,
            read_text_json,
            free_buffer,
            artifacts: Some(artifacts),
        })
    }
}

pub fn validate_fastdeploy_ppocr_provider_abi(path: impl AsRef<OsStr>) -> VisionFfiResult<()> {
    let library = load_library("fastdeploy-ppocr", path)?;
    let _: VisionFfiInvokeJson = load_symbol(&library, "fastdeploy-ppocr", OCR_READ_TEXT_SYMBOL)?;
    let _: VisionFfiFreeBuffer = load_symbol(&library, "fastdeploy-ppocr", FREE_BUFFER_SYMBOL)?;
    Ok(())
}

impl OcrEngine for FastDeployPpocrBackend {
    fn read_text(&mut self, request: OcrInferenceRequest) -> VisionFfiResult<OcrInferenceResult> {
        request.validate()?;
        if let Some(artifacts) = &self.artifacts {
            invoke_json(
                "fastdeploy-ppocr",
                self.read_text_json,
                self.free_buffer,
                &FastDeployPpocrInvokeRequest {
                    request,
                    artifacts: artifacts.clone(),
                },
            )
        } else {
            invoke_json(
                "fastdeploy-ppocr",
                self.read_text_json,
                self.free_buffer,
                &request,
            )
        }
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
        artifacts.validate_existing_files()?;
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
        Self::from_artifacts(manifest.require_onnxruntime()?.clone())
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
        artifacts.validate()?;
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
        if let Some(artifacts) = &self.artifacts {
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
        }
    }
}

fn load_library(module: &'static str, path: impl AsRef<OsStr>) -> VisionFfiResult<Arc<Library>> {
    let path = path.as_ref();
    // SAFETY: loading a dynamic library is the required FFI boundary. The
    // handle is retained in the backend so loaded symbols cannot outlive it.
    let library = unsafe { Library::new(path) }.map_err(|err| {
        VisionFfiError::fatal(
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
        return Err(VisionFfiError::fatal(
            module,
            format!("FFI backend returned status {status}: {response_text}"),
        ));
    }
    if response_bytes.is_empty() {
        return Err(VisionFfiError::fatal(
            module,
            "FFI backend returned an empty response",
        ));
    }
    serde_json::from_slice(&response_bytes).map_err(|err| {
        VisionFfiError::fatal(module, format!("failed to parse FFI response JSON: {err}"))
    })
}

fn take_owned_buffer(
    module: &'static str,
    buffer: VisionFfiOwnedBuffer,
    free_buffer: VisionFfiFreeBuffer,
) -> VisionFfiResult<Vec<u8>> {
    if buffer.len == 0 {
        if !buffer.data.is_null() {
            // SAFETY: non-null zero-length buffers are still owned by the FFI
            // provider and must be released through its paired free function.
            unsafe {
                free_buffer(buffer);
            }
        }
        return Ok(Vec::new());
    }
    if buffer.data.is_null() {
        return Err(VisionFfiError::fatal(
            module,
            "FFI backend returned a null data pointer with non-zero length",
        ));
    }
    if buffer.capacity < buffer.len {
        // SAFETY: the provider still owns this malformed buffer; release it
        // before reporting the ABI violation so leaks do not hide the failure.
        unsafe {
            free_buffer(buffer);
        }
        return Err(VisionFfiError::fatal(
            module,
            "FFI backend returned a buffer capacity smaller than its length",
        ));
    }
    if buffer.len > MAX_FFI_RESPONSE_BYTES {
        // SAFETY: this sane maximum is accident hardening for malformed
        // provider lengths. A malicious provider is still inside the FFI trust
        // boundary once its dynamic library is loaded.
        unsafe {
            free_buffer(buffer);
        }
        return Err(VisionFfiError::fatal(
            module,
            format!(
                "FFI backend returned an oversized response buffer: {} bytes exceeds {}",
                buffer.len, MAX_FFI_RESPONSE_BYTES
            ),
        ));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_owned_buffer_rejects_oversized_response_before_copy() {
        let buffer = VisionFfiOwnedBuffer {
            data: std::ptr::NonNull::<u8>::dangling().as_ptr(),
            len: MAX_FFI_RESPONSE_BYTES + 1,
            capacity: MAX_FFI_RESPONSE_BYTES + 1,
        };

        let err = take_owned_buffer("test", buffer, fake_free_buffer)
            .expect_err("oversized buffer must be rejected");

        assert!(err.message().contains("oversized response buffer"));
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

    unsafe extern "C" fn fake_free_buffer(_buffer: VisionFfiOwnedBuffer) {}
}
