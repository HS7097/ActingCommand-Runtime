# ActingCommand Runtime

Independent runtime service for ActingCommand.

This repository is converging to a Rust mainline runtime. The older Python `AliceRuntimeOrchestrator` remains as legacy/mock material only, and the older Go contracts remain as historical reference and benchmark material only.

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

## Rust workspace

The Rust mainline skeleton starts with contracts and device-layer validation before broad runtime implementation.

- workspace root: `Cargo.toml`
- Rust contracts: `crates/actingcommand-contract`
- device layer: `crates/device`
- MaaTouch device probe: `apps/device-test`

Run Rust checks:

```powershell
cargo test --workspace
```

Run the MaaTouch input backend tool:

```powershell
cargo run -p actingcommand-device-test -- reset
cargo run -p actingcommand-device-test -- tap 100 100
cargo run -p actingcommand-device-test -- longtap 500 500 1000
cargo run -p actingcommand-device-test -- swipe 300 500 900 500 500
```

Multiple subcommands can be supplied in one invocation. They reuse one long-lived MaaTouch session.

The default MaaTouch binary path is ignored by Git:

```text
external-tools/maatouch/maatouch
```

You can also pass an explicit external path:

```powershell
cargo run -p actingcommand-device-test -- --local ..\upstream-sources\AzurPilot\bin\MaaTouch\maatouch --port 16384 reset
```

MaaTouch failure is a fatal device-layer error. ADB input fallback is intentionally not implemented.

## Historical contracts and benchmarks

The Go and Python materials are not current implementation targets.

- Python mock runtime: `runtime`
- Go historical interfaces: `pkg/contract`
- Go/Python/Rust microbenchmarks: `benchmarks`
- UI HTTP contract: `contracts/runtime-api.openapi.yaml`
- UI event contract: `contracts/runtime-events.schema.json`
- task-flow schema: `contracts/task-flow.schema.json`
- SQLite schema: `contracts/sqlite/schema.sql`
- server variant policy: `contracts/server-keys.md`
- execution-layer boundary: `contracts/primitive-service.md`

Do not continue expanding the Python mock runtime or Go runtime/core line. UI code should use the runtime API and should not open the runtime SQLite database directly.

## Local run

Install legacy Python mock dependencies:

```powershell
python -m pip install -r .\runtime\requirements.txt
```

Start the legacy Python mock runtime:

```powershell
.\scripts\start-runtime.ps1
```

Stop the legacy Python mock runtime:

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
