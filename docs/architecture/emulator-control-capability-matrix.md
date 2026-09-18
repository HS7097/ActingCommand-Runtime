# Emulator Control-Plane Capability Matrix

Status: G3 offline research baseline. No provider command was executed while producing this
matrix.

## Runtime boundary

`actingcommand-device` exposes `EmulatorCapabilityBackend` only as a read-only probe contract.
The checked-in backends are an in-memory fake for deterministic Runtime rehearsals and, since
Runtime slice #316-A, a MuMu backend over an already obtained discovery report (see below). Neither
probe opens a process, socket, ADB channel, emulator, or device. A real control adapter must be
approved separately and must remain behind the execution-kernel device ownership boundary.

The contract vocabulary is closed:

| Capability ID | Meaning |
| --- | --- |
| `inventory.read` | List configured provider instances. |
| `instance.status.read` | Read lifecycle/readiness state for an instance. |
| `instance.start` | Start one instance. |
| `instance.stop` | Stop one instance. |
| `instance.restart` | Restart one instance. |
| `instance.create` | Create one instance. |
| `instance.clone` | Clone one instance. |
| `instance.delete` | Delete one instance. |
| `instance.configure` | Change instance configuration. |
| `application.control` | Query, install, start, stop, or remove an application. |
| `adb.bridge` | Obtain or use the provider's documented ADB bridge. |
| `snapshot.manage` | List, save, load, or delete emulator snapshots. |

Unknown capability IDs are fatal. Every profile must state `available`, `unavailable`, or
`unverified` for every capability. `unavailable` and `unverified` are denials, never fallback
success.

## Public-document matrix

`D` means the reviewed official document describes the capability. `C` means conditional support
that needs prior user/provider configuration. `U` means the reviewed official material did not
establish the capability and Runtime must not claim it.

| Provider surface | Version evidence | Inventory / status | Start / stop / restart | Create / clone / delete | Configure | App control | ADB bridge | Snapshots |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| MuMu Player 12 `MuMuManager.exe` | Minimum `4.0.0.3179` | D / D | D / D / D | D / D / D | D | D | D | U |
| LDPlayer `ldconsole.exe` / `dnconsole.exe` | No minimum stated in reviewed page | D / D (`list2`) | D / D / D | D / D / D | D | D | D | U |
| Android Emulator CLI + authenticated console | Tool version must be probed by a future adapter | D / D | D / D / D | D / U / D | D | C via ADB | C via ADB or authenticated console | D |
| BlueStacks 5 public support surface | Product family only; no control-CLI version contract found | U / U | U / U / U | U / U / U | U | C via ADB | C; user must enable ADB | U |

The Android Emulator `instance.delete` and `instance.configure` entries refer to `avdmanager` over
configured AVDs, while running-instance state and snapshots use the authenticated emulator console.
Those are separate tools and a future adapter must not pretend they form one atomic provider API.

## Failure and version semantics

- MuMu `info` documents structured fields including `error_code`, `launch_err_code`,
  `launch_err_msg`, process state, Android-started state, PID, window handles, and ADB endpoint.
  A future adapter must validate both process and Android readiness rather than treating process
  creation as successful boot.
- LDPlayer documents comma-separated `list2` output but does not publish a stable structured error
  schema in the reviewed page. Non-zero exit, timeout, malformed field count, invalid numeric data,
  or empty required output must therefore fail explicitly.
- The Android Emulator console requires localhost access and token authentication and uses `OK` as
  the ready/success acknowledgement. Authentication failure, missing `OK`, connection loss, or
  malformed status is fatal for that request.
- BlueStacks documents an opt-in ADB bridge, not a lifecycle management contract. Runtime must
  report lifecycle capabilities as unverified instead of inferring private executables or registry
  behavior.
- Vendor versions are opaque bounded strings. Runtime records exact, minimum, or unavailable
  version evidence and does not compare unrelated vendor formats as semantic versions.
- A future real adapter must use bounded timeouts and preserve provider stdout, stderr, exit status,
  parsed error fields, provider/version, capability, and target instance in diagnostics. G3 adds no
  retry, reconnect, fallback, or provider process invocation.

## MuMuManager 6.5.7.0 observation (Runtime slice #316, discovery only)

Observed on the owner's machine with the read-only subcommands only; this is the basis of
`actingcommand-device::mumu_manager`:

- The executable is `<root>\nx_main\MuMuManager.exe` (v5/v6 layout; legacy installs used
  `<root>\shell\MuMuManager.exe`). There is no `nx_device\<version>\shell` variant, so the ADB
  version-match invariants do not apply to it.
- `MuMuManager.exe version` prints `{"version":"6.5.7.0"}` and exits 0.
- `MuMuManager.exe info -v all` prints a JSON map keyed by the index string; each entry carries
  `index` (a string), `name`, `adb_host_ip`, `adb_port` (a number), `is_process_started`,
  `is_android_started`, `player_state`, `pid`, optional `headless_pid`, window handles,
  `vt_enabled`, `hyperv_enabled`, `error_code`, `launch_err_code`, `launch_err_msg`,
  `launch_time`, `android_version`, `disk_size_bytes`, `created_timestamp`, `is_main` and
  `info_source`. `info -v <single>` returns a flat object instead; Runtime always asks for `all`
  and tolerates the flat shape. Output is UTF-8 without BOM, LF-terminated.
- Failures are an undocumented top-level `{"errcode":<n>,"errmsg":"..."}` envelope with the exit
  code equal to `errcode` (for example `-200` index not found, `-23` bad parameter). A
  multi-index query with one failing entry still exits 0 and carries the envelope inside that
  entry, so every map entry is checked for `errcode` before it is read as an instance.
- The hidden `api` subcommand attaches to an instance merely on dispatch, so it is banned; only
  `version` and `info` are dispatched, `control`, `setting`, `launch`, `shutdown` and `restart`
  never are.
- `6.3.2.0` is a Runtime policy floor with no vendor basis; the vendor documents only
  `V4.0.0.3179` as the `MuMuManager` baseline. Windows registry `DisplayVersion` is advisory;
  `MuMuManager version` is authoritative.

## MuMu provider profile (Runtime slice #316-A)

`actingcommand-device::mumu_manager::mumu_capability_profile` derives one
`EmulatorCapabilityProfile` from a `MumuDiscoveryReport`. It is a pure function: it dispatches
nothing and reads nothing; every row states what `MuMuManager version` and `info -v all` already
answered. `MumuEmulatorCapabilityBackend` holds one report and implements
`EmulatorCapabilityBackend` by calling that builder; its `from_discovery` constructor runs the
read-only discoverer once. This slice dispatches no `control` subcommand.

- `provider_id` is `mumu.manager`; the version evidence is `exact` with the `MuMuManager version`
  value, kept as an opaque bounded string and never compared semantically in the profile.
- Available (supported): `inventory.read` and `instance.status.read`, evidence
  `mumu_manager.info`: `info -v all` answered with exit 0 and a parseable instance map at
  discovery. A refusal surfaces as a fatal typed device error (`mumu_manager.run`, `.exit`,
  `.decode`, `.json`, `.errcode`, `.shape`, `.output_bound`), never as an empty inventory.
- Unverified (supported): `instance.start`, `instance.stop` and `instance.restart`, evidence the
  vendor command reference (https://mumu.163.com/help/20240807/40912_1170006.html), which documents
  `control -v <index> launch|shutdown|restart`. The Runtime has not exercised it, so
  `capability()` still refuses these rows; the emulator control slice flips them.
- Unavailable (unsupported), evidence `mumu.manager`: `instance.create`, `instance.clone`,
  `instance.delete`, `instance.configure` and `snapshot.manage` are not driven by this Runtime;
  `application.control` and `adb.bridge` are not driven through `MuMuManager` (the instance is
  reached through its discovered ADB target, not through this provider claim); every `input.*`,
  `capture.frame`, `application.launch`, `application.stop` and `application.restart` belong to
  the touch/capture backend registry and the ADB application lifecycle path.

At provider startup `actingd` admits the profile through `admit_emulator_capabilities` with the
required ids `inventory.read` and `instance.status.read`, records it in the ledger (stage
`capability_admission`, see `contracts/provider-startup.md`) and attaches it to every discovered
registration. `ExecutionBackendRegistry::register` then merges it with the registry-derived
profile: `provider_id`, the version and the rows `inventory.read`, `instance.status.read`,
`instance.start`, `instance.stop`, `instance.restart`, `instance.create`, `instance.clone`,
`instance.delete`, `instance.configure`, `snapshot.manage` and `adb.bridge` come from the
provider profile; `application.control`, every `input.*`, `capture.frame` and
`application.launch|stop|restart` keep the registry-derived evidence. The merged profile is
rebuilt through `EmulatorCapabilityProfile::new`, so the completeness and duplicate rules run
again, and a merge that fails validation refuses the registration with
`execution_capability_profile_merge_invalid`. Explicit (non-discovered) entries, fixtures and
tests keep the registry-only placeholder profile (`runtime.execution_backend_registry`, version
unavailable) unchanged. The `mumu-discover` probe prints the same profile as a `capabilities`
object (`provider_id`, `version` and the sorted `available`, `unverified` and `unavailable` id
lists) without starting the daemon.

## Offline workstation observation

A static file-only inspection found MuMu and LDPlayer manager binaries, while their PE
`FileVersion`/`ProductVersion` fields were empty. No executable was invoked. This confirms that a
future adapter cannot rely on Windows version resources alone and must obtain version evidence from
a documented read-only provider query or mark it unavailable.

## Primary sources

- NetEase MuMu Player 12, `MuMuManager` developer guide:
  https://mumu.163.com/help/20240726/35047_1170006.html
- NetEase MuMu Player, `MuMuManager` command reference (`version`, `info`):
  https://mumu.163.com/help/20240807/40912_1170006.html
- LDPlayer, command-line interface guide:
  https://www.ldplayer.net/blog/introduction-to-ldplayer-command-line-interface.html
- LDPlayer Korea, extended command table including restart and running-state queries:
  https://kr.ldplayer.net/blog/an-introduction-to-ldplayer-command-line-interface.html
- Android Developers, emulator command line:
  https://developer.android.com/studio/run/emulator-commandline
- Android Developers, authenticated emulator console:
  https://developer.android.com/studio/run/emulator-console
- Android Developers, `avdmanager`:
  https://developer.android.com/tools/avdmanager
- BlueStacks 5, official ADB enablement guide:
  https://support.bluestacks.com/hc/en-us/articles/23925869130381-How-to-enable-Android-Debug-Bridge-on-BlueStacks-5
