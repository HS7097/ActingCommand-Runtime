# actingd configuration check

`actingd check-config` validates a configuration file exactly as startup would
and stops before the first side effect. It runs the configuration load, the
typed assembly of `actingcommand.actingd.config.v1` and
`RuntimeHostConfig::validate`, the same checks as startup's first step, then
drops the assembly. It never stats, creates or reads anything under
`state_root`, never opens the ledger, never acquires `owner.lock`, never binds a
socket and records no lifecycle failure. A passing check is not a startup: the
daemon's own startup path remains the only authority on the state root and the
vision provider manifest.

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
{"schema_version":"actingcommand.actingd.check-config.v1","status":"ok","config_path":"runtime.json","state_root":"D:/runtime/state","bind_host":"127.0.0.1","bind_port":0,"instance_count":3,"instances":[{"alias":"fixture.b","mode":"fixture_simulation","binding":"explicit","adb_host":null,"adb_port":null,"startup_package":null},{"alias":"mumu.c","mode":"device_registry","binding":"discovery_pending","instance_index":1,"instance_name":null,"startup_package":{"package":"D:/runtime/packages/neutral-startup.zip","expected_sha256":"<64 hex>"}},{"alias":"node.a","mode":"device_registry","binding":"explicit","adb_host":"127.0.0.1","adb_port":16384,"startup_package":null}],"policy_configured":false,"config_manifest":{"subsystems":[...],"parameters":[...]},"not_checked":["vision_provider_manifest","state_root"]}
```

`config_manifest` for a zero-instance configuration that names only
`bind_host`, `bind_port` and the salt (the parameter list is shortened here to
one entry per group; the real object carries every key listed below):

```json
{"subsystems":[{"name":"frame_retention","enabled":false,"reason":"flag absent"},{"name":"agent_dispatcher","enabled":false,"reason":"section absent"},{"name":"governance","enabled":false,"reason":"capability absent"},{"name":"policy_driver","enabled":false,"reason":"section absent"},{"name":"vision_provider","enabled":false,"reason":"manifest absent"},{"name":"device_diagnostic","enabled":true,"reason":"always on; mode shadow"},{"name":"performance_monitor","enabled":true,"reason":"sample interval 2000 ms (default)"},{"name":"mumu_discovery","enabled":false,"reason":"no instance bound by instance_index or instance_name"},{"name":"emulator_control","enabled":false,"reason":"no discovery-bound instance"},{"name":"runtime_fact_snapshot","enabled":true,"reason":"rides the performance monitor thread"}],"parameters":[{"key":"bind_host","value":{"type":"string","value":"127.0.0.1"},"source":"explicit"},{"key":"bind_port","value":{"type":"integer","value":0},"source":"explicit"},{"key":"device_diagnostic_mode","value":{"type":"string","value":"shadow"},"source":"default"},{"key":"frame_retention_enabled","value":{"type":"boolean","value":false},"source":"default"},{"key":"secret_fingerprint_salt_bytes","value":{"type":"integer","value":64},"source":"explicit"},{"key":"instances_count","value":{"type":"integer","value":0},"source":"explicit"},{"key":"instances_deferred_count","value":{"type":"integer","value":0},"source":"explicit"},{"key":"instances_startup_package_count","value":{"type":"integer","value":0},"source":"explicit"},{"key":"scheduler.lease_ttl_ms","value":{"type":"duration_ms","value":120000},"source":"default"},{"key":"policy_cadence.debounce_ms","value":{"type":"duration_ms","value":250},"source":"default"},{"key":"io_timeout_ms","value":{"type":"duration_ms","value":5000},"source":"default"},{"key":"maximum_frame_bytes","value":{"type":"integer","value":1048576},"source":"default"},{"key":"performance_control.escalation_samples","value":{"type":"integer","value":2},"source":"default"},{"key":"performance_monitor.sample_interval_ms","value":{"type":"duration_ms","value":2000},"source":"default"},{"key":"capacity_thresholds.hard_bytes","value":{"type":"integer","value":536870912},"source":"default"},{"key":"mumu_manager.control_timeout_ms","value":{"type":"duration_ms","value":60000},"source":"default"}]}
```

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
  `startup_package_invalid`.
- `policy_configured` states whether a `policy` section was assembled.
- `config_manifest` is the in-memory runtime configuration manifest exactly
  as `assemble` hands it to the host (`RuntimeConfigManifest`, see
  `contracts/runtime-fact-store.md`, "Producers"); at startup the daemon
  records the same content as the program facts `config.subsystems` and
  `config.parameters`, which `actingctl facts --program` returns. Printing it
  here has no side effect.
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
    `frame_retention_enabled`, `secret_fingerprint_salt_bytes` (the byte
    length only; the salt itself is never printed), `mumu_root` (only when
    set), `instances_count`, `instances_deferred_count`,
    `instances_startup_package_count` (instances declaring a startup package), the
    `capacity_thresholds.*` bytes and, when the section is present, the
    `agent_dispatcher.*` budget; plus the library defaults the daemon applies
    without a file field: `scheduler.*`, `policy_cadence.*`, `io_timeout_ms`,
    `maximum_frame_bytes`, `performance_control.*`,
    `performance_monitor.sample_interval_ms` and `mumu_manager.*`. `source`
    is `explicit` when the file named the value and `default` otherwise;
    `discovered` is reserved and not produced yet. A `value` is a typed
    scalar (`string`, `integer`, `boolean`, `duration_ms`).
- `not_checked` is a fixed list of what this command cannot validate:
  `vision_provider_manifest` (only read and validated inside host startup,
  which records `provider.startup_observed`) and `state_root` (nothing under it
  is inspected).

```json
{"schema_version":"actingcommand.actingd.check-config.v1","status":"failed","error":{"code":"config_decode_failed","stage":"load"}}
```

`error.code` is the code startup would fail with. `error.stage` is `load`
(`config_unavailable`, `config_size_invalid`, `config_read_failed`,
`config_decode_failed`), `assemble` (the typed configuration codes, for example
`config_invalid`, `bind_host_not_loopback`, `execution_registry_invalid`,
`instance_binding_key_invalid`, `mumu_root_invalid`,
`scheduled_execution_instance_unknown`, `policy_governance_capability_missing`,
`config_manifest_value_out_of_range`, `config_manifest_invalid`)
or `validate` (`invalid_runtime_host_config`,
`invalid_runtime_config_manifest` and the other
`RuntimeHostConfig::validate` codes). The secret fingerprint salt and the
governance capability bytes are never printed.

## Exit code

`0` only when `status` is `ok`. A failed check prints its JSON object, then the
normal `FATAL actingd: <code>` line on stderr, and exits `1`.
