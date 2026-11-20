# acib, the ActingCommand iOS Bridge

`acib` (in Chinese 「iOS 调试桥」) is the project's own bridge to iOS devices (board #479). It is a
program of its own: the Runtime will speak to it as a separate process, as it speaks to adb.
Nothing of the workspace depends on it.

## This version (ACIB-0, ACIB-1 in 0.12.73; ACIB-2, ACIB-3 in 0.12.74; ACIB-3b, ACIB-3c in 0.12.75; ACIB-3d in 0.12.76)

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
command only reads. (ACIB-3d, fix round 2) The library's calls that would restart, shut down or
otherwise change the phone, or install, remove or change an application, are named in "Calls this
program never makes", below. `launch` starts one application, through the instruments server's process
control (whether it then comes to the front the library does not say: to be observed on the
phone); with `--relaunch` it first ends that application if it runs (what was not saved in it is
lost). It is undone by `acib stop` with the same bundle, or by the person on the phone (the home
gesture leaves it; the app switcher ends it); `acib` does not bring back what was in front
before. `stop` asks the instruments server to kill the application's own processes at once (its
`killPid:`, sent in the form `--via` names, below; the library does not name the signal: what was
not saved in the application is lost); it leaves the processes of its extensions (a widget, say)
to the system, as a person who swipes the application away does. It is undone by `acib launch`
with the same bundle, which starts the application anew, not in the state it was in. A `launch`
or `stop` whose deadline passes, or a `launch` or `stop` answered with any problem (below), may
already have changed the phone: `acib running` tells what runs. (ACIB-3d) On a phone with iOS
27.0.1 the library's own form of the kill ended nothing, sent as the library sends it and (fix
round 1) with its answer waited for, the way `primitive` (below); **the way `archived` has not
run on a phone**.

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
installed and runs, the pids of its own processes and of its extensions', and of the list a count
(`located`) and a word (`application_located`). No other
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
  process control channel), and (ACIB-3d) for the list again, on a connection of its own.

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
  path of the executable is read from no description. **What was seen** (an observation of one
  phone and one system version, not a rule): on a phone with iOS 27.0.1, on 2026-11-19, with the
  build of `aef1bd17`, `running` answered `running` true with one pid for each of three installed
  applications (asked twice for one of them, the same pid both times), so there `realAppName`
  held the path of the executable directly in the bundle's directory, and the rule below matched
  it. Another phone or system version may name its processes otherwise: that is why an entry at
  the bundle's directory itself, and a list that locates no process, are read as below. An
  answer that is no list, or one entry that is no dictionary or has no pid from 0 to 4294967295,
  is `answer_unreadable` for the whole list (a process whose pid cannot be read could be the
  bundle's); an entry without a `realAppName` text is of no application.
- **The launch** (`launch`): the channel `com.apple.instruments.server.services.processcontrol` and
  on it the library's `launch_app` (`services/dvt/process_control.rs` 120-218, below).
- **The kill** (`stop`): the same process control channel and on it `killPid:`, once per process
  of the application, in the form of the way `--via` names (ACIB-3d; `services/dvt/process_control.rs`
  275-296 is the library's own form, below); the looks after the kills on another connection to
  the instruments server, in the same tunnel.

**Which processes are of a bundle.** The installation proxy's record gives the bundle's path, a
directory; the instruments server's list gives each process its pid and its `realAppName`, and no
bundle identifier. A process is of the bundle when its executable lies inside the bundle's
directory, and the two kinds are told apart (Apple's "Placing content in a bundle": on iOS the
main executable is at the top of the bundle, app extensions in `PlugIns/`). **The application
itself** is a process whose executable lies directly in the directory, or (ACIB-3c, fix round 1)
whose location is the directory itself: only it counts for `running`, and only it is ended by
`stop`. (A list that names bundles in place of executables names the application by its
directory and its extensions deeper, by theirs; a list that names executables names no process
by the directory: neither is misread, and `application_located` says which way the application
was found: `directly_inside`, `at_directory`, or `both` when it has entries of each.) **A known
limit** (ACIB-3c, fix round 2; not an observation): an entry at the bundle's directory is read
rightly as the application itself only while a list keeps to one way of naming. Were a phone to
record an extension at its host's directory while the application itself does not run, `running`
would answer that the application runs (`application_located` `at_directory`) and `stop` would
send that extension a kill request; no phone has shown such a list (the one looked at names
executables). A process whose executable lies deeper (an app extension in `PlugIns/`, a helper
inside a nested bundle) is the bundle's but is not the application running: a widget the system
runs for it does not make the application run. Its pid is named apart, in `extension_pids`. A
process's `name` is never read, so a process of another application with the same name is not
the bundle's. Paths are
compared exactly, case included (iOS's file system is case-sensitive); a location given as a
`file://` URL is read as its percent-decoded path, `/private/var/…` as `/var/…`, and one that is
no absolute path is of no application. A process of another application whose path merely starts
like the bundle's directory (a longer name, a sibling directory) is not of it.

**What cannot be told is not "does not run" (ACIB-3c, fix round 1).** Each look counts the
entries of the whole list whose location is read as a path (`located`). A list with none (an
empty one, or one whose entries carry a bare name there, or no `realAppName`) gives nothing to
match a bundle against: `running` answers `process_paths_unreadable` (status 1) in place of
`running` false; `stop` answers the same word and sends no kill request when its first look
locates nothing, and gives no verdict when a look after its kills locates nothing. `located` is
a number and `application_located` a word: no name, path or pid of another process is in any
answer, error text or log line.

- **A failed match looks like an application that does not run: `installed` true, `running`
  false, `pids` and `extension_pids` empty (and `located` above 0); of an application open on
  the phone it means only that `acib` matched none of the list's entries to it, for a cause it
  does not tell apart, not that the application does not run.** Where no entry of the list has a
  location read as a path, nothing can be matched: `running` then answers
  `process_paths_unreadable` (status 1), never `running` false.

  `acib running --device <n> --bundle <id>`, read-only: whether the lookup holds the bundle
  (`installed`), whether the application itself runs (`running`), the pids of its own processes,
  ascending (`pids`), the pids of its extensions' processes, ascending (`extension_pids`, which
  do not make it run), how many entries of the list have a location read as a path (`located`)
  and, when the application runs, how its entries were located (`application_located`): for
  example `{"command":"running","device":3,"bundle":"com.example.app","installed":true,
  "running":true,"pids":[412],"extension_pids":[530],"located":180,
  "application_located":"directly_inside"}`. Only an extension alive answers `running` false,
  `pids` empty and its pid in `extension_pids`. A bundle the lookup does not hold answers
  `installed` false, `running` false and no pid, status 0 (above). Its words:
  `application_lookup_failed`, `service_handshake_failed` (the instruments server's first
  message) and `process_list_failed` (each with `error_kind`; a line stopped after the lookup
  keeps `installed`), `bundle_path_unreadable` and `process_paths_unreadable`. Nothing on the
  phone changes: the process control channel is not opened. That the record's path and the
  list's `realAppName` meet as described here was seen on one phone (iOS 27.0.1, 2026-11-19, the
  build of `aef1bd17`: above); on another phone or system version it is to be seen again.

  **Reading `running` on a phone**, each outcome and what it means:

  - `running` true with `pids`: a process of the application itself is listed, in front or in
    the background (which application is in front `acib` cannot tell); `application_located`
    says how its entry was found: `directly_inside` (an executable directly in the bundle's
    directory), `at_directory` (the directory itself) or `both`.
  - An installed application that is open answered `installed` true, `running` false, `pids`
    and `extension_pids` empty (`located` above 0): it was not matched, and why is not told
    apart (its entry's location lies outside the bundle's directory, its entry carries a bare
    name where others carry paths, or the list does not hold it); it is not "does not run".
  - `running` false with `extension_pids`: processes deeper in the bundle's directory are listed
    and none of the application itself: only an extension runs, or the application's own entry
    was not matched (as above).
  - `installed` false: the installation proxy's lookup of the bundle, as written, does not hold
    it (an identifier written wrong; whether one written in another case than the phone's is
    found has not been seen); `stage_ms` names only `usbmuxd` and `application_lookup`: no
    tunnel was built.
  - `process_paths_unreadable`: the list was read, but no entry of it has a location read as a
    path (an empty list, bare names, no `realAppName`): nothing could be matched, and whether
    the application runs is not told.
  - `process_list_failed` with `answer_unreadable`: the device information channel's request was
    answered with a payload, or the list is of another shape; the two are not told apart. With
    `connection_failed`: the connection failed, or the library could not read a message the
    phone sent. With `other`: another error of the library, for example a channel the phone
    cancelled before `acib` read it (`services/dvt/remote_server.rs` 938-960).
  - `deadline_passed` at `server_first_message`: the phone sent on the root channel no message
    the library hands on (none, or capabilities it could not decode); at `process_list`: the
    channel's request or the list was not answered in time.
  - `service_handshake_failed`: the connection to the instruments server failed before or while
    its first message was read.

  **Reading `both`** (ACIB-3c, fix round 2): the application's entries were located both at the
  bundle's directory and directly in it. Neither way of naming explains such a list (one that
  names bundles has no entry one level inside the directory, one that names executables none at
  the directory itself); the likeliest cause is an extension recorded at its host's directory.
  The application runs either way, so `running` answers `running` true, its `pids` those of the
  entries of both kinds, and the word shows the oddity: which entry is the application itself
  the list does not tell, and `stop` then sends nothing (`application_entry_ambiguous`, below).
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
- `acib stop --device <n> --bundle <id> [--via <kill>] [--wait-ms <ms>]`: the application's own
  processes the first look lists are each sent a kill request, one after the other, on the
  instruments server's process control channel: the method `killPid:`, in the form of the way
  `--via` names (fix round 1: `primitive`, the default, `archived` or `library`; ACIB-3d, below,
  where each way's library calls are listed); the library does not name the signal the server
  sends. A way that asks for answers then waits for them, at most `--wait-ms` (1 to 120000,
  default 5000) in all, (fix round 1) each answer paired with its request by the number the
  library gave the request, never by its place among the answers (below).
  Its extensions' processes are sent nothing: `stop` ends the application, not its extensions,
  as a person who swipes the application away does, and names the extensions' pids it left,
  which is no failure. Then (ACIB-3d) another connection to the instruments server is made, in
  the same tunnel, and its first message read, and on it the list is read again at once, on a
  device information channel of its own, and again every 200 ms while it still lists a process
  the command signalled and the next look starts before the command's deadline (a look the
  deadline cuts leaves the one before it as the last); the connection the kills went out on is
  held, unused, until the command ends. The line answers `via`, the way (on every line of
  `stop`, a refusal's too), `installed`, whether the application
  ran at the first look (`was_running`), the pids a kill request was made for (`signalled`), the
  pids of the application the last look lists (`remaining`), whether any runs (`running`), the
  pids of its extensions the last look lists (`extension_pids`) and, of that look, `located` and,
  while the application is still listed, `application_located`, as `running` names them; (ACIB-3d)
  of a way that asks for answers, what each kill request was answered (`replies`: per pid,
  `reply` `without_payload`, `with_payload` or `not_in_time`, and of an answer that came its
  `message_type`, the first byte of its payload header; a payload is never read), and (fix round
  2) how many answers this program saw that were given to no kill request (`unpaired_replies`,
  only when there are any, below); and, of each
  stage after the first look, whether the command reached it (`reached`: `kill_requests`,
  `kill_replies` of a way that asks for answers, `look_connection`, `looks_after_kills`):
  `{"command":"stop","device":3,"bundle":"com.example.app","via":"primitive","installed":true,
  "was_running":true,"signalled":[412],"replies":[{"pid":412,"reply":"without_payload",
  "message_type":0}],"remaining":[],"running":false,"extension_pids":[530],"located":181,
  "reached":{"kill_requests":true,"kill_replies":true,"look_connection":true,
  "looks_after_kills":true}}`. An
  application that does not run (or that the lookup does not hold, above) is a success that says
  so: `was_running` false, nothing signalled, nothing remaining, its extensions' pids and the
  first look's `located` named (none when the lookup does not hold it: nothing was looked at).
  (ACIB-3c, fix round 1) A first look that locates no process answers `process_paths_unreadable`
  (status 1) and sends no kill request: what cannot be told is not "does not run". A look after
  the kills that locates none answers the same word, keeping `signalled`, with no verdict: a
  `signalled` that is not empty means kill requests were sent to those pids, and only whether
  the processes ended could not be seen. (Fix round 2) A first look that locates the
  application's entries both at the bundle's directory and directly in it (`both`, as `running`
  reads it, above) answers `application_entry_ambiguous` (status 1) and sends no kill request:
  the list does not tell which entry is the application itself, and `stop` ends only that. A
  look after the kills that meets `both` sends nothing more, as no look after the kills does: the
  kills went to the pids of a first look that told the application apart, and that look is read
  as any other, its `application_located` `both`. **The verdict comes from the looks, never from
  the requests**: when the last look inside the deadline still lists a process the command
  signalled, the line answers `still_running` (status 1); when it lists none of them, the
  application is stopped (status 0). A kill request that failed (the process control channel not
  opened, or the request not written) is a fact beside the verdict: its pid and the error's kind
  are in `signal_errors` (only when there are any), whether its process is gone (stopped) or
  still listed (`still_running`); (ACIB-3d) of a way that asks for answers, a request whose
  answer could not be waited for to its end is such a failed request too, and (fix round 1)
  names why in `reply_failure`, as far as the library's error tells it (below). A process of
  the application the last look lists that the command did not signal (one started after the
  first look) is named in `remaining` and `running`, and is not `still_running`. A look after the
  kills failing answers `process_list_failed` and keeps `signalled`; (ACIB-3d) the looks'
  connection not made answers `service_unreachable`, its first message not read
  `service_handshake_failed`, each with `error_kind`, keeping `signalled`, and none of the three
  says anything of whether the application runs (no `running`, no `remaining`: `reached` names
  the stage it stopped at). A deadline that passes before the first look after the kills has
  answered is `deadline_passed`, with the `stage` (`kill_requests`, `kill_replies`,
  `look_connection` or `looks_after_kills`). An application that caught the server's signal and
  stayed would be seen so, in the looks. It changes what the phone runs (above).

  **`stop` on a phone, and its ways (ACIB-3d).** What ran on a phone (observations of one phone,
  iOS 27.0.1, on 2026-11-19, the system's calculator alone; not a rule):

  - **13:28 to 13:29 JST, the build of ACIB-3c.** `launch` and `launch --relaunch` worked (the
    relaunch answered another pid: the server ended the running one and started it anew), while
    `stop`, twice, sent the library's `kill_app` for the one pid the first look listed and then
    answered `process_list_failed` with `connection_failed`, its `stage_ms` `kill_requests` 1 and
    `looks_after_kills` 1: the look made right after the kill, on the same connection, failed
    within about a millisecond; the application kept its pid and the screen did not change,
    while a `running` right after connected and answered as always.
  - **15:17 JST, the build of ACIB-3d (`087efba2`).** `stop --via primitive`, once, for the
    process the 13:28 round had left running: `signalled` named it; `signal_errors` held it with
    `error_kind` `connection_failed`; `reached` named `kill_requests`, `kill_replies`,
    `look_connection` and `looks_after_kills` all reached; `stage_ms` `kill_requests` 2,
    `kill_replies` 2, `look_connection` 398, `looks_after_kills` 29076; `remaining` the same
    process, `running` true, `still_running`, status 1; a `running` afterwards listed the same
    process. So the looks on a connection of their own work there, and the kill in the library's
    own form, its answer asked for, ends nothing: the wait for its answer failed within about two
    milliseconds, the library's reader of the connection having stopped (the reason the build of
    fix round 1 names `reader_stopped`, below). `library` was not sent again (the same bytes as at
    13:28), and `archived` was not sent. (Fix round 3) That build, as every build up to fix round
    2, read nothing more after the first failed read, and the library can hand a read that is
    already waiting the end of the connection before an answer it has just read (below): **so
    the run does not show that the phone sent no answer to the kill before the connection ended**,
    only that `acib` saw none.

  **What the library's source says of the run at 13:28**, and what it does not:

  - The look after the kill was the library's request for a new device information channel
    (`_requestChannelWithCode:identifier:` on the root channel, its answer waited for:
    `services/dvt/remote_server.rs` 425-458, 798-812) on the connection the first look and the
    kill had used (the first look's channel and the process control channel stay open: the
    library's channel closes nothing when it is dropped, 118-127, and the one thing that does,
    the client's own drop, 1265-1271, was not reached). It failed with the library's socket error,
    which it gives once its reader of the connection has stopped (`closed_error`, 1245-1250; the
    reader stops on any error of a message it reads: the connection ended or reset, or a message
    it cannot read, 853-896, `services/dvt/message.rs` 585-665), or when the write itself fails
    (its TCP stack's connection already gone: `jktcp` 0.1.8, `handle.rs` 92-130, 456-497,
    `adapter.rs` 330-345). A reader stopped before the request was written would have left it
    waiting until the deadline (`remote_server.rs` 729-739, 1241-1243): the connection ended while
    the request was under way, or before it, from the phone's side or from the library's reading
    of something the phone sent. Which of these, the library cannot tell (one error for all).
  - The library's `kill_app` waits for its request to be written and flushed alone
    (`process_control.rs` 286-296: `killPid:` with the pid as a 32-bit number, `expect_reply`
    false; `remote_server.rs` 784-795, 712-748, 1273-1279); the flush waits for the TCP stack to
    take the bytes, which it sends at once unless the phone's window is full (`jktcp`
    `handle.rs` 178-189, 456-497, `adapter.rs` 474-525). Nothing `acib` did after it dropped the
    connection before the failed look. Whether the phone's server read the request, refused its
    form or ended the connection on it, its source does not show.
  - `launch --relaunch`, which ended the application, differs in each of these: its request
    (`launchSuspendedProcessWithDevicePath:…`) waits for its answer, correlated by its number
    (`process_control.rs` 182-199, `remote_server.rs` 1346-1354, 798-812), all five of its
    arguments are archived values, and it was the only request on a fresh connection. The
    library's own device information client sends a pid as an archived number with its answer
    asked for (`execnameForPid:` and `isRunningPid:`, `services/dvt/device_info.rs` 95-122), where
    `killPid:` sends it as a 32-bit number and asks for none.
  - The library's repository (compared on 2026-11-19, for comparison only) has these three files
    of 0.1.68 (`process_control.rs`, `remote_server.rs`, `message.rs`) line for line on its main
    branch: no later commit changed the kill or the reading of the connection. What else the
    library's public interface offers to end a process is listed below (fix round 1).

  After the run at 13:28 the likeliest cause was the form of the request (the server failing the
  connection on a pid given as a 32-bit number, where it acts on an archived one); then that the
  request was not acted on before the connection ended (the library waits for no answer); then
  that the second device information channel on the same connection ended it. The run at 15:17
  leaves out the last two: `primitive` waited for its answer and asked nothing more on that
  connection, and the library's reader of that connection stopped all the same, within two
  milliseconds of the kill. Left: the form of the request; or (a cause the review of #1849
  added) a server that does not end a process its own connection did not start (`--relaunch`
  ends one by another mechanism). Either way the server ended the connection or sent a message
  the library could not read, and the library's error does not tell which (below); whether it
  answered the kill before that, the run at 15:17 does not tell (above). The ways, by `--via`:

  - `primitive` (fix round 1: the default, the form that is known):
    `RemoteServerClient::make_channel` of `com.apple.instruments.server.services.processcontrol`
    (its answer waited for), then per pid `Channel::call_method` with `killPid:`, the argument the
    library's own, `AuxValue::U32` of the pid, its answer asked for, then `Channel::read_message`
    for the answers, at most `--wait-ms` in all, each paired with its request by its number
    (below). The same bytes as the library's `kill_app` but for the one flag that asks for an
    answer. On the phone at 15:17: nothing ended, and no answer was seen, the wait for it
    failing at once (above: whether one came is not known).
  - `archived`, sent only when `--via archived` names it: the same calls, the argument
    `AuxValue::archived_value` of the pid as a signed number (as `device_info.rs` 99-101 makes
    one). Not known: whether this server's `killPid:` takes an archived number, what it answers,
    which signal it sends. **Its worst reading** (the review of #1849): the library never sends
    `killPid:` in this form, and its source does not show how the server reads the archived
    object, so that the server reads it as another number cannot be excluded: a kill may reach
    another process. It has not run on a phone.
  - `library`: the library's `ProcessControlClient::new` and `kill_app`, unchanged (what ended
    nothing at 13:28); only the looks after it differ, on a connection of their own. Not sent
    again since: the same bytes as at 13:28.

  Every way then makes the looks' connection (`AdapterHandle::connect_to_service_port` to the
  instruments server's port, `RsdService::from_stream`, its first message read: `tcp/mod.rs`
  10-17, `services/dvt/mod.rs` 37-46) and looks on it as above. To read a run: `via` names the
  way; `reached` how far it went; `replies` (or a pid in `signal_errors`, with its
  `reply_failure`) what became of each request, and `unpaired_replies` how many answers it saw
  that were of none; `running`, `remaining` and the problem
  (`still_running` or none) whether the application was ended; `stage_ms` how long each step
  took.

  **A first run on a phone**, one way at a time, each round from its first step, on an
  application whose state nothing depends on (a system application, as the calculator was):

  1. `acib launch --device <n> --bundle <id>`: it answers the application's `pid`.
  2. `acib running --device <n> --bundle <id>`: `running` true, that pid in `pids`.
  3. `acib stop --device <n> --bundle <id> --via <way>`: read `via`, `reached`, `replies` or
     `signal_errors` (and their `reply_failure`), `remaining`, `running`, `problem` and
     `stage_ms`.
  4. `acib running --device <n> --bundle <id>`: whether the pid is gone (another pid is the
     application started anew, by the system or by a person: no stop).

  `primitive` first (its bytes are known); `archived` only where its worst reading (above) is
  accepted; `library` adds nothing to the run at 13:28.

  **Pairing the answers (fix round 1).** The library numbers the messages it sends on a
  connection 1, 2, 3, … in their order (`services/dvt/remote_server.rs` 249, 791, 804; its
  reader's own replies to the phone take the number of what they answer, 1020-1032, 1281-1295;
  making the client sends nothing, `services/dvt/mod.rs` 43-45), and the server answers a request
  with the request's number (the library pairs every answer it waits for by it:
  `remote_server.rs` 1138-1161). Before the kills `stop` sends three requests on that connection
  (the first look's channel request and its `runningProcesses`, then the process control
  channel's request), so its kill requests are numbered 4, 5, … in the order sent, and an answer
  is given to the request whose number it carries (bytes 16 to 19 of its header,
  `services/dvt/message.rs` 531-545, 597) and to no other: an answer that is missing, late or out
  of order is never given to another pid, and one whose number is of no request still waited for
  is given to none and (fix round 2) counted, in `unpaired_replies` (only when there are any; no
  byte of such an answer is read or kept). A request whose answer did not come in time is
  `not_in_time`. (Fix round 3) `unpaired_replies` counts the answers this program saw before
  its reading ended (every request answered, the wait passed, a read failed, or after the
  connection's end the reading of what the library held done, below): one that came later, or
  that the library had not read whole when the connection ended, is not counted.

  **Reading the answers to their end (fix round 3).** The library's reader of the connection puts
  each message it reads in its channel's queue before it reads the next, and marks the connection
  closed only at the read that fails (`services/dvt/remote_server.rs` 861-885, 1062-1086). A read
  of the channel begun after a message was queued takes it: it looks at the queue first
  (836-839) and at the closed mark only then (842-844), and once the mark is set it answers at
  once, never waiting. But a read already waiting when an answer and the end come together
  returns whichever of the two its `tokio::select!` checks first (846-849), and that macro picks
  its first branch at random: the error may come first, the answer left in the queue. Up to fix
  round 2 `stop` ended its reading at the first failed read, so an answer that had reached this
  computer just before the end could be named `reader_stopped` (two tests did so on CI, and the
  run at 15:17 may have, above). Since fix round 3 a failed read whose error is the connection's
  end (`reader_stopped`, the one error the library gives only once its closed mark is set, a
  mark it never takes back) does not end the reading: each read after it takes the next answer
  the library still holds, at once, until a read fails again, and only then is a request still
  without its answer a failed one. **It reads on only after the connection's end.** After
  `channel_cancelled` (the library dropped the channel's queue with it, `remote_server.rs`
  1234-1239: nothing is left to take) or `other` (the connection's state is not known) the
  reading ends at that first failure, as before. These reads send nothing and wait for nothing:
  what `stop` sends, and when, is as before.

  **When the wait for an answer fails (fix round 1).** The reason is named in `reply_failure`,
  beside the failed request's pid and `error_kind`, from the library's error value alone (its
  variant and, of a socket error, its kind; never its text, never a byte of a message, nothing of
  the device or of another application), one of:

  - `reader_stopped`: the library's error once its reader of the connection has stopped, a socket
    error of the kind `BrokenPipe`, "remote server connection closed"
    (`services/dvt/remote_server.rs` 842-849, 1245-1250). The reader stops at the first error of a
    message it reads (853-896): the stream's end (the phone ended the connection), a reset or
    another failed read, or a message it cannot read (`services/dvt/message.rs` 585-665). It
    traces that error at warning level (878) and keeps nothing of it, and every wait after it
    answers the one fixed error. (Fix round 2) The same error comes, too, when the library fails
    to write its acknowledgement of a channel the phone asked to open (1020-1032): it then marks
    the connection closed (1029-1031), its reader still running. **The library gives a caller one
    undivided error for all of these**, and `acib` does not tell them apart. (Fix round 3) **What
    it says, and what not:** beside a request, `reader_stopped` says that the connection ended
    before this program saw an answer to it (every answer the library had read is taken first,
    above), **not that the phone sent none**: an answer still under way, cut by the end, or one
    the library could not read as a message (which itself stops the reader) is never seen.
  - `channel_cancelled`: the library's `UnknownChannel` (`remote_server.rs` 828-833): the channel
    is no longer the library's, which it makes so only when the phone cancels it
    (`_channelCanceled:`, 938-960, 1234-1239); (fix round 3) what the library held of the channel
    goes with it, so an answer read before the cancel and not yet taken is not seen either.
  - `other`: any other error of the library.

  A message whose stated lengths do not fit its bytes ends the library's reader by a panic
  (`message.rs` 623, 634, 640-643: the slices are not checked), which leaves its closed flag unset:
  the wait then runs to its end and answers `not_in_time`, and the panic's text (a place in the
  library's source) goes to the standard error. So `not_in_time` does not prove that nothing
  came. What the library's public interface would allow beyond this, none of it built:

  - **A lower call, a reader of `acib`'s own.** `Message::from_reader` is public
    (`message.rs` 585), and so are the parts of a request (`Message::new`, `MessageHeader::new`,
    `PayloadHeader::method_invocation`, `Aux::from_values`, `Message::serialize`: 674-687,
    495-514, 565-570, 330, 692-720): `acib` could hold the kill's connection itself in place of the
    library's client and read each message with it, getting the parsed message (its payload's
    bytes too, `raw_data`, 170-171, which `acib` would not read) or the reader's own error. That
    error divides more, not all: the stream's end and an argument list shorter than it says are one
    kind, `UnexpectedEof` (590 and 616; 230-252); a reset is the kind the TCP stack gives; a message
    it cannot read otherwise is `NotEnoughBytes`, `UnknownAuxValueType` or `NsKeyedArchiveError`
    (184, 211, 299, 308; 272; 653-654). It reads a whole message, every fragment by its length,
    before it decodes it (588-621), so such a loop could read on after a message it could not
    decode: a reader that survives an unreadable message. It would give each answer's number and
    message type as well. Cost here: `acib`'s own conversation on that connection, in place of the
    library's (the first message, the channel's request and its answer, the numbering, the
    answers to what the phone itself asks on the connection, which the library gives,
    `remote_server.rs` 961-1035, and the reader in a task of its own against its panics): some 150
    to 250 lines and their tests, no crate added.
  - **The bytes beneath the library.** The library takes any stream (`RemoteServerClient::new`,
    `remote_server.rs` 1253-1257; `RsdService::from_stream`, `services/dvt/mod.rs` 43-45), so
    the kill's connection could pass through a wrapper of `acib`'s own, as `probe`'s tap passes
    the application service's: it would see each read's result, the stream's end, a failed read
    and its kind, or bytes, and a reader stopped with neither under it would be a message the
    library could not read. Cost: some 40 lines and a test, no crate, no byte kept; but its words
    would come from the transport, not from the library's error value.
  - **The library's own trace** (878: the error's text and its debug form): only a log
    subscriber reads it, which `acib` installs none of (above) and which needs a crate the
    dependency list does not have.

  **What else the library offers to end or signal a process (fix round 1; none built).** Of
  idevice 0.1.68's public interface, beside the `killPid:` above:

  - The instruments server's process control (`services/dvt/process_control.rs`, the feature
    `dvt`, on): `kill_app` (286-296: `killPid:` with the pid as `AuxValue::U32`, no answer asked
    for: the way `library`); `launch_app` with `kill_existing` (156-218: the method
    `launchSuspendedProcessWithDevicePath:bundleIdentifier:environment:arguments:options:`, its
    options `KillExisting` true, 168-171): it ends a running application only to start it anew,
    a relaunch and no stop, so `stop` never sends it; `disable_memory_limit` (311-335:
    `requestDisableMemoryLimitsForPid:` with the pid as `AuxValue::U32`, its answer, a boolean,
    waited for), which ends nothing (it is the library's one other request of that channel on a
    pid, and asks for its answer with the pid in the form of `primitive`); `launch_with_options`
    (234-273) is the crate's own, not public. Any other method of that channel the library does
    not name: `Channel::call_method` sends whatever it is given (`remote_server.rs` 1334-1343).
  - CoreDevice's application service (`services/core_device/app_service.rs`, the feature
    `core_device`, on), which answered none of seven read-only requests on the phone (`probe`,
    below): `send_signal` (343-372: the feature `com.apple.coredevice.feature.sendsignaltoprocess`
    with the input `process` `{processIdentifier: <pid>}` and `signal` `<number>`, its answer read
    as the process, a time on the device and the signal: 78-84); `launch_application` with
    `kill_existing` (232-300: `terminateExisting` true, 255), a relaunch; `uninstall_app`
    (325-341: the feature `com.apple.coredevice.feature.uninstallapp`), which removes the
    application and is no stop, and which `acib` never calls (fix round 2: "Calls this program
    never makes", below).
  - The debug server through its proxy (`services/debug_proxy.rs`, the service
    `com.apple.internal.dt.remote.debugproxy`, 14-24, the feature `debug_proxy`, off):
    `DebugProxyClient::send_command` (92-123) sends any packet of the GDB remote serial protocol;
    the library names no packet that ends a process, so what to send would come from that
    protocol's own documentation, not read here.
  - WebDriverAgent (`services/wda.rs`, the feature `wda`, off): `terminate_app` (626-641, its
    `terminateApp` with the bundle) and `delete_session` (696-704): they need WebDriverAgent
    running on the phone, another project's program.
  - The diagnostics relay (`services/diagnostics_relay.rs`, the feature `diagnostics_relay`,
    off): `restart`, `shutdown` and `sleep` (187, 207, 227), the whole phone, no one process,
    which `acib` never calls (fix round 2: below, with every other call of the library that
    restarts, shuts down or changes a phone, or installs or removes an application).

  Which next: the debug server, because it is the one way that goes through neither the
  instruments server's process control (whose `killPid:`, in the form that is known, ended
  nothing on that phone) nor the application service (which answers nothing there). It needs the
  feature `debug_proxy` turned on (a change of the dependency's entry), the protocol's
  documentation, and first a read-only look whether the phone lists the service and what
  entitlement the handshake names for it (`acib services --device <n> --service
  com.apple.internal.dt.remote.debugproxy`). `send_signal` costs the least (its feature is on),
  but its service has answered nothing on that phone.

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
`services/dvt/remote_server.rs` 862-884, 1245-1250; of a kill request's answer, fix round 1,
`reply_failure` says what the library's error tells of it, above) and `other` (any other error of
the library).
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
- `kill_requests`: the process control channel's request, whose answer is waited for (as every
  channel's request: `services/dvt/remote_server.rs` 446-455), and one kill request per process
  of the application, in the form of the way (ACIB-3d), each written and flushed; of the way
  `library` no answer is asked for (`services/dvt/process_control.rs` 286-296).
- `kill_replies` (ACIB-3d; the ways `primitive` and `archived`): the server's answer to each kill
  request, read on the process control channel, at most `--wait-ms` in all (fix round 1: each
  paired with its request by its number).
- `look_connection` (ACIB-3d): another connection inside the tunnel to the instruments server's
  port, and its first message.
- `looks_after_kills`: a device information channel's request of its own (ACIB-3d: on the
  connection of `look_connection`), the looks at the list after the kills, and the waits of
  200 ms between them.

`running` goes through `usbmuxd`, `application_lookup`, `proxy_tunnel`, `discovery_handshake`,
`service_connection`, `server_first_message`, `process_list`. `launch` goes through `usbmuxd`,
`application_lookup`, `proxy_tunnel`, `discovery_handshake`, `service_connection`,
`server_first_message`, `launch_request`. `stop` goes through `usbmuxd`, `application_lookup`,
`proxy_tunnel`, `discovery_handshake`, `service_connection`, `server_first_message`,
`process_list`, `kill_requests`, `kill_replies`, `look_connection`, `looks_after_kills`. A command
that stops early (a bundle the lookup does not hold, an application that does not run, a problem)
goes through the first of them only, and `stop` the way `library` through all but
`kill_replies`. A screenshot's `session` covers what `usbmuxd` and the start of `proxy_tunnel` split, its
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

### Calls this program never makes

(ACIB-3d, fix round 2.) Of the public interface of idevice 0.1.68, these calls restart, shut down,
put to sleep or into recovery a phone, unpair it or change its settings, install, upgrade or
remove an application or change its data, or change what the phone has mounted, holds on its
pasteboard or shows. **`acib` makes none of them**, in no command and under no flag. Each is named
by its type and its name, with its file under the library's `src/` and the line its function
starts at, and what it would do to the phone. Read for this list was the public interface of every
module the features `acib` turns on compile in (`core_device`, `core_device_proxy`, `dvt`,
`installation_proxy`, `mobile_image_mounter`, `rsd`, `springboardservices`, `tunnel_tcp_stack`,
`usbmuxd`, and what they bring in: the lockdown client, always built, the XPC client, the TCP
stack, the pairing record, the providers, the library's root); of the modules under features it
does not turn on, the diagnostics relay and the restore service, which the reviews of #1849
named. No call of these modules erases a phone.

**The whole phone.**

- `DiagnosticsRelayClient::restart` (`services/diagnostics_relay.rs` 187; the feature
  `diagnostics_relay`, off): it asks the diagnostics relay to restart the phone (`Restart`).
- `DiagnosticsRelayClient::shutdown` (207): it asks the diagnostics relay to shut the phone down
  (`Shutdown`).
- `DiagnosticsRelayClient::sleep` (227): it asks the diagnostics relay to put the phone to sleep
  (`Sleep`).
- `LockdownClient::enter_recovery` (`services/lockdown.rs` 568): it asks lockdown to put the phone
  into recovery mode (`EnterRecovery`).
- `RestoreServiceClient::enter_recovery` (`services/restore_service.rs` 39; the feature
  `restore_service`, off): it asks the restore service to put the phone into recovery mode
  (`recovery`).
- `RestoreServiceClient::reboot` (78): it asks the restore service to restart the phone (`reboot`).

**The pairing.**

- `LockdownClient::unpair` (`services/lockdown.rs` 207): it removes this computer's pairing from
  the phone (`Unpair`), so that the phone trusts this computer no more.
- `UsbmuxdConnection::delete_pair_record` (`usbmuxd/mod.rs` 344): it deletes the pairing record
  usbmuxd holds for the phone on this computer (`DeletePairRecord`).
- `UsbmuxdConnection::save_pair_record` (320): it replaces the pairing record usbmuxd holds for
  the phone on this computer with the one given (`SavePairRecord`).

**The phone's settings.**

- `LockdownClient::set_value` (`services/lockdown.rs` 135): it sets a value of the phone's
  lockdown domains (`SetValue`; the library's own example turns on debugging over Wi-Fi).
- `LockdownClient::remove_value` (168): it removes a value of the phone's lockdown domains
  (`RemoveValue`).
- `ConfigurationServiceClient::set_user_interface_style`
  (`services/core_device/configuration_service.rs` 88): it switches the phone between its light
  and dark appearance.
- `ConfigurationServiceClient::set_liquid_glass_opacity` (101): it sets the opacity of the
  system's glass material.
- `ConfigurationServiceClient::set_color_filter` (145): it turns the accessibility colour filter on
  or off.
- `ConfigurationServiceClient::set_device_text_size` (191): it sets the size of the phone's
  dynamic type.
- `ConfigurationServiceClient::set_reduce_motion` (210): it turns Reduce Motion on or off.
- `ConfigurationServiceClient::set_increase_contrast` (220): it turns Increase Contrast on or off.
- `ConfigurationServiceClient::set_show_borders` (239): it turns the overlay of layout borders on
  or off.
- `ConfigurationServiceClient::set_reduce_transparency` (258): it turns Reduce Transparency on or
  off.
- `SpringBoardServicesClient::set_icon_state` (`services/springboardservices.rs` 181): it replaces
  the layout of the phone's home screen (`setIconState`).
- `SpringBoardServicesClient::set_icon_state_with_version` (207): it replaces the layout of the
  home screen as `set_icon_state` does, in a format version given.
- `RestoreServiceClient::restore_lang` (`services/restore_service.rs` 212): it asks the restore
  service to set the phone's language to the one given (`restorelang`).

**Applications and their data.**

- `InstallationProxyClient::install` (`services/installation_proxy.rs` 131): it installs an
  application on the phone from a package put there before.
- `InstallationProxyClient::install_with_callback` (159): it installs an application as
  `install` does, reporting its progress.
- `InstallationProxyClient::upgrade` (198): it replaces an installed application by the one in a
  package put on the phone before.
- `InstallationProxyClient::upgrade_with_callback` (223): it replaces an application as `upgrade`
  does, reporting its progress.
- `InstallationProxyClient::uninstall` (262): it removes an application and its data from the
  phone.
- `InstallationProxyClient::uninstall_with_callback` (287): it removes an application and its
  data as `uninstall` does, reporting its progress.
- `AppServiceClient::uninstall_app` (`services/core_device/app_service.rs` 326): it removes an
  application and its data from the phone through CoreDevice
  (`com.apple.coredevice.feature.uninstallapp`).
- `FileServiceClient::propose_empty_file` (`services/core_device/file_service.rs` 188): it
  creates an empty file in the domain of its session, an application's data container among them.

**What the phone has mounted, holds or shows.**

- `ImageMounter::upload_image` (`services/mobile_image_mounter.rs` 116): it sends a disk image to
  the phone's image mounter.
- `ImageMounter::upload_image_with_progress` (141): it sends a disk image as `upload_image` does,
  reporting its progress.
- `ImageMounter::mount_image` (224): it mounts on the phone an image sent before (`MountImage`).
- `ImageMounter::unmount_image` (272): it unmounts an image from the phone (`UnmountImage`), the
  developer image among them.
- `ImageMounter::mount_developer` (442): it sends a developer disk image to the phone and mounts
  it.
- `ImageMounter::roll_personalization_nonce` (412): it makes the phone roll the nonce of its
  personalized images (`RollPersonalizationNonce`).
- `ImageMounter::roll_cryptex_nonce` (425): it makes the phone roll its cryptex nonce
  (`RollCryptexNonce`).
- `PasteboardServiceClient::set_text` (`services/core_device/pasteboard_service.rs` 318): it
  replaces what the phone's pasteboard holds with a text.
- `PasteboardServiceClient::set_image` (346): it replaces what the phone's pasteboard holds with
  an image.
- `OrientationServiceClient::rotate` (`services/core_device/orientation_service.rs` 90): it turns
  the phone's interface by 90 degrees.

The image mounter's four `mount_personalized` calls (467 to 621) are built only under the
library's feature `tss`, which `acib` does not turn on.

**Not in this list, and why.** `launch`, with or without `--relaunch` (the instruments server's
`launch_app`, its `KillExisting` false or true), and the kill requests of `stop` (`killPid:` in
each of its three ways, the way `library` the library's `kill_app`) are this program's commands,
each described in its own section above. The library's other calls that launch, end or signal one
process (the application service's `send_signal` and its `launch_application` with
`terminateExisting`, the process control's `disable_memory_limit`, the debug server's packets,
WebDriverAgent's `terminate_app`: listed under `stop`, above) are of the kind of those two
commands: whether one of them is ever built is decided with them, not here. Nor are the
library's general requests in it, which send whatever they are given (`Channel::call_method` and
`RemoteServerClient::call_method`, CoreDevice's `invoke` calls, lockdown's `start_service`, the XPC
client's `send_object`): `acib` uses some of them, and what it sends with each is written in its
command's section. A feature of the library turned on later is read the same way, and its calls
of these kinds are added here and to the test.

**What holds it.** A test (`main.rs`,
`the_source_names_none_of_the_calls_this_program_never_makes`) reads every source file of `acib`
and fails when one names a call of this list: a method call (`.name(` or `.name::<`) or a path to
it whose segment before the name is a type's (`Type::name`, through an alias of the type or `Self`
too, or after generic arguments, `>::name`); lines that open with `//` are comments and are not
read, and a function of a module (`time::sleep`) is no such path. It fails, too, when this list
and the test's own do not name the same calls in the same order. The files it reads are listed in
it by their paths under `src/`, each with `include_str!`: a file added under `src/` fails the test,
named by its path, until it is added to that list, and a file of the list that is gone fails its
build. It is a tripwire for how a call is written, not a proof: what a general request carries no
name holds.

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
`bundle_path_unreadable`, `process_paths_unreadable`, `application_entry_ambiguous`,
`launch_failed`, `still_running`, `not_installed`. The error kinds:
`device_refused`, `answer_unreadable`, `connection_failed`, `xpc_failed`, `other` (each named,
with its cause, where `probe`'s ways and `running`, `launch` and `stop` are described above).
(Fix round 1) The words of `reply_failure`, beside a kill request of `stop` whose answer could not
be waited for to its end: `reader_stopped`, `channel_cancelled`, `other` (above).

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
documentation. (ACIB-3d) The ways of `stop`, and the reading of what the phone showed, were
derived from the library's own source (0.1.68, and its repository's main branch for comparison),
the source of its own MIT-licensed TCP stack (`jktcp` 0.1.8, which the library uses for the
tunnel) and nothing else. (Fix round 1) The reading of the library's errors, the pairing of the
answers by their numbers and the list of the library's other ways to end or signal a process were
derived from the library's own source (0.1.68) and nothing else.
