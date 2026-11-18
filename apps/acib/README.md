# acib, the ActingCommand iOS Bridge

`acib` (in Chinese 「iOS 调试桥」) is the project's own bridge to iOS devices (board #479). It is a
program of its own: the Runtime will speak to it as a separate process, as it speaks to adb.
Nothing of the workspace depends on it.

## This version (ACIB-0, ACIB-1 in 0.12.73, and ACIB-2, ACIB-3, ACIB-3b and ACIB-3c in 0.12.74)

Each run is one command; it prints **one JSON line** on standard output and ends with status 0
when the command was answered, or 1 when it was refused or failed. A refused or failed line
names a `problem` word of this program (the words are its own, not Runtime outcome codes) and,
where the library gave one, its error text in `error` (of `probe`'s ways, and of `running`,
`launch` and `stop` from the installation proxy's lookup on, only the error's kind, in
`error_kind`). The one file it writes is the picture `screenshot` takes, at the
path `--out` names; it removes that file again when the picture could not be written whole into
it. With `--out -` it writes no file: the picture's bytes follow the line on the standard output
(below).

**Two commands change what the phone runs and shows: `launch` and `stop`.** Every other
command only reads. `launch` starts one application, through the instruments server's process
control (whether it then comes to the front the library does not say: to be observed on the
phone); with `--relaunch` it first ends that application if it runs (what was not saved in it is
lost). It is undone by `acib stop` with the same bundle, or by the person on the phone (the home
gesture leaves it; the app switcher ends it); `acib` does not bring back what was in front
before. `stop` has the instruments server kill the application's own processes at once (its
`killPid:`; the library does not name the signal: what was not saved in the application is
lost); it leaves the processes of its extensions (a widget, say) to the system, as a person who
swipes the application away does. It is undone by `acib launch` with the same bundle, which
starts the application anew, not in the state it was in. A `launch` or `stop` whose deadline
passes, or a `launch` answered with any problem (below), may already have changed the phone:
`acib running` tells what runs.

usbmuxd is reached where the library finds it on this platform: on Windows the TCP port 27015
of 127.0.0.1, which the device service of Apple's own software serves. Pairing is not part of
this version: a device whose pairing record usbmuxd does not hold answers `not_paired` (to
`services`, `images`, `apps`, `screenshot`, `orientation`, `running`, `launch`, `stop` and
`probe`, and to the second half of `info`).

It never prints, logs or writes a device's serial number, UDID, name or address, nor anything of
a pairing record (the record is asked for, kept in memory for the command and never shown). A
library error whose text could carry such a value is shown by its kind alone; a remote service's
name or entitlement, or a member name of the image mounter's listing, that could carry one is
shown as `withheld`. Which applications a person has installed is
theirs: `apps` names only the ones whose bundle identifier holds the text asked for, and none
when they are more than its bound or more than half of the user's applications. What is on the
screen is theirs too: of a picture `screenshot` prints only its format, width and height as its
header gives them and its length; no pixel, no text in it or recognised in it is printed,
logged or kept anywhere but in the file `--out` names, or on the standard output after the line
when `--out -` asks for it there. A salt is never printed, logged or written, and neither is
the identifier a key was made from; a key is printed only when its salt's flag was given. The
phone's applications and processes are a person's too: `running`, `launch` and `stop` answer of
them (ACIB-3c: the installation proxy's record of the one bundle, the instruments server's list
of running processes) only the one bundle asked for (as the command line gave it), whether it is
installed and runs, and the pids of its own processes and of its extensions'. No other
application's name, bundle identifier, path or pid is printed, logged or kept: the record and the
list stay in memory for the command, the answer types have no member that could hold one, and an
error from the lookup on is shown by its kind alone, never by its text; of `probe`, an error of
the application service too (the library hands the phone's refusal as the text of its error
dictionary, which can name other applications).

**No log subscriber, ever, without redaction.** `acib` installs no log subscriber and no
logger. The library itself traces, at warning level, the whole error dictionary a phone answers
a CoreDevice request with (idevice 0.1.68, `services/core_device/mod.rs`: "XPC response did not
have an output"), which can name other applications, their paths and their processes, and other
texts of the phone's answers beside it; and (ACIB-3c) its instruments client traces, at debug
level, every message it sends and reads (`services/dvt/remote_server.rs` 727 and 865), the
phone's list of running processes among them; with no subscriber those events go nowhere. A
subscriber or a logger may be added only with a redaction that holds all of the above. A test
fails on any new dependency of `acib` (a tripwire against adding one unnoticed, not a proof that
none of its dependencies could install one), and a comment where `main` would install one says
why.

## Keys: naming a phone across connections

A device's session number (`device`) belongs to one connection: it changes when the phone is
plugged in again, so nothing can be bound to it. A **key** names the phone itself: `acib` makes
it of the identifier usbmuxd lists for the device (the library's `UsbmuxdDevice::udid`, which it
reads from usbmuxd's `SerialNumber` for a USB row and a network row alike) and a salt, as
`sha256(salt ‖ 0x00 ‖ "device_serial" ‖ 0x00 ‖ identifier)`, written `sha256:` and 64
lowercase hexadecimal digits. That is byte for byte the Runtime's own fingerprint of a handset's
serial (the ledger's `Sha256SecretFingerprinter`, the field `device_serial`); one test vector,
asserted in this program's tests and in the ledger's, holds the two together. Under the
installation's secret, a key is the fingerprint the Runtime records for the device. The same
phone under the same salt has the same key at every listing; under another salt it has another
key, and a key holds neither the identifier nor the salt, only their digest.

- **Salts come only from the environment.** A flag names an environment variable of this
  process, never the salt itself: `--key-salt-env <NAME>`, and for `devices` also
  `--binding-salt-env <NAME2>`. A salt is 16 to 1024 bytes of UTF-8, the bounds of the Lab's
  `--key-salt-env`. A variable that is unset, empty, not UTF-8 or outside those bounds answers
  `key_salt_unusable` (`binding_salt_unusable` for the second), before usbmuxd is asked. A
  variable name that is empty or holds `=` is `command_line_invalid`.
- **Two salts, one call.** The installer lists the phones under a salt of its own, and the Lab
  binds a phone under the installation's secret: `devices --key-salt-env <NAME>
  --binding-salt-env <NAME2>` gives each row both keys, `device_key` under the first salt and
  `binding_key` under the second, so a row chosen by its first key can be bound by its second.
- **A phone by its key.** Every command that takes `--device <n>` takes, in its place,
  `--device-key <key> --key-salt-env <NAME>`: the device usbmuxd lists whose key under that
  salt is `<key>`. Rows with one key are rows of one phone, listed with one identifier: of a
  phone listed both over USB and over the network its USB row is taken, whichever of the two
  usbmuxd lists first, and of a phone listed only over the network its network row. (That
  usbmuxd lists a phone's network row with the identifier of its USB row has not been checked
  on a device; where it does not, the two rows have two keys and each key finds its own row
  alone.) The two flags go together; `--device` beside `--device-key`, either of the two
  without the other, or a key not written as `sha256:` and 64 lowercase hexadecimal digits is
  `command_line_invalid`. No listed device with that key answers `device_key_unknown`. The
  line's `device` stays the session number of the connection the device was found on (it
  identifies nobody), and the line gains nothing else.

## The commands

Every command takes `--timeout-ms <ms>` (default 30000); a command not finished by then answers
`deadline_passed` (for `screenshot`, with the `stage` it was waiting in, below; for `running`,
`launch` and `stop`, with the `stage` it was waiting in and `stage_ms`, below; for
`orientation`, per reader; for `probe`, per way, below). `<n>` is the device's usbmuxd session
number, as `acib devices` lists it; wherever `--device <n>` is written below, `--device-key
<key> --key-salt-env <NAME>` may stand in its place.

- `acib devices [--key-salt-env <NAME> [--binding-salt-env <NAME2>]]`: the devices usbmuxd
  lists now, each with its session number (`device`) and its connection (`usb`, `network`, or
  `unknown` for one usbmuxd names otherwise); with `--key-salt-env`, each with its `device_key`
  under that salt, and with `--binding-salt-env` too, its `binding_key` under the second salt
  (`--binding-salt-env` alone is `command_line_invalid`). Without the flags the line is as
  before.
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
  whether it needs the trusted tunnel), and their `count`. No service is opened. With
  `--service <name>` (one service's name, as the list names it: ASCII letters and digits, `.`,
  `-` and `_`) the line names that service alone, in `service`, with what else the handshake
  says of it (idevice 0.1.68, `services/rsd.rs`): `name`, `port`, `entitlement`,
  `uses_remote_xpc` (its properties' `UsesRemoteXPC`; `false` also when they do not say),
  `service_version` (its `ServiceVersion`) and `features` (its `Features`, in the handshake's
  order), each of the last two only when the handshake gives it; a name, an entitlement or a
  feature that could carry an identifier is `withheld`. A name the handshake does not list
  answers `service_not_listed` (status 1). For example `acib services --device 3 --service
  com.apple.coredevice.appservice` tells which features the application service names, among
  them whether it offers the streamed list of applications (`probe`, below).
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
- `acib screenshot --device <n> --out <file|-> [--via <way>]`: in a session, through the tunnel
  of `services` and the remote service discovery handshake, one picture of the screen as the
  device gives it, written byte for byte (not re-encoded) to `<file>`, or with `--out -` to the
  standard output after the line (below; a file named `-` is `./-`). `<file>` must not exist,
  and the directory it is to be made in (the current one for a bare file name) must: a path
  that names anything already is refused with `out_exists`, and one whose directory does not
  exist or is no directory with `out_unwritable`, both before the device is asked.
  The line names the way used as `via`, the picture's `format` (`png` or `jpeg`), `width` and
  `height` as the picture's own header gives them (read by a few lines of this program's own,
  with no image library), its length in `bytes`, `elapsed_ms` with `request` (the screenshot
  request alone) and `command` (the whole command), and `out`. A picture whose header ends too
  early or is of no format read here keeps its file and its length and answers
  `picture_header_truncated` or `picture_header_unknown` (status 1). A way the device refuses
  answers its word (`service_not_listed`, `service_unreachable`, `service_handshake_failed`,
  `channel_refused`, `screenshot_failed`, and those of the session and the tunnel) and writes
  no file. `out_unwritable` is the word of a file that could not be made at all: then nothing
  was written. A picture that could not be written whole into the file made (a full disk, say)
  answers `out_write_failed` with the system's error and `out_left`: the program removes the
  file it made (its own: it was made where nothing was), and `out_left` is `true` only when
  that removal failed too, a file then being left at `<file>` with a part of the picture or
  none of it. A screenshot whose deadline passes names in `stage` the step it was waiting in:
  `session` (usbmuxd's device list, the pairing record and the lockdown session that starts the
  tunnel's service), `tunnel` (the tunnel and the remote service discovery handshake),
  `service_handshake` (the connection to the way's remote service and its own handshake),
  `server_first_message` (the first message of the instruments server, which the `instruments`
  way reads before it asks for anything) and `picture_request` (the picture's request; for
  `instruments`, that of its screenshot channel and then of the picture). No deadline passes
  while the file is written. The ways, by `--via` (default `screen_capture`):
  - `screen_capture`: CoreDevice's screen capture service
    (`com.apple.coredevice.screencaptureservice`), asked for a PNG of the main display in one
    request.
  - `instruments`: the instruments server (`com.apple.instruments.dtservicehub`) and its
    screenshot channel; the picture is in the format the device gives.

  **`--out -`, the picture on the standard output.** Nothing is written to disk. The standard
  output holds the line the command prints for a file (with `"out": "-"`), one line feed
  (`0x0A`), then exactly `bytes` raw bytes of the picture as the service gave them, and nothing
  after them. Bytes follow exactly the line of a screenshot (a line that names `bytes`; one
  whose header could not be read too, with its `picture_header_*` word and status 1). A way the
  device refuses prints its problem line and no byte, and so does every other refusal. A reader
  reads one line, then `bytes` bytes. The standard output is written as bytes: Rust's standard
  library hands them to the system as they are (on Windows to `WriteFile` on the handle, with no
  C runtime text mode in between), so no line end is translated. A console refuses bytes that
  are not UTF-8, which fails the run (status 1): redirect the output to a file or a pipe of
  bytes (a shell that re-encodes a native program's output as text, as Windows PowerShell 5.1
  does, corrupts it). A picture whose bytes cannot be written whole (a closed pipe) ends with
  status 1. `elapsed_ms.command` is taken before the bytes are written; no deadline passes
  while they are.
- `acib orientation --device <n> [--via <reader>]`: the orientation of the interface as the
  phone reports it, read by the library's SpringBoard services client, whose one read
  (`getInterfaceOrientation`) is all that is sent: nothing sets, locks or turns an orientation.
  The device and its pairing record are found once (`not_paired` refuses the whole command);
  then each reader asked answers in `candidates`, in the order asked: `{"via": <reader>,
  "orientation": <word>}`, or `{"via": <reader>, "problem": <word>}` with the library's `error`
  where it gave one. The command is answered (status 0) only when every reader asked answered an
  orientation. A reader that has not answered when the deadline passes answers
  `deadline_passed`, and so does each reader after it, which is then not asked; the readers that
  answered before keep their answers (a deadline that passes before the first reader is asked
  answers `deadline_passed` for the whole command or for each reader). The orientation words,
  one for each value the library names (its `InterfaceOrientation`, the phone's numbers 0 to
  4): `unknown` (0, or a value the library does not name: "unknown or cannot be determined"),
  `portrait` (1, the normal vertical way), `portrait_upside_down` (2), `landscape_right` (3,
  the home button on the right, the notch to the left) and `landscape_left` (4, the home button
  on the left, the notch to the right). The readers, by `--via` (without it, both, in this
  order):
  - `springboard`: the lockdown service `com.apple.springboardservices`, started in a session
    with this host's pairing record, as `images` and `apps` start theirs; no tunnel.
  - `springboard_shim`: the same service's remote shim
    (`com.apple.springboardservices.shim.remote`), inside the tunnel of `services`, as the
    remote service discovery handshake lists it (`service_not_listed` when it does not); its
    check-in failing is `service_handshake_failed`.

  A reader the phone refuses answers its word (`service_refused`, `service_unreachable`,
  `service_not_listed`, `service_handshake_failed`, `orientation_failed` and those of the
  session and the tunnel). The library's other orientation service, CoreDevice's
  `com.apple.coredevice.devicecontrol`, only rotates the device (and answers the orientation
  after the turn), so it is not built; neither is WebDriverAgent's orientation, which would run
  another project's program on the phone.

### One application: `running`, `launch`, `stop`

These three take `--bundle <id>`: one bundle identifier, written in the characters Apple allows
in one (ASCII letters and digits, `-` and `.`; anything else is `command_line_invalid`). Bundle
identifiers are compared without regard to case, as Apple's documentation says they are. Each
line names `bundle` as the command line gave it.

**Which way they take, and why (ACIB-3c).** On a phone with iOS 27.0.1 (2026-11-19, read-only
requests only), CoreDevice's application service (`com.apple.coredevice.appservice`) was offered,
named fifteen features, took a connection and its handshake, and answered none of seven read-only
requests within 8 s each (the library's two forms of its list of applications, its list of
processes, the streamed list, a mere listen, and the two lists in the form of the screenshot's
request: `probe`, below), while in the same minutes the installation proxy answered `apps` and the
instruments server (`com.apple.instruments.dtservicehub`) answered `screenshot --via instruments`
at once. Why the application service answers nothing there was not found. So these three ask it
nothing: each looks the one bundle up in the installation proxy, and then asks the instruments
server, inside the tunnel of `services`:

- `running`: for its list of running processes (its device information channel), read-only;
- `launch`: to launch the application (its process control channel);
- `stop`: for its list of running processes, to kill each of the application's own processes (its
  process control channel), and for the list again.

The application service is asked by `probe` alone (below), for other systems and later versions.
What these three send, and how their answers are read, was derived from the library's own source
(idevice 0.1.68) and Apple's public documentation, and from nothing else.

**The lookup.** In a session with this host's pairing record, the installation proxy
(`com.apple.mobile.installation_proxy`, started as `apps` starts it) is asked for that one bundle,
of every application type: `Lookup` with `ApplicationType` `Any` (the library's default, "for all
applications") and `BundleIDs` the one bundle (idevice 0.1.68, `services/installation_proxy.rs`
62-112). Of its answer only each entry's bundle identifier and its record's `Path` are read (the
member the library's own reader of such a record names the "On-device path" of the application's
bundle: `services/dvt/xctest/mod.rs` 106 and 184); nothing else of a record is read. A bundle the
answer does not hold is not installed: `running` answers `installed` false and `running` false
(status 0); `stop` answers `installed` false and `was_running` false (status 0) and sends nothing;
`launch` answers `installed` false with `not_installed` (status 1) and sends nothing; and none of
the three builds the tunnel. So `installed` false, and the word `not_installed`, have one cause:
the installation proxy's lookup of the bundle, as the command line wrote it, does not hold it
(whether the phone finds a bundle written in another case than its own has not been seen). An
entry of the bundle whose record has no `Path`, or one read as no place, is
`bundle_path_unreadable` for `running` and `stop`; `launch` needs no path and goes on.

**The instruments server.** In the tunnel of `services`, a connection to the port the discovery
handshake names for the instruments server, and its first message read (the capabilities it
publishes, read before anything is asked of it, as a screenshot's `instruments` way reads it).
Then each request on a channel of its own, opened by the library's request
`_requestChannelWithCode:identifier:` with a channel number it counts and the channel's name
(`services/dvt/remote_server.rs` 425-458; an answer to it that holds anything, the library takes
as no channel):

- **The list of running processes** (`running`; `stop`'s looks), read-only: the channel
  `com.apple.instruments.server.services.deviceinfo` and on it the request `runningProcesses`,
  with no argument, its answer asked for and read on the channel. That is what the library's
  `DeviceInfoClient::running_processes` sends (`services/dvt/device_info.rs` 26-46); that client
  is under a feature of the library `acib` does not turn on, so `acib` sends the same through the
  library's public channel of the `dvt` feature it has (`Channel::call_method` and
  `read_message`, `services/dvt/remote_server.rs` 1298-1355): no feature and no crate is added.
  The answer is a list of entries; of each, `acib` reads its `pid` and its `realAppName`, as the
  location of its executable. The library's reader names these two beside `name`, `isApplication`
  and `startPageCount`, which are not read, and documents none of them: that `realAppName` is the
  path of the executable is read from no description, and is to be seen at the first `running`
  on a phone while the application is open. An answer that is no list, or one entry that is no
  dictionary or has no pid from 0 to 4294967295, is `answer_unreadable` for the whole list (a
  process whose pid cannot be read could be the bundle's); an entry without a `realAppName` text
  is of no application.
- **The launch** (`launch`): the channel `com.apple.instruments.server.services.processcontrol` and
  on it the library's `launch_app` (`services/dvt/process_control.rs` 120-218, below).
- **The kill** (`stop`): the same process control channel and on it the library's `kill_app`, once
  per process of the application (`services/dvt/process_control.rs` 275-296, below).

**Which processes are of a bundle.** The installation proxy's record gives the bundle's path, a
directory; the instruments server's list gives each process its pid and its `realAppName`, and no
bundle identifier. A process is of the bundle when its executable lies inside the bundle's
directory, and the two kinds are told apart (Apple's "Placing content in a bundle": on iOS the
main executable is at the top of the bundle, app extensions in `PlugIns/`). **The application
itself** is a process whose executable lies directly in the directory: only it counts for
`running`, and only it is ended by `stop`. A process whose executable lies deeper (an app
extension in `PlugIns/`, a helper inside a nested bundle) is the bundle's but is not the
application running: a widget the system runs for it does not make the application run. Its pid
is named apart, in `extension_pids`. A process's `name` is never read, so a process of another
application with the same name is not the bundle's. Paths are compared exactly, case included
(iOS's file system is case-sensitive); a location given as a `file://` URL is read as its
percent-decoded path, `/private/var/…` as `/var/…`, and one that is no absolute path is of no
application. A process of another application whose path merely starts like the bundle's
directory (a longer name, a sibling directory) is not of it.

- `acib running --device <n> --bundle <id>`, read-only: whether the lookup holds the bundle
  (`installed`), whether the application itself runs (`running`), the pids of its own processes,
  ascending (`pids`), and the pids of its extensions' processes, ascending (`extension_pids`,
  which do not make it run): for example
  `{"command":"running","device":3,"bundle":"com.example.app","installed":true,"running":true,
  "pids":[412],"extension_pids":[530]}`. Only an extension alive answers `running` false, `pids`
  empty and its pid in `extension_pids`. A bundle the lookup does not hold answers `installed`
  false, `running` false and no pid, status 0 (above). Its words: `application_lookup_failed`,
  `service_handshake_failed` (the instruments server's first message) and `process_list_failed`
  (each with `error_kind`; a line stopped after the lookup keeps `installed`), and
  `bundle_path_unreadable`. Nothing on the phone changes: the process control channel is not
  opened. That the record's path and the list's `realAppName` meet as described here (and so
  that a running application answers `running` true) is to be seen at the first `running` on a
  phone while the application is open.
- `acib launch --device <n> --bundle <id> [--relaunch]`: first the lookup, as `running` and `stop`
  ask it: a bundle it does not hold answers `installed` false, as they do, with `not_installed`
  (status 1), and no launch is sent. Then the instruments server's process control launches the
  application and the line answers `relaunch`, `installed` true and the pid it answered (`pid`).
  Without `--relaunch` an application that already runs is not ended first (`KillExisting`
  false): what the phone then does (bring it to the front, start a second one, refuse) the
  library does not say, and the line answers what the server answered; it is to be observed on
  the phone. With `--relaunch` it is ended and started anew. It changes what the phone runs and
  shows (above).

  **What `launch` sends.** The channel's request (above), then the library's `launch_app`
  (idevice 0.1.68, `services/dvt/process_control.rs` 156-218): the method
  `launchSuspendedProcessWithDevicePath:bundleIdentifier:environment:arguments:options:` (the
  library fixes it), its answer asked for, with these five arguments, each an archived value, and
  nothing else:
  - the device path: an empty text (the library fixes it);
  - the bundle identifier: the bundle, as `--bundle` gives it (`acib` chooses);
  - the environment: an empty dictionary (`acib` passes none);
  - the arguments: an empty list (`acib` passes none);
  - the options: `StartSuspendedKey` `false` (`acib` chooses: the application is not started
    suspended) and `KillExisting` `true` with `--relaunch`, else `false` (`acib` chooses).

  The answer the library reads is an integer, the pid. What the library's source does not show,
  and the first run on a phone observes: whether the application comes to the front; what
  `KillExisting` false does to an application that runs; whether the application stays once
  `acib` has exited and the connection is closed (the application still running some seconds
  after `launch` has answered and `acib` has ended); whether the server then sends anything of
  the application on the channel (`acib` reads nothing more from it, and nothing of the
  application's standard input, output or error is connected to `acib`: the request names
  none).

  **What a problem means.** `launch_failed` comes with `answer_unreadable` (the server answered
  with something other than a pid: the library tells a refusal of the launch from no other such
  answer), `connection_failed` or `other`. None of them says that nothing was started: the
  request may have reached the phone. After any `launch` problem, and after a `launch` whose
  deadline passed, `acib running` tells what is true.
- `acib stop --device <n> --bundle <id>`: the application's own processes the first look lists
  are each sent a kill request, one after the other: the library's `kill_app`, the method
  `killPid:` with the pid as a 32-bit number, no answer asked for (`services/dvt/process_control.rs`
  275-296: it "doesn't wait for confirmation"); the library does not name the signal the server
  sends. Its extensions' processes are sent nothing: `stop` ends the application, not its
  extensions, as a person who swipes the application away does, and names the extensions' pids
  it left, which is no failure. Then the list is read again at once, on a device information
  channel of its own, and again every 200 ms while it still lists a process the command
  signalled and the next look starts before the command's deadline (a look the deadline cuts
  leaves the one before it as the last). The line answers `installed`, whether the application
  ran at the first look (`was_running`), the pids a kill request was made for (`signalled`), the
  pids of the application the last look lists (`remaining`), whether any runs (`running`) and the
  pids of its extensions the last look lists (`extension_pids`):
  `{"command":"stop","device":3,"bundle":"com.example.app","installed":true,"was_running":true,
  "signalled":[412],"remaining":[],"running":false,"extension_pids":[530]}`. An application
  that does not run (or that the lookup does not hold, above) is a success that says so:
  `was_running` false, nothing signalled, nothing remaining, its extensions' pids named (none
  when the lookup does not hold it). **The verdict comes from the looks, never from the
  requests**: when the last look inside the deadline still lists a process the command
  signalled, the line answers `still_running` (status 1); when it lists none of them, the
  application is stopped (status 0). A kill request that failed (the process control channel not
  opened, or the request not written) is a fact beside the verdict: its pid and the error's kind
  are in `signal_errors` (only when there are any), whether its process is gone (stopped) or
  still listed (`still_running`). A process of the application the last look lists that the
  command did not signal (one started after the first look) is named in `remaining` and
  `running`, and is not `still_running`. A look after the kills failing answers
  `process_list_failed` and keeps `signalled`; a deadline that passes before the first look after
  the kills has answered is `deadline_passed`. An application that caught the server's signal
  and stayed would be seen so, in the looks. It changes what the phone runs (above).

**Errors.** The steps before the lookup and between it and the instruments server (the device,
the pairing record, the lockdown sessions, the tunnel, the instruments server listed in the
handshake and its port) fail with their own words, as every command's. From the lookup on, an
error is shown by `error_kind` alone, one of: `answer_unreadable` (what the phone answered is not
in the shape read: the lookup's answer without its result; of the instruments server, an answer to
a channel's request that holds anything, an answer to the launch that holds no pid, or a list of
processes not of the shape above), `connection_failed` (the library reported the connection under
the exchange as failed; of the instruments server's connection that is also what it reports once
its reader of the connection has stopped, whatever stopped it: the phone ending the connection, a
read failing, or a message the phone sent that it could not read, which it tells none apart:
`services/dvt/remote_server.rs` 862-884, 1245-1250) and `other` (any other error of the library).
`device_refused` and `xpc_failed` are kinds of the application service's requests, which only
`probe` asks. A locked phone has no word of its own: the library maps no refusal to one, and none
is guessed here; what a locked phone answers is to be observed on the phone.

**Where a command waits (ACIB-3b; ACIB-3c: the lookup and the instruments server).** Each of the
three goes through stages, in the order its code takes the steps, and a line names them by these
words:

- `usbmuxd`: usbmuxd's device list and the pairing record usbmuxd holds for the device.
- `application_lookup`: a lockdown session with that record, the installation proxy started in
  it, and its lookup of the one bundle.
- `proxy_tunnel`: the lockdown session that starts the tunnel's service, CoreDeviceProxy, then
  CoreDeviceProxy's own exchange on that connection (the tunnel's parameters), the TCP stack
  inside this process over it, and the connection inside the tunnel to the remote service
  discovery port.
- `discovery_handshake`: the remote service discovery handshake, which answers the services and
  their ports.
- `service_connection`: the connection inside the tunnel to the port the handshake names for the
  instruments server, `com.apple.instruments.dtservicehub`.
- `server_first_message`: the instruments server's first message (the same step, and the same
  word, as a screenshot's `instruments` way).
- `process_list`: the device information channel's request and the list of running processes
  (for `stop`, the first look, before any kill).
- `launch_request`: the process control channel's request, the launch request and its answer.
- `kill_requests`: the process control channel's request and one kill request per process of the
  application (no answer is waited for).
- `looks_after_kills`: a device information channel's request of its own, the looks at the list
  after the kills, and the waits of 200 ms between them.

`running` goes through `usbmuxd`, `application_lookup`, `proxy_tunnel`, `discovery_handshake`,
`service_connection`, `server_first_message`, `process_list`. `launch` goes through `usbmuxd`,
`application_lookup`, `proxy_tunnel`, `discovery_handshake`, `service_connection`,
`server_first_message`, `launch_request`. `stop` goes through `usbmuxd`, `application_lookup`,
`proxy_tunnel`, `discovery_handshake`, `service_connection`, `server_first_message`,
`process_list`, `kill_requests`, `looks_after_kills`. A command that stops early (a bundle the
lookup does not hold, an application that does not run, a problem) goes through the first of them
only. A screenshot's `session` covers what `usbmuxd` and the start of `proxy_tunnel` split, its
`tunnel` the rest of `proxy_tunnel` and `discovery_handshake`, and its `service_handshake` what
`service_connection` names: the screenshot keeps its words and these commands use theirs, so that
one word has one meaning; `server_first_message` is one step, the same in both.

A stage says where a command waited, not why.

Every line of the three (one that answers and one stopped by a problem from the lookup on alike)
names in `stage_ms` how long each stage it went through took, in whole milliseconds, by the
stage's word, in the order taken: for example `"stage_ms":{"usbmuxd":12,"application_lookup":230,
"proxy_tunnel":812,"discovery_handshake":40,"service_connection":3,"server_first_message":20,
"process_list":120}`. (It is a member of its own: a screenshot's `elapsed_ms` is of another
shape.) A deadline that passes answers `deadline_passed` with the `stage` the command was waiting
in and, in `stage_ms`, the stages that finished before it (not the one it was waiting in):
`{"command":"running","device":3,"problem":"deadline_passed","stage":"server_first_message",
"stage_ms":{"usbmuxd":12,"application_lookup":230,"proxy_tunnel":812,"discovery_handshake":40,
"service_connection":3}}`. A step the library gives no way to interrupt is named as its stage all
the same: the deadline cuts the command there. Nothing but a stage's word and a number is in
either: no identifier of the phone or of any application. A refusal of a step before the lookup or
between it and the instruments server (`not_paired`, `session_refused`, `tunnel_failed`,
`service_not_listed` and the like) is answered as before, with no `stage_ms`.

### What the application service does with a request: `probe` (ACIB-3b, second and third rounds)

`acib probe --device <n> [--wait-ms <ms>]`, read-only: in the tunnel of `services`, each of
seven ways in turn, each on a connection of its own to its service's port, each waited for
`--wait-ms` (1 to 120000, default 4000) and no later than the command's deadline (a way the
deadline leaves no time for answers `not_asked`, and the last one asked may be cut short by it:
seven ways of 4000 ms take at most 28000 ms, within the default 30000 ms when the steps before
them take less than 2000 ms; with a longer `--wait-ms`, give `--timeout-ms` seven times it and
some seconds more). Nothing on the phone changes: two ways ask nothing, the others ask for a
list of applications or of processes. (ACIB-3c) Only `probe` asks the application service:
`running`, `launch` and `stop` ask it nothing (above).

**The list of applications it asks for.** The service's list of applications is asked for by
five flags, one per category of application. The library (idevice 0.1.68, `list_apps`) names
them by the request's keys, `includeAppClips`, `includeRemovableApps`, `includeHiddenApps`,
`includeInternalApps` and `includeDefaultApps`, and documents none of them: what each category
holds is read from the key's name alone. Three are asked for: the removable applications (by the
name, those a person can remove), the default ones (those the phone comes with) and the hidden
ones; App Clips and internal applications are not, to ask the phone for less. Beside the five,
the library puts three keys of its own into the request, each `false`:
`includeAppGroupIdentifiers`, `includeContainerPaths` and `requireContainerAccess` (idevice
0.1.68, `app_list_options` in `services/core_device/app_service.rs`, whose comment says the
phone's decoder refuses a request without them). The library offers two ways to ask for the list:
its `list_apps` (the feature `listapps`, protocol version 0, its answer read on the reply
channel) and its `stream_apps` (`services/core_device/mod.rs` 109-159: the feature
`streamapplist`, protocol version 2, the CoreDevice version 629.3, a device identifier and an
invocation identifier that are random UUIDs made for the request, and as input the flags above
beside a stream proxy naming a side channel, a random UUID too; its answers read on the root
channel, a batch of entries at a time until the stream's end). The library's `stream_apps` hands
the entries as a `futures` stream, which `acib` cannot read without a crate of its own, so
`acib` makes that request itself, member for member, and sends it through the library's
RemoteXPC client; a message that names the stream's error, or the service's error in place of a
status, is `device_refused`, and an entry without a bundle identifier or a path is
`answer_unreadable` for the whole list. Of each entry nothing is kept: the way answers their
count. `acib services --service com.apple.coredevice.appservice` tells which features the phone
names.

The ways, in this order:

- `listen_screen_capture`: CoreDevice's screen capture service, which answers on the phone, the
  library's RemoteXPC handshake on it, and nothing asked: what a service that answers sends
  unasked.
- `listen`: the application service, the same handshake, nothing asked: what it sends unasked,
  its answers to the handshake among it. The five ways after it count the messages beyond it.
- `list_apps`: the library's `list_apps` with the flags above: the feature `listapps`, protocol
  version 0, the answer read on the reply channel (stream 3).
- `stream_apps`: the streamed list, as above: the feature `streamapplist`, protocol version 2, a
  side channel, the answer read on the root channel (stream 1) to the stream's end, on a
  connection of its own (the connections of the ways before it are dropped when their waits end;
  whether the library's TCP stack then closes them toward the phone has not been read).
- `list_processes`: the library's `list_processes`: the feature `listprocesses`, protocol
  version 0, the reply channel.
- `list_apps_action` (third round): the list of applications in the form of the request the
  phone answers for a screenshot. The library sends the screenshot's request through its
  CoreDevice client's `invoke_with_plist_action` (idevice 0.1.68,
  `services/core_device/screencaptureservices.rs` 59-66), which puts beside the feature an
  action identifier (`CoreDevice.actionIdentifier`) and sends protocol version 2 in place of 0
  (`services/core_device/mod.rs` 68-82, 161-205: one envelope, these two members apart). That
  client is public, so `acib` opens it on the application service's port (its RemoteXPC
  handshake, as every CoreDevice client of the library does) and sends through it the feature
  `com.apple.coredevice.feature.listapps`, the flags of `list_apps` (above) as input, and the
  action the screenshot's pair gives by its rule: its feature
  `com.apple.coredevice.feature.capturescreenshot` and its action
  `com.apple.coredevice.action.capturescreenshot` share the name after the prefix, so the action
  for `…feature.listapps` is `com.apple.coredevice.action.listapps`. The library documents no
  such rule: it is the screenshot's one pair, and the library's one other pair of a feature and
  an action, the location service's (`services/core_device/location_service.rs` 46-53: feature
  `…feature.simulatelocation`, action `…action.availablelocationscenarios`), does not follow
  it; whether the phone knows this action is what the way tells. The answer is read on the
  reply channel; `answered` when the service's output is a list, as the library's `list_apps`
  reads its output (`entries`, its count), and `failed` with `answer_unreadable` when it is of
  another shape.
- `list_processes_action` (third round): the list of processes the same way: the feature
  `com.apple.coredevice.feature.listprocesses`, an empty input (as `list_processes` sends), the
  action `com.apple.coredevice.action.listprocesses`; `answered` when the output's
  `processTokens` is a list, as `list_processes` reads it (`entries`, its count).

The line names `wait_ms` and, in `ways`, per way: `way`, `outcome`, `error_kind` (where the
library gave an error, one of: `device_refused`, the service answered with its error in place of
an output; `answer_unreadable`, what the phone sent is in a shape the library does not read, the
service's answer or an HTTP/2 frame of it; `connection_failed`, the connection under the exchange
failed; `xpc_failed`, the XPC layer over the connection failed otherwise; `other`, any other
error of the library; its text is never shown: the library hands the service's refusal as the
text of its error dictionary, which can name other applications), `entries` (the count of the
list's entries, when one was answered; nothing of an entry), `ms` (from the way's connection on)
and `phone_sent`: what the phone sent on that connection, as far as the library read it, counted
by `acib`'s own reading of the bytes as they pass (no byte is changed): `bytes`, `closed` (a
read found the connection ended), `read_failed`, `frames` (the HTTP/2 frames by type:
`settings`, `window_update`, `headers`, `data`, `rst_stream`, `goaway` and `other`, a type the
library fails on), for the root channel (`root`) and the reply channel (`reply`) the XPC
messages by what the library does with them (`content`, a body it returns; `empty`, an empty
dictionary, and `bodyless`, no body at all, both of which it skips; `unreadable`;
`replies_without_body`, the skipped ones flagged as a reply; `pending_bytes`, a message not
whole yet), `other_stream_bytes` and `pending_frame_bytes` (a frame not whole yet). The library
reads only while it waits for an answer, so everything the phone sent on the connection until
the way ended is counted. Only counts and words: no identifier of the phone, nothing of any
application, no content of a message. The words of `outcome`, one cause each:

- `answered`: the library returned what the way asks for (for a `listen`: a message with a body
  the phone sent unasked; for an action way, an output of the shape above).
- `refused`: the service answered with its error in place of an output.
- `failed`: the library returned another error, or (an action way) the output is of another
  shape, with its `error_kind`; the phone did not end the connection.
- `connection_closed`: the phone ended the connection or reset a stream (a read found its end,
  a GOAWAY or a RST_STREAM frame), with the `error_kind` the library made of it (the library's
  own kind: `answer_unreadable` for a read that found the end, as it reports the end as an
  unexpected response, `xpc/http2/mod.rs` 70-73; `xpc_failed` for a GOAWAY or a reset;
  `phone_sent` tells which).
- When the wait passed before the library returned, from what the phone sent, in this order:
  `answer_on_other_channel` (beyond `listen`, a message with a body came on the channel the way
  does not read), `answer_incomplete` (a frame or a message was started and is not whole, or
  messages with a body came on the channel read without the library returning, as a streamed
  list without its end), `answer_without_body` (beyond `listen`, a message flagged as a reply
  came on the channel read with an empty dictionary or no body, which the library skips),
  `silent` (not one byte, not even HTTP/2 settings) and `no_answer` (bytes, but none of these).
  **Of a `listen` way**, which asks nothing and has nothing to count beyond,
  `answer_without_body` names the phone's reply to the handshake (the library's handshake
  sends messages without a body or with an empty dictionary on both channels, `xpc/mod.rs`
  50-85, and a message flagged as a reply and without a body that the phone sends after it is
  its answer to them), not an answer to a request; the ways after `listen` count only what came
  beyond it.
- `service_not_listed` (only the screen capture service can be missing: without the
  application service the probe answers `service_not_listed` itself), `service_unreachable` and
  `handshake_failed` (the connection, or the library's handshake on it, failed, with its
  `error_kind`), and `not_asked`.

The problem words: `command_line_invalid`, `async_runtime_failed`, `deadline_passed`,
`usbmuxd_unreachable`, `usbmuxd_list_failed`, `device_not_listed`, `lockdown_unreachable`,
`lockdown_read_failed`, `lockdown_value_unexpected`, `pair_record_unreadable`, `not_paired`,
`session_refused`, `service_refused`, `service_unreachable`, `tunnel_failed`,
`rsd_unreachable`, `rsd_handshake_failed`, `apps_lookup_failed`, `too_many_matching`,
`out_exists`, `out_unwritable`, `out_write_failed`, `service_not_listed`,
`service_handshake_failed`, `channel_refused`, `screenshot_failed`, `picture_header_truncated`,
`picture_header_unknown`, `key_salt_unusable`, `binding_salt_unusable`, `device_key_unknown`,
`orientation_failed`, `application_lookup_failed`, `process_list_failed`,
`bundle_path_unreadable`, `launch_failed`, `still_running`, `not_installed`. The error kinds:
`device_refused`, `answer_unreadable`, `connection_failed`, `xpc_failed`, `other` (each named,
with its cause, where `probe`'s ways and `running`, `launch` and `stop` are described above).

**A locked phone.** Nothing read-only that `acib` can ask tells whether the phone is locked:
the library maps a lockdown refusal `DeviceLocked` to an error of its own (idevice 0.1.68,
`lib.rs` 935-944) but names no request a locked phone refuses; none of its clients `acib` builds
with (the lockdown values, the image mounter, the installation proxy, SpringBoard services, the
instruments server's device information and notifications, the CoreDevice services) reads a
lock state, and its lockdown client speaks of a locked phone only for the escrow bag a
service's session may present to open data protection (`services/lockdown.rs` 348-356), which
starts a service and asks nothing of the lock. Apple's public developer documentation read for
this names no such query either. So `probe` does not tell a locked phone from an unlocked one:
running it once with the phone locked and once unlocked is what tells whether the lock matters.

Not in this version: which application is in front (the library offers no way to ask it: its
lists of applications and processes carry no mark of the application in front, and its
application state notifications tell only what changes after they are turned on, so there is no
command for it), any input, pairing, mounting an image, a stream of pictures, and a mode that
stays up between requests.

## Clean room

`acib` is built on the MIT-licensed Rust library `idevice` (pinned to one exact version in
`Cargo.toml`), which the owner allowed as a dependency (board #479), and on public
documentation. No code of another project is in it, and no build of another project is run by
it or for it. (ACIB-3c) Which way `running`, `launch` and `stop` take, what they send and how
they read the answers were derived from nothing but the library's own source and Apple's public
documentation.
