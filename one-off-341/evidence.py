# One-off (to be reverted), Workflow #341 evidence (frozen model, board comment 5962396625, section 6,
# E1-E6). Every printed line starts with "DRY|". Usage:
#   evidence.py <work> <new runtime dir> <new tools dir> <old runtime dir> <old tools dir> <repo> <product sha>
import collections
import hashlib
import json
import os
import shutil
import struct
import subprocess
import sys
import time
import zipfile
import zlib

FIXTURE_ALIAS = "node.a"
FIXTURE_INSTANCE_ID = "instance_00000000000000000000000000000341"
FAILURES = []
BASE_ENV = dict(os.environ)
for name in ("ACTINGLAB_REQUIRE_SESSION_DAEMON", "ACTINGLAB_TRUSTED_REMOTE_TOKEN", "ACTINGLAB_TRUSTED_REMOTE_CLIENT_CERT",
             "ACTINGLAB_CONFIG_PATH", "ACTINGCOMMAND_RUNTIME_STATE_ROOT", "ACTINGLAB_SESSION_STATE_DIR"):
    BASE_ENV.pop(name, None)
CLIENT_ORIGINS = {"cli", "lab"}
CLIENT_PREFIXES = ("command.received", "application.", "lease.", "input.", "monitor.", "artifact.pin_")
EXPECTED_FILES = sorted([
    "apps/actinglab/src/cli_information.rs", "apps/actinglab/src/cli_result.rs", "apps/actinglab/src/commands/capabilities.rs",
    "apps/actinglab/src/commands/device_commands.rs", "apps/actinglab/src/commands/session_record.rs",
    "apps/actinglab/src/dry_run_gate.rs", "apps/actinglab/src/env_detection.rs", "apps/actinglab/src/lab2_cli.rs",
    "apps/actinglab/src/lab_package_control.rs", "apps/actinglab/src/main.rs", "apps/actinglab/src/resource_runtime_support.rs",
    "apps/actinglab/src/run_summary.rs", "apps/actinglab/src/runtime_session_adapter.rs", "apps/actinglab/src/runtime_slice_cli.rs",
    "apps/actinglab/src/session_management.rs", "contracts/README.md", "contracts/actinglab-capabilities.md",
    "contracts/actinglab-dry-run.md", "crates/lab/src/env_detection.rs",
])


def say(*parts):
    print("DRY|" + "|".join(str(part) for part in parts), flush=True)


def check(label, condition, detail=""):
    say("CHECK", label, "PASS" if condition else "FAIL", detail)
    if not condition:
        FAILURES.append(label)
    return condition


def short(text, limit=900):
    text = (text or "").strip().replace("\r", "").replace("\n", " ")
    return text if len(text) <= limit else text[:limit] + "..."


def sha_bytes(data):
    return hashlib.sha256(data).hexdigest()


def file_sha(path):
    with open(path, "rb") as handle:
        return sha_bytes(handle.read())


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


# ---------------------------------------------------------------- fixtures

def png(width, height, pixel):
    raw = b"".join(b"\x00" + b"".join(bytes(pixel(x, y)) for x in range(width)) for y in range(height))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


def solid(color):
    return png(1, 1, lambda _x, _y: color)


def write(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as handle:
        handle.write(data if isinstance(data, bytes) else data.encode("utf-8"))


PACK = json.dumps({"schema_version": "0.3", "coordinate_space": {"width": 1, "height": 1}, "targets": [
    {"type": "color", "id": "home_anchor", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
    {"type": "color", "id": "target_anchor", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]},
    {"type": "color", "id": "home_button", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0],
     "click": {"x": 10, "y": 20, "width": 4, "height": 6}}]})
PAGES = json.dumps({"schema_version": "0.3", "pages": [{"id": "arknights/home", "required": ["home_anchor"]},
                                                        {"id": "arknights/target", "required": ["target_anchor"]}]})
NAVIGATION = json.dumps({"schema_version": "0.3", "game": "arknights", "server": "cn",
                         "control_points": [{"name": "wake", "point": [3, 4], "note": "evidence wake"}],
                         "navigation": [{"id": "home_to_target", "from_page": "arknights/home", "to_page": "arknights/target",
                                         "click": {"kind": "rect", "x": 10, "y": 20, "width": 4, "height": 6}},
                                        {"id": "target_to_home", "from_page": "arknights/target", "to_page": "arknights/home",
                                         "click": {"kind": "point", "point": "2,3"}}],
                         "destructive_actions": []})
KEYS = [{"key": "resolution", "min_confidence": 1.0, "stale_below_confidence": 1.0, "ttl_ms": None, "allowed_values": ["1x1"],
         "candidates": [{"value": "1x1", "width": 1, "height": 1, "source": "evidence-scene"}]}]
DETECTIONS = json.dumps({"schema_version": "env-detections.v1", "detections": [
    {"id": "detect_resolution", "version": "1", "game_id": "arknights", "server_id": "cn", "resource_pack_id": "evidence-pack",
     "keys": KEYS},
    {"id": "detect_steps", "version": "1", "game_id": "arknights", "server_id": "cn", "resource_pack_id": "evidence-pack",
     "steps": [{"kind": "tap", "x": 0, "y": 0}], "keys": KEYS}]})
STARTUP_LOGIN = "# startup\n| **弹窗关闭 ×** | **(1205, 67)** |\n| 推进/点击继续 | (640, 360) |\n"
PACKAGE_TASK = json.dumps({
    "schema_version": "0.3", "task_id": "operator_task", "game": "arknights", "server_scope": ["cn"], "locale": "zh-CN",
    "goal": "evidence fixture", "coordinate_space": {"width": 1280, "height": 720},
    "defaults": {"template_threshold": 0.9, "color_max_distance": 20.0},
    "anchors": [{"id": "home", "template": "assets/HOME.png", "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
                 "threshold": 0.8, "color_check": None}],
    "entry_page": "home", "target_page": "home",
    "operations": [{"id": "noop", "purpose": "evidence fixture", "from": "home", "to": None,
                    "click": {"kind": "rect", "x": 1, "y": 1, "width": 1, "height": 1}, "verify_template": None,
                    "unguarded_trusted_coordinate": True, "consumes": [], "produces": []}]})


def zip_stored(path, files):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_STORED) as archive:
        for name, data in files:
            archive.writestr(zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0)), data)


def build_fixture(root):
    """Inputs only (resources, packages, scenes); every output and state path stays absent."""
    inputs = os.path.join(root, "inputs")
    resources = os.path.join(inputs, "resources")
    write(os.path.join(resources, "recognition", "arknights.cn.pack.json"), PACK)
    write(os.path.join(resources, "recognition", "arknights.cn.pages.json"), PAGES)
    write(os.path.join(resources, "navigation", "arknights.cn.navigation.json"), NAVIGATION)
    write(os.path.join(resources, "env-detection", "detections.json"), DETECTIONS)
    write(os.path.join(resources, "STARTUP-LOGIN.md"), STARTUP_LOGIN)
    semantic = os.path.join(inputs, "semantic.zip")
    zip_stored(semantic, [("control.json", '{"game":"arknights","server":"cn","entry_task_id":"task"}'),
                          ("resources/manifest.json", '{"schema_version":"0.3","entry_task_id":"task"}'),
                          ("resources/operations/task/task.json", "{}"),
                          ("resources/recognition/arknights.cn.pack.json", PACK),
                          ("resources/recognition/arknights.cn.pages.json", PAGES),
                          ("resources/navigation/arknights.cn.navigation.json", NAVIGATION)])
    safe = os.path.join(inputs, "safe.zip")
    zip_stored(safe, [("module/manifest.json", '{"schema_version":"0.2"}'), ("module/operations/task/task.json", '{"id":"task"}'),
                      ("module/operations/resources.json", "{}")])
    repo = os.path.join(inputs, "package-repo")
    write(os.path.join(repo, "operations", "resources.json"), '{"schema_version":"1.0","resources":[],"resource_count":0}')
    write(os.path.join(repo, "operations", "operator_task", "assets", "HOME.png"), solid((255, 0, 0)))
    write(os.path.join(repo, "operations", "operator_task", "task.json"), PACKAGE_TASK)
    write(os.path.join(repo, "navigation", "arknights.cn.navigation.json"),
          '{"schema_version":"0.3","control_points":[{"name":"home","point":[1,1]}]}')
    promote_repo = os.path.join(inputs, "promote-repo")
    write(os.path.join(promote_repo, "ours", "operations", "resources.json"),
          '{"schema_version":"1.0","resources":[{"id":"keep"}],"resource_count":1}')
    os.makedirs(os.path.join(promote_repo, "ours", "recognition"), exist_ok=True)
    write(os.path.join(inputs, "red.png"), solid((255, 0, 0)))
    write(os.path.join(inputs, "blue.png"), solid((0, 0, 255)))
    write(os.path.join(inputs, "standby.png"), solid((1, 1, 1)))
    write(os.path.join(inputs, "source.png"), png(12, 10, lambda x, y: ((x * 19 + y * 7) % 256, (x * 5 + y * 23) % 256, (x * y * 11) % 256)))
    config = os.path.join(root, "config", "config.json")
    write(config, json.dumps({"instances": {FIXTURE_ALIAS: {}}}))
    with open(semantic, "rb") as handle:
        semantic_sha = sha_bytes(handle.read())
    return {"root": root, "inputs": inputs, "resources": resources, "semantic": semantic, "semantic_sha": semantic_sha,
            "safe": safe, "repo": repo, "promote_repo": promote_repo, "config": config,
            "red": os.path.join(inputs, "red.png"), "blue": os.path.join(inputs, "blue.png"),
            "standby": os.path.join(inputs, "standby.png"), "source": os.path.join(inputs, "source.png"),
            "local": os.path.join(root, "local"), "session": os.path.join(root, "session-state"),
            "out": os.path.join(root, "out"), "runtime": os.path.join(root, "no-runtime")}


def lab_env(fixture, runtime_root=None):
    env = dict(BASE_ENV)
    env["LOCALAPPDATA"] = fixture["local"]
    env["APPDATA"] = fixture["local"]
    env["ACTINGLAB_CONFIG_PATH"] = fixture["config"]
    env["ACTINGCOMMAND_RUNTIME_STATE_ROOT"] = runtime_root or fixture["runtime"]
    env["ACTINGLAB_SESSION_STATE_DIR"] = fixture["session"]
    return env


def run_exe(args, env, cwd=None, timeout=300):
    try:
        result = subprocess.run(args, capture_output=True, timeout=timeout, env=env, cwd=cwd)
    except subprocess.TimeoutExpired:
        return None, "", "timeout"
    return result.returncode, result.stdout.decode("utf-8", "replace"), result.stderr.decode("utf-8", "replace")


def lab(exe, args, label, fixture, runtime_root=None, limit=1500, quiet=False):
    code, out, err = run_exe([exe, "--json", *args], lab_env(fixture, runtime_root), cwd=fixture["root"])
    try:
        value = json.loads(out)
    except ValueError:
        value = None
    if not quiet:
        say(label, "exit", code, "envelope", short(out, limit) if out else short(err, 600))
    return code, value, out


def data_of(value):
    return (value or {}).get("data") or {}


def error_of(value):
    return (value or {}).get("error") or {}


def snapshot(root, skip=()):
    files, dirs = {}, set()
    for folder, subdirs, names in os.walk(root):
        subdirs[:] = [name for name in subdirs if os.path.join(folder, name) not in skip]
        dirs.add(os.path.relpath(folder, root))
        for name in names:
            path = os.path.join(folder, name)
            files[os.path.relpath(path, root)] = file_sha(path)
    return files, dirs


# ---------------------------------------------------------------- daemon and ledger

def fixture_config(config_dir, state_root):
    frames = []
    for color in ((224, 225, 227), (255, 229, 26)):
        frames.append({"width": 64, "height": 36, "rgb": list(bytes(color) * (64 * 36))})
    config = {"schema_version": "actingcommand.actingd.config.v1", "state_root": state_root, "bind_host": "127.0.0.1",
              "bind_port": 0, "secret_fingerprint_salt": "oneoff-341-dry-run-fixture-salt-value",
              "instances": [{"alias": FIXTURE_ALIAS, "instance_id": FIXTURE_INSTANCE_ID,
                             "fixture_backend": {"frames": frames, "max_inputs": 4}}]}
    path = os.path.join(config_dir, "actingd.json")
    write(path, json.dumps(config))
    return path


class Daemon:
    def __init__(self, runtime_dir, root, label):
        self.label = label
        self.root = root
        self.runtime_root = os.path.join(root, "runtime-state")
        os.makedirs(self.runtime_root)
        self.config = fixture_config(os.path.join(root, "config"), self.runtime_root)
        self.actingd = os.path.join(runtime_dir, "actingcommand-actingd.exe")
        self.actingctl = os.path.join(runtime_dir, "actingctl.exe")
        self.process = None

    def start(self):
        # Readiness is the published runtime-info.json, so no client request precedes the commands.
        self.out = open(os.path.join(self.root, "actingd.out"), "wb")
        self.err = open(os.path.join(self.root, "actingd.err"), "wb")
        self.process = subprocess.Popen([self.actingd, "--config", self.config], stdout=self.out, stderr=self.err,
                                        cwd=os.path.dirname(self.config))
        started = time.time()
        info = os.path.join(self.runtime_root, "runtime-info.json")
        ready = False
        while time.time() - started < 90 and self.process.poll() is None:
            if os.path.isfile(info):
                ready = True
                break
            time.sleep(0.5)
        time.sleep(1.0)
        say(self.label, "actingd ready (runtime-info.json, no client request)", ready, "after_s", round(time.time() - started, 1))
        check(self.label + ".actingd_ready", ready and self.process.poll() is None, "")
        return ready

    def stop(self):
        code, out, err = None, "", ""
        for _ in range(10):
            code, out, err = run_exe([self.actingctl, "request-shutdown", "--state-root", self.runtime_root, "--wait", "60"],
                                     dict(BASE_ENV), timeout=120)
            if code == 0 or self.process.poll() is not None:
                break
            time.sleep(1)
        say(self.label, "request-shutdown exit", code, short(out, 300), short(err, 300))
        try:
            exit_code = self.process.wait(timeout=120)
        except subprocess.TimeoutExpired:
            self.process.kill()
            exit_code = "killed"
        self.out.close()
        self.err.close()
        say(self.label, "actingd exit", exit_code)
        check(self.label + ".actingd_shutdown", code == 0 and exit_code != "killed", f"request-shutdown {code} actingd {exit_code}")


def ledger_events(ledger, root):
    after, events = 0, []
    while True:
        code, out, err = run_exe([ledger, "--state-root", root, "events", "--after", str(after), "--limit", "500"], dict(BASE_ENV))
        if code != 0:
            return None, f"exit {code}: {short(err or out)}"
        try:
            data = json.loads(out)["data"]
            batch = data["events"]
        except (ValueError, KeyError, TypeError) as error:
            return None, f"unreadable events page: {error}: {short(out)}"
        if not batch:
            return events, ""
        events.extend(batch)
        after = batch[-1]["sequence"]
        if after >= data["through_sequence"]:
            return events, ""


def origin_of(event):
    origin = event.get("origin") or {}
    return str(origin.get("source", "")).lower(), str(origin.get("actor", "")).lower()


def is_client(event):
    source, actor = origin_of(event)
    kind = str(event.get("event_type", ""))
    return source in CLIENT_ORIGINS or actor in CLIENT_ORIGINS or kind.startswith(CLIENT_PREFIXES) or "signature" in kind


def request_of(event):
    return (event.get("links") or {}).get("request_id")


def split_at_shutdown(label, events):
    """Returns (events before the shutdown request's first event, the shutdown request id)."""
    client = [event for event in events if is_client(event) and request_of(event)]
    if not client:
        say(label, "no client event in the ledger")
        return list(events), None
    shutdown = request_of(client[-1])
    owned = [event for event in events if request_of(event) == shutdown]
    first = min(event["sequence"] for event in owned)
    mentions = any("shutdown" in json.dumps(event.get("payload"), ensure_ascii=False).lower() for event in owned)
    say(label, "shutdown request", shutdown, "first sequence", first, "events", json.dumps([event.get("event_type") for event in owned]),
        "payload mentions shutdown", mentions)
    return [event for event in events if event["sequence"] < first], shutdown


def ledger_summary(label, ledger, root):
    events, failure = ledger_events(ledger, root)
    if events is None:
        check(label + ".ledger_readable", False, failure)
        return []
    types = collections.Counter(event.get("event_type") for event in events)
    say(label, "ledger events", len(events), "types", json.dumps(dict(sorted(types.items()))))
    return events


# ---------------------------------------------------------------- E1 positive control (v0.9.1)

def e1(old_runtime, old_lab, old_ledger, work):
    say("E1", "v0.9.1 actinglab ignores --dry-run: session app restart and tap reach the fixture actingd")
    root = os.path.join(work, "e1")
    fixture = build_fixture(root)
    daemon = Daemon(old_runtime, os.path.join(root, "daemon"), "E1")
    if not daemon.start():
        return
    lab(old_lab, ["--dry-run", "session", "app", "restart"], "E1 v0.9.1 --dry-run session app restart", fixture, daemon.runtime_root)
    lab(old_lab, ["--dry-run", "tap", "10", "20"], "E1 v0.9.1 --dry-run tap 10 20", fixture, daemon.runtime_root)
    daemon.stop()
    events = ledger_summary("E1", old_ledger, daemon.runtime_root)
    before, _ = split_at_shutdown("E1", events)
    hits = [event for event in before if is_client(event)]
    for event in hits:
        source, actor = origin_of(event)
        say("E1", "client event", event["sequence"], event.get("event_type"), f"source={source} actor={actor}",
            "request", request_of(event), short(json.dumps(event.get("payload"), ensure_ascii=False), 400))
    requests = {request_of(event) for event in hits if request_of(event) and "cli" in origin_of(event)}
    application = [event for event in hits if "application" in json.dumps(event.get("payload")).lower()
                   or str(event.get("event_type", "")).startswith("application.")]
    inputs = [event for event in hits if str(event.get("event_type", "")).startswith(("input.", "lease."))]
    say("E1", "client requests before shutdown", len(requests), "application-related events", len(application),
        "input/lease events", len(inputs))
    check("E1.positive_control_cli_requests_reached_runtime", len(requests) >= 2 and bool(hits), json.dumps(sorted(requests)))


# ---------------------------------------------------------------- E2 refusals and E3 previews (new build, one session)

def refusal(new_lab, fixture, runtime_root, label, args, alternative, extra=None, forbidden=()):
    code, value, _ = lab(new_lab, args, "E2 " + label, fixture, runtime_root)
    error = error_of(value)
    details = error.get("details") or {}
    ok = (code == 2 and error.get("code") == "dry_run_unsupported" and error.get("blocked_by") == []
          and details.get("dry_run") is True and details.get("executed") is False
          and details.get("read_only_alternative") == alternative)
    for key, expected in (extra or {}).items():
        ok = ok and details.get(key) == expected
    check("E2." + label, ok, json.dumps({"exit": code, "code": error.get("code"), "blocked_by": error.get("blocked_by"), "details": details}, ensure_ascii=False))
    for path in forbidden:
        check("E2." + label + ".absent " + os.path.basename(path), not os.path.exists(path), path)
    return value


def e2_e3(new_runtime, new_lab, new_ledger, work):
    say("E2", "refusals and E3 previews: new build, one fixture actingd session, no client request but the final shutdown")
    root = os.path.join(work, "e23")
    fixture = build_fixture(root)
    out = fixture["out"]
    os.makedirs(out)
    config_before = file_sha(fixture["config"])
    daemon = Daemon(new_runtime, os.path.join(root, "daemon"), "E23")
    watched_skip = {os.path.join(root, "daemon")}
    before_files, before_dirs = snapshot(root, watched_skip)
    if not daemon.start():
        return {}
    rt = daemon.runtime_root
    alt_status = "session status --diagnostics"
    for verb in ("launch", "stop", "force-stop", "restart"):
        refusal(new_lab, fixture, rt, f"session app {verb}", ["--dry-run", "session", "app", verb], alt_status,
                {"action": verb, "instance": FIXTURE_ALIAS, "form": f"session app {verb}"})
        refusal(new_lab, fixture, rt, f"session instance app {verb}", ["--dry-run", "session", "instance", "app", verb], alt_status,
                {"action": verb, "instance": FIXTURE_ALIAS})
    leading = refusal(new_lab, fixture, rt, "leading --dry-run session app restart", ["--dry-run", "session", "app", "restart"], alt_status)
    trailing = refusal(new_lab, fixture, rt, "trailing session app restart --dry-run", ["session", "app", "restart", "--dry-run"], alt_status)
    check("E2.trailing_equals_leading_session_app", canonical(leading) == canonical(trailing), "")
    refusal(new_lab, fixture, rt, "touch-probe", ["--dry-run", "touch-probe"], alt_status)
    refusal(new_lab, fixture, rt, "capture --out", ["--dry-run", "capture", "--out", os.path.join(out, "capture.png")],
            "capture diagnose", forbidden=[os.path.join(out, "capture.png")])
    refusal(new_lab, fixture, rt, "session capture --out", ["--dry-run", "session", "capture", "--out", os.path.join(out, "session-capture.png")],
            "capture diagnose", forbidden=[os.path.join(out, "session-capture.png")])
    package = ["--zip", fixture["semantic"], "--expected-sha256", fixture["semantic_sha"]]
    refusal(new_lab, fixture, rt, "observe --scene --with-frame <path>",
            ["--dry-run", "observe", "--scene", fixture["red"], "--with-frame", os.path.join(out, "observe.png"), *package],
            "observe", forbidden=[os.path.join(out, "observe.png")])
    refusal(new_lab, fixture, rt, "observe --capture --with-frame <path>",
            ["--dry-run", "--instance", FIXTURE_ALIAS, "observe", "--capture", "--with-frame", os.path.join(out, "observe-capture.png"), *package],
            "observe", forbidden=[os.path.join(out, "observe-capture.png")])
    code, value, _ = lab(new_lab, ["--resource-root", fixture["resources"], "--game", "arknights", "--server", "cn", "--dry-run",
                                   "observe", "--targets", "home_button", "--scene", fixture["red"], "--with-frame", *package],
                         "E2 bare --with-frame is not refused", fixture, rt)
    check("E2.bare_with_frame_not_refused", error_of(value).get("code") != "dry_run_unsupported" and code == 0
          and "frame_path" not in data_of(value), f"exit {code} code {error_of(value).get('code')}")
    refusal(new_lab, fixture, rt, "runtime reset", ["--dry-run", "runtime", "reset", "--state-root", rt, "--instance", FIXTURE_ALIAS],
            None, {"instance": FIXTURE_ALIAS})
    for action, alternative in (("start", "record status"), ("stop", "record status"), ("step", "record candidates"),
                                ("amend", "record candidates")):
        refusal(new_lab, fixture, rt, f"record {action}", ["--dry-run", "record", action, "--task-id", "evidence"], alternative,
                {"action": action}, forbidden=[fixture["session"]])
    refusal(new_lab, fixture, rt, "session record start", ["--dry-run", "session", "record", "start", "--task-id", "evidence"],
            "record status", {"action": "start"}, forbidden=[fixture["session"]])
    for action in ("set", "clear"):
        extra = ["--scene", fixture["red"]] if action == "set" else []
        refusal(new_lab, fixture, rt, f"session monitor-policy {action}", ["--dry-run", "session", "monitor-policy", action, *extra],
                "session monitor-policy status", {"action": action})
    replay = ("actingledger --state-root <historical-root> signatures --through <sequence> --catalog-state-root "
              "<registered-ledger-root> --catalog-through <sequence>")
    for operation in ("register", "match", "retire"):
        refusal(new_lab, fixture, rt, f"lab signatures {operation}", ["--dry-run", "lab", "signatures", operation], replay,
                {"form": f"lab signatures {operation}"})
    refusal(new_lab, fixture, rt, "lab debug-package", ["--dry-run", "lab", "debug-package"], "lab validate")
    refusal(new_lab, fixture, rt, "lab unpin", ["--dry-run", "lab", "unpin"], "lab watch")
    refusal(new_lab, fixture, rt, "lab export-evidence", ["--dry-run", "lab", "export-evidence"], None)
    refusal(new_lab, fixture, rt, "package run --out", ["--dry-run", "--instance", FIXTURE_ALIAS, "package", "run", "--zip", fixture["safe"],
                                                        "--out", os.path.join(out, "package-run.zip")],
            "package dry-run", forbidden=[os.path.join(out, "package-run.zip")])
    refusal(new_lab, fixture, rt, "package bundle", ["--dry-run", "package", "bundle", "--out", os.path.join(out, "bundle")],
            "package digest", forbidden=[os.path.join(out, "bundle")])
    refusal(new_lab, fixture, rt, "resource restore", ["--dry-run", "resource", "restore", "--repo", os.path.join(out, "restored")],
            None, forbidden=[os.path.join(out, "restored")])
    refusal(new_lab, fixture, rt, "run export", ["--dry-run", "--run-root", os.path.join(root, "runs"), "run", "export", "run-evidence",
                                                 "--out", os.path.join(out, "run-export.zip")],
            None, {"run_id": "run-evidence"}, forbidden=[os.path.join(out, "run-export.zip")])
    refusal(new_lab, fixture, rt, "report export", ["--dry-run", "report", "export", "--last-error", "--out", os.path.join(out, "report.zip")],
            None, forbidden=[os.path.join(out, "report.zip")])

    say("E3", "previews")
    previews = {}
    for label, args, action in (("tap", ["tap", "10", "20"], {"type": "tap", "x": 10, "y": 20}),
                                ("swipe", ["swipe", "1", "2", "30", "40", "300"], {"type": "swipe", "x1": 1, "y1": 2, "x2": 30, "y2": 40, "duration_ms": 300}),
                                ("long-tap", ["long-tap", "10", "20", "900"], {"type": "long-tap", "x": 10, "y": 20, "duration_ms": 900}),
                                ("key back", ["key", "back"], {"type": "key", "key": "4"}),
                                ("text hello", ["text", "hello"], {"type": "text", "text": "hello"})):
        code, value, _ = lab(new_lab, ["--dry-run", *args], "E3 --dry-run " + label, fixture, rt)
        data = data_of(value)
        check("E3." + label, code == 0 and data.get("status") == "planned" and data.get("dry_run") is True
              and data.get("executed") is False and (data.get("input_outcome") or {}).get("input_stage") == "not_submitted"
              and "lease" in (data.get("not_checked") or []) and data.get("instance") == FIXTURE_ALIAS and data.get("action") == action,
              json.dumps({"exit": code, "status": data.get("status"), "action": data.get("action"), "instance": data.get("instance")}))
        previews[label] = value
    code, trailing, _ = lab(new_lab, ["tap", "10", "20", "--dry-run"], "E3 trailing tap 10 20 --dry-run", fixture, rt)
    check("E3.trailing_tap_equals_leading", code == 0 and canonical(trailing) == canonical(previews["tap"]), "")
    code, value, _ = lab(new_lab, ["--dry-run", "config", "set", "run_root", os.path.join(root, "configured-runs")], "E3 --dry-run config set", fixture, rt)
    data = data_of(value)
    check("E3.config_set", code == 0 and data.get("status") == "validated" and data.get("dry_run") is True and data.get("persisted") is False
          and file_sha(fixture["config"]) == config_before, json.dumps({"exit": code, "status": data.get("status"), "persisted": data.get("persisted"),
                                                                       "config_sha256_unchanged": file_sha(fixture["config"]) == config_before}))
    previews["config set"] = value
    detect = ["--resource-root", fixture["resources"], "--game", "arknights", "--server", "cn", "--instance", "fixture:5555",
              "detect", "--task", "detect_resolution", "--scene", fixture["red"]]
    code, value, _ = lab(new_lab, ["--dry-run", *detect], "E3 --dry-run detect (detector without steps)", fixture, rt, limit=2500)
    data = data_of(value)
    check("E3.detect", code == 0 and data.get("status") == "validated" and data.get("dry_run") is True and data.get("persisted") is False
          and "result_path" not in data and data.get("steps_executed") is False and isinstance(data.get("result"), dict),
          json.dumps({"exit": code, "status": data.get("status"), "persisted": data.get("persisted"), "next": data.get("next")}, ensure_ascii=False))
    previews["detect"] = value
    daemon.stop()

    after_files, after_dirs = snapshot(root, watched_skip)
    salt = os.path.join("local", "ActingCommand", "actinglab", "env-detection", ".local_salt")
    new_files = sorted(set(after_files) - set(before_files))
    changed = sorted(name for name in before_files if name in after_files and before_files[name] != after_files[name])
    removed = sorted(set(before_files) - set(after_files))
    allowed_dirs = {os.path.dirname(salt)}
    parent = os.path.dirname(salt)
    while parent:
        allowed_dirs.add(parent)
        parent = os.path.dirname(parent)
    new_dirs = sorted(set(after_dirs) - set(before_dirs))
    say("E2", "file surface: new files", json.dumps(new_files), "changed", json.dumps(changed), "removed", json.dumps(removed), "new dirs", json.dumps(new_dirs))
    check("E2E3.file_surface_only_registered_salt", set(new_files) <= {salt} and not changed and not removed
          and set(new_dirs) <= allowed_dirs, "")
    check("E2E3.session_state_dir_absent", not os.path.exists(fixture["session"]), fixture["session"])
    events = ledger_summary("E23", new_ledger, daemon.runtime_root)
    before, shutdown = split_at_shutdown("E23", events)
    offenders = [event for event in before if is_client(event)]
    for event in offenders:
        say("E23", "client event before shutdown", event["sequence"], event.get("event_type"), json.dumps(event.get("origin")))
    check("E23.zero_client_requests_before_shutdown", not offenders,
          json.dumps({"events_before_shutdown": len(before), "types": dict(collections.Counter(event.get("event_type") for event in before))}))
    stray = sorted({request_of(event) for event in events if is_client(event) and request_of(event) and request_of(event) != shutdown})
    check("E23.only_client_request_is_shutdown", not stray, json.dumps(stray))

    say("E3", "detect copy without --dry-run: same result after volatile fields")
    copy_root = os.path.join(work, "e3-detect-copy")
    copy_fixture = build_fixture(copy_root)
    code, stored, _ = lab(new_lab, detect, "E3 detect without --dry-run (copy)", copy_fixture, limit=2500)

    def stable(result):
        if isinstance(result, dict):
            return {key: stable(value) for key, value in result.items() if not key.endswith("_unix_ms")}
        if isinstance(result, list):
            return [stable(value) for value in result]
        return result
    check("E3.detect_result_matches_real_run", code == 0 and stable(data_of(stored).get("result")) == stable(data.get("result"))
          and data_of(stored).get("status") == "detected" and bool(data_of(stored).get("result_path")),
          short(canonical(stable(data.get("result"))), 500))
    return previews


# ---------------------------------------------------------------- E4 invariance against v0.9.1

def mask_paths(first, second, path=()):
    if type(first) is not type(second):
        return [path]
    if isinstance(first, dict):
        if set(first) != set(second):
            return [path]
        masks = []
        for key in first:
            masks.extend(mask_paths(first[key], second[key], path + (key,)))
        return masks
    if isinstance(first, list):
        if len(first) != len(second):
            return [path]
        masks = []
        for index, (left, right) in enumerate(zip(first, second)):
            masks.extend(mask_paths(left, right, path + (index,)))
        return masks
    return [] if first == second else [path]


def apply_masks(value, masks):
    value = json.loads(json.dumps(value))
    for path in masks:
        if not path:
            return "<MASK>"
        cursor = value
        try:
            for step in path[:-1]:
                cursor = cursor[step]
            cursor[path[-1]] = "<MASK>"
        except (KeyError, IndexError, TypeError):
            pass
    return value


def normalize_root(value, root):
    variants = sorted({root, root.replace("\\", "/"), "\\\\?\\" + root, os.path.realpath(root)}, key=len, reverse=True)
    if isinstance(value, dict):
        return {key: normalize_root(item, root) for key, item in value.items()}
    if isinstance(value, list):
        return [normalize_root(item, root) for item in value]
    if isinstance(value, str):
        for variant in variants:
            index = value.lower().find(variant.lower())
            while index >= 0:
                value = value[:index] + "<ROOT>" + value[index + len(variant):]
                index = value.lower().find(variant.lower())
        return value
    return value


def record_seed(old_lab, fixture, prefix):
    state = os.path.join(fixture["root"], "record-state")
    base = ["--instance", FIXTURE_ALIAS, *prefix, "record"]
    steps = [["start", "--state-dir", state, "--task-id", "daily-check"],
             ["step", "--state-dir", state, "--kind", "anchor", "--step-id", "home-anchor", "--id", "page/home", "--region", "2,3,4,5",
              "--frame", fixture["source"]],
             ["step", "--state-dir", state, "--kind", "anchor", "--step-id", "mail-anchor", "--id", "page/mail", "--region", "2,3,4,5",
              "--frame", fixture["source"]],
             ["step", "--state-dir", state, "--kind", "operation", "--step-id", "home-to-mail", "--from", "page/home", "--to", "page/mail",
              "--click", "5,6"]]
    codes = []
    for step in steps:
        code, _, _ = lab(old_lab, base + step, "seed", fixture, quiet=True)
        codes.append(code)
    return state, codes


def e4(old_lab, new_lab, new_runtime, work, contract):
    say("E4", "table B envelopes: v0.9.1 twice on fresh copies (differing leaves masked, run root normalized), then the new build")
    semantic = lambda f: ["--zip", f["semantic"], "--expected-sha256", f["semantic_sha"]]
    common = lambda f: ["--resource-root", f["resources"], "--run-root", os.path.join(f["root"], "runs"), "--game", "arknights", "--server", "cn"]
    cases = [
        ("do", "do", lambda f, s: [*common(f), "--dry-run", "do", "home_button", "--scene", f["red"], *semantic(f)], None, False),
        ("do (blocked target)", "do", lambda f, s: [*common(f), "--dry-run", "do", "home_button", "--scene", f["blue"], *semantic(f)], None, False),
        ("ensure (already at target)", "ensure", lambda f, s: [*common(f), "--dry-run", "ensure", "home", "--scene", f["red"], *semantic(f)], None, False),
        ("ensure (route)", "ensure", lambda f, s: [*common(f), "--dry-run", "ensure", "target", "--scene", f["red"], *semantic(f)], None, False),
        ("tap-target", "tap-target", lambda f, s: [*common(f), "--dry-run", "tap-target", "home_button", "--scene", f["red"], *semantic(f)], None, False),
        ("navigate", "navigate", lambda f, s: [*common(f), "--dry-run", "navigate", "--to", "target", "--scene", f["red"], *semantic(f)], None, False),
        ("stream", "stream", lambda f, s: ["--instance", FIXTURE_ALIAS, "stream", "--dry-run", "--max-frames", "2"], None, False),
        ("stream --input-relay tap 10 20", "stream", lambda f, s: ["--instance", FIXTURE_ALIAS, "--dry-run", "stream", "--input-relay", "tap", "10", "20"], None, False),
        ("session stream", "session stream", lambda f, s: ["--instance", FIXTURE_ALIAS, "--dry-run", "session", "stream", "--max-frames", "2"], None, False),
        ("stream check", None, lambda f, s: ["--instance", FIXTURE_ALIAS, "--dry-run", "stream", "check"], None, False),
        ("session recover --to home", "session recover", lambda f, s: [*common(f), "--dry-run", "session", "recover", "--to", "home", "--scene", f["blue"]], None, False),
        ("session recover standby", "session recover", lambda f, s: [*common(f), "--dry-run", "session", "recover", "--scene", f["standby"]], None, False),
        ("session recover --startup-login", "session recover", lambda f, s: [*common(f), "--dry-run", "session", "recover", "--startup-login", "--to", "home", "--scene", f["standby"]], None, False),
        ("record build-task", "record build-task", lambda f, s: ["--instance", FIXTURE_ALIAS, "--game", "arknights", "--server", "cn", "--dry-run", "record", "build-task", "--state-dir", s, "--out", os.path.join(f["root"], "built"), "--locale", "zh-CN"], [], False),
        ("session record build-task", "session record build-task", lambda f, s: ["--instance", FIXTURE_ALIAS, "--game", "arknights", "--server", "cn", "--dry-run", "session", "record", "build-task", "--state-dir", s, "--out", os.path.join(f["root"], "built"), "--locale", "zh-CN"], ["session"], False),
        ("record promote", "record promote", lambda f, s: ["--instance", FIXTURE_ALIAS, "--game", "arknights", "--server", "cn", "--dry-run", "record", "promote", "--state-dir", s, "--repo", f["promote_repo"], "--locale", "zh-CN"], [], False),
        ("session record promote", "session record promote", lambda f, s: ["--instance", FIXTURE_ALIAS, "--game", "arknights", "--server", "cn", "--dry-run", "session", "record", "promote", "--state-dir", s, "--repo", f["promote_repo"], "--locale", "zh-CN"], ["session"], False),
        ("package build-task", "package build-task", lambda f, s: [*common(f), "--dry-run", "package", "build-task", "--repo", f["repo"], "--task", "operator_task", "--out", os.path.join(f["root"], "task.zip")], None, False),
        ("package build-pack", "package build-pack", lambda f, s: [*common(f), "--dry-run", "package", "build-pack", "--repo", f["repo"], "--out", os.path.join(f["root"], "pack.zip")], None, False),
        ("resource convert", "resource convert", lambda f, s: ["--dry-run", "resource", "convert", "--repo", f["repo"]], None, False),
        ("detect with steps (planned)", "detect", lambda f, s: ["--dry-run", "--resource-root", f["resources"], "--game", "arknights", "--server", "cn", "--instance", "fixture:5555", "detect", "--task", "detect_steps", "--scene", f["red"]], None, False),
        ("lab run (refused)", "lab run", lambda f, s: ["--dry-run", "lab", "run"], None, False),
        ("package dry-run (refused)", "package dry-run", lambda f, s: ["--dry-run", "package", "dry-run"], None, False),
        ("scheduling compile (refused)", "scheduling compile", lambda f, s: ["--dry-run", "scheduling", "compile"], None, False),
        ("scheduling timeline (refused)", "scheduling timeline", lambda f, s: ["--dry-run", "scheduling", "timeline"], None, False),
        ("do --capture on the fixture actingd", "do", lambda f, s: ["--instance", FIXTURE_ALIAS, "--dry-run", "do", "--capture", "home_button", *semantic(f)], None, True),
    ]
    marker_of = {entry["command"]: entry for entry in contract}
    equal = 0
    for index, (name, declared, build_args, seed_prefix, daemon_case) in enumerate(cases):
        runs = []
        for slot, exe in (("c1", old_lab), ("c2", old_lab), ("c3", new_lab)):
            root = os.path.join(work, "e4", f"case{index:02d}", slot)
            fixture = build_fixture(root)
            state = None
            if seed_prefix is not None:
                state, codes = record_seed(old_lab, fixture, seed_prefix)
                check(f"E4.{name}.{slot}.v0.9.1 seed (record start, two anchor steps, one operation step)",
                      all(code == 0 for code in codes), json.dumps(codes))
            daemon = None
            runtime_root = None
            if daemon_case:
                daemon = Daemon(new_runtime, os.path.join(root, "daemon"), f"E4 {name} {slot}")
                daemon.start()
                runtime_root = daemon.runtime_root
            code, value, out = lab(exe, build_args(fixture, state), f"E4 {name} {slot}", fixture, runtime_root, quiet=True)
            if daemon:
                daemon.stop()
            envelope = value if value is not None else {"raw_stdout": out}
            runs.append((code, normalize_root(envelope, root)))
        masks = mask_paths(runs[0][1], runs[1][1])
        masked = [apply_masks(envelope, masks) for _, envelope in runs]
        hashes = [sha_bytes(canonical(envelope).encode("utf-8")) for envelope in masked]
        say("E4", name, "exit v0.9.1", runs[0][0], runs[1][0], "new", runs[2][0], "masked leaves", len(masks),
            "sha256 v0.9.1", hashes[0][:16], "new", hashes[2][:16], "ok", runs[2][1].get("ok") if isinstance(runs[2][1], dict) else None)
        say("E4", name, "masked envelope (new build)", short(canonical(masked[2]), 700))
        check("E4." + name + ".mask_leaves_only", all(len(path) >= 2 for path in masks), json.dumps([list(path) for path in masks][:12]))
        same = runs[0][0] == runs[2][0] and runs[1][0] == runs[0][0] and hashes[0] == hashes[1] == hashes[2]
        if not same:
            say("E4", name, "masked envelope (v0.9.1)", short(canonical(masked[0]), 1500))
        equal += 1 if same else 0
        check("E4." + name + ".identical", same, f"exit {runs[0][0]}/{runs[2][0]}")
        new_value = runs[2][1]
        if declared and runs[2][0] == 0 and isinstance(new_value, dict):
            entry = marker_of.get(declared) or {}
            mode = entry.get("dry_run_mode")
            marker = entry.get("dry_run_marker")
            if mode == "mixed":
                marker = next((form.get("dry_run_marker") for form in entry.get("dry_run_forms", []) if form.get("dry_run_mode") == "preview"), None)
            data = new_value.get("data") or {}
            if mode in ("preview", "mixed") and marker:
                carried = {"dry_run": data.get("dry_run") is True, "executed_false": data.get("executed") is False,
                           "capture_dry_run": (data.get("capture") or {}).get("dry_run") is True}.get(marker, False)
                check("E5.marker " + name, carried, f"{declared}: {mode}/{marker}")
    say("E4", "cases", len(cases), "identical", equal)


# ---------------------------------------------------------------- E5 contract

def e5(new_lab, work, ratchet, doc_text, previews):
    say("E5", "capabilities and help declare dry_run_mode for every entry")
    root = os.path.join(work, "e5")
    fixture = build_fixture(root)
    results = {}
    for command in ("capabilities", "help"):
        code, value, _ = lab(new_lab, [command], f"E5 {command}", fixture, quiet=True)
        entries = data_of(value).get("commands") or []
        modes = collections.Counter(entry.get("dry_run_mode") for entry in entries)
        say("E5", command, "exit", code, "entries", len(entries), "modes", json.dumps(dict(sorted((str(k), v) for k, v in modes.items()))))
        bad = []
        for entry in entries:
            mode = entry.get("dry_run_mode")
            if mode not in ("preview", "refused", "no_effect", "mixed"):
                bad.append(f"{entry.get('command')}: {mode}")
            elif mode == "refused" and not entry.get("dry_run_refusal_code"):
                bad.append(f"{entry.get('command')}: refused without code")
            elif mode == "preview" and not entry.get("dry_run_marker"):
                bad.append(f"{entry.get('command')}: preview without marker")
            elif mode == "mixed" and not entry.get("dry_run_forms"):
                bad.append(f"{entry.get('command')}: mixed without forms")
        check(f"E5.{command}_declared", code == 0 and entries and not bad, json.dumps(bad))
        results[command] = entries
    for name in ("session app restart", "tap", "observe", "record", "lab run", "package dry-run", "operation run", "devices"):
        entry = next((entry for entry in results.get("capabilities", []) if entry.get("command") == name), {})
        say("E5", "entry", name, json.dumps({key: entry.get(key) for key in ("status", "dry_run_mode", "dry_run_marker", "dry_run_refusal_code", "dry_run_forms") if key in entry}, ensure_ascii=False))
    code, value, _ = lab(new_lab, ["help"], "E5 help", fixture, quiet=True)
    option = next((item for item in data_of(value).get("global_options", []) if str(item).startswith("--dry-run")), None)
    say("E5", "help --dry-run line", option)
    check("E5.help_dry_run_line", option == "--dry-run (global; read at any argv position; commands[].dry_run_mode states each command's behaviour)", option)
    for label, value in previews.items():
        check("E5.marker " + label, data_of(value).get("dry_run") is True, label)
    rows = [line for line in doc_text.splitlines() if line.startswith("| `")]
    documented = set()
    for line in rows:
        cell = line.split("|")[1].strip()
        if cell.startswith("`") and cell.endswith("`"):
            documented.add(cell.strip("`"))
    missing = [command for command in ratchet if command not in documented]
    elided = [command for command in missing if command.startswith(("resource import-", "resource drift-"))]
    say("E5", "document rows", len(rows), "ratchet commands", len(ratchet), "named rows", len([c for c in ratchet if c in documented]),
        "rows naming the two elided reserved routes", sum(1 for line in rows if "import route" in line or "drift route" in line))
    check("E5.document_covers_ratchet", len(ratchet) == 134 and missing == elided and len(elided) == 2
          and sum(1 for line in rows if "import route" in line or "drift route" in line) == 2, json.dumps(missing))
    return results.get("capabilities", [])


# ---------------------------------------------------------------- E6 scope

def git(repo, *args):
    return subprocess.run(["git", "-C", repo, *args], capture_output=True, check=True).stdout.decode("utf-8")


def e6(repo, product_sha):
    say("E6", "scope of the product commit against main")
    base = git(repo, "merge-base", "origin/main", product_sha).strip()
    files = sorted(line for line in git(repo, "diff", "--name-only", base, product_sha).splitlines() if line)
    for line in git(repo, "diff", "--stat", base, product_sha).splitlines():
        say("E6", "stat", line)
    check("E6.file_list_is_the_model_list", files == EXPECTED_FILES, json.dumps(sorted(set(files) ^ set(EXPECTED_FILES))))
    forbidden = [path for path in files if path.startswith(("tests/", "apps/actinglab/tests/", "apps/actinglab/src/tests/", "ratchet/",
                                                             "tools/", "crates/ledger", "crates/runtime-host", "crates/actingcommand-contract"))
                 or "golden" in path or path.endswith(".schema.json")]
    check("E6.no_forbidden_paths", not forbidden, json.dumps(forbidden))
    say("E6", "merge-base", base)


def run(work, new_runtime, new_tools, old_runtime, old_tools, repo, product_sha):
    os.makedirs(work, exist_ok=False)
    new_lab = os.path.join(new_tools, "actinglab.exe")
    new_ledger = os.path.join(new_tools, "actingledger.exe")
    old_lab = os.path.join(old_tools, "actinglab.exe")
    old_ledger = os.path.join(old_tools, "actingledger.exe")
    for path in (new_lab, new_ledger, old_lab, old_ledger, os.path.join(new_runtime, "actingcommand-actingd.exe"),
                 os.path.join(old_runtime, "actingcommand-actingd.exe")):
        check("E0.present " + os.path.basename(os.path.dirname(path)) + "/" + os.path.basename(path), os.path.isfile(path), path)
    with open(os.path.join(repo, "ratchet", "actinglab_commands.json"), encoding="utf-8") as handle:
        ratchet = json.load(handle)["commands"]
    with open(os.path.join(repo, "contracts", "actinglab-dry-run.md"), encoding="utf-8") as handle:
        doc_text = handle.read()
    previews = {}
    contract = []
    for name, step in (("E1", lambda: e1(old_runtime, old_lab, old_ledger, work)),
                       ("E23", lambda: previews.update(e2_e3(new_runtime, new_lab, new_ledger, work) or {})),
                       ("E5", lambda: contract.extend(e5(new_lab, work, ratchet, doc_text, previews))),
                       ("E4", lambda: e4(old_lab, new_lab, new_runtime, work, contract)),
                       ("E6", lambda: e6(repo, product_sha))):
        try:
            step()
        except Exception as error:  # report and continue with the next item
            say(name, "exception", repr(error))
            FAILURES.append(name + ".exception")
    say("RESULT", "failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(run(*sys.argv[1:8]))
