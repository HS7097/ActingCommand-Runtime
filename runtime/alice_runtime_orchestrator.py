#!/usr/bin/env python
# SPDX-License-Identifier: AGPL-3.0-only
"""AliceRuntimeOrchestrator V1.

The UI is a disposable client and never owns this process lifecycle. This
runtime must survive UI reloads, crashes, and closes, and all UI/runtime
communication goes through the local HTTP/WebSocket boundary.

Upstream Alas/AzurPilot code stays behind the adapter and license boundary. V1
ships original runtime contracts plus mock automation profiles; real ADB,
device, and game automation failures must be classified and logged when adapter
execution is added.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import os
import platform
import signal
import sys
import threading
import time
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import parse_qs, urlparse

try:
    from websockets.server import serve
except ImportError as exc:  # pragma: no cover - exercised by manual startup.
    print(
        "AliceRuntimeOrchestrator requires the runtime dependency 'websockets'. "
        "Install it with: python -m pip install -r runtime/requirements.txt",
        file=sys.stderr,
    )
    raise SystemExit(2) from exc


SERVICE_NAME = "AliceRuntimeOrchestrator"
APP_NAME = "GachaPilot"
DEFAULT_HTTP_PORT = 8765
DEFAULT_WS_PORT = 8766
MAX_LOG_EVENTS = 400


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def local_state_root() -> Path:
    base = os.environ.get("LOCALAPPDATA")
    if base:
        return Path(base) / APP_NAME / SERVICE_NAME
    return Path.home() / ".local" / "state" / APP_NAME / SERVICE_NAME


class SingleInstanceLock:
    def __init__(self, path: Path) -> None:
        self.path = path
        self._handle = None

    def acquire(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self._handle = self.path.open("a+b")
        try:
            if os.name == "nt":
                import msvcrt

                self._handle.seek(0)
                msvcrt.locking(self._handle.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl

                fcntl.flock(self._handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as exc:
            raise RuntimeError(f"{SERVICE_NAME} is already running or lock is busy: {self.path}") from exc
        self._handle.seek(0)
        self._handle.truncate()
        self._handle.write(str(os.getpid()).encode("utf-8"))
        self._handle.flush()

    def release(self) -> None:
        if self._handle is None:
            return
        try:
            if os.name == "nt":
                import msvcrt

                self._handle.seek(0)
                msvcrt.locking(self._handle.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                import fcntl

                fcntl.flock(self._handle.fileno(), fcntl.LOCK_UN)
        finally:
            self._handle.close()
            self._handle = None


class RuntimeState:
    """State owner for idempotent runtime commands and UI-facing snapshots."""

    def __init__(self, *, state_dir: Path, host: str, http_port: int, ws_port: int) -> None:
        self.state_dir = state_dir
        self.host = host
        self.http_port = http_port
        self.ws_port = ws_port
        self.started_at = utc_now()
        self.lock = threading.RLock()
        self.shutdown_event = threading.Event()
        self.loop: asyncio.AbstractEventLoop | None = None
        self.ws_clients: set[Any] = set()
        self.runtime_alive = False
        self.last_severity = "info"

        self.log_dir = self.state_dir / "logs"
        self.resource_dir = self.state_dir / "resources"
        self.acquisition_dir = self.state_dir / "acquisitions"
        for path in (self.log_dir, self.resource_dir, self.acquisition_dir):
            path.mkdir(parents=True, exist_ok=True)

        self.runtime_info_path = self.state_dir / "runtime_info.json"
        self.snapshot_path = self.state_dir / "state_snapshot.json"
        self.log_path = self.log_dir / "runtime.log"
        self.resource_history_path = self.resource_dir / "resource_history.jsonl"
        self.acquisition_index_path = self.acquisition_dir / "index.jsonl"

        self.profiles = self._initial_profiles()
        self.logs: list[dict[str, Any]] = []
        self.resource_history: list[dict[str, Any]] = self._initial_resource_history()
        self.acquisitions: list[dict[str, Any]] = self._initial_acquisitions()
        self._write_json(self.runtime_info_path, self.runtime_info())
        self._append_initial_files()
        self.log("info", SERVICE_NAME, "Runtime initialized; waiting for automation start.")
        self.write_snapshot()

    def runtime_info(self) -> dict[str, Any]:
        return {
            "service": SERVICE_NAME,
            "pid": os.getpid(),
            "host": self.host,
            "httpPort": self.http_port,
            "wsPort": self.ws_port,
            "startedAt": self.started_at,
            "stateDir": str(self.state_dir),
            "runtimeInfoPath": str(self.runtime_info_path),
        }

    def health(self) -> dict[str, Any]:
        return {
            "ok": True,
            "service": SERVICE_NAME,
            "pid": os.getpid(),
            "host": self.host,
            "httpPort": self.http_port,
            "wsPort": self.ws_port,
            "startedAt": self.started_at,
            "stateDir": str(self.state_dir),
            "platform": platform.platform(),
        }

    def runtime_status(self) -> dict[str, Any]:
        with self.lock:
            return {
                "runtime": {
                    "orchestratorAlive": True,
                    "automationAlive": self.runtime_alive,
                    "state": "running" if self.runtime_alive else "idle",
                    "lastSeverity": self.last_severity,
                    "startedAt": self.started_at,
                    "stateDir": str(self.state_dir),
                },
                "scheduler": self.scheduler_summary(),
            }

    def scheduler_summary(self) -> dict[str, Any]:
        profile = self.profiles[0]
        return {
            "alive": self.runtime_alive,
            "currentTaskLabel": profile["scheduler"]["currentTaskLabel"] if self.runtime_alive else "无任务",
            "nextTaskLabel": profile["scheduler"]["nextTaskLabel"],
            "nextRunTime": profile["scheduler"]["nextRunTime"],
            "pendingCount": profile["scheduler"]["pendingCount"] if self.runtime_alive else 0,
            "waitingCount": profile["scheduler"]["waitingCount"],
            "lastSeverity": self.last_severity,
        }

    def all_profile_summaries(self) -> list[dict[str, Any]]:
        with self.lock:
            return [self._profile_summary(profile) for profile in self.profiles]

    def profile_summary(self, profile_id: str) -> dict[str, Any] | None:
        with self.lock:
            for profile in self.profiles:
                if profile["id"] == profile_id:
                    return self._profile_summary(profile)
        return None

    def start_runtime(self) -> dict[str, Any]:
        with self.lock:
            if self.runtime_alive:
                self.log("warning", SERVICE_NAME, "Start ignored: automation runtime is already running.")
                return self.runtime_status()
            self.runtime_alive = True
            self.last_severity = "info"
            self.profiles[0]["state"] = "running"
            self.profiles[0]["stateText"] = "运行中"
            self.profiles[0]["scheduler"]["currentTaskLabel"] = "每日任务"
            self.profiles[0]["scheduler"]["pendingCount"] = 1
            self.log("info", SERVICE_NAME, "Automation runtime started by command.")
            self.broadcast_event("runtime.state_changed", self.runtime_status())
            self.broadcast_event("scheduler.current_task_changed", self.scheduler_summary())
            self.write_snapshot()
            return self.runtime_status()

    def stop_runtime(self) -> dict[str, Any]:
        with self.lock:
            if not self.runtime_alive:
                self.log("warning", SERVICE_NAME, "Stop ignored: automation runtime is already idle.")
                return self.runtime_status()
            self.runtime_alive = False
            self.profiles[0]["state"] = "idle"
            self.profiles[0]["stateText"] = "待机"
            self.profiles[0]["scheduler"]["currentTaskLabel"] = "无任务"
            self.profiles[0]["scheduler"]["pendingCount"] = 0
            self.log("info", SERVICE_NAME, "Automation runtime stopped by command.")
            self.broadcast_event("runtime.state_changed", self.runtime_status())
            self.broadcast_event("scheduler.current_task_changed", self.scheduler_summary())
            self.write_snapshot()
            return self.runtime_status()

    def restart_runtime(self) -> dict[str, Any]:
        self.stop_runtime()
        return self.start_runtime()

    def refresh_runtime(self) -> dict[str, Any]:
        with self.lock:
            self.log("info", SERVICE_NAME, "Runtime status refreshed.")
            self.broadcast_event("runtime.state_changed", self.runtime_status())
            self.write_snapshot()
            return self.runtime_status()

    def recent_logs(self, limit: int = 80) -> list[dict[str, Any]]:
        with self.lock:
            return self.logs[-max(1, min(limit, MAX_LOG_EVENTS)) :]

    def recent_acquisitions(self, limit: int = 24) -> list[dict[str, Any]]:
        with self.lock:
            return self.acquisitions[-max(1, min(limit, 100)) :]

    def resource_points(self) -> list[dict[str, Any]]:
        with self.lock:
            return list(self.resource_history)

    def request_shutdown(self) -> dict[str, Any]:
        self.log("warning", SERVICE_NAME, "Orchestrator shutdown requested.")
        self.shutdown_event.set()
        return {"ok": True, "message": "shutdown requested"}

    def log(self, level: str, source: str, message: str) -> dict[str, Any]:
        event = {
            "timestamp": utc_now(),
            "level": level.upper(),
            "source": source,
            "message": message,
        }
        with self.lock:
            self.logs.append(event)
            if len(self.logs) > MAX_LOG_EVENTS:
                self.logs = self.logs[-MAX_LOG_EVENTS:]
            if level.lower() in {"warning", "error", "fatal"}:
                self.last_severity = level.lower()
            with self.log_path.open("a", encoding="utf-8") as handle:
                handle.write(json.dumps(event, ensure_ascii=False) + "\n")
        self.broadcast_event("log.appended", {"log": event})
        if level.lower() in {"warning", "error", "fatal"}:
            self.broadcast_event("severity.changed", {"lastSeverity": self.last_severity})
        return event

    def broadcast_event(self, event_type: str, payload: dict[str, Any]) -> None:
        if self.loop is None:
            return
        event = {"type": event_type, "timestamp": utc_now(), "payload": payload}
        asyncio.run_coroutine_threadsafe(self._broadcast(event), self.loop)

    async def _broadcast(self, event: dict[str, Any]) -> None:
        if not self.ws_clients:
            return
        message = json.dumps(event, ensure_ascii=False)
        dead = []
        for client in list(self.ws_clients):
            try:
                await client.send(message)
            except Exception:
                dead.append(client)
        for client in dead:
            self.ws_clients.discard(client)

    def snapshot(self) -> dict[str, Any]:
        return {
            "health": self.health(),
            "status": self.runtime_status(),
            "profiles": self.all_profile_summaries(),
            "logs": self.recent_logs(),
            "resourceHistory": self.resource_points(),
            "acquisitions": self.recent_acquisitions(),
        }

    def write_snapshot(self) -> None:
        self._write_json(self.snapshot_path, self.snapshot())

    def _profile_summary(self, profile: dict[str, Any]) -> dict[str, Any]:
        return {
            "id": profile["id"],
            "name": profile["name"],
            "short": profile["short"],
            "gameServerLabel": profile["gameServerLabel"],
            "fullName": profile["fullName"],
            "state": profile["state"],
            "stateText": profile["stateText"],
            "color": profile["color"],
            "resourceSnapshot": profile["resources"],
            "resourceHistorySummary": profile["resourceHistorySummary"],
            "acquisitionSummary": [
                item for item in self.acquisitions if item["profileId"] == profile["id"]
            ][-3:],
            "recentLogSummary": self.recent_logs(8),
            "scheduler": profile["scheduler"],
        }

    def _append_initial_files(self) -> None:
        if not self.resource_history_path.exists():
            with self.resource_history_path.open("w", encoding="utf-8") as handle:
                for item in self.resource_history:
                    handle.write(json.dumps(item, ensure_ascii=False) + "\n")
        if not self.acquisition_index_path.exists():
            with self.acquisition_index_path.open("w", encoding="utf-8") as handle:
                for item in self.acquisitions:
                    handle.write(json.dumps(item, ensure_ascii=False) + "\n")

    @staticmethod
    def _write_json(path: Path, data: dict[str, Any]) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")

    @staticmethod
    def _initial_profiles() -> list[dict[str, Any]]:
        return [
            {
                "id": "alasr",
                "name": "AlasR",
                "short": "AZ",
                "gameServerLabel": "Azur.jp",
                "fullName": "Azur.jp 港区OA",
                "state": "idle",
                "stateText": "待机",
                "color": "#786ee6",
                "resources": [
                    {"key": "oil", "label": "石油", "value": "464 / 17350", "updatedAgo": "1小时前", "color": "#050505", "delta": "+120"},
                    {"key": "coin", "label": "物资", "value": "10556 / 101400", "updatedAgo": "1小时前", "color": "#ffb23e", "delta": "+2460"},
                    {"key": "gem", "label": "钻石", "value": "295", "updatedAgo": "2小时前", "color": "#ff4b4b", "delta": "0"},
                    {"key": "pt", "label": "活动PT", "value": "62750 / 1500000", "updatedAgo": "1小时前", "color": "#16c4f6", "delta": "+840"},
                    {"key": "cube", "label": "魔方", "value": "844", "updatedAgo": "2小时前", "color": "#34e3e8", "delta": "+2"},
                    {"key": "action", "label": "行动力", "value": "54 (404)", "updatedAgo": "3天前", "color": "#1017ff", "delta": "-60"},
                ],
                "resourceHistorySummary": {"primaryKey": "oil", "labels": ["18:00", "19:00", "20:00", "21:00", "22:00", "23:00"], "values": [310, 360, 330, 420, 410, 464]},
                "scheduler": {"alive": False, "currentTaskLabel": "无任务", "nextTaskLabel": "科研", "nextRunTime": "2026-05-31 21:59", "pendingCount": 0, "waitingCount": 4, "lastSeverity": "info"},
            },
            {
                "id": "maab",
                "name": "MaaB",
                "short": "AK",
                "gameServerLabel": "Ark.cn",
                "fullName": "Ark.cn 罗德岛B",
                "state": "idle",
                "stateText": "待机",
                "color": "#35d8a4",
                "resources": [
                    {"key": "sanity", "label": "理智", "value": "128 / 135", "updatedAgo": "刚刚", "color": "#48d17d", "delta": "+28"},
                    {"key": "lmd", "label": "龙门币", "value": "1245800", "updatedAgo": "刚刚", "color": "#ffb23e", "delta": "+43200"},
                    {"key": "orundum", "label": "合成玉", "value": "42600", "updatedAgo": "12分钟前", "color": "#ff6464", "delta": "+100"},
                ],
                "resourceHistorySummary": {"primaryKey": "sanity", "labels": ["18:00", "19:00", "20:00", "21:00", "22:00", "23:00"], "values": [72, 86, 104, 118, 123, 128]},
                "scheduler": {"alive": False, "currentTaskLabel": "无任务", "nextTaskLabel": "信用商店", "nextRunTime": "2026-05-31 22:30", "pendingCount": 0, "waitingCount": 3, "lastSeverity": "info"},
            },
            {
                "id": "baasjp",
                "name": "BaasJP",
                "short": "BA",
                "gameServerLabel": "BA.jp",
                "fullName": "BA.jp 夏莱档案",
                "state": "warning",
                "stateText": "等待确认",
                "color": "#ff8c6b",
                "resources": [
                    {"key": "ap", "label": "AP", "value": "172 / 230", "updatedAgo": "3分钟前", "color": "#48d17d", "delta": "+34"},
                    {"key": "credit", "label": "信用点", "value": "8154200", "updatedAgo": "3分钟前", "color": "#ffb23e", "delta": "+120000"},
                    {"key": "pyroxene", "label": "青辉石", "value": "23640", "updatedAgo": "1小时前", "color": "#4da3ff", "delta": "+30"},
                ],
                "resourceHistorySummary": {"primaryKey": "ap", "labels": ["18:00", "19:00", "20:00", "21:00", "22:00", "23:00"], "values": [96, 118, 137, 151, 166, 172]},
                "scheduler": {"alive": False, "currentTaskLabel": "无任务", "nextTaskLabel": "总力战", "nextRunTime": "2026-05-31 23:00", "pendingCount": 0, "waitingCount": 3, "lastSeverity": "warning"},
            },
        ]

    @staticmethod
    def _initial_resource_history() -> list[dict[str, Any]]:
        return [
            {"timestamp": "2026-05-31T18:00:00Z", "profileId": "alasr", "resourceKey": "oil", "value": 310, "source": "mock-snapshot"},
            {"timestamp": "2026-05-31T19:00:00Z", "profileId": "alasr", "resourceKey": "oil", "value": 360, "source": "mock-snapshot"},
            {"timestamp": "2026-05-31T20:00:00Z", "profileId": "alasr", "resourceKey": "oil", "value": 330, "source": "mock-snapshot"},
            {"timestamp": "2026-05-31T21:00:00Z", "profileId": "alasr", "resourceKey": "oil", "value": 420, "source": "mock-snapshot"},
            {"timestamp": "2026-05-31T22:00:00Z", "profileId": "alasr", "resourceKey": "oil", "value": 410, "source": "mock-snapshot"},
            {"timestamp": "2026-05-31T23:00:00Z", "profileId": "alasr", "resourceKey": "oil", "value": 464, "source": "mock-snapshot"},
        ]

    @staticmethod
    def _initial_acquisitions() -> list[dict[str, Any]]:
        return [
            {"timestamp": "2026-05-31T21:23:49Z", "profileId": "alasr", "imageReference": "runtime://mock/acquisition-001", "labels": ["物资", "活动PT"], "sourceTask": "每日任务"},
            {"timestamp": "2026-05-31T21:28:12Z", "profileId": "maab", "imageReference": "runtime://mock/acquisition-002", "labels": ["龙门币"], "sourceTask": "日常关卡"},
            {"timestamp": "2026-05-31T21:31:46Z", "profileId": "baasjp", "imageReference": "runtime://mock/acquisition-003", "labels": ["信用点", "青辉石"], "sourceTask": "悬赏通缉"},
        ]


class ApiHandler(BaseHTTPRequestHandler):
    state: RuntimeState

    def do_OPTIONS(self) -> None:
        self._send_json({"ok": True})

    def do_GET(self) -> None:
        try:
            parsed = urlparse(self.path)
            path = parsed.path.rstrip("/") or "/"
            query = parse_qs(parsed.query)
            if path == "/health":
                self._send_json(self.state.health())
            elif path == "/profiles":
                self._send_json({"profiles": self.state.all_profile_summaries()})
            elif path.startswith("/profiles/") and path.endswith("/summary"):
                profile_id = path.split("/")[2]
                profile = self.state.profile_summary(profile_id)
                if profile is None:
                    self._send_json({"error": f"profile not found: {profile_id}"}, status=404)
                else:
                    self._send_json({"profile": profile})
            elif path == "/runtime/status":
                self._send_json(self.state.runtime_status())
            elif path == "/logs/recent":
                limit = self._int_query(query, "limit", 80)
                self._send_json({"logs": self.state.recent_logs(limit)})
            elif path == "/resources/history":
                self._send_json({"history": self.state.resource_points()})
            elif path == "/acquisitions/recent":
                limit = self._int_query(query, "limit", 24)
                self._send_json({"acquisitions": self.state.recent_acquisitions(limit)})
            else:
                self._send_json({"error": f"unknown endpoint: {path}"}, status=404)
        except Exception as exc:
            logging.exception("Unhandled GET error")
            self.state.log("error", "http", f"GET {self.path} failed: {exc}")
            self._send_json({"error": str(exc)}, status=500)

    def do_POST(self) -> None:
        try:
            parsed = urlparse(self.path)
            path = parsed.path.rstrip("/") or "/"
            self._read_body()
            if path == "/runtime/start":
                self._send_json(self.state.start_runtime())
            elif path == "/runtime/stop":
                self._send_json(self.state.stop_runtime())
            elif path == "/runtime/restart":
                self._send_json(self.state.restart_runtime())
            elif path == "/runtime/refresh":
                self._send_json(self.state.refresh_runtime())
            elif path == "/orchestrator/shutdown":
                self._send_json(self.state.request_shutdown())
            else:
                self._send_json({"error": f"unknown endpoint: {path}"}, status=404)
        except Exception as exc:
            logging.exception("Unhandled POST error")
            self.state.log("error", "http", f"POST {self.path} failed: {exc}")
            self._send_json({"error": str(exc)}, status=500)

    def log_message(self, fmt: str, *args: Any) -> None:
        logging.info("HTTP " + fmt, *args)

    def _send_json(self, data: dict[str, Any], *, status: int = 200) -> None:
        body = json.dumps(data, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "Content-Type")
        self.end_headers()
        self.wfile.write(body)

    def _read_body(self) -> bytes:
        length = int(self.headers.get("Content-Length", "0") or 0)
        return self.rfile.read(length) if length > 0 else b""

    @staticmethod
    def _int_query(query: dict[str, list[str]], key: str, default: int) -> int:
        try:
            return int(query.get(key, [str(default)])[0])
        except ValueError:
            return default


async def ws_handler(websocket: Any, path: str | None = None) -> None:
    state = RuntimeHolder.state
    request_path = path or getattr(getattr(websocket, "request", None), "path", "/events")
    if request_path not in {"/events", "/events/"}:
        await websocket.close(code=1008, reason="unsupported endpoint")
        return
    state.ws_clients.add(websocket)
    try:
        await websocket.send(json.dumps({"type": "runtime.snapshot", "timestamp": utc_now(), "payload": state.snapshot()}, ensure_ascii=False))
        async for _message in websocket:
            await websocket.send(json.dumps({"type": "runtime.ack", "timestamp": utc_now(), "payload": {"message": "commands use HTTP POST"}}, ensure_ascii=False))
    finally:
        state.ws_clients.discard(websocket)


class RuntimeHolder:
    state: RuntimeState


def configure_logging(state_dir: Path) -> None:
    log_dir = state_dir / "logs"
    log_dir.mkdir(parents=True, exist_ok=True)
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(message)s",
        handlers=[
            logging.FileHandler(log_dir / "orchestrator.log", encoding="utf-8"),
            logging.StreamHandler(sys.stdout),
        ],
    )


async def run_servers(state: RuntimeState) -> None:
    RuntimeHolder.state = state
    state.loop = asyncio.get_running_loop()
    ApiHandler.state = state
    http_server = ThreadingHTTPServer((state.host, state.http_port), ApiHandler)
    http_thread = threading.Thread(target=http_server.serve_forever, name="alice-runtime-http", daemon=True)
    http_thread.start()
    state.log("info", SERVICE_NAME, f"HTTP API listening on http://{state.host}:{state.http_port}")

    async with serve(ws_handler, state.host, state.ws_port):
        state.log("info", SERVICE_NAME, f"WebSocket events listening on ws://{state.host}:{state.ws_port}/events")
        while not state.shutdown_event.is_set():
            await asyncio.sleep(0.25)

    http_server.shutdown()
    http_server.server_close()
    state.write_snapshot()
    state.log("info", SERVICE_NAME, "Orchestrator stopped cleanly.")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=SERVICE_NAME)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--http-port", type=int, default=DEFAULT_HTTP_PORT)
    parser.add_argument("--ws-port", type=int, default=DEFAULT_WS_PORT)
    parser.add_argument("--state-dir", default=None)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    state_dir = Path(args.state_dir).expanduser() if args.state_dir else local_state_root()
    state_dir.mkdir(parents=True, exist_ok=True)
    configure_logging(state_dir)

    lock = SingleInstanceLock(state_dir / "orchestrator.lock")
    try:
        lock.acquire()
    except RuntimeError as exc:
        logging.error(str(exc))
        return 3

    state = RuntimeState(state_dir=state_dir, host=args.host, http_port=args.http_port, ws_port=args.ws_port)

    def _signal_shutdown(_signum: int, _frame: Any) -> None:
        state.log("warning", SERVICE_NAME, "Shutdown signal received.")
        state.shutdown_event.set()

    signal.signal(signal.SIGINT, _signal_shutdown)
    if hasattr(signal, "SIGTERM"):
        signal.signal(signal.SIGTERM, _signal_shutdown)

    try:
        asyncio.run(run_servers(state))
    finally:
        lock.release()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
