// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #392 DA-5ab fix round 1 (contracts/display-adapter.md section 11, "The projection's
// frame"; register §1.436; the coordinator's work order of 11-16 after the review of pull request
// #1780, point 2): the check `ContainedPageObservation::validate` makes of an observation's
// projection frame against its device frame. It takes the device frame itself and the device
// frame reduced to a base of its ratio, no larger than the frame (1920x1080 or 3840x2160 to
// 1280x720, 3x3 to 2x2, and 1600x900 to 1280x720, whose observation stands while its presses are
// refused at their input); it refuses a frame with a side of zero, a frame larger than the device
// frame, and a frame of another ratio. It cannot know the package's base: 640x360 on a 1280x720
// frame passes, as a package of that base would project it.

use super::online_observation::reduced_to;

#[test]
fn da5ab_fix1_a_projection_frame_is_its_device_frame_or_that_frame_reduced_by_a_pure_scale() {
    for (device, projected) in [
        ((1280, 720), (1280, 720)),
        ((1920, 1080), (1280, 720)),
        ((3840, 2160), (1280, 720)),
        ((1600, 900), (1280, 720)),
        ((3, 3), (2, 2)),
        ((1280, 720), (640, 360)),
    ] {
        assert!(
            reduced_to(device, projected),
            "{projected:?} on a {device:?} frame is taken"
        );
    }
    for (device, projected) in [
        ((0, 0), (0, 0)),
        ((1280, 720), (0, 0)),
        ((1280, 720), (1280, 0)),
        ((1280, 720), (0, 720)),
        ((1280, 720), (1920, 1080)),
        ((1280, 720), (1280, 960)),
        ((1366, 768), (1280, 720)),
        ((1920, 1080), (1280, 1080)),
    ] {
        assert!(
            !reduced_to(device, projected),
            "{projected:?} on a {device:?} frame is refused"
        );
    }
}
