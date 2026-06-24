# External Tools Notice

This repository does not commit DroidCast_raw APK files or MuMu/Nemu IPC DLL files.

The Rust capture backend can discover optional local tools through environment variables:

- `ACTINGCOMMAND_DROIDCAST_RAW_APK`: local path to a reviewed DroidCast_raw APK.
- `ACTINGCOMMAND_NEMU_FOLDER`: local MuMu Player folder.
- `ACTINGCOMMAND_NEMU_IPC_DLL`: local path to `external_renderer_ipc.dll`.

These files are host-local runtime tools. Keep their license review, source location, and version evidence outside the committed binary path unless a later milestone explicitly approves vendoring.
