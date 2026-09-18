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
backend and one observation: stage started/completed, file binding, model
binding, original failure, not configured, or ready. Stages cover manifest
read/parse, path binding, model identity, backend construction and registry
binding. File bindings retain the configured value, resolution base and
resolved value; model bindings retain the logical model reference and digest.
Paths resolve through the existing manifest-parent algorithm. Absolute paths
and native runtime closure ordering keep their existing meanings.

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

## Instance binding

Immediately after `runtime.started` or `runtime.takeover`, the Host records one
`runtime.instance_bound` event per registered instance, ordered by instance id.
The event is family Runtime with severity Info; its sensitivity is derived as
Internal rather than declared. Its links carry the registered `instance_id`, and
its payload carries the registered `instance_alias`, the backend `provenance`,
the configured `adb_host` and `adb_port`, `serial_configured` and
`binding_source`. `binding_source` is `explicit` for a configured registry entry;
`discovered` is reserved for later discovery. The port is a plain payload field,
because an instance is recognised in the ledger by its ADB port; the audit
`device_endpoint` keeps its existing redaction wherever it is carried, and this
event carries no audit endpoint of its own. `serial_configured` states that
an explicit serial was configured, so the recorded host and port are the
configured target rather than the resolved transport serial; the resolved serial
is never parsed. An instance with no ADB target, including a fixture simulation,
omits host and port. A failed append is fatal, as for `runtime.started`.

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
