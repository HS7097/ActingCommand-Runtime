// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #407 GPU-1 D (0.12.44; register §1.286; design rd-1031/perf-407/design-gpu1.md §1.2,
// §1.4, §2.5, §6.1 and §7 row D; `actingd-config` revision 27): the `vision` section admits
// `execution_provider` `directml` with its adapter selector `directml_device` and the optional
// `gpu_fallback` (`cpu` by default, or `refuse`), and refuses either member with any other
// provider. A `directml` root needs ONNX Runtime's DirectML build as pinned: one off the pin fails
// check-config and the start with `vision_runtime_unavailable`. At startup the configured adapter
// is resolved once: a missing one records the start on the CPU and writes one
// `vision_gpu_unavailable` warning, and the start goes on. check-config lists every adapter, the
// one the identity resolves to and the pinned files, and notes the warning when DirectML cannot be
// used.
//
// The adapters come from a fake adapter source shaped like a machine whose one card is also listed
// as an indirect-display and a paravirtualized copy, beside another card and a software adapter;
// every identity is a fake. The runtime pin of the startup and listing tests is a fixture pin over
// small placeholder files, so no real runtime file is needed; the pin test uses the real pin.

use super::provider_startup::assemble_vision_provider;
use super::*;
use actingcommand_contract::outcome::Category;
use actingcommand_contract::{EventPayload, ProviderStartupObservation};
use actingcommand_ledger::{GlobalLedger, GlobalLedgerEvidenceConfig};
use actingcommand_runtime_host::RuntimeHost;
use actingcommand_vision_ffi::{
    D3dkmtAdapterType, DIRECTML_LIBRARY, DXGI_ADAPTER_FLAG_SOFTWARE, DxgiAdapter, DxgiLuid,
    KernelAdapterReading, ONNXRUNTIME_LIBRARY, ONNXRUNTIME_SHARED_PROVIDERS_LIBRARY, PciAddress,
    PinnedRuntimeFile, VisionFfiResult,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

// The configured card: listed physically at index 0, and again as two copies.
const IDENTITY: &str = "dxgi-pci:1234:5678:9abcdef0:01";
// Another physical card.
const OTHER_IDENTITY: &str = "dxgi-pci:4321:8765:0fedcba9:02";
// The software adapter.
const SOFTWARE_IDENTITY: &str = "dxgi-pci:0fed:0001:00000000:00";
// An identity no adapter of the listing has (the card's revision differs).
const ABSENT_IDENTITY: &str = "dxgi-pci:1234:5678:9abcdef0:02";

// The placeholder runtime files the fixture pin pins.
const ONNXRUNTIME_BYTES: &[u8] = b"fixture onnxruntime";
const SHARED_BYTES: &[u8] = b"fixture onnxruntime providers shared";
const DIRECTML_BYTES: &[u8] = b"fixture directml";

struct FakeAdapters(Vec<DxgiAdapter>);

impl AdapterSource for FakeAdapters {
    fn adapters(&self) -> VisionFfiResult<Vec<DxgiAdapter>> {
        Ok(self.0.clone())
    }
}

fn adapter(
    dxgi_index: u32,
    [vendor_id, device_id, subsys_id, revision]: [u32; 4],
    luid: u32,
    adapter_type: u32,
    pci_address: Option<PciAddress>,
    dxgi_flags: u32,
) -> DxgiAdapter {
    DxgiAdapter {
        dxgi_index,
        vendor_id,
        device_id,
        subsys_id,
        revision,
        luid: DxgiLuid {
            high_part: 0,
            low_part: luid,
        },
        dxgi_flags,
        kernel: KernelAdapterReading::Read {
            adapter_type: D3dkmtAdapterType(adapter_type),
            pci_address,
        },
    }
}

// Five adapters in DXGI order: the configured card (physical, index 0), another card, the
// configured card's indirect-display and paravirtualized copies, and a software adapter.
fn listing() -> Vec<DxgiAdapter> {
    let card = [0x1234, 0x5678, 0x9abc_def0, 0x01];
    let render = D3dkmtAdapterType::RENDER_SUPPORTED | D3dkmtAdapterType::DISPLAY_SUPPORTED;
    let pci = |bus| {
        Some(PciAddress {
            bus,
            device: 0,
            function: 0,
        })
    };
    vec![
        adapter(0, card, 0x0001_4ad3, render, pci(0x01), 0),
        adapter(
            1,
            [0x4321, 0x8765, 0x0fed_cba9, 0x02],
            0x0001_7dd6,
            render,
            pci(0x0a),
            0,
        ),
        adapter(
            2,
            card,
            0x0002_7657,
            D3dkmtAdapterType::RENDER_SUPPORTED | D3dkmtAdapterType::INDIRECT_DISPLAY_DEVICE,
            None,
            0,
        ),
        adapter(
            3,
            card,
            0x0002_1e23,
            D3dkmtAdapterType::RENDER_SUPPORTED | D3dkmtAdapterType::PARAVIRTUALIZED,
            None,
            0,
        ),
        adapter(
            4,
            [0x0fed, 0x0001, 0, 0],
            0x0001_7d7d,
            D3dkmtAdapterType::RENDER_SUPPORTED | D3dkmtAdapterType::SOFTWARE_DEVICE,
            None,
            DXGI_ADAPTER_FLAG_SOFTWARE,
        ),
    ]
}

// The lowercase SHA-256 of `bytes`, kept for the process lifetime as a pin's digest is.
fn digest(bytes: &[u8]) -> &'static str {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
        .leak()
}

// The runtime pin with the placeholder files' sizes and digests in place of the real ones.
fn fixture_pin() -> DirectmlRuntimePin {
    let pinned = |file: PinnedRuntimeFile, bytes: &[u8]| PinnedRuntimeFile {
        size: u64::try_from(bytes.len()).expect("a fixture size"),
        sha256: digest(bytes),
        ..file
    };
    DirectmlRuntimePin {
        onnxruntime: pinned(DIRECTML_RUNTIME_PIN.onnxruntime, ONNXRUNTIME_BYTES),
        providers_shared: pinned(DIRECTML_RUNTIME_PIN.providers_shared, SHARED_BYTES),
        directml: pinned(DIRECTML_RUNTIME_PIN.directml, DIRECTML_BYTES),
        ..DIRECTML_RUNTIME_PIN
    }
}

// A vision root under `parent` with one flat OCR model and the placeholder runtime files, with
// `directml` as `ort\DirectML.dll` (`None` leaves it out). No file is loaded.
fn vision_root(parent: &Path, directml: Option<&[u8]>) -> PathBuf {
    let root = parent.join("vision");
    let model = root.join("models").join("neutral.model");
    fs::create_dir_all(&model).expect("the model folder");
    fs::write(model.join("rec.onnx"), b"model").expect("the recognizer");
    fs::write(model.join("keys.txt"), b"a").expect("the dictionary");
    let ort = root.join("ort");
    fs::create_dir_all(&ort).expect("the runtime folder");
    fs::write(ort.join(ONNXRUNTIME_LIBRARY), ONNXRUNTIME_BYTES).expect("onnxruntime.dll");
    fs::write(ort.join(ONNXRUNTIME_SHARED_PROVIDERS_LIBRARY), SHARED_BYTES)
        .expect("onnxruntime_providers_shared.dll");
    if let Some(bytes) = directml {
        fs::write(ort.join(DIRECTML_LIBRARY), bytes).expect("DirectML.dll");
    }
    root
}

// An actingd configuration without instances whose `vision` section is `vision`.
fn configuration(state_root: &Path, vision: &Value) -> serde_json::Result<ActingdConfigFile> {
    serde_json::from_value::<ActingdConfigFile>(json!({
        "schema_version": CONFIG_SCHEMA_VERSION,
        "state_root": state_root,
        "bind_host": "127.0.0.1",
        "secret_fingerprint_salt": "0123456789abcdef",
        "vision": vision,
        "instances": []
    }))
}

// The assembled `vision` section of `vision`, or the code that refused it.
fn assembled(vision: &Value) -> Result<ConfiguredVision, Code> {
    let mut config = configuration(Path::new("state"), vision).expect("the file decodes");
    config.source_root = PathBuf::from("fixture-root");
    config
        .assemble()
        .map(|assembly| assembly.provider.vision.expect("the vision section"))
}

// The assembled `vision` section of a `directml` `section` over the vision root `root`, checked
// against the fixture pin and `adapters`.
fn directml_vision(section: &Value, root: &Path, adapters: Vec<DxgiAdapter>) -> ConfiguredVision {
    let mut section = section.clone();
    section["root"] = json!(root);
    let mut vision = assembled(&section).expect("the directml section assembles");
    vision
        .directml_mut()
        .expect("a directml section")
        .set_probe(fixture_pin(), Arc::new(FakeAdapters(adapters)));
    vision
}

fn device(identity: &str) -> Value {
    json!({ "expected_identity": identity })
}

#[test]
fn directml_config_requires_its_device_and_refuses_it_elsewhere() {
    let cuda_device = json!({ "ordinal": 0, "expected_stable_identity": "neutral-cuda-device" });
    // Admitted: `directml` with its device, and `cpu` and `cuda` without one.
    let vision = assembled(&json!({
        "execution_provider": "directml",
        "directml_device": device(IDENTITY),
    }))
    .unwrap_or_else(|code| panic!("directml with its device assembles: {code}"));
    assert_eq!(vision.execution_provider(), OnnxExecutionProvider::Directml);
    assert_eq!(
        vision
            .directml()
            .expect("the directml part")
            .device()
            .expected_identity,
        IDENTITY
    );
    for admitted in [
        json!({ "execution_provider": "cpu" }),
        json!({ "execution_provider": "cuda", "cuda_device": cuda_device }),
    ] {
        let vision = assembled(&admitted).unwrap_or_else(|code| panic!("{admitted}: {code}"));
        assert!(vision.directml().is_none(), "{admitted}");
    }
    // Refused at assembly with `vision_config_invalid`: `directml` without its device, with a
    // device that is no identity or with a CUDA selector, and the device beside `cpu` or `cuda`.
    for refused in [
        json!({ "execution_provider": "directml" }),
        json!({
            "execution_provider": "directml",
            "directml_device": device("dxgi-pci:1234:5678:9ABCDEF0:01"),
        }),
        json!({
            "execution_provider": "directml",
            "directml_device": device("dxgi-pci:1234:5678:9abcdef0"),
        }),
        json!({
            "execution_provider": "directml",
            "directml_device": device("dxgi-pci:1234:5678:9abcdef0:1"),
        }),
        json!({
            "execution_provider": "directml",
            "directml_device": device(IDENTITY),
            "cuda_device": cuda_device,
        }),
        json!({ "execution_provider": "cpu", "directml_device": device(IDENTITY) }),
        json!({
            "execution_provider": "cuda",
            "cuda_device": cuda_device,
            "directml_device": device(IDENTITY),
        }),
    ] {
        assert_eq!(
            assembled(&refused).err(),
            Some(ActingdCode::VisionConfigInvalid.code()),
            "{refused}"
        );
    }
    // The device names an adapter by its identity only: a DXGI index or a LUID is an unknown
    // member, and the file does not decode.
    for member in ["dxgi_index", "luid"] {
        let mut named = device(IDENTITY);
        named[member] = json!(0);
        let section = json!({ "execution_provider": "directml", "directml_device": named });
        assert!(
            configuration(Path::new("state"), &section).is_err(),
            "{section}"
        );
    }
}

#[test]
fn gpu_fallback_is_admitted_only_with_directml_and_defaults_to_cpu() {
    let directml = |fallback: Option<&str>| {
        let mut section = json!({
            "execution_provider": "directml",
            "directml_device": device(IDENTITY),
        });
        if let Some(fallback) = fallback {
            section["gpu_fallback"] = json!(fallback);
        }
        section
    };
    for (section, expected) in [
        (directml(None), GpuFallback::Cpu),
        (directml(Some("cpu")), GpuFallback::Cpu),
        (directml(Some("refuse")), GpuFallback::Refuse),
    ] {
        let vision = assembled(&section).unwrap_or_else(|code| panic!("{section}: {code}"));
        assert_eq!(
            vision.directml().expect("the directml part").gpu_fallback(),
            expected,
            "{section}"
        );
    }
    // Beside `cpu` or `cuda` the member is refused, whatever its value.
    for refused in [
        json!({ "execution_provider": "cpu", "gpu_fallback": "cpu" }),
        json!({ "execution_provider": "cpu", "gpu_fallback": "refuse" }),
        json!({
            "execution_provider": "cuda",
            "cuda_device": { "ordinal": 0, "expected_stable_identity": "neutral-cuda-device" },
            "gpu_fallback": "cpu",
        }),
    ] {
        assert_eq!(
            assembled(&refused).err(),
            Some(ActingdCode::VisionConfigInvalid.code()),
            "{refused}"
        );
    }
    // A value other than `cpu` and `refuse` does not decode.
    let section = directml(Some("gpu"));
    assert!(
        configuration(Path::new("state"), &section).is_err(),
        "{section}"
    );
}

// What one start recorded: its provider startup bindings `(field, configured, resolved)` in
// order, and what it wrote to actingd's stderr.
struct Started {
    bindings: Vec<(String, String, String)>,
    daemon_log: String,
}

impl Started {
    fn binding(&self, field: &str) -> (&str, &str) {
        let (_, configured, resolved) = self
            .bindings
            .iter()
            .find(|(name, ..)| name == field)
            .unwrap_or_else(|| panic!("no {field} binding in {:?}", self.bindings));
        (configured.as_str(), resolved.as_str())
    }

    // The file names of the recorded runtime closure, in load order.
    fn closure(&self) -> Vec<String> {
        self.bindings
            .iter()
            .filter(|(field, ..)| field.starts_with("runtime_library_paths["))
            .map(|(_, _, resolved)| {
                Path::new(resolved)
                    .file_name()
                    .expect("a library file")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }
}

// Starts a Runtime whose provider startup assembles the `directml` vision `section` over the
// fixture pin and `adapters`, then opens the ledger it left.
fn start(section: &Value, adapters: Vec<DxgiAdapter>) -> Started {
    let temp = TempDir::new().expect("tempdir");
    let state_root = temp.path().join("state");
    let root = vision_root(temp.path(), Some(DIRECTML_BYTES));
    let mut section = section.clone();
    section["root"] = json!(root);
    let mut config = configuration(&state_root, &section).expect("the file decodes");
    config.source_root = temp.path().to_path_buf();
    let assembly = config.assemble().expect("the configuration assembles");
    let mut vision = assembly.provider.vision.expect("the vision section");
    vision
        .directml_mut()
        .expect("a directml section")
        .set_probe(fixture_pin(), Arc::new(FakeAdapters(adapters)));
    let mut daemon_log = Vec::new();
    let host = RuntimeHost::start_with_provider(assembly.host, |startup| {
        let provider = assemble_vision_provider(startup, &vision, None, &mut daemon_log)?;
        let registry = ExecutionBackendRegistry::from_assembly(ProviderAssembly {
            instances: Vec::new(),
            vision: Some(VisionSpec::new(provider)),
            discovery: None,
        })
        .expect("a registry with the vision provider");
        Ok(Arc::new(registry) as Arc<dyn ExecutionBackendProvider>)
    })
    .unwrap_or_else(|error| panic!("the start goes on: {error}"));
    host.close().expect("close the host");
    let events =
        GlobalLedger::open_evidence(GlobalLedgerEvidenceConfig::new(&state_root), |_| None)
            .expect("the startup ledger is readable")
            .events()
            .to_vec();
    let bindings = events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Provider(payload) => match &payload.record.observation {
                ProviderStartupObservation::Binding {
                    field,
                    configured,
                    resolved,
                    ..
                } => Some((field.clone(), configured.clone(), resolved.clone())),
                _ => None,
            },
            _ => None,
        })
        .collect();
    Started {
        bindings,
        daemon_log: String::from_utf8(daemon_log).expect("daemon lines are text"),
    }
}

#[test]
fn a_missing_adapter_at_startup_records_cpu_and_one_warning() {
    // No adapter has the configured identity: the start goes on, records that it starts on the
    // CPU with the CPU closure, and writes one warning, as a WARN and an OUTCOME line.
    let started = start(
        &json!({ "execution_provider": "directml", "directml_device": device(ABSENT_IDENTITY) }),
        listing(),
    );
    assert_eq!(
        started.binding("execution_provider"),
        ("directml", "cpu"),
        "{:?}",
        started.bindings
    );
    assert_eq!(
        started.binding("directml_adapter"),
        (ABSENT_IDENTITY, "none")
    );
    assert_eq!(
        started.closure(),
        [ONNXRUNTIME_SHARED_PROVIDERS_LIBRARY, ONNXRUNTIME_LIBRARY],
        "the CPU closure loads no DirectML.dll"
    );
    let lines = started.daemon_log.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2, "one warning: {}", started.daemon_log);
    assert!(
        lines[0].starts_with(&format!(
            "WARN actingd: {}",
            VisionGpuCode::VisionGpuUnavailable.as_str()
        )),
        "{}",
        lines[0]
    );
    let envelope: Value = serde_json::from_str(
        lines[1]
            .strip_prefix("OUTCOME actingd: ")
            .expect("the OUTCOME line"),
    )
    .expect("the envelope");
    assert_eq!(
        envelope["code"],
        VisionGpuCode::VisionGpuUnavailable.as_str(),
        "{envelope}"
    );
    assert_eq!(envelope["category"], "warning", "{envelope}");
    assert_eq!(
        envelope["values"],
        json!({
            "execution_provider": "directml",
            "gpu_unavailable_reason": "adapter_missing",
            "gpu_fallback": "cpu",
            "stage": crate::codes::ActingdLocation::VisionStartup.as_str(),
        }),
        "{envelope}"
    );

    // The configured card present: the start records DirectML on index 0 with the LUID it
    // observed, the DirectML closure, and writes nothing.
    let started = start(
        &json!({ "execution_provider": "directml", "directml_device": device(IDENTITY) }),
        listing(),
    );
    assert_eq!(
        started.binding("execution_provider"),
        ("directml", "directml")
    );
    assert_eq!(
        started.binding("directml_adapter"),
        (
            IDENTITY,
            "dxgi-pci:1234:5678:9abcdef0:01;index=0;luid=0x0000000000014ad3"
        )
    );
    assert_eq!(
        started.closure(),
        [
            ONNXRUNTIME_SHARED_PROVIDERS_LIBRARY,
            DIRECTML_LIBRARY,
            ONNXRUNTIME_LIBRARY
        ]
    );
    assert!(started.daemon_log.is_empty(), "{}", started.daemon_log);
}

fn arguments(values: &[&str]) -> Vec<std::ffi::OsString> {
    values.iter().map(std::ffi::OsString::from).collect()
}

#[test]
fn a_runtime_off_the_pin_fails_check_config_with_vision_runtime_unavailable() {
    // The placeholder files are not the pinned ONNX Runtime 1.24.4 DirectML build.
    let temp = TempDir::new().expect("tempdir");
    let root = vision_root(temp.path(), Some(DIRECTML_BYTES));
    let canonical = fs::canonicalize(&root).expect("the canonical vision root");
    let write = |name: &str, vision: Value| {
        let path = temp.path().join(name);
        let file = json!({
            "schema_version": CONFIG_SCHEMA_VERSION,
            "state_root": temp.path().join("state"),
            "bind_host": "127.0.0.1",
            "secret_fingerprint_salt": "0123456789abcdef",
            "vision": vision,
            "instances": []
        });
        fs::write(&path, serde_json::to_vec(&file).expect("the JSON")).expect("the file");
        path
    };
    let directml = json!({
        "execution_provider": "directml",
        "directml_device": device(IDENTITY),
        "root": root,
    });
    let config = write("directml.json", directml.clone());
    let refused = crate::check_config::run(arguments(&[
        "check-config",
        "--config",
        config.to_str().expect("a UTF-8 path"),
    ]))
    .expect_err("a runtime off the pin is refused");
    let outcome = refused.outcome();
    assert!(
        outcome.is(ActingdCode::VisionRuntimeUnavailable),
        "{outcome}"
    );
    assert_eq!(outcome.category(), Category::Error, "{outcome}");
    assert_eq!(
        outcome.value("stage"),
        Some(&json!(crate::codes::ActingdLocation::ConfigVision.as_str())),
        "{outcome}"
    );
    assert_eq!(
        outcome.value("path"),
        Some(&json!(
            canonical
                .join("ort")
                .join(ONNXRUNTIME_LIBRARY)
                .to_string_lossy()
        )),
        "{outcome}"
    );

    // A start from the same section is refused with the same code before the Runtime is ready.
    let mut file =
        configuration(&temp.path().join("state-start"), &directml).expect("the file decodes");
    file.source_root = temp.path().to_path_buf();
    let assembly = file.assemble().expect("the configuration assembles");
    let started = RuntimeHost::start_with_provider(assembly.host, |startup| {
        assembly.provider.assemble_provider(startup)
    });
    match started {
        Err(error) => assert_eq!(error.code(), ActingdCode::VisionRuntimeUnavailable.as_str()),
        Ok(host) => {
            host.close().expect("close the unexpected host");
            panic!("a runtime off the pin must refuse the start");
        }
    }

    // The pin is DirectML's only: the same root under `cpu` passes.
    let config = write(
        "cpu.json",
        json!({ "execution_provider": "cpu", "root": root }),
    );
    let reported = crate::check_config::run(arguments(&[
        "check-config",
        "--config",
        config.to_str().expect("a UTF-8 path"),
    ]))
    .unwrap_or_else(|error| panic!("cpu passes: {}", error.outcome()));
    let report = reported.data.get("report").expect("the report");
    assert_eq!(report["vision"]["execution_provider"], "cpu", "{report}");
    assert!(report["vision"].get("directml").is_none(), "{report}");
}

#[test]
fn check_config_lists_every_adapter_and_the_one_it_resolves() {
    let temp = TempDir::new().expect("tempdir");
    let adapters = json!([
        {
            "dxgi_index": 0, "identity": IDENTITY, "luid": "0x0000000000014ad3",
            "physical": true, "software": false, "indirect_display": false,
            "paravirtualized": false, "pci_address": "01:00.0"
        },
        {
            "dxgi_index": 1, "identity": OTHER_IDENTITY, "luid": "0x0000000000017dd6",
            "physical": true, "software": false, "indirect_display": false,
            "paravirtualized": false, "pci_address": "0a:00.0"
        },
        {
            "dxgi_index": 2, "identity": IDENTITY, "luid": "0x0000000000027657",
            "physical": false, "software": false, "indirect_display": true,
            "paravirtualized": false, "pci_address": null
        },
        {
            "dxgi_index": 3, "identity": IDENTITY, "luid": "0x0000000000021e23",
            "physical": false, "software": false, "indirect_display": false,
            "paravirtualized": true, "pci_address": null
        },
        {
            "dxgi_index": 4, "identity": SOFTWARE_IDENTITY, "luid": "0x0000000000017d7d",
            "physical": false, "software": true, "indirect_display": false,
            "paravirtualized": false, "pci_address": null
        }
    ]);
    let checked = |name: &str, section: Value, directml: Option<&[u8]>| {
        let parent = temp.path().join(name);
        let root = vision_root(&parent, directml);
        let vision = directml_vision(&section, &root, listing());
        crate::check_config::check_vision(&vision)
            .unwrap_or_else(|error| panic!("{name} passes: {}", error.outcome()))
    };

    // The configured card resolves to index 0 among its copies; nothing is noted.
    let (report, notes) = checked(
        "resolved",
        json!({ "execution_provider": "directml", "directml_device": device(IDENTITY) }),
        Some(DIRECTML_BYTES),
    );
    assert_eq!(report["execution_provider"], "directml", "{report}");
    assert_eq!(
        report["directml"],
        json!({
            "expected_identity": IDENTITY,
            "gpu_fallback": "cpu",
            "adapters": adapters,
            "resolved": { "dxgi_index": 0, "luid": "0x0000000000014ad3" },
            "unavailable_reason": null,
            "runtime_pin": { "onnxruntime": "ok", "providers_shared": "ok", "directml": "ok" },
            "nn_execution_provider": "cpu",
        }),
        "{report}"
    );
    assert!(notes.is_empty(), "{notes:?}");

    // An identity no adapter has: every adapter is still listed, none resolves, and the check
    // passes with one warning note.
    let (report, notes) = checked(
        "missing",
        json!({ "execution_provider": "directml", "directml_device": device(ABSENT_IDENTITY) }),
        Some(DIRECTML_BYTES),
    );
    assert_eq!(report["directml"]["adapters"], adapters, "{report}");
    assert_eq!(report["directml"]["resolved"], Value::Null, "{report}");
    assert_eq!(
        report["directml"]["unavailable_reason"], "adapter_missing",
        "{report}"
    );
    let stage = crate::codes::ActingdLocation::ConfigVision.as_str();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(
        notes[0].is(VisionGpuCode::VisionGpuUnavailable),
        "{}",
        notes[0]
    );
    assert_eq!(notes[0].category(), Category::Warning);
    assert_eq!(
        Value::Object(notes[0].values().clone()),
        json!({
            "execution_provider": "directml",
            "gpu_unavailable_reason": "adapter_missing",
            "gpu_fallback": "cpu",
            "stage": stage,
        })
    );

    // `ort\DirectML.dll` off the pin under `refuse`: the card still resolves, the pinned file is
    // reported, and the note names the library and the fallback.
    let (report, notes) = checked(
        "library",
        json!({
            "execution_provider": "directml",
            "directml_device": device(IDENTITY),
            "gpu_fallback": "refuse",
        }),
        Some(b"another directml".as_slice()),
    );
    assert_eq!(report["directml"]["gpu_fallback"], "refuse", "{report}");
    assert_eq!(
        report["directml"]["resolved"],
        json!({ "dxgi_index": 0, "luid": "0x0000000000014ad3" }),
        "{report}"
    );
    assert_eq!(
        report["directml"]["runtime_pin"]["directml"], "sha256_mismatch",
        "{report}"
    );
    assert_eq!(
        report["directml"]["unavailable_reason"], "directml_library_mismatch",
        "{report}"
    );
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(
        Value::Object(notes[0].values().clone()),
        json!({
            "execution_provider": "directml",
            "gpu_unavailable_reason": "directml_library_mismatch",
            "gpu_fallback": "refuse",
            "stage": stage,
        })
    );

    // No `ort\DirectML.dll` at all.
    let (report, notes) = checked(
        "no-library",
        json!({ "execution_provider": "directml", "directml_device": device(IDENTITY) }),
        None,
    );
    assert_eq!(
        report["directml"]["runtime_pin"]["directml"], "missing",
        "{report}"
    );
    assert_eq!(
        report["directml"]["unavailable_reason"], "directml_library_missing",
        "{report}"
    );
    assert_eq!(notes.len(), 1, "{notes:?}");
}
