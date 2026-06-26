# External Tools Notice

This repository does not commit DroidCast_raw APK files or MuMu/Nemu IPC DLL files.

The Rust capture backend can discover optional local tools through environment variables:

- `ACTINGCOMMAND_ADB_PATH`: local path to the adb executable that matches the running emulator.
- `ACTINGCOMMAND_DROIDCAST_RAW_APK`: local path to a reviewed DroidCast_raw APK.
- `ACTINGCOMMAND_NEMU_FOLDER`: local MuMu Player folder.
- `ACTINGCOMMAND_NEMU_IPC_DLL`: local path to `external_renderer_ipc.dll`.

These files are host-local runtime tools. Keep their license review, source location, and version evidence outside the committed binary path unless a later milestone explicitly approves vendoring.

## ADB version boundary

Do not commit `adb.exe` to this repository.

When controlling MuMu Player instances, ActingCommand must use the adb executable that matches the MuMu adb server. On the current Windows test host, the reviewed matching executable is:

```text
D:\BST\MuMuPlayer\nx_main\adb.exe
```

That MuMu adb is currently `1.0.41 / 36.0.0`. Mixing it with other installed adb builds, such as Android SDK/platform-tools or Python virtualenv copies, can kill and restart the MuMu adb server because the adb server version differs. That version fight can disconnect emulator devices and cause `adb exec-out screencap -p` calls to hang until the Runtime timeout fires.

Preferred configuration order:

1. Set `ACTINGCOMMAND_ADB_PATH` to the matching MuMu adb path when the host has multiple adb versions.
2. Or set `ACTINGCOMMAND_NEMU_FOLDER` to the MuMu folder so Runtime discovery can find `nx_main\adb.exe`.
3. Or configure `actinglab config set adb_path <matching-adb-path>` as a host-local option.

ActingCommand intentionally does not fall back to a bare `adb` on `PATH` for Runtime device operations.
