# Emulator Control-Plane Capability Matrix

Status: G3 offline research baseline. No provider command was executed while producing this
matrix.

## Runtime boundary

`actingcommand-device` exposes `EmulatorCapabilityBackend` only as a read-only probe contract.
The checked-in backend is an in-memory fake for deterministic Runtime rehearsals. It cannot open a
process, socket, ADB channel, emulator, or device. A real adapter must be approved separately and
must remain behind the execution-kernel device ownership boundary.

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

## Offline workstation observation

A static file-only inspection found MuMu and LDPlayer manager binaries, while their PE
`FileVersion`/`ProductVersion` fields were empty. No executable was invoked. This confirms that a
future adapter cannot rely on Windows version resources alone and must obtain version evidence from
a documented read-only provider query or mark it unavailable.

## Primary sources

- NetEase MuMu Player 12, `MuMuManager` developer guide:
  https://mumu.163.com/help/20240726/35047_1170006.html
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
