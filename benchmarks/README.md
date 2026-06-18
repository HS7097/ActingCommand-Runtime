# ActingCommand Runtime Performance Benchmarks

These benchmarks measure the Rust mainline runtime contracts before the full runtime is implemented.

The goal is to keep future runtime transport/database changes measurable in the Rust mainline.

Historical Go and Python benchmark harnesses were moved to:

- https://github.com/HS7097/ActingCommand-Legacy-Runtime

## What is measured

- JSON encode/decode cost for runtime contract payloads.
- Local transport round-trip cost using length-prefixed JSON over TCP.
- SQLite write cost for resource history and acquisition capture metadata.

## What is not measured yet

- Real OCR, image matching, capture, or touch performance.
- Emulator frame throughput.
- Full scheduler execution latency.
- Real UI rendering performance.

Those belong in later operation-layer and end-to-end benchmarks.

## Commands

Run Rust benchmarks:

```powershell
cargo run --release --manifest-path .\benchmarks\rust\Cargo.toml -- --iterations 100000
```

## Shared workload files

- `workloads/acquisition_capture.json`
- `workloads/runtime_event.json`
- `workloads/task_flow.json`

Rust benchmark harnesses should use these files so results stay comparable with historical benchmark data.

## Result interpretation

Treat these numbers as regression checks and rough design signals, not absolute product performance.

Important thresholds should be decided later after real device/capture/OCR measurements exist.

For now, the useful questions are:

- Is JSON overhead small enough for control-plane events?
- Is length-prefixed local IPC fast enough for primitive calls that do not transfer raw frames?
- Is SQLite metadata insertion fast enough for resource history and acquisition screenshot indexes?
- Does the Rust runtime create acceptable adapter/transport overhead for future execution workers?
