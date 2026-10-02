# External Tools Notice

This repository does not commit DroidCast_raw APK files or MuMu/Nemu IPC DLL files.

The Rust capture backend can use optional local tools named by environment variables:

- `ACTINGCOMMAND_ADB_PATH`: local path to an adb executable, read by `actinglab` and `device-test` only (`actingd` has no reader for it, see "ADB version boundary").
- `ACTINGCOMMAND_DROIDCAST_RAW_APK`: local path to a reviewed DroidCast_raw APK.
- `ACTINGCOMMAND_NEMU_FOLDER`: local MuMu Player folder.
- `ACTINGCOMMAND_NEMU_IPC_DLL`: local path to `external_renderer_ipc.dll`.

The device crate never reads these variables itself; the calling program injects them (`EnvOverrides`, Workflow #318 cfg3). `actingd` does so only when its configuration sets `allow_env_overrides: true` and otherwise reports each set variable as `env_override_ignored:<VAR>` (`contracts/actingd-check-config.md`, "Environment overrides"); prefer the `device_paths` section there. `actinglab` and `device-test` pass them through unconditionally.

When one MuMu installation has several kernel versions (`nx_device\<version>`) and capture uses the shared `nx_main\adb.exe`, Runtime picks `external_renderer_ipc.dll` from the version the target instance's running `MuMuNxDevice.exe` belongs to. `ACTINGCOMMAND_NEMU_IPC_DLL` and an explicit Nemu DLL configuration still take priority. If no running process matches the target instance, capture keeps failing with the `shared_adb_multiple_dll_versions` ambiguity error.

These files are host-local runtime tools. Keep their license review, source location, and version evidence outside the committed binary path unless a later milestone explicitly approves vendoring.

## ADB version boundary

Do not commit `adb.exe` to this repository. The Windows exact-SHA build fetches the official Android SDK Platform-Tools 37.0.1 archive from Google at build time, verifies it against `scripts/windows-tools/windows-tool-sources.v1.json` and ships `adb.exe`, `AdbWinApi.dll`, `AdbWinUsbApi.dll`, `NOTICE.txt` and `source.properties` in the Tools release artifact only (see the root `NOTICE.md`). acsetup installs them under `<install root>\tools\platform-tools\`; that `adb.exe version` reports `1.0.41` and `Version 37.0.1-15733141`.

Which adb `actingd` uses (Workflow #337; `contracts/actingd-check-config.md`, "Default ADB"):

- An install root is recognised from `actingd`'s own path: two levels above the executable, `runtime\BUILD-MANIFEST.json` must be a file. Any Runtime artifact directory named `runtime` therefore makes its parent an install root, whoever laid it out: an acsetup install, acsetup's upgrade staging directory, or a hand layout such as `<dir>\runtime\` + `<dir>\tools\`. A hand layout that should keep using the MuMu adb must give the Runtime directory another name or declare `adb_path`.
- From an install root, an instance without `adb_path` (key omitted), discovery-bound (`instance_index` / `instance_name`) or explicit (`serial`, or `host` and `port`), uses `<install root>\tools\platform-tools\adb.exe`. Before the ledger opens, `actingd` compares the SHA-256 of `adb.exe`, `AdbWinApi.dll` and `AdbWinUsbApi.dll` with the pin and refuses to start with `adb_install_missing` or `adb_install_mismatch`; it never falls back to another adb.
- A non-empty `adb_path` on a discovery-bound instance is accepted only when it is canonically the discovered MuMu adb, or, from an install root, when it names the install root's adb (an absolute path; canonicalized, or when the file is missing its nearest existing ancestor canonicalized and the rest joined back; compared ignoring case). The latter is checked as above. Any other value fails startup with `instance_discovery_conflict` / `adb_path_conflict`, whose message lists the accepted values.
- A non-empty `adb_path` on an explicit instance is used as written and is not hashed, unless it names the install root's adb.
- An empty or blank `adb_path` is refused with `instance_config_invalid`, as before.
- Outside an install root (a development build, a Runtime directory under another name than `runtime`), an explicit instance still requires `adb_path` (`instance_config_invalid`), and a discovery-bound instance without it uses the discovered MuMu adb.

All adb clients on a host share one adb server on port 5037. A client whose adb server protocol version differs from the running server's kills and restarts that server; tools with different adb builds can then keep restarting each other, which disconnects emulator devices and can make `adb exec-out screencap -p` hang until the Runtime timeout fires. By the adb source only the protocol version (the third field of the first `adb version` line, `41` for 37.0.1) decides; mixing builds has not been measured, so every adb that shares port 5037 stays on 37.0.1 (ruling of 2026-09-30). `actingd` does not check the version of a configured adb: a non-empty `adb_path` must point at a 37.0.1 adb, such as the install root's adb, a MuMu adb whose files were replaced by the same 37.0.1 files, or a separate byte-identical copy. Other tools that share port 5037 should use their own 37.0.1 copy, not the install root's adb, because an upgrade moves `tools\` aside.

`actinglab` and `device-test` resolve their own adb, in this order: `ACTINGCOMMAND_ADB_PATH`, the configured `adb_path` (`actinglab config set adb_path <path>`), `ACTINGCOMMAND_NEMU_FOLDER`, MuMu discovery, then `adb` on `PATH` with a warning. `actinglab`'s device commands run in the Runtime; its `adb_source` label does not show which adb the Runtime uses.

`actingd` never falls back to a bare `adb` on `PATH`; it runs one only when an explicit instance names it as `adb_path`.
