# ActingLab A6 Issue 26 Handoff

Status: A6 move-only record

A6 preserves the existing Issue #26 behavior. It does not close G2 or G3.

## Relocated G2 Seams

| A6 path | New location | Preserved behavior |
| --- | --- | --- |
| `package validate` | `crates/lab/src/package_validate.rs:29` | The selected zip bytes are still hashed locally and that same digest is supplied to containment as `expected_hash`. The command still has no external expected-hash input and does not add an unverified/self-computed marker. |
| generated package validation used by `package build-task` | `crates/lab/src/package_build.rs:716` | The newly written temporary zip is still validated with a digest computed from those same local bytes before dry-run removal or final rename. No external hash source was added. |

## G3 Boundary

A6 relocates no Issue #26 G3 semantic drive command. The existing G3 behavior for semantic read/write commands is unchanged and remains outside this node. In particular, A6 does not introduce `LoadedBundle` admission for those commands or alter their scattered-resource compatibility behavior.
