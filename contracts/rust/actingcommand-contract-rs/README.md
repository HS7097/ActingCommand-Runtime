# actingcommand-contract-rs

Rust backup contract definitions for ActingCommand runtime boundaries.

The Go contracts under `pkg/contract` remain the P0a owner. This crate mirrors the Go boundary models and traits so future Rust execution workers or adapters can share a stable vocabulary without importing Go code.

## Scope

- shared runtime model types
- `PrimitiveLayer` trait
- `GameEngine` trait
- task-flow data structures

## Notes

- This crate is not wired into the runtime core yet.
- It has no production dependencies.
- Time values are represented as strings and duration values as milliseconds for transport neutrality.
- JSON field names are still governed by the OpenAPI and JSON schema files in `contracts/`.
