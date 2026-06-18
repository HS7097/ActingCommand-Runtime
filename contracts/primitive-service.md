# Primitive Service Boundary

The execution layer language is not frozen. The runtime mainline is Rust, while future operation/perception workers may be Rust, Go, C++, Python, or another language if they satisfy the primitive service contract.

Rust is a strong candidate for the execution layer because image processing, device control, and safe native integration can benefit from Rust's ownership model and predictable binaries.

## Boundary model

```text
UI  <->  Rust runtime/core  <->  Primitive service adapter  <->  execution worker
```

The Rust runtime owns:

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
- The Rust runtime records durable state, logs, resource history, and acquisition metadata.
- Severe execution-layer errors must propagate visibly through the Rust runtime.
- Transient execution-layer failures may retry or fall back only with complete warning-level logs.

## Recommended P0a transport

The P0a contract should remain transport-neutral.

Acceptable first transports:

- local HTTP JSON;
- JSON-RPC over stdio;
- named-pipe JSON messages on Windows.

The initial choice should favor debuggability and schema stability over maximum throughput. High-throughput frame handling should stay inside the worker, with only file/content references crossing the boundary.

## Historical Go adapter

The old Go `pkg/contract.PrimitiveLayer` interface was moved to:

- https://github.com/HS7097/ActingCommand-Legacy-Runtime

This keeps the decision core independent from the worker language.
