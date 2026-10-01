# Selection graph

This document collects the selection graph of
[Workflow #308](https://github.com/HS7097/ActingCommand-Workflow/issues/308). Each section is
frozen by the #308 slice that adds it; this revision holds the section **Records**. No
game-specific values are part of this contract.

## Records

### `task.selection_evaluated`

A select step records its decision in the task ledger as the event
`task.selection_evaluated` (family `task`), carrying the task fact

```json
{"kind": "selection_evaluated", "step_index": 3, "operation_label": "choose_slot",
 "selection": { … }}
```

Each select attempt appends exactly one, before any input and before any failure return.
Implementation status: the event type, the fact and the record exist; no producer appends
them yet. The host emits them with the in-task select step of a later #308 slice. A package
without a select step never produces the event.

The record has the shape of a `policy.*` decision: identity hashes and a complete breakdown.

| Field | Meaning |
| --- | --- |
| `layout_id`, `page_id` | The evaluated layout and its page; equal to the projection's. |
| `projection` | The full core candidate projection of the step's frame ([candidate-projection.md](candidate-projection.md)), including its `candidate_set_sha256`. |
| `policy.path` | The package-relative policy document path the step declares. |
| `policy.package_sha256` | SHA-256 of the document's bytes in the package, the step's declared `sha256` (64 lowercase hex). |
| `policy.policy_sha256` | The evaluator's canonical document identity, `sha256:<hex>`. |
| `policy.policy_id` | The document's `policy_id`. |
| `fact_snapshot_id`, `input_ledger_position` | The instance fact snapshot the evaluation read and its ledger position. |
| `now_unix_ms` | The instant the host supplied to the evaluation. |
| `input_sha256` | The evaluator's canonical identity of candidates, facts and instant, `sha256:<hex>`. |
| `outcome`, `outcome_key`, `selected[]` | The decision, its resource-declared key and the chosen candidate IDs. |
| `verdicts[]` | One breakdown per evaluated candidate: status, score, rank, gates, terms, reasons. |
| `reasons[]` | The decision's reason chain, `{code, detail}`. |
| `confirmation` | How the confirmation frame compared, below. |

`confirmation` is one of

| `kind` | When |
| --- | --- |
| `not_attempted` | The outcome is not `selected`. |
| `matched` `{candidate_set_sha256}` | The confirmation frame's candidate set hashes like the evaluated one. |
| `mismatched` `{candidate_set_sha256}` | It hashes differently; no input follows. |
| `page_changed` | The confirmation frame is not the step's page. |
| `guard_failed` `{code}` | The step's guard failed on the confirmation frame. |
| `capture_failed` `{code}` | The confirmation frame could not be captured. |

A confirmation is attempted exactly when the outcome is `selected`. Together with the
projection, a matched or mismatched record holds both candidate-set hashes.

#### Evaluator mirror

The breakdown types belong to the contract and mirror the selection-policy evaluator
(`crates/selection-policy/src/evaluator.rs`, `facts.rs`) field for field, with the same serde
shape. The contract does not depend on the evaluator; the kernel converts one into the other,
and the evaluator is unchanged.

| Contract type | Evaluator type | Fields and serde shape |
| --- | --- | --- |
| `TaskSelectionOutcome` | `SelectionOutcome` | tagged by `kind`: `selected{count}`, `empty`, `insufficient{surviving, required}`, `ambiguous{candidate_ids}`, `unknown{reason, detail}` |
| `TaskSelectionUnknownReason` | `UnknownReason` | `fact_missing`, `fact_expired`, `fact_stale`, `fact_low_confidence`, `fact_not_scalar`, `field_missing`, `type_mismatch`, `lookup_miss` |
| `TaskSelectionVerdict` | `CandidateVerdict` | `candidate_id`, `status`, `score_milli` (absent when none), `rank` (absent when none), `gates`, `terms`, `reasons` |
| `TaskSelectionCandidateStatus` | `CandidateStatus` | `ranked`, `gate_rejected`, `unknown_dropped` |
| `TaskSelectionGateResult` | `GateResult` | `gate_id`, `outcome` |
| `TaskSelectionGateOutcome` | `GateOutcome` | tagged by `kind`: `passed`, `failed`, `unknown_substituted{reason, passes}`, `unknown_dropped{reason}` |
| `TaskSelectionTermResult` | `TermResult` | `term_id`, `outcome`, `weight_milli`, `contribution_milli` |
| `TaskSelectionTermOutcome` | `TermOutcome` | tagged by `kind`: `scored{transformed_milli}`, `unknown_substituted{reason, transformed_milli}`, `unknown_dropped{reason}` |
| `PolicyReasonRecord` | `DecisionReason` | `code`, `detail` |

The record takes the remaining decision fields under its own names: `policy_id` and
`policy_sha256` into `policy`, `candidate_layout_id` as `layout_id`, `evaluated_at_unix_ms` as
`now_unix_ms`, and `candidates` as `verdicts`; `input_sha256`, `fact_snapshot_id`, `outcome`,
`outcome_key`, `selected` and `reasons` keep their names. The decision's `schema_version` is
not recorded: it is the schema of the policy document that `policy_sha256` identifies.

#### Validation and bounds

A record is checked when it is written and when it is read:

- the projection is valid and its sealed hash equals the recomputed one; `layout_id` and
  `page_id` equal the projection's;
- `policy.package_sha256` is 64 lowercase hex, `policy_sha256` and `input_sha256` are
  `sha256:<hex>`, `fact_snapshot_id` is a token, `input_ledger_position` and `now_unix_ms` are
  positive;
- at most 64 verdicts, each for a distinct actionable candidate of the projection;
- `selected` holds exactly `count` distinct IDs of `ranked` verdicts for a `selected` outcome
  and nothing otherwise; `selected` has a positive count and `insufficient` fewer survivors
  than required;
- `matched` carries the projection's hash and `mismatched` a different 64-hex hash;
- reason codes and confirmation codes are tokens, reason details text of at most 1024 bytes,
  the decision chain holds 1–128 reasons, as for a policy dispatch; labels and IDs are at most
  256 bytes without control characters.

A structural violation is `invalid_task_selection_record` with the field it concerns. Only
the writer measures bytes, on the current encoding: the projection within its 32 KiB budget
(`candidate_projection_budget_exceeded`) and the record's compact JSON within 64 KiB
(`selection_record_too_large`). The producer runs the same check before any input, so an
oversized record fails the task and nothing is truncated.

#### Ledger placement

- Durability: durable. It is the decision an input follows.
- Views: the event stream, Errors by severity and Lab by request context. The Observation
  and Changes type lists are compiled into the stored view DDL and compared at every open,
  so the event joins them only together with a view schema upgrade.
- Privacy: the ledger is a controlled surface. v1 values are booleans and integers, like the
  targets of `task.recognition_completed`, and the record carries no text read from the
  screen. The `Ui` and `Normal` profiles project it in the public payload like every task
  fact.
- Compatibility: the event is an added variant; no existing persisted structure gains a
  field, and every existing record re-serializes to the same bytes. A reader built before the
  variant rejects a ledger that contains it, so the UI and `actingledger` move to a Runtime
  that knows it before such a ledger reaches them.
