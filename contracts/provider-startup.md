# Provider startup facts

`actingd` parses configuration before entering `RuntimeHost::start_with_provider`.
The Host invokes the one-shot assembly closure after acquiring its OwnerGuard
and opening ArtifactStore and GlobalLedger. `ProviderStartup` borrows that
ledger and accepts only the typed Provider startup record. Its records share
the owner epoch and startup request/correlation/action links. The existing
`RuntimeHost::start` delegates to the same owner for already supplied backends;
it does not claim to have observed their construction.

The event is `provider.startup_observed`, with origin module `provider` and
payload schema `actingcommand.payload.provider.v1`. Each record identifies its
backend (`configured`, `fastdeploy_ppocr`, `onnxruntime` or `mumu_manager`) and
one observation: stage started/completed, file binding, model binding, instance
discovery, capability profile, original failure, not configured, or ready.
Stages cover manifest read/parse, path binding, model identity, backend
construction, registry binding, instance discovery and capability admission.
File bindings retain the configured value,
resolution base and resolved value; model bindings retain the logical model
reference and digest. Paths resolve through the existing manifest-parent
algorithm. Absolute paths and native runtime closure ordering keep their
existing meanings.

Failures retain the original module, classification, severity and message,
including sensitive native detail. The failure event precedes Host startup
failure cleanup. No device session has opened at this assembly boundary;
native library caches retain their existing process lifetime, including partial
initialization failure. A failed ledger append, ledger close or owner close is
fatal. No SDK shutdown confirmation or inference success is inferred from
constructor completion. Only successful configured registry binding records
ready, before listener, RuntimeStarted/RuntimeTakeover and runtime-info
publication. A missing optional Provider records not configured.

Each text field is bounded to 64 KiB; invalid or oversized records fail
explicitly instead of truncating evidence. These records are Sensitive. Public,
Lab and verbose projections withhold the raw startup record; forensic reads
retain it. A construction-ready observation covers only work actually performed
by construction. Lazy initialization and inference remain unobserved.

## MuMu instance discovery

When at least one configured instance carries a discovery binding key
(`instance_index` or `instance_name`), the same assembly closure runs one
`MuMuManager.exe` discovery before the vision provider is assembled and before
any instance is bound. At startup only the vendor-documented read-only
subcommands `version` and `info -v all` are dispatched, with a 10 s timeout and
no console window, and no vendor-private file is read. The documented
`control -v <index> launch|shutdown|restart` IS dispatched later, but only by an
explicit User+Ui or Cli `ControlEmulatorInstance` request, only after the
per-instance lease fence and the device-session close, once per request, with
every dispatch recorded intent -> result (`emulator-control.md`); the hidden
`api` subcommand stays banned, and the resolved `MuMuManager.exe` path is
carried on each discovered binding for that purpose. `MuMuManager.exe` is
resolved in this priority:
the configured `mumu_root`, `ACTINGCOMMAND_NEMU_FOLDER`, the install root of a
running MuMu process, the Windows uninstall entry, then vendor folder
enumeration. The registry tier enumerates `MuMuPlayer*` subkeys under
`HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall`, its
`WOW6432Node` twin and the `HKCU` twin, and reads only the standard uninstall
values `InstallLocation` (fallback: the directory of `DisplayIcon`) and
`DisplayVersion`; `DisplayVersion` is advisory, `MuMuManager version` stays
authoritative, and the vendor's own `SOFTWARE\Netease` keys are never opened.
Several distinct registry roots, or a root without a `MuMuManager.exe`
candidate, are typed refusals. The version floor `6.3.2.0` is a Runtime policy
with no vendor basis (the vendor documents only `4.0.0.3179` as the
`MuMuManager` baseline).

The observation records `started`/`completed` with stage `instance_discovery`
around one `instance_discovery` record that names the resolved source, the
`MuMuManager.exe` path, the reported version and every reported instance
(`instance_index`, `instance_name`, `adb_host`, `adb_port`, `running` and the
`bound_alias` it was matched to, if any; `adb_host` and `adb_port` are omitted
for a stopped instance, which `info -v all` reports as a flat object without
`adb_host_ip`, `adb_port` or `player_state`, observed on MuMuManager 6.5.7.0).
A stopped instance anywhere in the inventory no longer breaks discovery: a
non-running entry is parsed with those three fields optional, while a running
entry still requires a non-zero `adb_port`. Every refusal is recorded as a
`failed` observation with stage `instance_discovery` whose failure message
carries the discovery facts (source, path, version, index, name, port and the
declared values) before Host startup fails with the same classification:
`instance_discovery_unavailable` (tool, spawn, exit, decode or JSON failure,
including a missing install), `mumu_manager_version_unsupported` (below the
policy floor or unparseable), `instance_discovery_no_match` (no reported
instance has the index or exact name), `instance_discovery_ambiguous` (more
than one instance carries the name), `instance_discovered_stopped` (the
matched instance reports no ADB endpoint because it is stopped; the failure
message names the alias, the binding key and the discovered index; it is never
bound with a guessed port, so today the daemon must be started while the
configured instance is running, and starting a stopped instance from a cold
daemon lands in the next slice) and `instance_discovery_conflict` (a
declared `adb_path`, `host` or `port` differs from the discovered value; the
failure message carries both values). A resolved instance is then registered
exactly like an explicit one, with the discovered ADB path, host and port and
no serial. Discovery runs once per startup; nothing is re-probed later.

Between the discovery answer and the first instance binding, still inside the
`instance_discovery` bracket, the same closure records `started`/`completed`
with stage `capability_admission` around one `capability_profile` record. The
profile is derived from the discovery report by a pure builder (no further
subcommand is dispatched; `control` is never used) and admitted through the
Host's `admit_emulator_capabilities` with the required ids `inventory.read`
and `instance.status.read`. The record carries `provider_id` (`mumu.manager`),
`version` (the `MuMuManager version` value) and the closed capability ids
grouped as `available`, `unverified` and `unavailable`; each list is sorted and
the three lists together name every capability id exactly once, or the record
is rejected at sanitization. A refusal is recorded as a `failed` observation
with stage `capability_admission` (module
`actingcommand_runtime_host::emulator_control`, the admission code, the
provider id and version in the message) before Host startup fails with
`emulator_capability_admission_refused`. The admitted profile is attached to
every discovered registration and merged into the instance capability profile
that `status` reports (see
`docs/architecture/emulator-control-capability-matrix.md`); explicit entries
keep the registry-only profile.

## Instance binding

Immediately after `runtime.started` or `runtime.takeover`, the Host records one
`runtime.instance_bound` event per registered instance, ordered by instance id.
A registry with no configured instance records `runtime.started` and zero
`runtime.instance_bound` events; the daemon runs control-plane-only until the
configuration lists instances and it is restarted.
The event is family Runtime with severity Info; its sensitivity is derived as
Internal rather than declared. Its links carry the registered `instance_id`, and
its payload carries the registered `instance_alias`, the backend `provenance`,
the configured `adb_host` and `adb_port`, `serial_configured` and
`binding_source`. `binding_source` is `explicit` for a configured registry entry
and `discovered` for an instance bound through MuMu instance discovery. A
discovered binding also carries `discovered_instance_index`,
`discovered_instance_name` (at most 256 bytes) and `provider_version` (the
`MuMuManager version` value, at most 64 bytes); an explicit binding carries none
of the three, and a discovered binding without index and name is rejected at
sanitization. The port is a plain payload field,
because an instance is recognised in the ledger by its ADB port; the audit
`device_endpoint` keeps its existing redaction wherever it is carried, and this
event carries no audit endpoint of its own. `serial_configured` states that
an explicit serial was configured, so the recorded host and port are the
configured target rather than the resolved transport serial; the resolved serial
is never parsed. An instance with no ADB target, including a fixture simulation,
omits host and port. A serial-configured instance, like one without a port, is
excluded from port grouping: its port never identifies it in the ledger port map
or in the `status` `adb_port` field. A failed append is fatal, as for
`runtime.started`.

`actingcommand-vision-provider-check --state-root <runtime-state>` reads the
specified Runtime ledger through B's `ForensicRequest::events` and the shared
`origin_module=provider` filter. It uses `--after` (exclusive), `--through`
(fixed upper boundary) and `--limit` (1 through 1024). The returned page includes
its frozen boundary and next cursor. An empty page states only that it has no
matching Provider facts; it does not prove readiness or failure. Consumers
retain owner epochs and pagination boundaries when associating startup facts.

The checker reports inference and lazy initialization as unobserved by startup.
It does not assemble a Provider. Manifest validation, artifact locks and PE
export inspection remain file operations and are labelled `mechanical_files`
in command output. The production graph does not depend on the checker or B.
