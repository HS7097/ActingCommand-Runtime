# Windows Runtime distribution

Version: the tag (`vX.Y.Z`) of the `HS7097/ActingCommand-Runtime` Release that
carries this zip; download the zip and `SHA256SUMS` from that Release. The
version exists only in the Release tag, not in the binaries or the manifest.
The same files taken from a pull-request or manually started Actions build are
an unreleased acceptance build with no version. The exact source commit, tree,
Cargo.lock hash, Rust toolchain, target, build profile, Actions run/attempt and
file identities are recorded in the accompanying `BUILD-MANIFEST.json`; the zip
and artifact names bind the complete source SHA.

The Windows `x86_64-pc-windows-msvc` release-profile Runtime artifact includes the
resident `actingcommand-actingd.exe`, the `actingctl.exe` command client, an
editable configuration template, `INSTALL.md` and these notes. The manifest's
`runtime_payload_layout` is `distribution-v1`; all five payload files are required
and individually bound by size and SHA-256.

The existing exact-artifact downloader understands this layout and the historical
fixed two-executable Runtime layout whose manifest omits the field. It continues
to reject incomplete or unexpected payloads. The separate Tools artifact carries
`actinglab.exe`, `actingledger.exe`, `actingcommand-vision-provider-check.exe`,
the watchdog launcher `actingwatch.exe` and,
under `platform-tools/`, the official Android platform-tools 37.0.1 files
`adb.exe`, `AdbWinApi.dll`, `AdbWinUsbApi.dll`, `NOTICE.txt` and
`source.properties`. Its own manifest declares
`tools_payload_layout: "platform-tools-v3"` and binds all nine files.
The OCR and NN engine is linked into `actingcommand-actingd.exe`; no vision
provider DLL ships (Workflow #360), and actingd reads its models from the vision
root named by the configuration's `vision` section. The downloader still accepts
`platform-tools-v3` Tools artifacts built before v0.11.6 (the same files plus
`actingcommand-device-test.exe`), the historical `platform-tools-v2` layout (those
ten files without `actingwatch.exe`), the historical `platform-tools-v1` layout (the
`platform-tools-v2` files plus `ac_fastdeploy_ppocr.dll`) and the five-file Tools
layout whose manifest omits the field, and rejects any other layout.

The device-test probe `actingcommand-device-test.exe` is retired (v0.11.6), and the
Tools artifact no longer contains it.

`actingctl watchdog status` / `run-once` and the launcher `actingwatch.exe`
(Workflow #374) start an A/B installation's Runtime again after an end without a
formal close, and stay down after a FATAL; see `INSTALL.md`, "Runtime watchdog".

The build takes `platform-tools_r37.0.1-win.zip` only from Google's official
`https://dl.google.com/android/repository/` URL and fails unless the archive has
the size and SHA-1 that Google publishes, the pinned SHA-256, and each shipped
file its pinned size and SHA-256, and unless `adb.exe version` reports
`37.0.1-15733141`. Of the archive's binaries only `adb.exe` and its two DLLs are
shipped; Google's `NOTICE.txt` and `source.properties` stay unchanged beside them.
They are redistributed by the owner's decision under the Android SDK License and
the open-source licenses in `NOTICE.txt`. Installed, they are in
`<install root>\tools\platform-tools\`, and
`<install root>\tools\platform-tools\adb.exe` is the adb that an instance without
`adb_path` (key omitted) uses.

Real-device screen stream (#402 RD-3): `actingcommand-actingd.exe` carries the
official scrcpy server 5.0.1 (`scrcpy-server-v5.0.1`, 733930 bytes, SHA-256
`764eb6f79811d5211fe9df341120882ba9994c7a61b897d7bf3fb662e53bc536`), which the
build takes only from the scrcpy GitHub release and checks against its pin, and
OpenH264 compiled from source (Cisco's OpenH264 bundled by the `openh264-sys2`
0.9.8 crate). The capture backend `scrcpy` pushes that server to a real device's
`/data/local/tmp`, starts it without its control and audio channels and without
any option that changes a device setting, and decodes its H.264 stream; it is never
chosen automatically and is refused on emulator instances
(`device_config_invalid`).

The scrcpy server: Copyright (C) 2018 Genymobile, Copyright (C) 2018-2026 Romain
Vimont, licensed under the Apache License, Version 2.0
(http://www.apache.org/licenses/LICENSE-2.0); distributed on an "AS IS" basis,
without warranties or conditions of any kind. OpenH264: Copyright (c) 2013, Cisco
Systems, all rights reserved; the `openh264` and `openh264-sys2` crates: Ralf
Biedert and contributors. Both are distributed under this BSD-2-Clause licence:
Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met: redistributions of
source code must retain the above copyright notice, this list of conditions and
the following disclaimer; redistributions in binary form must reproduce the above
copyright notice, this list of conditions and the following disclaimer in the
documentation and/or other materials provided with the distribution. THIS SOFTWARE
IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR
IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT
SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT,
INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED
TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR
BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN
ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH
DAMAGE.

The daemon recognises an install root from its own path: any directory named
`runtime` that holds this artifact with its `BUILD-MANIFEST.json` makes its
parent one (an acsetup install, acsetup's upgrade staging, or a hand layout). There an instance without `adb_path`, explicit or discovery-bound,
uses that adb; startup and `check-config` compare the SHA-256 of `adb.exe`,
`AdbWinApi.dll` and `AdbWinUsbApi.dll` with the build's pin before the ledger
opens and refuse with `adb_install_missing` or `adb_install_mismatch`, with no
fallback; a discovery-bound `adb_path` may name the discovered MuMu adb or the
install root's adb. Outside an install root behaviour is unchanged.
`check-config` reports the adb as `adb_default`. See `INSTALL.md`, "Bundled adb".

Before rolling back to v0.9.0, give every explicit instance without `adb_path`
an adb that still exists afterwards (MuMu's own or a separate 37.0.1 copy, not
the install root's), and remove an `adb_path` that names the install root's adb
from discovery-bound instances; otherwise v0.9.0 refuses with
`instance_config_invalid` or `instance_discovery_conflict`. When `ui\` is the
working directory of a running adb server, v0.9.0's acsetup reports that the
console must be closed first although it is closed; stop the adb server and
retry (this disconnects other tools sharing port 5037), or roll back by hand
(see `INSTALL.md`, "Upgrade boundary").

Upgrading in place from v0.11.1, v0.11.2 or v0.11.3 while the Runtime runs
(Workflow #376): the old daemon's policy monitor fails when acsetup's drain
closes admission, before `commit_shutdown` is read, so the installer can end with
a connection reset and an unchanged selection. This is likely when the Runtime is
idle and certain while it is running work. For these upgrades, first close the
old Runtime formally (`actingctl request-shutdown --state-root <state-root>
--wait 60`; if it is refused as busy, retry once it is idle), then run acsetup,
which takes the cold ledger gate, then start the Runtime. That cold gate runs
the old version's `ledger-maintenance verify`, which reads every frame twice in
two passes under a 120 s deadline and refuses ledgers above 200,000 events: on a
large state root (for example 130k events and 30 GB of frames) it fails with
`ledger_read_budget_exceeded`, and the upgrade stops with the selection
unchanged. Such a root cannot be upgraded through either path of the old
version; check its verify time on a copy before upgrading. From this release on
the resident daemon waits through a drain, resumes after an abort or the drain
timeout, and ends with the accepted `commit_shutdown`. A drain that work in
flight holds past its timeout (60 s by default) still stops the installer, and
the Runtime keeps running.

Ledger verification and the start (Workflow #375): from this release on,
`ledger-maintenance verify` and the ledger open at start verify each distinct
frame once, on up to eight workers, in one pass bounded by the authenticated
head, with no event-count cap; a frame deleted with an authenticated retention
proof is accepted. The daemon prints one stdout line after the open, also
before a failure: `actingd ledger_open events=… artifacts=… artifact_bytes=…
workers=… sql_read_ms=… verify_ms=… material_ms=… restore_ms=…
deadline_ms=120000`. Follow its timings as the state root grows: the open
still has a 120 s deadline, and acsetup waits 60 s for a Runtime it starts.

Kept frames (Workflow #375): this release moves the screenshots it keeps for
errors and for Lab into `<state root>\kept\<date>\…`. Delete a folder to delete
them, with Shift+Delete or `Remove-Item -Recurse`; Explorer's Delete keeps them in
the Recycle Bin. Other screenshots are deleted automatically: near-duplicates once
their run has ended, resource readings after 7 days, the rest after a day.
Near-duplicates inside the 30 s before an error are also deleted, and Lab
screenshots are all kept; to change either, add `"frame_retention_dedup_error":
false` or `"frame_retention_dedup_lab": true` to `actingd.config.json`, commit it
with `acsetup --commit-config` and restart actingd. A change applies only to
screenshots not yet moved into a folder. The cleaner runs while
`frame_retention_enabled` is on (the default), and prints one
`actingd frame_retention pass …` line per sweep that visited frames; with it off,
actingd prints `actingd frame_retention disabled` once at start. Keep `kept`
a plain folder on the state root's volume: a junction there makes reads of
kept screenshots fail with `artifact_kept_walk_failed`, and moving a leaf out
by hand counts as deleting it. Once a screenshot has been deleted or moved, or
once the file names either switch, v0.11.4 can no longer open this state root.

check-config's catalog preview (Workflow #375) now reads only the catalog and
approval records (authenticated, with the ledger head and contiguity) instead of
every ledger record; startup and `ledger-maintenance verify` still check every
record. It no longer runs the whole-file `PRAGMA quick_check(1)` when it opens
the database, so its cost no longer grows with the size of
`runtime-state.sqlite` (it was about 13 s at 550 MB on a file not in the cache).
Startup and `ledger-maintenance verify` still run that check.

Rollback: an explicit `acsetup --rollback` runs the target slot's own cold
ledger gate. Rolling back from this release to v0.11.3 or earlier therefore
runs the old verify described above, and on a large state root it fails, which
leaves the installation unchanged. Treat the upgrade to this release as one-way
on such roots.

Configuration uses `actingcommand.actingd.config.v1` and the existing
`actingcommand-actingd --config <path>` entry. The supplied template has empty
private state-root and salt values and no device instances; once `state_root`
and `secret_fingerprint_salt` are filled, the template's `"instances": []` starts
a control-plane-only daemon, and instances are added by editing the
configuration and restarting. Follow `INSTALL.md`
and the exact source schema to provide a private configuration and any required
provider dependencies before use.

This distribution change adds packaging and documentation to the existing
Actions build. Apart from the adb default above, it does not change
daemon/client operation, install dependencies
or services, or perform startup, state migration or device actions. Actions
results establish only their recorded build and check outcomes; installation,
provider availability, real-device behavior and functional acceptance require
their own evidence. Features from other unaccepted candidates are not represented
by these notes. Release version, tag, publication channel and final sign-off
remain separate decisions.
