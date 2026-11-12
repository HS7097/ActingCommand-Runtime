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

A declaration that breaks one of these rules is refused at stage `assemble`, by check-config and
by a start alike, with `device_config_invalid` and the value `device_config_problem`
(`kind_unknown`, `transport_unsupported`, `endpoint_with_handset`, `serial_in_config`,
`discovery_key_with_handset`, `backend_kind_unsupported`, `alias_too_long`). check-config
prints `{alias, device_config_problem}` as `error.detail`.

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

| Condition | Code | check-config | Start |
|---|---|---|---|
| The file cannot be read, is not JSON of its shape and schema, is too large, names an alias twice or a serial twice, or holds a serial of the wrong form | `device_binding_file_invalid` (`path`, `device_binding_problem`: `unreadable`, `malformed`, `duplicate_alias`, `duplicate_serial`, `serial_invalid`) | fails at `assemble`, `error.detail` `{path, device_binding_problem}` | refused |
| A handset has no binding | `device_binding_missing` (`alias`) | fails at `assemble`, `error.detail` `{alias}` | goes on; the handset is registered unbound (below) |
| The file binds an alias no handset of the configuration has | `device_binding_unused` (`alias`, info) | one warning `device_binding_unused:<alias>`, status stays `ok` | nothing |

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
- **One device, several instances** (Workflow #437 MAPP, not built yet;
  [device-group.md](device-group.md)). Several instance entries may drive one emulator only as one
  device group (`device_group`), whose members name the device by one key with one value. Two
  entries outside one group that name the same device are refused at assembly, and after the
  start's discovery when two keys resolve to one emulator (`device_group_config_invalid`,
  `undeclared_shared_device`, proposed). Before MAPP nothing refuses them, and both drive the
  device. A handset cannot be a member in the first slice: the private binding file binds one
  serial to one alias (`duplicate_serial`, below).
- **Label** (`device_label`): what people, logs and records see. An emulator's is `HOST:PORT`, a
  handset's `handset:<alias>`. The device layer's `DeviceSerial` shows only the label in `Debug`
  and `Display`, has no `Serialize`, and hands the raw serial out for the adb command line only;
  adb output and error text name a bound serial only as `[device:<label>]`, with
  `raw_text_redacted` (RD-1a, [adb-target-recovery.md](adb-target-recovery.md)).
- **Fingerprint.** The ledger's salted SHA-256 (`secret_fingerprint_salt`) of the serial under
  the field `device_serial`: `sha256:<64 hex>`, never equal to the serial.

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
| `actingctl devices list --state-root <dir> [--root <install root>]` | `ac_devices` | `{devices: [{handle?, transport, state, model?, bound_alias?, endpoint?}]}` |
| `actingctl devices bind <alias> <handle> --state-root <dir> [--root <install root>]` | `ac_device_bind {alias, handle}` | `{alias, handle, model?, restart_required: true}` |
| `actingctl devices unbind <alias> --state-root <dir>` | `ac_device_unbind {alias}` | `{alias, unbound, restart_required}` |

- They never run the Runtime and record nothing in the ledger; the next start records a binding
  by fingerprint in `runtime.instance_bound`, so a change takes effect at the next start
  (`restart_required`). The MCP tools take the server's install root and state root for
  `--root` and `--state-root`. A success line carries `device_list_reported` or
  `device_binding_changed`.
- **The adb** is the install root's, `<install root>\tools\platform-tools\adb.exe` (Workflow #337),
  the install root found as `actingctl packs` finds it (this process's install selection, else
  `--root`, else the install root the executable runs from; `install_root_unresolved` without
  one). It runs `adb devices -l` once per `list` and once per `bind`.
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
  `bound_at_unix_ms` and `bound_via` `cli` or `mcp`. The new file is checked by the file's own
  rules before it is written, so a device bound to another alias already is refused
  (`device_bindings_invalid`, `duplicate_serial`); a binding of the same alias is replaced. The
  file is replaced whole through a temporary sibling (`device_bindings_write_failed` with its
  `path` when that fails).
- **`unbind`** removes the alias's binding; an alias without one answers `unbound: false` and
  writes nothing. A start then registers the handset unbound (above).
- A private file that breaks its rules is refused before anything is listed or written
  (`device_bindings_invalid` with `path` and `device_binding_problem`). adb that does not start
  or fails is `device_list_failed` with `adb_path`, caused by `foreign_os_error` or by
  `foreign_adb_error` with adb's exit code and text, every serial the file binds replaced by
  `[device:handset:<alias>]` and `raw_text_redacted`.
- The subcommands take the flags of their row and nothing else (Workflow #382 A1-FLAG): an
  unknown or repeated flag or a stray word is `actingctl_usage_invalid` naming it.

## Later slices

- RD-1c: done, "Choosing a device" above.
- RD-2: the device link (`device.link`, `device_link:<state>` for the other link states).
- Wireless (`transport: tcp`): reserved; identity stays the serial (`ro.serialno`), and neither
  label nor fingerprint holds an address.
