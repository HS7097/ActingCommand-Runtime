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
  `actingcommand-vision-provider-check.exe`, and `actingcommand-device-test.exe`.

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
- `provider-v0.3` accepts an exact-hash caller manifest using
  `actingcommand.vision_provider_artifacts.v0.3`. It rehashes and copies the
  declared provider, single ONNX Runtime library, detector, recognizer, and
  dictionary. It also remains `PendingVerification` without a permitted
  functional provider check.
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
  -ProviderArtifactManifestSha256 <64-lowercase-hex>
```

Every published cache directory contains `PROVENANCE.json` with selected sources,
versions, sizes, hashes, license notes, explicit backend, fallback state, execution
state, and cleanup classification. A source archive is not evidence that its
contents satisfy the Runtime ONNX/provider contract.

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
