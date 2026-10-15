// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #385 P-m, test row Tm-6 (frozen model PERF v1.4.2 §3.4): the GPU's utilisation is
// summed per engine type and the busiest type counts, as the task manager shows it; the sum of
// every engine is kept only as the configured alternative. Engine instances are named as the
// `GPU Engine` counter names them.

use super::{GpuAggregate, gpu_utilization_basis_points};

fn named_engines(rows: &[(&str, f64)]) -> Vec<(String, f64)> {
    rows.iter()
        .map(|(instance, value)| ((*instance).to_owned(), *value))
        .collect()
}

#[test]
fn tm_6_a_3d_load_beside_a_video_decode_load_is_the_busiest_type_not_their_sum() {
    let engines = named_engines(&[
        ("pid_100_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 60.0),
        (
            "pid_100_luid_0x0_0x1_phys_0_eng_3_engtype_VideoDecode",
            50.0,
        ),
    ]);
    assert_eq!(
        gpu_utilization_basis_points(&engines, GpuAggregate::MaxEngineType),
        Some(6_000)
    );
    assert_eq!(
        gpu_utilization_basis_points(&engines, GpuAggregate::Sum),
        Some(10_000)
    );
    // Two processes on the same engine type add up within that type.
    let shared = named_engines(&[
        ("pid_100_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 30.0),
        ("pid_200_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 25.0),
        (
            "pid_200_luid_0x0_0x1_phys_0_eng_3_engtype_VideoDecode",
            40.0,
        ),
    ]);
    assert_eq!(
        gpu_utilization_basis_points(&shared, GpuAggregate::MaxEngineType),
        Some(5_500)
    );
    assert_eq!(
        gpu_utilization_basis_points(&[], GpuAggregate::MaxEngineType),
        None
    );
}
