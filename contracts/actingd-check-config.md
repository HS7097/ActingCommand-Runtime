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
{"schema_version":"actingcommand.actingd.check-config.v1","status":"ok","config_path":"runtime.json","state_root":"D:/runtime/state","bind_host":"127.0.0.1","bind_port":0,"instance_count":3,"instances":[{"alias":"fixture.b","mode":"fixture_simulation","binding":"explicit","adb_host":null,"adb_port":null},{"alias":"mumu.c","mode":"device_registry","binding":"discovery_pending","instance_index":1,"instance_name":null},{"alias":"node.a","mode":"device_registry","binding":"explicit","adb_host":"127.0.0.1","adb_port":16384}],"policy_configured":false,"not_checked":["vision_provider_manifest","state_root"]}
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
  runs. Nothing is probed and no serial is parsed.
- `policy_configured` states whether a `policy` section was assembled.
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
`scheduled_execution_instance_unknown`, `policy_governance_capability_missing`)
or `validate` (`invalid_runtime_host_config` and the other
`RuntimeHostConfig::validate` codes). The secret fingerprint salt and the
governance capability bytes are never printed.

## Exit code

`0` only when `status` is `ok`. A failed check prints its JSON object, then the
normal `FATAL actingd: <code>` line on stderr, and exits `1`.
