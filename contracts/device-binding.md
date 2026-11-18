# Device binding: device kind, private binding file, fingerprint

Workflow #398 R1 (model-398 v1.1 §3, §9, §10, §11), slice RD-1b. An instance drives either an
emulator or a handset, a real phone on USB. A handset is identified by its adb serial, and the
serial lives in one file only: the private binding file of the state root. The Runtime reads that
file, records the device by a fingerprint and shows it to people as a label; no configuration,
record, log, export or archive holds the serial.

## Device kind

An instance entry of the actingd configuration takes an optional member `device`:

```json
{"alias": "neutral.phone", "instance_id": "instance_<32 hex>", "application_id": "neutral.application",
 "touch_backend": "adb_shell_input", "capture_backend": "adb",
 "device": {"kind": "handset", "transport": "usb"}}
```

- `kind` is `emulator` or `handset`. An entry without `device` is an emulator: everything an
  emulator entry did before stays as it was (HOST:PORT or discovery binding, the port as the
  instance's identity, emulator control).
- `transport` is a handset's member only: `usb`, the one transport of R1, and also what an absent
  `transport` means. `tcp` (wireless) is reserved and refused, and an emulator names no
  transport.
- A handset names no `serial`, `host`, `port` or `connect` (its target is the bound serial, and
  USB has no `adb connect`), no discovery key (`instance_index`, `instance_name`) and no
  `nemu_app_index`.
- A handset selects only the backends a handset has: input `adb_shell_input`, `maatouch` or
  `minitouch`; capture `adb` or `scrcpy`. Nemu IPC does not exist on a phone, and DroidCast is a
  third-party component no handset may select. `scrcpy` is selectable on a handset only (#402).
- A handset's alias is at most 248 bytes, so its label `handset:<alias>` fits the 256 bytes of a
  `name` value (coordinator ruling 2026-10-13).
- A fixture entry names no `device` (`fixture_device_fields_forbidden`).

- Workflow #479 F-2 (since 0.12.74, `actingd-config` revision 34): `system` is a handset's member
  only, `android` (also what an absent `system` means) or `ios` ("An iOS handset", below); an iOS
  handset names no touch or capture backend and no member only a device reached through adb has
  (`adb_path`, `command_timeout_ms`, a touch tool's member).

A declaration that breaks one of these rules is refused at stage `assemble`, by check-config and
by a start alike, with `device_config_invalid` and the value `device_config_problem`
(`kind_unknown`, `transport_unsupported`, `endpoint_with_handset`, `serial_in_config`,
`discovery_key_with_handset`, `backend_kind_unsupported`, `alias_too_long`; since 0.12.74
`system_unsupported`, `adb_member_with_ios`). check-config prints `{alias,
device_config_problem}` as `error.detail`.

The manifest reports the kind of an instance that names `device` as the parameter
`instance.<instance id>.device_kind` (section `instance`, not settable, read at assembly); an
instance without `device` has no such row, and no row ever holds a serial.

## Backends

#402 HS-1 ([v012-register.md](v012-register.md) §1.267; since 0.12.39). The manifest reports every
device instance's backends as `instance.<instance id>.touch_backend` and
`instance.<instance id>.capture_backend` (section `instance`, by the device layer's own names,
settable since #402 HS-2: a change switches the device, [runtime-config.md](runtime-config.md),
"Switching an instance's backends"); a fixture has neither. Which backends each kind has is the
rule above (an emulator: every touch backend, every capture backend but `scrcpy`; a handset: no
`nemu_ipc`, no `droidcast_raw`), and Nemu touch works only beside Nemu capture. actingd applies it
to the file (`device_config_invalid` with `backend_kind_unsupported`;
`nemu_input_requires_paired_capture`); the configuration's whole-instance check applies the same
rule, which the host fills in from the device layer, to the values in effect, and so to an
instruction from HS-2 on (`config_value_invalid` with `backend_kind_unsupported` or
`backend_pair_required`; a fixture has no backend at all; [runtime-config.md](runtime-config.md),
"Backends").

`nemu_app_index` (`actingd-config` revision 26): an emulator entry may name it whatever its
backends; it is checked whenever named (`nemu_app_index_invalid`) and used only by Nemu touch, which
no longer needs it in the file. A handset still names none.

#402 HS-3 ([v012-register.md](v012-register.md) §1.271; since 0.12.39). The Nemu application index
is resolved when the paired session opens (touch and capture `nemu_ipc`), never when the registry is
built: Nemu touch without `nemu_app_index` passes check-config and a start.

- **The file's value wins.** The registration keeps a named index whatever its backends, so a
  switch to Nemu touch opens with it too; nothing is read from MuMuManager, and
  `device.backend_switched` records it with `nemu_app_index_source` `file`.
- **Otherwise one read.** Before the paired open the registry reads
  `MuMuManager control -v <MuMu index> app info -pkg <application_id>` once
  ([emulator-control.md](emulator-control.md), "Application state read"), bounded as every
  MuMuManager read (10 s, `MUMU_MANAGER_COMMAND_TIMEOUT`) and inside the preparation's own time. A
  discovery-bound instance reads through its binding's MuMuManager and index; an instance bound by
  HOST:PORT through the one MuMuManager under the MuMu root its capture resolves and the index the
  capture names or derives from the port. `running` resolves index 0, the main application (source
  `resolved`), and since #402 HS-5b (0.12.40, register §1.277) so does `stopped`: the index is the
  MuMu clone's whether the application runs or not, and a paired session without its package's
  display takes the main display (HS-5, below). Every other answer refuses the paired open,
  `paired_backend_open_failed`, with `nemu_app_index_missing` as its `caused_by` and that cause's
  `nemu_app_index_problem`: `package_not_installed` (`not_installed`), `info_unavailable` (no
  MuMuManager to read through, a failed or expired run, a non-zero exit, an errcode envelope) or
  `info_malformed` (not UTF-8, not JSON, over the output bound, or no known `state`).
  `application_not_running` stays in the vocabulary and is no longer reported since 0.12.40. Only
  that instance's open fails (#419); a refusal is not kept, so the next open reads again. A preparation (start, emulator control, resume, self-check) records the refusal as every
  failed open (`backend.open_observed`, the `backend.selfcheck.*` facts, `device.self_check` with
  `paired_backend_open_failed`) and, though it acquired nothing, with the `runtime.failed` record of
  stage `runtime_lifecycle_connection_preparation` that a failed open with released resources gets:
  that record's outcome carries the cause and its problem.
- **The resolution is kept** in the registry entry, in memory only, for the entry's MuMu index,
  package and root: a later paired open reads nothing. Only a rebinding after emulator control
  (`rebind_discovered_endpoint`) drops it (since #402 HS-5b: an open that fails, the application
  stopping or starting keep it); the next paired open reads again. With the multi-Nemu gate closing
  the session at every lease end, an emulator start usually costs one read.
- **A handset never reads MuMuManager**: its kinds of backends have no Nemu, and the registry
  refuses a paired open on a handset before any read (`info_unavailable`).

#402 HS-2 ([v012-register.md](v012-register.md) §1.270; since 0.12.39). A switch replaces the
registration's two backends under the registration's own checks (both explicit, one target, Nemu
touch only beside Nemu capture) and recomputes its capability table from them, merged with the
provider's rows as at registration; every other setting of the two backends (adb, target,
timeouts, tool paths, the Nemu folder, the binding's MuMu index) stays the registration's. It opens
nothing and is refused while the instance's session is open (`execution_endpoint_rebind_session_open`,
the rebind's guard); the provider's refusal is `execution_endpoint_rebind_failed`. A switch never
`adb connect`s, `kill-server`s or `start-server`s by itself: the preparation after it opens the
backends through the existing bounded opens. A handset's switch offers only its own backends (the
kind rule above), so it never touches Nemu or MuMuManager. Since HS-3 (above) a switch to Nemu touch
opens with the file's index, or resolves one at the candidate's paired open; its
`device.backend_switched` names the index with `nemu_app_index_source` `file` or `resolved`.

#402 HS-5 ([v012-register.md](v012-register.md) §1.276 HS-5a, §1.277 HS-5b; since 0.12.40). A
Nemu-touch instance whose application does not run behaves as an adb-touch one: whether the
application runs is not an open's condition.

- **The paired session's display.** Before every capture, every resolution probe and every touch
  the paired session looks its package's display up (`nemu_get_display_id`). With one it addresses
  that display; without one (the application does not run, or its display is not up yet) it
  addresses the main display, the one a capture-only Nemu session captures (the configured
  `display_id`, 0), and captures the desktop as an adb session does; the lookup fails nothing and
  no paired session reports `display_missing` any more. A display that appears, goes or changes,
  also from the main display 0 to the package's display 0, invalidates the geometry of every
  earlier frame, so no touch is decided on a frame of another display; the session itself stays,
  so a run keeps its geometry producer. Application commands still invalidate the display, and the
  geometry fence, deadlines and the unconfirmed-contact rules are unchanged.
- **An open that fails releases what it made.** An open that fails after it connected (its
  resolution probe, its first frame, its memory admission) and before it handed its backend over
  releases the connection it made itself under `LocalOnly`, nothing having been pressed on it; the
  open then fails with its resources confirmed (#411 A7: only the work that opened it fails), not
  fenced ([nemu-owned-resource-close.md](nemu-owned-resource-close.md), "The opener's own
  connection").
- **The index read**: `stopped` resolves 0 and only a rebinding drops the resolution (HS-3, above).
- **Pointer input** while the application does not run meets the foreground gate first, as on adb
  touch: `application_not_foreground` (or `application_foreground_unknown`), nothing pressed,
  nothing fenced ([application-lifecycle.md](application-lifecycle.md), "The foreground gate"). The
  reset of a contact never pressed calls nothing.

No record, key, code or word is added; the switch's refusal no longer names `display_missing` or
`application_not_running`, and a switch to Nemu touch no longer needs the application running.

## The private binding file

`<state root>\private\device-bindings.json`, read-only for the Runtime:

```json
{"schema": "actingcommand.device-bindings.v1",
 "bindings": [{"alias": "neutral.phone", "serial": "TESTSERIAL0001",
               "bound_at_unix_ms": 1700000000000, "bound_via": "cli"}]}
```

- One binding per handset instance, by alias. An alias is bound once and a serial is bound once
  (one device drives one instance). A serial is 1 to 64 bytes of printable ASCII without white
  space; an alias is an instance alias of at most 248 bytes. `bound_via` is `cli`, `mcp`, `ui` or
  `manual`. The file is at most 1 MiB.
- Writers: `actingctl devices bind|unbind` (RD-1c; the UI calls the same command) and a person.
  The Runtime never writes it.
- Readers: actingd's configuration assembly, once per start and per check-config, and only when
  the configuration has a handset. A change takes effect at the next start.
- It is not part of the actingd configuration: no configuration generation, configuration
  check, manifest parameter or defaults file carries it, and the evidence export, the ledger
  maintenance archive, the kept-frame folders, the forensic reads and the diagnostic paths never
  read it (architecture guard RD-1b-T10: the file's path constants and reader are named only in
  `crates/actingcommand-contract/src/device_binding.rs` and actingd's binding reader
  `apps/actingd/src/config/device_binding.rs`). A new state root needs its own binding.
- Absent, the file binds nothing.
- Workflow #479 F-2 (since 0.12.74): an iOS handset's row is `{alias, system: "ios", device_key,
  bound_at_unix_ms, bound_via}`, its `device_key` the phone's key under the installation's own
  secret (`sha256:` and 64 lowercase hexadecimal digits, `device_key_invalid` otherwise), never an
  identifier of the phone. A file that holds one is `actingcommand.device-bindings.v2`; a file of
  Android rows alone is written as `v1`, byte for byte as before, and a `v1` file loads as before.
  A `v1` file with an iOS row, and a row of any other shape, is `malformed` (fix round 1: each
  kind of row is read by its own members, so a `v1` file is read exactly as before this slice: an
  Android row with a `system` member, `null` included, is `malformed`); two iOS rows of one key are
  `duplicate_serial` (one device bound twice). A row of another device system than its handset's
  configuration names (`system_mismatch`) binds nothing (fix round 1): a start registers that
  handset unbound, writes one warning daemon line `device_binding_ignored` `{alias,
  device_binding_problem}` (no identifier) and starts every other instance; check-config refuses
  the file (`device_binding_file_invalid` `system_mismatch`).

| Condition | Code | check-config | Start |
|---|---|---|---|
| The file cannot be read, is not JSON of its shape and schema, is too large, names an alias twice or a serial twice, or holds a serial of the wrong form | `device_binding_file_invalid` (`path`, `device_binding_problem`: `unreadable`, `malformed`, `duplicate_alias`, `duplicate_serial`, `serial_invalid`) | fails at `assemble`, `error.detail` `{path, device_binding_problem}` | refused |
| A handset has no binding | `device_binding_missing` (`alias`) | fails at `assemble`, `error.detail` `{alias}` | goes on; the handset is registered unbound (below) |
| The file binds an alias no handset of the configuration has | `device_binding_unused` (`alias`, info) | one warning `device_binding_unused:<alias>`, status stays `ok` | nothing |
| Workflow #479 F-2 fix round 1: a handset's row is of another device system than its configuration names | `device_binding_file_invalid` (`path`, `system_mismatch`) at check-config; `device_binding_ignored` (`alias`, `device_binding_problem`, warning) at a start | fails at `assemble` | goes on; that handset alone is registered unbound, one warning daemon line |

## Unbound handset

A handset without a binding is registered (its alias, instance id and records exist) with a
stand-in target that names no device. The registry refuses every session-opening path on it
(input, capture, application control, ADB baseline probe, foreground query) before any adb
command, with the stage `device_endpoint_unbound`. Its policy availability
`session.instance.available` is `false` with `source_detector` `runtime.device-link` and
`source_snapshot_id` `device_link:unbound`, published at the start and kept whatever its
self-check says ([instance-fact-store.md](instance-fact-store.md), "Backend self-check
availability"); the other instances run. Binding it and restarting makes it a bound handset.

## Identity, label and fingerprint

- **Identity.** A handset is its serial; an emulator is its port (#322).
- **One device, several instances** (Workflow #437 MAPP-1, since 0.12.56;
  [device-group.md](device-group.md); register §1.328). Several instance entries may drive one
  emulator only as one device group (`device_group`), whose members name the device by one key with
  one value; until the slice "order within one device" a group of several is refused at load. Two
  entries outside one group that name the same device are refused at assembly, and after the
  start's discovery when two keys resolve to one emulator (`device_group_config_invalid`,
  `undeclared_shared_device`). Before MAPP-1 nothing refused them, and both drove the device. A
  handset cannot be a member in the first slice: the private binding file binds one serial to one
  alias (`duplicate_serial`, below).
- **Label** (`device_label`): what people, logs and records see. An emulator's is `HOST:PORT`, a
  handset's `handset:<alias>`. The device layer's `DeviceSerial` shows only the label in `Debug`
  and `Display`, has no `Serialize`, and hands the raw serial out for the adb command line only;
  adb output and error text name a bound serial only as `[device:<label>]`, with
  `raw_text_redacted` (RD-1a, [adb-target-recovery.md](adb-target-recovery.md)).
- **Fingerprint.** The ledger's salted SHA-256 (`secret_fingerprint_salt`) of the serial under
  the field `device_serial`: `sha256:<64 hex>`, never equal to the serial. #398 DB-2: a device
  listing's `device_key` is this fingerprint, made by the same function under the caller's salt
  or under the installation's own `secret_fingerprint_salt` ("Choosing a device", below).

`runtime.instance_bound` ([provider-startup.md](provider-startup.md), "Instance binding"):

- An instance addressed by a serial (`serial_configured` `true`) records no `adb_host` and no
  `adb_port`; the sanitization refuses a record that names them (`invalid_instance_binding_device`).
- A handset records the optional member `device`: `{kind: "handset", fingerprint, bound_at_unix_ms,
  bound_via}` for a bound handset, `{kind: "handset"}` for an unbound one. The fingerprint, the
  time and the channel are present together or not at all. An emulator records no `device`, so
  its record is the one it was.

Forensic reads (`ledger-forensics::instance_bindings`, [global-ledger-query.md](global-ledger-query.md))
group every instance id ever bound to the same fingerprint in `devices`, beside the port map of
the emulators.

## Connect, emulator control, status

- A handset's targets carry `connect: false`: no path runs `adb connect` against it, the ADB
  baseline probe included ([application-lifecycle.md](application-lifecycle.md), "ADB
  baseline").
- Emulator control refuses a handset as it refuses every instance bound without discovery
  (`emulator_control_unavailable`, [emulator-control.md](emulator-control.md)).
- `actingctl status` gives no ADB port for an instance addressed by a serial, as before; check-config
  prints `adb_host` and `adb_port` `null` for a handset.

## Choosing a device (RD-1c)

Workflow #398 RD-1c (model-398 v1.1 §3.3). A person or an agent picks a handset's device from
what adb discovers; the UI's device page will call the same command. Three subcommands of
`actingctl`, and the MCP tools that answer the same (observer tools before U-3, as the
configuration tools of #387 D1b):

| CLI | MCP | Answer |
|---|---|---|
| `actingctl devices list [--state-root <dir> \| --config <file>] [--root <install root>] [--key-salt <text> \| --key-salt-env <NAME>]` | `ac_devices` | `{devices: [{handle?, device_key?, transport, state, model?, bound_alias?, endpoint?}], bindings_unread?: {why, cause?}, device_keys_unmade?: {why, cause}}` |
| `actingctl devices bind <alias> (<handle> \| --device-key <key> [--key-salt <text> \| --key-salt-env <NAME>]) (--state-root <dir> \| --config <file>) [--root <install root>]` | `ac_device_bind {alias, handle}` | `{alias, handle, model?, restart_required: true}`; by key `{alias, device_key, handle?, model?, restart_required: true}` |
| `actingctl devices unbind <alias> --state-root <dir>` | `ac_device_unbind {alias}` | `{alias, unbound, restart_required}` |

- They never run the Runtime and record nothing in the ledger; the next start records a binding
  by fingerprint in `runtime.instance_bound`, so a change takes effect at the next start
  (`restart_required`). The MCP tools take the server's install root and state root for
  `--root` and `--state-root`. A success line carries `device_list_reported` or
  `device_binding_changed`.
- **The adb** is the install root's, `<install root>\tools\platform-tools\adb.exe` (Workflow #337),
  the install root found as `actingctl packs` finds it (this process's install selection, else
  `--root`, else the install root the executable runs from; `install_root_unresolved` without
  one; against a candidate configuration, and for a listing without a state root, without the
  selection, below). It runs `adb devices -l` once per `list` and once per `bind`.
- **A row** is one device of adb's list: `handle` `<transport>:<transport_id>` (the adb server's
  session number for the device, never a device value; it changes when the device is replugged
  or the server restarts), `transport` `usb` when the row has a `usb:` segment and `tcp`
  otherwise, `state` as adb reports it (`device`, `unauthorized`, `offline`, …), `model`, the
  alias the private file binds the device to (`bound_alias`, compared locally) and, for a `tcp`
  row whose address is a loopback address (an emulator), `endpoint`. A row without a
  `transport_id` has no handle. **No row names a serial.**
- **`bind`** checks that the selected configuration has the alias as a handset (`instance_unknown`
  with `instance_selector` when it has no such instance, `device_kind_unsupported` with `alias`
  when the instance is not a handset; actingd reports a handset's broken fields as
  `device_config_invalid`), reads adb's list again (`device_handle_unknown` with `handle` when
  the handle is not in it), and writes the device's serial for the alias, with
  `bound_at_unix_ms` and `bound_via` `cli` or `mcp` (or `ui`, below). #398 DB-2: before anything
  is written it refuses a row that cannot be the handset's device ("What a bind refuses", below).
  The new file is checked by the file's own
  rules before it is written, so a device bound to another alias already is refused
  (`device_bindings_invalid`, `duplicate_serial`); a binding of the same alias is replaced. The
  file is replaced whole through a temporary sibling (`device_bindings_write_failed` with its
  `path` when that fails).
- **Against a candidate configuration** (#398 DB-1, 0.12.71; [v012-register.md](v012-register.md)
  §1.460; the coordinator's ruling on board #398). An installer that binds a phone while it
  installs creates the folders (the state root created empty), lays the programs, writes the
  candidate configuration, binds, runs check-config on the candidate, adopts the root and starts
  it for the first time. check-config refuses a handset without a binding
  (`device_binding_missing`, above), and `bind` looked its alias up in the selected configuration,
  which a new installation does not have yet: neither could come first. `devices bind <alias>
  <handle> --config <file>` (the Lab's row `devices bind` with `config`; `actingctl devices bind`
  takes it too, the command being one) binds against the candidate `file` instead:
  - the alias is checked in `file` by the same rules and codes (`instance_unknown`,
    `device_kind_unsupported`); a file that cannot be read is `config_read_failed`, one that does
    not decode `config_decode_failed`, each with its `path`;
  - the binding goes into the private file of the state root `file` names, its `state_root`, which
    must be absolute (else `install_state_root_unresolved` with `config_path`, the rule the Lab
    locates a state root by): the file actingd's check-config of `file` reads, with or without
    `--install-root`, and so the Lab's rescue row `check-config <file>`, which runs it;
  - **no install selection is read**, of the process (one it inherited from another installation
    included) or of a root, and no located state root: the Lab's line carries neither the
    located `--state-root` nor the located `--root` then (`--state-root` beside `--config` is
    `conflicting`). The install root gives its adb alone: the root `--root` names when it is
    given (on the Lab's command line a further `--root`, the row's parameter `root` since #398
    DB-2 fix round 1), else the root the program runs from by
    its place (`locate::root_of_place`: a program in an A/B slot, `<root>\<A|B>\runtime\` or
    `tools\`, beside the slot's `runtime\BUILD-MANIFEST.json` and the root's
    `install\slot-<A|B>.lock`, belongs to `<root>`; a program in a kept build or in a
    single-location root to that root), else `install_root_unresolved`. So a root whose files are
    laid and whose configuration is not selected yet binds, from a slot too. check-config of a
    candidate reads no selection either;
  - before the bind the candidate and the install root's adb must exist; **the bind creates the
    state root's folder and its `private` folder when they are missing**, and the private file,
    nothing else of the state root. So an operation that requires an empty state root must come
    before the bind. Of today's operations that is one: the rescue row `new` (`install new`), which
    refuses a state root that exists and is not an empty folder (`install_command_refused`
    `state_root_not_empty`, answered by `new` alone, `crates/lab-rescue/src/install/new.rs`). The
    installer does not use `install new`: it creates the folders itself;
  - **the first start accepts the state root the bind made**, which holds exactly
    `private\device-bindings.json`: the start reads the state root's installation record before it
    writes anything (no `installation.json`: `unclaimed`, and it goes on), claims the state root
    under the owner lock, reading only `installation.json` and the installation's
    `install\identity.json` (it mints the identity and places the record,
    [state-root-installation.md](state-root-installation.md), "The claim"), and takes the storage
    as new while none of `runtime-state.sqlite`, `runtime-state.key`, `ledger`, `release-blobs` and
    `artifacts` is there. The code that decides it: `RuntimeHost::start_with_provider` in
    `crates/runtime-host/src/host.rs` (`state_root_installation::check` and `::claim` in
    `state_root_installation.rs`, then the storage's materials); check-config's catalog preview
    judges the state root by the same five names (`policy_host/catalog_preview.rs`,
    `STATE_MATERIALS`). Neither a start nor a check asks a state root to be empty;
  - the answer is `bind`'s and `config`, the file as given.
- **The channel `ui`** (#398 DB-1). `--bound-via ui` (the row's `bound_via`, not named `via`,
  which is `actinglab`'s global option) writes `bound_via` `ui`, the installer's; another word is
  `actingctl_usage_invalid` `not_recognised` (the table types it `enum:ui`, so a front refuses it
  first). Without it a binding records `cli` on the Lab's command line, through `serve --stdio`
  and through the Lab's MCP server (which serves the row with the command line's code), and `mcp`
  through `actingctl mcp-serve`'s `ac_device_bind`, as before. The Lab's MCP tool
  `ac_devices_bind` does not offer it (its schema has `instance`, `handle` and `config`), and one
  sent there is refused `arguments_invalid` `unknown`. The next start records `ui` as
  `runtime.instance_bound`'s `device.bound_via`; nothing decides by it.
- **Before an installation is selected** (#398 DB-2, 0.12.72; [v012-register.md](v012-register.md)
  §1.467). An installer shows the attached phones on a page that comes before it installs, when
  the root holds only its tools: no configuration, no state root. `devices` lists there:
  - with neither `--state-root` nor `--config` (the Lab's row where its place locates no state
    root: its line then carries neither the module's state root nor its root) it reads no binding,
    takes its adb as a bind against a candidate does (the root `--root` names, else the root the
    program runs from by its place, never an install selection; else `install_root_unresolved`),
    and its answer says so: `bindings_unread` `{why, cause?}`, `why` `state_root_unknown`, so that
    rows without `bound_alias` never read as "nothing is bound". The place's own refusal is then
    not the answer: a listing needs no state root; it is the member's `cause` (fix round 1: the
    Lab's row hands it on, so that a selection that cannot be read, or a configuration that names
    no state root, is not lost), as an outcome envelope; a listing that asked no place (the
    command line's own `devices list` without a state root) has `why` alone;
  - with `--config <file>` it reads the bindings of the state root the candidate names, its adb
    found the same way, the line carrying neither the module's state root nor its root;
  - with a state root (the located installation, as before) it reads that state root's bindings
    and the adb as before. The Lab's row takes `root`, which stands for the located root.
- **Device keys** (#398 DB-2). A handle lives only as long as the adb server's session; a row
  also carries `device_key`, the same at every listing while its key is the same, which tells
  nothing of the serial to whoever lacks the key: the fingerprint above (the contract's one
  function, `sha256:` and 64 lowercase hexadecimal digits). Its key is, in this order: the
  caller's salt, 16 to 1024 bytes of text, given as `--key-salt <text>` or read from the
  environment variable `--key-salt-env <NAME>` names in the Lab's own process; else the
  installation's own `secret_fingerprint_salt`, of the selected configuration (read as `bind`
  reads it) or of the candidate `--config` names, when it is text of 16 to 1024 bytes; else the
  row has no `device_key`. **A listing never refuses for want of a key** (fix round 1): a selected
  configuration that cannot be read or decoded provides no secret, so its rows carry no
  `device_key` and the answer says so, `device_keys_unmade` `{why, cause}`, `why`
  `config_unreadable`, `cause` the reader's refusal (`config_read_failed` or
  `config_decode_failed`, with its `path`) as an outcome envelope; the older `actingctl devices
  list --state-root` and `ac_devices` of `actingctl mcp-serve` reach the same code and answer
  the same. A configuration without the secret, or with one outside its bounds, is no such
  failure: no key and no member, as before. Under the installation's
  secret the key is the fingerprint the ledger records for the device. A command line is visible
  to the other processes of the same user (process listings, and the system's log where it records
  command lines), so `--key-salt-env` is the form for a salt that must not be. The salt and the
  variable's value are never printed, logged or written; a refusal names the flag only:
  `--key-salt` outside its bounds `out_of_range` (`range_min` 16, `range_max` 1024);
  `--key-salt-env` naming no possible variable (empty, or holding `=` or NUL) `not_recognised`,
  an unset, empty or non-UTF-8 variable `unusable`, a value outside the bounds `out_of_range`;
  both forms `--key-salt-env` `conflicting` (each `actingctl_usage_invalid`).
- **Bind by key** (#398 DB-2). `devices bind <alias> --device-key <key>` names the device by its key
  in place of its handle (neither: `handle` `required`; both: `--device-key` `conflicting`). The
  bind lists the devices again and takes the row whose key, under the salt given with it, else
  under the installation's own secret, is the key; neither salt nor secret is `--key-salt`
  `required`, a salt without `--device-key` its flag `conflicting`. No such row is
  `device_key_unknown` (nothing written). The binding written is the one a bind by the row's
  handle writes; the answer names the `device_key` and the row's current `handle`. On an installed
  root whose selected configuration cannot be read a bind, by key with a salt or without, is
  refused `config_read_failed` (with its `path`) before any key is made: its alias is checked in
  that configuration first, as a bind by handle's is.
- **What a bind refuses** (#398 DB-2). By handle or by key, before anything is written: a row
  adb reports in another state than `device`, the state of an authorized device online
  (`unauthorized`, `offline`, no permissions, which `adb devices -l` prints as `no`,
  `authorizing`, `recovery` and the rest), and a row whose transport is not the one the handset
  declares (`device.transport`, `usb` when it names none: a handset on USB is not bound to a `tcp`
  row, an emulator's loopback row among them). Each is `device_row_unbindable` with `alias`, the
  row's `handle` when it has one and `device_row_problem` (`state_not_device`,
  `transport_mismatch`); fix round 1: `state_not_device` with `device_link_state`, the row's state
  as a word of that vocabulary (`unauthorized`, `offline`, `no_permissions` for adb's `no`,
  `other`), never the row's text. This changes what `bind` accepted before 0.12.72, which bound
  any row adb listed by its handle.
- **What a client asks the Lab** (#398 DB-2). No interface name says what a Lab can do: a client
  asks the Lab it carries for the rows of the operation table its build embeds,
  `actinglab --json help devices` and `actinglab --json help devices bind` (`command.params`, each
  parameter's `name`, `form`, `type`, `required`, `note` and `written`, how the command line
  writes it, as `--root <dir>`), which reads nothing else and sends nothing. A Lab whose `devices`
  declares `root`, `key_salt` and `key_salt_env` and whose `devices bind` declares `config`,
  `device_key` and `root` lists before an installation and binds by key. Every option either row
  takes is among its `params` (fix round 1: `devices bind`'s `--root`, a flag only the command
  line handed on before, is the row's `root` now, written where it went before: beside a located
  root it is `--root` `repeated`; `serve --stdio` takes it; its MCP tool leaves it out, the
  server handing on the root it runs from).
- **Through MCP** (#398 DB-2). The Lab's MCP tools take `config`, `root` (`ac_devices`) and
  `device_key` as the table gives them, and neither `key_salt` nor `key_salt_env` (a tool call's
  arguments stay in the client's transcript, and a variable of the server's process is no
  caller's to name): there a device key is made with the installation's own secret alone.
  `ac_devices_bind` takes no `root` (fix round 1). `actingctl mcp-serve`'s `ac_devices` carries
  the keys of the installation's secret too, and `device_keys_unmade` where its configuration
  cannot be read; its tools take no new argument.
- **`unbind`** removes the alias's binding; an alias without one answers `unbound: false` and
  writes nothing. A start then registers the handset unbound (above).
- A private file that breaks its rules is refused before anything is listed or written
  (`device_bindings_invalid` with `path` and `device_binding_problem`). adb that does not start
  or fails is `device_list_failed` with `adb_path`, caused by `foreign_os_error` or by
  `foreign_adb_error` with adb's exit code and text, every serial the file binds replaced by
  `[device:handset:<alias>]` and `raw_text_redacted`.
- The subcommands take the flags of their row and nothing else (Workflow #382 A1-FLAG): an
  unknown or repeated flag or a stray word is `actingctl_usage_invalid` naming it.

## An iOS handset (Workflow #479 F-2, since 0.12.74)

[v012-register.md](v012-register.md) §1.482; the owner's rulings of 11-18 on board #479 (an
iPhone is scheduled like any instance; the release may carry `acib`; the phone's frames may be kept
in the state root); the rest are the coordinator's rulings in the work order. `acib`, the
project's iOS device bridge (`apps/acib`), is a program of its own, as adb is the Android one.
This build captures nothing and presses nothing on an iOS handset: capture is the next slice.

- **The configuration.** `"device": {"kind": "handset", "system": "ios"}`. `system` is a closed
  pair, `android` or `ios`; absent means `android`; beside another kind, or another word, it is
  `device_config_invalid` `system_unsupported`. The manifest reports it as
  `instance.<instance id>.device_system` only for an entry that names it (no default, read at
  assembly, not settable). An iOS handset names no touch or capture backend
  (`backend_kind_unsupported`) and uses no adb: fix round 1, a member only a device reached
  through adb has (`adb_path`, `command_timeout_ms`, `maatouch_local_path`,
  `minitouch_local_path`, `push_touch_tool`, `handshake_timeout_ms`, `shutdown_timeout_ms`,
  `tap_hold_ms`) is refused, `adb_member_with_ios`, never ignored. The configuration names no path
  of `acib` (fix round 1: the member that did, which nothing read, went): the Lab runs the install
  root's `tools\acib.exe`, the one place one function names, and the slice that first runs `acib`
  from the Runtime adds the member with what resolves it. A configuration written before this
  slice loads unchanged.
- **The private row.** `{alias, system: "ios", device_key, bound_at_unix_ms, bound_via}` ("The
  private binding file", above): the key `acib` makes of the phone's identifier under the
  installation's own `secret_fingerprint_salt`, by the function the ledger fingerprints a handset's
  serial with. No identifier of the phone (UDID, serial, name) is stored, answered, recorded or
  logged by anything outside `acib`. Changing the secret orphans every iOS binding: each phone is
  bound again.
- **The Lab's `devices`.** After adb's rows come the phones `acib devices` lists, each
  `{device_key?, system: "ios", transport, bound_alias?}`: its key under the caller's salt
  (`--key-salt`, `--key-salt-env`), else under the installation's secret, by the rule adb's rows
  have; `transport` is `acib`'s connection (`usb`, `network`, `unknown`); `bound_alias` is the
  alias of the iOS row whose key is the phone's key under the secret (`acib` is asked for both keys
  in one call). An adb row names no `system`: it is Android, as an entry without `system` is. A
  salt reaches `acib` only as a variable of that one child's environment, under the Lab's names
  `ACTINGCOMMAND_ACIB_KEY_SALT` and `ACTINGCOMMAND_ACIB_BINDING_SALT` (never an argument, a log
  line or an answer; the caller's own `--key-salt-env` variable is not inherited by it), and `acib`
  is started as adb is: standard input from nowhere, pipes for its output and errors (register
  §1.475). Fix round 1: the call carries `acib`'s own deadline, `--timeout-ms 10000`, well under
  the installer's 30 s; the Lab waits for the child 12 s at most and reads at most 64 KiB of each
  of its streams (its errors read and dropped), and ends the child (its own) past either. When
  `acib` is not installed and the configuration the listing reads has no iOS handset, nothing is
  asked and the answer is the one of before. Otherwise a listing `acib` did not give answers adb's
  rows as before and one member, `ios_devices_unlisted` `{why}`, `why` a word of
  `ios_list_problem`: `not_installed` (no file), `not_started`, `answer_unreadable`, fix round 1's
  `no_answer_in_time` (not ended with its answer within the Lab's wait; ended), `answer_too_long`
  (more than the cap on a stream; ended) and `not_ended` (the Lab asked it to end and ending it
  failed: it may still run; fix round 2, that cause alone), fix round 2's `state_unreadable` (the
  Lab could not read whether it had ended, so it ended it, and it did end: what it answered is not
  known; a state that cannot be read and an end that then fails is `not_ended`), or
  one of `acib`'s own words for `devices` (`usbmuxd_unreachable` when Apple's device service does
  not answer, `usbmuxd_list_failed`, `deadline_passed`, `async_runtime_failed`,
  `command_line_invalid`, `key_salt_unusable`, `binding_salt_unusable`); the listing is a success.
- **`devices bind` and `unbind`.** For an alias whose configuration says `ios`, `bind` takes the
  listing's `device_key` (with the salt it was listed under, else under the secret), has `acib`
  list the phones under that salt and the secret, and writes the row with the phone's key under
  the secret; the answer is `{alias, device_key, system: "ios", restart_required: true}` (and
  `config` against a candidate). The refusals of an Android bind keep their words: an alias that is
  no handset `device_kind_unsupported`, a key no phone has `device_key_unknown`, a phone bound to
  another alias `device_bindings_invalid` `duplicate_serial`. A handle (adb's) or an Android
  device's key for an iOS handset, and an iOS phone's key for an Android handset, are
  `device_row_unbindable` `system_mismatch` (with the adb row's handle where there is one).
  `acib` giving no list is `device_ios_list_failed` with `alias` and `ios_list_problem`; a
  configuration without the secret is `device_binding_secret_missing`. Nothing is written by a
  refusal; `unbind`, `restart_required` and the bind against a candidate are as for Android.
- **The Runtime.** An iOS handset is bound when its private row is there; it is registered either
  way. Its `runtime.instance_bound` records `device` `{kind: "handset", fingerprint,
  bound_at_unix_ms, bound_via}` with the stored key as the fingerprint, as it is (it is one under
  the installation's salt), and `{kind: "handset"}` while unbound: the record of an Android
  handset's shape, so no `ledger` revision. Until the capture slice it is never available to
  policy: `session.instance.available` `false` under `source_detector` `runtime.device-system` and
  `source_snapshot_id` `device_system:ios`, bound or not, whatever its self-check says
  (`availability_source` `device_system`). Fix round 1: the host refuses every input, capture and
  application operation on it at the front of its admission (`HostShared::input`,
  `control_application_of`, a read-only observe, a capture sequence, a Lab or recovery look, a
  run's capture, a monitor's configuration), before any key table, foreground gate, ADB baseline
  or backend of its stand-in registration is looked at, denied with the operation's own failure
  code (`input_backend_open_failed`, `capture_backend_open_failed`,
  `application_backend_operation_failed`) and the cause `device_system_backend_missing` (error,
  ladder class `declaration_error`): this build has no backend for that operation on that device
  system; nothing is recorded or changed. Behind it the registry refuses the same way (the stage
  `device_system_unsupported`, the same codes and cause), the ADB baseline probe and the
  foreground query included, before anything is opened or run. Only that instance's operation
  fails. Fix round 2: the paths a person starts that would otherwise reach its stand-in
  registration are refused at their entry the same way, denied, each with its own failure code
  caused by `device_system_backend_missing`: a self-check (`SelfCheckInstance`), before its claim
  and its preparation, `input_backend_open_failed` (the code a preparation ends with at its first
  open, the input's); a drill of its recovery ladder (`DrillRecoveryLadder`, plan or device),
  `recovery_ladder_drill_unsupported` (error, exit 1), after the command's `command.rejected` and
  before any ladder is admitted, so neither a rung nor the readiness's baseline probe and
  foreground query meets the stand-in. Its pause and its resume are never refused, since one
  instance never keeps the others from being paused, held, resumed, placed or deployed (tools
  pause and resume every instance at once). An instance pause of it, a person's (`stand`) or a
  tool's hold (`quiet_or_lift`), is quiet at once: the gate closes and the stages move to
  `released` with nothing drained, waited for (a lease, a queued entry, a run or the admission
  guard: none can reach its device) or handed back and closed, so no stage fails and a hold over
  several instances that includes it is quiet as it would be without it and never lifted because
  of it. Its resume (`ResumeScheduling`), a person's or a tool's, succeeds: the pause's refusals
  are those of every instance; the pause is lifted (or held again by a `start_paused` not yet
  applied, as for every instance) and no reconnect is attempted, as the start's preparation
  attempts none: no claim, no preparation, no open. The answer is `SchedulingResumed` with a
  self-check of both sides `ok: false`, no values and no `failure_code`, the receipt's success
  code caused by `scheduling_resume_without_backend` (info), and the ledger holds one
  `device_state_observed` record of it under the pause's links; nothing is answered or recorded
  as a failure. It is inert (fix round 1): the start's preparation, the
  idle planner and every other party that acts on an instance without a person asking skip it, so
  nothing is attempted, and no failure, ladder or stability count arises from it. What the ledger
  says of it is the availability withdrawal above, information and no failure, and the program
  facts `device.system` (`ios`) and `device.bound` (boolean), which the Lab's status shows on its
  row as `device` `{kind: "handset", system, bound}` (nothing of these for an Android handset or an
  emulator). Fix round 2, when they are written: on change, the rule every other availability
  withdrawal follows (Workflow #398 RD-1b's unbound handset, `device_link:unbound`, is the
  precedent; the device link's, the device state's and the backend self-check gate's at a start
  follow it too). The start's preparation, and the self-check gate wherever it decides again,
  writes the withdrawal of a policy instance (one the configuration's policy inputs seed; an
  instance without a seed has no availability) only when the record in effect is not already that
  withdrawal: the first start with the handset writes it once, and a restart over the same ledger
  writes none, because the fact store reloads that record from the ledger and it stays in effect
  (a person reading the ledger finds it there, written by the earlier start, and the scheduler's
  explanation reads `availability_source` `device_system` from it). The program facts are written
  when their value changes, as `device.link`'s are. The status says it at every read: its
  capability table names no backend, every row unsupported and unavailable, its source
  `device_system:ios`, its failure semantics why. No adb device-link watch or device-state read
  concerns it.

## Later slices

- RD-1c: done, "Choosing a device" above.
- RD-2: the device link (`device.link`, `device_link:<state>` for the other link states).
- Wireless (`transport: tcp`): reserved; identity stays the serial (`ro.serialno`), and neither
  label nor fingerprint holds an address.
