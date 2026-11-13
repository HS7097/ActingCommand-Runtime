# Windows build artifacts and task-local tools

These scripts provide three fail-closed, on-demand paths for Windows work:

- download one GitHub Actions artifact built from one complete Runtime commit SHA;
- materialize explicitly selected tools under a caller-owned task workspace on drive `D:`;
- fill or verify one vision root for the CPU execution provider (`Materialize-VisionRoot.ps1`,
  below), the one implementation a resource repository's CI calls.

They do not install global tools, change `PATH`, use system `%TEMP%`, or execute ADB,
MuMu/Nemu, model, provider, or Runtime binaries.

## Exact build artifacts

`Windows exact-SHA build` produces these artifacts:

- `actingcommand-runtime-<40-character-commit-sha>`: `actingcommand-actingd.exe`,
  `actingctl.exe`, `actingd.config.example.json`, `actingd.defaults.json` (from Workflow
  #387 D1b on; the downloader accepts a Runtime artifact with or without it), `INSTALL.md`,
  and `RELEASE-NOTES.md`;
- `actingcommand-tools-<40-character-commit-sha>`: `actinglab.exe`,
  `actingledger.exe`,
  `actingcommand-vision-provider-check.exe`,
  the watchdog launcher `actingwatch.exe` (Workflow #374), the MCP entry
  `actingcommand-mcp-entry.exe` (Workflow #421 MCP-ENTRY, since v0.12.18), the Lab's MCP server
  `actinglab-mcp.exe` and the MCP entry's second generation `actingcommand-mcp-entry-g2.exe`
  (Workflow #434 LAB-5G6, since 0.12.57), plus the official Android platform-tools 37.0.1 files under `platform-tools/`:
  `adb.exe`, `AdbWinApi.dll`, `AdbWinUsbApi.dll`, `NOTICE.txt` and
  `source.properties`. The OCR engine is linked into `actingcommand-actingd.exe`;
  no vision provider DLL is built or staged (Workflow #360). The device-test probe
  `actingcommand-device-test.exe` was retired in Runtime v0.11.6 and is no longer
  built or staged.

Before it compiles anything, the build fetches
`https://dl.google.com/android/repository/platform-tools_r37.0.1-win.zip` (the only
source: no other host, no cache, no fallback), requires HTTP 200 and the size and
SHA-1 that Google publishes for it, then the pinned sha256, extracts only the five
files above and checks each size and sha256, and requires `adb.exe version` to
print `Version 37.0.1-15733141`; all values come from the `platform-tools-37.0.1`
entry of `windows-tool-sources.v1.json`. Only network errors and non-200 responses
are retried (three attempts on the same URL). The build fails with
`platform_tools_source_unavailable` (no HTTP 200 response),
`platform_tools_hash_mismatch` (size, SHA-1 or sha256 differs) or
`platform_tools_content_invalid` (a file is missing, unsafe or differs, or the
revision or adb version line is wrong). So a build fails whenever `dl.google.com`
is unreachable.

Each artifact contains a root `BUILD-MANIFEST.json`. The verifier independently
resolves the commit tree and `Cargo.lock` bytes from GitHub, selects exactly one
successful workflow run, then checks the complete manifest tuple and every payload
file before publishing the download directory.

Runtime manifests declare `runtime_payload_layout: "distribution-v1"`, which
requires exactly the five Runtime payloads listed above. Historical Runtime
manifests without this field still require exactly the two original executables.
Explicit unknown, empty or non-string layouts fail; an incomplete distribution
cannot fall back to the two-file layout. Both layouts retain the flat directory,
exact case/path, complete declared/physical set, size/hash and source checks.

Tools manifests declare `tools_payload_layout: "platform-tools-v3"`, which requires
exactly `actinglab.exe`, `actingledger.exe`, `actingcommand-vision-provider-check.exe`,
`actingwatch.exe` and the five `platform-tools/` files (manifest paths use `/`), plus each of
the three MCP programs above that the manifest declares, and allows no directory other than
`platform-tools`. A build since v0.12.18 declares `actingcommand-mcp-entry.exe` (ten payloads),
and one since 0.12.57 also `actinglab-mcp.exe` and `actingcommand-mcp-entry-g2.exe` (twelve);
the layout name stays, so an artifact built before either still verifies with nine. A
`platform-tools-v3` manifest built before
Runtime v0.11.6 also declares `actingcommand-device-test.exe`, and the verifier then
requires those ten payloads. Historical `platform-tools-v2` manifests
require `actinglab.exe`, `actingledger.exe`, `actingcommand-vision-provider-check.exe`,
`actingcommand-device-test.exe` and the five `platform-tools/` files, historical
`platform-tools-v1` manifests require the `platform-tools-v2` files plus
`ac_fastdeploy_ppocr.dll`, and historical Tools manifests without this field still
require exactly the five flat files. An explicit unknown, empty or
non-string layout fails, a Runtime manifest may not declare a Tools layout, and a
Tools manifest may not declare a Runtime layout.

The three static Runtime files come from `distribution/windows` at the same build
commit. See the [installation instructions](../../distribution/windows/INSTALL.md)
and [release notes](../../distribution/windows/RELEASE-NOTES.md).
The template requires private values before startup; artifact verification does
not establish successful installation, device execution or release publication.

```powershell
pwsh -NoProfile -File scripts/windows-tools/Get-ExactBuildArtifact.ps1 `
  -Repository HS7097/ActingCommand-Runtime `
  -SourceSha 0123456789abcdef0123456789abcdef01234567 `
  -ArtifactKind Tools `
  -TaskRoot D:\ActingCommand\runtime-check `
  -OutputPath D:\ActingCommand\runtime-check\artifacts\tools
```

Supply `-RunId` when more than one successful run exists for the exact SHA. The
script never chooses a newest, `latest`, partial-SHA, failed, stale, expired, or
ambiguous result.

## Task-local tool cache

`windows-tool-sources.v1.json` is the versioned source and license inventory.
The materializer accepts only a strict child of the caller's existing `-TaskRoot`
on drive `D:`. Component selection is explicit:

- `platform-tools-37.0.1` downloads the hash-bound official Google archive only
  after `-AcceptAndroidSdkLicense`. The same entry pins the five files the
  Tools artifact redistributes (`distributed_files`, by the owner's decision in
  Workflow #337; see `license.redistribution_note`).
- `ppocrv6-medium-source` downloads the pinned official Paddle inference sources.
  Source archives alone cannot satisfy the Runtime ONNX contract, so this selection
  is published only as `PendingVerification` and fails before ready use.
- `ppocrv6-medium-onnx` downloads pinned official ONNX detector/recognizer files
  and the v3.7.0 dictionary. Because this script must not load a model/provider,
  the result is recorded as `PendingVerification` and fails before it can be used
  as a ready Runtime contract. The script decides this by the component's id and
  does not read its `compatibility.state`, which says `Ready` since Workflow #428
  CI-OCR-1 (the Runtime's vision pipeline runs these files; see the entry's
  `reason`). `Materialize-VisionRoot.ps1` places these files in a vision root.
- `onnxruntime-gpu-1.24.4` downloads the exact official ONNX Runtime v1.24.4
  Windows GPU archive and extracts only `onnxruntime.dll`,
  `onnxruntime_providers_shared.dll`, and `onnxruntime_providers_cuda.dll` under
  fixed file-count and byte bounds. It does not infer or copy CUDA/cuDNN/driver
  files from `PATH`, System32, or another cache.
- `onnxruntime-directml-1.24.4` and `directml-1.15.4` (Workflow #407 GPU-1)
  download the official NuGet packages `Microsoft.ML.OnnxRuntime.DirectML` 1.24.4
  and `Microsoft.AI.DirectML` 1.15.4 from `api.nuget.org` (size and SHA-256 pinned;
  NuGet's published SHA-512 is recorded as `official_sha512_base64`) and extract
  only the x64 `onnxruntime.dll` and `onnxruntime_providers_shared.dll`, the x64
  `DirectML.dll`, and each package's license files. These are the DirectML runtime
  of a vision root: the Runtime package and the Tools package do not carry them;
  the Windows installation package does, with their license files and with the
  three files of `ppocrv6-medium-onnx` and PaddleOCR's `LICENSE`, by the owner's
  decision (Workflow #441; each entry's `license.redistribution_note`).
  Whoever places them copies each `pinned_files` entry to
  its `vision_root_path` under the vision root and the license files to
  `ort\licenses\onnxruntime\` and `ort\licenses\directml\`
  (`contracts/vision-model-folders.md`). The sizes and SHA-256 of `pinned_files`
  equal the runtime pin in `crates/vision-ffi/src/runtime_pin.rs` (a logic test
  holds them equal, and `Test-WindowsRemoteBuildTools.ps1` freezes both entries).
  `onnxruntime-directml-1.24.4` also pins its two license files the same way
  (`pinned_license_files`, Workflow #428 CI-OCR-1).
- `mumu-nemu-installed` records metadata for one explicit installed root and
  `nx_device` version. Vendor files remain in place and are never copied or run.

CPU/CUDA selection is exact lowercase and has no automatic fallback. CUDA also
requires both an ordinal and stable identity. Example:

```powershell
pwsh -NoProfile -File scripts/windows-tools/Materialize-TaskToolCache.ps1 `
  -TaskRoot D:\ActingCommand\runtime-check `
  -CacheRoot D:\ActingCommand\runtime-check\cache\platform-tools `
  -Component platform-tools-37.0.1 `
  -AcceptAndroidSdkLicense
```

Every published cache directory contains `PROVENANCE.json` with selected sources,
versions, original paths, cache paths, sizes, hashes, license notes, explicit
backend, fallback state, execution state, and cleanup classification. `Ready`
means exact bytes were materialized; `functional_validation_performed` remains
false. The `provider-v0.3` component that copied a caller's v0.3 provider closure
is retired with the provider DLL (Workflow #360).

## CPU vision root

`Materialize-VisionRoot.ps1` (Workflow #428 CI-OCR-1, section CI-OCR-1 of
[the register](../../contracts/v012-register.md)) fills or verifies one vision root for the CPU
execution provider from `windows-tool-sources.v1.json`. It is the one implementation: a resource
repository's CI fetches it, with the registry beside it, at its Tools pin's commit and calls it; no
repository keeps a copy of its logic. `Materialize-TaskToolCache.ps1` keeps its own behaviour.

```powershell
pwsh -NoProfile -File scripts/windows-tools/Materialize-VisionRoot.ps1 `
  -Destination "$env:RUNNER_TEMP\runtime-tools\vision" `
  -ExecutionProvider cpu
```

- `-Destination`: the vision root; created when it does not exist (a restored cache exists).
- `-Registry`: the registry; by default `windows-tool-sources.v1.json` beside the script.
- `-ExecutionProvider`: only `cpu` is taken; any other value (`directml` among them) is refused
  by name. The files of a DirectML vision root are not placed here.
- `-Attempts`: attempts per fetched file, at least 1, default 3.

It places, in the layout of [vision-model-folders.md](../../contracts/vision-model-folders.md):

| Under the vision root | From |
|---|---|
| `ort/onnxruntime.dll`, `ort/onnxruntime_providers_shared.dll` | the `pinned_files` of `onnxruntime-directml-1.24.4` at their `vision_root_path`, extracted from the package that entry pins |
| `ort/licenses/onnxruntime/LICENSE`, `ort/licenses/onnxruntime/ThirdPartyNotices.txt` | the `pinned_license_files` of the same entry at their `vision_root_path`, extracted from the same package, as the entry's `placement_note` asks; a registry without them is refused |
| `models/PP-OCRv6_medium/det.onnx`, `rec.onnx`, `keys.txt` | the artifacts `PP-OCRv6_medium_det_onnx`, `PP-OCRv6_medium_rec_onnx` and `ppocrv6_dict` of `ppocrv6-medium-onnx`, in the flat layout (the folder name is the `model_ref`); the entry's `compatibility.state` must be `Ready` |

Every address, size and SHA-256 comes from the registry; the script holds none. Every file has a
pin, the license files included. A file that exists with its pinned size and SHA-256 is kept
(`verified`). Any other file is removed if present, fetched to a temporary name beside it, checked
for size and SHA-256 and moved into place (`downloaded`); a fetched file that fails its check is
removed. A fetch that fails (a network error, an HTTP status other than 200, a wrong length or
SHA-256, no complete answer within five minutes) is tried again, 5 s, 10 s, … later, up to
`-Attempts` times; then the run fails. The package (12.5 MB) is fetched only when a file it pins
(a runtime or a license file) did not verify, and only those files are extracted from it; a
package entry that differs from its pin fails the run and is not left in place. A run on a complete
vision root (a restored cache) fetches nothing at all. The package is fetched into the destination
under a temporary name and removed before the script ends; nothing is written outside the
destination.

It writes no configuration file and starts no program: the caller writes its configuration and
runs its own self-check. On success it prints one JSON line on stdout and exits 0:

```json
{"execution_provider":"cpu","destination":"…","registry":{"path":"…","sha256":"<SHA-256 of the registry file>"},"files":[{"path":"ort/onnxruntime.dll","size":17328152,"sha256":"…","state":"verified","component":"onnxruntime-directml-1.24.4"},…]}
```

`files` lists all seven files, each `verified` or `downloaded`. Diagnostics go to stderr. Any
other outcome exits 1 with `Materialize-VisionRoot: FAILED: <file>: <reason>` on stderr; nothing is
skipped silently.

`Test-MaterializeVisionRoot.ps1` checks it without the network: a made-up registry on the reserved
host `example.invalid` and made-up files served from a local directory through a hidden test-only
parameter, which the script refuses unless every address of the entries it reads is on that host.
The `Test Windows artifact tooling` step of the Windows exact-SHA build runs it.

## Cleanup

Cache payloads are reproducible task-local copies, but deletion is not automatic.
At task end:

1. stop and verify release of every process that could hold a cache file;
2. durably preserve `PROVENANCE.json`, source URLs, versions, hashes, logs, first
   reds, fixtures, and other required evidence outside the cache directory;
3. re-check that the candidate is committed/pushed and the cache path is a strict
   child of the intended task root on `D:`;
4. remove only the exact `actingcommand-windows-tools-v1.ready` or
   `actingcommand-windows-tools-v1.pending-verification` directory.

Do not use broad `git clean`, delete a worktree, remove an unknown cache, or delete
the provenance/evidence needed to reproduce an unresolved failure.
