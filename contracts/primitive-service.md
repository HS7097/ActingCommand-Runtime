# Primitive Service Boundary

The execution layer language is not frozen. The runtime/core remains Go for P0a, but the operation/perception worker may be Rust, Go, C++, Python, or another language if it satisfies the primitive service contract.

Rust is a strong candidate for the execution layer because image processing, device control, and safe native integration can benefit from Rust's ownership model and predictable binaries.

## Boundary model

```text
UI  <->  Go runtime/core  <->  Primitive service adapter  <->  Rust execution worker
```

The Go runtime owns:

- profile and config state;
- scheduler decisions;
- task-flow interpretation;
- SQLite writes;
- runtime events and logs;
- UI HTTP/WebSocket API.

The Rust worker may own:

- capture implementation;
- template matching;
- OCR execution;
- input/touch/device commands;
- low-level emulator or ADB integration;
- operation-layer caches that are not the source of truth.

## Contract rules

- The UI must not call the Rust worker directly.
- The Rust worker must not own runtime lifecycle.
- The Rust worker must not write the runtime SQLite database directly in normal operation.
- The Rust worker returns structured observations, action results, image references, and classified errors.
- The Rust worker should not return raw frame buffers across the primitive boundary.
- The Go runtime records durable state, logs, resource history, and acquisition metadata.
- Severe execution-layer errors must propagate visibly through the Go runtime.
- Transient execution-layer failures may retry or fall back only with complete warning-level logs.

## Recommended P0a transport

The P0a contract should remain transport-neutral.

Acceptable first transports:

- local HTTP JSON;
- JSON-RPC over stdio;
- named-pipe JSON messages on Windows.

The initial choice should favor debuggability and schema stability over maximum throughput. High-throughput frame handling should stay inside the worker, with only file/content references crossing the boundary.

## Go adapter

`pkg/contract.PrimitiveLayer` is the Go adapter interface. A Rust implementation should be exposed through a Go adapter that satisfies `PrimitiveLayer` and translates method calls into the chosen IPC protocol.

This keeps the decision core independent from the worker language.

