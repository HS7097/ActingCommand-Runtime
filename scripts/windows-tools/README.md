# Windows build artifacts and task-local tools

These scripts provide two fail-closed, on-demand paths for Windows work:

- download one GitHub Actions artifact built from one complete Runtime commit SHA;
- materialize explicitly selected tools under a caller-owned task workspace on drive `D:`.

They do not install global tools, change `PATH`, use system `%TEMP%`, or execute ADB,
MuMu/Nemu, model, provider, or Runtime binaries.

## Exact build artifacts

`Windows exact-SHA build` produces these artifacts:

- `actingcommand-runtime-<40-character-commit-sha>`: `actingcommand-actingd.exe`
  and `actingctl.exe`;
- `actingcommand-tools-<40-character-commit-sha>`: `actinglab.exe`,
  `actingcommand-vision-provider-check.exe`, `actingcommand-device-test.exe`, and
  the existing PP-OCR cdylib staged as `ac_fastdeploy_ppocr.dll`.

Each artifact contains a root `BUILD-MANIFEST.json`. The verifier independently
resolves the commit tree and `Cargo.lock` bytes from GitHub, selects exactly one
successful workflow run, then checks the complete manifest tuple and every payload
file before publishing the download directory.

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
  after `-AcceptAndroidSdkLicense`; it is not redistributed in build artifacts.
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
- `provider-v0.3` accepts an exact-hash caller manifest using
  `actingcommand.vision_provider_artifacts.v0.3` plus an exact-hash private
  `actingcommand.provider_runtime_dependencies.v1` manifest. It rehashes and
  copies the canonical provider, detector, recognizer, dictionary, selected
  `onnxruntime.dll`, and every declared companion DLL.
- `mumu-nemu-installed` records metadata for one explicit installed root and
  `nx_device` version. Vendor files remain in place and are never copied or run.

CPU/CUDA selection is exact lowercase and has no automatic fallback. CUDA also
requires both an ordinal and stable identity. Examples:

```powershell
pwsh -NoProfile -File scripts/windows-tools/Materialize-TaskToolCache.ps1 `
  -TaskRoot D:\task\runtime-check `
  -CacheRoot D:\task\runtime-check\cache\platform-tools `
  -Component platform-tools-37.0.1 `
  -AcceptAndroidSdkLicense

pwsh -NoProfile -File scripts/windows-tools/Materialize-TaskToolCache.ps1 `
  -TaskRoot D:\task\runtime-check `
  -CacheRoot D:\task\runtime-check\cache\provider `
  -Component provider-v0.3 `
  -OcrBackend cuda `
  -CudaDeviceOrdinal 0 `
  -CudaStableIdentity 'cuda-uuid:...' `
  -ProviderArtifactManifestPath D:\task\runtime-check\provider\artifacts.json `
  -ProviderArtifactManifestSha256 <64-lowercase-hex> `
  -ProviderDependencyManifestPath D:\task\runtime-check\provider\dependencies.json `
  -ProviderDependencyManifestSha256 <64-lowercase-hex>
```

The private dependency manifest must declare `backend`, `closure_complete: true`,
`selected_core_path`, and a bounded `dependencies` array. Each dependency records
`path`, `sha256`, `source`, `version`, `license_provenance_note`, and `kind` set to
either `onnxruntime_archive` or `external_cuda`. The selected core occurs exactly
once. CPU omits `cuda_device`; CUDA requires an ordinal, stable identity, all three
pinned ONNX Runtime DLL names, and at least one explicit task-local external CUDA
dependency record. Duplicate or case-colliding names, path escapes, reparse points,
missing companions, undeclared fallback, hash mismatch, and count or byte overflow
fail loud.

Every published cache directory contains `PROVENANCE.json` with selected sources,
versions, original paths, cache paths, sizes, hashes, license notes, explicit
backend, fallback state, execution state, and cleanup classification. A clean
`provider/vision-provider-artifacts.v0.3.json` is emitted with only existing public
fields and rewritten task-cache paths. `Ready` means exact bytes were materialized;
`functional_validation_performed` remains false and provider identity, DLL-load
closure, selected device, accuracy, and performance remain Pending Verification.

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
