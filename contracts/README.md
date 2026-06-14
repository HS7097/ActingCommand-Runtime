# ActingCommand Runtime Contracts

These files are the P0a contracts between the runtime decision/data core, UI, and future execution layer.

## Files

- `runtime-api.openapi.yaml` — local HTTP API reserved for the UI.
- `runtime-events.schema.json` — WebSocket event envelope and payload schema.
- `task-flow.schema.json` — declarative task-flow schema.
- `sqlite/schema.sql` — initial SQLite schema for profiles, runs, resources, acquisition captures, logs, and manifests.
- `server-keys.md` — persisted server variant key policy.
- `primitive-service.md` — language-neutral execution-layer boundary for Rust or other worker implementations.

## Go boundary

The compile-time Go interfaces live in:

- `pkg/contract/primitive.go`
- `pkg/contract/game_engine.go`
- `pkg/contract/taskflow.go`
- `pkg/contract/types.go`

The UI must use the runtime API and must not own the runtime lifecycle. The execution layer should satisfy `PrimitiveLayer` through a Go adapter and may be implemented by a Rust worker or another process. It returns structured observations and image references, not raw frame buffers.
