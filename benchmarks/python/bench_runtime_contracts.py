#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only

from __future__ import annotations

import argparse
import json
import pathlib
import socket
import sqlite3
import struct
import threading
import time
from collections.abc import Callable


ROOT = pathlib.Path(__file__).resolve().parents[2]
WORKLOADS = ROOT / "benchmarks" / "workloads"


def load_bytes(name: str) -> bytes:
    return (WORKLOADS / name).read_bytes()


def report(name: str, iterations: int, elapsed: float, bytes_per_iter: int | None = None) -> None:
    per_op_us = elapsed * 1_000_000 / iterations
    ops_per_sec = iterations / elapsed if elapsed else 0
    suffix = ""
    if bytes_per_iter:
        mib = (bytes_per_iter * iterations) / (1024 * 1024)
        suffix = f", {mib / elapsed:.2f} MiB/s"
    print(f"{name}: {iterations} ops, {per_op_us:.2f} us/op, {ops_per_sec:.2f} ops/s{suffix}")


def bench(name: str, iterations: int, fn: Callable[[], None], bytes_per_iter: int | None = None) -> None:
    start = time.perf_counter()
    for _ in range(iterations):
        fn()
    report(name, iterations, time.perf_counter() - start, bytes_per_iter)


def bench_json(iterations: int) -> None:
    acquisition_bytes = load_bytes("acquisition_capture.json")
    event_bytes = load_bytes("runtime_event.json")
    task_flow_bytes = load_bytes("task_flow.json")
    acquisition_obj = json.loads(acquisition_bytes)

    bench("json.loads acquisition", iterations, lambda: json.loads(acquisition_bytes), len(acquisition_bytes))
    bench("json.dumps acquisition", iterations, lambda: json.dumps(acquisition_obj, separators=(",", ":")), len(acquisition_bytes))
    bench("json.loads runtime event", iterations, lambda: json.loads(event_bytes), len(event_bytes))
    bench("json.loads task flow", max(1, iterations // 10), lambda: json.loads(task_flow_bytes), len(task_flow_bytes))


def bench_sqlite(iterations: int) -> None:
    schema = (ROOT / "contracts" / "sqlite" / "schema.sql").read_text(encoding="utf-8")
    conn = sqlite3.connect(":memory:")
    conn.executescript(schema)
    conn.execute(
        "insert into profiles (id, name, game, server, resolution_width, resolution_height) values (?, ?, ?, ?, ?, ?)",
        ("alas-jp-main", "Alas JP", "Azur", "alas.jp", 1280, 720),
    )
    conn.execute(
        "insert into task_runs (id, profile_id, task_id, flow_id, state, started_at) values (?, ?, ?, ?, ?, ?)",
        ("run-20260614-000001", "alas-jp-main", "daily.claim_rewards", "azur.daily.claim_rewards", "running", "2026-06-14T10:00:00.000Z"),
    )

    start = time.perf_counter()
    with conn:
        for i in range(iterations):
            conn.execute(
                "insert into resource_history (profile_id, task_run_id, game, server, key, value, source, observed_at) values (?, ?, ?, ?, ?, ?, ?, ?)",
                ("alas-jp-main", "run-20260614-000001", "Azur", "alas.jp", "coin", str(i), "benchmark", "2026-06-14T10:00:00.000Z"),
            )
    report("sqlite resource_history insert", iterations, time.perf_counter() - start)

    start = time.perf_counter()
    with conn:
        for i in range(iterations):
            conn.execute(
                "insert into acquisition_captures (id, profile_id, task_id, task_run_id, game, server, resolution_width, resolution_height, image_ref, source_trigger, recognition_state, captured_at) values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    f"acq-{i}",
                    "alas-jp-main",
                    "daily.claim_rewards",
                    "run-20260614-000001",
                    "Azur",
                    "alas.jp",
                    1280,
                    720,
                    f"runtime://images/acq-{i}",
                    "reward_screen",
                    "pending",
                    "2026-06-14T10:00:00.000Z",
                ),
            )
    report("sqlite acquisition_captures insert", iterations, time.perf_counter() - start)


def bench_tcp(iterations: int) -> None:
    payload = load_bytes("runtime_event.json")
    server = LengthPrefixedEchoServer()
    server.start()
    try:
        with socket.create_connection(server.address) as sock:
            start = time.perf_counter()
            for _ in range(iterations):
                send_frame(sock, payload)
                recv_frame(sock)
            report("tcp length-prefixed roundtrip", iterations, time.perf_counter() - start, len(payload))
    finally:
        server.stop()


class LengthPrefixedEchoServer:
    def __init__(self) -> None:
        self._sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self._sock.bind(("127.0.0.1", 0))
        self._sock.listen()
        self.address = self._sock.getsockname()
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._serve, daemon=True)

    def start(self) -> None:
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        try:
            with socket.create_connection(self.address, timeout=1):
                pass
        except OSError:
            pass
        self._thread.join(timeout=2)
        self._sock.close()

    def _serve(self) -> None:
        while not self._stop.is_set():
            try:
                conn, _ = self._sock.accept()
            except OSError:
                return
            threading.Thread(target=self._handle, args=(conn,), daemon=True).start()

    def _handle(self, conn: socket.socket) -> None:
        with conn:
            while not self._stop.is_set():
                try:
                    payload = recv_frame(conn)
                    send_frame(conn, payload)
                except OSError:
                    return


def send_frame(sock: socket.socket, payload: bytes) -> None:
    sock.sendall(struct.pack(">I", len(payload)))
    sock.sendall(payload)


def recv_frame(sock: socket.socket) -> bytes:
    header = recv_exact(sock, 4)
    size = struct.unpack(">I", header)[0]
    return recv_exact(sock, size)


def recv_exact(sock: socket.socket, size: int) -> bytes:
    chunks = bytearray()
    while len(chunks) < size:
        chunk = sock.recv(size - len(chunks))
        if not chunk:
            raise OSError("socket closed")
        chunks.extend(chunk)
    return bytes(chunks)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--iterations", type=int, default=10_000)
    parser.add_argument("--skip-tcp", action="store_true")
    args = parser.parse_args()

    bench_json(args.iterations)
    bench_sqlite(args.iterations)
    if not args.skip_tcp:
        bench_tcp(args.iterations)


if __name__ == "__main__":
    main()
