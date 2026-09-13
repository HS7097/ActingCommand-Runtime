# Windows Runtime distribution candidate

Status: unreleased candidate. The exact source commit, tree, Cargo.lock hash,
Rust toolchain, target, build profile, Actions run/attempt and file identities
are recorded in the accompanying `BUILD-MANIFEST.json`. The artifact name binds
the complete source SHA. These notes do not assign a release version or tag.

The Windows `x86_64-pc-windows-msvc` release-profile Runtime artifact includes the
resident `actingcommand-actingd.exe`, the `actingctl.exe` command client, an
editable configuration template, `INSTALL.md` and these notes. The manifest's
`runtime_payload_layout` is `distribution-v1`; all five payload files are required
and individually bound by size and SHA-256.

The existing exact-artifact downloader understands this layout and the historical
fixed two-executable Runtime layout whose manifest omits the field. It continues
to reject incomplete or unexpected payloads. The separate Tools artifact retains
`actinglab.exe`, `actingledger.exe`, `actingcommand-vision-provider-check.exe`,
`actingcommand-device-test.exe`, `ac_fastdeploy_ppocr.dll` and its own manifest.

Configuration uses `actingcommand.actingd.config.v1` and the existing
`actingcommand-actingd --config <path>` entry. The supplied template has empty
private state-root and salt values and no device instances. Follow `INSTALL.md`
and the exact source schema to provide a private configuration and any required
provider dependencies before use.

This distribution change adds packaging and documentation to the existing
Actions build. It does not change daemon/client operation, install dependencies
or services, or perform startup, state migration or device actions. Actions
results establish only their recorded build and check outcomes; installation,
provider availability, real-device behavior and functional acceptance require
their own evidence. Features from other unaccepted candidates are not represented
by these notes. Release version, tag, publication channel and final sign-off
remain separate decisions.
