// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #383 SH-1, register §1.18 (#406, approved 2026-10-14): the build of v0.12.0 decoded the
// single-touch path drag and refused an input request that named it (`input_action_unsupported`)
// until #406 T-3. Its test, `h1_the_host_refuses_the_reserved_path_drag_before_any_effect`,
// retired with Workflow #406 T3 (register §1.137), which performs the path drag. Its replacements
// (gate map host key 58) are `gate_406_t3::t3_a_run_performs_a_path_drag_on_a_segmented_backend_and_records_its_points`
// (the drag is performed) and the device test
// `touch::tests::t3_adb_sends_one_shell_of_motionevents_and_refuses_below_sdk_23` (a backend that
// cannot perform it refuses it with the segmented swipe capability error before any command).
