// SPDX-License-Identifier: AGPL-3.0-only

use serde_json::Value;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

fn main() {
    let iterations = parse_iterations();
    let root = repo_root();
    let acquisition = fs::read(root.join("benchmarks/workloads/acquisition_capture.json"))
        .expect("read acquisition workload");
    let event = fs::read(root.join("benchmarks/workloads/runtime_event.json"))
        .expect("read event workload");
    let task_flow = fs::read(root.join("benchmarks/workloads/task_flow.json"))
        .expect("read task flow workload");

    let acquisition_value: Value = serde_json::from_slice(&acquisition).expect("parse acquisition");

    bench(
        "serde_json parse acquisition",
        iterations,
        acquisition.len(),
        || {
            let _: Value = serde_json::from_slice(&acquisition).expect("parse acquisition");
        },
    );
    bench(
        "serde_json stringify acquisition",
        iterations,
        acquisition.len(),
        || {
            let _ = serde_json::to_vec(&acquisition_value).expect("stringify acquisition");
        },
    );
    bench(
        "serde_json parse runtime event",
        iterations,
        event.len(),
        || {
            let _: Value = serde_json::from_slice(&event).expect("parse event");
        },
    );
    bench(
        "serde_json parse task flow",
        (iterations / 10).max(1),
        task_flow.len(),
        || {
            let _: Value = serde_json::from_slice(&task_flow).expect("parse task flow");
        },
    );
    bench_tcp(iterations, &event);
}

fn parse_iterations() -> usize {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--iterations" {
            let value = args.next().expect("--iterations value");
            return value.parse().expect("valid iteration count");
        }
    }
    100_000
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn bench<F>(name: &str, iterations: usize, bytes_per_iter: usize, mut f: F)
where
    F: FnMut(),
{
    let start = Instant::now();
    for _ in 0..iterations {
        f();
    }
    let elapsed = start.elapsed().as_secs_f64();
    let us_per_op = elapsed * 1_000_000.0 / iterations as f64;
    let ops_per_sec = iterations as f64 / elapsed;
    let mib_per_sec = (bytes_per_iter * iterations) as f64 / (1024.0 * 1024.0) / elapsed;
    println!("{name}: {iterations} ops, {us_per_op:.2} us/op, {ops_per_sec:.2} ops/s, {mib_per_sec:.2} MiB/s");
}

fn bench_tcp(iterations: usize, payload: &[u8]) {
    let (addr_tx, addr_rx) = mpsc::channel();
    let (stop_tx, stop_rx) = mpsc::channel();

    let server = thread::spawn(move || {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind echo server");
        let addr = listener.local_addr().expect("local addr");
        addr_tx.send(addr).expect("send addr");

        while let Ok((mut stream, _)) = listener.accept() {
            if stop_rx.try_recv().is_ok() {
                break;
            }
            thread::spawn(move || {
                while let Ok(frame) = read_frame(&mut stream) {
                    if write_frame(&mut stream, &frame).is_err() {
                        break;
                    }
                }
            });
        }
    });

    let addr = addr_rx.recv().expect("server addr");
    let mut stream = TcpStream::connect(addr).expect("connect echo server");
    let start = Instant::now();
    for _ in 0..iterations {
        write_frame(&mut stream, payload).expect("write frame");
        let _ = read_frame(&mut stream).expect("read frame");
    }
    let elapsed = start.elapsed().as_secs_f64();
    let us_per_op = elapsed * 1_000_000.0 / iterations as f64;
    let ops_per_sec = iterations as f64 / elapsed;
    let mib_per_sec = (payload.len() * iterations) as f64 / (1024.0 * 1024.0) / elapsed;
    println!("tcp length-prefixed roundtrip: {iterations} ops, {us_per_op:.2} us/op, {ops_per_sec:.2} ops/s, {mib_per_sec:.2} MiB/s");

    let _ = stop_tx.send(());
    let _ = TcpStream::connect(addr);
    let _ = server.join();
}

fn write_frame(stream: &mut TcpStream, payload: &[u8]) -> std::io::Result<()> {
    let len = (payload.len() as u32).to_be_bytes();
    stream.write_all(&len)?;
    stream.write_all(payload)
}

fn read_frame(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut len = [0_u8; 4];
    stream.read_exact(&mut len)?;
    let size = u32::from_be_bytes(len) as usize;
    let mut payload = vec![0_u8; size];
    stream.read_exact(&mut payload)?;
    Ok(payload)
}
