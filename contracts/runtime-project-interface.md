# Runtime Project Interface

The Runtime project interface is a read-only, game-neutral projection for UI, CLI, and external
clients. It is transported through the resident Runtime's existing local IPC boundary. Clients do
not receive GlobalLedger write authority, device ownership, or an execution lease from this query.

The current contract version is `actingcommand.project-interface.v1`. One response contains typed
project, instance, catalog, fact, goal, decision, approval, runtime-state, and diagnostic sections.
The Runtime translates its internal domain state into these transport DTOs; transport JSON is not
used as an authoritative domain or persistence model.

## Negotiation

Clients send `actingcommand.project-interface.request.v1` with a bounded ordered set of accepted
contract versions. Runtime selects the newest version it supports from that set. A request with no
shared version is rejected with a protocol error; it is never interpreted as the current version.

| Client accepts | Runtime supports | Result |
| --- | --- | --- |
| v1 | v1 | v1 response |
| v1 plus unknown future versions | v1 | v1 response |
| unknown versions only | v1 | fail loud |
| malformed request schema | v1 | fail loud |
| response version unknown to client | v1 | client rejects response |

V1 rejects unknown JSON fields at every transport object. A future version requires an explicit
compatibility row and translator; changing V1 field semantics in place is not compatible.
Responses are bounded below the local IPC frame limit; an oversized projection is rejected with a
typed protocol error rather than truncating sections or closing the connection as fake success.

## Authority Boundary

`RuntimeProjectClient` exposes only runtime discovery and project snapshot queries. The project
interface cannot append ledger facts, acquire leases, open capture or input backends, control an
application, or execute a task. Existing privileged Runtime commands remain separate contracts
with their existing host-side validation and ownership rules.

Identifiers and values in the projection are generic strings or closed framework enums. Games,
servers, task templates, recognition assets, and policy defaults remain external data.
