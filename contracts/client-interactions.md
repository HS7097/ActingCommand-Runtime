# Client interactions and received receipts

`RuntimeClient::begin_interaction(&self) -> RuntimeClientResult<RuntimeClient>`
returns a handle sharing the same connection and frozen RuntimeInfo. It mints one
new `IssuedCorrelationId` with that connection's existing IdentifierIssuer. The
selection lives only in the returned handle. Cloning that handle preserves its
selection; calling `begin_interaction` again mints a new selection without changing
any existing handle. `correlation_id(&self) -> Option<CorrelationId>` observes the
selection. An ordinary connected handle has no selection and still mints a fresh
correlation for each independent request.

No caller-supplied correlation string, deserialized identifier or identifier from
another client is accepted. Every independent operation still mints its own
request ID. Only the original replay paths reuse their original request ID and
complete payload. Causation IDs keep their existing rules.

## Existing calls and authority

The handle uses the original shared connection mutex, identity checks, issuer,
fatal state and shutdown lifecycle. Starting an interaction performs no IPC or
device operation and establishes no server session, cache, token or new holder.
A latched fatal error remains the original error; starting another interaction
does not reopen or migrate the connection. Dropping handles follows the existing
shared connection lifetime.

The private request paths select the handle's correlation, or mint the ordinary
per-request correlation when none is selected:

| Existing call path | Correlation selection |
| --- | --- |
| Status, event queries, material reads, client actions, approvals and other ordinary public calls | Common `execute_receipt` and connection request construction |
| Readonly observation and capture sequences | Existing `issue_correlation` and explicit receipt execution |
| Safe reset, application control and contained tasks | The same selection while holding the existing connection lock; holder issuance and execution order stay intact |
| Authoring and debug sessions | Existing Lab actor/source gate, then the selected correlation passed into the original session |
| Contained page observation, Lab operations and explicit session calls | Their original session correlation and request-specific verification |

Selection while a connection guard is held uses the lock-local issuance function;
it never recursively acquires the same mutex. The generic operation executor
remains private. All calls keep their original actor/source, governance, lease,
holder, approval, replay and Runtime checks. A shared correlation is an association,
not approval or proof of execution.

A first consumer on an existing authorized User/Ui connection can use this sequence
(illustrative text only, not an executable example):

```text
interaction = client.begin_interaction()
receipt = interaction.record_client_action_receipt(valid_client_action)
page = interaction.query_event_page(query, original_profile, original_page_request)
```

Those submissions have different request IDs and the same selected correlation.
An approval or control call on that handle still needs its original authorization.

## Current flow and OCR ownership

The original complete paginated correlation query first reaches one frozen Ledger
snapshot under its existing time, event and page bounds. An interaction flow then
uses the current validated receipt's request ID and typed run/task anchor. A failed
task's anchor comes from its exact terminal event ID/sequence, request/correlation
links and original failure payload. The terminal must be present in the complete
source and agree with the current request/run. Missing or conflicting ownership
is a projection error retaining the original after-commit receipt.

The resulting interaction flow includes that request's events and every event in
its run, including legitimate child requests, recognition and frame events. Other
identified requests/runs are excluded. Events that cannot be assigned to a request
or run fail closed. Ordinary handles retain their original single-call flow shape.
This uses the complete snapshot, not a page subset or a text search. Larger shared
interactions still consume the original complete-query budget; there is no added
query round, continuation cache or increased limit.

OCR checks event/reference run, task, correlation and frame relationships before
selecting the current run's artifacts. Conflicting immutable artifact identities,
run/task assignments and duplicate lifecycle facts are rejected before other runs
are excluded. The current run keeps its original complete created/verified pairs,
retention/redaction requirements, full material/hash/payload verification, frame
coverage and report/terminal checks. Other runs' material bytes are not read for
the current projection. An incomplete current source remains a failure or the
existing explicit evidence gap.

## Receipt access and submission semantics

`record_client_action_receipt` and `record_approval_decision_receipt` return the
original `RuntimeReceipt`. They use the existing payload, result, approval ID,
disposition and terminal checks. The original `record_client_action` and
`record_approval_decision` call these methods once and return the validated
`TerminalEvent`; they do not resubmit.

`RuntimeClientError::received_receipt()` exposes an actual reply after the existing
structure, request/correlation and applicable result/selection checks. It includes
Denied/Failed replies with their actual state, error, result, rejection details
and optional terminal. A refusal without a terminal remains without a terminal.
Receipt arrival alone establishes neither Ledger commitment nor execution.
Transport failure, malformed structure and mismatched identity have no fabricated
received receipt. A latched error retains its original receipt and original IDs.
Contained-task completed/cancelled results must also identify the current request
in their `task_request_id` before reaching flow projection or receipt retention.

`committed_receipt()` keeps its original narrower eligibility and consumers:
contained-task/shutdown failures with a terminal, the material failure receipt
carried by the material-read contract, and the original after-commit projection
errors. General received refusals do not enter contained-task after-commit handling.
Material failures retain their original typed result even when its terminal is
absent. Callers inspect the actual terminal rather than infer one from an accessor
name. Already retained committed receipts are also visible through
`received_receipt()` without storing a second copy.

Lab's dedicated failure/evidence verification and the existing native error codes,
fatal disposition, header I/O facts and related errors remain. Host client-fact
replay, approval Ledger/RuntimeState transactions and approval target consumption
are unchanged. Source/CI delivery does not establish live UI or device operation.
