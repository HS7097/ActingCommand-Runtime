# ActingCommand Runtime Performance Benchmarks

These benchmarks measure the P0a runtime contracts before the full runtime is implemented.

The goal is to compare interface overhead across Python, Go, and Rust, and to keep future runtime transport/database changes measurable.

## What is measured

- JSON encode/decode cost for runtime contract payloads.
- Local transport round-trip cost using length-prefixed JSON over TCP.
- SQLite write cost for resource history and acquisition capture metadata.
- Go contract model overhead for primitive, task-flow, and GameEngine-facing payloads.

## What is not measured yet

- Real OCR, image matching, capture, or touch performance.
- Emulator frame throughput.
- Full scheduler execution latency.
- Real UI rendering performance.

Those belong in later operation-layer and end-to-end benchmarks.

## Commands

Run Go benchmarks:

```powershell
go test ./benchmarks/go -bench . -benchmem
```

Run Python benchmarks:

```powershell
python .\benchmarks\python\bench_runtime_contracts.py --iterations 10000
```

Run Rust benchmarks:

```powershell
cargo run --release --manifest-path .\benchmarks\rust\Cargo.toml -- --iterations 100000
```

## Shared workload files

- `workloads/acquisition_capture.json`
- `workloads/runtime_event.json`
- `workloads/task_flow.json`

Every language harness should use these files so results stay comparable.

## Result interpretation

Treat these numbers as regression checks and rough design signals, not absolute product performance.

Important thresholds should be decided later after real device/capture/OCR measurements exist.

For now, the useful questions are:

- Is JSON overhead small enough for control-plane events?
- Is length-prefixed local IPC fast enough for primitive calls that do not transfer raw frames?
- Is SQLite metadata insertion fast enough for resource history and acquisition screenshot indexes?
- Does a Rust execution worker create acceptable adapter/transport overhead when called from the Go runtime?
