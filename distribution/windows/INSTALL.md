# Windows Runtime

This is the Windows x86_64 MSVC Runtime. Its version is the tag (`vX.Y.Z`) of the
`HS7097/ActingCommand-Runtime` Release that carries this zip; its source identity
is the exact commit and tree in `BUILD-MANIFEST.json`. Use the instructions and
downloader from that same source revision. Installation and startup require the
user's private configuration; the included template has not been validated as a
working configuration for any device or host.

## Obtain and verify the artifact

A released version is downloaded from the Release page of
`HS7097/ActingCommand-Runtime`: take `actingcommand-runtime-<sha>.zip` and
`SHA256SUMS` from the Release, check the zip against `SHA256SUMS`, extract it into
a new directory and check every file against `BUILD-MANIFEST.json`.

An unreleased commit has no Release; its exact build is an Actions artifact.
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

Replace the bracketed values before use. With an output directory named
`runtime` as in this example, its parent (`artifacts`) becomes an install root
for this Runtime (see "Bundled adb"): instances without `adb_path` then use
`artifacts\tools\platform-tools\adb.exe` (startup refuses while it is missing),
so choose another name to keep the MuMu adb. The task directory must already exist;
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
`distribution-v1` always requires all five payloads.

Tools use their separate artifact (`-ArtifactKind Tools`,
`actingcommand-tools-<sha>`). Its manifest declares
`tools_payload_layout: "platform-tools-v2"`: the root holds `actinglab.exe`,
`actingledger.exe`, `actingcommand-vision-provider-check.exe`,
`actingcommand-device-test.exe` and `BUILD-MANIFEST.json`, and the one
subdirectory `platform-tools` holds the official Android platform-tools 37.0.1
files `adb.exe`, `AdbWinApi.dll`, `AdbWinUsbApi.dll`, `NOTICE.txt` and
`source.properties`. The manifest binds all nine payload files (paths use `/`).
The OCR engine is linked into `actingcommand-actingd.exe`, so no vision provider
DLL ships (Workflow #360). Historical Tools artifacts with
`platform-tools-v1` also carry `ac_fastdeploy_ppocr.dll` (ten files), and those
without the layout field have only five root files; any other layout is
rejected.

The build takes these platform-tools files only from Google's official archive
`https://dl.google.com/android/repository/platform-tools_r37.0.1-win.zip`, after
checking its size and the SHA-1 that Google publishes, a pinned SHA-256, and each
file's pinned size and SHA-256 (`scripts/windows-tools/windows-tool-sources.v1.json`);
`adb.exe version` reports `Version 37.0.1-15733141`. Keep `NOTICE.txt` and
`source.properties` with the binaries. Installed, the files are in
`<install root>\tools\platform-tools\`, and
`<install root>\tools\platform-tools\adb.exe` is the adb that an instance without
`adb_path` (key omitted) uses.

## Prepare private configuration

### A/B installation inputs and Host control

An A/B installation selects `<root>/A/{runtime,tools,ui}` or
`<root>/B/{runtime,tools,ui}` through `<root>/install/active.json`.
`actingcommand-contract::InstallSelection` defines this input: schema
`actingcommand.install-selection.v1`, slot, positive generation, and SHA-256
references for the slot's `MEMBERS.json` and private generation config/provider
files. acsetup owns selection and configuration commits. Runtime reads them and
keeps its state under the one configured state root.

The stable launcher supplies `ACTINGCOMMAND_INSTALL_ROOT` and
`ACTINGCOMMAND_INSTALL_SELECTION` (the complete selection JSON) to the chosen
program. Each process verifies and retains those inputs. Direct slot entry also
checks that the selected slot matches its executable. The fixed configuration
argument `<root>/actingd.config.json` resolves to the pinned private generation.
MCP subprocesses inherit that same selection and use that slot's tools; restart
the MCP process to select another generation. ADB subprocesses use the installation
root as their working directory and retain the existing slot tool hash checks.

acsetup creates and permanently retains the empty `install/slot-A.lock` and
`slot-B.lock` locators. `InstallSlotLock::try_shared(root, slot)` opens an existing
locator and acquires nonblocking OS read occupancy before slot material is read;
`try_exclusive` acquires materialization occupancy. Neither operation creates,
truncates or writes a locator. Missing, nonempty, unavailable and occupied locators
return distinct errors. `release(self)` reports an explicit unlock error; dropping
the handle or terminating its process releases the OS occupancy.

All six Runtime/Tools binary entrypoints take process-lifetime read occupancy,
including direct slot entry. An `InstalledProcess` clone retains shared occupancy
with its immutable inputs. MCP children receive the same snapshot and acquire
their own occupancy for their full process lifetimes, including after MCP exits.
UI and stable forwarding consumers use this same contract. acsetup must also
check native process/file occupation for older consumers and ADB: shared-lock
availability alone does not prove their absence or permit a slot replacement.

The installation owner uses `InstalledProcess::read_active(root)` for its current
CAS baseline, independent of inherited environment, and `from_selection_bytes(root,
bytes)` for an explicit candidate or an exact selection returned to UI. Both use
the same bounded reader and shared occupancy as `read(root)`. Running consumers
retain their original `read` result. The installer releases exclusive occupancy
after materialization, then retains the candidate reader's shared occupancy
through checking and commit; acquisition failure stops that step.

`check-config` and `ledger-maintenance verify` use `InstallConfigPurpose::CandidateCheck`:
they lock their actual executable slot and read the explicitly supplied config,
without substituting the active selection. Relative paths keep that config's
parent directory as their base. The launcher only resolves the root config alias;
an explicit candidate path is preserved. Running processes use `Running` and
require the configuration selected by their pinned snapshot.

Installation control uses the ordinary Runtime connection, with `(Cli, Cli)` or
`(User, Ui)` origin, an accepted governance identity and the exact owner target.
The CLI declares `client: actingctl`; a configured governance allowlist must
permit that identity. The client method is `RuntimeClient::install_transition`.
The corresponding CLI entry is:

```powershell
actingctl install-transition --state-root <state-root> --action-json '<action-json>'
```

The JSON action is `begin_drain` (transition_id and optional timeout_ms), `query`
(transition_id), `abort` (ticket), `commit_shutdown` (ticket), or `release`
(ticket and optional timeout_ms). Save the complete returned ticket. A drain
closes new root admission while admitted calls, leases, queued work and their
cleanup finish. Wait for `drained`, then submit that ticket's `commit_shutdown`;
`--wait 60` on this action observes the accepted owner's actual close and process
exit through the existing shutdown waiter. A completed control receipt describes
the action; only `released` describes completed new-owner preparation.

Start the selected daemon in held mode with:

```powershell
actingcommand-actingd --config <selected-config> --install-held '<held-json>'
```

The held input contains transition_id, request_id, optional timeout_ms and an
optional previous drained ticket. The Host opens its owner and ledger, answers
installation control and necessary ledger reads, and waits before constructing
the Provider. Verify its new exact owner/ticket and send `release`. It keeps root
admission closed through Provider/basic preparation and restores user pauses only
from the supplied predecessor's matching drained and accepted-shutdown ledger
facts. Ordinary startup invalidates those old installation facts.

Drain, held and release deadlines default to 60 seconds and are bounded at
600 seconds; supply a shorter duration when the authorized window is shorter.
A drain timeout removes only this transaction's barrier. A held timeout stays
closed before Provider assembly; release failure or timeout stays closed and
preserves native preparation/cleanup failures. A missing client receipt is
unknown: reconnect, declare identity and query the original transition. The
client never retries an installation action automatically. Installation status
and original pauses are the Host-owned ledger facts `host.install_transition`
and `host.install_transition.pauses`.

### Standalone configuration

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
  config schema. `adb_path` may be left out when the daemon runs from an install
  root; otherwise an explicit entry (`serial`, or `host` and `port`) still needs
  it (see "Bundled adb"). The template's `"instances": []` starts a control-plane-only
  daemon with no device targets; instances are added later by editing the
  configuration and restarting the daemon. Retain the existing identity,
  unique-registration, path, backend and timeout rules.
- A MuMu instance may instead be bound by discovery: give the entry exactly one
  of `instance_index` (the index `MuMuManager info -v all` reports) or
  `instance_name` (the exact instance name) and omit `serial`. `adb_path`,
  `host` and `port` may then be omitted; no default host or port applies, and
  any of them you do declare is cross-checked against the discovered value
  (a declared `adb_path` may also name the install root's adb, see "Bundled
  adb").
  The optional top-level `mumu_root` names the MuMu install root explicitly and
  must be absolute. At startup the daemon runs `MuMuManager.exe version` and
  `info -v all` once (read-only, 10 s timeout, never any mutating subcommand),
  resolving `MuMuManager.exe` in this order: `mumu_root`,
  `ACTINGCOMMAND_NEMU_FOLDER` (only when the top-level `allow_env_overrides` is
  `true`; otherwise a set variable is ignored and reported as
  `env_override_ignored:ACTINGCOMMAND_NEMU_FOLDER`), the install root of a running MuMu process, the
  Windows uninstall entry (`MuMuPlayer*` under the standard `Uninstall` keys of
  `HKLM`, `HKLM\...\WOW6432Node` and `HKCU`; only `InstallLocation`,
  `DisplayIcon` and `DisplayVersion` are read), then the vendor folders under
  Program Files. `MuMuManager` must report at least `6.3.2.0`, a Runtime policy
  floor. Startup refuses with `instance_discovery_unavailable`,
  `mumu_manager_version_unsupported`, `instance_discovery_no_match`,
  `instance_discovery_ambiguous` or `instance_discovery_conflict`, and
  `check-config` reports such entries as `"binding":"discovery_pending"`
  without running discovery; see `contracts/provider-startup.md`. A configured
  instance that is stopped when the daemon starts is bound pending (no port,
  `status` shows `adb_port: null`, device requests are denied with
  `instance_not_running`) and is started with
  `.\actingctl.exe emulator start --state-root <private-state-root> --instance <alias>`;
  declare no `port` for such an instance. `actingctl` reports that denial as
  `host code instance_not_running during require_bound_adb_endpoint`, from
  optional receipt fields an `actingctl` older than this daemon cannot decode.

The parser rejects unknown fields and configuration files larger than 1 MiB.
The blank state root and salt must be filled before startup. Packages with OCR
or NN targets need the top-level `vision` section, for example
`"vision": {"execution_provider": "cpu"}`: the vision root (by default
`<install root>\vision`, else `vision` next to the configuration file) holds
`ort\onnxruntime.dll` and one folder per model under `models\`, named by the
`model_ref` the packages use (`contracts/vision-model-folders.md`). The retired
`vision_provider_manifest` key is refused. Use your existing connection
configuration when those capabilities are required; optional fields must follow
the same source schema. Provider models,
SDKs, drivers, device tools and private connection data are separate dependencies
and are not installed by this Runtime artifact. Nothing in the template creates
an instance, chooses a device or supplies credentials.

Before starting, validate the filled copy without side effects:

```powershell
.\actingcommand-actingd.exe check-config --config <private-config-path>
```

It prints one JSON result line and exits 0 only when the configuration loads,
assembles and validates exactly as startup would. It creates, reads or locks
nothing under `state_root`, lists the vision root without reading any model
file, and
resolves relative policy package paths against the current directory exactly as
startup does; see `contracts/actingd-check-config.md`.

## Bundled adb

An install root is the layout acsetup installs: `<install root>\runtime\` holds
this Runtime artifact with its `BUILD-MANIFEST.json`, and `<install root>\tools\`
the Tools artifact. The daemon recognises it from its own executable path alone
(two levels above `actingcommand-actingd.exe`, `runtime\BUILD-MANIFEST.json` must
be a file). So any directory named `runtime` that holds this Runtime artifact
makes its parent an install root, whoever laid it out: an acsetup install,
acsetup's upgrade staging directory (the check acsetup runs with the staged
Runtime then uses the staged adb), or a hand layout such as `<dir>\runtime\` +
`<dir>\tools\`. A hand layout that should keep using the MuMu adb must give the
Runtime directory another name or declare `adb_path` on every instance.

When the daemon runs from an install root, an instance without `adb_path`
(explicit or discovery-bound) uses `<install root>\tools\platform-tools\adb.exe`.
An `adb_path` that names that file is the same choice. Before the ledger opens,
startup and `check-config` read `adb.exe`, `AdbWinApi.dll` and `AdbWinUsbApi.dll`
there and compare their SHA-256 with the values pinned at build time. A missing or
unreadable file refuses with `adb_install_missing`, a different one with
`adb_install_mismatch`; the `FATAL actingd:` line (and `check-config`'s
`error.detail`) names the file, the expected and the actual value. Nothing falls
back to another adb. Two fixes:

- reinstall `tools\platform-tools` (for an acsetup install, run acsetup v0.10 or
  later on it again; for a hand layout, copy the Tools artifact's
  `platform-tools` directory);
- or set the instance's `adb_path` to another adb: MuMu's own adb to use it as
  before (an explicit instance may name any other adb too).

An explicit instance may name any other adb; that one is used as before, without
a hash check. Naming the install root's adb is the same choice as omitting the
key and is checked as above. A discovery-bound instance may name only the
discovered MuMu adb or the install root's adb; anything else fails startup with
`instance_discovery_conflict` (`adb_path_conflict`), whose message lists both
accepted values. An empty or blank `adb_path` is refused with
`instance_config_invalid`, as before. Outside an install root (a development
build, or a Runtime directory under another name than `runtime`) nothing
changes: an explicit instance
without `adb_path` is refused with `instance_config_invalid`, and a
discovery-bound one uses the discovered MuMu adb. `check-config` reports the
install root's adb as `adb_default`: `{"path": ..., "state": "ok" | "missing" |
"sha256_mismatch"}`, or `null` outside an install root; a state other than `ok`
fails the check only when an instance uses that adb.

`actinglab` resolves an adb on its own only for its `adb_source` label and its
doctor output; its device commands run in the Runtime. That label does not show
which adb the Runtime uses.

Sharing the adb server (port 5037) with other tools:

- Do not point ALAS, MAA or other tools at the install root's adb. An upgrade
  moves `tools\` into `previous\`, so that path does not exist until the new
  Tools are laid out and their adb calls fail meanwhile; an adb server they
  started keeps running from `previous\`, and until it stops every later upgrade
  warns that the older previous version could not be removed. To use 37.0.1
  there as well, keep a separate byte-identical copy elsewhere.
- Do not change into `tools\`, `tools\platform-tools\` or `runtime\` and start
  adb or `actingcommand-actingd.exe` by hand there. The adb server started that
  way keeps that directory as its working directory, and an upgrade then stops
  and restores everything because the directory cannot be moved.
- An adb client reuses a running server whose protocol version equals its own
  (the third field of the first line of `adb version`, `41` for 37.0.1) and
  otherwise kills and restarts it. This has not been measured with several adb
  versions; keep the tools that share port 5037 on the same adb version.

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

Rolling back to v0.9.0 (moving `previous\` back by hand, or v0.9.0's acsetup
with a downgrade) needs two configuration edits first, because v0.9.0 requires
`adb_path` on every explicit instance and its Tools contain no
`platform-tools`:

1. Give every explicit instance without `adb_path` an adb that still exists after
   the rollback: MuMu's own adb or a separate 37.0.1 copy, not
   `<install root>\tools\platform-tools\adb.exe`.
2. Remove an `adb_path` that names the install root's adb from discovery-bound
   instances.

Forgetting either is not silent, but they surface at different points.
Forgetting edit 1 makes v0.9.0's `check-config` refuse with
`instance_config_invalid` (stage `assemble`), so v0.9.0's acsetup stops before it
changes anything (and v0.9.0 startup refuses the same way). Forgetting edit 2 is
not caught before the swap, because `check-config` does not run discovery: the
rollback completes, and the v0.9.0 Runtime then fails at provider startup with
`instance_discovery_conflict` (`adb_path_conflict`); remove that `adb_path` and
start it again. v0.9.0's acsetup also cannot move `ui\` entry by entry: if
`ui\` is the working directory of a running adb server at the moment of the
rollback (an adb server started by a Runtime the console launched), it reports
that the console must be closed first and restores everything, although the
console is closed. Then either stop the adb server and retry (this disconnects
other tools that share port 5037, such as ALAS or MAA), or roll back by hand:
move `runtime\` and `tools\` aside as whole directories and move the ones in
`previous\` back; when renaming `ui\` fails, move its entries out one by one and
the previous ones in one by one. Rolling back to an earlier v0.10.x, whose
`previous\tools` already holds `platform-tools`, needs no configuration edit.
