// SPDX-License-Identifier: AGPL-3.0-only

use serde_json::Value;
use std::fs;
use std::process::Command;
use tempfile::TempDir;

fn actinglab_binary() -> &'static str {
    env!("CARGO_BIN_EXE_actinglab")
}

fn write_lab2_resource_root() -> TempDir {
    let temp = TempDir::new().unwrap();
    fs::create_dir(temp.path().join("recognition")).unwrap();
    fs::create_dir(temp.path().join("navigation")).unwrap();
    fs::write(
        temp.path().join("recognition").join("arknights.cn.pack.json"),
        r#"{
            "schema_version":"0.3",
            "coordinate_space":{"width":1,"height":1},
            "targets":[
                {"type":"color","id":"home_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                {"type":"color","id":"home_button","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0],"click":{"x":10,"y":20,"width":4,"height":6}}
            ]
        }"#,
    )
    .unwrap();
    fs::write(
        temp.path()
            .join("recognition")
            .join("arknights.cn.pages.json"),
        r#"{
            "schema_version":"0.3",
            "pages":[{"id":"arknights/home","required":["home_anchor"]}]
        }"#,
    )
    .unwrap();
    fs::write(
        temp.path()
            .join("navigation")
            .join("arknights.cn.navigation.json"),
        r#"{
            "schema_version":"0.3",
            "game":"arknights",
            "server":"cn",
            "navigation":[],
            "destructive_actions":[]
        }"#,
    )
    .unwrap();
    temp
}

fn encode_png(width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
    let mut scanlines = Vec::with_capacity((width * height * 3 + height) as usize);
    for _y in 0..height {
        scanlines.push(0);
        for _x in 0..width {
            scanlines.extend_from_slice(&color);
        }
    }
    let mut png = Vec::new();
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    write_chunk(&mut png, b"IHDR", &ihdr);

    let mut zlib = vec![0x78, 0x01];
    write_uncompressed_deflate(&mut zlib, &scanlines);
    zlib.extend_from_slice(&adler32(&scanlines).to_be_bytes());
    write_chunk(&mut png, b"IDAT", &zlib);
    write_chunk(&mut png, b"IEND", &[]);
    png
}

fn write_uncompressed_deflate(out: &mut Vec<u8>, data: &[u8]) {
    for (index, chunk) in data.chunks(65_535).enumerate() {
        let is_last = index == data.len().div_ceil(65_535) - 1;
        out.push(u8::from(is_last));
        let len = chunk.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(chunk);
    }
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(kind.len() + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let mut a = 1_u32;
    let mut b = 0_u32;
    for byte in data {
        a = (a + u32::from(*byte)) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

#[test]
fn lab2_child_process_stdout_starts_with_json_object() {
    let temp = write_lab2_resource_root();
    let scene = temp.path().join("home.png");
    fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();
    let output = Command::new(actinglab_binary())
        .args([
            "--json",
            "--dry-run",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "ark",
            "do",
            "home_button",
            "--scene",
            scene.to_str().unwrap(),
        ])
        .output()
        .expect("actinglab child process should run");

    assert!(output.status.success());
    assert_eq!(output.stdout.first(), Some(&b'{'));
    assert!(output.stderr.is_empty());
    let parsed = serde_json::from_slice::<Value>(&output.stdout).expect("stdout should parse");
    assert_eq!(parsed.get("ok").and_then(Value::as_bool), Some(true));
}
