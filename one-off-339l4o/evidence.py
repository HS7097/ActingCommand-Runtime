# One-off (to be reverted), Workflow #339 L4o evidence (frozen model 5959674074 section 6.4
# "L4 evidence" with sections 2.5 and 4.7): record stop generates optional steps and runs the
# optional-step self-checks. Synthetic frames are built from the real assets of the BA
# notice_home pack of the umbrella v0.9.0 standard package, pasted at their declared
# coordinates; small 64x36 frames run the generated package on the fixture backend. Every
# printed line starts with "L4O|". Usage:
#   evidence.py prepare <work> <umbrella standard package zip>
#   evidence.py run <work> <new tools> <new runtime> <L4 tools> <catalog dir>
import hashlib
import io
import json
import os
import re
import shutil
import subprocess
import sys
import time
import traceback
import zipfile

from PIL import Image

SCHEMA_DIR = "actingcommand.package.content-directory.v1"
MARK_SCHEMA = "actingcommand.lab-record-mark.v1"
FAILURES = []
ENV = dict(os.environ)
INSTANCE = "emu-a"

# The fixture packages (64x36; the actingd configuration is limited to 1 MiB).
GAME = "fixture-game-a"
SERVER = "fixture-server-a"
ALIAS = "node.a"
W, H = 64, 36
BG = (32, 32, 32)

# BA notice_home declared regions (task.json of the pack): asset -> (x, y).
BA_AT = {
    "news_title.png": (121, 88),
    "news_close.png": (1127, 88),
    "event_reminder_header.png": (607, 103),
    "event_reminder_message.png": (410, 190),
    "event_reminder_ok.png": (520, 510),
    "home_cafe_label.png": (77, 680),
    "home_work_label.png": (1165, 667),
    "event_reminder_hud_cafe.png": (77, 680),
    "event_reminder_hud_work.png": (1165, 667),
}
DIM = 45  # the reminder backdrop: every channel times 0.45, as the real HUD crops show
BRIGHT = (560, 20, 40, 20)  # a fixed bright area of the home frame (x, y, w, h)


def say(*parts):
    print("L4O|" + "|".join(str(part) for part in parts), flush=True)


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


def save_png(image, path, optimize=False):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    image.save(path, format="PNG", optimize=optimize)


def pretty(value):
    return (json.dumps(value, indent=2) + "\n").encode("utf-8")


# ---------------------------------------------------------------------------------------------
# prepare

def pack_files(bundle, names, index, package_id):
    index_name = next(name for name in names if name.endswith("bundle.json"))
    pack = next(pack for pack in index["packs"] if pack["package_id"] == package_id)
    prefix = index_name[: -len("bundle.json")] + pack["path"] + "/"
    files = {name[len(prefix):]: bundle.read(name) for name in names if name.startswith(prefix) and not name.endswith("/")}
    check(f"prepare.source_digest.{package_id}", digest(files) == pack["digest"], digest(files))
    return files


def ccoeff(a, b):
    """ccoeff_normed of two same-size RGB images over all channels (a printout only)."""
    xs, ys = list(a.tobytes()), list(b.tobytes())
    mx, my = sum(xs) / len(xs), sum(ys) / len(ys)
    num = sum((x - mx) * (y - my) for x, y in zip(xs, ys))
    den = (sum((x - mx) ** 2 for x in xs) * sum((y - my) ** 2 for y in ys)) ** 0.5
    return num / den if den else float("nan")


def prepare(work, bundle_zip):
    os.makedirs(work, exist_ok=False)
    with zipfile.ZipFile(bundle_zip) as bundle:
        names = bundle.namelist()
        index = json.loads(bundle.read(next(name for name in names if name.endswith("bundle.json"))))
        battle = pack_files(bundle, names, index, "bluearchive.jp.battle_auto_enable")
        notice = pack_files(bundle, names, index, "bluearchive.jp.notice_home")
    images = {}
    for name in ("auto_off.png", "auto_on.png", "battle_cost_label.png"):
        images[name] = Image.open(io.BytesIO(battle["resources/operations/battle_auto_enable/assets/" + name])).convert("RGB")
    for name in BA_AT:
        images[name] = Image.open(io.BytesIO(notice["resources/operations/notice_home/assets/" + name])).convert("RGB")
    sizes = {name: list(image.size) for name, image in images.items()}
    say("prepare", "asset sizes", json.dumps(sizes))
    for home_name, dim_name in (("home_cafe_label.png", "event_reminder_hud_cafe.png"), ("home_work_label.png", "event_reminder_hud_work.png")):
        say("prepare", "ccoeff_normed (printout)", home_name, dim_name, round(ccoeff(images[home_name], images[dim_name]), 5))

    def texture():
        image = Image.new("RGB", (1280, 720), (32, 32, 32))
        for x in range(0, 1280, 8):
            for y in range(0, 640, 8):
                image.putpixel((x, y), (32 + (x // 8) % 16, 32 + (y // 8) % 16, 40))
        return image

    def paste(image, *names):
        for name in names:
            image.paste(images[name], BA_AT[name])
        return image

    def fill(image, rect, color):
        x0, y0, w, h = rect
        for x in range(x0, x0 + w):
            for y in range(y0, y0 + h):
                image.putpixel((x, y), color)
        return image

    def dim(image):
        return image.point(lambda value: value * DIM // 100)

    def ba_home():
        # The main screen: its HUD labels and a fixed bright area.
        return fill(paste(texture(), "home_cafe_label.png", "home_work_label.png"), BRIGHT, (250, 250, 250))

    def ba_news():
        return paste(texture(), "news_title.png", "news_close.png")

    def ba_reminder_visible():
        # The reminder over the main screen: the backdrop darkens it, the HUD stays visible
        # (the pack's own dimmed HUD crops), then the reminder's header, message and OK.
        return paste(dim(ba_home()), "event_reminder_hud_cafe.png", "event_reminder_hud_work.png",
                     "event_reminder_header.png", "event_reminder_message.png", "event_reminder_ok.png")

    def ba_reminder_hidden():
        # The same reminder with the home UI hidden: no HUD, no bright area.
        return paste(dim(texture()), "event_reminder_header.png", "event_reminder_message.png", "event_reminder_ok.png")

    def battle_frame(state, band=False):
        image = texture()
        image.paste(images["auto_off.png" if state == "off" else "auto_on.png"], (1180, 664))
        image.paste(images["battle_cost_label.png"], (778, 649))
        image.putpixel((1173, 680), (224, 225, 227) if state == "off" else (255, 229, 26))
        if band:
            fill(image, (200, 356, 880, 8), (90, 200, 255))
        return image

    def battle_popup():
        image = battle_frame("on").point(lambda value: value // 2)
        fill(image, (300, 160, 680, 400), (235, 235, 240))
        image.paste(images["battle_cost_label.png"], (600, 480))
        return image

    frames = os.path.join(work, "frames")
    save_png(ba_news(), os.path.join(frames, "news.png"))
    save_png(ba_home(), os.path.join(frames, "home.png"))
    save_png(ba_reminder_visible(), os.path.join(frames, "rem_visible.png"))
    save_png(ba_reminder_hidden(), os.path.join(frames, "rem_hidden.png"))
    save_png(battle_frame("off"), os.path.join(frames, "off.png"))
    save_png(battle_frame("on"), os.path.join(frames, "on.png"))
    save_png(battle_frame("on"), os.path.join(frames, "on_copy.png"), optimize=True)
    save_png(battle_frame("off", band=True), os.path.join(frames, "loading.png"))
    save_png(battle_popup(), os.path.join(frames, "popup.png"))

    # Small frames: a 3x3 block A at (9..11, 9..11) and an optional block B at (19..21, 9..11).
    def small(name, a, b=None):
        image = Image.new("RGB", (W, H), BG)
        for y in range(9, 12):
            for x in range(9, 12):
                image.putpixel((x, y), a)
                if b is not None:
                    image.putpixel((x + 10, y), b)
        save_png(image, os.path.join(work, "small", name + ".png"))

    red, blue, green, gray, white, yellow = (200, 40, 40), (40, 40, 200), (40, 200, 40), (120, 120, 120), (250, 250, 250), (230, 230, 40)
    small("fx_s1", (200, 40, 200))
    small("fx_pop", (40, 200, 200))
    small("fx_s3", (40, 200, 40))
    small("prev_s1", red, gray)
    small("prev_o2", blue, gray)
    small("prev_s3", green, white)
    small("other_s1", red)
    small("other_o2", blue, gray)
    small("other_o3", blue, white)
    small("other_s4", green)
    small("detour_s1", red, yellow)
    small("detour_o2", blue, gray)
    small("detour_s3", green, yellow)
    for number, color in enumerate(((200, 40, 40), (40, 40, 200), (40, 200, 200), (40, 200, 40), (200, 200, 40), (200, 40, 200)), 1):
        small(f"t{number}", color)
    with open(os.path.join(work, "cases.json"), "w", encoding="utf-8") as handle:
        json.dump({"sizes": sizes}, handle, indent=2)
    say("prepare", "frames", sorted(os.listdir(frames)), "small", sorted(os.listdir(os.path.join(work, "small"))))
    return 1 if FAILURES else 0


# ---------------------------------------------------------------------------------------------
# ActingLab helpers

def run_exe(args, timeout=600, env=None, cwd=None):
    try:
        result = subprocess.run(args, capture_output=True, timeout=timeout, env=env or ENV, cwd=cwd)
    except subprocess.TimeoutExpired:
        return None, "", "timeout"
    return result.returncode, result.stdout.decode("utf-8", "replace"), result.stderr.decode("utf-8", "replace")


def lab(exe, args, label, limit=2500):
    code, out, err = run_exe([exe, "--json", *args])
    try:
        value = json.loads(out)
    except ValueError:
        value = None
    say(label, "exit", code, "envelope", short(out, limit) if out else short(err, 800))
    return code, value, (value or {}).get("error")


def data_of(value):
    return (value or {}).get("data") or {}


def lab_of(value):
    return data_of(value).get("lab") or {}


def recording_file(state):
    for folder, _dirs, names in os.walk(os.path.join(state, "record-artifacts")):
        if "recording.json" in names:
            return os.path.join(folder, "recording.json")
    return None


def warnings_of(lab_data, code=None):
    return [warning for warning in lab_data.get("warnings") or [] if code is None or warning.get("code") == code]


def show(label, lab_data, keys):
    for key in keys:
        say(label, key, short(json.dumps(lab_data.get(key), ensure_ascii=False), 4000))


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
        check(f"{self.name}.{label}", code == exit_code and (error_code is None or got == error_code), f"exit {code} code {got}")
        return code, value, error

    def request(self, label, request):
        return self.mark(label, ["--request-json", json.dumps({"schema_version": MARK_SCHEMA, **request})])

    def stop(self, label, args, exe=None, state=None, limit=8000):
        return lab(exe or self.exe, self.base + ["record", "stop", "--state-dir", state or self.state, *args], f"{self.name} {label}", limit=limit)

    def status(self):
        code, value, _ = lab(self.exe, self.base + ["record", "status", "--state-dir", self.state], f"{self.name} status", limit=600)
        data = data_of(value)
        return (data.get("record") or {}).get("status"), (data.get("lab") or {}).get("status")

    def sha(self):
        return file_sha(recording_file(self.state))

    def frame_ids(self):
        with open(recording_file(self.state), encoding="utf-8") as handle:
            recording = json.load(handle)
        return {step["index"]: [frame["frame_id"] for frame in step["frames"] if not frame["superseded"]] for step in recording["steps"]}

    def out_dir_exists(self):
        return os.path.exists(os.path.join(os.path.dirname(recording_file(self.state)), "out"))


def read_container(path):
    if path.endswith(".zip"):
        with zipfile.ZipFile(path) as archive:
            return {info.filename: archive.read(info.filename) for info in archive.infolist() if not info.filename.endswith("/")}
    with open(path, encoding="utf-8") as handle:
        document = json.load(handle)
    return {key: value.encode("utf-8") for key, value in document["files"].items()}


def task_of(files):
    return json.loads(files[next(path for path in files if path.endswith("/task.json"))])


def tpl(name, region_id, sizes, x=None, y=None):
    w, h = sizes[name]
    if x is None:
        x, y = BA_AT[name]
    return f"{region_id}={x},{y},{w},{h}"


def expected_timeout(effect_steps, settles, step_timeout=15000, arrival=15000):
    """The L4 formula for click steps without window, page transition or retry, plus the
    Workflow #339 settles (one value per run: its largest settle)."""
    return step_timeout + effect_steps * (200 + arrival) + sum(settles) + 10000


# ---------------------------------------------------------------------------------------------
# Fixture actingd runs (scheduled dispatch; the fixture refuses direct task-run)

def catalog_documents(catalog_dir):
    def load(name):
        with open(os.path.join(catalog_dir, name + ".json"), encoding="utf-8") as handle:
            return json.load(handle)

    scope = {"kind": "instance", "instance_id": ALIAS}
    tasks, pools, activity, timeline = (load(name) for name in ("tasks", "pools", "activity", "timeline"))
    task = tasks["tasks"][0]
    task["scope"] = scope
    task["trigger"]["predicates"][0]["schedule"]["every_ms"] = 60000
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


def write_config(config_dir, state_root, package_path, package_ref, frames, catalog_dir, instance_number):
    os.makedirs(os.path.join(config_dir, "policy"), exist_ok=False)
    for name, document in catalog_documents(catalog_dir).items():
        with open(os.path.join(config_dir, "policy", name + ".json"), "w", encoding="utf-8") as handle:
            json.dump(document, handle, indent=2)
    now = int(time.time() * 1000)
    config = {
        "schema_version": "actingcommand.actingd.config.v1",
        "state_root": state_root,
        "bind_host": "127.0.0.1",
        "bind_port": 0,
        "secret_fingerprint_salt": "oneoff-339-l4o-fixture-salt-value",
        "policy": {
            "facts": {
                "ledger_position": 0, "fact_snapshot_id": "snapshot:oneoff-339-l4o",
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
                "package_digest": package_ref,
                "operation_id": "operation.observe",
                "yield_points": ["after_observation"],
                "scheduled_execution": {"mode": "fixture_simulation", "package_path": package_path},
            }],
        },
        "instances": [{
            "alias": ALIAS,
            "instance_id": f"instance_{instance_number:032x}",
            "fixture_backend": {
                "frames": [{"width": W, "height": H, "rgb": list(Image.open(frame).convert("RGB").tobytes())} for frame in frames],
                "max_inputs": 4,
            },
        }],
    }
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


def scheduled_run(work, label, runtime_dir, package_path, package_ref, frames, catalog_dir, instance_number, settle_s=20):
    run_dir = os.path.join(work, "runs", re.sub(r"[^A-Za-z0-9_.-]", "_", label))
    state_root = os.path.join(run_dir, "state")
    os.makedirs(state_root)
    config = write_config(os.path.join(run_dir, "config"), state_root, package_path, package_ref, frames, catalog_dir, instance_number)
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
            facts.append((event["sequence"], event["event_type"], fact, event.get("timestamp_unix_ms")))
    return facts


def first_run(facts):
    for index, item in enumerate(facts):
        if item[2]["kind"] == "terminal_committed":
            return facts[:index + 1]
    return facts


def fixture_case(work, label, runtime, ledger, package_path, package_ref, frames, catalog_dir, instance_number):
    root = scheduled_run(work, label, runtime, package_path, package_ref, frames, catalog_dir, instance_number)
    events, failure = ledger_events(ledger, root)
    if events is None:
        say(label, "events failed", failure)
        FAILURES.append(f"{label}.ledger_events")
        events = []
    facts = first_run(facts_of(events))
    for sequence, event_type, fact, stamp in facts:
        kind = fact["kind"]
        if kind in ("recognition_started", "recognition_completed"):
            detail = {"candidate_pages": fact.get("candidate_pages"), "matched_page": fact.get("matched_page")}
        elif kind in ("step_started", "step_finished", "effect_intent", "effect_completed"):
            detail = {key: fact.get(key) for key in ("step_index", "operation_label", "from_page", "page_label") if key in fact}
        elif kind == "terminal_committed":
            detail = {key: fact.get(key) for key in ("outcome", "final_page", "executed_steps", "failure_code")}
        elif kind == "package_admitted":
            detail = {key: fact.get(key) for key in ("package_label", "task_label")}
        else:
            detail = {}
        say(label, "fact", sequence, stamp, kind, short(json.dumps(detail, ensure_ascii=False), 800))
    for event in events:
        if event.get("event_type") in ("runtime.failed", "command.rejected", "task.failed", "runtime.resource_declaration_rejected"):
            say(label, event["event_type"], event["sequence"], short(json.dumps(event.get("payload"), ensure_ascii=False), 2000))
    return [item[2] for item in facts], [item[3] for item in facts]


def kinds(facts, kind):
    return [fact for fact in facts if fact["kind"] == kind]


def suffix(page):
    return page.rsplit("/", 1)[-1] if isinstance(page, str) else page


# ---------------------------------------------------------------------------------------------
# The evidence

def guarded(name, function):
    """Runs one evidence section; an exception is a failed check, the next section still runs."""
    try:
        function()
    except Exception:  # noqa: BLE001 - every failure is reported, never swallowed
        check(f"{name}.completed", False, short(traceback.format_exc(), 3000))


def run(work, new_tools, new_runtime, l4_tools, catalog_dir):
    exe = os.path.join(new_tools, "actinglab.exe")
    l4_exe = os.path.join(l4_tools, "actinglab.exe")
    ledger = os.path.join(new_tools, "actingledger.exe")
    with open(os.path.join(work, "cases.json"), encoding="utf-8") as handle:
        sizes = json.load(handle)["sizes"]
    f = lambda name: os.path.join(work, "frames", name + ".png")
    sf = lambda name: os.path.join(work, "small", name + ".png")
    out = os.path.join(work, "out")
    # --lab-dir creates only its last level: the parents exist, as in an install root.
    os.makedirs(os.path.join(out, "install", "packages"))
    os.makedirs(os.path.join(out, "fixture", "packages"))
    ba_dir = os.path.join(out, "install", "packages", "bluearchive")
    fixture_dir = os.path.join(out, "fixture", "packages", GAME)

    def news(s):
        s.mark("mark NEWS + close click", ["--frame", f("news"), "--page", "news",
                                            "--template", tpl("news_title.png", "news/title", sizes),
                                            "--template", tpl("news_close.png", "news/close", sizes), "--click-from", "news/close"])

    def section_ba_a():
        say("BA-A", "home marked by its HUD templates only -> skip target insensitive; a bright-area color fixes it; ZIP")
        s = Session(exe, work, "ba_a", "notice_reminder")
        news(s)
        s.mark("mark reminder (optional, settle 1500)", ["--frame", f("rem_visible"), "--page", "reminder",
                                                         "--template", tpl("event_reminder_header.png", "rem/header", sizes),
                                                         "--template", tpl("event_reminder_ok.png", "rem/ok", sizes),
                                                         "--click-from", "rem/ok", "--optional", "--settle-ms", "1500"])
        s.mark("mark home (HUD templates only)", ["--frame", f("home"), "--page", "home",
                                                  "--template", tpl("home_cafe_label.png", "hud/cafe", sizes),
                                                  "--template", tpl("home_work_label.png", "hud/work", sizes)])
        ids = s.frame_ids()
        before = s.sha()
        for label, args in (("stop --dry-run (HUD only)", ["--dry-run"]), ("stop (HUD only)", [])):
            code, value, error = s.stop(label, args)
            details = (error or {}).get("details") or {}
            check(f"BA-A.{label}.skip_target_insensitive", code == 3 and (error or {}).get("code") == "record_optional_skip_target_insensitive"
                  and details == {"step": 3, "optional_step": 2, "frames": ids[2]} and s.sha() == before
                  and s.status() == ("active", "active") and not s.out_dir_exists(), json.dumps(details))
        s.mark("add a --color on the bright area of home", ["--step", "3", "--color", "hud/bright=570,25,4,4"])
        before = s.sha()
        code, value, _ = s.stop("stop --dry-run (with the color)", ["--dry-run"])
        dry = lab_of(value)
        show("BA-A dry-run", dry, ["status", "digest", "optional_steps", "warnings", "timeouts", "cross_check"])
        check("BA-A.dry_run_validated", code == 0 and dry.get("status") == "validated" and s.sha() == before
              and s.status() == ("active", "active") and not s.out_dir_exists(), "")
        code, value, _ = s.stop("stop --lab-dir", ["--lab-dir", ba_dir])
        a = lab_of(value)
        show("BA-A", a, ["status", "container", "digest", "entries", "pages", "optional_steps", "timeouts", "warnings",
                         "arrival_by_time_window", "first_decision", "cross_check", "binding_requires"])
        d = a.get("digest")
        path = os.path.join(ba_dir, f"{d}.zip")
        files = read_container(path)
        task, control = task_of(files), json.loads(files["control.json"])
        say("BA-A", "task.json operations", json.dumps(task["operations"]))
        say("BA-A", "task.json provenance steps", json.dumps(task["provenance"]["steps"]))
        say("BA-A", "control.json", json.dumps(control))
        ops = {op["id"]: op for op in task["operations"]}
        expected = expected_timeout(2, [1500])
        check("BA-A.generated_zip_admitted", code == 0 and a.get("status") == "generated" and a.get("container") == "zip"
              and dry.get("digest") == d and digest(files) == d and os.path.isfile(path), d or "")
        check("BA-A.operation_optional", ops["step_02_click"].get("optional") == {"settle_ms": 1500}
              and "optional" not in ops["step_01_click"]
              and ops["step_02_click"]["provenance"].get("optional", {}).get("settle_ms") == 1500
              and isinstance(ops["step_02_click"]["provenance"]["optional"].get("marked_at_unix_ms"), int)
              and task["provenance"]["steps"][1].get("optional", {}).get("settle_ms") == 1500
              and "optional" not in task["provenance"]["steps"][0] and "optional" not in task["provenance"]["steps"][2], "")
        check("BA-A.optional_steps_output", a.get("optional_steps") == [
            {"step": 2, "op": "step_02_click", "settle_ms": 1500, "skip_to": "step_03_home", "same_as": None}], json.dumps(a.get("optional_steps")))
        check("BA-A.default_timeout_with_settle", control.get("timeout_ms") == expected and task.get("timeout_ms") == expected
              and (a.get("timeouts") or {}).get("timeout_ms") == expected, f"expected {expected}")
        check("BA-A.binding_requires_optional_line", any("Optional steps cover only" in item for item in a.get("binding_requires") or []), "")
        gates = (a.get("cross_check") or {}).get("gates") or []
        check("BA-A.gates", gates == [{"step": 1, "gate": "step_03_home", "result": "distinct"},
                                     {"step": 2, "gate": "step_03_home", "result": "distinct"}], json.dumps(gates))

    guarded("BA-A", section_ba_a)

    def section_ba_b():
        say("BA-B", "pop-up page marked only by the home HUD -> record_optional_step_ambiguous (skip_target)")
        s = Session(exe, work, "ba_b", "notice_popup_hud")
        news(s)
        s.mark("mark reminder (optional, HUD templates only)", ["--frame", f("rem_visible"), "--page", "reminder",
                                                                "--template", tpl("event_reminder_hud_cafe.png", "popup/hud_cafe", sizes),
                                                                "--template", tpl("event_reminder_hud_work.png", "popup/hud_work", sizes),
                                                                "--click", "600,530,80,40", "--optional"])
        s.mark("mark home (HUD + bright color)", ["--frame", f("home"), "--page", "home",
                                                  "--template", tpl("home_cafe_label.png", "hud/cafe", sizes),
                                                  "--template", tpl("home_work_label.png", "hud/work", sizes),
                                                  "--color", "hud/bright=570,25,4,4"])
        ids = s.frame_ids()
        before = s.sha()
        for label, args in (("stop --dry-run", ["--dry-run"]), ("stop", [])):
            code, value, error = s.stop(label, args)
            details = (error or {}).get("details") or {}
            check(f"BA-B.{label}.step_ambiguous_skip_target", code == 3 and (error or {}).get("code") == "record_optional_step_ambiguous"
                  and details == {"step": 2, "page": "step_02_reminder", "against": "skip_target", "frames": ids[3]}
                  and s.sha() == before and s.status() == ("active", "active"), json.dumps(details))

    guarded("BA-B", section_ba_b)

    def section_ba_c():
        say("BA-C", "reminder hidden and visible, both optional, same OK click -> same_as")
        s = Session(exe, work, "ba_c", "notice_reminder_states")
        news(s)
        s.mark("mark reminder hidden (optional, default settle)", ["--frame", f("rem_hidden"), "--page", "rem_hidden",
                                                                   "--template", tpl("event_reminder_header.png", "rem/header", sizes),
                                                                   "--template", tpl("event_reminder_message.png", "rem/message", sizes),
                                                                   "--template", tpl("event_reminder_ok.png", "rem/ok", sizes),
                                                                   "--click", "600,530,80,40", "--optional"])
        s.mark("mark reminder visible (optional, settle 1500)", ["--frame", f("rem_visible"), "--page", "rem_visible",
                                                                 "--reuse", "rem/header", "--reuse", "rem/message", "--reuse", "rem/ok",
                                                                 "--template", tpl("event_reminder_hud_cafe.png", "vis/hud_cafe", sizes),
                                                                 "--template", tpl("event_reminder_hud_work.png", "vis/hud_work", sizes),
                                                                 "--click", "600,530,80,40", "--optional", "--settle-ms", "1500"])
        s.mark("mark home (HUD + bright color)", ["--frame", f("home"), "--page", "home",
                                                  "--template", tpl("home_cafe_label.png", "hud/cafe", sizes),
                                                  "--template", tpl("home_work_label.png", "hud/work", sizes),
                                                  "--color", "hud/bright=570,25,4,4"])
        ids = s.frame_ids()
        before = s.sha()
        code, value, _ = s.stop("stop --dry-run", ["--dry-run"])
        dry = lab_of(value)
        check("BA-C.dry_run_writes_nothing", code == 0 and dry.get("status") == "validated" and s.sha() == before
              and s.status() == ("active", "active") and not s.out_dir_exists(), "")
        code, value, _ = s.stop("stop --lab-dir", ["--lab-dir", ba_dir])
        c = lab_of(value)
        show("BA-C", c, ["status", "digest", "pages", "optional_steps", "timeouts", "warnings", "arrival_by_time_window", "cross_check"])
        files = read_container(os.path.join(ba_dir, f"{c.get('digest')}.zip"))
        task = task_of(files)
        say("BA-C", "task.json operations", json.dumps(task["operations"]))
        ops = {op["id"]: op for op in task["operations"]}
        same = [w for w in warnings_of(c, "arrival_unconfirmed") if w.get("same_as")]
        check("BA-C.same_as", code == 0 and c.get("status") == "generated" and c.get("optional_steps") == [
            {"step": 2, "op": "step_02_click", "settle_ms": 2000, "skip_to": "step_04_home", "same_as": None},
            {"step": 3, "op": "step_03_click", "settle_ms": 1500, "skip_to": "step_04_home", "same_as": 2}]
              and dry.get("digest") == c.get("digest"), json.dumps(c.get("optional_steps")))
        check("BA-C.one_arrival_unconfirmed", len(warnings_of(c, "arrival_unconfirmed")) == 1 and len(same) == 1
              and same[0].get("step") == 3 and same[0].get("gate") == "step_02_rem_hidden" and same[0].get("frames") == ids[3]
              and same[0].get("same_as") == [2, 3], json.dumps(same))
        check("BA-C.no_refusal_no_other_warnings", not warnings_of(c, "optional_ambiguity_not_evaluated"), "")
        expected = expected_timeout(3, [2000])
        check("BA-C.timeout_largest_settle_of_the_run", task.get("timeout_ms") == expected and expected != expected_timeout(3, [2000, 1500]),
              f"{task.get('timeout_ms')} expected {expected}")
        check("BA-C.operations_optional", ops["step_02_click"].get("optional") == {"settle_ms": 2000}
              and ops["step_03_click"].get("optional") == {"settle_ms": 1500} and "optional" not in ops["step_01_click"], "")

    guarded("BA-C", section_ba_c)

    def section_pre():
        say("PRE", "pre-checks: optional last step; optional main interface after a restart")
        s = Session(exe, work, "pre_final", "optional_final")
        s.mark("mark t1 + click", ["--frame", sf("t1"), "--page", "a", "--color", "s/a=10,10,1,1", "--click", "8,8,5,5"])
        s.mark("mark t2 + click", ["--frame", sf("t2"), "--page", "b", "--color", "s/b=10,10,1,1", "--click", "8,8,5,5"])
        s.mark("mark t3 optional (last step)", ["--frame", sf("t3"), "--page", "c", "--color", "s/c=10,10,1,1", "--optional"])
        before = s.sha()
        for label, args in (("stop --dry-run", ["--dry-run"]), ("stop", [])):
            code, value, error = s.stop(label, args)
            check(f"PRE.final_step.{label}", code == 3 and (error or {}).get("code") == "record_optional_final_step"
                  and (error or {}).get("details") == {"step": 3, "artifact_step": 3} and s.sha() == before
                  and s.status() == ("active", "active"), json.dumps((error or {}).get("details")))
        s = Session(exe, work, "pre_home", "optional_home")
        s.mark("declare restart (entry step)", ["--application", "restart"])
        s.mark("mark title + click", ["--frame", sf("t1"), "--page", "title", "--color", "s/title=10,10,1,1", "--click", "8,8,5,5"])
        s.mark("mark home + click, optional", ["--frame", sf("t2"), "--page", "home", "--color", "s/home=10,10,1,1", "--click", "8,8,5,5",
                                                "--optional"])
        s.mark("mark next", ["--frame", sf("t3"), "--page", "next", "--color", "s/next=10,10,1,1"])
        before = s.sha()
        code, value, error = s.stop("stop --dry-run", ["--dry-run"])
        check("PRE.restart_segment_end", code == 3 and (error or {}).get("code") == "record_optional_restart_segment_end"
              and (error or {}).get("details") == {"step": 3, "artifact_step": 3} and s.sha() == before, json.dumps((error or {}).get("details")))

    guarded("PRE", section_pre)

    def section_self():
        say("SELF", "optional page on the previous step's frames; on another optional step with another click; detour hint; OCR")
        s = Session(exe, work, "self_prev", "self_previous")
        s.mark("mark s1 + click", ["--frame", sf("prev_s1"), "--color", "p/s1=10,10,1,1", "--click", "8,8,5,5"])
        s.mark("mark o2 by block B only, optional", ["--frame", sf("prev_o2"), "--color", "p/o2b=20,10,1,1", "--click", "30,8,5,5", "--optional"])
        s.mark("mark s3", ["--frame", sf("prev_s3"), "--color", "p/s3=10,10,1,1"])
        ids = s.frame_ids()
        code, value, error = s.stop("stop --dry-run", ["--dry-run"])
        check("SELF.previous", code == 3 and (error or {}).get("code") == "record_optional_step_ambiguous"
              and (error or {}).get("details") == {"step": 2, "page": "step_02", "against": "previous", "frames": ids[1]},
              json.dumps((error or {}).get("details")))

        s = Session(exe, work, "self_other", "self_other")
        s.mark("mark s1 + click", ["--frame", sf("other_s1"), "--color", "q/s1=10,10,1,1", "--click", "8,8,5,5"])
        s.mark("mark o2 by block A, optional", ["--frame", sf("other_o2"), "--color", "q/o2a=10,10,1,1", "--click", "8,8,5,5", "--optional"])
        s.mark("mark o3 by block A (reused) and B, other click, optional", ["--frame", sf("other_o3"), "--reuse", "q/o2a",
                                                                          "--color", "q/o3b=20,10,1,1", "--click", "30,8,5,5", "--optional"])
        s.mark("mark s4", ["--frame", sf("other_s4"), "--color", "q/s4=10,10,1,1"])
        ids = s.frame_ids()
        code, value, error = s.stop("stop --dry-run", ["--dry-run"])
        check("SELF.other_optional", code == 3 and (error or {}).get("code") == "record_optional_step_ambiguous"
              and (error or {}).get("details") == {"step": 2, "page": "step_02", "against": "other_optional", "frames": ids[3]},
              json.dumps((error or {}).get("details")))

        s = Session(exe, work, "self_detour", "self_detour")
        s.mark("mark s1 + click", ["--frame", sf("detour_s1"), "--color", "d/s1=10,10,1,1", "--click", "8,8,5,5"])
        s.mark("mark o2, optional", ["--frame", sf("detour_o2"), "--color", "d/o2=10,10,1,1", "--click", "30,8,5,5", "--optional"])
        s.mark("mark s3 by block B only (also on s1)", ["--frame", sf("detour_s3"), "--color", "d/s3b=20,10,1,1"])
        ids = s.frame_ids()
        code, value, _ = s.stop("stop --dry-run", ["--dry-run"])
        d = lab_of(value)
        arrival = warnings_of(d, "arrival_unconfirmed")
        check("SELF.detour_hint", code == 0 and len(arrival) == 1 and arrival[0].get("step") == 1 and arrival[0].get("gate") == "step_03"
              and arrival[0].get("frames") == ids[1] and "page-graph package" in arrival[0].get("message", "")
              and "settle" in arrival[0].get("message", ""), json.dumps(arrival))

        s = Session(exe, work, "self_ocr", "self_ocr")
        s.mark("mark s1 + click", ["--frame", sf("t1"), "--color", "o/s1=10,10,1,1", "--click", "8,8,5,5"])
        s.request("mark o2 by OCR only, optional", {"frame": sf("t2"), "add": [
            {"id": "o/text", "family": "ocr", "region": {"x": 2, "y": 2, "width": 30, "height": 10}, "languages": ["ja"],
             "timeout_ms": 1000, "match_mode": "contains", "expected": ["OK"], "case_sensitive": False, "minimum_confidence": 0.8,
             "model_ref": "PP-OCRv6_medium", "model_sha256": "0" * 64}],
            "click": {"region": {"x": 8, "y": 8, "width": 5, "height": 5}}, "optional": True, "optional_settle_ms": 1000})
        s.mark("mark s3", ["--frame", sf("t3"), "--color", "o/s3=10,10,1,1"])
        ids = s.frame_ids()
        code, value, _ = s.stop("stop --dry-run", ["--dry-run"])
        o = lab_of(value)
        show("SELF-OCR", o, ["warnings", "cross_check", "optional_steps"])
        not_evaluated = warnings_of(o, "optional_ambiguity_not_evaluated")
        check("SELF.ocr_not_evaluated", code == 0 and o.get("status") == "validated"
              and [(w.get("check"), w.get("step"), w.get("against"), w.get("frames")) for w in not_evaluated] == [
                  ("record_optional_step_ambiguous", 2, "previous", ids[1]), ("record_optional_step_ambiguous", 2, "skip_target", ids[3])]
              and (o.get("cross_check") or {}).get("status") == "partially_evaluated", json.dumps(not_evaluated))

    guarded("SELF", section_self)

    def section_timeout():
        say("TIMEOUT", "two runs: [o2 2000][o3 1500] and [o5 700]: the default timeout adds 2000 + 700")
        s = Session(exe, work, "timeout_runs", "timeout_runs")
        s.mark("mark t1 + click", ["--frame", sf("t1"), "--color", "t/1=10,10,1,1", "--click", "8,8,5,5"])
        s.mark("mark t2 optional 2000", ["--frame", sf("t2"), "--color", "t/2=10,10,1,1", "--click", "8,8,5,5", "--optional"])
        s.mark("mark t3 optional 1500", ["--frame", sf("t3"), "--color", "t/3=10,10,1,1", "--click", "8,8,5,5", "--optional", "--settle-ms", "1500"])
        s.mark("mark t4 + click", ["--frame", sf("t4"), "--color", "t/4=10,10,1,1", "--click", "8,8,5,5"])
        s.mark("mark t5 optional 700", ["--frame", sf("t5"), "--color", "t/5=10,10,1,1", "--click", "8,8,5,5", "--optional", "--settle-ms", "700"])
        s.mark("mark t6", ["--frame", sf("t6"), "--color", "t/6=10,10,1,1"])
        code, value, _ = s.stop("stop --dry-run", ["--dry-run"])
        t = lab_of(value)
        show("TIMEOUT", t, ["timeouts", "optional_steps", "warnings"])
        expected = expected_timeout(5, [2000, 700])
        check("TIMEOUT.sum_of_run_maxima", code == 0 and (t.get("timeouts") or {}).get("timeout_ms") == expected
              and [(item["step"], item["skip_to"]) for item in t.get("optional_steps") or []] == [(2, "step_04"), (3, "step_04"), (5, "step_06")],
              f"{(t.get('timeouts') or {}).get('timeout_ms')} expected {expected} (all settles summed: {expected_timeout(5, [2000, 1500, 700])})")

    guarded("TIMEOUT", section_timeout)

    def section_fixture():
        say("FIX", "a generated package with an optional pop-up, run on the fixture: pop-up shown, pop-up absent")
        s = Session(exe, work, "fix", "fix_optional", game=GAME, server=SERVER, locale="en-US")
        s.mark("mark s1 + click", ["--frame", sf("fx_s1"), "--page", "start", "--color", "f/s1=10,10,1,1", "--click", "8,8,5,5"])
        s.mark("mark pop-up + click, optional 500", ["--frame", sf("fx_pop"), "--page", "pop", "--color", "f/pop=10,10,1,1",
                                                      "--click", "8,8,5,5", "--optional", "--settle-ms", "500"])
        s.mark("mark s3", ["--frame", sf("fx_s3"), "--page", "done", "--color", "f/s3=10,10,1,1"])
        code, value, _ = s.stop("stop --lab-dir", ["--lab-dir", fixture_dir])
        x = lab_of(value)
        show("FIX", x, ["container", "digest", "package_id", "lab_dir_path", "optional_steps", "timeouts", "first_decision", "warnings"])
        task = task_of(read_container(x.get("lab_dir_path") or ""))
        say("FIX", "task.json operations", json.dumps(task["operations"]))
        check("FIX.generated", code == 0 and x.get("status") == "generated"
              and [op.get("optional") for op in task["operations"]] == [None, {"settle_ms": 500}]
              and (x.get("timeouts") or {}).get("timeout_ms") == expected_timeout(2, [500]), "")
        shown, stamps = fixture_case(work, "FIX pop-up shown", new_runtime, ledger, x.get("lab_dir_path"), x.get("package_ref"),
                                     [sf("fx_s1"), sf("fx_pop"), sf("fx_s3"), sf("fx_s3"), sf("fx_s3"), sf("fx_s3")], catalog_dir, 0x3390401)
        starts = [[fact.get("step_index"), fact.get("operation_label"), suffix(fact.get("from_page"))] for fact in kinds(shown, "step_started")]
        finishes = [[fact.get("step_index"), suffix(fact.get("page_label"))] for fact in kinds(shown, "step_finished")]
        recognitions = [[[suffix(page) for page in fact.get("candidate_pages") or []], suffix(fact.get("matched_page"))]
                        for fact in kinds(shown, "recognition_completed")]
        terminal = (kinds(shown, "terminal_committed") or [{}])[-1]
        say("FIX shown", "summary", json.dumps({"starts": starts, "finishes": finishes, "recognitions": recognitions,
                                                "inputs": len(kinds(shown, "effect_intent")), "terminal": terminal.get("outcome"),
                                                "executed_steps": terminal.get("executed_steps")}))
        check("FIX.popup_shown", starts == [[0, "step_01_click", "step_01_start"], [1, "step_02_click", "step_02_pop"]]
              and finishes == [[0, "step_02_pop"], [1, "step_03_done"]]
              and ["step_02_pop", "step_03_done"] in [item[0] for item in recognitions]
              and len(kinds(shown, "effect_intent")) == 2 and terminal.get("outcome") == "success"
              and terminal.get("executed_steps") == 2 and suffix(terminal.get("final_page")) == "step_03_done", "")
        absent, stamps = fixture_case(work, "FIX pop-up absent", new_runtime, ledger, x.get("lab_dir_path"), x.get("package_ref"),
                                      [sf("fx_s1")] + [sf("fx_s3")] * 8, catalog_dir, 0x3390402)
        starts = [[fact.get("step_index"), fact.get("operation_label"), suffix(fact.get("from_page"))] for fact in kinds(absent, "step_started")]
        finishes = [[fact.get("step_index"), suffix(fact.get("page_label"))] for fact in kinds(absent, "step_finished")]
        matched_s3 = [stamp for fact, stamp in zip(absent, stamps) if fact["kind"] == "recognition_completed" and suffix(fact.get("matched_page")) == "step_03_done"]
        terminal = (kinds(absent, "terminal_committed") or [{}])[-1]
        say("FIX absent", "summary", json.dumps({"starts": starts, "finishes": finishes, "inputs": len(kinds(absent, "effect_intent")),
                                                 "s3 recognitions (ledger ms)": matched_s3, "terminal": terminal.get("outcome"),
                                                 "executed_steps": terminal.get("executed_steps")}))
        check("FIX.popup_absent", starts == [[0, "step_01_click", "step_01_start"]] and finishes == [[0, "step_03_done"]]
              and len(kinds(absent, "effect_intent")) == 1 and len(matched_s3) >= 2 and terminal.get("outcome") == "success"
              and terminal.get("executed_steps") == 1 and suffix(terminal.get("final_page")) == "step_03_done", "")

    guarded("FIX", section_fixture)

    def section_equal():
        say("EQ", "recordings without optional steps: the same package bytes from L4 (a4cf27ef) and this build")

        def compare(name, s):
            # Both builds stop byte-identical copies of the recording state; frames and crops are
            # read from the recording's own paths.
            digests = {}
            for build, build_exe in (("l4", l4_exe), ("new", exe)):
                code, value, _ = s.stop(f"stop --dry-run ({build})", ["--dry-run"], exe=build_exe, limit=600)
                digests[f"{build}_dry"] = lab_of(value).get("digest")
            results = {}
            os.makedirs(os.path.join(out, "eq", name))
            for build, build_exe in (("l4", l4_exe), ("new", exe)):
                copy = s.state + "-" + build
                shutil.copytree(s.state, copy)
                target = os.path.join(out, "eq", name, build)
                code, value, _ = s.stop(f"stop --lab-dir ({build} on a copy)", ["--lab-dir", target], exe=build_exe, state=copy, limit=600)
                lab_data = lab_of(value)
                path = lab_data.get("lab_dir_path") or ""
                results[build] = (code, lab_data.get("digest"), lab_data.get("sha256"), file_sha(path) if os.path.isfile(path) else None,
                                  sorted(lab_data))
            say("EQ", name, "digests", json.dumps(digests), "l4", json.dumps(results["l4"][:4]), "new", json.dumps(results["new"][:4]),
                "output keys only in new", json.dumps(sorted(set(results["new"][4]) - set(results["l4"][4]))),
                "only in l4", json.dumps(sorted(set(results["l4"][4]) - set(results["new"][4]))))
            check(f"EQ.{name}", results["l4"][0] == 0 and results["new"][0] == 0 and results["l4"][1] == results["new"][1]
                  and results["l4"][2] == results["new"][2] and results["l4"][3] == results["new"][3] == results["new"][2]
                  and digests["l4_dry"] == digests["new_dry"] == results["new"][1], "")

        s = Session(exe, work, "eq_window", "battle_auto")
        s.mark("mark auto_off", ["--frame", f("off"), "--page", "auto_off", "--template", tpl("auto_off.png", "ui/auto_off", sizes, 1180, 664),
                                 "--template", tpl("battle_cost_label.png", "ui/battle_cost", sizes, 778, 649),
                                 "--color", "state/auto_off=1173,680,1,1", "--click-from", "ui/auto_off"])
        s.mark("window 1000-3000", ["--step", "1", "--transition", "window", "--min-ms", "1000", "--max-ms", "3000"])
        s.mark("mark auto_on", ["--frame", f("on"), "--page", "auto_on", "--template", tpl("auto_on.png", "ui/auto_on", sizes, 1180, 664),
                                "--color", "state/auto_on=1173,680,1,1"])
        compare("zip_window", s)

        s = Session(exe, work, "eq_transition", "battle_auto")
        s.mark("mark auto_off", ["--frame", f("off"), "--template", tpl("auto_off.png", "ui/auto_off", sizes, 1180, 664),
                                 "--color", "state/auto_off=1173,680,1,1", "--click-from", "ui/auto_off", "--click-retry", "2"])
        s.mark("mark loading", ["--frame", f("loading"), "--color", "load/bar=600,358,8,4"])
        s.mark("to-transition 2", ["--to-transition", "2"])
        s.mark("mark auto_on", ["--frame", f("on"), "--page", "transition", "--template", tpl("auto_on.png", "ui/auto_on", sizes, 1180, 664),
                                "--color", "state/auto_on=1173,680,1,1"])
        compare("zip_page_transition_retry", s)

        s = Session(exe, work, "eq_json", "color_steps")
        s.request("mark color and digest", {"frame": f("off"), "page": "dim", "add": [
            {"id": "state/dim", "family": "color", "region": {"x": 1173, "y": 680, "width": 1, "height": 1}},
            {"id": "hud/strip", "family": "color_digest", "region": {"x": 0, "y": 640, "width": 1280, "height": 80},
             "columns": 16, "rows": 2, "max_mean_milli": 1500}],
            "click": {"region": {"x": 600, "y": 300, "width": 80, "height": 40}}})
        s.mark("mark lit", ["--frame", f("on"), "--page", "lit", "--color", "state/lit=1173,680,1,1"])
        compare("json_color_digest", s)

        s = Session(exe, work, "eq_restart", "cold_start")
        s.mark("declare restart (entry step)", ["--application", "restart"])
        s.mark("mark title + click", ["--frame", f("off"), "--page", "title", "--template", tpl("auto_off.png", "ui/title", sizes, 1180, 664),
                                      "--color", "state/title=1173,680,1,1", "--click-from", "ui/title"])
        s.mark("mark home", ["--frame", f("on"), "--page", "home", "--template", tpl("auto_on.png", "ui/home", sizes, 1180, 664),
                             "--color", "state/home=1173,680,1,1"])
        compare("zip_restart_title_home", s)

        s = Session(exe, work, "eq_arrival", "popup_close")
        s.mark("mark pop-up", ["--frame", f("popup"), "--page", "notice", "--template", tpl("battle_cost_label.png", "notice/close", sizes, 600, 480),
                               "--color", "notice/panel=320,180,8,8", "--click-from", "notice/close"])
        s.mark("mark home (template only)", ["--frame", f("on"), "--page", "home", "--template", tpl("auto_on.png", "ui/auto_on", sizes, 1180, 664)])
        compare("zip_arrival_unconfirmed", s)

        s = Session(exe, work, "eq_ocr", "ocr_steps")
        ocr = {"languages": ["ja"], "timeout_ms": 1000, "match_mode": "contains", "expected": ["AUTO"], "case_sensitive": False,
               "minimum_confidence": 0.8, "model_ref": "PP-OCRv6_medium", "model_sha256": "0" * 64}
        w, h = sizes["auto_off.png"]
        s.request("mark step 1 (OCR + color)", {"frame": f("off"), "page": "start", "add": [
            {"id": "text/start", "family": "ocr", "region": {"x": 1180, "y": 664, "width": w, "height": h}, **ocr},
            {"id": "state/start", "family": "color", "region": {"x": 1173, "y": 680, "width": 1, "height": 1}}],
            "click": {"from": "text/start"}})
        s.mark("mark step 2", ["--frame", f("on"), "--page", "done", "--color", "state/done=1173,680,1,1"])
        compare("json_ocr", s)

        s = Session(exe, work, "eq_ba", "notice_plain")
        news(s)
        s.mark("mark home", ["--frame", f("home"), "--page", "home", "--template", tpl("home_cafe_label.png", "hud/cafe", sizes),
                             "--template", tpl("home_work_label.png", "hud/work", sizes), "--color", "hud/bright=570,25,4,4"])
        compare("zip_ba_news_home", s)

    guarded("EQ", section_equal)

    say("RESULT", "failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


if __name__ == "__main__":
    command = sys.argv[1]
    if command == "prepare":
        sys.exit(prepare(sys.argv[2], sys.argv[3]))
    sys.exit(run(*sys.argv[2:7]))
