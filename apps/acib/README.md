# acib, the ActingCommand iOS Bridge

`acib` (in Chinese 「iOS 调试桥」) is the project's own bridge to iOS devices (board #479). It is a
program of its own: the Runtime will speak to it as a separate process, as it speaks to adb.
Nothing of the workspace depends on it.

## This first version (ACIB-0)

It only reads. Each run is one command; it prints **one JSON line** on standard output and ends
with status 0 when the command was answered, or 1 when it was refused or failed. A refused or
failed line names a `problem` word of this program (the words are its own, not Runtime outcome
codes) and, where the library gave one, its error text in `error`.

usbmuxd is reached where the library finds it on this platform: on Windows the TCP port 27015
of 127.0.0.1, which the device service of Apple's own software serves. Pairing is not part of
this version: a device whose pairing record usbmuxd does not hold answers `not_paired` (to
`services`, `images` and `apps`, and to the second half of `info`).

It never prints, logs or writes a device's serial number, UDID, name or address, nor anything of
a pairing record (the record is asked for, kept in memory for the command and never shown). It
installs no logger. A library error whose text could carry such a value is shown by its kind
alone; a remote service's name or entitlement, or a member name of the image mounter's listing,
that could carry one is shown as `withheld`. Which applications a person has installed is
theirs: `apps` names only the ones whose bundle identifier holds the text asked for, and none
when they are more than its bound or more than half of the user's applications.

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

The problem words: `command_line_invalid`, `async_runtime_failed`, `deadline_passed`,
`usbmuxd_unreachable`, `usbmuxd_list_failed`, `device_not_listed`, `lockdown_unreachable`,
`lockdown_read_failed`, `lockdown_value_unexpected`, `pair_record_unreadable`, `not_paired`,
`session_refused`, `service_refused`, `service_unreachable`, `tunnel_failed`,
`rsd_unreachable`, `rsd_handshake_failed`, `apps_lookup_failed`, `too_many_matching`.

Not in this version: pairing, mounting an image, a screenshot, any input, starting or stopping an
application, a daemon mode.

## Clean room

`acib` is built on the MIT-licensed Rust library `idevice` (pinned to one exact version in
`Cargo.toml`), which the owner allowed as a dependency (board #479), and on public
documentation. No code of another project is in it, and no build of another project is run by
it or for it.
