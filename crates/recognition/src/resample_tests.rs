// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #396 REC-5 golden tests of the resampling primitive (REC-M v1.2.1 §7.1, §12): a gray
// image shrunk by 750, an RGB image grown by 1250 and an RGB 1920x1080 frame converted to
// 1280x720 give these bytes on every host. The expected bytes were computed off-repo by an
// independent reference of the contract's arithmetic (exact fractions for the 16.16 weights).
// Images are synthetic arithmetic patterns.

use super::*;
use image::{GrayImage, RgbImage};

/// 64-bit FNV-1a of `bytes`: one number that pins every byte of a large output.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[test]
fn a_gray_image_shrunk_by_750_keeps_its_golden_bytes() {
    let source = GrayImage::from_fn(8, 6, |x, y| {
        image::Luma([((x * 37 + y * 91 + (x * y) % 7 * 13) % 256) as u8])
    });
    let (width, height) = (scaled_length(8, 750), scaled_length(6, 750));
    assert_eq!((width, height), (6, 5));

    let scaled = resample(&source, width, height).expect("resampled");

    assert_eq!(
        scaled.as_raw().as_slice(),
        [
            25, 74, 123, 139, 188, 66, 135, 160, 185, 66, 134, 133, 117, 140, 129, 165, 153, 108,
            99, 174, 99, 108, 118, 82, 155, 75, 65, 151, 174, 185,
        ]
    );
}

#[test]
fn an_rgb_image_grown_by_1250_keeps_its_golden_bytes() {
    let source = RgbImage::from_fn(6, 4, |x, y| {
        image::Rgb([
            ((x * 53 + y * 17) % 256) as u8,
            ((x * 11 + y * 71 + 40) % 256) as u8,
            ((x * x * 5 + y * 29) % 256) as u8,
        ])
    });
    let (width, height) = (scaled_length(6, 1250), scaled_length(4, 1250));
    assert_eq!((width, height), (8, 5));

    let scaled = resample(&source, width, height).expect("resampled");

    assert_eq!(
        scaled.as_raw().as_slice(),
        [
            0, 40, 0, 33, 47, 3, 73, 55, 11, 113, 63, 23, 152, 72, 42, 192, 80, 67, 136, 88, 97, 9,
            95, 125, 12, 90, 20, 45, 97, 23, 85, 105, 31, 125, 113, 43, 164, 121, 62, 204, 130, 87,
            148, 138, 117, 21, 145, 145, 26, 147, 44, 59, 153, 47, 98, 162, 54, 138, 170, 67, 178,
            178, 85, 218, 186, 110, 161, 195, 140, 35, 202, 169, 39, 203, 67, 72, 162, 70, 112,
            142, 77, 152, 150, 90, 191, 158, 109, 183, 166, 134, 127, 175, 164, 48, 181, 192, 51,
            253, 87, 84, 100, 90, 124, 12, 98, 164, 20, 110, 203, 29, 129, 83, 37, 154, 27, 45,
            184, 60, 52, 212,
        ]
    );
}

#[test]
fn a_1920x1080_frame_converted_to_1280x720_keeps_its_golden_bytes() {
    let source = RgbImage::from_fn(1920, 1080, |x, y| {
        image::Rgb([
            ((x * 7 + y * 3) % 256) as u8,
            ((x ^ y) & 255) as u8,
            ((x * 5 + (y * y) % 251) % 256) as u8,
        ])
    });

    let converted = resample(&source, 1280, 720).expect("resampled");

    assert_eq!(converted.dimensions(), (1280, 720));
    for ((x, y), pixel) in [
        ((0, 0), [3, 0, 2]),
        ((1, 0), [13, 2, 9]),
        ((0, 1), [7, 2, 5]),
        ((640, 360), [151, 220, 154]),
        ((1001, 333), [235, 43, 90]),
        ((1279, 719), [27, 72, 173]),
    ] {
        assert_eq!(converted.get_pixel(x, y).0, pixel, "pixel ({x}, {y})");
    }
    assert_eq!(fnv1a(converted.as_raw()), 0xb136_42ce_9107_f3ee);
}
