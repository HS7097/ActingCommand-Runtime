// SPDX-License-Identifier: AGPL-3.0-only

// Shared test support of the contained-task kernel (inventory-exec §6 I4: kernel tests live under
// `contained_task/tests/`).

// Test plan v2.2 §3.10, E-3 seam EXEC-I4: the scripted-frame runtime.
pub(super) mod scripted;

// Workflow #411 B15 R1: the deadline a gate test hands an admission by path. A gate file reads no
// clock (test plan v2.2 §2.1 guard 3); this deadline only bounds the admission, which never takes
// this long, so it decides nothing (the CD-27 D2 (a) form).
pub(super) fn admission_deadline() -> std::time::Instant {
    std::time::Instant::now() + std::time::Duration::from_secs(60)
}
