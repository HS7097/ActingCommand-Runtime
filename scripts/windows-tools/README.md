# Windows build artifacts and task-local tools

These scripts provide two fail-closed, on-demand paths for Windows work:

- download one GitHub Actions artifact built from one complete Runtime commit SHA;
- materialize explicitly selected tools under a caller-owned task workspace on drive `D:`.

They do not install global tools, change `PATH`, use system `%TEMP%`, or execute ADB,
MuMu/Nemu, model, provider, or Runtime binaries.

## Exact build artifacts

`Windows exact-SHA build` produces these artifacts:

- `actingcommand-runtime-<40-character-commit-sha>`: `actingcommand-actingd.exe`,
  `actingctl.exe`, `actingd.config.example.json`, `INSTALL.md`, and `RELEASE-NOTES.md`;
- `actingcommand-tools-<40-character-commit-sha>`: `actinglab.exe`,
  `actingledger.exe`,
  `actingcommand-vision-provider-check.exe`, `actingcommand-device-test.exe` and
  the watchdog launcher `actingwatch.exe` (Workflow #374), plus the official Android platform-tools 37.0.1 files under `platform-tools/`:
  `adb.exe`, `AdbWinApi.dll`, `AdbWinUsbApi.dll`, `NOTICE.txt` and
  `source.properties`. The OCR engine is linked into `actingcommand-actingd.exe`;
  no vision provider DLL is built or staged (Workflow #360).

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
exactly the ten Tools payloads listed above (manifest paths use `/`) and allows no
directory other than `platform-tools`. Historical `platform-tools-v2` manifests
require the same files without `actingwatch.exe`, historical `platform-tools-v1`
manifests require the `platform-tools-v2` files plus `ac_fastdeploy_ppocr.dll`, and
historical Tools manifests without this field still require exactly the five flat
files. An explicit unknown, empty or
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
  -TaskRoot D:\task\runtime-check `
  -OutputPath D:\task\runtime-check\artifacts\tools
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
  as a ready Runtime contract.
- `onnxruntime-gpu-1.24.4` downloads the exact official ONNX Runtime v1.24.4
  Windows GPU archive and extracts only `onnxruntime.dll`,
  `onnxruntime_providers_shared.dll`, and `onnxruntime_providers_cuda.dll` under
  fixed file-count and byte bounds. It does not infer or copy CUDA/cuDNN/driver
  files from `PATH`, System32, or another cache.
- `mumu-nemu-installed` records metadata for one explicit installed root and
  `nx_device` version. Vendor files remain in place and are never copied or run.

CPU/CUDA selection is exact lowercase and has no automatic fallback. CUDA also
requires both an ordinal and stable identity. Example:

```powershell
pwsh -NoProfile -File scripts/windows-tools/Materialize-TaskToolCache.ps1 `
  -TaskRoot D:\task\runtime-check `
  -CacheRoot D:\task\runtime-check\cache\platform-tools `
  -Component platform-tools-37.0.1 `
  -AcceptAndroidSdkLicense
```

Every published cache directory contains `PROVENANCE.json` with selected sources,
versions, original paths, cache paths, sizes, hashes, license notes, explicit
backend, fallback state, execution state, and cleanup classification. `Ready`
means exact bytes were materialized; `functional_validation_performed` remains
false. The `provider-v0.3` component that copied a caller's v0.3 provider closure
is retired with the provider DLL (Workflow #360).

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
