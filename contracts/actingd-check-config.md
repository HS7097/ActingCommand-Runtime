# actingd configuration check

`actingd check-config` validates a configuration file exactly as startup would
and stops before the first side effect. It runs the configuration load, the
typed assembly of `actingcommand.actingd.config.v1`,
`RuntimeHostConfig::validate` and the instance resource package admission (see
"Instance resource package"), the same checks as startup's first step, reports
the MuMu install root startup would use (resolved read-only when `mumu_root` is
not configured, see "MuMu install root"), then drops the assembly. It never
stats, creates or reads anything under `state_root`, never opens the ledger,
never acquires `owner.lock`, never binds a socket and records no lifecycle
failure. A passing check is not a startup: the daemon's own startup path
remains the only authority on the state root and the vision provider manifest.

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
resource packages are stat'ed or canonicalized from disk. Relative
`GitSourceTree` package paths resolve against the process working directory,
exactly as startup does, so run the check from the directory the daemon will be
started in. An empty `instances` array passes, as at startup, and describes a
control-plane-only daemon.

## Result

Exactly one JSON object is written to stdout on both outcomes.

```json
{"schema_version":"actingcommand.actingd.check-config.v1","status":"ok","config_path":"runtime.json","state_root":"D:/runtime/state","bind_host":"127.0.0.1","bind_port":0,"instance_count":3,"instances":[{"alias":"fixture.b","mode":"fixture_simulation","binding":"explicit","adb_host":null,"adb_port":null,"startup_package":null,"stuck_recovery":true,"stuck_recovery_cooldown_secs":600},{"alias":"mumu.c","mode":"device_registry","binding":"discovery_pending","instance_index":1,"instance_name":null,"startup_package":{"package":"D:/runtime/packages/neutral-startup.zip","expected_sha256":"<64 hex>"},"stuck_recovery":true,"stuck_recovery_cooldown_secs":1800},{"alias":"node.a","mode":"device_registry","binding":"explicit","adb_host":"127.0.0.1","adb_port":16384,"startup_package":null,"stuck_recovery":false,"stuck_recovery_cooldown_secs":600,"resource_package":{"path":"D:/runtime/packages/neutral.zip","kind":"file"}}],"policy_configured":false,"performance":{"pressure_start_samples":{"value":3,"source":"default"},"pressure_end_samples":{"value":5,"source":"explicit"}},"device_paths":{"nemu_folder":null,"nemu_ipc_dll":{"path":"D:/runtime/MuMuPlayer/nx_device/12.0/shell/sdk/external_renderer_ipc.dll","source":"explicit"},"droidcast_apk":null,"minitouch_path":null,"maatouch_path":null},"config_manifest":{"subsystems":[...],"parameters":[...]},"not_checked":["vision_provider_manifest","state_root"],"mumu_root":{"path":"D:/runtime/MuMuPlayer","source":"config"}}
```

`config_manifest` for a zero-instance configuration that names only
`bind_host`, `bind_port` and the salt (the parameter list is shortened here to
one entry per group; the real object carries every key listed below):

```json
{"subsystems":[{"name":"frame_retention","enabled":true,"reason":"flag absent"},{"name":"agent_dispatcher","enabled":false,"reason":"section absent"},{"name":"governance","enabled":false,"reason":"capability absent"},{"name":"policy_driver","enabled":false,"reason":"section absent"},{"name":"vision_provider","enabled":false,"reason":"manifest absent"},{"name":"device_diagnostic","enabled":true,"reason":"always on; mode shadow"},{"name":"performance_monitor","enabled":true,"reason":"sample interval 2000 ms (default)"},{"name":"mumu_discovery","enabled":false,"reason":"no instance bound by instance_index or instance_name"},{"name":"emulator_control","enabled":false,"reason":"no discovery-bound instance"},{"name":"runtime_fact_snapshot","enabled":true,"reason":"rides the performance monitor thread"}],"parameters":[{"key":"bind_host","value":{"type":"string","value":"127.0.0.1"},"source":"explicit"},{"key":"bind_port","value":{"type":"integer","value":0},"source":"explicit"},{"key":"device_diagnostic_mode","value":{"type":"string","value":"shadow"},"source":"default"},{"key":"frame_retention_enabled","value":{"type":"boolean","value":true},"source":"default"},{"key":"secret_fingerprint_salt_bytes","value":{"type":"integer","value":64},"source":"explicit"},{"key":"instances_count","value":{"type":"integer","value":0},"source":"explicit"},{"key":"instances_deferred_count","value":{"type":"integer","value":0},"source":"explicit"},{"key":"instances_startup_package_count","value":{"type":"integer","value":0},"source":"explicit"},{"key":"scheduler.lease_ttl_ms","value":{"type":"duration_ms","value":120000},"source":"default"},{"key":"policy_cadence.debounce_ms","value":{"type":"duration_ms","value":250},"source":"default"},{"key":"io_timeout_ms","value":{"type":"duration_ms","value":5000},"source":"default"},{"key":"maximum_frame_bytes","value":{"type":"integer","value":1048576},"source":"default"},{"key":"performance_control.escalation_samples","value":{"type":"integer","value":2},"source":"default"},{"key":"performance_monitor.sample_interval_ms","value":{"type":"duration_ms","value":2000},"source":"default"},{"key":"capacity_thresholds.hard_bytes","value":{"type":"integer","value":536870912},"source":"default"},{"key":"mumu_manager.control_timeout_ms","value":{"type":"duration_ms","value":60000},"source":"default"}]}
```

`frame_retention_enabled` defaults to `true` in host construction and daemon
configuration. Omission reports an enabled subsystem with reason `flag absent`
and a `true` parameter from `default`. Explicit `false` reports a disabled
subsystem with reason `configured off` and a `false` parameter from `explicit`.
The first periodic round can process eligible historical frames; rounds do not
require low disk capacity. Existing verified/success/close, summary/settlement,
pin, material-use and intent/outcome protections and round budgets apply.
Explicit `false` disables periodic retention and new periodic evictions; startup
still completes previously committed pending `EvictionIntent` records. The
manifest reports effective configuration, not evidence that a round or deletion
has occurred.

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
  the bare hex digest, or `null` when none is declared. The file is neither
  opened nor hashed here; admission happens when the package runs. A
  `startup_package` on a fixture instance fails assembly with
  `instance_config_invalid`; a non-absolute locator, a digest that is not 64
  lowercase hex digits or a request the contract refuses fail with
  `startup_package_path_invalid`, `startup_package_digest_invalid`,
  `startup_package_invalid`. `stuck_recovery` and `stuck_recovery_cooldown_secs`
  are the instance's effective stuck-recovery ladder settings (slice #316-B4,
  `contracts/emulator-control.md`, "Stuck-recovery ladder"): the configured
  values or the defaults `true` and `600`, on every instance. A cool-down
  outside `1..=86400` fails assembly with `stuck_recovery_cooldown_invalid`;
  `false` turns the ladder off for the instance, and a fixture instance never
  starts one. `resource_package` is present only on an instance
  that declares one: the admitted `{ path, kind }` (see "Instance resource
  package"); an instance without the field carries no `resource_package` key.
- `policy_configured` states whether a `policy` section was assembled.
- `performance` echoes the effective pressure streaks of the performance
  monitor (see "Performance and device paths"): `pressure_start_samples` and
  `pressure_end_samples`, each `{ value, source }` with `source` `explicit`
  when the file's `performance` section named it and `default` otherwise. The
  values are read from the manifest, so they equal the
  `performance_monitor.pressure_*` parameters there and what
  `actingctl status --config` shows after startup.
- `device_paths` echoes the daemon-level device tool paths, always with all
  five names (`nemu_folder`, `nemu_ipc_dll`, `droidcast_apk`,
  `minitouch_path`, `maatouch_path`): `{ path, source: "explicit" }` for a
  configured path, `null` for an absent one (today's environment-variable,
  discovery or bundled-tool behaviour then applies; nothing discovered is
  reported here).
- `config_manifest` is the in-memory runtime configuration manifest exactly
  as `assemble` hands it to the host (`RuntimeConfigManifest`, see
  `contracts/runtime-fact-store.md`, "Producers"); at startup the daemon
  records the same content as the program facts `config.subsystems` and
  `config.parameters`, which `actingctl facts --program` returns whole and
  `actingctl status --config` returns on their own. Printing it here has no
  side effect.
  - `subsystems` (`name`, `enabled`, `reason`): `frame_retention` (the
    `frame_retention_enabled` flag), `agent_dispatcher` (section present),
    `governance` (`governance_capability` present), `policy_driver` (`policy`
    section present), `vision_provider` (`vision_provider_manifest` present),
    `device_diagnostic` (always on; the reason carries the mode),
    `performance_monitor` (always on with the default sample interval),
    `mumu_discovery` and `emulator_control` (on only when at least one
    instance is bound by `instance_index` or `instance_name`; the reason
    carries the count), `runtime_fact_snapshot` (rides the performance monitor
    thread).
  - `parameters` (`key`, `value`, `source`): the effective values of
    `bind_host`, `bind_port`, `device_diagnostic_mode`,
    `frame_retention_enabled`, `frame_retention_failed_run_successes`,
    `frame_retention_failed_run_days`, `secret_fingerprint_salt_bytes` (the byte
    length only; the salt itself is never printed), `mumu_root` (only when
    set), `device_paths.<name>` (only the configured ones, see "Performance
    and device paths"), `instances_count`, `instances_deferred_count`,
    `instances_startup_package_count` (instances declaring a startup package), the
    `capacity_thresholds.*` bytes, the performance monitor's
    `performance_monitor.pressure_start_samples` /
    `performance_monitor.pressure_end_samples` and, when the section is
    present, the `agent_dispatcher.*` budget; plus the values the daemon
    applies without a file field: `scheduler.*`, `policy_cadence.*`,
    `io_timeout_ms`, `maximum_frame_bytes`, `performance_control.*`,
    `performance_monitor.sample_interval_ms` and `mumu_manager.*`. Every
    value is read back from the assembled `RuntimeHostConfig` (Workflow #318,
    cfg2), never copied from a library `Default`; a host that cannot report
    one (no performance monitor configuration installed) fails assembly with
    `config_manifest_incomplete`. `source` is `explicit` when the file named
    the value and `default` otherwise; `discovered` is reserved and not
    produced yet. A `value` is a typed scalar (`string`, `integer`,
    `boolean`, `duration_ms`).
- `not_checked` lists what this command did not validate. It always starts
  with `vision_provider_manifest` (only read and validated inside host startup,
  which records `provider.startup_observed`) and `state_root` (nothing under it
  is inspected). `resource_package_directory_declarations` is appended when at
  least one instance's `resource_package` is a directory: its existence is
  checked, its declarations are not (see "Instance resource package").
  `mumu_discovery` is appended when no MuMu install root could be resolved
  (`mumu_root` is `null`).
- `mumu_root` is always present; `mumu_root_unresolved` only when `mumu_root`
  is `null` (see "MuMu install root").

```json
{"schema_version":"actingcommand.actingd.check-config.v1","status":"failed","error":{"code":"config_decode_failed","stage":"load"}}
```

`error.code` is the code startup would fail with. `error.stage` is `load`
(`config_unavailable`, `config_size_invalid`, `config_read_failed`,
`config_decode_failed`), `assemble` (the typed configuration codes, for example
`config_invalid`, `bind_host_not_loopback`, `execution_registry_invalid`,
`stuck_recovery_cooldown_invalid`, `invalid_pressure_samples`,
`device_path_invalid`,
`instance_binding_key_invalid`, `mumu_root_invalid`,
`scheduled_execution_instance_unknown`, `policy_governance_capability_missing`,
`config_manifest_value_out_of_range`, `config_manifest_invalid`,
`config_manifest_incomplete`),
`validate` (`invalid_runtime_host_config`,
`invalid_runtime_config_manifest`, `invalid_stuck_recovery` and the other
`RuntimeHostConfig::validate` codes) or `resource_package`
(`resource_package_missing`, `resource_package_invalid`), in that order. The
secret fingerprint salt and the governance capability bytes are never printed.

A `resource_package` failure also carries `error.detail`; no other stage does:

```json
{"schema_version":"actingcommand.actingd.check-config.v1","status":"failed","error":{"code":"resource_package_invalid","stage":"resource_package","detail":{"alias":"node.a","path":"D:/runtime/packages/neutral.zip","loader_code":"contained_task_admission_failed","loader_message":"fatal containment error: missing package entry: resources/operations/task/task.json"}}}
```

`detail.alias` is the instance and `detail.path` the absolute path that was
checked. `detail.loader_code` and `detail.loader_message` are the package
loader's own code and message for `resource_package_invalid` on a package file,
and `null` otherwise.

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
  directory, or it is a file that the contained-task package loader
  (`PreparedContainedTask::load`, the loader `actingctl task-run` uses) cannot
  read. The file's own SHA-256 serves as the expected digest, so only the
  package content is judged.
- A directory is checked for existence only. The loader reads a package
  directory solely against a Git source-tree reference
  (`contracts/package-reference.md`), which this field does not carry; hence the
  `resource_package_directory_declarations` entry in `not_checked`.
- Packages declare no application id (`contracts/application-lifecycle.md`), so
  nothing is compared against the instance's `application_id`.

The admitted `{ path, kind }` (`kind` is `file` or `directory`) is echoed here
and reported by the instance status entry (`RuntimeInstanceStatus`,
`ProjectInstanceView`) as `resource_package`, omitted when none is configured.
Nothing else consumes it yet.

## Performance and device paths

Workflow #318 (cfg2) adds two optional top-level sections. Both are checked
here exactly as at startup, at stage `assemble`.

`performance { pressure_start_samples, pressure_end_samples }` sets the
performance monitor's pressure streaks (`PerformanceMonitorConfig` in
`crates/runtime-host/src/performance.rs`): the consecutive samples above a start
threshold before a pressure is recorded, and below an end threshold before it
is ended. Each is optional, default `3`, range `1..=30`; a value outside the
range fails with `invalid_pressure_samples` before host validation. The
effective values appear as `performance` here and as the manifest parameters
`performance_monitor.pressure_start_samples` /
`performance_monitor.pressure_end_samples` (`explicit` when named).

`device_paths { nemu_folder, nemu_ipc_dll, droidcast_apk, minitouch_path,
maatouch_path }` names the device tool paths every device instance's backend
configuration receives: `nemu_folder` and `nemu_ipc_dll` become
`NemuIpcConfig.nemu_folder` / `.dll_path` (the explicit MuMu root and capture
DLL of Nemu IPC capture and input), `droidcast_apk` becomes
`DroidcastRawConfig.local_apk`, and `minitouch_path` / `maatouch_path` become
the `MinitouchConfig` / `MaaTouchConfig` `local_path`. Each is optional; a set
path must be absolute and exist (`device_path_invalid` otherwise; nothing is
opened, resolved or compared against `mumu_root`). An absent path leaves
today's behaviour unchanged: the `ACTINGCOMMAND_NEMU_FOLDER`,
`ACTINGCOMMAND_NEMU_IPC_DLL`, `ACTINGCOMMAND_DROIDCAST_RAW_APK` and
`ACTINGCOMMAND_MINITOUCH_PATH` environment variables, MuMu discovery and the
bundled tool lookup. A per-instance `minitouch_local_path` /
`maatouch_local_path` keeps precedence over the daemon-level value. A
discovery-bound instance receives the same paths when it is registered after
discovery. Configured paths are reported as the manifest parameters
`device_paths.<name>` with source `explicit`; unconfigured ones are omitted
from the manifest and `null` in `device_paths` here (no `discovered` value
is produced).

## Discovery-bound instances

An instance bound by `instance_index` or `instance_name` runs through the same
registration function as an explicit entry, with a stand-in ADB target, so a
refusal carries the code startup would report, at stage `assemble`.

Checked here, without discovery:

- the binding key and a declared `serial` (`instance_binding_key_invalid`);
  a non-empty `adb_path` and `host` and a non-zero `port` when declared
  (`instance_config_invalid`);
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
  (`execution_registry_invalid`).

Still deferred to startup, because each needs the discovery result
(`contracts/provider-startup.md`): the `MuMuManager` version floor and
capability admission, exactly one discovered instance matching the key
(`instance_discovery_no_match`, `instance_discovery_ambiguous`), a declared
`adb_path`, `host` or `port` against the discovered values
(`instance_discovery_conflict`; a declared `adb_path` is only compared there,
never resolved here) and the ADB endpoint itself.

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
  startup: `ACTINGCOMMAND_NEMU_FOLDER`, then the install root of a running MuMu
  process, then the Windows uninstall registry entries, then the vendor
  folders. `path` is the canonical root the resolver returns (on Windows a
  `\\?\` verbatim path), which startup accepts as `mumu_root`. `source` is
  `env`, `running_process`, `registry_uninstall` or `vendor_enumeration`.
- `mumu_root` absent and not resolved: `"mumu_root":null` plus a sibling
  `"mumu_root_unresolved":{"reason":"<reason>","message":"<message>"}`, and
  `mumu_discovery` is appended to `not_checked`. This is not a check failure:
  `status` stays `ok` and the exit code `0`; startup runs this resolution only
  when an instance is discovery-bound (`contracts/provider-startup.md`).
  `message` is the resolver's own message. `reason` follows the order of a
  startup discovery refusal's native code: the resolver's resolution reason
  (`installation_absent`, `installation_ambiguous`, `candidate_absent`,
  `candidate_outside_root`, `registry_entry_invalid`,
  `registry_source_unavailable`), else its diagnostic `<category>.<stage>`
  (for example `native.mumu_manager.registry`), else `device_error` (for
  example a root path that cannot be canonicalized).

```json
{"mumu_root":null,"mumu_root_unresolved":{"reason":"installation_absent","message":"no MuMu installation was found: configure mumu_root, set ACTINGCOMMAND_NEMU_FOLDER, start MuMu, or install it at a registered or vendor path"}}
```

The resolution is read-only: it reads one environment variable, the process
list (on Windows one `Get-CimInstance Win32_Process` query through PowerShell,
as startup does), the uninstall registry keys and the file system. It never
runs `MuMuManager.exe` or ADB, starts or stops no instance and records nothing.
With two installations the winner can differ between runs (a running process
comes first), which is why a configured `mumu_root` is preferred.

## Exit code

`0` only when `status` is `ok`. A failed check prints its JSON object, then the
normal `FATAL actingd: <code>` line on stderr, and exits `1`.
