// SPDX-License-Identifier: AGPL-3.0-only

//! Nearest-rank percentiles of the warm repetitions (design §1.2: p50, p95, max).

use serde::{Deserialize, Serialize};

/// The summary of one list of samples, in the samples' own unit (the bench uses microseconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Percentiles {
    pub count: usize,
    pub min: u64,
    pub p50: u64,
    pub p95: u64,
    pub max: u64,
    /// The arithmetic mean, rounded down.
    pub mean: u64,
}

/// The nearest-rank percentiles of `values`: the p-th percentile is the smallest sample with at
/// least p percent of the samples at or below it (rank `ceil(p * n / 100)`, at least 1). An
/// empty list has none.
pub fn percentiles(values: &[u64]) -> Option<Percentiles> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let sum: u128 = sorted.iter().map(|value| u128::from(*value)).sum();
    let count = sorted.len();
    let mean = u64::try_from(sum / count as u128).unwrap_or(u64::MAX);
    Some(Percentiles {
        count,
        min: sorted[0],
        p50: nearest_rank(&sorted, 50),
        p95: nearest_rank(&sorted, 95),
        max: sorted[count - 1],
        mean,
    })
}

fn nearest_rank(sorted: &[u64], percent: usize) -> u64 {
    let rank = (percent * sorted.len())
        .div_ceil(100)
        .clamp(1, sorted.len());
    sorted[rank - 1]
}
