# acib, the ActingCommand iOS Bridge

`acib` (in Chinese 「iOS 调试桥」) is the project's own bridge to iOS devices (board #479). It is a
program of its own: the Runtime will speak to it as a separate process, as it speaks to adb.
Nothing of the workspace depends on it.

## This version (ACIB-0, and ACIB-1 in 0.12.73)

It only reads. Each run is one command; it prints **one JSON line** on standard output and ends
with status 0 when the command was answered, or 1 when it was refused or failed. A refused or
failed line names a `problem` word of this program (the words are its own, not Runtime outcome
codes) and, where the library gave one, its error text in `error`. The one file it writes is
the picture `screenshot` takes, at the path `--out` names.

usbmuxd is reached where the library finds it on this platform: on Windows the TCP port 27015
of 127.0.0.1, which the device service of Apple's own software serves. Pairing is not part of
this version: a device whose pairing record usbmuxd does not hold answers `not_paired` (to
`services`, `images`, `apps` and `screenshot`, and to the second half of `info`).

It never prints, logs or writes a device's serial number, UDID, name or address, nor anything of
a pairing record (the record is asked for, kept in memory for the command and never shown). It
installs no logger. A library error whose text could carry such a value is shown by its kind
alone; a remote service's name or entitlement, or a member name of the image mounter's listing,
that could carry one is shown as `withheld`. Which applications a person has installed is
theirs: `apps` names only the ones whose bundle identifier holds the text asked for, and none
when they are more than its bound or more than half of the user's applications. What is on the
screen is theirs too: of a picture `screenshot` prints only its format, width and height as its
header gives them and its length; no pixel, no text in it or recognised in it is printed,
logged or kept anywhere but in the file `--out` names.

## The commands

Every command takes `--timeout-ms <ms>` (default 30000); a command not finished by then answers
`deadline_passed`. `<n>` is the device's usbmuxd session number, as `acib devices` lists it.

- `acib devices`: the devices usbmuxd lists now, each with its session number (`device`) and
  its connection (`usb`, `network`, or `unknown` for one usbmuxd names otherwise).
- `acib info --device <n>`: without a session, `product_version`, `product_type`,
  `build_version` and `device_class`; then `paired`, whether usbmuxd holds a pairing record of
  this host for the device. When paired, a session is started with that record and
  `password_protected` and `developer_mode` are read, each `{"value": <bool>}` or
  `{"refused": "<the word the device refused it with>"}`. When not paired, the line keeps the
  first values and answers `not_paired` (status 1); a failure in the second half keeps them too.
- `acib services --device <n>`: in a session, the tunnel of iOS 17.4 and later (the lockdown
  service CoreDeviceProxy, with the library's own TCP stack inside this process: no network
  adapter is created and no administrator right is needed), the remote service discovery
  handshake, and its answer: the services sorted by name, each with its `name`, the `port` it is
  served on inside the tunnel and the `entitlement` the handshake names for it (which tells
  whether it needs the trusted tunnel), and their `count`. No service is opened.
- `acib images --device <n>`: in a session, the image mounter's read-only queries, each in
  `queries` with what it answered: the listing of what is mounted (`copy_devices`: `listed`,
  each entry with the names of its members, never a value), then the lookup of the image types
  `Personalized` and `Developer` (`lookup_image`: `found` or `not_found`; a signature is never
  shown). A query that failed answers `failed` with the library's `error_kind`. `not_found`
  means only that the answer held no image signature: the library's lookup looks for the
  signature alone, so a phone's answer that carries no signature for another reason (one that
  reports a failure otherwise than in an `Error` member, or one of a shape the library does not
  expect) reads `not_found` too; an answer with an `Error` member is a library error, `failed`.
  `mounted` is `true` when the listing holds an entry, or when the listing failed and a lookup
  found an image; `false` only when the listing answered an empty list, no lookup found an image
  and no query failed; and `"unknown"` for everything else: an empty listing beside a lookup
  that found an image is `"unknown"`, because the two answers disagree, and so is an empty
  listing beside a failed query. That an empty listing means nothing is mounted has not
  been checked on a device that has a personalized image mounted. Nothing is mounted and
  nothing uploaded.
- `acib apps --device <n> --contains <text> [--max-matching <m>]`: in a session, the
  installation proxy's list of the user's applications: how many bundle identifiers hold
  `<text>` (at least three characters, compared without regard to case) as `matching_count`, the
  count of all user applications as `user_applications`, and those identifiers, sorted, as
  `matching`. `<m>` (1 to 20, default 5; a larger bound is `command_line_invalid`) is the most
  identifiers named. The identifiers are named only when the matching ones are at most `<m>`
  and at most half of the user's applications (`2 * matching_count <= user_applications`);
  otherwise the line names none (no `matching`), keeps the two counts and answers
  `too_many_matching` (status 1). For example, with the default bound: 2 matching of 4 user
  applications are named; 3 of 4, 6 of 40, or the one user application of a device matching,
  are not. So neither the whole list nor most of it is ever printed, and nothing else of an
  application's record (no display name, path, version or signer).
- `acib screenshot --device <n> --out <file> [--via <way>]`: in a session, through the tunnel of
  `services` and the remote service discovery handshake, one picture of the screen as the
  device gives it, written byte for byte (not re-encoded) to `<file>`. `<file>` must not exist:
  a path that names anything already is refused with `out_exists` before the device is asked.
  The line names the way used as `via`, the picture's `format` (`png` or `jpeg`), `width` and
  `height` as the picture's own header gives them (read by a few lines of this program's own,
  with no image library), its length in `bytes`, `elapsed_ms` with `request` (the screenshot
  request alone) and `command` (the whole command), and `out`. A picture whose header ends too
  early or is of no format read here keeps its file and its length and answers
  `picture_header_truncated` or `picture_header_unknown` (status 1). A way the device refuses
  answers its word (`service_not_listed`, `service_unreachable`, `service_handshake_failed`,
  `channel_refused`, `screenshot_failed`, and those of the session and the tunnel) and writes
  no file. The ways, by `--via` (default `screen_capture`):
  - `screen_capture`: CoreDevice's screen capture service
    (`com.apple.coredevice.screencaptureservice`), asked for a PNG of the main display in one
    request.
  - `instruments`: the instruments server (`com.apple.instruments.dtservicehub`) and its
    screenshot channel; the picture is in the format the device gives.

The problem words: `command_line_invalid`, `async_runtime_failed`, `deadline_passed`,
`usbmuxd_unreachable`, `usbmuxd_list_failed`, `device_not_listed`, `lockdown_unreachable`,
`lockdown_read_failed`, `lockdown_value_unexpected`, `pair_record_unreadable`, `not_paired`,
`session_refused`, `service_refused`, `service_unreachable`, `tunnel_failed`,
`rsd_unreachable`, `rsd_handshake_failed`, `apps_lookup_failed`, `too_many_matching`,
`out_exists`, `out_unwritable`, `service_not_listed`, `service_handshake_failed`,
`channel_refused`, `screenshot_failed`, `picture_header_truncated`, `picture_header_unknown`.

Not in this version: pairing, mounting an image, any input, starting or stopping an
application, a stream of pictures, a daemon mode, and which application is in front: the
library offers no way to ask it (its process lists carry no mark of the application in front
and no bundle identifier, and its application state notifications tell only what changes after
they are turned on), so there is no command for it.

## Clean room

`acib` is built on the MIT-licensed Rust library `idevice` (pinned to one exact version in
`Cargo.toml`), which the owner allowed as a dependency (board #479), and on public
documentation. No code of another project is in it, and no build of another project is run by
it or for it.
