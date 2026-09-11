# ActingLab capability declarations

`capabilities`, help and `list commands` use the existing command inventory.
Lab2 verb/schema summaries consume the same command declarations. These queries
do not connect to Runtime, start a provider, inspect a device, load a model or
fetch backend assets.

Each command has `status`, `available`, `reason_code`, `needs` and
`availability_scope: offline_declaration`. `available` is true only for the
`available` status. Status values are:

- `available`: an implemented offline handler has a normal path with its caller
  supplied inputs; this does not promise a particular input will validate.
- `retired`: the current route returns its defined retirement error.
- `reserved`: the route has no implemented normal operation.
- `unavailable`: the current handler always refuses the requested operation,
  with the existing failure code stated in `reason_code`.
- `unverified`: required Runtime, device, provider, configuration or material
  availability is not established by this offline query.

Runtime-dependent entries remain unverified even when the client code is
compiled. Their normal execution still requires the existing admission and
error handling. Retired command entries remain discoverable; the query does
not invoke their handlers. Session access/transport summaries mark the retired
file queue and daemon authority explicitly and identify Runtime as the current
execution authority. Lease, authentication and direct-device-access constraints
remain in their existing contracts.

Capture backends retain their declared choices and asset requirements, with
`declared_support` separate from `available`. Client configuration and asset
names do not establish the Runtime's actual backend or provider readiness.
Backend availability is unverified and `availability_checked` is false.
Discovered recognition-pack metadata is likewise unverified until normal
resource admission. Built-in template/color operations and Runtime provider
requirements are distinguished in the engine summary.

`schema_domains` and Lab2 `schema_versions` describe separate contract domains:
CLI envelope, legacy task, task operation, recognition pack, control and package
reference. No common highest version is defined. Each resource still passes
its existing version-specific validation; schema support does not prove an
installed resource or provider is usable. Package references include the
accepted typed Git source-tree version and the legacy ZIP digest form.
