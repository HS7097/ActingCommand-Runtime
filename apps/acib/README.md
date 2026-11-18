# acib, the ActingCommand iOS Bridge

`acib` (in Chinese 「iOS 调试桥」) is the project's own bridge to iOS devices (board #479). It is a
program of its own: the Runtime will speak to it as a separate process, as it speaks to adb.
Nothing of the workspace depends on it.

## This version (ACIB-0, ACIB-1 in 0.12.73, and ACIB-2 and ACIB-3 in 0.12.74)

Each run is one command; it prints **one JSON line** on standard output and ends with status 0
when the command was answered, or 1 when it was refused or failed. A refused or failed line
names a `problem` word of this program (the words are its own, not Runtime outcome codes) and,
where the library gave one, its error text in `error` (of the application service, only the
error's kind, in `error_kind`). The one file it writes is the picture `screenshot` takes, at the
path `--out` names; it removes that file again when the picture could not be written whole into
it. With `--out -` it writes no file: the picture's bytes follow the line on the standard output
(below).

**Two commands change what the phone runs and shows: `launch` and `stop`.** Every other
command only reads. `launch` starts one application, which comes to the front; with
`--relaunch` it first ends that application if it runs (what was not saved in it is lost). It is
undone by `acib stop` with the same bundle, or by the person on the phone (the home gesture
leaves it; the app switcher ends it); `acib` does not bring back what was in front before.
`stop` ends the application's own processes at once (SIGKILL, which the application cannot
catch: what was not saved in it is lost); it leaves the processes of its extensions (a widget,
say) to the system, as a person who swipes the application away does. It is undone by
`acib launch` with the same bundle, which starts the application anew, not in the state it was
in. A `launch` or `stop` whose deadline passes, or a `launch` answered with any problem but the
phone's refusal (below), may already have changed the phone: `acib running` tells what runs.

usbmuxd is reached where the library finds it on this platform: on Windows the TCP port 27015
of 127.0.0.1, which the device service of Apple's own software serves. Pairing is not part of
this version: a device whose pairing record usbmuxd does not hold answers `not_paired` (to
`services`, `images`, `apps`, `screenshot`, `orientation`, `running`, `launch` and `stop`, and
to the second half of `info`).

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
phone's lists of applications and of processes are a person's too: `running`, `launch` and
`stop` answer of them only the one bundle asked for (as the command line gave it), whether it is
installed and runs, and the pids of its own processes and of its extensions'. No other
application's name, bundle identifier, path or pid is printed, logged or kept: the lists stay in
memory for the command, the answer types have no member that could hold one, and an error of the
application service is shown by its kind alone, never by its text (the library hands the phone's
refusal as the text of its error dictionary, which can name other applications).

**No log subscriber, ever, without redaction.** `acib` installs no log subscriber and no
logger. The library itself traces, at warning level, the whole error dictionary a phone answers
a CoreDevice request with (idevice 0.1.68, `services/core_device/mod.rs`: "XPC response did not
have an output"), which can name other applications, their paths and their processes, and other
texts of the phone's answers beside it; with no subscriber those events go nowhere. A
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
`deadline_passed` (for `screenshot`, with the `stage` it was waiting in, below; for
`orientation`, per reader). `<n>` is the device's usbmuxd session number, as `acib devices`
lists it; wherever `--device <n>` is written below, `--device-key <key> --key-salt-env <NAME>`
may stand in its place.

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

These three speak to CoreDevice's application service (`com.apple.coredevice.appservice`)
inside the tunnel of `services`, in a session with this host's pairing record, and take
`--bundle <id>`: one bundle identifier, written in the characters Apple allows in one (ASCII
letters and digits, `-` and `.`; anything else is `command_line_invalid`). Bundle identifiers
are compared without regard to case, as Apple's documentation says they are. Each line names
`bundle` as the command line gave it. The steps before the service (the device, the pairing
record, the session, the tunnel, the service listed in the handshake and its port) fail with
their own words, as every command's; from the service's handshake on, an error is shown by
`error_kind` alone, one of: `device_refused` (the service answered with its error in place of
an output: the phone refused), `answer_unreadable` (the service answered in a shape the library
does not read), `connection_failed` (the connection under the exchange failed),
`xpc_failed` (the XPC layer over it failed) and `other` (any other error of the library). A
locked phone has no word of its own: the library maps no refusal to one, and none is guessed
here; what a locked phone answers is to be observed on the phone.

**The list of applications.** The service's list of applications is asked for by five flags,
one per category of application. The library (idevice 0.1.68, `list_apps`) names them by the
request's keys, `includeAppClips`, `includeRemovableApps`, `includeHiddenApps`,
`includeInternalApps` and `includeDefaultApps`, and documents none of them: what each category
holds is read here from the key's name alone, not from any description. Three are asked for:
the removable applications (by the name, those a person can remove, as one they installed) and
the default ones (by the name, those the phone comes with), so that an application a person
installs and the system's own are in the list; and the hidden ones, so that an application a
person installed and then hid is not left out should the phone list it only when hidden ones
are asked for. App Clips and internal applications are not asked for, to ask the phone for
less. Each entry of the answer carries flags of its own (among them `isRemovable`, `isHidden`,
`isInternal` and `isAppClip`), each true or false apart from the others; which entries the
phone lists for which flags of the request has not been checked on a phone.

**An application the list does not hold.** The three commands answer a bundle the phone's list,
for the categories asked, does not hold (one the phone does not have, or one it has only under
a category not asked for) alike, as not installed: `running` answers `installed` false and
`running` false, status 0, whether or not it runs; `stop` answers `installed` false and
`was_running` false, status 0, and sends nothing, so an application that runs goes on running;
`launch` answers `installed` false with `not_installed`, status 1, and sends nothing. So
`installed` false, and the word `not_installed`, have one cause: the phone's list, for the
categories asked, does not hold this bundle.

The library reads each entry of the list into a record of its own (the entry's identifier,
name, path and flags); one entry it cannot read fails the whole list, for every bundle:
`application_list_failed` with `answer_unreadable`. That is the library's behaviour, not
`acib`'s.

**Which processes are of a bundle.** The service's list of applications gives each application
its bundle identifier and the path of its bundle, a directory; its list of processes gives each
process its pid and the location of its executable, and no bundle identifier. A process is of
the bundle when its executable lies inside the bundle's directory, and the two kinds are told
apart (Apple's "Placing content in a bundle": on iOS the main executable is at the top of the
bundle, app extensions in `PlugIns/`). **The application itself** is a process whose executable
lies directly in the directory: only it counts for `running`, and only it is ended by `stop`. A
process whose executable lies deeper (an app extension in `PlugIns/`, a helper inside a nested
bundle) is the bundle's but is not the application running: a widget the system runs for it
does not make the application run. Its pid is named apart, in `extension_pids`. Paths are
compared exactly, case included (iOS's file system is case-sensitive); a location given as a
`file://` URL is read as its percent-decoded path, `/private/var/…` as `/var/…`, and one that
is no absolute path is of no application. A process of another application whose path merely
starts like the bundle's directory (a longer name, a sibling directory) is not of it.

- `acib running --device <n> --bundle <id>`, read-only: whether the list of applications holds
  the bundle (`installed`, for the three categories above), whether the application itself runs
  (`running`), the pids of its own processes, ascending (`pids`), and the pids of its
  extensions' processes, ascending (`extension_pids`, which do not make it run): for example
  `{"command":"running","device":3,"bundle":"com.example.app","installed":true,"running":true,
  "pids":[412],"extension_pids":[530]}`. Only an extension alive answers `running` false,
  `pids` empty and its pid in `extension_pids`. A bundle the list does not hold answers
  `installed` false, `running` false and no pid, status 0 (above). Its words:
  `application_list_failed` and `process_list_failed` (each with `error_kind`; a line stopped
  after the list of applications keeps `installed`), and `bundle_path_unreadable` (the bundle is
  listed with a path that is no absolute path or file URL, so which processes are of it cannot
  be told). Nothing on the phone changes. That the list's paths and the process list's
  locations meet as described here (and so that a running application answers `running` true)
  is to be seen at the first `running` on a phone while the application is open.
- `acib launch --device <n> --bundle <id> [--relaunch]`: first the list of applications is
  asked, as `running` and `stop` ask it: a bundle it does not hold answers `installed` false,
  as they do, with `not_installed` (status 1), and no launch is sent. Then the service launches
  the application and the line answers `relaunch`, `installed` true and the pid of its process
  (`pid`). Without `--relaunch` an application that already runs is not ended first
  (`terminateExisting` false): what the phone then does (bring it to the front, start a second
  one, refuse) the library does not say, and the line answers what the service answered; it is
  to be observed on the phone. With `--relaunch` it is ended and started anew. It changes what
  the phone runs and shows (above).

  **What `launch` sends.** The library (idevice 0.1.68, `launch_application` in
  `services/core_device/app_service.rs`) sends the feature
  `com.apple.coredevice.feature.launchapplication` with this input, every member of it:
  - `applicationSpecifier.bundleIdentifier._0`: the bundle, as `--bundle` gives it (`acib`
    chooses).
  - `options.arguments`: an empty list (`acib` chooses: it passes no argument).
  - `options.environmentVariables`: an empty dictionary (`acib` chooses: it passes no
    environment).
  - `options.standardIOUsesPseudoterminals`: `true` (the library fixes it).
  - `options.startStopped`: `false` (`acib` chooses: the application is not stopped at its start
    for a debugger).
  - `options.terminateExisting`: `true` with `--relaunch`, else `false` (`acib` chooses).
  - `options.user.active`: `true`, the active user (the library fixes it).
  - `options.platformSpecificOptions`: data, the XML property list of an empty dictionary
    (`acib` passes no platform option; the library fixes the encoding).
  - `standardIOIdentifiers`, beside `options`: an empty dictionary (`acib` chooses: it passes no
    stream for the application's standard input, output or error, so nothing of it is connected
    to `acib`).

  Around the input the library puts what it puts around every request of the service (it fixes
  them all): `CoreDevice.featureIdentifier` (the feature above), `CoreDevice.action` (an empty
  dictionary), `CoreDevice.coreDeviceVersion` (629.3), `CoreDevice.CoreDeviceDDIProtocolVersion`
  (0), and `CoreDevice.deviceIdentifier` and `CoreDevice.invocationIdentifier`, each a random
  UUID the library makes for the request (neither is the phone's identifier). What standard I/O
  on pseudo-terminals does to the application once `acib` has exited and its connection is
  closed cannot be read from the library's source: it is to be observed at the first run on a
  phone (the application still running some seconds after `launch` has answered and `acib` has
  ended).

  **What a problem means.** `launch_failed` with `device_refused` is the phone's refusal: the
  service answered with its error in place of an output. Any other kind does not say that
  nothing was started: with `answer_unreadable` the service answered, but the library could not
  read its answer (it needs `processIdentifierVersion`, `processIdentifier`,
  `executableURL.relative` and `auditToken` in the answer's `processToken`; one missing or of
  another shape is enough), and the phone may have started the application all the same; with
  `connection_failed`, `xpc_failed` or `other` the request may have reached the phone. After any
  `launch` problem whose kind is not `device_refused`, and after a `launch` whose deadline
  passed, `acib running` tells what is true.
- `acib stop --device <n> --bundle <id>`: the application's own processes the list of
  processes holds are each sent SIGKILL (9) through the service's own signal request, one after
  the other. Its extensions' processes are sent nothing: `stop` ends the application, not its
  extensions, as a person who swipes the application away does, and names the extensions' pids
  it left, which is no failure. Then the list is read again at once, and again every 200 ms
  while it still lists a process the command signalled and the next look starts before the
  command's deadline (a look the deadline cuts leaves the one before it as the last). The line
  answers `installed`, whether the application ran at the first look (`was_running`), the pids
  the signal was sent to (`signalled`), the pids of the application the last look lists
  (`remaining`), whether any runs (`running`) and the pids of its extensions the last look lists
  (`extension_pids`):
  `{"command":"stop","device":3,"bundle":"com.example.app","installed":true,"was_running":true,
  "signalled":[412],"remaining":[],"running":false,"extension_pids":[530]}`. An application
  that does not run (or that the list does not hold, above) is a success that says so:
  `was_running` false, nothing signalled, nothing remaining, its extensions' pids named (none
  when the list does not hold it). **The verdict comes from the looks, never from the signals'
  answers**: when the last look inside the deadline still lists a
  process the command signalled, the line answers `still_running` (status 1); when it lists none
  of them, the application is stopped (status 0). A signal whose answer was an error or could
  not be read is a fact beside the verdict: its pid and the error's kind are in `signal_errors`
  (only when there are any), whether its process is gone (stopped) or still listed
  (`still_running`). A process of the application the last look lists that the command did not
  signal (one started after the first look) is named in `remaining` and `running`, and is not
  `still_running`. A look after the signals failing answers `process_list_failed` and keeps
  `signalled`; a deadline that passes before the first look after the signals has answered is
  `deadline_passed`. SIGKILL rather than SIGTERM: an application can catch or ignore SIGTERM, so
  a look after it would tell what the application chose, not whether its processes ended;
  SIGKILL cannot be caught. The service's own signal request is used (not the instruments
  server's `kill`) because it is on the same connection as the lists and answers each signal. It
  changes what the phone runs (above).

The problem words: `command_line_invalid`, `async_runtime_failed`, `deadline_passed`,
`usbmuxd_unreachable`, `usbmuxd_list_failed`, `device_not_listed`, `lockdown_unreachable`,
`lockdown_read_failed`, `lockdown_value_unexpected`, `pair_record_unreadable`, `not_paired`,
`session_refused`, `service_refused`, `service_unreachable`, `tunnel_failed`,
`rsd_unreachable`, `rsd_handshake_failed`, `apps_lookup_failed`, `too_many_matching`,
`out_exists`, `out_unwritable`, `out_write_failed`, `service_not_listed`,
`service_handshake_failed`, `channel_refused`, `screenshot_failed`, `picture_header_truncated`,
`picture_header_unknown`, `key_salt_unusable`, `binding_salt_unusable`, `device_key_unknown`,
`orientation_failed`, `application_list_failed`, `process_list_failed`,
`bundle_path_unreadable`, `launch_failed`, `still_running`, `not_installed`. The error kinds of
the application service: `device_refused`, `answer_unreadable`, `connection_failed`,
`xpc_failed`, `other`.

Not in this version: which application is in front (the library offers no way to ask it: its
lists of applications and processes carry no mark of the application in front, and its
application state notifications tell only what changes after they are turned on, so there is no
command for it), any input, pairing, mounting an image, a stream of pictures, and a mode that
stays up between requests.

## Clean room

`acib` is built on the MIT-licensed Rust library `idevice` (pinned to one exact version in
`Cargo.toml`), which the owner allowed as a dependency (board #479), and on public
documentation. No code of another project is in it, and no build of another project is run by
it or for it.
