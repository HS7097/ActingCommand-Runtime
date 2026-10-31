// SPDX-License-Identifier: AGPL-3.0-only

//! #402 step 1 (C2): `ac-capture-bench`, the experiment carrier of plan v2.1
//! (`rd-1031/design-402-step1-v2.1.md`). It lives on the experiment branch with a draft pull
//! request that is never merged, is in no release package and never connects to the production
//! Runtime, F:\AC, a ledger or MuMuManager. It drives only the whitelisted experiment
//! instances, through the device crate.
//!
//! - [`hub`]: the lock graph `admission → queue_order → scheduler`, the atomic grant point, the
//!   entry paths P1-P9 as step machines, leases, budget, the invariant check (§3.3-§3.6);
//! - [`gates`]: the entry × gate table (§3.7.1);
//! - [`runner`]: the plan executor, cell end and whole-run stop, results and exit codes,
//!   error classes, the device-side close confirmation (§3.5, §3.11);
//! - [`supervisor`]: `closing` first, the close thread, the supervising thread, the heartbeat
//!   and the control files (§3.5.2);
//! - [`real`], [`bmodel`], [`viewer`], [`metrics`], [`safety`], [`winproc`]: the device, the
//!   B_model pipeline, the viewers, the metrics, the start check, the job object;
//! - [`fake`] and [`clock::VirtualClock`]: the deterministic fake-backend rehearsal (§6.2).

pub mod apk;
pub mod backend;
pub mod bmodel;
pub mod clock;
pub mod fake;
pub mod gates;
pub mod hub;
pub mod journal;
pub mod locks;
pub mod metrics;
pub mod model;
pub mod plan;
pub mod real;
pub mod runner;
pub mod safety;
pub mod supervisor;
pub mod timecode;
pub mod viewer;
pub mod views;
pub mod winproc;
