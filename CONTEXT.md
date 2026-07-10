# ActingCommand Runtime Language

## Runtime Host

The long-lived production process that owns the Runtime lifetime and composes its production modules.

## Scheduler

The sole production authority that admits requests, owns per-instance control leases, and decides task lifecycle transitions.

## Execution Kernel

The production capability that performs admitted recognition, input, task, and recovery work without deciding global scheduling.

## Device Throat

The single production path through which device capture and input capabilities are obtained and exercised.

## DeviceProxy

The Runtime-owned interface that accepts fenced input requests and invokes a writable device backend only after scheduler authorization.

## Client

A UI, user CLI, Agent, or Lab process that submits requests and consumes receipts or projections without owning production state.

## Lab

An optional debugging and sealed-testing client. Removing Lab must not remove or change production behavior.

## Runtime Request

A typed client request carrying actor, source, correlation, target instance, and operation facts for scheduler admission.

## Receipt

A projection of persisted terminal facts for one request. A successful receipt cannot precede its required durable outcome.

## Global Ledger

The append-only production fact source shared by Runtime modules and all clients through typed append, query, subscription, and projection interfaces.

## Ledger Writer

The single live owner that allocates event sequence numbers and durably appends sanitized events.

## Event Draft

A typed event before its sensitive fields have been processed by the declared redaction policy. It cannot be persisted directly.

## Sanitized Event Draft

An event draft whose fields have passed the required pre-persistence redaction policy and may enter ledger ingress.

## Persisted Event

A sanitized event with ledger-assigned identity and sequence that is part of the durable fact source.

## Projection

A read-only view derived from persisted events for a particular client or diagnostic purpose.

## Owner Epoch

A unique identity for one successful Runtime-host ownership period. A takeover creates a new epoch and permanently fences older tokens.

## Lease

The scheduler's time-bounded grant of control over one target instance to one holder.

## Fencing Tuple

The owner epoch, lease id, instance id, holder id, and expiry facts that must match before a state-changing device operation is allowed.

## Backend Guard

A connection-scoped owner of a live device backend handle. Closing its connection revokes associated authority and closes the handle.

## Artifact

A screenshot, frame, diagnostic file, report, or archive stored outside event JSON and referenced by immutable metadata and hash.

## Semantic Frame

A frame required to explain an operation, recognition decision, state transition, failure, terminal state, or explicit human marker.

## Pinned Frame

A semantic frame that cannot be removed by similarity deduplication or ordinary pressure dropping.

## Retention Class

The lifecycle policy selected for artifacts and evidence: debug full, adaptive, or light.

## Task Outcome

The independent result of task execution, such as success, failure, or cancellation.

## Evidence Completeness

The independent assessment of whether required evidence is complete, partial, or failed.
