// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::{RecognitionEvaluator, load_pack_from_json_str};
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[test]
fn evaluates_template_target_from_disk_fixture() -> Result<(), Box<dyn Error>> {
    let root = temp_fixture_dir("from-disk");
    fs::create_dir_all(root.join("templates"))?;
    fs::create_dir_all(root.join("scenes"))?;

    fs::write(root.join("templates/button.png"), button_png(12, 10))?;
    fs::write(root.join("scenes/home_scene.png"), scene_png(64, 48))?;
    fs::write(root.join("recognition-pack.json"), pack_json())?;

    let pack_json = fs::read_to_string(root.join("recognition-pack.json"))?;
    let pack = load_pack_from_json_str(&pack_json)?;
    let scene_png = fs::read(root.join("scenes/home_scene.png"))?;
    let scene = Scene::from_png(&scene_png)?;
    let evaluator = RecognitionEvaluator::new(root.clone(), pack)?;

    let evaluation = evaluator.evaluate_target(&scene, "fixture/button")?;
    assert!(evaluation.passed);

    let click = evaluator.get_click_target("fixture/button")?;
    assert_eq!(
        (click.x, click.y, click.width, click.height),
        (30, 20, 18, 14)
    );

    fs::remove_dir_all(root)?;
    Ok(())
}

fn temp_fixture_dir(label: &str) -> PathBuf {
    let index = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "actingcommand-recognition-pack-{label}-{}-{index}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    root
}

fn pack_json() -> &'static str {
    r#"{
        "schema_version": "0.1",
        "game": "fixture",
        "server": "test",
        "locale": "test",
        "coordinate_space": { "width": 64, "height": 48 },
        "defaults": { "template_threshold": 0.90, "color_max_distance": 20.0 },
        "targets": [
          {
            "type": "template",
            "id": "fixture/button",
            "template_path": "templates/button.png",
            "region": { "x": 20, "y": 14, "width": 12, "height": 10 },
            "click": { "x": 30, "y": 20, "width": 18, "height": 14 }
          }
        ]
    }"#
}

fn scene_png(width: u32, height: u32) -> Vec<u8> {
    encode_png(width, height, |x, y| {
        if (20..32).contains(&x) && (14..24).contains(&y) {
            button_pixel(x - 20, y - 14)
        } else {
            [24, 28, 36]
        }
    })
}

fn button_png(width: u32, height: u32) -> Vec<u8> {
    encode_png(width, height, button_pixel)
}

fn button_pixel(x: u32, y: u32) -> [u8; 3] {
    let stripe = ((x * 11 + y * 17) % 97) as u8;
    [
        70 + stripe,
        120 + (stripe / 2),
        190_u8.saturating_sub(stripe / 3),
    ]
}

fn encode_png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let mut scanlines = Vec::with_capacity((height * (1 + width * 3)) as usize);
    for y in 0..height {
        scanlines.push(0);
        for x in 0..width {
            scanlines.extend_from_slice(&pixel(x, y));
        }
    }

    let mut zlib = Vec::new();
    zlib.extend_from_slice(&[0x78, 0x01]);
    write_stored_deflate_blocks(&mut zlib, &scanlines);
    let adler = adler32(&scanlines);
    zlib.extend_from_slice(&adler.to_be_bytes());

    let mut png = Vec::new();
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    append_chunk(&mut png, b"IHDR", &ihdr);
    append_chunk(&mut png, b"IDAT", &zlib);
    append_chunk(&mut png, b"IEND", &[]);
    png
}

fn write_stored_deflate_blocks(out: &mut Vec<u8>, data: &[u8]) {
    let block_count = data.len().div_ceil(65_535);
    for (index, chunk) in data.chunks(65_535).enumerate() {
        out.push(if index + 1 == block_count { 1 } else { 0 });
        let len = chunk.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(chunk);
    }
}

fn append_chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let mut crc_data = Vec::with_capacity(kind.len() + data.len());
    crc_data.extend_from_slice(kind);
    crc_data.extend_from_slice(data);
    png.extend_from_slice(&crc32(&crc_data).to_be_bytes());
}

fn adler32(data: &[u8]) -> u32 {
    let mut a = 1_u32;
    let mut b = 0_u32;
    for byte in data {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}
