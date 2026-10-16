# Provider startup facts

`actingd` parses configuration before entering `RuntimeHost::start_with_provider`.
The Host invokes the one-shot assembly closure after acquiring its OwnerGuard
and opening ArtifactStore and GlobalLedger. Pending eviction recovery, capacity
preflight and proven prior-epoch scope reconciliation complete before assembly;
any failure stops startup before providers or public service. `ProviderStartup` borrows that
ledger and accepts only the typed Provider startup record. Its records share
the owner epoch and startup request/correlation/action links. The existing
`RuntimeHost::start` delegates to the same owner for already supplied backends;
it does not claim to have observed their construction.

The event is `provider.startup_observed`, with origin module `provider` and
payload schema `actingcommand.payload.provider.v1`. Each record identifies its
backend (`configured`, `fastdeploy_ppocr`, `onnxruntime` or `mumu_manager`) and
one observation: stage started/completed, file binding, model binding, instance
discovery, discovery retried, capability profile, original failure, not configured,
ready, or model folder invalid (`model_folder_invalid`: a folder that breaks the folder
rule, whose `outcome` is `vision_model_folder_invalid` with the folder's `path` and, when
it fits the name rule, its `model_name`; startup goes on).
Stages cover manifest read/parse, path binding, model identity, backend
construction, registry binding, instance discovery and capability admission.
File bindings retain the configured value,
resolution base and resolved value; model bindings retain the logical model
reference and digest. Paths resolve through the existing manifest-parent
algorithm. Absolute paths and native runtime closure ordering keep their
existing meanings.

A failure records the module that reported it and its `outcome` (ledger
revision 3, Workflow #378 A3): the failure's code, values and links, whose
category replaces the former severity. Native text is kept only as the
`raw_text` of a `foreign_*` link of that outcome; the Runtime's own text about
the failure is not recorded. The failure event precedes Host startup
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
any instance is bound (tried again while it is unavailable, see "Discovery
unavailable at start" below). At startup only the vendor-documented read-only
subcommands `version` and `info -v all` are dispatched, with a 10 s timeout and
no console window, and no vendor-private file is read. The documented
`control -v <index> launch|shutdown` IS dispatched later (a restart is a shutdown and then a launch), but only by an
explicit User+Ui or Cli `ControlEmulatorInstance` request, only after the
per-instance lease fence and the device-session close, once per request (once per phase
for a restart), with
every dispatch recorded intent -> result (`emulator-control.md`); the hidden
`api` subcommand stays banned, and the resolved `MuMuManager.exe` path is
carried on each discovered binding for that purpose. `MuMuManager.exe` is
resolved in this priority:
the configured `mumu_root`, `ACTINGCOMMAND_NEMU_FOLDER` (only under
`allow_env_overrides`: `actingd` injects the value, the resolver never reads the
environment; see `contracts/actingd-check-config.md`, "Environment
overrides"), the install root of a running MuMu process, the Windows uninstall
entry, then vendor folder enumeration. The registry tier enumerates `MuMuPlayer*` subkeys under
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
`failed` observation with stage `instance_discovery` before Host startup fails
with its classification: `instance_discovery_unavailable` (tool, spawn, exit,
decode or JSON failure, including a missing install),
`mumu_manager_version_unsupported` (below the policy floor or unparseable),
`instance_discovery_no_match` (no reported instance has the index or exact
name), `instance_discovery_ambiguous` (more than one instance carries the name)
and `instance_discovery_conflict` (a declared `host` or `port` differs from the
discovered value, codes `host_conflict` and `port_conflict`; a declared
`adb_path` is neither canonically the discovered MuMu adb nor, when the daemon
runs from an install root, that root's adb, code `adb_path_conflict`; or a
`port` is declared for a matched instance that is stopped and therefore cannot
be cross-checked, code `port_unverifiable`). Ledger revision 3 (Workflow #378
A3): a refusal of the tool itself (module `actingcommand_device::mumu_manager`)
records the device failure as its outcome (a Nemu resolution reason with its
context values, else `provider_native_failed` with the device failure's
category), with the configured `mumu_root` when there is one and the device's
text as a `foreign_other_error` link. A no-match, ambiguous or conflict
refusal (module `actingd.config`) records its code with the instance's `alias`
and the declared value (`instance_index` or `instance_name`, `adb_host`,
`adb_port` or `adb_path`) as values and, except for no match, what discovery
reported for the instance as `instances` records (the candidates of an
ambiguous name); an adb path conflict also names the discovered adb as
`other_path`. It follows the `instance_discovery` record, which holds the whole
discovery. A resolved running instance is then
registered exactly like an explicit one, with the discovered host and port, no
serial and the discovered MuMu adb, or the install root's adb when `adb_path`
is absent or names it and the daemon runs from an install root (Workflow #337;
that adb was hashed before the ledger opened, see
`contracts/actingd-check-config.md`, "Default ADB"). A resolved instance that is stopped (it reports no ADB
endpoint) is registered with a PENDING binding instead of being refused: the
discovered facts plus the host the binding will be completed with (the reported
`adb_host_ip` if any, else the declared `host`, else `127.0.0.1`), but no port;
it is never bound with a guessed port, and `actingctl emulator start` resolves
the port (`emulator-control.md`, "Cold start"). Startup discovery succeeds at
most once per startup and startup re-probes nothing later. A later explicit instance
discovery query (`RuntimeOperation::DiscoverInstances`) re-runs the same
resolution on demand through the provider's `discover_instances` and reports
the provider version and each instance's index, name, ADB host and port,
`running` and `android_version`; it never binds, rebinds or registers anything,
records no startup observation, and refuses with the same
`instance_discovery_unavailable` / `mumu_manager_version_unsupported`
classification (a provider without discovery refuses with
`instance_discovery_unavailable`).

### Discovery unavailable at start

Workflow #390 DISC-1 (coordinator ruling 2 of wave 18; [v012-register.md](v012-register.md)
§1.89). After a boot the Runtime may start before the emulator, and the discovery then
answers `instance_discovery_unavailable`. The start tries it again, inside the same
`instance_discovery` bracket, at most `startup_discovery.attempts` times (the first
included) and `startup_discovery.retry_interval_ms` apart on the Runtime clock
([runtime-config.md](runtime-config.md), "The `startup_discovery` section"; defaults 25
attempts, 10 s apart, so about four minutes cover a normal cold start of the emulator).
Each failed attempt that is tried again is recorded, before the wait, as one
`discovery_retried` observation (backend `mumu_manager`) whose `outcome` is
`instance_discovery_retried` (warning; `attempts` the number of the attempt that failed,
`limit_count` the configured attempts, `retry_after_ms` the wait), with the attempt's failure
as its `caused_by` link (`instance_discovery_unavailable` over the device failure). The same
warning is written to actingd's stderr as its daemon lines, one developer line
(`WARN actingd: instance_discovery_retried attempts=<n> limit_count=<attempts>
retry_after_ms=<wait> [...]`) and one `OUTCOME actingd: <envelope>` line, so a person or a
start script reading the log sees the start wait; a failed write never stops the start. After
each wait the install preparation boundary is checked, as at a stage start. An attempt that
succeeds goes on exactly as a first one does. Any other refusal
(`mumu_manager_version_unsupported`), or the last attempt's, is recorded as the `failed`
observation and fails the start as before. Only the startup path retries; the explicit
`DiscoverInstances` query refuses at once, as before.

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
`actingcommand_runtime_host::emulator_control`; outcome
`emulator_capability_admission_refused` with the reported
`mumu_manager_version`, the configured `mumu_root` when there is one, and the host's
admission code as its `caused_by` link) before Host startup fails with
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
the instance is addressed by a serial (a configured `serial`, or a handset's bound
serial); such an instance records no host and no port (Workflow #398 RD-1b), and
the serial itself is never recorded or parsed. A handset also records `device`:
`{kind, fingerprint, bound_at_unix_ms, bound_via}`, the fingerprint being the
ledger's salted SHA-256 of the serial, or only `{kind}` while the private binding
file binds no device to it ([device-binding.md](device-binding.md)); an emulator
records no `device`. An instance with no ADB target, including a fixture simulation,
omits host and port, and so does a discovered binding that is still pending
(the instance was stopped at startup): it carries the discovered index, name
and provider version with `binding_source` `discovered` and no host or port.
Emulator control appends one more `runtime.instance_bound` for that instance,
with host and port, once a successful `start` / `restart` resolved them
(`emulator-control.md`, "Cold start"); the newest event's port represents the
instance from then on. A serial-configured instance, like one without a port,
is excluded from port grouping: its port never identifies it in the ledger port
map or in the `status` `adb_port` field. A failed append is fatal, as for
`runtime.started`.

Once an instance is bound, each of its backend opens is recorded as one
`runtime.lifecycle_observed` event with phase `backend_open_observed`
(`backend-open-observation.md`), and right after it the host records that
report as the instance's four `backend.selfcheck.<entry>.*` runtime facts
(`runtime-fact-store.md`, "Producers") and, for a device self-checked instance
(physical, with a device endpoint), one `device.self_check` status hint
(`backend-open-observation.md`, "Device self-check event"); a physical policy
instance is available only while that self-check passes
(`instance-fact-store.md`, "Backend self-check availability"). An explicitly
selected capture backend, like an automatic one, primes its first frame at open
and the first capture returns that frame (`backend-open-observation.md`,
"Primed first frame"), so the open's self-check already carries the capture
check.

Startup sequence (Workflow #317 sc3): after the provider is assembled, the
registry is read, the `runtime.instance_bound` events are appended and the
fact seeds, configuration manifest and settlement facts are recorded, the host
runs the connection preparation phase (`runtime-fact-store.md`, "Connection
preparation phase") for every registered device self-checked instance in
registry order, before any thread is spawned and so before actingd's
`runtime_ready` daemon line (`OUTCOME actingd:` with its developer line,
Workflow #378 A6a):
the instance's availability is withdrawn, a dedicated preparation lease is
granted, its input and capture backends are opened (a Nemu pair once) and
recorded (`backend_open_observed`, facts, `device.self_check`), and the session
is closed and the lease released again. An instance with a Nemu pair (touch
backend `nemu_ipc`) therefore shows one `device.self_check` (`nemu`) before
`runtime_ready`, one with independent backends two (`input`, `capture`). A failed preparation leaves its instance unavailable and does
not stop the start; only a fatal failure (a ledger append, an unconfirmed
close) does.

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
