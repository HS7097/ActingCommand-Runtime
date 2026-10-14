# actingd configuration check

`actingd check-config` validates a configuration file exactly as startup would
and stops before the first side effect. It runs the configuration load, the
typed assembly of `actingcommand.actingd.config.v1`, the check of the install
root's adb (see "Default ADB"),
`RuntimeHostConfig::validate`, the listing of the vision root (see "Vision
root") and the instance resource package admission (see "Instance resource
package"), the same checks as startup's first step, reports
the MuMu install root startup would use (resolved read-only when `mumu_root` is
not configured, see "MuMu install root") and the state of the install root's
adb, then previews startup's catalog plan against the state root (see
"Catalog plan") and drops the assembly. It reads the ledger read-only: it never
writes the database, takes no lock (no owner lock, no slot change) and stages
nothing under `state_root`, binds no socket and records no lifecycle failure, so
it runs beside a running daemon. On a cleanly stopped root SQLite may leave the
database's `-wal` / `-shm` sidecars behind, as every read-only reader of the
database does. A passing check is not a startup: the daemon's own startup path remains
the only authority on the state root, and a vision model's content is read and
hashed only when the model is first used.

## Invocation

```text
actingd check-config --config <path>
```

`--config <path>` is the only option. Argument errors print no result object:
`check_config_usage_invalid` (more than one option pair),
`check_config_option_invalid` (an option other than `--config`, a repeated
`--config`, a dangling flag or an empty path) and `check_config_config_missing`
(no `--config`) follow the normal `FATAL actingd: <code>` line with exit code 1.

Policy sections are assembled as at startup: catalog documents are read and
resource packages are stat'ed or canonicalized from disk. Relative directory
package paths (`GitSourceTree` or `ContentDirectory`) resolve against the process
working directory, exactly as startup does, so run the check from the directory the
daemon will be started in. A procedure binding's `scheduled_execution.package_path` must
name a regular file for a ZIP digest, a directory for a Git source-tree reference, and a
directory or a content container file (`.zip` or `.json`, ASCII case-insensitive; see
"Containers" in `contracts/package-reference.md`, Workflow #336) for a content-directory
reference. A content-directory binding to any other file fails
`procedure_package_container_unsupported`; a path that is not of its reference's kind fails
`procedure_package_not_regular`, as before. Procedure packages are not read here: their
content is admitted when the task runs. An empty `instances` array passes, as at startup,
and describes a control-plane-only daemon.

Workflow #361 C1 moves every policy check startup used to make after catalog activation into
the assembly, so this command and startup refuse the same configurations before anything is
recorded. After the catalog compiles and `catalog_approval_ids` equals the catalog's
`approval_refs` (`policy_catalog_approval_mismatch`, as before):

- every configured approval id must pass the approval decision record's own rule (the
  `approval:` prefix, then only `[a-z0-9._:-]`, at most the identifier bound):
  `policy_catalog_approval_invalid`. A catalog may declare a reference such as
  `daily-v12` or a URL that this rule refuses;
- every catalog task's `procedure_ref` must have a `procedure_manifest` binding:
  `procedure_manifest_entry_missing`;
- the set of `policy.facts.instances[].instance_id` must equal the set of configured
  instance aliases (`policy_instance_set_mismatch`), and every policy instance's `host_id`
  must be in `policy.resources.hosts` (`policy_instance_host_unknown`). These are the rules
  of the first policy cycle's input check (`policy_instance_metadata_untrusted`,
  `policy_resource_metadata_untrusted`), which still runs.

All four codes are stage `assemble`. Startup then plans the catalog transition and checks
the approval history before it activates anything (`contracts/client-interactions.md`).

The optional `policy.catalog_transition` (Workflow #361 A) asks for a switch to another
catalog id or a rollback to a generation that was active before:

```json
{"policy":{"catalog_transition":{"kind":"replace","expected_active_catalog_hash":"sha256:<64 hex>"}}}
```

`kind` must be `replace` (`catalog_transition_kind_unknown`) and the hash a lowercase
`sha256:` digest (`catalog_transition_invalid`), both stage `assemble`; unknown fields are
refused as everywhere else. Whether the transition applies is decided against the ledger
(see "Catalog Lineage" in `contracts/scheduling/README.md`): startup refuses
`catalog_transition_expectation_mismatch` when the expected hash is not the active generation,
`catalog_replace_version_not_newer` for a never-active generation of a catalog id at or below
a version of that id that was active, and `catalog_activation_not_newer` without the field
(its detail names the `replace` transition, expecting the active hash, that would apply).
When present, the manifest carries `policy.catalog_transition.kind` and
`policy.catalog_transition.expected_active_catalog_hash` (`explicit`).

The optional `policy.monitor` (Workflow #397) sets the policy monitor's own connection to its
Runtime: the subscription through which the daemon learns of new ledger events and wakes its
policy driver.

Since Workflow #383 AO-7 the policy monitor wakes the driver from the host's business
notifications and sends no request to its Runtime. The keys below are still read, checked and
reported as described, but nothing uses them until they are retired.

```json
{"policy":{"monitor":{"io_timeout_ms":15000,"retry_limit":5,"retry_backoff_ms":1000,"retry_backoff_max_ms":30000}}}
```

- `io_timeout_ms` (default `15000`, range `1000..=60000`): the I/O timeout of that connection
  (connect, request, receipt). The subscription is answered at once, so the timeout only
  matters while the host is slow to answer; the former 5 s did not cover a host at 100 % CPU.
- `retry_limit` (default `5`, range `0..=100`): how many transient failures in a row the
  monitor retries. A transient failure is a receipt header that did not arrive within
  `io_timeout_ms` (`runtime_receipt_header_failed` with I/O kind `timed_out`, or
  `would_block`) for a request sent to this Runtime. Each retry writes the warning
  `policy_monitor_request_retried` (values `attempts`, `limit_count`, `io_kind`,
  `operation`, `retry_after_ms`, `os_error` when the OS gave one) as a `WARN actingd:` line
  and an `OUTCOME actingd:` line on stderr, waits, and sends the request again on a fresh
  connection from the same cursor, so no event is missed. A request that succeeds starts the
  count again. One failure more than `retry_limit` in a row writes
  `policy_monitor_retries_exhausted` (error, the same values without `retry_after_ms`) and
  stops the daemon with that request's failure, as before: `runtime.failed` at stage
  `runtime_lifecycle_policy_monitor` and the `FATAL actingd: monitor:` line. `0` retries
  nothing.
- `retry_backoff_ms` (default `1000`, range `1..=600000`) and `retry_backoff_max_ms` (default
  `30000`, range `retry_backoff_ms..=600000`; when absent, at least `retry_backoff_ms`): the
  wait after the first failure in a row, doubled after each further one, at most
  `retry_backoff_max_ms`. The daemon keeps checking for a fatal error, a shutdown, its driver
  and an install drain while it waits.

Every other failure of the monitor's request stops the daemon at once, as before, and so does
a connection that reaches a Runtime other than the one the daemon hosts (another owner epoch
or process id in the runtime info file): `policy_monitor_runtime_identity_changed`. Dropping
the timed-out connection may make the Runtime record its own `runtime.failed` (stage
`runtime_ipc_request_read`) when its late answer finds the connection closed. A value outside
its range fails with `policy_monitor_config_invalid`, stage `assemble`; unknown fields are
refused as everywhere else. With a `policy` section the manifest carries
`policy.monitor.io_timeout_ms`, `policy.monitor.retry_limit`,
`policy.monitor.retry_backoff_ms` and `policy.monitor.retry_backoff_max_ms`, each `explicit`
when the file named it. `policy.monitor` is actingd-config revision 5
(`contracts/component-interfaces.md`): an earlier Runtime refuses a file that names it, and a
file without it keeps its revision.

## Result

Exactly one JSON object is written to stdout on both outcomes.

```json
{"schema_version":"actingcommand.actingd.check-config.v1","status":"ok","config_path":"runtime.json","state_root":"D:/runtime/state","bind_host":"127.0.0.1","bind_port":0,"instance_count":3,"instances":[{"alias":"fixture.b","mode":"fixture_simulation","binding":"explicit","adb_host":null,"adb_port":null,"startup_package":null,"stuck_recovery":true,"stuck_recovery_cooldown_secs":600},{"alias":"mumu.c","mode":"device_registry","binding":"discovery_pending","instance_index":1,"instance_name":null,"startup_package":{"package":"D:/runtime/packages/neutral-startup.zip","expected_sha256":"<64 hex>"},"stuck_recovery":true,"stuck_recovery_cooldown_secs":1800},{"alias":"node.a","mode":"device_registry","binding":"explicit","adb_host":"127.0.0.1","adb_port":16384,"startup_package":null,"stuck_recovery":false,"stuck_recovery_cooldown_secs":600,"resource_package":{"path":"D:/runtime/packages/neutral.zip","kind":"file"}}],"policy_configured":false,"performance":{"pressure_start_samples":{"value":3,"source":"default"},"pressure_end_samples":{"value":5,"source":"explicit"}},"device_paths":{"nemu_folder":null,"nemu_ipc_dll":{"path":"D:/runtime/MuMuPlayer/nx_device/12.0/shell/sdk/external_renderer_ipc.dll","source":"explicit"},"droidcast_apk":null,"minitouch_path":null,"maatouch_path":null},"config_manifest":{"subsystems":[...],"parameters":[...]},"config_selection":{"config_sha256":"sha256:<64 hex>","defaults_sha256":"sha256:<64 hex>"},"not_checked":["vision_model_content","instruction_values"],"mumu_root":{"path":"D:/runtime/MuMuPlayer","source":"config"},"adb_default":null,"vision":null,"warnings":["env_override_ignored:ACTINGCOMMAND_ADB_PATH"],"policy_plan":{"state":"read","active":null,"driver":"off","configured":null,"plan":null,"approvals":null,"phase_ms":{"compile":0,"ledger":12,"projection":3,"approvals":1,"total":16}}}
```

`config_manifest` for a zero-instance configuration that names only
`bind_host`, `bind_port` and the salt, with no `ACTINGCOMMAND_*` variable set
(the parameter list is shortened here to one entry per group; the real object
carries every key listed below):

```json
{"subsystems":[{"name":"frame_retention","enabled":true,"outcome":{"code":"config_subsystem_flag_absent","category":"info","values":{}}},{"name":"agent_dispatcher","enabled":false,"outcome":{"code":"config_subsystem_section_absent","category":"info","values":{}}},{"name":"governance","enabled":true,"outcome":{"code":"config_subsystem_always_on","category":"info","values":{}}},{"name":"policy_driver","enabled":false,"outcome":{"code":"config_subsystem_section_absent","category":"info","values":{}}},{"name":"vision_provider","enabled":false,"outcome":{"code":"config_subsystem_section_absent","category":"info","values":{}}},{"name":"device_diagnostic","enabled":true,"outcome":{"code":"config_subsystem_always_on","category":"info","values":{}}},{"name":"performance_monitor","enabled":true,"outcome":{"code":"config_subsystem_always_on","category":"info","values":{}}},{"name":"mumu_discovery","enabled":false,"outcome":{"code":"config_subsystem_discovery_unbound","category":"info","values":{}}},{"name":"emulator_control","enabled":false,"outcome":{"code":"config_subsystem_discovery_unbound","category":"info","values":{}}},{"name":"runtime_fact_snapshot","enabled":true,"outcome":{"code":"config_subsystem_always_on","category":"info","values":{}}},{"name":"env_overrides","enabled":false,"outcome":{"code":"config_subsystem_flag_absent","category":"info","values":{}}}],"parameters":[{"key":"bind_host","value":{"type":"string","value":"127.0.0.1"},"source":"explicit"},{"key":"bind_port","value":{"type":"integer","value":0},"source":"explicit"},{"key":"device_diagnostic_mode","value":{"type":"string","value":"shadow"},"source":"default"},{"key":"frame_retention_enabled","value":{"type":"boolean","value":true},"source":"default"},{"key":"secret_fingerprint_salt_bytes","value":{"type":"integer","value":64},"source":"explicit"},{"key":"allow_env_overrides","value":{"type":"boolean","value":false},"source":"default"},{"key":"governance.allowed_clients","value":{"type":"string","value":"any"},"source":"default"},{"key":"instances_count","value":{"type":"integer","value":0},"source":"explicit"},{"key":"instances_deferred_count","value":{"type":"integer","value":0},"source":"explicit"},{"key":"instances_startup_package_count","value":{"type":"integer","value":0},"source":"explicit"},{"key":"scheduler.lease_ttl_ms","value":{"type":"duration_ms","value":120000},"source":"default"},{"key":"policy_cadence.debounce_ms","value":{"type":"duration_ms","value":250},"source":"default"},{"key":"io_timeout_ms","value":{"type":"duration_ms","value":5000},"source":"default"},{"key":"maximum_frame_bytes","value":{"type":"integer","value":1048576},"source":"default"},{"key":"performance_control.escalation_samples","value":{"type":"integer","value":2},"source":"default"},{"key":"performance.monitor.sample_interval_ms","value":{"type":"duration_ms","value":2000},"source":"default"},{"key":"capacity_thresholds.hard_bytes","value":{"type":"integer","value":536870912},"source":"default"},{"key":"mumu_manager.control_timeout_ms","value":{"type":"duration_ms","value":60000},"source":"default"}]}
```

`frame_retention_enabled` defaults to `true` in host construction and daemon
configuration. Omission reports an enabled subsystem with the outcome
`config_subsystem_flag_absent` and a `true` parameter from `default`. Explicit
`false` reports a disabled subsystem with `config_subsystem_configured_off` and
a `false` parameter from `explicit`. Explicit `true` reports
`config_subsystem_configured`.
When enabled, the frame cleaner (Workflow #375 R5d, `contracts/ledger-store.md`,
"Frame cleaner") removes due frames and moves error and Lab frames into
`<state root>\kept\` within 2 s of the start: its first sweep covers every
historical frame that is due, and it does not wait for low disk capacity. It
records nothing in the ledger, so the first removal or move makes the state root
one-way: an earlier Runtime refuses to start on it. Explicit `false` runs no
sweep, and actingd prints `actingd frame_retention disabled` once at start;
startup still completes previously committed pending `EvictionIntent` records.
The manifest reports effective configuration, not evidence that a sweep or
deletion has occurred.

`frame_retention_failed_run_successes` (K, default `3`, range `1..=1024`) and
`frame_retention_failed_run_days` (T, default `7`, range `1..=36500`) configure
eligibility for failed/cancelled run frames. Either K later successful runs with
distinct RunIds on the same InstanceId, or T days since the original terminal's
ledger timestamp, satisfies the status condition. Both are integer configuration
parameters; omission and explicit values retain their independent `default` or
`explicit` sources in the manifest. Invalid values fail configuration assembly,
even when periodic retention is disabled. Days convert to milliseconds within
`u64`; a backward clock cannot supply a negative elapsed duration as an expiry.
Confirmed close, summary/settlement, material binding and permanent evidence
protections still apply. GlobalLedger seals the effective K/T and original
terminal with the chosen eligibility basis in the original eviction intent.
Since Workflow #375 R5d the cleaner admits no eviction intent, so K and T gate
nothing; both keys are still validated and reported.

`frame_retention_dedup_error` (boolean, default `true`) and
`frame_retention_dedup_lab` (boolean, default `false`) are the frame classes'
two dedup switches (Workflow #375 R5c, `contracts/ledger-store.md`, "Frame
classes"). Ordinary frames are always deduplicated and have no switch. With
`frame_retention_dedup_error` on, each 30 s error window drops its interior
near-duplicates; with `frame_retention_dedup_lab` on, Lab output drops its
interior near-duplicates, except Lab operation evidence. A change applies only
to frames not yet moved into a kept folder. Omission and explicit values keep
their `default` or `explicit` sources in the manifest. They are actingd-config
revision 4 (`contracts/component-interfaces.md`): a Runtime up to v0.11.4
refuses a file that names either key, and a file that names neither keeps its
revision.

- `config_path` is the path as given; `state_root` is the configured value,
  neither resolved nor inspected.
- `bind_port` `0` means the OS chooses the listening port.
- `instances` lists the assembled registry in alias order. `mode` is
  `device_registry` or `fixture_simulation`. `binding` is `explicit` for a
  configured entry, whose `adb_host` and `adb_port` are the configured ADB
  target of a device entry and `null` for a fixture entry, or
  `discovery_pending` for an instance bound by `instance_index` or
  `instance_name`, which echoes that key (the other key is `null`) and carries
  no ADB fields: the target is completed by one `MuMuManager` discovery inside
  host startup (see `contracts/provider-startup.md`), which this command never
  runs. Nothing is probed and no serial is parsed. `startup_package` is the
  instance's `startup_package { package, expected_sha256 }` declaration as
  assembled (slice #316-B3, `contracts/application-lifecycle.md`): the locator
  with a relative path resolved against the configuration file's directory and
  the bare hex digest, or `null` when none is declared. When the locator's last
  segment equals that digest (a content directory named by its digest, Workflow
  #288), `expected_sha256` is the content-directory reference object
  (`contracts/package-reference.md`) instead of the bare digest. The package is
  neither opened nor hashed here; admission happens when the package runs. A
  `startup_package` on a fixture instance fails assembly with
  `instance_config_invalid`; a non-absolute locator, a digest that is not 64
  lowercase hex digits or a request the contract refuses fail with
  `startup_package_path_invalid`, `startup_package_digest_invalid`,
  `startup_package_invalid`. `stuck_recovery` and `stuck_recovery_cooldown_secs`
  are the instance's effective stuck-recovery ladder settings (slice #316-B4,
  `contracts/emulator-control.md`, "Stuck-recovery ladder"): the configured
  values or the `recovery_ladder` section's `enabled` and `cooldown_secs`
  (Workflow #387 D1c; defaults file `true` and `600`), on every instance. A cool-down
  outside `1..=86400` fails assembly with `stuck_recovery_cooldown_invalid`;
  `false` turns the ladder off for the instance, and a fixture instance never
  starts one. The same values are the manifest parameters
  `instance.<instance_id>.stuck_recovery` and
  `instance.<instance_id>.stuck_recovery_cooldown_secs` (see `config_manifest`).
  `resource_package` is present only on an instance
  that declares one: the admitted `{ path, kind }` (see "Instance resource
  package"); an instance without the field carries no `resource_package` key.
- `policy_configured` states whether a `policy` section was assembled.
- `performance` echoes the effective pressure streaks of the performance
  monitor (see "Performance and device paths"): `pressure_start_samples` and
  `pressure_end_samples`, each `{ value, source }` with `source` `explicit`
  when the file's `performance` section named it and `default` otherwise. The
  values are read from the manifest, so they equal the
  `performance.pressure_start_samples` / `performance.pressure_end_samples`
  parameters there and what `actingctl status --config` shows after startup.
- `device_paths` echoes the daemon-level device tool paths, always with all
  five names (`nemu_folder`, `nemu_ipc_dll`, `droidcast_apk`,
  `minitouch_path`, `maatouch_path`): `{ path, source: "explicit" }` for a
  configured path, `null` for an absent one (the injected environment variable
  when `allow_env_overrides` is `true`, else discovery or the bundled tool, then
  applies; nothing discovered or injected is reported here).
- `config_manifest` is the in-memory runtime configuration manifest exactly
  as `assemble` hands it to the host (`RuntimeConfigManifest`, see
  `contracts/runtime-fact-store.md`, "Producers"); at startup the daemon
  records it as the program facts `config.subsystems` and
  `config.parameters.<section>`, which `actingctl facts --program` returns
  whole and `actingctl status --config` returns on their own. Every parameter
  carries `settable` (Workflow #387 D1b). Printing it here has no side effect.
- `config_selection` (Workflow #387 D1b): what startup records as the program
  fact `config.selection`: `config_sha256` and `defaults_sha256`
  (`sha256:<hex>`) and, for an installed Runtime, `config_generation`. The
  exact-SHA build compares `defaults_sha256` with the shipped
  `actingd.defaults.json`. This command reads only the files: the instruction
  values a running Runtime holds are not read, and `not_checked` names
  `instruction_values` ([runtime-config.md](runtime-config.md)).
  - `subsystems` (`name`, `enabled`, `outcome`): `outcome` says why the
    subsystem is on or off, as a registered code (ledger revision 3, Workflow
    #378 A3; it replaces the former prose `reason`, and the schema version stays
    `v1`). It is an outcome in record form, `{ code, category, values, causes }`:
    `category` is `info` for every code below, `values` is always present (`{}`
    when empty) and `causes` only when the outcome has links. The codes:
    - `config_subsystem_configured`: the flag is `true` or the section is
      present.
    - `config_subsystem_configured_off`: the flag is `false`.
    - `config_subsystem_flag_absent`: the file does not name the flag, so its
      default applies.
    - `config_subsystem_section_absent`: the file has no section for the
      subsystem.
    - `config_subsystem_always_on`: the Runtime always runs the subsystem.
    - `config_subsystem_discovery_bound`: at least one instance is bound by
      `instance_index` or `instance_name`; the value `count` is how many.
    - `config_subsystem_discovery_unbound`: no instance is bound that way.

    The subsystems, in this order:
    - `frame_retention`: the `frame_retention_enabled` flag
      (`config_subsystem_configured`, `config_subsystem_configured_off` or
      `config_subsystem_flag_absent`).
    - `agent_dispatcher`: its section (`config_subsystem_configured` or
      `config_subsystem_section_absent`).
    - `governance`: always on (`config_subsystem_always_on`). With a
      `governance` section the value `count` is the size of the effective
      allow-list; without one there is no `count` (any well-formed card is
      accepted). See "Governance".
    - `policy_driver` (the `policy` section) and `vision_provider` (the
      `vision` section): `config_subsystem_configured` or
      `config_subsystem_section_absent`.
    - `device_diagnostic` and `performance_monitor`: always on
      (`config_subsystem_always_on`). The mode and the sample interval are the
      parameters `device_diagnostic_mode` and
      `performance.monitor.sample_interval_ms`.
    - `mumu_discovery` and `emulator_control`: on only when at least one
      instance is bound by `instance_index` or `instance_name`
      (`config_subsystem_discovery_bound` with `count`, else
      `config_subsystem_discovery_unbound`).
    - `runtime_fact_snapshot`: always on (`config_subsystem_always_on`); it
      rides the performance monitor thread.
    - `env_overrides`: on only when `allow_env_overrides` is `true`
      (`config_subsystem_configured`, `config_subsystem_configured_off` or
      `config_subsystem_flag_absent`). When the flag is off, the outcome
      carries one `note` link per ignored variable: the outcome
      `env_override_ignored` (category `warning`) with the value `variable`.
      See "Environment overrides".

    The `config.subsystems` program fact holds one row per subsystem: `name`,
    `enabled`, `code` (the outcome's code) and each scalar value of the
    outcome under its own key (`count` where present). A row carries no links;
    instead the `env_overrides` row names the `variable` of each `note` link
    under `ignored_variables`, comma-separated in link order, so an ignored
    variable is never silent.
  - `parameters` (`key`, `value`, `source`): the effective values of
    `bind_host`, `bind_port`, `device_diagnostic_mode`,
    `frame_retention_enabled`, `frame_retention_failed_run_successes`,
    `frame_retention_failed_run_days`, `frame_retention_dedup_error`,
    `frame_retention_dedup_lab`, `secret_fingerprint_salt_bytes` (the byte
    length only; the salt itself is never printed), `allow_env_overrides`
    (boolean, default `false`), `governance.allowed_clients` (string: the
    effective allow-list joined with `,`, which always includes
    `actingd-policy-driver`, or `any`; `explicit` when the file has a
    `governance` section, see "Governance"), `mumu_root` (only when
    set), `device_paths.<name>` (only the configured ones, see "Performance
    and device paths"), `instances_count`, `instances_deferred_count`,
    `instances_startup_package_count` (instances declaring a startup package),
    the per-instance keys below, the
    `capacity_thresholds.*` bytes, every key of the `performance` section
    (Workflow #385 P-c, [runtime-config.md](runtime-config.md), "The performance
    section"; the pressure streaks are `performance.pressure_start_samples` /
    `performance.pressure_end_samples`), with a `policy` section the
    `policy.monitor.*` settings (Workflow #397, described above) and, when the section is
    present, the `agent_dispatcher.*` budget; plus the values the daemon
    applies without a file field: `scheduler.*`, `policy_cadence.*`,
    `io_timeout_ms`, `maximum_frame_bytes`, `performance_control.escalation_samples`,
    `.recovery_samples`, `.transition_cooldown_ms` and `mumu_manager.*`. Per
    configured instance, in declaration order (Workflow #318, cfg3):
    `instance.<instance_id>.stuck_recovery` (boolean) and
    `instance.<instance_id>.stuck_recovery_cooldown_secs` (integer), keyed by
    the registry's bounded `instance_id` (`instance_<32 hex>`; an alias may
    exceed the 128-byte key bound), each `explicit` when the instance named
    the field; since Workflow #387 D1b the configuration mechanism resolves
    them (`instances[]` entry with the same `instance_id`); a field the
    instance does not name takes the `recovery_ladder` section's value. They form
    the section `instance`, which with every other section counts against its own
    256-parameter bound ([runtime-config.md](runtime-config.md)). Workflow #387
    D1c adds, before the per-instance keys, the sections the mechanism resolves:
    the 15 `recovery_ladder.*` keys and `host_claims.renewal_slack_ms` (settable,
    `default` from `actingd.defaults.json` unless the file names them; a list is
    one parameter of canonical JSON text) and `standard_packs.root` when the
    file names it (not settable). A `recovery_ladder` section that breaks a rule
    fails assembly with `recovery_ladder_config_invalid`
    ([runtime-config.md](runtime-config.md), "The recovery ladder section"). Every
    value of the other keys is read back from the assembled `RuntimeHostConfig` (Workflow #318,
    cfg2), never copied from a library `Default`; a host that cannot report
    one (no performance monitor configuration installed) fails assembly with
    `config_manifest_incomplete`; a key the configuration key table does not
    have fails it with `config_key_unknown`, and a section that fails its
    whole-section check with `config_value_invalid`. `source` is `explicit` when the file named
    the value and `default` otherwise; `discovered` is reserved and not
    produced yet (`instruction` exists only in the running Runtime's facts).
    `settable` says whether an instruction may change the key while the
    Runtime runs. A `value` is a typed scalar (`string`, `integer`,
    `boolean`, `duration_ms`).
- `not_checked` lists what this command did not validate. It always starts
  with `vision_model_content` (a model's files are read, hashed and loaded only
  on its first use, see "Vision root") and `instruction_values` (the settings a
  running Runtime holds by instruction; Workflow #387 D1b). `state_root_absent` is appended when
  the configured state root does not exist (see "Catalog plan"); an existing
  state root is read. Since Workflow #336 PKG-S1 every `resource_package` is
  admitted in full by its path (see "Instance resource package"), so no package
  entry is ever added here.
  `mumu_discovery` is appended when no MuMu install root could be resolved
  (`mumu_root` is `null`).
- `mumu_root` is always present; `mumu_root_unresolved` only when `mumu_root`
  is `null` (see "MuMu install root").
- `adb_default` is always present: `null` when the daemon does not run from an
  install root, otherwise `{ path, state }` for the install root's adb, with
  `state` `ok`, `missing` or `sha256_mismatch` (see "Default ADB"). It is
  computed whenever an install root is recognised, also for a configuration
  with no instance; a state other than `ok` fails the check only when an
  instance uses that adb.
- `vision` is always present: `null` without a `vision` section, otherwise
  the listed vision root `{ root, execution_provider, onnxruntime_library,
  ocr_models, nn_models }` (see "Vision root").
- `warnings` is always present: one `env_override_ignored:<VAR>` per
  `ACTINGCOMMAND_*` fallback variable that is set while `allow_env_overrides`
  is off, in the order of "Environment overrides", and empty otherwise. A
  warning is not a failure: `status` stays `ok`. Workflow #373 NAV-6 adds one
  `<code>:<game>` per broken standard-pack game root, and the bare code when
  the root itself cannot be listed (see "Standard packs").
- `standard_packs` is always present (Workflow #373 NAV-6): `{ root,
  root_problem, games }`, `root` `null` without a `standard_packs` section (see
  "Standard packs").

```json
{"schema_version":"actingcommand.actingd.check-config.v1","status":"failed","error":{"code":"config_decode_failed","stage":"load"}}
```

`error.code` is the code startup would fail with. `error.stage` is `load`
(`config_unavailable`, `config_size_invalid`, `config_read_failed`,
`config_decode_failed`), `assemble` (the typed configuration codes, for example
`config_invalid`, `bind_host_not_loopback`, `duplicate_instance_alias`,
`duplicate_instance_id`, `stuck_recovery_cooldown_invalid`, `invalid_pressure_samples`,
`device_path_invalid`,
`instance_binding_key_invalid`, `mumu_root_invalid`,
`scheduled_execution_instance_unknown`, `policy_catalog_approval_mismatch`,
`policy_catalog_approval_invalid`, `procedure_manifest_entry_missing`,
`policy_instance_set_mismatch`, `policy_instance_host_unknown`,
`catalog_transition_kind_unknown`, `catalog_transition_invalid`,
`procedure_package_not_regular`,
`procedure_package_container_unsupported`, `governance_capability_retired`,
`governance_allowed_clients_invalid`, `config_manifest_value_out_of_range`, `config_manifest_invalid`,
`config_manifest_incomplete`, `vision_provider_manifest_retired`,
`vision_config_invalid`, the `prerequisite_package*` and `return_home_package*` codes
of "Prerequisite packages", and after the typed assembly the install root's
adb check `adb_install_missing` / `adb_install_mismatch`, see "Default ADB"),
`validate` (`invalid_runtime_host_config`,
`invalid_runtime_config_manifest`, `invalid_stuck_recovery`,
`invalid_governance_policy` and the other
`RuntimeHostConfig::validate` codes), `vision` (`vision_root_unavailable`,
`vision_root_in_program_slot`, `vision_runtime_unavailable`,
`vision_models_empty`, `vision_model_folder_invalid`, see "Vision root") or
`resource_package` (`resource_package_missing`, `resource_package_invalid`) or
`policy_state` (startup's catalog plan refused, or the state root unreadable, see
"Catalog plan"), in
that order. The
secret fingerprint salt is never printed.

A `resource_package` failure, an `adb_install_missing` /
`adb_install_mismatch` failure (stage `assemble`, see "Default ADB"), a
`vision` failure and a `policy_state` failure (see "Catalog plan") also carry
`error.detail`; no other failure does:

```json
{"schema_version":"actingcommand.actingd.check-config.v1","status":"failed","error":{"code":"resource_package_invalid","stage":"resource_package","detail":{"alias":"node.a","path":"D:/runtime/packages/neutral.zip","loader_code":"contained_task_admission_failed","loader_message":"fatal containment error: missing package entry: resources/operations/task/task.json"}}}
```

`detail.alias` is the instance and `detail.path` the absolute path that was
checked. `detail.loader_code` and `detail.loader_message` are the package
loader's own code and message for `resource_package_invalid` on a package file
or directory the loader refused, and `null` otherwise. Workflow #336 PKG-S1: no
digest is compared, so a changed file is admitted as it now is.

## Catalog plan

Workflow #361 C3. After every other check passed, the command previews the
catalog plan startup would make (see "Catalog Lineage" in
`contracts/scheduling/README.md` and the driver's approval rules in
`contracts/client-interactions.md`), with the same decision and approval
functions. It reads the state root's ledger without the owner lock and without
referenced material (SQLite read-only), and without the whole-file
`PRAGMA quick_check`. In one read transaction it
authenticates the keyed head, checks that the stored sequences are exactly 1 to
the head, and reads and authenticates only the head record and the records the
plan uses: `state.migrated`, the four `catalog.*` transition types and
`approval.decision`, with their link and artifact rows. Other records are not
read, so the preview does not establish their integrity; startup and
`actingd ledger-maintenance verify` check every record. A segment ledger is read
whole. The command then reads the State documents through a read-only view of
the same database, opened the same way, and the catalog generations from their
immutable directories; the configured catalog is compiled in memory, nothing is
staged. Page-level damage in parts of the file the preview does not read is
found by startup and `actingd ledger-maintenance verify`, which run
`PRAGMA quick_check(1)` when they open the database. The ledger read is bounded to
60 seconds, well under the 90 seconds `acsetup` gives this command, so a slow
read fails with the reader's own `ledger_read_budget_exceeded` at stage
`policy_state`. A missing state root is previewed as a fresh one; an existing root
without any state material (`runtime-state.sqlite`, `runtime-state.key`,
`ledger`, `release-blobs`, `artifacts`) reads as a fresh one too.

The result carries `policy_plan`:

```json
{"policy_plan":{"state":"read","active":{"catalog_id":"...","catalog_version":12,"catalog_hash":"sha256:..."},"driver":"on","configured":{"catalog_id":"...","catalog_version":12,"catalog_hash":"sha256:..."},"plan":"unchanged","approvals":{"record":[],"reapprove":[],"revoke":[]},"phase_ms":{"compile":40,"ledger":700,"projection":900,"approvals":300,"total":1940}}}
```

- `state` is `read` or `state_root_absent`.
- `active` is the generation the ledger projects as active (also the last
  active one while the driver is off), or `null`.
- `driver` is `off` without a `policy` section; `configured`, `plan` and
  `approvals` are then `null`.
- `plan` is `unchanged`, `first`, `forward`, `switch` or `rollback`.
- `approvals` lists the approval ids startup would record, approve again and
  revoke.
- `phase_ms` is the time of each phase in milliseconds: `compile` (the
  configured catalog), `ledger` (opening the database without its whole-file
  check, then the head, contiguity and the records above),
  `projection` (the State documents, the catalog lineage and the active
  generation), `approvals` (the approval decisions, the plan and its approvals)
  and `total`. A fresh or absent state root reads no ledger.

A refusal fails the check at stage `policy_state` with the code startup would
fail with (`catalog_activation_not_newer`,
`catalog_transition_expectation_mismatch`, `catalog_replace_version_not_newer`,
`policy_catalog_approval_conflict`, `policy_catalog_approval_invalid`), and an
unreadable state root with the reader's own code, or
`policy_state_ledger_incomplete`, `catalog_active_source_mismatch`,
`catalog_legacy_pointer_pending` (a pre-migration active-pointer file that only
startup migrates). Such a failure carries `error.detail`
`{ "operation": ..., "message": ..., "remedy": ..., "elapsed_ms": ... }`:
`remedy` is the plan refusal's own hint, or `null` (a
`catalog_activation_not_newer` names the `replace` transition, expecting the
active hash, that would switch or roll back instead), and `elapsed_ms` the time
the preview ran before it failed. Since `acsetup --commit-config` runs the
selected slot's `check-config` before it commits, a CAS mismatch, a wrong
approval or an approval conflict stops the commit before the installation or
the running daemon changes.

## Vision root

Workflow #360 replaces `vision_provider_manifest` with the optional top-level
`vision` section; `contracts/vision-model-folders.md` defines the vision root:

```json
"vision":{"execution_provider":"cpu"}
```

`execution_provider` is `cpu` or `cuda`; `cuda` requires `cuda_device
{ ordinal, expected_stable_identity }` and `cpu` refuses it
(`vision_config_invalid`, stage `assemble`). `root` is optional: without it the
vision root is `<install root>\vision` when the daemon runs from an install
root, else `vision` next to the configuration file; a relative `root` resolves
against the configuration file's directory. A file that still names
`vision_provider_manifest`, whatever its value, fails with
`vision_provider_manifest_retired`, and an install selection that still names a
provider file fails loading with `install_provider_retired`.

The check lists the vision root exactly as startup does and reads no model
file: the root must exist (`vision_root_unavailable`) outside the install root's
`A\` and `B\` (`vision_root_in_program_slot`), `models\` must be readable
(`vision_root_unavailable`), `ort\onnxruntime.dll` must exist, with
`ort\onnxruntime_providers_cuda.dll` for `cuda` (`vision_runtime_unavailable`),
and `models\` must hold at least one folder (`vision_models_empty`). A folder
that breaks the folder rule fails the check with `vision_model_folder_invalid`
and `error.detail.invalid_models`, one `{ name, path, reason }` per such folder,
although daemon startup records such a folder and keeps running (only the
targets naming it fail). The other `vision` failures carry
`error.detail.message`.

On success `vision` reports the canonical `root`, the `execution_provider`, the
`onnxruntime_library` path and every model folder by name: `ocr_models`
entries `{ model_ref, layout, detector, description_sha256, languages }`
(`layout` `flat` or `nested`, `detector` whether a detector is present) and
`nn_models` entries `{ model_ref, description_sha256, languages }`.

## Instance resource package

An instance may declare `resource_package`, the local path of its default
resource package: a package file (ZIP) or a package directory. A relative path
resolves against the configuration file's directory, as
`startup_package.package` does, and is then made absolute. The field carries no
digest and URLs are not accepted: whoever writes the configuration downloads the
package first. Daemon startup runs the same admission right after assembly,
before any side effect, and fails with the same codes on the normal
`FATAL actingd: <code>: instance "<alias>" resource_package "<path>"` line
(followed by `; loader <code>: <message>` when the loader refused the file).

- `resource_package_missing`: the path does not exist (or is empty).
- `resource_package_invalid`: the path exists but is neither a file nor a
  directory, or the contained-task package loader (`PreparedContainedTask::load_path`,
  the loader `actingctl task-run` uses, no vision provider, one 60 s deadline)
  refuses it. Workflow #336 PKG-S1: the path chooses the reading (a directory, a
  `.json` or `.zip` container, or a `.zip` assembled package;
  `contracts/package-reference.md`, "Content-directory admission"), admission is
  structural only, no digest is computed or compared and no name is read as an
  identity; a refusal carries the loader's code.
- Packages declare no application id (`contracts/application-lifecycle.md`), so
  nothing is compared against the instance's `application_id`.

The admitted `{ path, kind }` (`kind` is `file` or `directory`) is echoed here
and reported by the instance status entry (`RuntimeInstanceStatus`,
`ProjectInstanceView`) as `resource_package`, omitted when none is configured.
Nothing else consumes it yet.

## Standard packs

Workflow #373 NAV-6 (model M-373 v1.1 §4.3, §7) adds the optional top-level
section `standard_packs` with one key, `root`: the directory that holds one root
per game, `<root>/<game>/` (`contracts/package-reference.md`, "Standard package
root"). The installer writes it once; a relative path resolves against the
configuration file's directory, and an empty one is refused with
`config_decode_failed`. It is a startup key: actingd reads it when it starts,
and a change needs a restart. Without the section no standard-pack root is read.

```json
"standard_packs":{"root":"packages"}
```

This command reads every game root as startup does, root files only and no
package admitted, and reports `standard_packs`:

```json
"standard_packs":{"root":"D:/runtime/packages","root_problem":null,"games":[{"game":"neutral","bundle_version":3,"pack_count":4,"root_files":["bundle.json","pages.json","interrupts.json"],"problem":null},{"game":"other","bundle_version":null,"pack_count":null,"root_files":["bundle.json","interrupts.json"],"problem":{"code":"standard_pack_root_invalid","category":"error","values":{"operation":"standard_pack_read","game":"other","path":"D:/runtime/packages/other/interrupts.json","standard_pack_problem":"pages_missing","json_pointer":""}}}]}
```

A directory that holds none of the root files is not a game root and is not
listed. A broken game root is listed with its `problem` and adds one warning;
`status` stays `ok`, because the configuration itself is valid: the root's
problem is reported by this list and by the startup record, and a run that
needs the root is refused when it is prepared. `root_problem` is set when the
root directory itself cannot be listed (`standard_pack_root_unreadable`).

## Prerequisite packages

Workflow #336 L2b adds the optional top-level `prerequisite_packages`, the
packages a `linear_steps` package may name as its `prerequisite_package_id`
(`contracts/linear-steps.md`, "Prerequisite packages"):

```json
"prerequisite_packages":[{"package_id":"neutral.test.stage_page","package_path":"packages/neutral/<D>.zip","package_digest":{"schema_version":"actingcommand.package.content-directory.v1","sha256":"<D>"}}]
```

Each entry has exactly these three fields. `package_id` is not empty, at most
256 bytes and has no control character (`prerequisite_package_id_invalid`); no
two entries share one (`prerequisite_package_duplicate`); at most as many
entries as a catalog has tasks (`prerequisite_packages_size_invalid`).
`package_digest` takes the forms of a procedure binding's `package_digest`, and
`package_path` is checked as a procedure binding's
`scheduled_execution.package_path` is (a relative path resolves against the
configuration file's directory), under its own codes:
`prerequisite_package_unavailable`, `prerequisite_package_not_regular`,
`prerequisite_package_container_unsupported`,
`prerequisite_package_digest_invalid`, `prerequisite_package_request_invalid`.
All of them fail at stage `assemble`. No package is opened or hashed here: a
run admits each package against its reference when it resolves its chain. The
map needs no `policy` section, so a manual `task-run` uses it too; it is not a
configuration fact (neither the `config_manifest` nor the `config.*` runtime
facts name it) and it is read once at startup, so a change needs a restart. A
build that predates the field refuses it with `config_decode_failed`. A Lab
recording's `record stop` prints the entry of the package it wrote as
`prerequisite_entry_example` (`contracts/lab-recording.md`, "Output").

Workflow #336 L2c adds the optional top-level `return_home_packages`, per game
and server the return-home package a `linear_steps` package without a declared
prerequisite package falls back to (`contracts/linear-steps.md`, "Return-home
fallback"); since L2d the stuck-recovery ladder and the page-graph home entry of a
run whose request binds no recovery package use it too:

```json
"return_home_packages":[{"game":"neutral","server":"test","package_id":"neutral.test.return_home"}]
```

Each entry has exactly these three fields. `game` and `server` are not empty, at
most 64 bytes and have no control character (`return_home_package_key_invalid`);
`package_id` is a `package_id` of `prerequisite_packages`
(`return_home_package_unbound`); no two entries share one game and server
(`return_home_package_duplicate`); at most as many entries as a catalog has
tasks (`return_home_packages_size_invalid`). All of them fail at stage
`assemble`; no package is opened here (a run checks the package's game, server
and resolution when it resolves its chain). Like `prerequisite_packages`, it is
not a configuration fact, it is read once at startup and a build that predates
the field refuses it with `config_decode_failed`.

A scheduled task paused by a failure is lifted once the daemon has restarted
with a configuration in which its procedure binding's `package_digest` changed
or, for a `linear_steps` task, a `prerequisite_packages` entry its paused run
went through maps another digest, or its game and server's
`return_home_packages` entry names another package or that package maps another
digest. `actingd suspended --config <path>` loads and assembles the
configuration exactly as this command does and lists the paused, lifted and
repeating tasks (`contracts/policy-suspension.md`, "Lifting (R19)" and
"`actingd suspended`").

## Performance and device paths

Workflow #318 (cfg2) adds two optional top-level sections. Both are checked
here exactly as at startup, at stage `assemble`.

`performance { pressure_start_samples, pressure_end_samples }` sets the
performance monitor's pressure streaks (`PerformanceMonitorConfig` in
`crates/runtime-host/src/performance.rs`): the consecutive samples above a start
threshold before a pressure is recorded, and below an end threshold before it
is ended. Each is optional, default `3` (from `actingd.defaults.json`), range
`1..=30`; a value outside the range fails with `invalid_pressure_samples` before
host validation. The effective values appear as `performance` here and as the
manifest parameters `performance.pressure_start_samples` /
`performance.pressure_end_samples` (`explicit` when named). Since Workflow #385
P-c the `performance` section holds every key of performance pacing
([runtime-config.md](runtime-config.md), "The performance section"); a rule the
section breaks fails with `perf_config_invalid` at stage `assemble`, as at
startup.

`device_paths { nemu_folder, nemu_ipc_dll, droidcast_apk, minitouch_path,
maatouch_path }` names the device tool paths every device instance's backend
configuration receives: `nemu_folder` and `nemu_ipc_dll` become
`NemuIpcConfig.nemu_folder` / `.dll_path` (the explicit MuMu root and capture
DLL of Nemu IPC capture and input), `droidcast_apk` becomes
`DroidcastRawConfig.local_apk`, and `minitouch_path` / `maatouch_path` become
the `MinitouchConfig` / `MaaTouchConfig` `local_path`. Each is optional; a set
path must be absolute and exist (`device_path_invalid` otherwise; nothing is
opened, resolved or compared against `mumu_root`). An absent path falls back
to the `ACTINGCOMMAND_NEMU_FOLDER`, `ACTINGCOMMAND_NEMU_IPC_DLL`,
`ACTINGCOMMAND_DROIDCAST_RAW_APK` and `ACTINGCOMMAND_MINITOUCH_PATH`
environment variables only under `allow_env_overrides` (see "Environment
overrides"), then to MuMu discovery and the bundled tool lookup. A per-instance
`minitouch_local_path` /
`maatouch_local_path` keeps precedence over the daemon-level value. A
discovery-bound instance receives the same paths when it is registered after
discovery. Configured paths are reported as the manifest parameters
`device_paths.<name>` with source `explicit`; unconfigured ones are omitted
from the manifest and `null` in `device_paths` here (no `discovered` value
is produced).

## Governance

Workflow #318 (cfg4) retires the shared governance secret: governance
connections declare an identity card instead (`contracts/client-interactions.md`,
"Governance connections: the identity card"). The top-level
`governance_capability` key no longer exists. A file that still contains it,
whatever its value (including `null`), fails at stage `assemble` with
`governance_capability_retired`, before any other assembly check, both here and
at startup; delete the key. `policy_governance_capability_missing` is retired
with it: a `policy` section needs no governance setting.

The optional top-level section `governance { allowed_clients }` restricts which
card `client` names the daemon accepts. `allowed_clients` is required inside the
section: at most 32 entries, each following the card's client rule (`1..=64`
bytes of `[A-Za-z0-9._-]`), no duplicates; anything else fails with
`governance_allowed_clients_invalid`. An empty list accepts only the daemon's
own driver. The client `actingd-policy-driver` is always allowed, whatever the
list names: it is the card the daemon's policy driver declares on its own
(User, Ui) connection before it records the configured `catalog_approval_ids`
and revokes the approvals of superseded catalogs.
Without the section any well-formed card is accepted. The effective list is the
manifest parameter `governance.allowed_clients`. Its size is the value `count`
of the `governance` subsystem's outcome, present only with the section (see
`config_manifest`).

```json
{"governance":{"allowed_clients":["ui"]}}
```

gives the parameter value `actingd-policy-driver,ui` (`explicit`) and the
`governance` subsystem outcome
`{"code":"config_subsystem_always_on","category":"info","values":{"count":2}}`,
so its `config.subsystems` row carries `count` `2`.

## Environment overrides

Workflow #318 (cfg3) adds the optional top-level boolean
`allow_env_overrides` (default `false`; manifest parameter
`allow_env_overrides`, `explicit` when named). The device and PPOCR provider
crates no longer read any `ACTINGCOMMAND_*` variable themselves: each former
read point takes a value `actingd` injects (`EnvOverrides` in
`crates/device/src/adb.rs`), and `actingd` fills those values from its
environment only when `allow_env_overrides` is `true`. The variables, in
warning order:

- `ACTINGCOMMAND_ADB_PATH`: preferred over a configured ADB by the device
  crate's ADB resolver. `actingd` itself never hands that resolver an empty
  ADB path: an instance without `adb_path` uses the install root's adb when the
  daemon runs from an install root (see "Default ADB"), otherwise an explicit
  instance is refused and a discovery-bound one takes the discovered MuMu adb;
  an instance with `adb_path` uses the configured one. The injected value
  therefore still has no reader in the daemon.
- `ACTINGCOMMAND_NEMU_FOLDER`: the `MuMuManager.exe` resolver's environment
  rung (startup discovery and "MuMu install root" here) and the Nemu IPC
  capture root when `device_paths.nemu_folder` is absent.
- `ACTINGCOMMAND_NEMU_IPC_DLL`: the Nemu IPC capture DLL when
  `device_paths.nemu_ipc_dll` is absent.
- `ACTINGCOMMAND_DROIDCAST_RAW_APK`: the DroidCast_raw APK when
  `device_paths.droidcast_apk` is absent.
- `ACTINGCOMMAND_MINITOUCH_PATH`: the minitouch path in place of the bundled
  one, below `device_paths.minitouch_path` and a per-instance
  `minitouch_local_path`.
- `ACTINGCOMMAND_PPOCR_NODE_PLACEMENT_DIAGNOSTIC`: sent to the PPOCR ONNX
  provider with each OCR request (`node_placement_diagnostic`, omitted when
  not injected); the provider still requires exactly `1` and an explicit CUDA
  session.

Precedence is otherwise unchanged: configured `device_paths`, then a
per-instance `*_local_path`, then the injected variable (flag on only), then
discovery or the bundled tool. `PATH`, `ProgramFiles`, `ProgramFiles(x86)`
and `SystemRoot` stay discovery sources and are not governed by the flag.

When the flag is off (absent or `false`), every variable above that is set is
ignored and reported. This command's `warnings` array lists it as
`env_override_ignored:<VAR>`. The `env_overrides` manifest subsystem's outcome
carries it as a `note` link, here and in the manifest startup hands to the
host:

```json
{"name":"env_overrides","enabled":false,"outcome":{"code":"config_subsystem_flag_absent","category":"info","values":{},"causes":[{"code":"env_override_ignored","category":"warning","relation":"note","values":{"variable":"ACTINGCOMMAND_ADB_PATH"}}]}}
```

The host records the manifest as the `config.subsystems` program fact. Its
rows keep the code and the scalar values but no links (ledger revision 3,
Workflow #378 A3), so the `env_overrides` row names every ignored variable
under `ignored_variables`, comma-separated in the order above, for example
`"ignored_variables":"ACTINGCOMMAND_ADB_PATH"`. The flag is never silent: an
ignored variable is named both here and in the ledger. No
`provider.startup_observed` kind carries a warning. The variable's value is
never printed. When the flag is `true` nothing is ignored and `warnings` is
empty; a bad injected value fails where it is used (for example
`mumu_root_unresolved` below, or the backend open).

## Discovery-bound instances

An instance bound by `instance_index` or `instance_name` runs through the same
registration function as an explicit entry, with a stand-in ADB target, so a
refusal carries the code startup would report, at stage `assemble`.

Checked here, without discovery:

- the binding key and a declared `serial` (`instance_binding_key_invalid`);
  a non-empty `adb_path` and `host` and a non-zero `port` when declared
  (`instance_config_invalid`);
- at an install root, an absent `adb_path` or one that names the install
  root's adb: that adb is selected here and its files are hashed
  (`adb_install_missing`, `adb_install_mismatch`, see "Default ADB");
- `application_id` (`application_identity_missing`), explicit backends
  (`touch_backend_invalid`, `capture_backend_invalid`,
  `touch_backend_must_be_explicit`, `capture_backend_must_be_explicit`) and
  timeouts (`timeout_invalid`);
- the `nemu_app_index` pairing: allowed only with `touch_backend` and
  `capture_backend` both `nemu_ipc`, which require it
  (`nemu_app_index_requires_paired_input`,
  `nemu_paired_input_configuration_missing`, `nemu_app_index_invalid`);
- alias and application identity as the execution registry accepts them
  (`instance_registration_invalid`), and duplicate aliases or instance ids
  (`duplicate_instance_alias` / `duplicate_instance_id`).

Still deferred to startup, because each needs the discovery result
(`contracts/provider-startup.md`): the `MuMuManager` version floor and
capability admission, exactly one discovered instance matching the key
(`instance_discovery_no_match`, `instance_discovery_ambiguous`), a declared
`host` or `port` against the discovered values and any other declared
`adb_path` against the discovered MuMu adb (`instance_discovery_conflict`),
and the ADB endpoint itself. A declared `adb_path` that names the install
root's adb is recognised and hashed here instead, without the discovery
result; every other declared `adb_path` is only compared at startup, never
resolved here (see "Default ADB").

## Default ADB

Workflow #337 ships the official Android platform-tools in the Tools artifact,
installed as `<install root>\tools\platform-tools\`. An instance without
`adb_path` (key omitted) uses that adb when the daemon runs from an install
root; an empty or blank `adb_path` is still refused.

The install root is judged once per assembly from the running executable
alone: `R` is two levels above the canonical path of the `actingd` executable,
and `R` is an install root only when `R\runtime\BUILD-MANIFEST.json` is a file
(the same evidence acsetup takes for "installed"). The install root's adb is
then `R\tools\platform-tools\adb.exe`. Under the A/B layout an `actingd` in
`<root>\<A|B>\runtime` holds its slot lock, and the install root is the root
`<root>` that the lock names: its adb is `<root>\tools\platform-tools\adb.exe`,
never a slot's (Workflow #359). acsetup runs `check-config` with the `actingd`
of the slot it prepares, so that check sees and hashes the root's adb; acsetup
runs nothing from its staging directory. A development build, the boot smoke of
the exact-SHA build and a staging directory named after the zip files (older
acsetup) are no install root and behave as before.

Explicit instances (`serial`, or `host` and `port`):

| `adb_path` | install root | adb used |
|---|---|---|
| absent | yes | the install root's adb, hashed below |
| absent | no | refused, `instance_config_invalid` |
| empty or blank | either | refused, `instance_config_invalid` |
| names the install root's adb | yes | the install root's adb, hashed below |
| anything else (including a relative path such as `adb`) | either | the configured path, not hashed |

Discovery-bound instances (`instance_index` or `instance_name`):

| `adb_path` | install root | adb used |
|---|---|---|
| absent | yes | the install root's adb, hashed below |
| absent | no | the discovered MuMu adb |
| canonically the discovered MuMu adb | either | the discovered MuMu adb |
| names the install root's adb | yes | the install root's adb, hashed below |
| anything else | either | refused at startup, `instance_discovery_conflict` / `adb_path_conflict`, whose message lists the accepted values |

Whether a declared `adb_path` names the install root's adb needs no discovery
and not even the file: only an absolute path can; it is canonicalized, or,
when it does not exist, its nearest existing ancestor is canonicalized and the
remaining components are joined back; the result is compared with the install
root's adb ignoring case. A removed or quarantined adb therefore still
refuses before the ledger opens. The MuMu adb is never hashed.

When at least one instance uses the install root's adb, the three distributed
binaries `platform-tools/adb.exe`, `platform-tools/AdbWinApi.dll` and
`platform-tools/AdbWinUsbApi.dll` under `R\tools\` are read and their SHA-256
compared with the `distributed_files` pin of
`scripts/windows-tools/windows-tool-sources.v1.json`, the same pin the
exact-SHA build verified the shipped bytes against (it embeds the file at
build time, so there is one source of the expected values). Nothing is
started: a file with the pinned SHA-256 is the file the build ran
`adb version` on. The check runs after the typed assembly, before any side
effect, identically here and at daemon startup:

- `adb_install_missing`: a binary cannot be read (absent, quarantined, not a
  file);
- `adb_install_mismatch`: every binary can be read, but at least one has
  another SHA-256 (or the pin embedded in the build cannot be read, so no file
  can be confirmed).

Both are stage `assemble` here, with `error.detail`, and the startup line
`FATAL actingd: <code>: <message>` before the ledger is opened, so nothing is
recorded. There is no fallback to the MuMu adb. `error.detail` names the adb,
the instances that use it, every file that is not as pinned and the remedy:

```json
{"schema_version":"actingcommand.actingd.check-config.v1","status":"failed","error":{"code":"adb_install_mismatch","stage":"assemble","detail":{"adb_path":"\\\\?\\D:\\AC\\tools\\platform-tools\\adb.exe","instances":["node.a"],"files":[{"path":"\\\\?\\D:\\AC\\tools\\platform-tools\\adb.exe","state":"sha256_mismatch","expected_sha256":"<64 hex>","actual_sha256":"<64 hex>"}],"remedy":"fix: reinstall tools\\platform-tools under this install root (acsetup v0.10 or later for an acsetup install), or set the instance's adb_path to another adb (MuMu's own adb as before; an explicit instance may name any other adb)"}}}
```

A missing file is `{"path":"...","state":"missing","error":"<OS error>"}`.
The FATAL message carries the same facts:
`AC-installed adb "<path>" used by instance(s) ["node.a"]: "<file>" sha256 expected <hex>, actual <hex>; fix: ...`
(`missing "<file>" (<OS error>)` for a missing one).

`adb_default` reports the result whenever an install root is recognised, also
when no instance uses the adb (so a zero-instance smoke check shows it);
outside an install root it is `null`. Paths are canonical (on Windows a `\\?\`
verbatim path). The selected adb reaches the ledger only through the existing
effective device configuration strings; nothing here is a configuration field
or a ledger value.

Rollback to v0.9.0: v0.9.0 requires `adb_path` on every explicit instance and
compares a discovery-bound `adb_path` with the MuMu adb only. Before rolling
back, give every explicit instance without `adb_path` an adb that still exists
afterwards (not the install root's adb, which v0.9.0's Tools do not ship), and
remove an `adb_path` that names the install root's adb from discovery-bound
instances; otherwise v0.9.0 refuses with `instance_config_invalid` or
`instance_discovery_conflict`.

## MuMu install root

`mumu_root` reports the MuMu install root the daemon's `MuMuManager` discovery
would use, so a caller can pin it into the configuration's `mumu_root`. It has
three shapes:

- `mumu_root` configured: `{"path":"<configured path>","source":"config"}`.
  The path is echoed as configured, after the usual assembly check (a relative
  or empty value still fails with `mumu_root_invalid` at stage `assemble`);
  nothing is resolved.
- `mumu_root` absent and resolved:
  `{"path":"<resolved root>","source":"<source>"}`. The command runs
  `resolve_mumu_manager` once with no explicit root, the same resolution as
  startup: `ACTINGCOMMAND_NEMU_FOLDER` (only under `allow_env_overrides`, see
  "Environment overrides"), then the install root of a running MuMu
  process, then the Windows uninstall registry entries, then the vendor
  folders. `path` is the canonical root the resolver returns (on Windows a
  `\\?\` verbatim path), which startup accepts as `mumu_root`. `source` is
  `env` (flag on only), `running_process`, `registry_uninstall` or
  `vendor_enumeration`.
- `mumu_root` absent and not resolved: `"mumu_root":null` plus a sibling
  `"mumu_root_unresolved":{"reason":"<reason>","message":"<message>"}`, and
  `mumu_discovery` is appended to `not_checked`. This is not a check failure:
  `status` stays `ok` and the exit code `0`; startup runs this resolution only
  when an instance is discovery-bound (`contracts/provider-startup.md`).
  `message` is the resolver's own message. `reason` is the resolver's
  resolution reason (`installation_absent`, `installation_ambiguous`,
  `candidate_absent`, `candidate_outside_root`, `registry_entry_invalid`,
  `registry_source_unavailable`), else its diagnostic `<category>.<stage>`
  (for example `native.mumu_manager_registry`), else `device_error` (for
  example a root path that cannot be canonicalized). Only the resolution
  reason matches what a startup discovery refusal records. Since ledger
  revision 3 (Workflow #378 A3) startup records `provider_native_failed` in
  place of the other two, with the value `device_category` when the device
  error has a category. Workflow #378 A2-R aligns this field.

```json
{"mumu_root":null,"mumu_root_unresolved":{"reason":"installation_absent","message":"no MuMu installation was found: configure mumu_root, set ACTINGCOMMAND_NEMU_FOLDER, start MuMu, or install it at a registered or vendor path"}}
```

The resolution is read-only: it uses the injected `ACTINGCOMMAND_NEMU_FOLDER`
value (flag on only), the process list (on Windows one `Get-CimInstance Win32_Process` query through PowerShell,
as startup does), the uninstall registry keys and the file system. It never
runs `MuMuManager.exe` or ADB, starts or stops no instance and records nothing.
With two installations the winner can differ between runs (a running process
comes first), which is why a configured `mumu_root` is preferred.

## Exit code

`0` only when `status` is `ok`. A failed check prints its JSON object, then the
normal `FATAL actingd: <code>` line on stderr, and exits `1`.
