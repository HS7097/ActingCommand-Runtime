# One-off (to be reverted), Workflow #336 L4 evidence: record stop generates the linear_steps
# package (frozen model v2 section 8 "L4" items 1-7, a15 items 8-10, a20 section 4.8, R24 "L4"
# items 1-8 with item 2 replaced by restart -> title -> home (R25 clarification 5961093808), one
# R25 refusal, the optional-step refusal until L4o, and the dry-run preview). Every printed line
# starts with "L4|". Usage:
#   evidence.py prepare <work> <umbrella standard package zip>
#   evidence.py run <work> <new tools> <new runtime> <old tools> <catalog dir>
import hashlib
import io
import json
import os
import re
import shutil
import subprocess
import sys
import time
import zipfile

from PIL import Image

SCHEMA_DIR = "actingcommand.package.content-directory.v1"
CONTENT_JSON = "actingcommand.package.content-json.v1"
MARK_SCHEMA = "actingcommand.lab-record-mark.v1"
APPLICATION_REFUSAL = "application_effect_requires_assigned_application"
FAILURES = []
ENV = dict(os.environ)
INSTANCE = "emu-a"

# The small fixture packages (64x36, the actingd configuration is limited to 1 MiB).
GAME = "fixture-game-a"
SERVER = "fixture-server-a"
ALIAS = "node.a"
INSTANCE_ID = "instance_00000000000000000000000000000336"
W, H = 64, 36
BG = (32, 32, 32)
COLORS = {"x": (200, 40, 40), "home": (200, 40, 200), "a2": (40, 200, 40), "m": (40, 40, 200), "c2": (120, 120, 40)}
ID_H = "fixture.prereq.h"


def say(*parts):
    print("L4|" + "|".join(str(part) for part in parts), flush=True)


def check(label, condition, detail=""):
    say("CHECK", label, "PASS" if condition else "FAIL", detail)
    if not condition:
        FAILURES.append(label)


def short(text, limit=900):
    text = text.strip().replace("\r", "").replace("\n", " ")
    return text if len(text) <= limit else text[:limit] + "..."


def digest(files):
    hasher = hashlib.sha256()
    hasher.update((SCHEMA_DIR + "\n").encode())
    for path in sorted(files, key=lambda item: item.encode("utf-8")):
        hasher.update(f"{hashlib.sha256(files[path]).hexdigest()}  {path}\n".encode("utf-8"))
    return hasher.hexdigest()


def file_sha(path):
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def tree_hashes(root):
    hashes = {}
    for folder, _dirs, names in os.walk(root):
        for name in names:
            path = os.path.join(folder, name)
            hashes[os.path.relpath(path, root)] = file_sha(path)
    return hashes


def save_png(image, path, optimize=False):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    image.save(path, format="PNG", optimize=optimize)


def pretty(value):
    return (json.dumps(value, indent=2) + "\n").encode("utf-8")


# ---------------------------------------------------------------------------------------------
# prepare: synthetic frames from the assets of two packs of the umbrella v0.9.0 standard package.

def pack_files(bundle, names, index, package_id):
    index_name = next(name for name in names if name.endswith("bundle.json"))
    pack = next(pack for pack in index["packs"] if pack["package_id"] == package_id)
    prefix = index_name[: -len("bundle.json")] + pack["path"] + "/"
    files = {name[len(prefix):]: bundle.read(name) for name in names if name.startswith(prefix) and not name.endswith("/")}
    check(f"prepare.source_digest.{package_id}", digest(files) == pack["digest"], digest(files))
    return files


def prepare(work, bundle_zip):
    os.makedirs(work, exist_ok=False)
    with zipfile.ZipFile(bundle_zip) as bundle:
        names = bundle.namelist()
        index = json.loads(bundle.read(next(name for name in names if name.endswith("bundle.json"))))
        battle = pack_files(bundle, names, index, "bluearchive.jp.battle_auto_enable")
        home = pack_files(bundle, names, index, "bluearchive.jp.notice_home")
    images = {}
    for name in ("auto_off.png", "auto_on.png", "battle_cost_label.png"):
        images[name] = Image.open(io.BytesIO(battle["resources/operations/battle_auto_enable/assets/" + name])).convert("RGB")
    for name in ("home_cafe_label.png", "home_work_label.png"):
        images[name] = Image.open(io.BytesIO(home["resources/operations/notice_home/assets/" + name])).convert("RGB")

    def texture():
        image = Image.new("RGB", (1280, 720), (32, 32, 32))
        for x in range(0, 1280, 8):
            for y in range(0, 640, 8):
                image.putpixel((x, y), (32 + (x // 8) % 16, 32 + (y // 8) % 16, 40))
        return image

    def battle_frame(state, band=False):
        image = texture()
        image.paste(images["auto_off.png" if state == "off" else "auto_on.png"], (1180, 664))
        image.paste(images["battle_cost_label.png"], (778, 649))
        image.putpixel((1173, 680), (224, 225, 227) if state == "off" else (255, 229, 26))
        if band:
            for x in range(200, 1080):
                for y in range(356, 364):
                    image.putpixel((x, y), (90, 200, 255))
        return image

    def popup():
        # A darkening pop-up over the battle HUD: the background at half brightness, a light
        # panel with a button (the battle cost label asset) on it.
        image = battle_frame("on").point(lambda value: value // 2)
        for x in range(300, 980):
            for y in range(160, 560):
                image.putpixel((x, y), (235, 235, 240))
        image.paste(images["battle_cost_label.png"], (600, 480))
        return image

    def home_frame():
        # The main screen HUD labels at their declared coordinates, and a fixed bright area.
        image = texture()
        image.paste(images["home_cafe_label.png"], (77, 680))
        image.paste(images["home_work_label.png"], (1165, 667))
        for x in range(560, 600):
            for y in range(20, 40):
                image.putpixel((x, y), (250, 250, 250))
        return image

    def desktop():
        image = Image.new("RGB", (1280, 720), (20, 60, 110))
        for x in range(0, 1280, 16):
            for y in range(0, 720, 16):
                image.putpixel((x, y), (40, 90, 150))
        image.paste(images["battle_cost_label.png"], (40, 40))
        return image

    frames = os.path.join(work, "frames")
    save_png(battle_frame("off"), os.path.join(frames, "off.png"))
    save_png(battle_frame("off"), os.path.join(frames, "off_copy.png"), optimize=True)
    save_png(battle_frame("on"), os.path.join(frames, "on.png"))
    save_png(battle_frame("on"), os.path.join(frames, "on_copy.png"), optimize=True)
    save_png(battle_frame("off", band=True), os.path.join(frames, "loading.png"))
    save_png(popup(), os.path.join(frames, "popup.png"))
    save_png(home_frame(), os.path.join(frames, "home.png"))
    save_png(desktop(), os.path.join(frames, "desktop.png"))
    with open(os.path.join(frames, "off.png"), "rb") as a, open(os.path.join(frames, "off_copy.png"), "rb") as b:
        check("prepare.off_copy_other_bytes", a.read() != b.read(), "")

    small_asset = images["auto_off.png"].resize((14, 5))
    small = os.path.join(work, "small")
    for name, color in COLORS.items():
        image = Image.new("RGB", (W, H), BG)
        for y in range(9, 12):
            for x in range(9, 12):
                image.putpixel((x, y), color)
        image.paste(small_asset, (40, 26))
        save_png(image, os.path.join(small, name + ".png"))
    sizes = {name: list(image.size) for name, image in images.items()}
    with open(os.path.join(work, "cases.json"), "w", encoding="utf-8") as handle:
        json.dump({"sizes": sizes}, handle, indent=2)
    say("prepare", "frames", sorted(os.listdir(frames)), "small", sorted(os.listdir(small)), "asset sizes", json.dumps(sizes))
    return 1 if FAILURES else 0


# ---------------------------------------------------------------------------------------------
# ActingLab helpers.

def run_exe(args, timeout=600, env=None, cwd=None):
    try:
        result = subprocess.run(args, capture_output=True, timeout=timeout, env=env or ENV, cwd=cwd)
    except subprocess.TimeoutExpired:
        return None, "", "timeout"
    return result.returncode, result.stdout.decode("utf-8", "replace"), result.stderr.decode("utf-8", "replace")


def lab(exe, args, label, limit=2500, env=None):
    code, out, err = run_exe([exe, "--json", *args], env=env)
    try:
        value = json.loads(out)
    except ValueError:
        value = None
    say(label, "exit", code, "envelope", short(out, limit) if out else short(err, 800))
    return code, value, (value or {}).get("error")


def data_of(value):
    return (value or {}).get("data") or {}


def recording_file(state):
    for folder, _dirs, names in os.walk(os.path.join(state, "record-artifacts")):
        if "recording.json" in names:
            return os.path.join(folder, "recording.json")
    return None


def load_recording(state):
    with open(recording_file(state), encoding="utf-8") as handle:
        return json.load(handle)


class Session:
    def __init__(self, exe, work, name, task, game="bluearchive", server="jp", locale="ja-JP"):
        self.exe = exe
        self.name = name
        self.state = os.path.join(work, "sessions", name)
        self.base = ["--instance", INSTANCE]
        code, value, _ = lab(exe, self.base + ["record", "start", "--task-id", task, "--game", game, "--server", server,
                                               "--locale", locale, "--state-dir", self.state], f"{name} record start")
        check(f"{name}.start", code == 0 and (data_of(value).get("lab_recording") or {}).get("status") == "active", f"exit {code}")

    def mark(self, label, args, exit_code=0, error_code=None):
        code, value, error = lab(self.exe, self.base + ["record", "mark", "--state-dir", self.state, *args], f"{self.name} {label}")
        got = (error or {}).get("code")
        ok = code == exit_code and (error_code is None or got == error_code)
        check(f"{self.name}.{label}", ok, f"exit {code} code {got}")
        return code, value, error

    def request(self, label, request):
        request = {"schema_version": MARK_SCHEMA, **request}
        return self.mark(label, ["--request-json", json.dumps(request)])

    def stop(self, label, args, limit=6000):
        return lab(self.exe, self.base + ["record", "stop", "--state-dir", self.state, *args], f"{self.name} {label}", limit=limit)

    def status(self):
        code, value, _ = lab(self.exe, self.base + ["record", "status", "--state-dir", self.state], f"{self.name} status", limit=600)
        data = data_of(value)
        return (data.get("record") or {}).get("status"), (data.get("lab") or {}).get("status")

    def sha(self):
        return file_sha(recording_file(self.state))


def lab_of(value):
    return data_of(value).get("lab") or {}


def warning_codes(lab_data):
    return [warning.get("code") for warning in lab_data.get("warnings") or []]


def show(label, lab_data, keys):
    for key in keys:
        say(label, key, short(json.dumps(lab_data.get(key), ensure_ascii=False), 4000))


def package_digest(exe, path, label):
    code, value, _ = lab(exe, ["package", "digest", "--package", path], label, limit=800)
    return code, (data_of(value).get("reference") or {}).get("sha256")


def read_zip(path):
    with zipfile.ZipFile(path) as archive:
        return {info.filename: archive.read(info.filename) for info in archive.infolist() if not info.filename.endswith("/")}, archive.infolist()


def read_container(path):
    if path.endswith(".zip"):
        return read_zip(path)[0]
    with open(path, encoding="utf-8") as handle:
        document = json.load(handle)
    return {key: value.encode("utf-8") for key, value in document["files"].items()}


def task_of(files):
    name = next(path for path in files if path.endswith("/task.json"))
    return json.loads(files[name])


def tpl(name, region_id, sizes, x, y):
    w, h = sizes[name]
    return f"{region_id}={x},{y},{w},{h}"


# ---------------------------------------------------------------------------------------------
# Fixture actingd runs (scheduled dispatch; the fixture refuses direct task-run).

def h_package(root):
    """The hand-written page-graph package H: from the screen x back to home (a15 L2b H)."""
    probes = [{"id": f"state/{state}", "region": {"mode": "rect", "rect": {"x": 10, "y": 10, "width": 1, "height": 1}},
               "expected": list(COLORS[state])} for state in ("home", "x")]
    task = {
        "schema_version": "0.9", "task_id": "pre_h", "game": GAME, "server_scope": [SERVER], "locale": "en-US",
        "goal": "Return to home", "coordinate_space": {"width": W, "height": H},
        "defaults": {"template_threshold": 0.95, "color_max_distance": 20, "match_metric": "ccoeff_normed"},
        "timeout_ms": 30000, "max_steps": 1, "entry_page": "x", "target_page": "home",
        "color_probes": probes,
        "page_rules": {"x": {"required": ["state/x"]}, "home": {"required": ["state/home"]}},
        "scheduling_outcome": {"mappings": [{"outcome_key": "pre_h_done", "effect": "no_designated_effect", "terminal_pages": ["home"]}]},
        "operations": [{"id": "step_01_click", "purpose": "Return home", "from": "x", "to": "home",
                        "expect_after": {"page_id": "home", "timeout_ms": 500, "interval_ms": 100}, "post_delay_ms": 50,
                        "click": {"kind": "rect", "x": 8, "y": 8, "width": 5, "height": 5},
                        "guard": {"page_id": "x", "target_id": "state/x", "expected_rect": {"x": 10, "y": 10, "width": 1, "height": 1},
                                  "color_probe": "state/x"}}],
    }
    control = {"schema_version": "Lab-1y.control.v2", "package_id": ID_H, "execution_mode": "navigable_route", "game": GAME,
               "server": SERVER, "resolution": {"width": W, "height": H}, "entry_task_id": "pre_h", "timeout_ms": 30000,
               "step_timeout_ms": 500, "capture_interval_ms": 100, "max_steps": 1}
    files = {"control.json": pretty(control),
             "resources/operations/resources.json": pretty({"schema_version": "1.0", "resources": [], "resource_count": 0}),
             "resources/operations/pre_h/task.json": pretty(task)}
    for path, data in files.items():
        target = os.path.join(root, *path.split("/"))
        os.makedirs(os.path.dirname(target), exist_ok=True)
        with open(target, "wb") as handle:
            handle.write(data)
    return digest(files)


def catalog_documents(catalog_dir, every_ms):
    def load(name):
        with open(os.path.join(catalog_dir, name + ".json"), encoding="utf-8") as handle:
            return json.load(handle)

    scope = {"kind": "instance", "instance_id": ALIAS}
    tasks, pools, activity, timeline = (load(name) for name in ("tasks", "pools", "activity", "timeline"))
    task = tasks["tasks"][0]
    task["scope"] = scope
    task["trigger"]["predicates"][0]["schedule"]["every_ms"] = every_ms
    task["trigger"]["predicates"][1]["scope"] = scope
    task["feedback_stop"] = {"kind": "clock", "schedule": {"kind": "at", "clock_source": {
        "kind": "server", "timezone_id": "etc/utc", "utc_offset_minutes": 0, "dst_offset_minutes": 0,
        "maintenance_drift_ms": 0}, "at_ms": 4102444800000}}
    task["instance_overrides"] = []
    pools["pools"][0]["scope"] = scope
    profile = activity["profiles"][0]
    profile["scope"] = scope
    profile["windows"][0]["start_minute_of_day"] = 0
    profile["windows"][0]["end_minute_of_day"] = 0
    profile["minimum_interval_ms"] = 1
    profile["maximum_interval_ms"] = 1
    return {"tasks": tasks, "pools": pools, "activity": activity, "timeline": timeline}


def write_config(config_dir, state_root, package_path, package_digest_value, frames, catalog_dir, prerequisites):
    os.makedirs(os.path.join(config_dir, "policy"), exist_ok=False)
    for name, document in catalog_documents(catalog_dir, 60000).items():
        with open(os.path.join(config_dir, "policy", name + ".json"), "w", encoding="utf-8") as handle:
            json.dump(document, handle, indent=2)
    now = int(time.time() * 1000)
    config = {
        "schema_version": "actingcommand.actingd.config.v1",
        "state_root": state_root,
        "bind_host": "127.0.0.1",
        "bind_port": 0,
        "secret_fingerprint_salt": "oneoff-336-l4-fixture-salt-value",
        "policy": {
            "facts": {
                "ledger_position": 0, "fact_snapshot_id": "snapshot:oneoff-336-l4",
                "facts": [], "outcomes": [], "tasks": [],
                "instances": [{"instance_id": ALIAS, "server_id": SERVER, "game_id": GAME,
                               "host_id": "fixture-host-a", "available": True,
                               "capability_operation_ids": ["operation.observe"], "preferred_task_ids": []}],
            },
            "resources": {
                "pools": [{"pool_id": "fixture-pool-a", "value": 10, "observed_at_unix_ms": now}],
                "hosts": [{"host_id": "fixture-host-a", "cpu_available_milli": 1000, "gpu_available_milli": 1000,
                           "io_available_milli": 1000, "host_responsiveness_basis_points": 10000,
                           "third_party_pressure_basis_points": 0, "heavy_dispatch_limit": 1,
                           "active_heavy_dispatches": 0}],
            },
            "catalog": {"tasks": "policy/tasks.json", "pools": "policy/pools.json",
                        "activity": "policy/activity.json", "timeline": "policy/timeline.json"},
            "catalog_approval_ids": ["approval:fixture-a"],
            "procedure_manifest": [{
                "procedure_ref": "procedure.observe",
                "package_digest": package_digest_value,
                "operation_id": "operation.observe",
                "yield_points": ["after_observation"],
                "scheduled_execution": {"mode": "fixture_simulation", "package_path": package_path},
            }],
        },
        "instances": [{
            "alias": ALIAS,
            "instance_id": INSTANCE_ID,
            "fixture_backend": {
                "frames": [{"width": W, "height": H, "rgb": list(Image.open(frame).convert("RGB").tobytes())} for frame in frames],
                "max_inputs": 4,
            },
        }],
    }
    if prerequisites is not None:
        config["prerequisite_packages"] = prerequisites
    path = os.path.join(config_dir, "actingd.json")
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(config, handle, separators=(",", ":"))
    say("fixture config", path, "bytes", os.path.getsize(path))
    return path


def wait_for_minute_window():
    second = time.time() % 60
    if second < 2:
        time.sleep(2 - second)
    elif second > 20:
        time.sleep(62 - second)


def scheduled_run(work, label, runtime_dir, package_path, package_digest_value, frames, catalog_dir, prerequisites, settle_s=20):
    run_dir = os.path.join(work, "runs", re.sub(r"[^A-Za-z0-9_.-]", "_", label))
    state_root = os.path.join(run_dir, "state")
    os.makedirs(state_root)
    config = write_config(os.path.join(run_dir, "config"), state_root, package_path, package_digest_value, frames,
                          catalog_dir, prerequisites)
    actingd = os.path.join(runtime_dir, "actingcommand-actingd.exe")
    actingctl = os.path.join(runtime_dir, "actingctl.exe")
    wait_for_minute_window()
    with open(os.path.join(run_dir, "actingd.out"), "wb") as out, open(os.path.join(run_dir, "actingd.err"), "wb") as err:
        process = subprocess.Popen([actingd, "--config", config], stdout=out, stderr=err, cwd=os.path.dirname(config))
        started = time.time()
        ready = False
        while time.time() - started < 30 and process.poll() is None:
            code, _, _ = run_exe([actingctl, "status", "--state-root", state_root], timeout=30)
            if code == 0:
                ready = True
                break
            time.sleep(0.5)
        say(label, "daemon ready", ready, "after_s", round(time.time() - started, 1))
        if ready:
            time.sleep(settle_s)
        code, _, shutdown_err = run_exe([actingctl, "request-shutdown", "--state-root", state_root, "--wait", "60"], timeout=120)
        say(label, "request-shutdown exit", code, short(shutdown_err, 300))
        try:
            exit_code = process.wait(timeout=120)
        except subprocess.TimeoutExpired:
            process.kill()
            exit_code = "killed"
    say(label, "actingd exit", exit_code)
    if exit_code != 0:
        for stream in ("out", "err"):
            with open(os.path.join(run_dir, f"actingd.{stream}"), "rb") as handle:
                say(label, "actingd " + stream, short(handle.read().decode("utf-8", "replace"), 1500))
    return state_root


def ledger_events(ledger, root):
    after, events = 0, []
    while True:
        code, out, err = run_exe([ledger, "--state-root", root, "events", "--after", str(after), "--limit", "500"])
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


def facts_of(events):
    facts = []
    for event in events:
        payload = event.get("payload") or {}
        inner = payload.get("payload") if isinstance(payload, dict) else None
        data = inner.get("data") if isinstance(inner, dict) else None
        fact = data.get("fact") if isinstance(data, dict) else None
        if isinstance(fact, dict) and "kind" in fact:
            facts.append((event["sequence"], event["event_type"], fact))
    return facts


def first_run(facts):
    for index, (_, _, fact) in enumerate(facts):
        if fact["kind"] == "terminal_committed":
            return facts[:index + 1]
    return facts


def ref_sha(value):
    if isinstance(value, dict):
        return value.get("sha256")
    if isinstance(value, str):
        return value.split(":", 1)[-1]
    return value


def rows_of(facts):
    rows = []
    for _, _, fact in facts:
        kind = fact["kind"]
        if kind == "entry_recognition":
            rows.append([kind, fact.get("phase"), fact.get("required_page"), fact.get("matched")])
        elif kind == "entry_recovery_decision":
            rows.append([kind, fact.get("required")])
        elif kind in ("entry_recovery_package_admitted",):
            rows.append([kind, ref_sha(fact.get("package_sha256"))])
        elif kind == "entry_recovery_completed":
            rows.append([kind, ref_sha(fact.get("package_sha256")), fact.get("final_page"), fact.get("executed_steps")])
        elif kind == "entry_recovery_failed":
            rows.append([kind, ref_sha(fact.get("package_sha256")), fact.get("failure_code")])
        elif kind == "entry_target_disposition":
            rows.append([kind, fact.get("disposition"), fact.get("failure_code")])
        elif kind == "package_admitted":
            rows.append([kind, fact.get("package_label"), ref_sha(fact.get("package_sha256"))])
        elif kind in ("step_started", "step_finished"):
            rows.append([kind, fact.get("step_index"), fact.get("operation_label"), fact.get("from_page") or fact.get("page_label")])
        elif kind == "capture_completed":
            rows.append([kind])
        elif kind == "terminal_committed":
            rows.append([kind, fact.get("outcome"), fact.get("executed_steps"), fact.get("failure_code")])
    return rows


def fixture_case(work, label, runtime, new_ledger, package_path, package_ref, frames, catalog_dir, prerequisites):
    root = scheduled_run(work, label, runtime, package_path, package_ref, frames, catalog_dir, prerequisites)
    events, failure = ledger_events(new_ledger, root)
    if events is None:
        say(label, "events failed", failure)
        events = []
    facts = first_run(facts_of(events))
    rows = rows_of(facts)
    for row in rows:
        say(label, "row", json.dumps(row, ensure_ascii=False))
    types = [event.get("event_type") for event in events]
    say(label, "event types", json.dumps(types))
    for event in events:
        if event.get("event_type") in ("runtime.failed", "command.rejected", "task.failed",
                                       "runtime.resource_declaration_rejected"):
            say(label, event["event_type"], event["sequence"], short(json.dumps(event.get("payload"), ensure_ascii=False), 2500))
    return root, rows, types, json.dumps(events, ensure_ascii=False)


def ledger_reads(label, ledger, root):
    copy = root + "-read-copy"
    shutil.copytree(root, copy)
    before = tree_hashes(copy)
    results = {}
    for name, args in (("open", ["open"]), ("events", ["events"]), ("export --task-evidence", ["export", "--task-evidence"])):
        code, out, err = run_exe([ledger, "--state-root", copy, *args])
        corrupt = "corrupt_ledger_record" in out or "corrupt_ledger_record" in err
        say(label, name, "exit", code, "stdout bytes", len(out), "corrupt_ledger_record", corrupt, "stderr", short(err, 300))
        results[name] = (code, corrupt)
    after = tree_hashes(copy)
    check(f"{label}.reads", all(code == 0 and not corrupt for code, corrupt in results.values()), json.dumps(results))
    check(f"{label}.copy_unchanged", before == after, f"{len(before)} files")


# ---------------------------------------------------------------------------------------------
# The evidence.

def run(work, new_tools, new_runtime, old_tools, catalog_dir):
    exe = os.path.join(new_tools, "actinglab.exe")
    new_ledger = os.path.join(new_tools, "actingledger.exe")
    old_ledger = os.path.join(old_tools, "actingledger.exe")
    with open(os.path.join(work, "cases.json"), encoding="utf-8") as handle:
        sizes = json.load(handle)["sizes"]
    frames = os.path.join(work, "frames")
    small = os.path.join(work, "small")
    f = lambda name: os.path.join(frames, name + ".png")
    sf = lambda name: os.path.join(small, name + ".png")
    out = os.path.join(work, "out")
    os.makedirs(os.path.join(out, "install", "packages"))
    os.makedirs(os.path.join(out, "fixture", "packages"))

    # C0. Without Lab steps record stop behaves as before; --dry-run writes nothing; unknown flags.
    say("C0", "old behaviour without Lab steps, dry run, unknown flag")
    s = Session(exe, work, "c0", "plain")
    code, value, _ = s.stop("stop --dry-run (no Lab steps)", ["--dry-run"])
    check("C0.dry_run_without_steps", code == 0 and data_of(value).get("status") == "validated"
          and data_of(value).get("lab") is None and s.status()[0] == "active", "")
    code, value, error = s.stop("stop --lab_dir (unknown flag)", ["--lab_dir", out])
    check("C0.unknown_flag_refused", code == 2 and (error or {}).get("code") == "validation_failed"
          and "--lab_dir" in (error or {}).get("message", ""), "")
    code, value, _ = s.stop("stop (no Lab steps)", [])
    check("C0.old_stop", code == 0 and data_of(value).get("status") == "stopped" and data_of(value).get("lab") is None
          and s.status() == ("stopped", "stopped"), json.dumps(s.status()))

    # E1. battle_auto, two steps with a 1-3 s window -> <D>.zip in packages\bluearchive\.
    say("E1", "battle_auto two steps with a window -> ZIP in packages\\bluearchive\\ (created)")
    s = Session(exe, work, "e1", "battle_auto")
    s.mark("mark auto_off", ["--frame", f("off"), "--page", "auto_off", "--template", tpl("auto_off.png", "ui/auto_off", sizes, 1180, 664),
                             "--template", tpl("battle_cost_label.png", "ui/battle_cost", sizes, 778, 649),
                             "--color", "state/auto_off=1173,680,1,1", "--click-from", "ui/auto_off"])
    s.mark("window 1000-3000", ["--step", "1", "--transition", "window", "--min-ms", "1000", "--max-ms", "3000"])
    s.mark("mark auto_on", ["--frame", f("on"), "--page", "auto_on", "--template", tpl("auto_on.png", "ui/auto_on", sizes, 1180, 664),
                            "--color", "state/auto_on=1173,680,1,1"])
    lab_dir = os.path.join(out, "install", "packages", "bluearchive")
    before = s.sha()
    code, value, _ = s.stop("stop --dry-run --lab-dir", ["--dry-run", "--lab-dir", lab_dir])
    dry = lab_of(value)
    show("E1 dry-run", dry, ["status", "would_write", "lab_dir_status", "lab_dir_to_create", "warnings", "timeouts"])
    check("E1.dry_run_preview", code == 0 and data_of(value).get("status") == "validated" and dry.get("status") == "validated"
          and dry.get("lab_dir_to_create") is True and len(dry.get("would_write") or []) == 2
          and not os.path.exists(lab_dir) and s.sha() == before and s.status() == ("active", "active"), "")
    code, value, _ = s.stop("stop --lab-dir", ["--lab-dir", lab_dir])
    e1 = lab_of(value)
    show("E1", e1, ["status", "container", "digest", "package_id", "path", "lab_dir_path", "lab_dir_status", "lab_dir_created",
                    "entries", "pages", "transitions", "timeouts", "warnings", "arrival_by_time_window", "first_decision",
                    "cross_check", "entry_overlay", "package_ref", "binding_example", "task_run_example",
                    "prerequisite_entry_example", "catalog_on_failure_example", "binding_requires"])
    d1 = e1.get("digest")
    zip_path = os.path.join(lab_dir, f"{d1}.zip")
    check("E1.generated_zip", code == 0 and e1.get("status") == "generated" and e1.get("container") == "zip"
          and e1.get("lab_dir_created") is True and e1.get("lab_dir_status") == "written" and os.path.isfile(zip_path)
          and e1.get("dry_run") is False and dry.get("digest") == d1, json.dumps({"digest": d1}))
    check("E1.both_states_stopped", s.status() == ("stopped", "stopped"), json.dumps(s.status()))
    code, measured = package_digest(exe, zip_path, "E1 package digest --package <D>.zip")
    check("E1.package_digest_equals_D", code == 0 and measured == d1, f"{measured}")
    files, infos = read_zip(zip_path)
    say("E1", "zip entries", json.dumps([(info.filename, info.date_time, info.compress_type, info.flag_bits) for info in infos]))
    check("E1.zip_layout", [info.filename for info in infos] == sorted(files, key=lambda p: p.encode())
          and all(info.date_time == (1980, 1, 1, 0, 0, 0) and info.compress_type == zipfile.ZIP_DEFLATED for info in infos)
          and digest(files) == d1, "")
    control = json.loads(files["control.json"])
    task = task_of(files)
    say("E1", "control.json", json.dumps(control))
    say("E1", "task.json operations", json.dumps(task["operations"]))
    say("E1", "task.json page_rules", json.dumps(task["page_rules"]))
    check("E1.control_linear_steps", control.get("execution_mode") == "linear_steps" and control.get("timeout_ms") == 43000
          and control.get("step_timeout_ms") == 15000 and control.get("max_steps") == 1, "")
    check("E1.task_window_transition", task["operations"][0].get("transition") == {"kind": "window", "min_ms": 1000, "max_ms": 3000}
          and task["operations"][0]["guard"]["target_id"] == "ui/auto_off" and task["entry_page"] == "step_01_auto_off"
          and task["target_page"] == "step_02_auto_on", "")
    first = e1.get("first_decision") or {}
    check("E1.first_decision_would_click", first.get("status") == "would_click" and first.get("operation_label") == "step_01_click", json.dumps(first))
    check("E1.binding_and_a20_items", (e1.get("binding_example") or {}).get("package_digest") == {"schema_version": SCHEMA_DIR, "sha256": d1}
          and e1.get("catalog_on_failure_example") == {"action": "pause", "retry_limit": 1, "retry_backoff_ms": 60000, "escalation_threshold": 2}
          and any("on_failure" in item for item in e1.get("binding_requires") or [])
          and any("return_home_packages" in item for item in e1.get("binding_requires") or []), "")

    # E5. Writing: present (same content, other bytes), already_generated, a new directory.
    say("E5", "writing: already_generated with present, written; conflict; missing parent")
    code, value, _ = s.stop("stop --lab-dir again (stopped)", ["--lab-dir", lab_dir])
    again = lab_of(value)
    check("E5.already_generated_present", code == 0 and again.get("status") == "already_generated"
          and again.get("lab_dir_status") == "present" and file_sha(zip_path) == e1.get("sha256"), json.dumps(again.get("lab_dir_status")))
    other = os.path.join(out, "install2", "packages", "bluearchive")
    os.makedirs(other)
    rezipped = os.path.join(other, f"{d1}.zip")
    with zipfile.ZipFile(rezipped, "w", zipfile.ZIP_STORED) as archive:
        for path in sorted(files, reverse=True):
            archive.writestr(path, files[path])
    rezipped_sha = file_sha(rezipped)
    code, value, _ = s.stop("stop --lab-dir (same content, other ZIP bytes)", ["--lab-dir", other])
    present = lab_of(value)
    check("E5.present_by_content", code == 0 and present.get("lab_dir_status") == "present" and file_sha(rezipped) == rezipped_sha
          and rezipped_sha != e1.get("sha256"), "")
    third = os.path.join(out, "install", "packages", "copy")
    code, value, _ = s.stop("stop --lab-dir (new last level)", ["--lab-dir", third])
    copied = lab_of(value)
    check("E5.already_generated_written", code == 0 and copied.get("status") == "already_generated"
          and copied.get("lab_dir_status") == "written" and copied.get("lab_dir_created") is True
          and file_sha(os.path.join(third, f"{d1}.zip")) == e1.get("sha256"), "")

    s = Session(exe, work, "e5", "conflict_case")
    s.mark("mark auto_off", ["--frame", f("off"), "--color", "state/a=1173,680,1,1", "--click", "600,300,80,40"])
    s.mark("mark auto_on", ["--frame", f("on"), "--color", "state/b=1173,680,1,1"])
    code, value, _ = s.stop("stop --dry-run", ["--dry-run"])
    d5, ext5 = lab_of(value).get("digest"), lab_of(value).get("container")
    conflict_dir = os.path.join(out, "install3", "packages", "bluearchive")
    os.makedirs(conflict_dir)
    conflict_path = os.path.join(conflict_dir, f"{d5}.{ext5}")
    with open(conflict_path, "w", encoding="utf-8") as handle:
        json.dump({"schema_version": CONTENT_JSON, "files": {"control.json": "{}\n"}}, handle)
    conflict_sha = file_sha(conflict_path)
    code, value, error = s.stop("stop --lab-dir (name conflict)", ["--lab-dir", conflict_dir])
    out_dir = os.path.join(os.path.dirname(recording_file(s.state)), "out")
    check("E5.name_conflict", code == 3 and (error or {}).get("code") == "record_artifact_name_conflict"
          and file_sha(conflict_path) == conflict_sha and not os.path.exists(out_dir) and s.status() == ("active", "active"), "")
    code, value, error = s.stop("stop --lab-dir (missing parent)", ["--lab-dir", os.path.join(out, "nowhere", "packages", "bluearchive")])
    check("E5.lab_dir_invalid", code == 2 and (error or {}).get("code") == "record_lab_dir_invalid", "")
    code, value, _ = s.stop("stop (no --lab-dir)", [])
    check("E5.stop_without_lab_dir", code == 0 and lab_of(value).get("status") == "generated" and lab_of(value).get("lab_dir_path") is None
          and os.path.isfile(lab_of(value).get("path") or ""), "")

    # E1b. Step 2 named transition and a page transition after step 1: both pages exist.
    say("E1b", "page transition after step 1 and a step page named transition")
    s = Session(exe, work, "e1b", "battle_auto")
    s.mark("mark auto_off", ["--frame", f("off"), "--template", tpl("auto_off.png", "ui/auto_off", sizes, 1180, 664),
                             "--color", "state/auto_off=1173,680,1,1", "--click-from", "ui/auto_off"])
    s.mark("mark loading", ["--frame", f("loading"), "--color", "load/bar=600,358,8,4"])
    s.mark("to-transition 2", ["--to-transition", "2"])
    s.mark("mark auto_on --page transition", ["--frame", f("on"), "--page", "transition",
                                              "--template", tpl("auto_on.png", "ui/auto_on", sizes, 1180, 664),
                                              "--color", "state/auto_on=1173,680,1,1"])
    code, value, _ = s.stop("stop", ["--lab-dir", os.path.join(out, "install", "packages", "bluearchive")])
    e1b = lab_of(value)
    show("E1b", e1b, ["pages", "transitions", "cross_check", "warnings"])
    files = read_container(e1b.get("path") or "")
    task = task_of(files)
    say("E1b", "operation", json.dumps(task["operations"][0]))
    check("E1b.pages_coexist", code == 0 and e1b.get("pages") == ["step_01", "transition_01", "step_02_transition"]
          and task["operations"][0].get("transition") == {"kind": "page", "page_id": "transition_01"}
          and set(task["page_rules"]) == {"step_01", "transition_01", "step_02_transition"}, "")

    # E2. Only color marks (with a color digest) -> <D>.json; unpacked as a directory, same digest.
    say("E2", "color and color digest only -> JSON; directory form; session file already stopped")
    s = Session(exe, work, "e2", "color_steps")
    s.request("mark color and digest", {"frame": f("off"), "page": "dim", "add": [
        {"id": "state/dim", "family": "color", "region": {"x": 1173, "y": 680, "width": 1, "height": 1}},
        {"id": "hud/strip", "family": "color_digest", "region": {"x": 0, "y": 640, "width": 1280, "height": 80},
         "columns": 16, "rows": 2, "max_mean_milli": 1500}],
        "click": {"region": {"x": 600, "y": 300, "width": 80, "height": 40}}})
    s.mark("mark lit", ["--frame", f("on"), "--page", "lit", "--color", "state/lit=1173,680,1,1"])
    # The session file stopped by an older ActingLab while the Lab recording stays active.
    record_path = os.path.join(s.state, f"record-{INSTANCE}.json")
    with open(record_path, encoding="utf-8") as handle:
        record = json.load(handle)
    record["status"] = "stopped"
    with open(record_path, "w", encoding="utf-8") as handle:
        json.dump(record, handle)
    json_dir = os.path.join(out, "install", "packages", "bluearchive")
    code, value, _ = s.stop("stop --lab-dir", ["--lab-dir", json_dir])
    e2 = lab_of(value)
    show("E2", e2, ["status", "container", "digest", "entries", "session_already_stopped", "first_decision", "warnings"])
    d2 = e2.get("digest")
    json_path = os.path.join(json_dir, f"{d2}.json")
    check("E2.generated_json", code == 0 and e2.get("container") == "json" and os.path.isfile(json_path)
          and e2.get("session_already_stopped") is True, "")
    code, measured = package_digest(exe, json_path, "E2 package digest --package <D>.json")
    check("E2.package_digest_equals_D", code == 0 and measured == d2, f"{measured}")
    files = read_container(json_path)
    task = task_of(files)
    say("E2", "color_probes", json.dumps(task.get("color_probes")))
    check("E2.digest_probe", any("digest" in probe for probe in task.get("color_probes") or []), "")
    expanded = os.path.join(out, "e2-expanded")
    for path, data in files.items():
        target = os.path.join(expanded, *path.split("/"))
        os.makedirs(os.path.dirname(target), exist_ok=True)
        with open(target, "wb") as handle:
            handle.write(data)
    code, measured = package_digest(exe, expanded, "E2 package digest --package <expanded directory>")
    check("E2.directory_same_digest", code == 0 and measured == d2, f"{measured}")

    # E4. Arrival unconfirmed on a darkening pop-up; a bright-area color clears it; a window.
    say("E4", "arrival unconfirmed: darkening pop-up, then a --color on a bright area; same screen with a window")
    s = Session(exe, work, "e4", "popup_close")
    s.mark("mark pop-up", ["--frame", f("popup"), "--page", "notice", "--template", tpl("battle_cost_label.png", "notice/close", sizes, 600, 480),
                           "--color", "notice/panel=320,180,8,8", "--click-from", "notice/close"])
    s.mark("mark home (template only)", ["--frame", f("on"), "--page", "home", "--template", tpl("auto_on.png", "ui/auto_on", sizes, 1180, 664)])
    before = s.sha()
    code, value, _ = s.stop("stop --dry-run", ["--dry-run"])
    e4 = lab_of(value)
    show("E4 first dry-run", e4, ["warnings", "cross_check"])
    arrival = [w for w in e4.get("warnings") or [] if w.get("code") == "arrival_unconfirmed"]
    check("E4.arrival_unconfirmed", code == 0 and len(arrival) == 1 and arrival[0].get("step") == 1
          and arrival[0].get("gate") == "step_02_home" and "darken" in arrival[0].get("message", "")
          and "--color" in arrival[0].get("message", "") and s.sha() == before and s.status() == ("active", "active"), "")
    s.mark("add --color on the bright area of step 2", ["--step", "2", "--color", "state/auto_on=1173,680,1,1"])
    code, value, _ = s.stop("stop --dry-run after the color", ["--dry-run"])
    check("E4.warning_gone", code == 0 and "arrival_unconfirmed" not in warning_codes(lab_of(value)), json.dumps(warning_codes(lab_of(value))))
    code, value, _ = s.stop("stop", [])
    check("E4.final_stop", code == 0 and lab_of(value).get("status") == "generated", "")

    s = Session(exe, work, "e4w", "same_screen")
    s.mark("mark screen a", ["--frame", f("off"), "--page", "a", "--template", tpl("auto_off.png", "ui/auto_off", sizes, 1180, 664),
                             "--click-from", "ui/auto_off"])
    s.mark("window", ["--step", "1", "--transition", "window", "--min-ms", "1000", "--max-ms", "3000"])
    s.mark("same screen b (other PNG bytes)", ["--frame", f("off_copy"), "--page", "b", "--reuse", "ui/auto_off"])
    code, value, _ = s.stop("stop --dry-run", ["--dry-run"])
    e4w = lab_of(value)
    show("E4w", e4w, ["warnings", "arrival_by_time_window", "cross_check"])
    gates = (e4w.get("cross_check") or {}).get("gates") or []
    check("E4w.arrival_by_time_window", code == 0 and e4w.get("arrival_by_time_window") == [1]
          and "arrival_unconfirmed" not in warning_codes(e4w) and gates and gates[0].get("result") == "passes_on_previous", "")

    # E6. OCR: step 1 with OCR (not evaluated), an OCR-only step (trusted coordinate), a click
    # source that is OCR (the guard falls to the next target).
    say("E6", "OCR: first decision not evaluated, trusted coordinate, OCR click source")
    s = Session(exe, work, "e6", "ocr_steps")
    ocr = {"languages": ["ja"], "timeout_ms": 1000, "match_mode": "contains", "expected": ["AUTO"], "case_sensitive": False,
           "minimum_confidence": 0.8, "model_ref": "model", "model_sha256": "0" * 64}
    w, h = sizes["auto_off.png"]
    s.request("mark step 1 (OCR + color, click from OCR)", {"frame": f("off"), "page": "start", "add": [
        {"id": "text/start", "family": "ocr", "region": {"x": 1180, "y": 664, "width": w, "height": h}, **ocr},
        {"id": "state/start", "family": "color", "region": {"x": 1173, "y": 680, "width": 1, "height": 1}}],
        "click": {"from": "text/start"}})
    s.request("mark step 2 (OCR only)", {"frame": f("loading"), "page": "ocr_only", "add": [
        {"id": "text/only", "family": "ocr", "region": {"x": 200, "y": 300, "width": 200, "height": 40}, **ocr}],
        "click": {"region": {"x": 600, "y": 300, "width": 80, "height": 40}}})
    s.mark("mark step 3", ["--frame", f("on"), "--page", "done", "--color", "state/done=1173,680,1,1"])
    code, value, _ = s.stop("stop", [])
    e6 = lab_of(value)
    show("E6", e6, ["first_decision", "warnings", "marks_not_evaluated", "cross_check"])
    task = task_of(read_container(e6.get("path") or ""))
    say("E6", "operations", json.dumps(task["operations"]))
    ops = task["operations"]
    check("E6.admitted_ocr_first_step", code == 0 and e6.get("status") == "generated"
          and (e6.get("first_decision") or {}).get("status") == "not_evaluated"
          and (e6.get("first_decision") or {}).get("reason") == "lab_ocr_provider_unverified"
          and "first_decision_not_evaluated" in warning_codes(e6)
          and "text/start" in (e6.get("marks_not_evaluated") or [])
          and (e6.get("cross_check") or {}).get("status") == "partially_evaluated", "")
    check("E6.ocr_click_source_guard", ops[0].get("guard", {}).get("target_id") == "state/start"
          and ops[0].get("guard", {}).get("color_probe") == "state/start", json.dumps(ops[0].get("guard")))
    check("E6.ocr_only_trusted", ops[1].get("unguarded_trusted_coordinate") is True and "guard" not in ops[1], "")

    # E7. Timeouts.
    say("E7", "timeouts")
    s = Session(exe, work, "e7", "timeouts")
    s.mark("mark a", ["--frame", f("off"), "--color", "state/a=1173,680,1,1", "--click", "600,300,80,40"])
    s.mark("mark b", ["--frame", f("on"), "--color", "state/b=1173,680,1,1"])
    code, value, error = s.stop("stop --timeout-ms 1800001", ["--timeout-ms", "1800001"])
    check("E7.timeout_out_of_range", code == 2 and (error or {}).get("code") == "validation_failed" and s.status() == ("active", "active"), "")
    code, value, _ = s.stop("stop --dry-run --arrival-timeout-ms 90000", ["--dry-run", "--arrival-timeout-ms", "90000"])
    e7 = lab_of(value)
    check("E7.step_timeout_clamped", code == 0 and (e7.get("timeouts") or {}).get("step_timeout_ms") == 60000
          and "step_timeout_clamped" in warning_codes(e7), json.dumps(e7.get("timeouts")))

    # E10. Entry overlay on the main screen HUD templates; a bright-area color clears it.
    say("E10", "entry overlay: two HUD templates, then a --color on the bright area")
    s = Session(exe, work, "e10", "home_entry")
    s.mark("mark home (two HUD templates)", ["--frame", f("home"), "--page", "home",
                                             "--template", tpl("home_cafe_label.png", "hud/cafe", sizes, 77, 680),
                                             "--template", tpl("home_work_label.png", "hud/work", sizes, 1165, 667),
                                             "--click", "600,300,80,40"])
    s.mark("mark next", ["--frame", f("off"), "--page", "next", "--template", tpl("auto_off.png", "ui/auto_off", sizes, 1180, 664)])
    code, value, _ = s.stop("stop --dry-run", ["--dry-run"])
    e10 = lab_of(value)
    show("E10", e10, ["entry_overlay", "warnings"])
    check("E10.entry_overlay_insensitive", code == 0 and "entry_overlay_insensitive" in warning_codes(e10)
          and (e10.get("entry_overlay") or {}).get("status") == "insensitive", "")
    s.mark("add --color on the bright area of step 1", ["--step", "1", "--color", "hud/bright=570,25,4,4"])
    code, value, _ = s.stop("stop --dry-run after the color", ["--dry-run"])
    check("E10.warning_gone", code == 0 and "entry_overlay_insensitive" not in warning_codes(lab_of(value))
          and (lab_of(value).get("entry_overlay") or {}).get("status") == "sensitive", json.dumps(lab_of(value).get("entry_overlay")))

    # R24 L4 items 1, 6, 7, 8: restart -> title -> home, declared offline.
    say("R24-1", "restart -> title -> home (declared offline) -> ZIP")
    s = Session(exe, work, "r1", "cold_start")
    s.mark("declare restart (entry step)", ["--application", "restart"])
    s.mark("mark title + click", ["--frame", f("off"), "--page", "title", "--template", tpl("auto_off.png", "ui/title", sizes, 1180, 664),
                                  "--color", "state/title=1173,680,1,1", "--click-from", "ui/title"])
    s.mark("mark home", ["--frame", f("on"), "--page", "home", "--template", tpl("auto_on.png", "ui/home", sizes, 1180, 664),
                         "--color", "state/home=1173,680,1,1"])
    code, value, error = s.stop("stop --dry-run --requires (application entry)", ["--dry-run", "--requires", "bluearchive.jp.return_home"])
    check("R24-6.requires_on_application_entry", code == 2 and (error or {}).get("code") == "record_requires_invalid"
          and ((error or {}).get("details") or {}).get("reason") == "application_entry", "")
    code, value, error = s.stop("stop --application-arrival-timeout-ms 1800001", ["--application-arrival-timeout-ms", "1800001"])
    check("R24-7.application_timeout_out_of_range", code == 2 and (error or {}).get("code") == "validation_failed", "")
    r_dir = os.path.join(out, "install", "packages", "bluearchive")
    code, value, _ = s.stop("stop --lab-dir", ["--lab-dir", r_dir])
    r1 = lab_of(value)
    show("R24-1", r1, ["container", "digest", "pages", "application_steps", "timeouts", "first_decision", "warnings", "cross_check",
                       "entry_overlay", "binding_requires"])
    d_r1 = r1.get("digest")
    files = read_container(os.path.join(r_dir, f"{d_r1}.zip"))
    control, task = json.loads(files["control.json"]), task_of(files)
    say("R24-1", "control.json", json.dumps(control))
    say("R24-1", "task.json head", json.dumps({key: task.get(key) for key in ("entry_page", "target_page", "timeout_ms", "max_steps")}))
    say("R24-1", "operations", json.dumps(task["operations"]))
    say("R24-1", "provenance step 1", json.dumps(task["provenance"]["steps"][0]))
    gates = (r1.get("cross_check") or {}).get("gates") or []
    first = r1.get("first_decision") or {}
    check("R24-1.generated", code == 0 and r1.get("container") == "zip" and task["entry_page"] == "any"
          and task["operations"][0]["id"] == "step_01_app" and task["operations"][0]["from"] == "any"
          and task["operations"][0]["application"] == {"action": "restart"}, "")
    check("R24-1.first_decision_not_evaluated", first.get("status") == "not_evaluated" and first.get("refusal") == APPLICATION_REFUSAL
          and first.get("reason") == "lab_application_effect_offline" and "first_decision_not_evaluated" not in warning_codes(r1), json.dumps(first))
    check("R24-1.not_applicable", gates and gates[0].get("result") == "not_applicable"
          and (r1.get("entry_overlay") or {}).get("status") == "not_applicable", "")
    check("R24-1.provenance_entry_step", set(task["provenance"]["steps"][0]) == {"step", "record_index", "page", "application"}, "")
    check("R24-1.binding_requires_application", any("startup_package" in item for item in r1.get("binding_requires") or [])
          and any("application_id" in item for item in r1.get("binding_requires") or []), "")
    check("R24-7.default_timeout_126200", task["timeout_ms"] == 126200 and control["timeout_ms"] == 126200
          and (r1.get("timeouts") or {}).get("timeout_ms") == 126200, f"{task['timeout_ms']}")
    check("R24-2.restart_title_home", task["max_steps"] == 2 and task["target_page"] == "step_03_home", "")
    dir_form = os.path.join(out, "dir-form", d_r1)
    for path, data in files.items():
        target = os.path.join(dir_form, *path.split("/"))
        os.makedirs(os.path.dirname(target), exist_ok=True)
        with open(target, "wb") as handle:
            handle.write(data)
    code, measured = package_digest(exe, dir_form, "R24-8 package digest --package <D> directory")
    check("R24-8.directory_form", code == 0 and measured == d_r1, f"{measured}")

    # R24 item 2 (replaced, R25): restart -> splash (page transition) -> title -> home.
    say("R24-2", "restart, splash as the transition of the entry step, title, home")
    s = Session(exe, work, "r2", "cold_splash")
    s.mark("declare restart", ["--application", "restart"])
    s.mark("mark splash", ["--frame", f("loading"), "--color", "splash/bar=600,358,8,4"])
    s.mark("splash to transition", ["--to-transition", "2"])
    s.mark("mark title + click", ["--frame", f("off"), "--page", "title", "--color", "state/title=1173,680,1,1", "--click", "600,300,80,40"])
    s.mark("mark home", ["--frame", f("on"), "--page", "home", "--color", "state/home=1173,680,1,1"])
    code, value, _ = s.stop("stop", [])
    r2 = lab_of(value)
    show("R24-2", r2, ["pages", "transitions", "timeouts", "first_decision"])
    task = task_of(read_container(r2.get("path") or ""))
    check("R24-2.generated", code == 0 and r2.get("pages") == ["transition_01", "step_02_title", "step_03_home"]
          and task["max_steps"] == 2 and task["target_page"] == "step_03_home"
          and task["operations"][0].get("transition") == {"kind": "page", "page_id": "transition_01"}, "")

    # R25 refusal: a restart without a later main interface.
    say("R25", "restart -> title only")
    s = Session(exe, work, "r25", "cold_title")
    s.mark("declare restart", ["--application", "restart"])
    s.mark("mark title", ["--frame", f("off"), "--page", "title", "--color", "state/title=1173,680,1,1"])
    for label, args in (("stop --dry-run", ["--dry-run"]), ("stop", [])):
        code, value, error = s.stop(label, args)
        check(f"R25.{label}", code == 3 and (error or {}).get("code") == "record_application_without_home"
              and s.status() == ("active", "active"), "")

    # R24 item 4: a launch in the middle whose next page already passes.
    say("R24-4", "launch in the middle, next page already on its frame")
    s = Session(exe, work, "r4", "launch_mid")
    s.mark("mark a + click", ["--frame", f("off"), "--page", "a", "--template", tpl("auto_off.png", "ui/a", sizes, 1180, 664),
                              "--click-from", "ui/a"])
    s.mark("mark b + launch", ["--frame", f("on"), "--page", "b", "--template", tpl("auto_on.png", "ui/b", sizes, 1180, 664),
                               "--color", "state/b=1173,680,1,1", "--application", "launch"])
    s.mark("mark home (same screen)", ["--frame", f("on_copy"), "--page", "home", "--reuse", "ui/b", "--reuse", "state/b"])
    code, value, _ = s.stop("stop --dry-run", ["--dry-run"])
    r4 = lab_of(value)
    show("R24-4", r4, ["warnings", "first_decision"])
    arrival = [w for w in r4.get("warnings") or [] if w.get("code") == "arrival_unconfirmed"]
    check("R24-4.arrival_and_application_hint", code == 0 and len(arrival) == 1 and arrival[0].get("step") == 2
          and "application operation" in arrival[0].get("message", "") and "first_decision_not_evaluated" in warning_codes(r4), "")

    # R24 item 5: stop then a click is refused; stop then launch is generated with a warning.
    say("R24-5", "stop then click; stop then launch")
    s = Session(exe, work, "r5a", "stop_click")
    s.mark("mark a + stop", ["--frame", f("off"), "--page", "a", "--color", "state/a=1173,680,1,1", "--application", "stop"])
    s.mark("mark desktop + click", ["--frame", f("desktop"), "--page", "desk", "--color", "state/desk=20,20,4,4", "--click", "600,300,80,40"])
    s.mark("mark c", ["--frame", f("on"), "--page", "c", "--color", "state/c=1173,680,1,1"])
    code, value, error = s.stop("stop --dry-run", ["--dry-run"])
    check("R24-5.click_after_stop", code == 3 and (error or {}).get("code") == "record_step_after_stop_invalid", "")
    s = Session(exe, work, "r5b", "stop_launch")
    s.mark("mark a + stop", ["--frame", f("off"), "--page", "a", "--color", "state/a=1173,680,1,1", "--application", "stop"])
    s.mark("mark desktop + launch", ["--frame", f("desktop"), "--page", "desk", "--color", "state/desk=20,20,4,4", "--application", "launch"])
    s.mark("mark home", ["--frame", f("on"), "--page", "home", "--color", "state/home=1173,680,1,1"])
    code, value, _ = s.stop("stop", [])
    r5 = lab_of(value)
    show("R24-5", r5, ["warnings", "first_decision", "application_steps"])
    check("R24-5.stop_then_launch", code == 0 and r5.get("status") == "generated"
          and "application_stop_target_external" in warning_codes(r5), "")

    # Optional steps are refused until L4o.
    say("OPT", "a recording with an optional step")
    s = Session(exe, work, "opt", "with_optional")
    s.mark("mark a + click", ["--frame", f("off"), "--color", "state/a=1173,680,1,1", "--click", "600,300,80,40"])
    s.mark("mark pop-up optional", ["--frame", f("popup"), "--color", "notice/panel=320,180,8,8", "--click", "600,480,40,16", "--optional"])
    s.mark("close the offline step", ["--close-step"])
    s.mark("mark b", ["--frame", f("on"), "--color", "state/b=1173,680,1,1"])
    before = s.sha()
    for label, args in (("stop --dry-run", ["--dry-run"]), ("stop", [])):
        code, value, error = s.stop(label, args)
        check(f"OPT.{label}", code == 6 and (error or {}).get("code") == "record_stop_generation_not_implemented"
              and ((error or {}).get("details") or {}).get("reason") == "optional_steps"
              and s.sha() == before and s.status() == ("active", "active"), "")

    # E3, E8, E9, R24-3: small packages on the fixture backend (scheduled dispatch).
    fixture_dir = os.path.join(out, "fixture", "packages", GAME)
    produced = []
    say("E3", "end to end: a small recording run on the fixture")
    s = Session(exe, work, "e3", "e2e_small", game=GAME, server=SERVER, locale="en-US")
    s.mark("mark home + click", ["--frame", sf("home"), "--page", "home", "--color", "state/home=10,10,1,1", "--click", "8,8,5,5"])
    s.mark("mark a2", ["--frame", sf("a2"), "--page", "a2", "--color", "state/a2=10,10,1,1"])
    code, value, _ = s.stop("stop --lab-dir", ["--lab-dir", fixture_dir])
    e3 = lab_of(value)
    show("E3", e3, ["container", "digest", "package_id", "lab_dir_path", "first_decision", "warnings"])
    root, rows, types, _ = fixture_case(work, "E3 fixture run", new_runtime, new_ledger, e3.get("lab_dir_path"),
                                        e3.get("package_ref"), [sf("home"), sf("a2")], catalog_dir, None)
    produced.append(("E3", root))
    check("E3.fixture_success", ["package_admitted", e3.get("package_id"), e3.get("digest")] in rows
          and ["terminal_committed", "success", 1, None] in rows, "")

    say("E8", "--requires: a package with a prerequisite, its refusals")
    h_root = os.path.join(out, "fixture", "h-package")
    h_digest = h_package(h_root)
    s = Session(exe, work, "e8", "pre_b", game=GAME, server=SERVER, locale="en-US")
    s.mark("mark home + click", ["--frame", sf("home"), "--page", "home", "--color", "state/home=10,10,1,1", "--click", "8,8,5,5"])
    s.mark("mark m", ["--frame", sf("m"), "--page", "m", "--color", "state/m=10,10,1,1"])
    own_id = f"{GAME}.{SERVER}.pre_b"
    code, value, error = s.stop("stop --dry-run --requires <own id>", ["--dry-run", "--requires", own_id])
    check("E8.requires_self", code == 2 and (error or {}).get("code") == "record_requires_invalid"
          and ((error or {}).get("details") or {}).get("reason") == "prerequisite_self", "")
    code, value, _ = s.stop("stop --requires H --lab-dir", ["--requires", ID_H, "--lab-dir", fixture_dir])
    e8 = lab_of(value)
    show("E8", e8, ["digest", "requires", "warnings", "prerequisite_entry_example", "binding_requires"])
    control = json.loads(read_container(e8.get("lab_dir_path") or "")["control.json"])
    say("E8", "control.json", json.dumps(control))
    check("E8.control_prerequisite", code == 0 and control.get("prerequisite_package_id") == ID_H
          and "requires_prefix_mismatch" in warning_codes(e8) and e8.get("requires") == ID_H, "")
    code, value, error = s.stop("stop --requires other (stopped)", ["--requires", "fixture.prereq.other", "--lab-dir", fixture_dir])
    check("E8.requires_conflict", code == 3 and (error or {}).get("code") == "record_requires_conflict", "")

    say("E9", "a two-level Lab chain on the fixture: C -> B (Lab) -> H")
    s = Session(exe, work, "e9", "pre_c", game=GAME, server=SERVER, locale="en-US")
    s.mark("mark m + click", ["--frame", sf("m"), "--page", "m", "--color", "state/m=10,10,1,1", "--click", "8,8,5,5"])
    s.mark("mark c2", ["--frame", sf("c2"), "--page", "c2", "--color", "state/c2=10,10,1,1"])
    code, value, _ = s.stop("stop --requires B --lab-dir", ["--requires", own_id, "--lab-dir", fixture_dir])
    e9 = lab_of(value)
    show("E9", e9, ["digest", "requires", "warnings"])
    mapping = [{"package_id": ID_H, "package_path": h_root, "package_digest": {"schema_version": SCHEMA_DIR, "sha256": h_digest}},
               e8.get("prerequisite_entry_example")]
    say("E9", "prerequisite_packages", json.dumps(mapping))
    root, rows, types, _ = fixture_case(work, "E9 fixture chain from X", new_runtime, new_ledger, e9.get("lab_dir_path"),
                                        e9.get("package_ref"),
                                        [sf(name) for name in ("x", "x", "x", "home", "home", "home", "m", "m", "m", "c2")],
                                        catalog_dir, mapping)
    produced.append(("E9", root))
    check("E9.chain_success", ["entry_recovery_package_admitted", e8.get("digest")] in rows
          and ["entry_recovery_package_admitted", h_digest] in rows
          and ["package_admitted", e9.get("package_id"), e9.get("digest")] in rows
          and ["terminal_committed", "success", 3, None] in rows and types.count("task.requested") == 1, "")

    say("R24-3", "restart -> title -> home on the fixture: denied before any capture")
    s = Session(exe, work, "r3", "cold_small", game=GAME, server=SERVER, locale="en-US")
    s.mark("declare restart", ["--application", "restart"])
    s.mark("mark title + click", ["--frame", sf("x"), "--page", "title", "--color", "state/title=10,10,1,1", "--click", "8,8,5,5"])
    s.mark("mark home", ["--frame", sf("home"), "--page", "home", "--color", "state/home=10,10,1,1"])
    code, value, _ = s.stop("stop --lab-dir", ["--lab-dir", fixture_dir])
    r3 = lab_of(value)
    show("R24-3", r3, ["digest", "first_decision"])
    root, rows, types, text = fixture_case(work, "R24-3 fixture run", new_runtime, new_ledger, r3.get("lab_dir_path"),
                                           r3.get("package_ref"), [sf("x"), sf("home")], catalog_dir, None)
    produced.append(("R24-3", root))
    check("R24-3.denied_without_capture", APPLICATION_REFUSAL in text and "capture.completed" not in types
          and not any(row[0] == "capture_completed" for row in rows), json.dumps(sorted(set(types))))

    # The v0.9.0 actingledger reads every fixture ledger.
    for label, root in produced:
        ledger_reads(f"v0.9.0 actingledger on {label}", old_ledger, root)

    say("RESULT", "failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


if __name__ == "__main__":
    command = sys.argv[1]
    if command == "prepare":
        sys.exit(prepare(sys.argv[2], sys.argv[3]))
    sys.exit(run(*sys.argv[2:7]))
