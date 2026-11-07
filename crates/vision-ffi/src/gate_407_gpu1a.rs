// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #407 GPU-1 A (design rd-1031/perf-407/design-gpu1.md §1.3, §3 and §5; coordinator
// work order rd-1031/perf-407/brief-gpu1a.md): DirectML adapter selection over a fake adapter
// source, the DirectML runtime-library closure, and the runtime pin against the tool sources.
// No GPU, no DXGI and no ONNX Runtime: every adapter is a fake with a fake identity, every
// runtime library a placeholder file that is never loaded.

use crate::ffi::{RuntimeLibraryClosureState, establish_runtime_library_closure_with};
use crate::{
    AdapterSource, D3dkmtAdapterType, D3dkmtCall, DIRECTML_RUNTIME_PIN, DIRECTML_TOOL_SOURCE,
    DXGI_ADAPTER_FLAG_SOFTWARE, DirectmlDeviceSelector, DirectmlUnavailable, DxgiAdapter, DxgiLuid,
    GpuFallback, GpuUnavailableReason, KernelAdapterReading, ONNXRUNTIME_DIRECTML_TOOL_SOURCE,
    OcrSessionKey, OcrSessionKeyParts, OnnxExecutionProvider, PciAddress, VisionFfiError,
    VisionFfiErrorCode, VisionFfiResult, VisionGpuCode, VisionRuntimeSpec,
    ppocr_model_content_sha256, resolve_directml_adapter,
};
use actingcommand_contract::outcome::{Category, Outcome};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The tool sources the runtime pin must equal.
const TOOL_SOURCES: &str =
    include_str!("../../../scripts/windows-tools/windows-tool-sources.v1.json");

/// A fake discrete card, a fake integrated adapter and a fake software adapter.
const CARD: (u32, u32, u32, u32) = (0x1234, 0x5678, 0x9abc_def0, 0x01);
const CARD_IDENTITY: &str = "dxgi-pci:1234:5678:9abcdef0:01";
const INTEGRATED: (u32, u32, u32, u32) = (0x4321, 0x8765, 0x0fed_cba9, 0x02);
const SOFTWARE: (u32, u32, u32, u32) = (0x1111, 0x2222, 0, 0);

const RENDER_AND_DISPLAY: u32 =
    D3dkmtAdapterType::RENDER_SUPPORTED | D3dkmtAdapterType::DISPLAY_SUPPORTED;

/// A fixed adapter listing, or a listing that fails.
struct FakeAdapters(VisionFfiResult<Vec<DxgiAdapter>>);

impl AdapterSource for FakeAdapters {
    fn adapters(&self) -> VisionFfiResult<Vec<DxgiAdapter>> {
        self.0.clone()
    }
}

fn adapter(
    dxgi_index: u32,
    (vendor_id, device_id, subsys_id, revision): (u32, u32, u32, u32),
    luid_low: u32,
    kernel: KernelAdapterReading,
) -> DxgiAdapter {
    DxgiAdapter {
        dxgi_index,
        vendor_id,
        device_id,
        subsys_id,
        revision,
        luid: DxgiLuid {
            high_part: 0,
            low_part: luid_low,
        },
        dxgi_flags: 0,
        kernel,
    }
}

fn read(bits: u32, pci: Option<(u32, u32, u32)>) -> KernelAdapterReading {
    KernelAdapterReading::Read {
        adapter_type: D3dkmtAdapterType(bits),
        pci_address: pci.map(|(bus, device, function)| PciAddress {
            bus,
            device,
            function,
        }),
    }
}

fn physical(dxgi_index: u32, ids: (u32, u32, u32, u32), luid_low: u32, bus: u32) -> DxgiAdapter {
    adapter(
        dxgi_index,
        ids,
        luid_low,
        read(RENDER_AND_DISPLAY, Some((bus, 0, 0))),
    )
}

/// A copy of a card that reports itself as a render adapter: `kind` is the indirect-display or
/// the paravirtualized bit.
fn copy(dxgi_index: u32, ids: (u32, u32, u32, u32), luid_low: u32, kind: u32) -> DxgiAdapter {
    adapter(
        dxgi_index,
        ids,
        luid_low,
        read(D3dkmtAdapterType::RENDER_SUPPORTED | kind, None),
    )
}

fn software(dxgi_index: u32, luid_low: u32) -> DxgiAdapter {
    let mut adapter = adapter(
        dxgi_index,
        SOFTWARE,
        luid_low,
        read(
            D3dkmtAdapterType::RENDER_SUPPORTED | D3dkmtAdapterType::SOFTWARE_DEVICE,
            None,
        ),
    );
    adapter.dxgi_flags = DXGI_ADAPTER_FLAG_SOFTWARE;
    adapter
}

/// The shape design §1.3 reports for the production host, with fake identities and LUIDs: the
/// card, an integrated adapter, two render copies of the card and the software adapter.
fn host_listing(luid_base: u32) -> Vec<DxgiAdapter> {
    vec![
        physical(0, CARD, luid_base, 1),
        physical(1, INTEGRATED, luid_base + 1, 2),
        copy(
            2,
            CARD,
            luid_base + 2,
            D3dkmtAdapterType::INDIRECT_DISPLAY_DEVICE,
        ),
        copy(3, CARD, luid_base + 3, D3dkmtAdapterType::PARAVIRTUALIZED),
        software(4, luid_base + 4),
    ]
}

fn selector(identity: &str) -> DirectmlDeviceSelector {
    DirectmlDeviceSelector {
        expected_identity: identity.to_string(),
    }
}

fn unavailable(
    adapters: Vec<DxgiAdapter>,
    identity: &str,
) -> (Vec<DxgiAdapter>, DirectmlUnavailable) {
    let resolution = resolve_directml_adapter(&FakeAdapters(Ok(adapters)), &selector(identity));
    let error = resolution
        .decision
        .expect_err("the identity must not resolve to an adapter");
    (resolution.adapters, error)
}

fn assert_warning(outcome: &Outcome, reason: GpuUnavailableReason, fallback: GpuFallback) {
    assert!(outcome.is(VisionGpuCode::VisionGpuUnavailable));
    assert_eq!(outcome.category(), Category::Warning);
    assert_eq!(
        outcome.value("execution_provider"),
        Some(&json!(OnnxExecutionProvider::Directml.as_str()))
    );
    assert_eq!(
        outcome.value("gpu_unavailable_reason"),
        Some(&json!(reason.as_str()))
    );
    assert_eq!(
        outcome.value("gpu_fallback"),
        Some(&json!(fallback.as_str()))
    );
}

#[test]
fn an_identity_resolves_the_one_physical_adapter_among_its_copies() {
    let listing = host_listing(0x1000);
    assert!(listing[0].physical());
    assert!(listing[1].physical());
    assert!(listing[2].indirect_display() && !listing[2].physical());
    assert!(listing[3].paravirtualized() && !listing[3].physical());
    assert!(listing[4].software() && !listing[4].physical());

    let resolution =
        resolve_directml_adapter(&FakeAdapters(Ok(listing.clone())), &selector(CARD_IDENTITY));
    assert_eq!(resolution.adapters, listing, "every adapter, in DXGI order");
    let chosen = resolution.decision.expect("the one physical card");
    assert_eq!(chosen.dxgi_index, 0);
    assert_eq!(chosen.identity, CARD_IDENTITY);
    assert_eq!(chosen.luid, listing[0].luid);
    assert_eq!(chosen.pci_address, listing[0].pci_address());
    assert_eq!(chosen.device().dxgi_index, 0);
    assert_eq!(chosen.device().identity, CARD_IDENTITY);

    // The physical card listed after its copies: its own index at this moment is handed on.
    let reordered = vec![
        copy(0, CARD, 0x2000, D3dkmtAdapterType::INDIRECT_DISPLAY_DEVICE),
        copy(1, CARD, 0x2001, D3dkmtAdapterType::PARAVIRTUALIZED),
        software(2, 0x2002),
        physical(3, CARD, 0x2003, 1),
    ];
    let chosen = resolve_directml_adapter(&FakeAdapters(Ok(reordered)), &selector(CARD_IDENTITY))
        .decision
        .expect("the card behind its copies");
    assert_eq!(chosen.dxgi_index, 3);
    assert_eq!(chosen.luid.low_part, 0x2003);
}

#[test]
fn an_identity_on_two_physical_adapters_is_ambiguous() {
    // Two physical cards of one identity, both with a PCI location.
    let (_, error) = unavailable(
        vec![
            physical(0, CARD, 0x3000, 1),
            physical(1, CARD, 0x3001, 2),
            physical(2, INTEGRATED, 0x3002, 3),
        ],
        CARD_IDENTITY,
    );
    assert_eq!(error.reason, GpuUnavailableReason::AdapterAmbiguous);
    assert_warning(
        &error.outcome(GpuFallback::Cpu),
        GpuUnavailableReason::AdapterAmbiguous,
        GpuFallback::Cpu,
    );

    // Neither PCI location readable: still ambiguous, never the first one.
    let (_, error) = unavailable(
        vec![
            adapter(0, CARD, 0x3100, read(RENDER_AND_DISPLAY, None)),
            adapter(1, CARD, 0x3101, read(RENDER_AND_DISPLAY, None)),
        ],
        CARD_IDENTITY,
    );
    assert_eq!(error.reason, GpuUnavailableReason::AdapterAmbiguous);

    // The PCI location breaks the tie only when exactly one of them has one.
    let chosen = resolve_directml_adapter(
        &FakeAdapters(Ok(vec![
            adapter(0, CARD, 0x3200, read(RENDER_AND_DISPLAY, None)),
            physical(1, CARD, 0x3201, 4),
        ])),
        &selector(CARD_IDENTITY),
    )
    .decision
    .expect("the one physical card with a PCI location");
    assert_eq!(chosen.dxgi_index, 1);
}

#[test]
fn a_missing_identity_never_picks_another_adapter() {
    let absent = "dxgi-pci:aaaa:bbbb:cccccccc:dd";
    let (adapters, error) = unavailable(host_listing(0x4000), absent);
    assert_eq!(adapters.len(), 5, "the listing stays an observation");
    assert_eq!(error.reason, GpuUnavailableReason::AdapterMissing);
    assert!(error.cause.is_none());
    assert_warning(
        &error.outcome(GpuFallback::Refuse),
        GpuUnavailableReason::AdapterMissing,
        GpuFallback::Refuse,
    );

    // The identity is there only as copies: the physical adapters of other identities are
    // never chosen in its place.
    let (_, error) = unavailable(
        vec![
            physical(0, INTEGRATED, 0x4100, 2),
            copy(1, CARD, 0x4101, D3dkmtAdapterType::INDIRECT_DISPLAY_DEVICE),
            copy(2, CARD, 0x4102, D3dkmtAdapterType::PARAVIRTUALIZED),
            software(3, 0x4103),
        ],
        CARD_IDENTITY,
    );
    assert_eq!(error.reason, GpuUnavailableReason::AdapterNotPhysical);
    assert_warning(
        &error.outcome(GpuFallback::Cpu),
        GpuUnavailableReason::AdapterNotPhysical,
        GpuFallback::Cpu,
    );

    // A card whose kernel type cannot be read is not shown physical; the failure is its cause.
    let (_, error) = unavailable(
        vec![
            physical(0, INTEGRATED, 0x4200, 2),
            adapter(
                1,
                CARD,
                0x4201,
                KernelAdapterReading::Failed {
                    call: D3dkmtCall::OpenAdapterFromLuid,
                    ntstatus: 0xC000_000D_u32.cast_signed(),
                },
            ),
        ],
        CARD_IDENTITY,
    );
    assert_eq!(error.reason, GpuUnavailableReason::AdapterNotPhysical);
    assert_eq!(
        error.cause.as_ref().map(VisionFfiError::code),
        Some(VisionFfiErrorCode::ProviderUnavailable)
    );

    // A listing that cannot be read chooses nothing either, and keeps its failure.
    let failure = VisionFfiError::fatal_with_code(
        VisionFfiErrorCode::ProviderUnavailable,
        "directml_adapter",
        "fake listing failure",
    );
    let resolution = resolve_directml_adapter(
        &FakeAdapters(Err(failure.clone())),
        &selector(CARD_IDENTITY),
    );
    assert!(resolution.adapters.is_empty());
    let error = resolution.decision.expect_err("no listing, no adapter");
    assert_eq!(error.reason, GpuUnavailableReason::AdapterMissing);
    assert_eq!(error.cause, Some(failure));

    // A selector that is no identity matches nothing.
    let (_, error) = unavailable(host_listing(0x4300), "DXGI-PCI:1234:5678:9ABCDEF0:01");
    assert_eq!(error.reason, GpuUnavailableReason::AdapterMissing);
    assert_eq!(
        error.cause.as_ref().map(VisionFfiError::code),
        Some(VisionFfiErrorCode::InvalidRequest)
    );
}

#[test]
fn the_luid_is_observed_never_matched() {
    // Two boots: every LUID differs, and the card's LUID of the first boot now belongs to its
    // copy.
    let first = host_listing(0x5000);
    let mut second = host_listing(0x6000);
    second[2].luid = first[0].luid;
    let resolve = |listing: &[DxgiAdapter]| {
        resolve_directml_adapter(
            &FakeAdapters(Ok(listing.to_vec())),
            &selector(CARD_IDENTITY),
        )
        .decision
        .expect("the card at every boot")
    };
    let at_first = resolve(&first);
    let at_second = resolve(&second);
    assert_eq!(at_first.dxgi_index, 0);
    assert_eq!(at_second.dxgi_index, 0);
    assert_eq!(at_first.luid, first[0].luid, "observed at the first boot");
    assert_eq!(
        at_second.luid, second[0].luid,
        "observed at the second boot"
    );
    assert_ne!(at_first.luid, at_second.luid);
    assert_eq!(
        at_first.to_string(),
        format!("{CARD_IDENTITY};index=0;luid={}", first[0].luid)
    );
    assert_eq!(at_first.device(), at_second.device());

    // A session is bound to the identity and the index: the LUID changes no key and no digest.
    let key = |device| {
        OcrSessionKey::from_parts(OcrSessionKeyParts {
            engine_binding_sha256: "e".repeat(64),
            runtime_library_path: "onnxruntime.dll".to_string(),
            runtime_library_sha256: "d".repeat(64),
            onnxruntime_version: DIRECTML_RUNTIME_PIN.onnxruntime_version.to_string(),
            model_ref: "fixture-ocr-model".to_string(),
            model_sha256: ppocr_model_content_sha256(
                &"a".repeat(64),
                &"b".repeat(64),
                &"c".repeat(64),
                None,
            )
            .expect("model identity"),
            requested_backend: OnnxExecutionProvider::Directml,
            requested_cuda_device: None,
            resolved_cuda_device: None,
            requested_directml_device: Some(selector(CARD_IDENTITY)),
            resolved_directml_device: Some(device),
        })
        .expect("DirectML session key")
    };
    let first_key = key(at_first.device());
    let second_key = key(at_second.device());
    assert_eq!(first_key, second_key);
    assert_eq!(
        first_key.provider_options_sha256(),
        second_key.provider_options_sha256()
    );

    // A selector cannot name a LUID or an index.
    for named in [
        format!("{CARD_IDENTITY};luid={}", first[0].luid),
        format!("{CARD_IDENTITY};index=0"),
        first[0].luid.to_string(),
    ] {
        assert_eq!(
            selector(&named)
                .validate()
                .expect_err("no LUID or index")
                .code(),
            VisionFfiErrorCode::InvalidRequest
        );
    }
}

/// A vision root of its own below the system temporary directory, removed when dropped.
struct VisionRoot(PathBuf);

impl VisionRoot {
    fn new(label: &str, files: &[&str]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "actingcommand-gate-407-gpu1a-{}-{label}",
            std::process::id()
        ));
        // A directory left behind by an aborted earlier run of the same process id would add
        // its files to this root.
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("ort")).expect("ort directory");
        for file in files {
            fs::write(root.join("ort").join(file), b"placeholder library bytes")
                .expect("runtime library placeholder");
        }
        Self(root)
    }

    fn ort(&self, file: &str) -> PathBuf {
        self.0.join("ort").join(file)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for VisionRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The paths a process closure loads, in load order, through the closure loader with a
/// recording fake load.
fn load_order(closure: &[PathBuf]) -> Vec<PathBuf> {
    let state = Mutex::new(RuntimeLibraryClosureState::Uninitialized);
    let mut loaded = Vec::new();
    establish_runtime_library_closure_with(&state, closure, |path| {
        loaded.push(path.to_path_buf());
        Ok(())
    })
    .expect("the closure loads");
    loaded
}

#[test]
fn a_directml_closure_loads_directml_before_onnxruntime_and_a_cpu_closure_is_unchanged() {
    let root = VisionRoot::new(
        "closure",
        &[
            "onnxruntime_providers_shared.dll",
            "onnxruntime.dll",
            "DirectML.dll",
        ],
    );
    let shared = root.ort("onnxruntime_providers_shared.dll");
    let onnxruntime = root.ort("onnxruntime.dll");
    let directml = root.ort("DirectML.dll");

    // CPU: the closure of every earlier release, although DirectML.dll lies beside it.
    let cpu = VisionRuntimeSpec::from_vision_root(root.path(), OnnxExecutionProvider::Cpu, None)
        .expect("CPU runtime");
    assert_eq!(cpu.onnxruntime_library, onnxruntime);
    assert_eq!(
        cpu.runtime_library_closure,
        vec![shared.clone(), onnxruntime.clone()]
    );
    assert_eq!(cpu.expected_onnxruntime_sha256, None);
    assert_eq!(cpu.directml, None);
    assert_eq!(cpu.directml_runtime_library_closure(), None);
    assert_eq!(
        load_order(&cpu.runtime_library_closure),
        vec![shared.clone(), onnxruntime.clone()]
    );

    // DirectML: DirectML.dll by absolute path right before onnxruntime.dll; the closure of a
    // run without DirectML is the CPU closure.
    let dml = VisionRuntimeSpec::from_vision_root_directml(
        root.path(),
        selector(CARD_IDENTITY),
        GpuFallback::Cpu,
    )
    .expect("DirectML runtime");
    assert_eq!(dml.execution_provider, OnnxExecutionProvider::Directml);
    assert_eq!(dml.cuda_device, None);
    assert_eq!(dml.onnxruntime_library, onnxruntime);
    assert_eq!(dml.runtime_library_closure, cpu.runtime_library_closure);
    assert_eq!(
        dml.expected_onnxruntime_sha256.as_deref(),
        Some(DIRECTML_RUNTIME_PIN.onnxruntime.sha256)
    );
    let spec = dml.directml.as_ref().expect("the DirectML part");
    assert_eq!(spec.library, directml);
    assert!(spec.library.is_absolute());
    assert_eq!(spec.device, selector(CARD_IDENTITY));
    assert_eq!(spec.gpu_fallback, GpuFallback::Cpu);
    let closure = dml
        .directml_runtime_library_closure()
        .expect("the DirectML closure");
    assert_eq!(
        closure,
        vec![shared.clone(), directml.clone(), onnxruntime.clone()]
    );
    let loaded = load_order(&closure);
    assert_eq!(loaded, closure);
    let position = |path: &PathBuf| {
        loaded
            .iter()
            .position(|member| member == path)
            .expect("a closure member is loaded")
    };
    assert!(position(&directml) < position(&onnxruntime));

    // A DirectML runtime comes only from its own constructor, and needs the pinned build's
    // shared providers library.
    assert_eq!(
        VisionRuntimeSpec::from_vision_root(root.path(), OnnxExecutionProvider::Directml, None)
            .expect_err("directml through the CPU and CUDA constructor")
            .code(),
        VisionFfiErrorCode::InvalidRequest
    );
    let without_shared = VisionRoot::new("closure-no-shared", &["onnxruntime.dll"]);
    assert_eq!(
        VisionRuntimeSpec::from_vision_root_directml(
            without_shared.path(),
            selector(CARD_IDENTITY),
            GpuFallback::Cpu,
        )
        .expect_err("the shared providers library is required")
        .code(),
        VisionFfiErrorCode::ProviderUnavailable
    );
}

#[test]
fn the_runtime_pin_equals_the_tool_sources_entries() {
    let sources: Value = serde_json::from_str(TOOL_SOURCES).expect("tool sources parse");
    let components = &sources["components"];
    assert_eq!(
        components[ONNXRUNTIME_DIRECTML_TOOL_SOURCE]["version"],
        DIRECTML_RUNTIME_PIN.onnxruntime_version
    );
    assert_eq!(
        components[DIRECTML_TOOL_SOURCE]["version"],
        DIRECTML_RUNTIME_PIN.directml_version
    );
    assert_eq!(DIRECTML_RUNTIME_PIN.onnxruntime_version, "1.24.4");
    assert_eq!(DIRECTML_RUNTIME_PIN.directml_version, "1.15.4");

    let names = DIRECTML_RUNTIME_PIN
        .files()
        .map(|pinned| pinned.file_name.to_string());
    assert_eq!(
        names,
        [
            "onnxruntime_providers_shared.dll",
            "DirectML.dll",
            "onnxruntime.dll"
        ]
        .map(str::to_string)
    );

    for pinned in DIRECTML_RUNTIME_PIN.files() {
        let component = &components[pinned.tool_source];
        assert_eq!(component["kind"], "download_zip", "{}", pinned.tool_source);
        assert_eq!(
            component["license"]["redistribution"], "task_local_cache_only",
            "{}",
            pinned.tool_source
        );
        let entries = component["pinned_files"]
            .as_array()
            .expect("the component pins its files");
        let matching = entries
            .iter()
            .filter(|entry| entry["path"] == pinned.archive_path)
            .collect::<Vec<_>>();
        assert_eq!(matching.len(), 1, "{}", pinned.archive_path);
        let entry = matching[0];
        assert_eq!(
            entry["vision_root_path"],
            format!("ort/{}", pinned.file_name)
        );
        assert_eq!(
            entry["size"].as_u64(),
            Some(pinned.size),
            "{}",
            pinned.file_name
        );
        assert_eq!(entry["sha256"], pinned.sha256, "{}", pinned.file_name);
        for list in ["extract_allowlist", "required_files"] {
            assert!(
                component[list]
                    .as_array()
                    .is_some_and(|paths| paths.contains(&json!(pinned.archive_path))),
                "{list} of {} lacks {}",
                pinned.tool_source,
                pinned.archive_path
            );
        }
    }

    // Both directions: the two components pin no file the table does not hold.
    let pinned_in_sources = [ONNXRUNTIME_DIRECTML_TOOL_SOURCE, DIRECTML_TOOL_SOURCE]
        .iter()
        .map(|id| {
            components[*id]["pinned_files"]
                .as_array()
                .map_or(0, Vec::len)
        })
        .sum::<usize>();
    assert_eq!(pinned_in_sources, DIRECTML_RUNTIME_PIN.files().len());
}
