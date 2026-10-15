// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #382 H-5, test plan v2.2 §3.6 G20 (host gate key 43): capacity sampling with its
// volume resolver and its free-space query injected (seam I6). Requirement:
// contracts/capacity-admission.md:3-7 (each target is resolved to its volume, the available
// bytes are queried once per distinct volume per cycle, an unresolved binding is Unknown with
// its original native cause) and the H-5 G20 basis row (rd-1009/tests/req-basis-ha.md): a
// resolution error belongs to its own target, a query error to the volume's sample, which lists
// every target on that volume.

use crate::capacity::sample_capacity_with;
use crate::{CapacitySample, CapacityTarget, CapacityUnavailable, HostMetricsLocation};
use actingcommand_contract::codes::ContractLocation;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const VOLUME_A: &str = r"\\?\volume{0a0a0a0a-0000-4000-8000-00000000000a}\";
const VOLUME_B: &str = r"\\?\volume{0b0b0b0b-0000-4000-8000-00000000000b}\";
const AVAILABLE_ON_A: u64 = 64 * 1024 * 1024 * 1024;

/// A resolution the operating system refused, with its native cause.
fn unresolved(raw_os_error: i32, detail: &str) -> CapacityUnavailable {
    CapacityUnavailable {
        operation: HostMetricsLocation::ResolveCapacityTarget.location(),
        raw_os_error: Some(raw_os_error),
        detail: detail.to_owned(),
    }
}

/// The samples with each one's targets ascending, ordered by their first target: which targets
/// share which sample, whatever order the sampler builds them in.
fn by_target(mut samples: Vec<CapacitySample>) -> Vec<CapacitySample> {
    for sample in &mut samples {
        sample.targets.sort_unstable();
    }
    samples.sort_by_key(|sample| sample.targets.first().copied());
    samples
}

/// G20: six targets, two on volume A, two on volume B (whose query fails) and two that do not
/// resolve. Each volume is queried once; volume B's failure is its sample's, listing both of its
/// targets; each unresolved target is a sample of its own with its own native cause.
#[test]
fn gate_capacity_queries_each_volume_once_and_keeps_each_failure_with_its_targets() {
    let targets = [
        "installation",
        "state",
        "artifacts",
        "staging",
        "exports",
        "streams",
    ]
    .map(|name| CapacityTarget {
        path: PathBuf::from(name),
    });
    let not_found = unresolved(3, "the system cannot find the path specified");
    let denied = unresolved(5, "access is denied");
    let query_failed = CapacityUnavailable {
        operation: ContractLocation::SampleCapacity.location(),
        raw_os_error: Some(21),
        detail: "the device is not ready".to_owned(),
    };
    let mut resolved = Vec::new();
    let mut queried = Vec::new();

    let samples = sample_capacity_with(
        &targets,
        |path: &Path| {
            resolved.push(path.to_path_buf());
            match path.to_str() {
                Some("installation" | "staging") => Ok(VOLUME_A.to_owned()),
                Some("state" | "exports") => Ok(VOLUME_B.to_owned()),
                Some("artifacts") => Err(not_found.clone()),
                _ => Err(denied.clone()),
            }
        },
        |volume: &str| {
            queried.push(volume.to_owned());
            if volume == VOLUME_A {
                Ok(AVAILABLE_ON_A)
            } else {
                Err(query_failed.clone())
            }
        },
    );

    // Every target is resolved; each distinct volume is queried exactly once, the failed one
    // included (no retry).
    assert_eq!(
        resolved.into_iter().collect::<BTreeSet<_>>(),
        targets
            .iter()
            .map(|target| target.path.clone())
            .collect::<BTreeSet<_>>()
    );
    queried.sort();
    assert_eq!(queried, [VOLUME_A, VOLUME_B]);
    assert_eq!(
        by_target(samples),
        [
            CapacitySample {
                targets: vec![0, 3],
                volume_id: Some(VOLUME_A.to_owned()),
                available_bytes: Ok(AVAILABLE_ON_A),
            },
            CapacitySample {
                targets: vec![1, 4],
                volume_id: Some(VOLUME_B.to_owned()),
                available_bytes: Err(query_failed),
            },
            CapacitySample {
                targets: vec![2],
                volume_id: None,
                available_bytes: Err(not_found),
            },
            CapacitySample {
                targets: vec![5],
                volume_id: None,
                available_bytes: Err(denied),
            },
        ]
    );
}
