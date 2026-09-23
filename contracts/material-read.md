# On-demand material reads

The resident Runtime and the existing `actingledger` entry read one range from one
committed artifact. Ledger owns reference selection and current retention. The
existing ArtifactStore reader owns file access, shared use protection, full length
and SHA-256 verification. A metadata reference never grants verified byte authority.

## Request

`RuntimeClient::read_material` submits `RuntimeOperation::ReadMaterial` with a
`RuntimeMaterialReadRequest`. The request contains:

- `event`: the original `event_id` and `sequence` (`LedgerEventPosition`).
- `artifact_id`, `snapshot_position`, original `byte_count` and canonical `sha256`.
- Optional `expected_run_id`, `expected_frame_id`, `expected_request_id` and
  `expected_correlation_id`, matched against that event's links.
- `offset`, `requested_length` and `max_reply_bytes`.

The source event must precede or equal the requested snapshot, which must not
exceed the committed source. Selection only searches the chosen event's formal
artifact references. An artifact from another event cannot fill a missing match.
The request contains no state root, object key, key material or arbitrary path.
Online source identity remains the existing validated RuntimeInfo/connection.

`requested_length` is 1..=196608 (192 KiB), so a worst-case JSON-encoded range and
its receipt stay within the 1 MiB frame. Checked addition rejects overflow; `offset`
must be below the committed material length. The last range may be shorter. The
original whole-material length type and producer limits remain unchanged.

The client clamps `max_reply_bytes` to its configured receiver bound. The Host
also applies its own bound and the original 1 MiB maximum. The bound covers the
complete serialized receipt, not just the raw range. A raw range is encoded using
the existing JSON byte-array representation. The client checks the response header
against that smaller bound before allocating its body and checks the material
selection on both successful and failed receipts. If even the failure receipt cannot
fit the requested bound, no success bytes are written and the connection reports
the original protocol failure; a client without a valid receipt remains unconfirmed.

## Verification and lifecycle

The first Ledger lookup yields an internal reference and current retention. A
known pending/completed/failed eviction is returned without opening material.
Otherwise the ArtifactStore reader acquires its original shared use protection.
While that protection is held, a second Ledger lookup verifies the same complete
reference and observes current retention. Offline reading reopens authenticated
metadata for this second lookup. No Ledger transaction spans the material read.

Only the requested range is retained while the reader scans the entire material.
The reader must reach EOF and successfully finish its length/hash verification
before that range is returned. Truncation, trailing bytes or a hash mismatch
discard all provisional bytes. Each request repeats whole-material verification;
there is no material session, cached response, token, new pin or persistent reader.
The original generic receipt cache is not populated or reused for material reads.

One monotonic four-second cooperative budget covers reference resolution, the
reader's chunk loop/EOF/finish and reply serialization. Offline metadata opening
uses the same deadline through `GlobalLedgerEvidenceConfig::with_deadline` and
preserves stricter source limits. The deadline is not reset for the second lookup.
Synchronous file I/O and the original writer queue are not forcibly cancelled;
checks observe budget expiry when control returns. Existing client I/O settings
and every other operation's deadlines/profile are unchanged.

The response separates the requested snapshot from `availability_through`, and
retains each native eviction observation's own through position. It does not claim
availability beyond the observed committed state. The shared reader guard is
released normally; no request acquires a cross-request holder or changes retention.

## Result and failure

`RuntimeResult::MaterialRead` contains the selected request, optional source
metadata, a read state, optional limit, optional verified chunk and optional failure.
Only `verified` carries a chunk: actual offset/length, whole length/hash, final-range
flag and bytes. The reference sent outside its owner omits `object_key`.

`not_provided` reports pending eviction, eviction, failed eviction, budget expiry
or reply-size refusal explicitly. `missing` requires a native NotFound from the
material read path. Root/lock/other I/O errors retain their own read-failure codes.
An absent file is never evidence of authorized eviction. `integrity_failed`,
`source_incomplete`, `request_denied` and `read_failed` contain no success bytes.
Existing views still report `material_read: not_requested`.

Expected retention states need no new event. Successful ranges create no per-range
success log and do not reuse publication-time `ArtifactVerified` as a new read event.
The Host explicitly records non-Ledger material failures through its original
module-owned lifecycle failure path, preserving operation, native details, secondary
causes, OS error and fatal disposition. Wire failures use the original redacted
ErrorProjection and safe codes. Fatal Ledger errors retain the program-failure
path; a failed Ledger cannot be followed by a successful material receipt.

Material failures use Failed/Denied receipts with the typed failure result and its
matching ErrorProjection. Existing outcomes for all other operations remain.
The client retains a received material failure receipt, while transport failures
remain unconfirmed. A valid receipt is not evidence of a device action.

## Offline consumer and privacy

The existing CLI entry accepts:

`actingledger --state-root <authorized-root> material --request <request-json>`

The typed JSON selection is limited to 16 KiB and rejects extra fields/arguments.
It uses the same Ledger resolver and ArtifactStore reader, returning a structured
`material_read` report within the requested response bound. Non-verified outcomes
return nonzero after the bounded report; native errors remain ForensicError values.
It never writes new facts into the source, verifies unrelated materials or runs a
device/provider. The state root follows the existing explicit offline entry.

Local access uses the already authorized local read scope and reports the actual
sensitivity/redaction facts. Actor/profile are provenance/projection choices, not
new permissions. The sending owner still applies the personal-information switch
before model/external transmission. Transformed or redacted bytes need their own
real material identity; the original hash is never attached to changed bytes.

An assembly must keep the same Runtime connection/source or explicit offline root,
event/material identity, total length and hash. Source changes, any failed range
or unconfirmed receipt invalidate unfinished assembly. Assembly happens only inside
runtime-client's complete-read helper `RuntimeClient::read_material_complete`, which
returns the offline `read_material_complete` result shape and verifies the assembled
length and SHA-256; the UI does not assemble segments itself. When a Runtime denies
a range above 64 KiB as an invalid request, the helper continues at 64 KiB ranges.
Its caller may also cancel cooperatively: the check runs between ranges, not inside
one exchange, and a cancelled read returns no bytes, only `material_read_cancelled`.
There are no other automatic retries, cross-call interaction linking or control
approval consumption. Existing client-action/approval authorization and replay stay
with their original owners.
