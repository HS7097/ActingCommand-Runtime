# AliceRuntimeOrchestrator

`AliceRuntimeOrchestrator` is the local long-lived runtime service for ActingCommand.

It is intentionally independent from the desktop UI. Closing or reloading the UI must not stop this process.

## Install runtime dependency

```powershell
python -m pip install -r runtime/requirements.txt
```

## Start

```powershell
.\scripts\start-runtime.ps1
```

Use foreground mode while debugging:

```powershell
.\scripts\start-runtime.ps1 -Foreground
```

## Stop

```powershell
.\scripts\stop-runtime.ps1
```

Runtime state is stored under:

```text
%LOCALAPPDATA%\GachaPilot\AliceRuntimeOrchestrator
```

The runtime state path still uses the historical `GachaPilot` directory until a dedicated state migration is implemented.

The runtime writes:

- `runtime_info.json`
- `state_snapshot.json`
- `logs/runtime.log`
- `logs/orchestrator.log`
- `resources/resource_history.jsonl`
- `acquisitions/index.jsonl`
