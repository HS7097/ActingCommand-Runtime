# Live adapter observations

`actingctl agent-publish-facts --state-root <runtime-state> --record-file <observation.json>`
publishes an authorized adapter declaration through request.v3 as Agent/Adapter.
Other actingctl commands use Cli/Cli. These origin enums are routing constraints,
not authentication credentials. There is no selectable actor flag.

The file contains `{"records": [<FactRecord>, ...]}`: 1–256 records, bounded to
512 KiB. Records carry their scope/key/content, actual observation and expiry
times, TTL policy, confidence, detector, source snapshot, schema, bundle hash,
and invalidation events. The adapter binds the declaration to its specified
Runtime page/OCR evidence through the original source snapshot and bundle.
Shape validation does not establish screenshot semantics. Import time never
replaces observation time. Complete related fields share scope, snapshot,
observation/expiry times, TTL, detector, schema, bundle and invalidation policy.

Runtime validates the entire observation before writing one FactPublished event.
Its `record` and optional `related_records` carry the whole observation; scoped
invalidation binds the existing registered native instances in `scope_instances`.
Single PublishFact uses this same owner. Identical complete retries reuse the
EventId; conflicting, invalidated, older or future observations are rejected.
A refresh replacing members of an existing observation includes all its active
members. Fact identities are bounded to 256 per ledger lineage.

The existing durable append owns failure and tail recovery. A failed write is
fatal: partial raw bytes are not projected as a subset of the observation. A
recovered complete event can be retried idempotently. Recovery retains original
times and versions, including invalidation watermarks, and never renews TTL.

For a pool sourced from these facts, declare the following in its PoolSpec:

```json
{
  "observation": {"kind": "fact", "fact_key": "resource.current"},
  "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 900}
}
```

Keep that pool out of startup `resources.pools` and its exact scope/key out of
startup `facts.facts`. Omitted `value_source` denotes a static snapshot. Dynamic
bindings reject conflicting static declarations. Runtime derives a nonnegative
integer within the existing capacity and unit range from the exact matching
scope/key, and includes it in the same fact snapshot identity as the predicates.
The derived pool is never independently persisted or writable.

Missing, non-inline, low-confidence, wrong-type or out-of-range facts supply no
known pool value. Expiry also removes eligibility and bounds the existing intent
freshness/wake path; regeneration cannot revive an expired observation. Live pool
records require `input.committed` and `input.failed` in `invalidate_on`, plus a
current native instance binding. This deliberately invalidates on every input
outcome in that scope, so navigation can also require a fresh balance observation.
Instance membership changes require a fresh observation. Global configuration
events retain their existing global scope. Invalidation closes later admission;
it does not undo or resubmit an already committed task or effect.
