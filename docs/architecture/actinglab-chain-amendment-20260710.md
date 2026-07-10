# ActingLab Issue 33 Chain Amendment

Status: approved and frozen for issue #33 after A7

Freeze algorithm: normalize the file to LF, then hash the UTF-8 bytes strictly between the freeze marker lines, excluding the marker lines and including the payload's final LF.

Frozen payload SHA-256: `09ac0a1e8c891f54eeaccd3e0aae3f59851621df61d9e9c53bbec97907d54fe6`

<!-- ISSUE33-CHAIN-FREEZE-BEGIN -->
## Approval And Purpose

Alice approved this amendment on 2026-07-10. The original `TASK-lab-extraction-chain.md` remains immutable. This amendment resolves two contradictions discovered during implementation without weakening its terminal gates.

## A8a Scope Correction

A8a remains the only behavior-repair node before the remaining mechanical migrations. It now owns both:

- Lab2 arbitrator cross-process locking, stale-lock recovery, crash recovery, and zero-stagger concurrency behavior; and
- same-session runtime-ledger writer conflict detection.

Both changes receive focused regression coverage. Any affected A1 golden is reviewed and re-frozen at A8a. A8b and later migration nodes return to I6 mechanical, behavior-preserving movement.

## A8b

A8b remains the pure migration of the repaired Lab2 observe/do/ensure/wait family and its `ArbitratorStore` ownership into `crates/lab`.

## New A8c

A8c mechanically migrates every remaining use-case body required to make the original A9 gates achievable. It includes the remaining Session, device, capture, monitor, stream, record, configuration, ledger, package, resource, operation, control, run, report, and diagnostic use cases, plus the `task-loop` behavior still consumed by `device-test`.

A8c may use multiple internal commits and reviews, but remains one sequential chain node on issue #34. It must preserve wire fields, exit codes, state ownership, error behavior, side-effect ordering, and static goldens. CLI adapters retain argument parsing, envelope serialization, output, and process exit mapping. `crates/lab` owns the migrated use cases.

## A9 Gates Remain Unchanged

A9 does not relax the original terminal metrics:

- `apps/actinglab/src/main.rs` is at most 6000 lines;
- at least 95 percent of the 44 frozen top-level dispatch arms satisfy the S3 pipeline rule;
- no non-dispatch/non-parser function in `main.rs` exceeds 50 lines unless named in the frozen terminal exemption table;
- S1-S10, I1-I4, I5a, and I6-I8 hold; I5b remains explicitly out of scope;
- all goldens, architecture guards, formatting, Clippy, workspace tests, and retirement evidence pass.

To keep the 95 percent denominator meaningful, A8c may pipeline commands that were previously eligible for an A0 exemption. No more than two of the 44 frozen top-level arms may remain outside the S3 pipeline rule.

## Linear Order

The amended tail is:

`A7 -> A8a -> A8b -> A8c -> A9`

No separate child issue is created. Each node keeps independent commits, review evidence, a checkpoint tag, and an issue #34 comment.
<!-- ISSUE33-CHAIN-FREEZE-END -->
