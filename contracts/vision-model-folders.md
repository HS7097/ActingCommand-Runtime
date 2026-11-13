# Vision model folders

Workflow #360. actingd runs its OCR and NN models in-process. The models of a
daemon live in one vision root; a recognition pack names a model by
`model_ref` and `model_sha256`, and the daemon finds it by folder name.

## Vision root

```text
<vision root>\
  ort\onnxruntime.dll                    required when `vision` is configured
  ort\onnxruntime_providers_shared.dll   optional; loaded first when present
                                         (required for execution_provider directml)
  ort\onnxruntime_providers_cuda.dll     required for execution_provider cuda
  ort\DirectML.dll                       execution_provider directml only; see "DirectML runtime"
  ort\licenses\onnxruntime\, ort\licenses\directml\
                                         the DirectML runtime's license files; not read
  models\<name>\                         one folder per model; <name> is its model_ref
```

The `vision` section of `actingd.config.json` selects it (see "Vision root" in
`contracts/actingd-check-config.md`): `root` when given, else
`<install root>\vision` when the daemon runs from an install root, else
`vision` next to the configuration file. A root inside the install root's
`A\` or `B\` is refused. Plain files directly under `models\` (the single-folder
file layout of earlier releases, manifests) are not models and are ignored.

Who places a vision root: for the CPU execution provider, the script
`scripts/windows-tools/Materialize-VisionRoot.ps1` (Workflow #428 CI-OCR-1)
fills or verifies one from the tool registry
`scripts/windows-tools/windows-tool-sources.v1.json`, every address, size and
SHA-256 read from it: `ort\onnxruntime.dll` and
`ort\onnxruntime_providers_shared.dll` (the `pinned_files` of
`onnxruntime-directml-1.24.4`, the pin of "DirectML runtime" below), that
package's license files in `ort\licenses\onnxruntime\` (the same entry's
`pinned_license_files`), and the OCR folder `models\PP-OCRv6_medium\` in the
flat layout (`det.onnx`, `rec.onnx`, `keys.txt` of `ppocrv6-medium-onnx`).
Every file it places has a pin: a file with its pinned size and SHA-256 is
kept, and the package is fetched only for a file of it that is not, so a
complete vision root is verified without a fetch. It is the one implementation
a resource repository's CI calls, at its Tools pin's commit; it writes no
configuration and loads no model. It places no `DirectML.dll`. The Windows
installation package, assembled and published outside this repository, carries
these files and `DirectML.dll`, each with its license files, by the owner's
decision (Workflow #441, 2026-11-13; the registry's `redistribution_note`s); the
Runtime package and the Tools package carry none of them.

## The manifest

Workflow #453 S7 (since 0.12.60; register §1.350). What a vision root must hold
is data of the build, not of a script, and two read-only actingd subcommands
answer it; nothing in them downloads, and no answer names an address.

**Where the data lives.** The tool registry
`scripts/windows-tools/windows-tool-sources.v1.json`, which actingd embeds (the
text its install adb's pin is read from), and the DirectML runtime pin
(`crates/vision-ffi/src/runtime_pin.rs`, "DirectML runtime" below). A component
that names a model carries its `model_ref` and `family` (`ppocrv6-medium-onnx`:
`PP-OCRv6_medium`, `ppocr-ctc`), and each of its artifacts its place
`vision_root_path` (`models/PP-OCRv6_medium/det.onnx`, `rec.onnx`, `keys.txt`).
A runtime component's license files are its `pinned_license_files`, each with
`vision_root_path`, size and SHA-256: `onnxruntime-directml-1.24.4`'s two since
Workflow #428 CI-OCR-1, and `directml-1.15.4`'s three (`LICENSE.txt`,
`LICENSE-CODE.txt`, `ThirdPartyNotices.txt` at `ort/licenses/directml/`) since
this slice, read from the pinned package itself. Every other member of the
registry is ignored by the manifest, and every member the manifest reads is
ignored by the install adb's reader. `Materialize-VisionRoot.ps1` keeps its own
constants for the places it fills.

**What it lists.** `actingd vision manifest --provider cpu|directml` answers
`{schema_version: "actingcommand.actingd.vision.v1", outcome:
vision_manifest_reported, report}`, `report` `{provider, files, models}`:

- `files`, in byte order of `vision_root_path`, each `{vision_root_path, size,
  sha256, component, archive_path, license, pinned}`: the runtime's libraries,
  the pin's own (`ort/onnxruntime.dll` and `ort/onnxruntime_providers_shared.dll`
  for both providers, `ort/DirectML.dll` for `directml` only), with the pin's
  component and its entry in that component's archive; the license files of each
  component a library comes from (`license` true); and the files of every model
  component (`archive_path` `null`: the component pins the file itself). Every
  file of this build's manifest has a size and a SHA-256 (`pinned` true).
- `models`, each `{model_ref, family, files, model_sha256}`: the model's places
  and its identity ("Identity" below), computed from its files' pins as the
  engine computes it from their bytes.

`cuda` has no manifest. A registry the build cannot read so is
`serialization_failed` with the reason as its `foreign_data_error` link, a
defect of the build.

**What `check` compares.** `actingd vision check --config <file>
[--install-root <root>] [--model <model_ref>]...` resolves the configured vision
root as a start does ("Vision root" above: the configured or default root,
canonical; a root that does not resolve is `vision_root_unavailable`, one inside
a program slot `vision_root_in_program_slot`, at stage `config_vision`, the
start's own refusals), takes the manifest of the section's `execution_provider`,
and answers `vision_check_completed` with `report` `{provider, vision_root,
files, folders, models}`. Each `--model` (Workflow #453 S7 fix 1) names a model
folder whose identity it computes besides the manifest's: the rescue row `vision
check` names the models the installed packs need. Fix 2: each `--model` must be
a model folder's name by the loader's own rule ("Invalid folders" below: not
blank, at most 255 bytes, not `.` or `..`, no path separator, colon or control
character; `invalid_model_folder_name`, the function listing applies), and the
check takes at most 64 of them (each model the packs of a root need, once: far
more than a root's packs name today, and 64 of the longest names fit about half
of a Windows command line). A name that breaks the rule, or a 65th `--model`, is
`invalid_arguments` before the configuration or the vision root is read.

- `files`: each manifest file with its `state`, compared first by its size and
  then by its SHA-256 read in 64 KiB chunks: `ok`, `missing`, `sha256_mismatch`
  (another size, another digest, or no regular file) or `unreadable` (with the OS
  failure as `cause`), the states of the DirectML runtime check. A file of
  another size than its pin is `sha256_mismatch` and is not opened, and no file
  is read past its pinned size: one that shrinks or grows while it is read is
  `sha256_mismatch`;
- `folders`: `models\` listed as a start lists it ("Invalid folders" below; none
  when it does not exist), each model folder `{model_ref, family, layout?,
  asked, identity?}`. `asked` is true for a folder the manifest or a `--model`
  names, and only such a folder's model files are hashed: its `identity` is
  computed from their bytes (`null`, with the OS failure as `cause` when a file
  cannot be read, or with `over_bound` `{path, size, bound, bound_bytes}` for a
  file over a bound, which is not read). Every other model folder is `asked`
  false, listed by name with no `identity`, and none of its model files is read.
  Listing itself reads, of every folder asked or not, what a start's listing
  reads (fix 2): its entries' metadata and its `model.json` when it holds one (at
  most 64 KiB; a larger one makes the folder invalid, unread). Each folder that
  breaks the rule is `{model_ref, invalid}` with the reason. Each name the
  manifest or a `--model` asks about that no folder of `models\` has (fix 2) is
  listed too, `{model_ref, asked: true, absent: true}`, in name order with the
  others, also when `models\` does not exist;
- `models`: each manifest model with `present` (a folder of its name is listed;
  an `absent` entry is none), that folder's `identity` and `identity_matches`.

**Bounds** (fix 1; of what the check reads, not tunables). A file of an asked
folder is hashed only when it holds at most 256 MiB (else `bound` `file`,
`bound_bytes` 268435456), and the check hashes at most 512 MiB in all, the
manifest's files included (a folder's file larger than what is left is `bound`
`total`, `bound_bytes` 536870912). The manifest's files are read at most to their
pins, about 175 MB for `directml` and 156 MB for `cpu` with this build's
registry; a build whose manifest's files hold more than 512 MiB is refused as a
defect of the build, `serialization_failed`. Each file is read at most once, and
at most to the length it had when it was opened. **What is counted** (fix 2): the
512 MiB counts the bytes hashed, the manifest's files and the asked folders'
model files, and nothing else. Listing `models\` is not counted: it reads the
folders' entries and their metadata and each folder's `model.json` (at most
64 KiB each, as a start reads it), of every folder, asked or not, however many
there are. The rescue row's bound for this child, 160 s, is 512 MiB at 4 MiB/s
and 32 s for the process and its listing ([install-cli.md](install-cli.md), "The
Lab's rescue module").

Whether the root is complete is the report's to say; the code is a success
either way. The configuration file is loaded as `check-config` loads it (a file
it cannot load is refused at stage `config_load`), and of it only the `vision`
section is assembled, by the rules a start assembles it with (a section that
breaks them is `vision_config_invalid` at stage `config_assemble`); a
configuration without a `vision` section, or of `cuda`, is `vision_config_invalid`
at stage `config_vision`. Nothing else of the configuration is assembled or
judged, so a configuration a start refuses for another part of it can still be
checked: the check answers for the vision root, not for the configuration.

**Where it differs from a start.** Two differences, on purpose, so that nobody
reads "check failed" as "the Runtime would not start":

- The `cpu` manifest lists the runtime library and the providers library of the
  pinned build (`ort/onnxruntime.dll`, `ort/onnxruntime_providers_shared.dll`),
  and `vision check` holds a CPU root to them, although a start with the CPU
  provider does not check those two files against the pin (only a start with the
  DirectML provider does, `check_runtime_pin` in `apps/actingd/src/config.rs`):
  the check is stricter than a start on purpose, because an installation set
  carries exactly the pinned files. The license files it holds a root to are read
  by no start either.
- A missing or empty `models\` answers a `folders` list with no folder of it
  (only the names asked about, each `absent`, fix 2) in the check, and refuses a
  start (`vision_root_unavailable` for a missing one, `vision_models_empty` for an
  empty one).

**What it never does.** It writes nothing (not the vision root, not a state
root, not a journal), takes no owner lock, opens no ledger, binds no socket,
starts no program, loads no model and no library, and fetches nothing; it
neither places nor removes a file. The Lab's rescue rows `vision manifest` and
`vision check` run these subcommands with the installed build ([install-cli.md](install-cli.md),
"The Lab's rescue module"); `vision check` there also asks the Tools'
`standard-pack check` of each game root which models the installed packs need
(its member `models`, [package-reference.md](package-reference.md)).

## Folder layouts

A model folder holds exactly one of these layouts:

| Role | Flat | Nested |
|---|---|---|
| OCR detector (optional) | `det.onnx` | `det\inference.onnx` |
| OCR recognizer | `rec.onnx` | `rec\inference.onnx` |
| OCR dictionary | `keys.txt` | `rec\keys.txt` |
| NN classifier | `model.onnx` | — |
| Description (optional) | `model.json` | `model.json` |

Other files in a folder are ignored. An OCR model without a detector reads only
regions; a `full_frame` region on it fails with the reason. An angle classifier
(`cls.onnx` or `cls\`) is not supported.

## Description `model.json`

UTF-8 JSON of at most 64 KiB, unknown fields refused:

```json
{"schema_version":"actingcommand.vision_model.v1","family":"ppocr-ctc","languages":["en"]}
```

- `schema_version`: `actingcommand.vision_model.v1`.
- `family`: `ppocr-ctc` for an OCR folder, `onnx-classify` for an NN folder;
  it must match the layout.
- `languages`: optional, informational; recorded at startup and never compared
  with a target's languages.
- `recognizer`, `detector`, `decoder`: optional `ppocr-ctc` parameter sections
  (see "Parameters"); an `onnx-classify` description has none.

A folder without `model.json` has the family's defaults. The description digest
is the SHA-256 of the canonical description with every default filled in, so a
missing file and a file that spells out the defaults have the same digest. The
description is read once at startup; a change needs a restart.

## Parameters

Every field of a `ppocr-ctc` section is optional; its default is the engine's
built-in behaviour, so a model without `model.json` reads exactly as before.

| Field | Default | Allowed | Meaning |
|---|---|---|---|
| `recognizer.input_height` | 48 | 8..=256 | tensor height; a static model height must equal it |
| `recognizer.min_width` | 32 | 8..=`max_width` | narrowest tensor width; narrower content is zero-padded |
| `recognizer.max_width` | 320 | ..=4096 | widest tensor width; wider content is squeezed; a dynamic width above 320 is `floor(height * w / h)`, as the official pipeline sizes wide crops (up to 320 the content keeps its `ceil` width) |
| `recognizer.channel_order` | `bgr` | `bgr`, `rgb` | plane order of the recognizer input |
| `detector.max_side` | 960 | >= `multiple` | longest side of a dynamic detector input |
| `detector.min_side` | 32 | >= 1 | shortest side |
| `detector.multiple` | 32 | 1..=256 | both sides round up to it |
| `detector.channel_order` | `rgb` | `rgb`, `bgr` | plane order; mean and deviation follow the plane |
| `detector.resize` | `nearest` | `nearest`, `linear` | resampling of the detector input |
| `detector.threshold` | 0.3 | (0, 1) | text probability of a map cell |
| `detector.min_area` | 4 | >= 1 | fewest cells of a text component |
| `detector.box_padding` | 16 | 0..=256 | frame pixels added around a box |
| `detector.max_boxes` | 64 | 1..=1024 | most boxes recognized per request |
| `detector.merge_min_overlap_percent` | 35 | 0..=100 | vertical overlap for merging boxes on a line |
| `detector.merge_gap_factor` | 3 | 0..=16 | horizontal gap allowed, in box heights |
| `detector.merge_min_height` | 24 | 0..=1024 | box height counted at least this |
| `decoder.space_class` | `drop` | `drop`, `space` | the class after the dictionary decodes to a space |

A value out of range, an unknown field or a section on an `onnx-classify`
description makes the folder invalid. The detector parameters apply to a request that
reads with the detector: a `full_frame` region of an OCR target without `locate`, and every
`blocks: "detect"` request of a locate target, on any region (Workflow #396 REC-1,
`ocr-fields.md`, section Located text). Box padding is clipped to the requested region. A
`detect` request's `line_merge` replaces `detector.merge_gap_factor` (`{"gap_factor": n}`)
or merges no boxes (`"none"`); every other detector parameter is the folder's. A `detect`
read reports how many boxes the detector found before `max_boxes` kept the first of them.

## Identity

- OCR: `actingcommand.ppocr-model-set.v1` over the SHA-256 of the detector
  (`none` when absent), recognizer and dictionary files, with the classifier
  `none`. It does not depend on the layout or the file names.
- NN: the SHA-256 of `model.onnx`.
- The description is not part of the identity. Its canonical form (every
  section and field filled in) enters the engine binding digest each OCR
  execution records as `provider_binary_sha256`, together with the SHA-256 of
  the running executable and of `onnxruntime.dll`, so two runs with different
  parameters show different `provider_binary_sha256` with the same
  `model_sha256`.

## Invalid folders

A folder breaks the rule when its name is not a valid `model_ref` (empty, over
255 bytes, a path separator, a colon or a control character, or not UTF-8), it
or a model file is a symbolic link or junction, it mixes layouts, a layout is
incomplete, it holds an angle classifier, or its `model.json` is unreadable,
invalid or names another family. Startup records such a folder as
`vision_model_folder_invalid` with its `path` and, when the folder name fits the
name rule, its `model_name` (not its reason) and keeps running; every request
naming it fails.
`actingd check-config` fails with `vision_model_folder_invalid`. A missing
`models\` folder, a missing `ort\onnxruntime.dll` and an empty `models\` fail
startup.

## Startup and first use

Startup reads directory entries, file metadata and descriptions only, and
records, under the `provider.startup_observed` path binding stage, the
`vision_root`, the `onnxruntime_library`, each runtime library, one
`ocr_model:<name>` or `nn_model:<name>` binding per folder (layout, family,
detector presence, description digest, languages) and one
`invalid_model:<name>` binding per invalid folder with its reason.

On a model's first use the engine reads its files once, computes the identity
from those bytes and builds the sessions from the same bytes only when the
identity equals the target's `model_sha256`; a different identity fails with
`ModelMismatch` and builds nothing. The first build initialises ONNX Runtime
once per process. At most four models are loaded at once; the least recently
used idle model is unloaded to make room and is read and hashed again on its
next use. A model whose files changed while the daemon runs is retired until
restart. Waiting for a model held by another request counts against the
target's `timeout_ms`; the model's own load does not. A target's `timeout_ms`
bounds inference and not the one-time loading of a model: a session the engine
builds on the first request that needs it (the detector, on a model's first
request that detects) is built before that request's deadline starts, and no
load is bounded by a target's `timeout_ms` (Workflow #428 VIS-LOAD-1, register
§1.298). An offline evaluation of a saved scene in the Lab may, on its switch
`--ocr-deadline-max`, take each OCR target's `timeout_ms` as 60 000, the most
it may declare, for that evaluation only; production never does (Workflow #428
LAB-OCR-DL, register §1.331).

An OCR recognizer's output class count must be the dictionary size plus one
(the CTC blank) or plus two (the blank and the space class); any other count
fails the load naming both counts.

## DirectML runtime

Workflow #407 GPU-1 (design `rd-1031/perf-407/design-gpu1.md`, register §1.286).
Slice A brings the parts below into the engine's boundary crate; the
configuration that selects `directml` comes with slice D and the engine's
DirectML sessions with slices B1 and B2. Since slice D the `vision` section
admits `execution_provider` `directml` with `directml_device
{ expected_identity }` and `gpu_fallback` (`actingd-config` revision 27;
[actingd-check-config.md](actingd-check-config.md), "Vision root"); check-config
and a start hash the pinned files below, list the adapters and resolve the
identity as "Adapter identity and selection" says. The engine builds DirectML
sessions (slice B1) and falls back to the CPU (slice B2, "Fallback to the CPU");
the kernel refuses the execution attestation of a `directml` section, a model
on the CPU after a fallback included, until slice C admits its evidence. A
`cpu` section runs as before. The CUDA path of this build is known not to work
(GPU-0d, GPU-0e).

### Pinned files

A DirectML runtime is ONNX Runtime 1.24.4's DirectML build with DirectML
1.15.4, pinned in `crates/vision-ffi/src/runtime_pin.rs`:

| File under `ort\` | Size (B) | SHA-256 | Source |
|---|---|---|---|
| `onnxruntime.dll` | 17,328,152 | `e7eedec6a6f26dc39dc948276a75ef6d2bee3fff944d874ceed0bbd3b97bff40` | NuGet `Microsoft.ML.OnnxRuntime.DirectML` 1.24.4, `runtimes/win-x64/native/` |
| `onnxruntime_providers_shared.dll` | 22,040 | `265c8daf29637cb259cac8be9f08f2cd45f3883f0f0e4949cbfddd5b4cbec3b6` | the same package |
| `DirectML.dll` | 18,527,776 | `9c9e6d822561c6c41b90e6994b3e8857cf1d66dbfb1e0c4c799c7c89b4e92da1` | NuGet `Microsoft.AI.DirectML` 1.15.4, `bin/x64-win/` |

The same sizes and digests are the `pinned_files` of the components
`onnxruntime-directml-1.24.4` and `directml-1.15.4` in
`scripts/windows-tools/windows-tool-sources.v1.json`, which reproduce the files
from the official NuGet URLs; a logic test holds the two equal. The pin is no
setting. The Runtime package and the Tools package do not carry these files; the
Windows installation package carries them, with each package's license files, by
the owner's decision (Workflow #441, 2026-11-13; "Vision root" above). Whoever
places them copies them, and each package's license files to
`ort\licenses\onnxruntime\` and `ort\licenses\directml\`. For a CPU vision
root, `Materialize-VisionRoot.ps1` places the two ONNX Runtime files and their
license files this way, the license files by their own pins
(`pinned_license_files`; "Vision root" above). The ONNX Runtime
of a DirectML runtime must be the pinned one (`expected_onnxruntime_sha256` is
the pin's); `onnxruntime_providers_shared.dll` is required beside it.

### Adapter identity and selection

The configuration names one DXGI adapter by its stable identity
`dxgi-pci:<vendor>:<device>:<subsys>:<rev>`: the `VendorId`, `DeviceId`,
`SubSysId` and `Revision` of `DXGI_ADAPTER_DESC1` in lowercase hexadecimal of 4,
4, 8 and 2 digits. It never names a DXGI index or a LUID, which change between
boots. Each resolution:

1. lists every adapter with `CreateDXGIFactory1` and `EnumAdapters1`, in that
   order, which is the order ONNX Runtime's DirectML `device_id` indexes, and
   reads each adapter's kernel type (`D3DKMT_ADAPTERTYPE`) and PCI location
   (`D3DKMT_ADAPTERADDRESS`) by its LUID; `dxgi.dll` and `gdi32.dll` are loaded
   from System32 only;
2. keeps the adapters with the configured identity;
3. of those, keeps the physical ones: the kernel type was read, it supports
   rendering, and it is neither a software, an indirect-display nor a
   paravirtualized device (a software adapter by DXGI's flag counts too);
4. when two or more remain, keeps those whose PCI location could be read;
5. requires exactly one, and hands its index in this listing to ONNX Runtime.

No adapter with the identity is `adapter_missing` (a listing that cannot be read
too), only copies `adapter_not_physical`, and more than one `adapter_ambiguous`.
Another adapter is never used in place of the configured one. The resolution
records every adapter it listed and, for the chosen one, its identity, index and
LUID; the LUID is an observation only and is never compared. A DirectML OCR
session is bound to the identity and the index (`OcrSessionKey`, and the
provider options digest), not to the LUID.

### Loading

A DirectML run loads, by absolute path, `ort\onnxruntime_providers_shared.dll`,
then `ort\DirectML.dll`, then `ort\onnxruntime.dll`: ONNX Runtime later asks for
`DirectML.dll` by name and finds the module already loaded. The process DLL search
path is never changed. A run on the CPU loads exactly the closure of earlier
releases, `[onnxruntime_providers_shared.dll, onnxruntime.dll]`, whether or not
`DirectML.dll` lies beside them.

A DirectML runtime initialises on the first model load, in this order: the
three files against the pin, the adapter, the closure, the version (`1.24.4`),
ONNX Runtime, the executable's hash. An ONNX Runtime or providers library off
the pin, or another version, fails with `ProviderUnavailable`; a missing or
different `DirectML.dll` is `directml_library_missing` or
`directml_library_mismatch`.

### Sessions and node placement

A DirectML session registers DirectML alone on the adapter's DXGI index, with
one intra-op thread, the memory pattern off and sequential execution, and lets
ONNX Runtime place the nodes DirectML cannot run on its CPU provider (the
recognition model has such nodes). Every session build resolves the adapter
again: the model load binds the model to that build's identity and index, and
the detector, built on the first full-frame request, must find its adapter at
that index (`adapter_missing` otherwise, until the model is loaded again).

Before each DirectML session, the engine builds a zero-inference shadow
session with exactly the same options and a verbose log, reads ONNX Runtime's
`Node placements` section from it and drops it. The placement becomes one
report `actingcommand.ppocr_node_placement_diagnostic.v2` per model role:
`accelerated_execution_provider` (`DmlExecutionProvider`) with
`accelerated_node_count`, `cpu_assigned_node_count` with each CPU-placed node,
and `device {identity, dxgi_index, luid}`; it is archived as a `DiagnosticJson`
artifact with the next request's result, as the CUDA diagnostic's report is. A
placement that cannot be read whole is a session that cannot be built
(`placement_unread`). `ACTINGCOMMAND_PPOCR_NODE_PLACEMENT_DIAGNOSTIC`, still
`1` only, is met for a DirectML session, still turns the read on for a CUDA
session (whose report has no `device`) and is still refused for a CPU session.

A DirectML OCR invocation attests the fallback policy `cpu_nodes`, and
`fallback_observed` says whether a role whose session ran has a CPU-placed node.

`vision_gpu_unavailable` (warning) says that the configured GPU execution
provider cannot be used, with `gpu_unavailable_reason` and `gpu_fallback`
(`cpu`: the work goes on on the CPU; `refuse`: OCR requests fail).

### Fallback to the CPU

Slice B2 (design §1.4, §2.3). Under `gpu_fallback` `cpu` (the default):

- **At the runtime initialisation** (the first model load), a `DirectML.dll`
  that is missing or off the pin, or an adapter that does not resolve, puts the
  whole process on the CPU until it restarts: its closure is the CPU one,
  `[onnxruntime_providers_shared.dll, onnxruntime.dll]` without `DirectML.dll`,
  DirectML is never registered, and every model it loads runs on the CPU with
  that one warning (no `model_name`). An ONNX Runtime or providers library off
  the pin, or another version, still fails.
- **At a session build**, an adapter that does not resolve, a provider that does
  not register, a session that does not build or a placement that cannot be
  read moves that model whole to the CPU, once: the failed DirectML session's
  report is dropped, the model's sessions are built on the CPU, and the model
  carries one warning that names it (`model_name`), with ONNX Runtime's status
  (`foreign_vision_error`, `raw_source` `vision`, its error code as `raw_code`
  and its message as `raw_text`) or the OS failure as its link. A detector that
  cannot be built on the first full-frame request moves the recognizer too: it
  is rebuilt on the CPU from its verified file, the reports of the replaced
  DirectML sessions still held are dropped, and the session generation steps.
  The model stays on the CPU until it is unloaded; a later load tries DirectML
  again, and a new fallback is a new warning.
- **After a failed run** (slice E, design §9 Q8), a DirectML session whose run
  fails (any ONNX Runtime error of the run, not its timeout: the device was
  removed or reset) fails the request with `ProviderUnavailable`, `DirectML
  cannot be used (inference_failed)`, and the kernel unloads the engine instead
  of keeping it, so the model's next request builds it again: its adapter is
  resolved again and the session-build rule above applies. When the first
  invocation of that rebuilt model fails the same way, the model runs whole on
  the CPU from its next load until the process ends, with one warning that names
  it (`inference_failed`, ONNX Runtime's status as its link); a rebuilt model
  whose first invocation succeeds is sound again. The kernel unloads an engine
  its load reported on a GPU whenever it answers `ProviderUnavailable`, so under
  `refuse` a detector whose adapter no longer resolves unloads its model too.

A model on the CPU after a fallback keeps the key that requests `directml`, with
the selector and no resolved adapter (its provider options digest names the CPU
model, one intra-op thread), and its invocations attest the resolved provider
`cpu`, the CPU provider registered, CPU fallback allowed, the fallback policy
`cpu_model` and `fallback_observed` true. `OcrModelLoad::Loaded` carries the
model's live execution status (`OcrExecutionStatus`: requested and actual
provider and the warning), which the engine updates when a detector moves the
model. Under `refuse` every such failure fails the request as before
(`ProviderUnavailable` for an adapter or library reason, `ProviderFailure` for
the others, the message leading with `DirectML cannot be used (<reason>)`), and
the next request tries again; a failed run builds the model again on the next
request every time. A model on the CPU, and every other failure, keeps its
engine as before. An NN classifier runs on the CPU under `directml`,
which is no fallback and has no warning.
