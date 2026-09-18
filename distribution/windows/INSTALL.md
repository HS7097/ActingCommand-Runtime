# Windows Runtime candidate

This is an unreleased Windows x86_64 MSVC candidate. Its source identity is the
exact commit and tree in `BUILD-MANIFEST.json`. Use the instructions and downloader
from that same source revision. Installation and startup require the user's
private configuration; the included template has not been validated as a working
configuration for any device or host.

## Obtain and verify the artifact

Select the complete 40-character lowercase commit SHA and a successful
`Windows exact-SHA build` Actions run in `HS7097/ActingCommand-Runtime`. Use the
existing `scripts/windows-tools/Get-ExactBuildArtifact.ps1` from that source
checkout with PowerShell 7 and authenticated GitHub CLI access to the repository.
For example, after creating your own task directory on drive D:

```powershell
pwsh -NoProfile -File scripts/windows-tools/Get-ExactBuildArtifact.ps1 `
  -Repository HS7097/ActingCommand-Runtime `
  -SourceSha <exact-40-character-commit-sha> `
  -ArtifactKind Runtime `
  -RunId <exact-successful-run-id> `
  -TaskRoot D:\task\runtime-install `
  -OutputPath D:\task\runtime-install\artifacts\runtime
```

Replace the bracketed values before use. The task directory must already exist;
the output directory must be new and a strict child of it. The downloader selects
the exact native Actions artifact, extracts it to staging and verifies it before
making the output directory available. It checks the source repository, commit,
tree, Cargo.lock hash, recorded Rust toolchain, target, release profile,
run/attempt and artifact name, then the fixed payload names and every size/hash.
Ambiguous or unsuccessful runs, missing/extra files and failed verification stop
the download. Keep the manifest and original source/run identity with the files.

The Runtime manifest declares `runtime_payload_layout: "distribution-v1"`.
The artifact root contains exactly these six files:

- `actingcommand-actingd.exe`
- `actingctl.exe`
- `actingd.config.example.json`
- `INSTALL.md`
- `RELEASE-NOTES.md`
- `BUILD-MANIFEST.json`

The manifest binds the five payload files individually. Historical Runtime
artifacts without the layout field have the fixed two-executable payload plus
their manifest. An explicit unknown, empty or non-string layout is rejected;
`distribution-v1` always requires all five payloads. Tools use their separate
artifact and fixed five-binary/DLL payload.

## Prepare private configuration

Copy `actingd.config.example.json` to a private editable configuration path. Keep
the distributed template unchanged so its manifest hash remains meaningful.
Fill the copy according to `apps/actingd/src/config.rs` at the manifest commit:

- Keep `schema_version` as `actingcommand.actingd.config.v1`.
- Set `state_root` to the intended private Runtime state directory, preferably
  an absolute path. Clients must use this same root, not its `ledger` subdirectory.
- Keep `bind_host` a numeric loopback IP, such as the template's `127.0.0.1`.
  `bind_port` is an unsigned 16-bit integer; zero, also the omitted-field default,
  lets the OS choose the listening port.
- Set `secret_fingerprint_salt` to your private value, 16 through 1024 UTF-8 bytes.
  The empty template value is deliberately incomplete. Do not publish the filled
  configuration or include its secrets in reports.
- Populate `instances` with your already authorized instance configuration.
  Each entry requires `alias` and the existing typed `instance_id`; device entries
  also need the explicit backend and connection fields required by the same
  config schema. The template's `"instances": []` starts a control-plane-only
  daemon with no device targets; instances are added later by editing the
  configuration and restarting the daemon. Retain the existing identity,
  unique-registration, path, backend and timeout rules.
- A MuMu instance may instead be bound by discovery: give the entry exactly one
  of `instance_index` (the index `MuMuManager info -v all` reports) or
  `instance_name` (the exact instance name) and omit `serial`. `adb_path`,
  `host` and `port` may then be omitted; no default host or port applies, and
  any of them you do declare is cross-checked against the discovered value.
  The optional top-level `mumu_root` names the MuMu install root explicitly and
  must be absolute. At startup the daemon runs `MuMuManager.exe version` and
  `info -v all` once (read-only, 10 s timeout, never any mutating subcommand),
  resolving `MuMuManager.exe` in this order: `mumu_root`,
  `ACTINGCOMMAND_NEMU_FOLDER`, the install root of a running MuMu process, the
  Windows uninstall entry (`MuMuPlayer*` under the standard `Uninstall` keys of
  `HKLM`, `HKLM\...\WOW6432Node` and `HKCU`; only `InstallLocation`,
  `DisplayIcon` and `DisplayVersion` are read), then the vendor folders under
  Program Files. `MuMuManager` must report at least `6.3.2.0`, a Runtime policy
  floor. Startup refuses with `instance_discovery_unavailable`,
  `mumu_manager_version_unsupported`, `instance_discovery_no_match`,
  `instance_discovery_ambiguous`, `instance_discovery_conflict` or
  `instance_discovered_stopped` (the configured instance must be running when
  the daemon starts), and
  `check-config` reports such entries as `"binding":"discovery_pending"`
  without running discovery; see `contracts/provider-startup.md`.

The parser rejects unknown fields and configuration files larger than 1 MiB.
The blank state root and salt must be filled before startup. Use your existing
provider manifest and connection configuration when those capabilities are
required; optional fields must follow the same source schema. Provider models,
SDKs, drivers, device tools and private connection data are separate dependencies
and are not installed by this Runtime artifact. Nothing in the template creates
an instance, chooses a device or supplies credentials.

Before starting, validate the filled copy without side effects:

```powershell
.\actingcommand-actingd.exe check-config --config <private-config-path>
```

It prints one JSON result line and exits 0 only when the configuration loads,
assembles and validates exactly as startup would. It creates, reads or locks
nothing under `state_root`, does not read the vision provider manifest, and
resolves relative policy package paths against the current directory exactly as
startup does; see `contracts/actingd-check-config.md`.

## Start, inspect and close

From the verified Runtime directory, using your filled private configuration:

```powershell
.\actingcommand-actingd.exe --config <private-config-path>
```

The daemon remains in that process. Its normal startup line is
`actingd ready pid=<pid> host=<host> port=<port>`. With `"instances": []`
the daemon starts control-plane-only: it records `runtime.started` and no
instance binding, `actingctl status` reports no instances, and instances are
added by editing the configuration and restarting the daemon. From a second
terminal, use the same private state root for the existing control commands:

```powershell
.\actingctl.exe status --state-root <private-state-root>
.\actingctl.exe request-shutdown --state-root <private-state-root>
```

These are manual commands; the artifact registers no service or startup task.
Wait for normal daemon exit and the existing shutdown/closure facts. A request
receipt alone does not establish that every resource has been released.

GlobalLedger is the Runtime's source of operational and diagnostic facts.
`actingctl` returns the official response or error for its request. The separate
Tools artifact supplies `actingledger.exe` for the existing read-only ledger
commands. Preserve errors and the associated ledger facts; when startup or the
ledger itself is unavailable, retain the daemon's original `FATAL actingd:` line
and nonzero exit result. Do not treat a startup line as proof of device execution.

## Upgrade boundary

Keep the exact prior artifact, manifest, private configuration and state
provenance. Confirm compatibility with the selected source before changing an
existing installation. This candidate supplies no automatic state migration,
recovery, cleanup or rollback procedure. Installation or successful Actions
verification does not establish real-device acceptance or authorize publication.
