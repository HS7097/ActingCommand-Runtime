# ActingCommand Runtime Contracts

These files are versioned data and protocol contracts between the runtime decision/data core, UI, and execution layer.

## Files

- `runtime-api.openapi.yaml` — local HTTP API reserved for the UI.
- `runtime-events.schema.json` — WebSocket event envelope and payload schema.
- `task-flow.schema.json` — declarative task-flow schema.
- `sqlite/schema.sql` — initial SQLite schema for profiles, runs, resources, acquisition captures, logs, and manifests.
- `server-keys.md` — persisted server variant key policy.
- `primitive-service.md` — language-neutral execution-layer boundary for Rust or other worker implementations.
- `scheduling/` — frozen four-document scheduling catalog, diagnostics, canonical hash contract, and neutral examples.

## Rust mainline boundary

The Rust mainline contract crate lives in:

- `crates/actingcommand-contract`

The Rust device-layer crate lives in:

- `crates/device`

The Rust scheduling policy contract lives in:

- `crates/policy`

## Historical Go boundary

The historical Go interfaces were moved to:

- https://github.com/HS7097/ActingCommand-Legacy-Runtime

The UI must use the runtime API and must not own the runtime lifecycle. The execution layer must return structured observations and image references, not raw frame buffers.
