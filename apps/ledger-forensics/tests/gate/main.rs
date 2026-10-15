// SPDX-License-Identifier: AGPL-3.0-only

// Test plan v2.2 L-4, gap G15 (ledger inventory key 36): `facts --at`, `material --request` and
// `views --instance-port` through the in-process `actingledger::run`. Each test builds a formal
// root in a tempdir with public APIs only (State tables, the formal SQLite ledger and its writer,
// and for a frame the ArtifactStore), closes the writer as an offline reader finds a root, and
// asserts the printed report and the result against the contracts each module names. No fixture
// reads a clock: every event time is a fixed constant.

mod facts;
mod fixture;
mod material;
mod views;
