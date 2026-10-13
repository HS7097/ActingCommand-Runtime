// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #396 REC-2: a page rule on a template that declares `unique` does not match while a
//! second place exists, and its page evidence shows why.

use super::*;
use actingcommand_recognition_pack::{
    AssetResolver, RecognitionPackCode, RecognitionPackResult, load_pack_from_json_str,
};
use serde_json::json;
use std::sync::Arc;

const ICON: u32 = 10;

#[derive(Debug)]
struct IconAsset(Vec<u8>);

impl AssetResolver for IconAsset {
    fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
        if path == "icon.png" {
            Ok(self.0.clone())
        } else {
            Err(RecognitionPackError::fatal("no such fixture asset"))
        }
    }
}

/// A neutral 10 by 10 pattern; no pixel is black.
fn icon_pixel(x: u32, y: u32) -> [u8; 3] {
    [
        ((x * 37 + y * 11) % 250 + 5) as u8,
        ((x * 13 + y * 41 + 7) % 250 + 5) as u8,
        ((x * 29 + y * 17 + 3) % 250 + 5) as u8,
    ]
}

/// A black 120 by 60 frame with the icon at each of `copies`.
fn frame(copies: &[(u32, u32)]) -> Scene {
    let png = encode_png(120, 60, |x, y| {
        copies
            .iter()
            .find(|(left, top)| {
                (*left..left + ICON).contains(&x) && (*top..top + ICON).contains(&y)
            })
            .map_or([0, 0, 0], |(left, top)| icon_pixel(x - left, y - top))
    });
    Scene::from_png(&png).expect("scene")
}

#[test]
fn a_page_rule_on_a_unique_template_with_a_second_place_does_not_match_and_shows_why() {
    let pack = load_pack_from_json_str(
        &json!({
            "schema_version": "0.7",
            "coordinate_space": {"width": 120, "height": 60},
            "targets": [{
                "type": "template", "id": "icon/unique", "template_path": "icon.png",
                "region": "full_frame", "threshold": 0.9, "unique": {"margin_milli": 50}
            }]
        })
        .to_string(),
    )
    .expect("recognition pack");
    let evaluator = RecognitionEvaluator::with_asset_resolver(
        pack,
        Arc::new(IconAsset(encode_png(ICON, ICON, icon_pixel))),
    )
    .expect("recognition evaluator");
    let detector = PageDetector::new(
        serde_json::from_value(json!({
            "schema_version": "0.6",
            "pages": [{"id": "neutral/list", "required": ["icon/unique"]}]
        }))
        .expect("page set"),
    )
    .expect("page detector");

    let once = detector
        .evaluate_page(&evaluator, &frame(&[(10, 10)]), "neutral/list")
        .expect("one icon");
    assert!(once.matched);

    let twice = detector
        .evaluate_page(&evaluator, &frame(&[(10, 10), (80, 40)]), "neutral/list")
        .expect("two icons");
    assert!(!twice.matched);
    let target = &twice.target_results[0];
    assert_eq!(target.target_id, "icon/unique");
    assert!(!target.passed);
    let failure = target
        .evaluation
        .failure
        .as_ref()
        .expect("the reason is in the page evidence");
    assert!(failure.is(RecognitionPackCode::TargetMatchNotUnique));
}

/// An uncompressed RGB PNG of at most one stored deflate block.
fn encode_png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let mut scanlines = Vec::new();
    for y in 0..height {
        scanlines.push(0);
        for x in 0..width {
            scanlines.extend_from_slice(&pixel(x, y));
        }
    }
    let length = u16::try_from(scanlines.len()).expect("one stored block");
    let mut zlib = vec![0x78, 0x01, 0x01];
    zlib.extend_from_slice(&length.to_le_bytes());
    zlib.extend_from_slice(&(!length).to_le_bytes());
    zlib.extend_from_slice(&scanlines);
    let (mut a, mut b) = (1_u32, 0_u32);
    for byte in &scanlines {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &header);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
}

fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let mut crc = 0xffff_ffff_u32;
    for byte in kind.iter().chain(data) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    png.extend_from_slice(&(!crc).to_be_bytes());
}
