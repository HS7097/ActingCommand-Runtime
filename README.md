# ActingCommand Runtime

Independent runtime service for ActingCommand.

This repository contains `AliceRuntimeOrchestrator`, the long-lived local runtime process. The UI is a disposable client and must not own this process lifetime.

## Responsibility

- configuration discovery, reading, creation, validation, and modification
- profile-to-runtime resolution
- scheduler and command state
- device and ADB management boundaries
- upstream backend task dispatch
- execution result collection and normalization
- runtime recovery
- log generation and streaming
- resource history recording
- acquisition screenshot metadata indexing

## Runtime boundary

`AliceRuntimeOrchestrator` communicates with the UI through localhost HTTP and WebSocket endpoints.

Default endpoints:

```text
HTTP: http://127.0.0.1:8765
WS:   ws://127.0.0.1:8766/events
```

The runtime must survive UI reload, crash, or close.

## P0a contracts

The current Go direction starts with decision/data contracts before the operation layer is implemented.

- Go interfaces: `pkg/contract`
- UI HTTP contract: `contracts/runtime-api.openapi.yaml`
- UI event contract: `contracts/runtime-events.schema.json`
- task-flow schema: `contracts/task-flow.schema.json`
- SQLite schema: `contracts/sqlite/schema.sql`

The operation layer should implement the primitive API later. It must return structured observations and image references, not raw frame buffers. UI code should use the runtime API and should not open the runtime SQLite database directly.

## Local run

Install dependencies:

```powershell
python -m pip install -r .\runtime\requirements.txt
```

Start the runtime:

```powershell
.\scripts\start-runtime.ps1
```

Stop the runtime:

```powershell
.\scripts\stop-runtime.ps1
```

## State path

The current V1 state path still uses the historical directory:

```text
%LOCALAPPDATA%\GachaPilot\AliceRuntimeOrchestrator
```

Move to an `ActingCommand` state path only through a dedicated migration step.

## License

ActingCommand Runtime is planned under `AGPL-3.0-only`.

Compatible upstream code may be copied, adapted, referenced directly, or refactored inside this repository when license conditions are satisfied. Preserve upstream notices, license texts, source availability, and modification records.
