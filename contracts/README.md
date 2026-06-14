# ActingCommand Runtime Contracts

These files are the P0a contracts between the runtime decision/data core, UI, and future execution layer.

## Files

- `runtime-api.openapi.yaml` — local HTTP API reserved for the UI.
- `runtime-events.schema.json` — WebSocket event envelope and payload schema.
- `task-flow.schema.json` — declarative task-flow schema.
- `sqlite/schema.sql` — initial SQLite schema for profiles, runs, resources, acquisition captures, logs, and manifests.

## Go boundary

The compile-time Go interfaces live in:

- `pkg/contract/primitive.go`
- `pkg/contract/game_engine.go`
- `pkg/contract/taskflow.go`
- `pkg/contract/types.go`

The UI must use the runtime API and must not own the runtime lifecycle. The execution layer should satisfy `PrimitiveLayer` and return structured observations, not raw frame buffers.

