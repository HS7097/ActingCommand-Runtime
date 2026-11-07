// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #407 GPU-1 A: the DXGI adapters of this machine and the one adapter a DirectML
//! configuration names (design `rd-1031/perf-407/design-gpu1.md` §1.3).
//!
//! An adapter is named by its stable identity `dxgi-pci:<vendor>:<device>:<subsys>:<rev>`
//! (`DXGI_ADAPTER_DESC1`), never by its DXGI index or its LUID. The adapters are listed in
//! `IDXGIFactory1::EnumAdapters1` order, which is the order ONNX Runtime's DirectML `device_id`
//! indexes (`dml_provider_factory.h` of ONNX Runtime 1.24.4). One physical card can be listed
//! more than once with the same identity (an indirect-display or paravirtualized copy that
//! reports itself as a render adapter), so the kernel's adapter type (`D3DKMT_ADAPTERTYPE`)
//! keeps only the physical ones. Exactly one must remain; its index at that moment is what ONNX
//! Runtime is given. The LUID changes at every boot: it is recorded as an observation and never
//! compared. Nothing here ever picks another adapter than the configured one.
//!
//! [`SystemAdapterSource`] reads DXGI through a minimal vtable and the adapter type through
//! `D3DKMTOpenAdapterFromLuid`, `D3DKMTQueryAdapterInfo` and `D3DKMTCloseAdapter`; `dxgi.dll` and
//! `gdi32.dll` are loaded from System32 only. Tests give [`resolve_directml_adapter`] a fake
//! [`AdapterSource`].

use crate::{
    DirectmlDeviceIdentity, DirectmlDeviceSelector, GpuFallback, GpuUnavailableReason,
    OnnxExecutionProvider, VisionFfiError, VisionFfiErrorCode, VisionFfiResult, VisionGpuCode,
};
use actingcommand_contract::outcome::Outcome;
use std::fmt;

/// The most DXGI adapters one listing reads.
pub const MAX_DXGI_ADAPTERS: u32 = 64;
/// The prefix of a DXGI adapter's stable identity.
pub const DXGI_PCI_IDENTITY_PREFIX: &str = "dxgi-pci:";
/// The hexadecimal widths of the identity's vendor, device, subsystem and revision.
const DXGI_PCI_IDENTITY_WIDTHS: [usize; 4] = [4, 4, 8, 2];
/// `DXGI_ADAPTER_FLAG_SOFTWARE` of `DXGI_ADAPTER_DESC1::Flags`.
pub const DXGI_ADAPTER_FLAG_SOFTWARE: u32 = 2;

const MODULE: &str = "directml_adapter";

/// The stable identity of an adapter with these `DXGI_ADAPTER_DESC1` values.
pub fn dxgi_pci_identity(vendor_id: u32, device_id: u32, subsys_id: u32, revision: u32) -> String {
    format!(
        "{DXGI_PCI_IDENTITY_PREFIX}{vendor_id:04x}:{device_id:04x}:{subsys_id:08x}:{revision:02x}"
    )
}

/// Whether `value` is a well-formed stable identity: the prefix, then lowercase hexadecimal of
/// exactly 4, 4, 8 and 2 digits separated by colons.
pub fn is_dxgi_pci_identity(value: &str) -> bool {
    let Some(rest) = value.strip_prefix(DXGI_PCI_IDENTITY_PREFIX) else {
        return false;
    };
    let parts = rest.split(':').collect::<Vec<_>>();
    parts.len() == DXGI_PCI_IDENTITY_WIDTHS.len()
        && parts
            .iter()
            .zip(DXGI_PCI_IDENTITY_WIDTHS)
            .all(|(part, width)| {
                part.len() == width
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
}

/// An adapter's locally unique identifier: valid until the next boot, an observation only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DxgiLuid {
    pub high_part: i32,
    pub low_part: u32,
}

impl fmt::Display for DxgiLuid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "0x{:08x}{:08x}",
            self.high_part.cast_unsigned(),
            self.low_part
        )
    }
}

/// `D3DKMT_ADAPTERTYPE` as the kernel reports it (`KMTQAITYPE_ADAPTERTYPE`), with its bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct D3dkmtAdapterType(pub u32);

impl D3dkmtAdapterType {
    pub const RENDER_SUPPORTED: u32 = 1 << 0;
    pub const DISPLAY_SUPPORTED: u32 = 1 << 1;
    pub const SOFTWARE_DEVICE: u32 = 1 << 2;
    pub const INDIRECT_DISPLAY_DEVICE: u32 = 1 << 6;
    pub const PARAVIRTUALIZED: u32 = 1 << 7;

    pub const fn render_supported(self) -> bool {
        (self.0 & Self::RENDER_SUPPORTED) != 0
    }

    pub const fn software_device(self) -> bool {
        (self.0 & Self::SOFTWARE_DEVICE) != 0
    }

    pub const fn indirect_display_device(self) -> bool {
        (self.0 & Self::INDIRECT_DISPLAY_DEVICE) != 0
    }

    pub const fn paravirtualized(self) -> bool {
        (self.0 & Self::PARAVIRTUALIZED) != 0
    }
}

/// An adapter's PCI location (`D3DKMT_ADAPTERADDRESS`), an observation; it is no part of the
/// identity (design §9 Q10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PciAddress {
    pub bus: u32,
    pub device: u32,
    pub function: u32,
}

impl fmt::Display for PciAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:02x}:{:02x}.{:x}",
            self.bus, self.device, self.function
        )
    }
}

/// The kernel graphics call that failed while reading an adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3dkmtCall {
    OpenAdapterFromLuid,
    QueryAdapterType,
    CloseAdapter,
}

impl D3dkmtCall {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAdapterFromLuid => "D3DKMTOpenAdapterFromLuid",
            Self::QueryAdapterType => "D3DKMTQueryAdapterInfo(KMTQAITYPE_ADAPTERTYPE)",
            Self::CloseAdapter => "D3DKMTCloseAdapter",
        }
    }
}

/// What the kernel said about one adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelAdapterReading {
    /// The adapter type and, when the kernel gave it, the PCI location.
    Read {
        adapter_type: D3dkmtAdapterType,
        pci_address: Option<PciAddress>,
    },
    /// A kernel call failed with this NTSTATUS; the adapter cannot be shown to be physical.
    Failed { call: D3dkmtCall, ntstatus: i32 },
}

/// One DXGI adapter as one listing observed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DxgiAdapter {
    /// The adapter's index in this listing's `EnumAdapters1` order.
    pub dxgi_index: u32,
    pub vendor_id: u32,
    pub device_id: u32,
    pub subsys_id: u32,
    pub revision: u32,
    pub luid: DxgiLuid,
    /// `DXGI_ADAPTER_DESC1::Flags`.
    pub dxgi_flags: u32,
    pub kernel: KernelAdapterReading,
}

impl DxgiAdapter {
    /// The stable identity `dxgi-pci:<vendor>:<device>:<subsys>:<rev>`.
    pub fn identity(&self) -> String {
        dxgi_pci_identity(
            self.vendor_id,
            self.device_id,
            self.subsys_id,
            self.revision,
        )
    }

    /// The kernel's adapter type, when it could be read.
    pub const fn adapter_type(&self) -> Option<D3dkmtAdapterType> {
        match self.kernel {
            KernelAdapterReading::Read { adapter_type, .. } => Some(adapter_type),
            KernelAdapterReading::Failed { .. } => None,
        }
    }

    /// The PCI location, when the kernel gave it.
    pub const fn pci_address(&self) -> Option<PciAddress> {
        match self.kernel {
            KernelAdapterReading::Read { pci_address, .. } => pci_address,
            KernelAdapterReading::Failed { .. } => None,
        }
    }

    /// A software adapter, by DXGI's flag or by the kernel's type.
    pub fn software(&self) -> bool {
        (self.dxgi_flags & DXGI_ADAPTER_FLAG_SOFTWARE) != 0
            || self
                .adapter_type()
                .is_some_and(D3dkmtAdapterType::software_device)
    }

    pub fn indirect_display(&self) -> bool {
        self.adapter_type()
            .is_some_and(D3dkmtAdapterType::indirect_display_device)
    }

    pub fn paravirtualized(&self) -> bool {
        self.adapter_type()
            .is_some_and(D3dkmtAdapterType::paravirtualized)
    }

    /// A physical adapter (design §1.3): the kernel's type was read, it renders, and it is
    /// neither a software, an indirect-display nor a paravirtualized device.
    pub fn physical(&self) -> bool {
        self.adapter_type()
            .is_some_and(D3dkmtAdapterType::render_supported)
            && !self.software()
            && !self.indirect_display()
            && !self.paravirtualized()
    }
}

/// Where the adapters come from: the system, or a fake in tests.
pub trait AdapterSource {
    /// Every DXGI adapter, in `IDXGIFactory1::EnumAdapters1` order. A listing that cannot be
    /// read fails; an adapter whose kernel type cannot be read is listed with the failure.
    fn adapters(&self) -> VisionFfiResult<Vec<DxgiAdapter>>;
}

/// The system's adapters: DXGI and the kernel graphics interface, both from System32.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemAdapterSource;

impl AdapterSource for SystemAdapterSource {
    fn adapters(&self) -> VisionFfiResult<Vec<DxgiAdapter>> {
        system::adapters()
    }
}

/// The one adapter a resolution chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDirectmlAdapter {
    /// Its index in this listing, which ONNX Runtime's DirectML `device_id` is given.
    pub dxgi_index: u32,
    pub identity: String,
    /// Observed in this listing; never compared.
    pub luid: DxgiLuid,
    pub pci_address: Option<PciAddress>,
}

impl ResolvedDirectmlAdapter {
    fn of(adapter: &DxgiAdapter) -> Self {
        Self {
            dxgi_index: adapter.dxgi_index,
            identity: adapter.identity(),
            luid: adapter.luid,
            pci_address: adapter.pci_address(),
        }
    }

    /// The identity a DirectML session is bound to: the identity and the index, not the LUID.
    pub fn device(&self) -> DirectmlDeviceIdentity {
        DirectmlDeviceIdentity {
            dxgi_index: self.dxgi_index,
            identity: self.identity.clone(),
        }
    }
}

/// `<identity>;index=<n>;luid=<luid>`, as a startup record's resolved binding spells it (design
/// §2.5).
impl fmt::Display for ResolvedDirectmlAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{};index={};luid={}",
            self.identity, self.dxgi_index, self.luid
        )
    }
}

/// Why no adapter was chosen, with the failure behind it when there was one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectmlUnavailable {
    pub reason: GpuUnavailableReason,
    pub cause: Option<VisionFfiError>,
}

impl DirectmlUnavailable {
    fn new(reason: GpuUnavailableReason, cause: Option<VisionFfiError>) -> Self {
        Self { reason, cause }
    }

    /// The warning `vision_gpu_unavailable` for this reason under `gpu_fallback`; the caller
    /// adds `stage` and, for one model, `model_name`. The OS failure behind the reason, when
    /// there is one, is its `caused_by` link.
    pub fn outcome(&self, gpu_fallback: GpuFallback) -> Outcome {
        let outcome = Outcome::new(VisionGpuCode::VisionGpuUnavailable)
            .execution_provider(OnnxExecutionProvider::Directml)
            .gpu_unavailable_reason(self.reason)
            .gpu_fallback(gpu_fallback);
        match self.cause.as_ref().and_then(VisionFfiError::os_cause) {
            Some(cause) => outcome.caused_by(cause.clone()),
            None => outcome,
        }
    }
}

impl fmt::Display for DirectmlUnavailable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "no DirectML adapter: {}", self.reason)?;
        if let Some(cause) = &self.cause {
            write!(formatter, " ({cause})")?;
        }
        Ok(())
    }
}

/// One resolution: every adapter listed, in DXGI order, and what was decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectmlAdapterResolution {
    pub adapters: Vec<DxgiAdapter>,
    pub decision: Result<ResolvedDirectmlAdapter, DirectmlUnavailable>,
}

/// Lists the adapters of `source` and chooses the one `selector` names (design §1.3):
/// - the adapters whose identity equals the selector's, in DXGI order;
/// - of those, the physical ones;
/// - exactly one: it is chosen with its index at this moment; none with the identity:
///   `adapter_missing`; some, none physical: `adapter_not_physical`; two or more physical:
///   only those whose PCI location could be read stay, and unless exactly one does,
///   `adapter_ambiguous`.
///
/// A listing that fails is `adapter_missing` with the failure. The LUID takes no part, and no
/// other adapter is ever chosen in place of the configured one.
pub fn resolve_directml_adapter(
    source: &dyn AdapterSource,
    selector: &DirectmlDeviceSelector,
) -> DirectmlAdapterResolution {
    match source.adapters() {
        Ok(adapters) => {
            let decision = select_directml_adapter(&adapters, selector);
            DirectmlAdapterResolution { adapters, decision }
        }
        Err(error) => DirectmlAdapterResolution {
            adapters: Vec::new(),
            decision: Err(DirectmlUnavailable::new(
                GpuUnavailableReason::AdapterMissing,
                Some(error),
            )),
        },
    }
}

fn select_directml_adapter(
    adapters: &[DxgiAdapter],
    selector: &DirectmlDeviceSelector,
) -> Result<ResolvedDirectmlAdapter, DirectmlUnavailable> {
    if let Err(error) = selector.validate() {
        return Err(DirectmlUnavailable::new(
            GpuUnavailableReason::AdapterMissing,
            Some(error),
        ));
    }
    let matching = adapters
        .iter()
        .filter(|adapter| adapter.identity() == selector.expected_identity)
        .collect::<Vec<_>>();
    if matching.is_empty() {
        return Err(DirectmlUnavailable::new(
            GpuUnavailableReason::AdapterMissing,
            None,
        ));
    }
    let physical = matching
        .iter()
        .copied()
        .filter(|adapter| adapter.physical())
        .collect::<Vec<_>>();
    let chosen = match physical.as_slice() {
        [] => {
            return Err(DirectmlUnavailable::new(
                GpuUnavailableReason::AdapterNotPhysical,
                kernel_failure(&matching),
            ));
        }
        [only] => *only,
        several => {
            let addressed = several
                .iter()
                .copied()
                .filter(|adapter| adapter.pci_address().is_some())
                .collect::<Vec<_>>();
            match addressed.as_slice() {
                [only] => *only,
                _ => {
                    return Err(DirectmlUnavailable::new(
                        GpuUnavailableReason::AdapterAmbiguous,
                        None,
                    ));
                }
            }
        }
    };
    Ok(ResolvedDirectmlAdapter::of(chosen))
}

/// The first kernel failure among the adapters with the configured identity, as the cause of
/// `adapter_not_physical`.
fn kernel_failure(matching: &[&DxgiAdapter]) -> Option<VisionFfiError> {
    matching.iter().find_map(|adapter| match adapter.kernel {
        KernelAdapterReading::Failed { call, ntstatus } => Some(VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::ProviderUnavailable,
            MODULE,
            format!(
                "{} failed with NTSTATUS 0x{:08x} for DXGI adapter {}",
                call.as_str(),
                ntstatus.cast_unsigned(),
                adapter.dxgi_index
            ),
        )),
        KernelAdapterReading::Read { .. } => None,
    })
}

#[cfg(windows)]
mod system {
    //! DXGI through a minimal hand-written vtable (vision-ffi depends on no Windows bindings),
    //! and the kernel's adapter type and PCI location through the `D3DKMT` exports of
    //! `gdi32.dll`. Both libraries are loaded from System32 only, once, and stay loaded.

    use super::{
        D3dkmtAdapterType, D3dkmtCall, DxgiAdapter, DxgiLuid, KernelAdapterReading,
        MAX_DXGI_ADAPTERS, MODULE, PciAddress,
    };
    use crate::{VisionFfiError, VisionFfiErrorCode, VisionFfiResult};
    use actingcommand_contract::outcome::{IoOp, Outcome};
    use libloading::os::windows::{LOAD_LIBRARY_SEARCH_SYSTEM32, Library};
    use std::ffi::c_void;
    use std::ptr::null_mut;
    use std::sync::Mutex;

    /// `GUID`; read by DXGI through its pointer only.
    #[repr(C)]
    struct Guid {
        _data1: u32,
        _data2: u16,
        _data3: u16,
        _data4: [u8; 8],
    }

    /// `IID_IDXGIFactory1` {770aae78-f26f-4dba-a829-253c83d1b387}.
    const IID_IDXGI_FACTORY1: Guid = Guid {
        _data1: 0x770a_ae78,
        _data2: 0xf26f,
        _data3: 0x4dba,
        _data4: [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87],
    };

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Luid {
        low_part: u32,
        high_part: i32,
    }

    /// `DXGI_ADAPTER_DESC1`; the description and the memory sizes are not read.
    #[repr(C)]
    struct DxgiAdapterDesc1 {
        _description: [u16; 128],
        vendor_id: u32,
        device_id: u32,
        sub_sys_id: u32,
        revision: u32,
        _dedicated_video_memory: usize,
        _dedicated_system_memory: usize,
        _shared_system_memory: usize,
        adapter_luid: Luid,
        flags: u32,
    }

    /// `D3DKMT_OPENADAPTERFROMLUID`: the LUID in, the adapter handle out.
    #[repr(C)]
    struct D3dkmtOpenAdapterFromLuid {
        _adapter_luid: Luid,
        adapter: u32,
    }

    /// `D3DKMT_QUERYADAPTERINFO`; read by the kernel through its pointer only.
    #[repr(C)]
    struct D3dkmtQueryAdapterInfo {
        _adapter: u32,
        _query_type: u32,
        _private_driver_data: *mut c_void,
        _private_driver_data_size: u32,
    }

    /// `D3DKMT_CLOSEADAPTER`; read by the kernel through its pointer only.
    #[repr(C)]
    struct D3dkmtCloseAdapter {
        _adapter: u32,
    }

    /// `D3DKMT_ADAPTERADDRESS`.
    #[repr(C)]
    #[derive(Default)]
    struct D3dkmtAdapterAddress {
        bus_number: u32,
        device_number: u32,
        function_number: u32,
    }

    const DXGI_ERROR_NOT_FOUND: i32 = 0x887A_0002_u32.cast_signed();
    /// `KMTQUERYADAPTERINFOTYPE` values (`d3dkmthk.h`).
    const KMTQAITYPE_ADAPTERADDRESS: u32 = 6;
    const KMTQAITYPE_ADAPTERTYPE: u32 = 15;

    type CreateDxgiFactory1 = unsafe extern "system" fn(*const Guid, *mut *mut c_void) -> i32;
    type Release = unsafe extern "system" fn(*mut c_void) -> u32;
    type EnumAdapters1 = unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32;
    type GetDesc1 = unsafe extern "system" fn(*mut c_void, *mut DxgiAdapterDesc1) -> i32;
    type OpenAdapterFromLuid = unsafe extern "system" fn(*mut D3dkmtOpenAdapterFromLuid) -> i32;
    type QueryAdapterInfo = unsafe extern "system" fn(*const D3dkmtQueryAdapterInfo) -> i32;
    type CloseAdapter = unsafe extern "system" fn(*const D3dkmtCloseAdapter) -> i32;

    /// IUnknown 0-2, IDXGIObject 3-6, IDXGIFactory 7-11, IDXGIFactory1 12 `EnumAdapters1`.
    const FACTORY1_ENUM_ADAPTERS1: usize = 12;
    /// IUnknown 0-2, IDXGIObject 3-6, IDXGIAdapter 7-9, IDXGIAdapter1 10 `GetDesc1`.
    const ADAPTER1_GET_DESC1: usize = 10;
    const UNKNOWN_RELEASE: usize = 2;

    #[derive(Clone, Copy)]
    struct DisplayApi {
        create_factory: CreateDxgiFactory1,
        open_adapter: OpenAdapterFromLuid,
        query_adapter: QueryAdapterInfo,
        close_adapter: CloseAdapter,
    }

    struct LoadedDisplayApi {
        api: DisplayApi,
        _dxgi: Library,
        _gdi32: Library,
    }

    /// The two libraries, loaded on the first listing and kept for the process lifetime.
    static DISPLAY_API: Mutex<Option<LoadedDisplayApi>> = Mutex::new(None);

    fn unavailable(message: impl Into<String>) -> VisionFfiError {
        VisionFfiError::fatal_with_code(VisionFfiErrorCode::ProviderUnavailable, MODULE, message)
    }

    fn hresult_failure(call: &str, status: i32) -> VisionFfiError {
        unavailable(format!(
            "{call} failed with HRESULT 0x{:08x}",
            status.cast_unsigned()
        ))
        .with_os_cause(Outcome::from_io_error(
            &std::io::Error::from_raw_os_error(status),
            IoOp::Inspect,
        ))
    }

    fn load_system32(name: &str) -> VisionFfiResult<Library> {
        // SAFETY: a system library loaded from System32 only; its initialisation runs no code
        // of ours.
        unsafe { Library::load_with_flags(name, LOAD_LIBRARY_SEARCH_SYSTEM32) }.map_err(|error| {
            let failure = unavailable(format!("{name} did not load from System32: {error}"));
            match std::error::Error::source(&error)
                .and_then(|source| source.downcast_ref::<std::io::Error>())
            {
                Some(os_error) => {
                    failure.with_os_cause(Outcome::from_io_error(os_error, IoOp::Open))
                }
                None => failure,
            }
        })
    }

    /// # Safety
    /// `T` is the documented signature of the export `symbol` of `library`.
    unsafe fn export<T: Copy>(library: &Library, name: &str, symbol: &[u8]) -> VisionFfiResult<T> {
        // SAFETY: the caller names the export's documented signature.
        unsafe { library.get::<T>(symbol) }
            .map(|symbol| *symbol)
            .map_err(|error| unavailable(format!("{name} lacks an export: {error}")))
    }

    fn display_api() -> VisionFfiResult<DisplayApi> {
        let mut loaded = DISPLAY_API.lock().map_err(|_| {
            unavailable("the display library state is poisoned; restart is required")
        })?;
        if let Some(loaded) = loaded.as_ref() {
            return Ok(loaded.api);
        }
        let dxgi = load_system32("dxgi.dll")?;
        let gdi32 = load_system32("gdi32.dll")?;
        // SAFETY: each type is the export's documented signature; both libraries stay loaded in
        // DISPLAY_API for the process lifetime, so the pointers stay valid.
        let api = unsafe {
            DisplayApi {
                create_factory: export(&dxgi, "dxgi.dll", b"CreateDXGIFactory1\0")?,
                open_adapter: export(&gdi32, "gdi32.dll", b"D3DKMTOpenAdapterFromLuid\0")?,
                query_adapter: export(&gdi32, "gdi32.dll", b"D3DKMTQueryAdapterInfo\0")?,
                close_adapter: export(&gdi32, "gdi32.dll", b"D3DKMTCloseAdapter\0")?,
            }
        };
        *loaded = Some(LoadedDisplayApi {
            api,
            _dxgi: dxgi,
            _gdi32: gdi32,
        });
        Ok(api)
    }

    /// One COM object this module owns a reference to.
    struct ComObject(*mut c_void);

    impl ComObject {
        /// # Safety
        /// `index` is a slot of the object's vtable whose function has type `F`.
        unsafe fn method<F: Copy>(&self, index: usize) -> F {
            // SAFETY: a COM object starts with its vtable pointer; the caller names a valid slot.
            unsafe {
                let vtable = *(self.0 as *const *const usize);
                let entry = *vtable.add(index);
                std::mem::transmute_copy::<usize, F>(&entry)
            }
        }
    }

    impl Drop for ComObject {
        fn drop(&mut self) {
            // SAFETY: slot 2 of every COM vtable is IUnknown::Release; this value owns one
            // reference.
            unsafe {
                let release: Release = self.method(UNKNOWN_RELEASE);
                release(self.0);
            }
        }
    }

    pub(super) fn adapters() -> VisionFfiResult<Vec<DxgiAdapter>> {
        let api = display_api()?;
        let mut factory: *mut c_void = null_mut();
        // SAFETY: a valid IID and a writable out pointer.
        let status = unsafe { (api.create_factory)(&IID_IDXGI_FACTORY1, &mut factory) };
        if status < 0 || factory.is_null() {
            return Err(hresult_failure("CreateDXGIFactory1", status));
        }
        let factory = ComObject(factory);
        let mut adapters = Vec::new();
        for index in 0..=MAX_DXGI_ADAPTERS {
            let mut adapter: *mut c_void = null_mut();
            // SAFETY: slot 12 of IDXGIFactory1 is EnumAdapters1; the out pointer is writable.
            let status = unsafe {
                let enum_adapters: EnumAdapters1 = factory.method(FACTORY1_ENUM_ADAPTERS1);
                enum_adapters(factory.0, index, &mut adapter)
            };
            if status == DXGI_ERROR_NOT_FOUND {
                return Ok(adapters);
            }
            if status < 0 || adapter.is_null() {
                return Err(hresult_failure("IDXGIFactory1::EnumAdapters1", status));
            }
            let adapter = ComObject(adapter);
            if index == MAX_DXGI_ADAPTERS {
                return Err(unavailable(format!(
                    "DXGI lists more than {MAX_DXGI_ADAPTERS} adapters"
                )));
            }
            let mut desc = DxgiAdapterDesc1 {
                _description: [0; 128],
                vendor_id: 0,
                device_id: 0,
                sub_sys_id: 0,
                revision: 0,
                _dedicated_video_memory: 0,
                _dedicated_system_memory: 0,
                _shared_system_memory: 0,
                adapter_luid: Luid {
                    low_part: 0,
                    high_part: 0,
                },
                flags: 0,
            };
            // SAFETY: slot 10 of IDXGIAdapter1 is GetDesc1; `desc` is writable.
            let status = unsafe {
                let get_desc: GetDesc1 = adapter.method(ADAPTER1_GET_DESC1);
                get_desc(adapter.0, &mut desc)
            };
            if status < 0 {
                return Err(hresult_failure("IDXGIAdapter1::GetDesc1", status));
            }
            adapters.push(DxgiAdapter {
                dxgi_index: index,
                vendor_id: desc.vendor_id,
                device_id: desc.device_id,
                subsys_id: desc.sub_sys_id,
                revision: desc.revision,
                luid: DxgiLuid {
                    high_part: desc.adapter_luid.high_part,
                    low_part: desc.adapter_luid.low_part,
                },
                dxgi_flags: desc.flags,
                kernel: kernel_reading(api, desc.adapter_luid),
            });
        }
        Err(unavailable(format!(
            "DXGI lists more than {MAX_DXGI_ADAPTERS} adapters"
        )))
    }

    fn kernel_reading(api: DisplayApi, luid: Luid) -> KernelAdapterReading {
        let mut open = D3dkmtOpenAdapterFromLuid {
            _adapter_luid: luid,
            adapter: 0,
        };
        // SAFETY: a valid in/out structure for the call's duration.
        let status = unsafe { (api.open_adapter)(&mut open) };
        if status < 0 {
            return KernelAdapterReading::Failed {
                call: D3dkmtCall::OpenAdapterFromLuid,
                ntstatus: status,
            };
        }
        let mut adapter_type = 0_u32;
        let type_status = query(api, open.adapter, KMTQAITYPE_ADAPTERTYPE, &mut adapter_type);
        let mut address = D3dkmtAdapterAddress::default();
        let address_status = query(api, open.adapter, KMTQAITYPE_ADAPTERADDRESS, &mut address);
        let close = D3dkmtCloseAdapter {
            _adapter: open.adapter,
        };
        // SAFETY: the handle came from D3DKMTOpenAdapterFromLuid and is closed once.
        let close_status = unsafe { (api.close_adapter)(&close) };
        if type_status < 0 {
            return KernelAdapterReading::Failed {
                call: D3dkmtCall::QueryAdapterType,
                ntstatus: type_status,
            };
        }
        if close_status < 0 {
            return KernelAdapterReading::Failed {
                call: D3dkmtCall::CloseAdapter,
                ntstatus: close_status,
            };
        }
        KernelAdapterReading::Read {
            adapter_type: D3dkmtAdapterType(adapter_type),
            pci_address: (address_status >= 0).then_some(PciAddress {
                bus: address.bus_number,
                device: address.device_number,
                function: address.function_number,
            }),
        }
    }

    /// One `D3DKMTQueryAdapterInfo` into `data` (a `u32` or a `D3DKMT_ADAPTERADDRESS`); its
    /// NTSTATUS.
    fn query<T>(api: DisplayApi, adapter: u32, query_type: u32, data: &mut T) -> i32 {
        let info = D3dkmtQueryAdapterInfo {
            _adapter: adapter,
            _query_type: query_type,
            _private_driver_data: (data as *mut T).cast(),
            _private_driver_data_size: size_of::<T>() as u32,
        };
        // SAFETY: `data` is writable for its whole size for the call's duration.
        unsafe { (api.query_adapter)(&info) }
    }
}

#[cfg(not(windows))]
mod system {
    use super::DxgiAdapter;
    use crate::{VisionFfiError, VisionFfiErrorCode, VisionFfiResult};

    pub(super) fn adapters() -> VisionFfiResult<Vec<DxgiAdapter>> {
        Err(VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::ProviderUnavailable,
            super::MODULE,
            "DXGI adapters exist only on Windows",
        ))
    }
}
