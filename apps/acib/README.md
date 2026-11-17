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
this version: a device whose pairing record usbmuxd does not hold answers `not_paired`.

It never prints, logs or writes a device's serial number, UDID, name or address, nor anything of
a pairing record (the record is asked for, kept in memory for the command and never shown). It
installs no logger. A library error whose text could carry such a value is shown by its kind
alone; a remote service's name that could carry one is shown as `withheld`.

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
  handshake, and its answer: the services' names, sorted, and their `count`. No service is
  opened.
- `acib images --device <n>`: in a session, the image mounter's lookup (read only): `mounted`,
  and the image type found (`Personalized`, then `Developer`, is asked for) as `kind`, `null`
  when none is mounted.

The problem words: `command_line_invalid`, `async_runtime_failed`, `deadline_passed`,
`usbmuxd_unreachable`, `usbmuxd_list_failed`, `device_not_listed`, `lockdown_unreachable`,
`lockdown_read_failed`, `lockdown_value_unexpected`, `pair_record_unreadable`, `not_paired`,
`session_refused`, `service_refused`, `service_unreachable`, `tunnel_failed`,
`rsd_unreachable`, `rsd_handshake_failed`, `image_lookup_failed`.

Not in this version: pairing, mounting an image, a screenshot, any input, starting or stopping an
application, a daemon mode.

## Clean room

`acib` is built on the MIT-licensed Rust library `idevice` (pinned to one exact version in
`Cargo.toml`), which the owner allowed as a dependency (board #479), and on public
documentation. No code of another project is in it, and no build of another project is run by
it or for it.
