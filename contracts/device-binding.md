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

## Later slices

- RD-1c: `actingctl devices [bind|unbind]` and the MCP tools `ac_devices`, `ac_device_bind`,
  `ac_device_unbind`, the writers of the private file; they never print a serial.
- RD-2: the device link (`device.link`, `device_link:<state>` for the other link states).
- Wireless (`transport: tcp`): reserved; identity stays the serial (`ro.serialno`), and neither
  label nor fingerprint holds an address.
