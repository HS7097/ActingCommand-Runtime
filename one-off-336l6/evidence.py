# One-off (to be reverted), Workflow #336 L6 evidence: failure identity, rerun, suspension,
# lifting and `actingd suspended` (R16-R20 amendment 5955585460 "L6" items 1-10, rulings R21/R22,
# R24 L6 additions, R25). Every printed line starts with "L6|". Usage:
#   evidence.py static <repo> <merge base> <product sha>
#   evidence.py prepare <work>
#   evidence.py frames <frames dir>
#   evidence.py frames-check <frames dir> <cargo test log>
#   evidence.py run <work> <new runtime> <new tools> <old runtime> <old tools> <catalog dir>
import hashlib
import json
import os
import random
import re
import shutil
import subprocess
import sys
import time

from PIL import Image, ImageDraw, ImageStat

SCHEMA_DIR = "actingcommand.package.content-directory.v1"
GAME = "fixture-game-a"
SERVER = "fixture-server-a"
ALIAS = "node.a"
TASK = "fixture.observe"
INSTANCE_ID = "instance_00000000000000000000000000000336"
W, H = 64, 36
BG = (32, 32, 32)
COLORS = {
    "x": (200, 40, 40),
    "home": (200, 40, 200),
    "a2": (40, 200, 40),
    "a3": (40, 200, 200),
}
RESOURCES = {"schema_version": "1.0", "resources": [], "resource_count": 0}
FAILURES = []

ID_AP = "fixture.l6.ap"
ID_H = "fixture.l6.h"
ID_U = "fixture.l6.u"
ID_PGF = "fixture.l6.pgf"
DEFAULT_ON_FAILURE = {"action": "pause", "retry_limit": 1, "retry_backoff_ms": 1000, "escalation_threshold": 2}
IDENTITY = re.compile(r"^(?P<base>[^~]+)~v1~k(?P<k>[0-9a-f]{12})~m(?P<m>[0-9a-f]{12})"
                      r"(?P<layers>(~p[0-9a-f]{8}([0-9a-f]{12}|x)){0,3}(~r[0-9a-f]{16}([0-9a-f]{12}|x))?)"
                      r"~f(?P<f>[0-9a-f]{12}|na|u[0-9a-f]{12})$")


def say(*parts):
    print("L6|" + "|".join(str(part) for part in parts), flush=True)


def check(label, condition, detail=""):
    say("CHECK", label, "PASS" if condition else "FAIL", detail)
    if not condition:
        FAILURES.append(label)


def short(text, limit=900):
    text = str(text).strip().replace("\r", "").replace("\n", " ")
    return text if len(text) <= limit else text[:limit] + "..."


def pretty(value):
    return (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode("utf-8")


def digest(files):
    hasher = hashlib.sha256()
    hasher.update((SCHEMA_DIR + "\n").encode())
    for path in sorted(files, key=lambda item: item.encode("utf-8")):
        hasher.update(f"{hashlib.sha256(files[path]).hexdigest()}  {path}\n".encode("utf-8"))
    return hasher.hexdigest()


def write_tree(root, files):
    for path, data in files.items():
        target = os.path.join(root, *path.split("/"))
        os.makedirs(os.path.dirname(target), exist_ok=True)
        with open(target, "xb") as handle:
            handle.write(data)


# ---------------------------------------------------------------------------------------------
# Frames (64x36): a state shows its color at the probe pixel (10, 10); the error frames show none.

def make_frame(name):
    if name in COLORS:
        image = Image.new("RGB", (W, H), BG)
        for y in range(9, 12):
            for x in range(9, 12):
                image.putpixel((x, y), COLORS[name])
        return image
    if name == "y":
        return Image.new("RGB", (W, H), (40, 40, 180))
    if name == "y2":
        # y plus a block of 12x10 pixels (about 5% of the frame), away from the probe.
        image = Image.new("RGB", (W, H), (40, 40, 180))
        for y in range(20, 30):
            for x in range(44, 56):
                image.putpixel((x, y), (240, 240, 40))
        return image
    if name == "z":
        return Image.new("RGB", (W, H), (120, 120, 120))
    raise ValueError(name)


FRAME_NAMES = ("x", "home", "a2", "a3", "y", "y2", "z")


def probe(state):
    return {"id": f"state/{state}", "region": {"mode": "rect", "rect": {"x": 10, "y": 10, "width": 1, "height": 1}},
            "expected": list(COLORS[state])}


def state_of(page):
    return page.rsplit("_", 1)[-1]


def guard(page):
    return {"page_id": page, "target_id": f"state/{state_of(page)}",
            "expected_rect": {"x": 10, "y": 10, "width": 1, "height": 1}, "color_probe": f"state/{state_of(page)}"}


def source_pack(task_id, package_id, spec, mode="linear_steps", distance=20):
    """spec: (from, to) per click operation."""
    pages = []
    for frm, to in spec:
        for page in (frm, to):
            if page not in pages:
                pages.append(page)
    states = sorted({state_of(page) for page in pages})
    operations = []
    for index, (frm, to) in enumerate(spec):
        operations.append({
            "id": f"step_{index + 1:02d}_click",
            "purpose": f"Suspension fixture step {index + 1}",
            "from": frm,
            "to": to,
            "expect_after": {"page_id": to, "timeout_ms": 500, "interval_ms": 250},
            "post_delay_ms": 50,
            "click": {"kind": "rect", "x": 8, "y": 8, "width": 5, "height": 5},
            "guard": guard(frm),
        })
    task = {
        "schema_version": "0.9",
        "task_id": task_id,
        "game": GAME,
        "server_scope": [SERVER],
        "locale": "en-US",
        "goal": f"Suspension fixture {task_id}",
        "coordinate_space": {"width": W, "height": H},
        "defaults": {"template_threshold": 0.95, "color_max_distance": distance, "match_metric": "ccoeff_normed"},
        "timeout_ms": 30000,
        "max_steps": len(operations),
        "entry_page": spec[0][0],
        "target_page": spec[-1][1],
        "color_probes": [probe(state) for state in states],
        "page_rules": {page: {"required": [f"state/{state_of(page)}"]} for page in pages},
        "operations": operations,
        "scheduling_outcome": {"mappings": [{
            "outcome_key": f"{task_id}_done", "effect": "no_designated_effect", "terminal_pages": [spec[-1][1]]}]},
    }
    control = {
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": mode,
        "game": GAME,
        "server": SERVER,
        "resolution": {"width": W, "height": H},
        "entry_task_id": task_id,
        "timeout_ms": 30000,
        "step_timeout_ms": 500,
        "capture_interval_ms": 250,
        "max_steps": len(operations),
    }
    return {
        "control.json": pretty(control),
        "resources/operations/resources.json": pretty(RESOURCES),
        f"resources/operations/{task_id}/task.json": pretty(task),
    }


AP_SPEC = [("home", "step_02_a2"), ("step_02_a2", "step_03_a3")]
H_SPEC = [("x", "home")]


def packs():
    return {
        # A': HOME -> A2 -> A3, two clicks, no prerequisite package.
        "ap": source_pack("sus_ap", ID_AP, AP_SPEC),
        # The same recording with one threshold changed: another digest.
        "ap2": source_pack("sus_ap", ID_AP, AP_SPEC, distance=21),
        # H: the page-graph return-home package X -> HOME, and its updated version.
        "h": source_pack("sus_h", ID_H, H_SPEC, mode="navigable_route"),
        "h2": source_pack("sus_h", ID_H, H_SPEC, mode="navigable_route", distance=21),
        # An unrelated mapped package.
        "u": source_pack("sus_u", ID_U, H_SPEC, mode="navigable_route"),
        # A page-graph task failing at its second step, and its updated version.
        "pgf": source_pack("sus_pgf", ID_PGF, AP_SPEC, mode="navigable_route"),
        "pgf2": source_pack("sus_pgf", ID_PGF, AP_SPEC, mode="navigable_route", distance=21),
    }


def load_state(work):
    with open(os.path.join(work, "state.json"), encoding="utf-8") as handle:
        return json.load(handle)


def save_state(work, state):
    with open(os.path.join(work, "state.json"), "w", encoding="utf-8") as handle:
        json.dump(state, handle, indent=2)


def git(repo, *args):
    return subprocess.run(["git", "-C", repo, *args], capture_output=True, check=True).stdout.decode("utf-8")


# ---------------------------------------------------------------------------------------------
# static: what the product changes and what it leaves alone.

def static(repo, base, product):
    say("STATIC", "merge base (first commit)", base[:8], "product", product[:8])
    for line in git(repo, "diff", "--stat", base, product).splitlines():
        say("STATIC", "diff --stat", line)
    for path in ("crates/actingcommand-contract", "crates/ledger", "crates/ledger-forensics", "apps/actingctl",
                 "crates/lab", "crates/recognition", "apps/actingd/src/config.rs", "apps/actingd/src/config",
                 "apps/actingd/src/check_config.rs", "crates/runtime-host/src/host/recovery_ladder.rs",
                 "crates/runtime-host/src/host/startup_package.rs"):
        stat = git(repo, "diff", "--stat", base, product, "--", path).strip()
        check(f"STATIC.unchanged.{path}", not stat, stat)
    for line in git(repo, "diff", base, product, "--", "crates/runtime-host/src/policy_control.rs").splitlines():
        if line.startswith(("+", "-")) and not line.startswith(("+++", "---")):
            say("STATIC", "PC diff", line)
    pc = git(repo, "show", f"{product}:crates/runtime-host/src/policy_control.rs")
    pc_base = git(repo, "show", f"{base}:crates/runtime-host/src/policy_control.rs")
    start = pc.index("    pub(crate) fn preview_execution(")
    start_base = pc_base.index("    pub(crate) fn preview_execution(")
    end = pc.index("    pub(crate) fn commit_execution(")
    end_base = pc_base.index("    pub(crate) fn commit_execution(")
    check("STATIC.preview_execution_count_unchanged", pc[start:end] == pc_base[start_base:end_base], "")
    say("RESULT", "static failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


# ---------------------------------------------------------------------------------------------
# prepare: packages and frames.

def prepare(work):
    os.makedirs(work, exist_ok=False)
    state = {"packs": {}, "frames": {}}
    for name, files in packs().items():
        sha = digest(files)
        path = os.path.join(work, "packs", name, sha)
        write_tree(path, files)
        state["packs"][name] = {"digest": sha, "path": path}
        say("prepare", "pack", name, "digest", sha)
        for file_path in sorted(files):
            if file_path.endswith("task.json") or file_path == "control.json":
                say("prepare", "pack file", name, file_path, short(files[file_path].decode("utf-8"), 3000))
    for name in FRAME_NAMES:
        path = os.path.join(work, "frames", name + ".png")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        make_frame(name).save(path, format="PNG")
        state["frames"][name] = path
    save_state(work, state)
    say("prepare", "done", work)
    return 0


# ---------------------------------------------------------------------------------------------
# frames: 1280x720 synthesized cases for compare_failure_frames and an exact replica of it.

def noise_layer(rng, columns, rows, size, amplitude):
    small = Image.new("RGB", (columns, rows))
    small.putdata([tuple(rng.randrange(256) for _ in range(3)) for _ in range(columns * rows)])
    layer = small.resize(size, getattr(Image, "Resampling", Image).BICUBIC)
    return Image.eval(layer, lambda value: int(128 + (value - 128) * amplitude))


def scene(seed, moved=0, dark=False, bar=False, dialog=None, sprites_seed=7):
    rng = random.Random(seed)
    base = noise_layer(rng, 8, 5, (1280, 720), 1.0)
    detail = noise_layer(rng, 160, 90, (1280, 720), 0.35)
    image = Image.blend(base, detail, 0.35)
    draw = ImageDraw.Draw(image)
    draw.rectangle((0, 0, 1279, 59), fill=(20, 24, 40))
    for index in range(6):
        x = 40 + index * 205
        draw.rectangle((x, 620, x + 160, 700), fill=(60 + index * 25, 90, 160 - index * 15), outline=(240, 240, 240))
    sprite_rng = random.Random(sprites_seed)
    for index in range(8):
        sprite = noise_layer(sprite_rng, 6, 8, (90, 120), 0.8)
        mask = Image.new("L", (90, 120), 0)
        ImageDraw.Draw(mask).ellipse((5, 0, 85, 119), fill=255)
        x = 80 + index * 140 + (60 if index < moved else 0)
        image.paste(sprite, (x, 300), mask)
    if dark:
        image = Image.eval(image, lambda value: int(value * 0.45))
        draw = ImageDraw.Draw(image)
    if bar:
        draw.rectangle((340, 70, 939, 129), fill=(236, 236, 230))
        for index in range(18):
            draw.rectangle((360 + index * 31, 92, 380 + index * 31, 106), fill=(40, 40, 40))
    if dialog:
        width, height = dialog
        left, top = (1280 - width) // 2, (720 - height) // 2
        draw.rectangle((left, top, left + width - 1, top + height - 1), fill=(236, 232, 222), outline=(70, 90, 140), width=4)
        draw.rectangle((left + width // 2 - 70, top + height - 60, left + width // 2 + 70, top + height - 20), fill=(70, 160, 230))
    return image


def frame_cases():
    base = scene(1)
    return [
        ("same frame", base, scene(1)),
        ("3 sprites moved", base, scene(1, moved=3)),
        ("5 sprites moved", base, scene(1, moved=5)),
        ("8 sprites moved", base, scene(1, moved=8)),
        ("600x60 prompt bar", base, scene(1, bar=True)),
        ("400x220 dialog, not darkened", base, scene(1, dialog=(400, 220))),
        ("560x320 dialog, not darkened", base, scene(1, dialog=(560, 320))),
        ("darkened x0.45 and 560x320 dialog", base, scene(1, dark=True, dialog=(560, 320))),
        ("another page", base, scene(2, sprites_seed=9)),
        ("black frame", base, Image.new("RGB", (1280, 720), (0, 0, 0))),
    ]


def replica(previous, current):
    """compare_failure_frames: color_digest.v1 on min(32, w) x min(18, h) cells, quantized
    channel means, a cell changed when |dR|+|dG|+|dB| > 6, different above 250 per mille."""
    if previous.size != current.size:
        return ("different", "size_changed", None, None)
    width, height = previous.size
    columns, rows = min(32, width), min(18, height)

    def digest_cells(image):
        cells = []
        for row in range(rows):
            y0, y1 = row * height // rows, (row + 1) * height // rows
            for column in range(columns):
                x0, x1 = column * width // columns, (column + 1) * width // columns
                sums = [int(round(value)) for value in ImageStat.Stat(image.crop((x0, y0, x1, y1))).sum[:3]]
                divisor = 8 * (x1 - x0) * (y1 - y0)
                cells.append(tuple(value // divisor for value in sums))
        return cells

    left, right = digest_cells(previous), digest_cells(current)
    deltas = [sum(abs(a - b) for a, b in zip(cell_a, cell_b)) for cell_a, cell_b in zip(left, right)]
    changed = sum(1 for delta in deltas if delta > 6)
    milli = changed * 1000 // len(deltas)
    mean = 1000 * sum(deltas) // len(deltas)
    return ("different" if milli > 250 else "similar", "digest_cells_changed" if milli > 250 else None, milli, mean)


def frames(directory):
    os.makedirs(directory, exist_ok=False)
    lines, expected = [], {}
    for index, (name, previous, current) in enumerate(frame_cases()):
        previous_name, current_name = f"case{index:02d}a.png", f"case{index:02d}b.png"
        previous.save(os.path.join(directory, previous_name), format="PNG")
        current.save(os.path.join(directory, current_name), format="PNG")
        lines.append(f"{name}|{previous_name}|{current_name}")
        expected[name] = replica(Image.open(os.path.join(directory, previous_name)).convert("RGB"),
                                 Image.open(os.path.join(directory, current_name)).convert("RGB"))
        say("FRAMES", "replica", name, json.dumps(expected[name]))
    small_a, small_b = Image.new("RGB", (64, 36), (10, 10, 10)), Image.new("RGB", (32, 36), (10, 10, 10))
    small_a.save(os.path.join(directory, "size_a.png"), format="PNG")
    small_b.save(os.path.join(directory, "size_b.png"), format="PNG")
    lines.append("frames of another size|size_a.png|size_b.png")
    expected["frames of another size"] = ("different", "size_changed", None, None)
    with open(os.path.join(directory, "cases.txt"), "w", encoding="utf-8") as handle:
        handle.write("\n".join(lines) + "\n")
    with open(os.path.join(directory, "replica.json"), "w", encoding="utf-8") as handle:
        json.dump(expected, handle)
    return 0


def frames_check(directory, log):
    with open(os.path.join(directory, "replica.json"), encoding="utf-8") as handle:
        expected = json.load(handle)
    measured = {}
    with open(log, encoding="utf-8", errors="replace") as handle:
        for line in handle:
            if "FRAMES|" not in line:
                continue
            parts = line[line.index("FRAMES|"):].strip().split("|")
            say("FRAMES", "rust", *parts[1:])
            if len(parts) == 7:
                measured[parts[1]] = parts[2:]
    for name, (verdict, reason, milli, mean) in expected.items():
        got = measured.get(name)
        same = got is not None and got[0] == verdict and got[1] == (reason or "-") \
            and got[2] == ("-" if milli is None else str(milli)) and got[3] == ("-" if mean is None else str(mean))
        check(f"FRAMES.rust_equals_replica.{name.replace(' ', '_')}", same,
              json.dumps({"rust": got, "replica": [verdict, reason, milli, mean]}))
    wanted = {"same frame": "similar", "3 sprites moved": "similar", "5 sprites moved": "similar",
              "600x60 prompt bar": "similar", "darkened x0.45 and 560x320 dialog": "different",
              "another page": "different", "black frame": "different", "frames of another size": "different"}
    for name, verdict in wanted.items():
        got = measured.get(name)
        check(f"FRAMES.expected_verdict.{name.replace(' ', '_')}", got is not None and got[0] == verdict,
              json.dumps({"expected": verdict, "measured": got}))
    say("FRAMES", "reported only", json.dumps({name: measured.get(name) for name in
                                               ("8 sprites moved", "400x220 dialog, not darkened",
                                                "560x320 dialog, not darkened")}))
    say("RESULT", "frames failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


# ---------------------------------------------------------------------------------------------
# run: scheduled fixture dispatches.

def run_exe(args, timeout=300, cwd=None):
    try:
        result = subprocess.run(args, capture_output=True, timeout=timeout, cwd=cwd)
    except subprocess.TimeoutExpired:
        return None, "", "timeout"
    return result.returncode, result.stdout.decode("utf-8", "replace"), result.stderr.decode("utf-8", "replace")


def catalog_documents(catalog_dir, on_failure, sensitive=False, window_limit=50):
    def load(name):
        with open(os.path.join(catalog_dir, name + ".json"), encoding="utf-8") as handle:
            return json.load(handle)

    scope = {"kind": "instance", "instance_id": ALIAS}
    tasks, pools, activity, timeline = (load(name) for name in ("tasks", "pools", "activity", "timeline"))
    task = tasks["tasks"][0]
    task["scope"] = scope
    # A clock every minute and a 30 s cooldown: an immediate rerun (R22) shows as a second
    # dispatch seconds after the first.
    task["trigger"]["predicates"][0]["schedule"]["every_ms"] = 60000
    task["trigger"]["predicates"][1]["scope"] = scope
    task["feedback_stop"] = {"kind": "clock", "schedule": {"kind": "at", "clock_source": {
        "kind": "server", "timezone_id": "etc/utc", "utc_offset_minutes": 0, "dst_offset_minutes": 0,
        "maintenance_drift_ms": 0}, "at_ms": 4102444800000}}
    task["instance_overrides"] = []
    task["on_failure"] = on_failure
    task["sensitive"] = sensitive
    task["cooldown_ms"] = 30000
    task["expected_duration_ms"] = 1000
    task["loop_budget"] = {"daily_limit": 50, "window_iteration_limit": window_limit, "max_runtime_ms": 3600000}
    pools["pools"][0]["scope"] = scope
    profile = activity["profiles"][0]
    profile["scope"] = scope
    profile["windows"][0]["start_minute_of_day"] = 0
    profile["windows"][0]["end_minute_of_day"] = 0
    profile["minimum_interval_ms"] = 1
    profile["maximum_interval_ms"] = 1
    profile["daily_budget"] = 50
    profile["max_window_iterations"] = 50
    return {"tasks": tasks, "pools": pools, "activity": activity, "timeline": timeline}


def prerequisite_entries(state, mapping):
    """mapping: list of (package id, pack name for the path, pack name for the digest)."""
    return [{"package_id": package_id, "package_path": state["packs"][path_pack]["path"],
             "package_digest": {"schema_version": SCHEMA_DIR, "sha256": state["packs"][digest_pack]["digest"]}}
            for package_id, path_pack, digest_pack in mapping]


def write_config(path, state, case):
    """case: main, frames, mapping (None or list), return_home (None or package id), on_failure,
    sensitive, window_limit."""
    config_dir = os.path.dirname(path)
    os.makedirs(os.path.join(config_dir, "policy"), exist_ok=True)
    for name, document in catalog_documents(case["catalog_dir"], case.get("on_failure", DEFAULT_ON_FAILURE),
                                            case.get("sensitive", False), case.get("window_limit", 50)).items():
        with open(os.path.join(config_dir, "policy", name + ".json"), "w", encoding="utf-8") as handle:
            json.dump(document, handle, indent=2)
    now = int(time.time() * 1000)
    main = case["main"]
    config = {
        "schema_version": "actingcommand.actingd.config.v1",
        "state_root": case["state_root"],
        "bind_host": "127.0.0.1",
        "bind_port": 0,
        "secret_fingerprint_salt": "oneoff-336-l6-fixture-salt-value",
        "policy": {
            "facts": {
                "ledger_position": 0, "fact_snapshot_id": "snapshot:oneoff-336-l6",
                "facts": [], "outcomes": [], "tasks": [],
                "instances": [{"instance_id": ALIAS, "server_id": SERVER, "game_id": GAME,
                               "host_id": "fixture-host-a", "available": case.get("available", True),
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
                "package_digest": {"schema_version": SCHEMA_DIR, "sha256": state["packs"][main]["digest"]},
                "operation_id": "operation.observe",
                "yield_points": ["after_observation"],
                "scheduled_execution": {"mode": "fixture_simulation", "package_path": state["packs"][main]["path"]},
            }],
        },
        "instances": [{
            "alias": ALIAS,
            "instance_id": INSTANCE_ID,
            "fixture_backend": {
                "frames": [{"width": W, "height": H, "rgb": list(Image.open(state["frames"][frame]).convert("RGB").tobytes())}
                           for frame in case["frames"]],
                "max_inputs": 32,
            },
        }],
    }
    if case.get("mapping") is not None:
        config["prerequisite_packages"] = prerequisite_entries(state, case["mapping"])
    if case.get("return_home") is not None:
        config["return_home_packages"] = [{"game": GAME, "server": SERVER, "package_id": case["return_home"]}]
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(config, handle, separators=(",", ":"))
    return path


def wait_for_fresh_minute():
    """Starts early in a minute that no earlier daemon dispatched in."""
    time.sleep(62 - time.time() % 60)


def daemon(label, runtime_dir, config, state_root, settle_s, during=None, align=True, after=None):
    actingd = os.path.join(runtime_dir, "actingcommand-actingd.exe")
    actingctl = os.path.join(runtime_dir, "actingctl.exe")
    run_dir = os.path.dirname(os.path.dirname(config))
    if align:
        wait_for_fresh_minute()
    stamp = str(int(time.time() * 1000))
    out_path, err_path = os.path.join(run_dir, f"actingd-{stamp}.out"), os.path.join(run_dir, f"actingd-{stamp}.err")
    with open(out_path, "wb") as out, open(err_path, "wb") as err:
        process = subprocess.Popen([actingd, "--config", config], stdout=out, stderr=err, cwd=os.path.dirname(config))
        started = time.time()
        ready = False
        while time.time() - started < 60 and process.poll() is None:
            code, _, _ = run_exe([actingctl, "status", "--state-root", state_root], timeout=30)
            if code == 0:
                ready = True
                break
            time.sleep(0.5)
        say(label, "daemon ready", ready, "after_s", round(time.time() - started, 1))
        result = None
        if ready:
            if during is not None:
                result = during()
            time.sleep(settle_s)
            if after is not None:
                code, out, err = run_exe([actingctl, "status", "--state-root", state_root], timeout=60)
                say(label, "actingctl status exit", code, short(out, 6000), short(err, 300))
                say(label, "status mentions policy_task_paused", "policy_task_paused" in out)
        code, _, shutdown_err = run_exe([actingctl, "request-shutdown", "--state-root", state_root, "--wait", "60"], timeout=120)
        say(label, "request-shutdown exit", code, short(shutdown_err, 300))
        try:
            exit_code = process.wait(timeout=120)
        except subprocess.TimeoutExpired:
            process.kill()
            exit_code = "killed"
    say(label, "actingd exit", exit_code)
    with open(out_path, "rb") as handle:
        out_text = handle.read().decode("utf-8", "replace")
    with open(err_path, "rb") as handle:
        err_text = handle.read().decode("utf-8", "replace")
    if exit_code != 0:
        say(label, "actingd out", short(out_text, 1500))
        say(label, "actingd err", short(err_text, 1500))
    return {"ready": ready, "exit": exit_code, "out": out_text, "err": err_text, "during": result}


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


def inner(event):
    payload = event.get("payload") or {}
    value = payload.get("payload") if isinstance(payload, dict) else None
    return (value or {}).get("kind"), (value or {}).get("data") or {}


def fact_of(event):
    _, data = inner(event)
    fact = data.get("fact") if isinstance(data, dict) else None
    return fact if isinstance(fact, dict) and "kind" in fact else None


def executions(events):
    found = []
    for event in events:
        if event.get("event_type") != "policy.execution_recorded":
            continue
        _, data = inner(event)
        outcome = data.get("outcome") or {}
        found.append({"sequence": event["sequence"], "severity": event.get("severity"),
                      "decision_id": data.get("decision_id"), "observed_at": data.get("observed_at_unix_ms"),
                      "kind": outcome.get("kind"), "failure": outcome.get("failure") or {}})
    return found


def intents(events):
    found = []
    for event in events:
        if event.get("event_type") != "policy.dispatch_intent":
            continue
        _, data = inner(event)
        found.append({"sequence": event["sequence"], "timestamp": event.get("timestamp_unix_ms"),
                      "decision_id": data.get("decision_id"),
                      "reasons": [reason.get("code") for reason in data.get("reasons") or []],
                      "run_id": (event.get("links") or {}).get("run_id")})
    return found


def run_facts(events, run_id):
    return [(event["sequence"], fact_of(event)) for event in events
            if fact_of(event) is not None and (event.get("links") or {}).get("run_id") == run_id]


def rows(facts):
    out = []
    for _, fact in facts:
        kind = fact["kind"]
        if kind in ("entry_recognition",):
            out.append([kind, fact.get("phase"), fact.get("matched")])
        elif kind in ("entry_recovery_package_admitted", "entry_recovery_completed", "entry_recovery_failed"):
            sha = fact.get("package_sha256")
            out.append([kind, sha.get("sha256") if isinstance(sha, dict) else sha])
        elif kind == "entry_target_disposition":
            out.append([kind, fact.get("disposition"), fact.get("failure_code")])
        elif kind in ("step_started", "step_finished"):
            out.append([kind, fact.get("step_index"), fact.get("operation_label"), fact.get("page_label")])
        elif kind == "terminal_committed":
            out.append([kind, fact.get("outcome"), fact.get("failure_code")])
    return out


def describe_executions(label, events):
    found = executions(events)
    for index, execution in enumerate(found):
        failure = execution["failure"]
        say(label, "execution", index + 1, execution["sequence"], execution["severity"], execution["kind"],
            failure.get("error_code"), failure.get("original_class"), failure.get("effective_class"),
            failure.get("consecutive_same_error"), failure.get("escalation_streak"), failure.get("disposition"))
    for index, intent in enumerate(intents(events)):
        say(label, "dispatch intent", index + 1, intent["sequence"], intent["timestamp"], intent["run_id"],
            json.dumps(intent["reasons"]))
        for row in rows(run_facts(events, intent["run_id"])):
            say(label, "run", index + 1, json.dumps(row))
    for event in events:
        if event.get("event_type") == "runtime.failed":
            text = json.dumps(event.get("payload"), ensure_ascii=False)
            if "policy_failure" in text or "native_detail" in text:
                say(label, "runtime.failed", event["sequence"], event.get("severity"), short(text, 1500))
    return found


def frame_artifact_shas(events, run_id):
    """The capture frame artifact SHA-256 of a run's latest capture.completed frame."""
    captures = [event for event in events if event.get("event_type") == "capture.completed"
                and (event.get("links") or {}).get("run_id") == run_id]
    if not captures:
        return None
    frame_id = (captures[-1].get("links") or {}).get("frame_id")
    for event in events:
        if event.get("event_type") == "artifact.verified" and (event.get("links") or {}).get("frame_id") == frame_id \
                and (event.get("links") or {}).get("run_id") == run_id:
            for artifact in event.get("artifacts") or []:
                if artifact.get("kind") == "capture_frame":
                    return str(artifact.get("sha256")).split(":")[-1]
    return None


def identity(code):
    match = IDENTITY.match(code or "")
    return match.groupdict() if match else None


def prefix(code):
    return code.rsplit("~f", 1)[0] if code and "~f" in code else None


def suspended(label, runtime_dir, config):
    actingd = os.path.join(runtime_dir, "actingcommand-actingd.exe")
    code, out, err = run_exe([actingd, "suspended", "--config", config], timeout=120)
    say(label, "actingd suspended exit", code, "stderr", short(err, 400))
    say(label, "actingd suspended stdout", short(out, 12000))
    try:
        report = json.loads(out) if out.strip() else None
    except ValueError:
        report = None
    return code, report


class Runner:
    def __init__(self, work, new_runtime, new_tools, old_runtime, old_tools, catalog_dir):
        self.work = work
        self.state = load_state(work)
        self.new_runtime, self.old_runtime = new_runtime, old_runtime
        self.new_ledger = os.path.join(new_tools, "actingledger.exe")
        self.old_ledger = os.path.join(old_tools, "actingledger.exe")
        self.catalog_dir = catalog_dir
        self.roots = []

    def digest_of(self, name):
        return self.state["packs"][name]["digest"]

    def config(self, label, state_root, **case):
        case.setdefault("catalog_dir", self.catalog_dir)
        case["state_root"] = state_root
        path = os.path.join(self.work, "configs", re.sub(r"[^A-Za-z0-9_.-]", "_", label), "actingd.json")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        write_config(path, self.state, case)
        say(label, "config", "main", case["main"], self.digest_of(case["main"]), "frames", json.dumps(case["frames"]),
            "mapping", json.dumps(case.get("mapping")), "return_home", case.get("return_home"),
            "on_failure", json.dumps(case.get("on_failure", DEFAULT_ON_FAILURE)), "sensitive", case.get("sensitive", False))
        return path

    def new_root(self, label):
        root = os.path.join(self.work, "roots", re.sub(r"[^A-Za-z0-9_.-]", "_", label))
        os.makedirs(root)
        self.roots.append((label, root))
        return root

    def events(self, label, root):
        events, failure = ledger_events(self.new_ledger, root)
        if events is None:
            say(label, "events failed", failure)
            check(f"{label}.ledger_readable", False, failure)
            return []
        return events

    def go(self, label, root, runtime=None, settle_s=25, during=None, align=True, after=None, **case):
        config = self.config(label, root, **case)
        result = daemon(label, runtime or self.new_runtime, config, root, settle_s, during=during, align=align,
                        after=after)
        check(f"{label}.daemon_ready_and_clean_exit", result["ready"] and result["exit"] == 0,
              json.dumps({"ready": result["ready"], "exit": result["exit"]}))
        check(f"{label}.no_record_mismatch", "policy_execution_record_mismatch" not in result["out"] + result["err"], "")
        return config, result


def run(work, new_runtime, new_tools, old_runtime, old_tools, catalog_dir):
    r = Runner(work, new_runtime, new_tools, old_runtime, old_tools, catalog_dir)
    h_map = [(ID_H, "h", "h")]
    home_d1 = ["home", "home", "a2", "x", "x", "x", "x"]
    home_d2 = ["x", "x", "home", "home", "home", "a2", "x", "x", "x", "x"]

    # 6 (state root missing): exit 1, suspended_ledger_unavailable.
    missing = os.path.join(work, "roots", "missing-state-root")
    config = r.config("E6 missing state root", missing, main="ap", frames=["home"], mapping=h_map, return_home=ID_H)
    code, report = suspended("E6 missing state root", new_runtime, config)
    check("E6.missing_state_root", code == 1 and report is not None and report.get("status") == "failed"
          and report.get("code") == "suspended_ledger_unavailable", json.dumps(report))

    # 1. Rerun then suspend, with the return-home package H.
    root1 = r.new_root("E1")
    config1, _ = r.go("E1 rerun then suspend", root1, main="ap", frames=home_d1 + home_d2, mapping=h_map,
                      return_home=ID_H, after=True)
    events1 = r.events("E1", root1)
    found = describe_executions("E1", events1)
    runs = intents(events1)
    first, second = (found + [{}, {}])[:2]
    f1, f2 = first.get("failure") or {}, second.get("failure") or {}
    code1 = f1.get("error_code")
    parsed = identity(code1)
    say("E1", "identity", json.dumps(parsed))
    check("E1.two_dispatches_two_records", len(found) == 2 and len(runs) == 2, f"{len(found)} records {len(runs)} intents")
    check("E1.first_retry_scheduled", parsed is not None and parsed["base"] == "page_confirmation_failed"
          and parsed["layers"].startswith("~r") and len(parsed["f"]) == 12
          and f1.get("consecutive_same_error") == 1 and f1.get("disposition") == "retry_scheduled",
          json.dumps(f1, ensure_ascii=False)[:600])
    sha1 = frame_artifact_shas(events1, runs[0]["run_id"]) if runs else None
    check("E1.F_is_the_first_error_frame", parsed is not None and sha1 is not None and sha1.startswith(parsed["f"]),
          json.dumps({"F": parsed and parsed["f"], "frame sha256": sha1}))
    check("E1.second_same_identity_paused", f2.get("error_code") == code1 and f2.get("consecutive_same_error") == 2
          and f2.get("escalation_streak") == 2 and f2.get("disposition") == "paused_task"
          and f2.get("effective_class") == "severe" and second.get("severity") == "error",
          json.dumps(f2, ensure_ascii=False)[:600])
    if len(runs) == 2 and len(found) == 2:
        gap = runs[1]["timestamp"] - first["observed_at"]
        check("E1.R22_rerun_before_clock_and_cooldown", gap < 15000 and "failure_retry_immediate" in runs[1]["reasons"]
              and runs[1]["timestamp"] // 60000 == runs[0]["timestamp"] // 60000,
              json.dumps({"rerun_after_ms": gap, "reasons": runs[1]["reasons"]}))
        second_rows = rows(run_facts(events1, runs[1]["run_id"]))
        check("E1.second_run_returned_home_first", ["entry_recovery_package_admitted", r.digest_of("h")] in second_rows,
              json.dumps(second_rows))
    code, report1 = suspended("E1 after", new_runtime, config1)
    entries = (report1 or {}).get("suspended") or []
    entry = entries[0] if entries else {}
    comparison = ((entry.get("frames") or {}).get("comparison") or {})
    check("E1.suspended_listed", code == 0 and len(entries) == 1 and entry.get("error_code") == code1
          and (entry.get("step") or {}).get("scope") == "main"
          and (entry.get("step") or {}).get("operation_label") == "step_02_click"
          and "operation=step_02_click" in str(entry.get("detail"))
          and comparison.get("status") == "similar" and comparison.get("changed_cells_milli") == 0
          and [layer.get("kind") for layer in (entry.get("package") or {}).get("layers") or []] == ["return_home"],
          json.dumps(entry)[:1500])
    root1_copy = root1 + "-copy-for-5b"
    shutil.copytree(root1, root1_copy)

    # 2. Different and similar error frames, without a return-home package.
    root2 = r.new_root("E2")
    config2, _ = r.go("E2 frames different then similar", root2, main="ap",
                      frames=["home", "a2", "x", "x", "x", "x", "home", "a2", "y", "y", "y", "y", "home", "a2",
                              "y2", "y2", "y2", "y2"])
    events2 = r.events("E2", root2)
    found = describe_executions("E2", events2)
    failures = [execution.get("failure") or {} for execution in found]
    codes = [failure.get("error_code") for failure in failures]
    check("E2.three_records", len(found) == 3, str(len(found)))
    if len(found) == 3:
        check("E2.second_same_prefix_other_frame_retry", prefix(codes[0]) == prefix(codes[1]) and codes[0] != codes[1]
              and failures[1].get("consecutive_same_error") == 1 and failures[1].get("disposition") == "retry_scheduled",
              json.dumps(codes))
        check("E2.third_equals_second_paused", codes[2] == codes[1] and failures[2].get("consecutive_same_error") == 2
              and failures[2].get("disposition") == "paused_task", json.dumps(codes))
    code, report2 = suspended("E2 after", new_runtime, config2)
    entry = ((report2 or {}).get("suspended") or [{}])[0]
    comparison = ((entry.get("frames") or {}).get("comparison") or {})
    say("E2", "changed_cells_milli", comparison.get("changed_cells_milli"), "digest_mean_milli",
        comparison.get("digest_mean_milli"), "ccoeff", comparison.get("ccoeff"), "ccoeff_error", comparison.get("ccoeff_error"))
    check("E2.suspended_similar", code == 0 and comparison.get("status") == "similar", json.dumps(comparison))

    # 3. H sees an unknown page Z twice: rerun only, never paused, listed as repeating.
    root3 = r.new_root("E3")
    config3 = r.config("E3 before", root3, main="ap", frames=["z"] * 32, mapping=h_map, return_home=ID_H, window_limit=2)
    code, report = suspended("E3 before (empty ledger)", new_runtime, config3)
    check("E3.before.missing_root_or_empty", code in (0, 1), str(code))
    config3, _ = r.go("E3 return-home sees an unknown page", root3, main="ap", frames=["z"] * 32, mapping=h_map,
                      return_home=ID_H, window_limit=2)
    events3 = r.events("E3", root3)
    found = describe_executions("E3", events3)
    failures = [execution.get("failure") or {} for execution in found]
    codes = [failure.get("error_code") for failure in failures]
    check("E3.two_records_rerun_only", len(found) == 2 and all(
        identity(code) is not None and identity(code)["f"].startswith("u") for code in codes)
          and all(failure.get("disposition") == "retry_scheduled" and failure.get("consecutive_same_error") == 1
                  for failure in failures), json.dumps(codes))
    check("E3.same_prefix", len(codes) == 2 and prefix(codes[0]) == prefix(codes[1]), json.dumps(codes))
    check("E3.no_paused_task", not any(failure.get("disposition") == "paused_task" for failure in failures), "")
    code, report3 = suspended("E3 after", new_runtime, config3)
    repeating = (report3 or {}).get("repeating") or []
    check("E3.repeating_listed", code == 0 and len(repeating) == 1 and not (report3 or {}).get("suspended"),
          json.dumps(report3)[:1500])

    # 4. Not comparable: a wrong mapped digest refuses preparation twice.
    root4 = r.new_root("E4")
    wrong = [(ID_H, "h", "h2")]
    config4, _ = r.go("E4 preparation refused twice", root4, main="ap", frames=["home"], mapping=wrong, return_home=ID_H)
    events4 = r.events("E4", root4)
    found = describe_executions("E4", events4)
    failures = [execution.get("failure") or {} for execution in found]
    codes = [failure.get("error_code") for failure in failures]
    check("E4.two_identical_identities_second_paused", len(found) == 2 and codes[0] == codes[1]
          and identity(codes[0]) is not None and identity(codes[0])["base"] == "contained_task_prerequisite_admission_failed"
          and identity(codes[0])["f"] == "na" and failures[1].get("disposition") == "paused_task", json.dumps(codes))
    check("E4.no_compare_unavailable_diagnostic", "policy_failure_frame_compare_unavailable" not in json.dumps(events4), "")
    code, report4 = suspended("E4 after", new_runtime, config4)
    entry = ((report4 or {}).get("suspended") or [{}])[0]
    comparison = ((entry.get("frames") or {}).get("comparison") or {})
    check("E4.unavailable_frame_missing", comparison.get("status") == "unavailable"
          and comparison.get("reason") == "frame_missing" and (entry.get("step") or {}).get("scope") == "prepare",
          json.dumps(entry)[:1500])

    # 5(a) and 6 (pending_restart): item 1's suspension, a new main package digest.
    lifted_case = dict(main="ap2", frames=home_d1 + home_d2, mapping=h_map, return_home=ID_H)
    new_config1 = r.config("E5a new binding", root1, **lifted_case)

    def during_old_daemon():
        # The configuration file is changed while the daemon that read the old one runs.
        time.sleep(2)
        r.config("E5a new binding", root1, **lifted_case)
        return suspended("E5a during the daemon (new configuration file)", new_runtime, new_config1)

    _, result = r.go("E5a restart with the old binding", root1, main="ap", frames=["home"] * 4, mapping=h_map,
                     return_home=ID_H, during=during_old_daemon, settle_s=15)
    code, report = result["during"] or (None, None)
    lifted = (report or {}).get("lifted") or []
    check("E5a.pending_restart_while_the_daemon_runs", code == 0 and len(lifted) == 1
          and lifted[0].get("lifted_by") == "main_digest" and lifted[0].get("effective") == "pending_restart"
          and "config_newer_than_daemon_start" in (report or {}).get("warnings", []), json.dumps(report)[:1500])
    events = r.events("E5a old binding", root1)
    check("E5a.not_lifted_without_a_new_digest", len(intents(events)) == len(intents(events1)),
          f"{len(intents(events))} vs {len(intents(events1))}")
    code, report = suspended("E5a before the restart", new_runtime, new_config1)
    _, result = r.go("E5a restart with the new binding", root1, **lifted_case)
    events = r.events("E5a new binding", root1)
    found = describe_executions("E5a", events)
    new = found[2:]
    failures = [execution.get("failure") or {} for execution in new]
    check("E5a.dispatched_again_new_streak", len(new) >= 1 and failures[0].get("consecutive_same_error") == 1
          and failures[0].get("disposition") == "retry_scheduled"
          and identity(failures[0].get("error_code")) is not None
          and identity(failures[0].get("error_code"))["m"] == r.digest_of("ap2")[:12], json.dumps(failures)[:1200])
    check("E5a.repeated_failure_pauses_again", len(new) == 2 and failures[1].get("disposition") == "paused_task"
          and failures[1].get("error_code") == failures[0].get("error_code"), json.dumps([f.get("error_code") for f in failures]))
    suspended("E5a after", new_runtime, new_config1)

    # 5(b): another copy of item 1's ledger; only H's mapped digest changes, or an unrelated entry.
    h2_case = dict(main="ap", frames=home_d1 + home_d2, mapping=[(ID_H, "h2", "h2")], return_home=ID_H)
    unrelated_case = dict(main="ap", frames=["home"], mapping=h_map + [(ID_U, "u", "u")], return_home=ID_H)
    unrelated_config = r.config("E5b unrelated mapping", root1_copy, **unrelated_case)
    code, report = suspended("E5b unrelated entry", new_runtime, unrelated_config)
    check("E5b.unrelated_entry_not_lifted", code == 0 and len((report or {}).get("suspended") or []) == 1
          and not (report or {}).get("lifted"), json.dumps(report)[:1200])
    h2_config = r.config("E5b H updated", root1_copy, **h2_case)
    code, report = suspended("E5b H digest changed", new_runtime, h2_config)
    lifted = (report or {}).get("lifted") or []
    check("E5b.H_digest_lifts_layer_1", code == 0 and len(lifted) == 1 and lifted[0].get("lifted_by") == "layer:1",
          json.dumps(report)[:1200])
    before = len(intents(r.events("E5b before", root1_copy)))
    r.roots.append(("E5b", root1_copy))
    r.go("E5b restart with H updated", root1_copy, **h2_case)
    events = r.events("E5b", root1_copy)
    found = describe_executions("E5b", events)
    check("E5b.dispatched_after_layer_change", len(intents(events)) > before and len(found) >= 3
          and (found[2].get("failure") or {}).get("consecutive_same_error") == 1, f"{before} -> {len(intents(events))}")

    # 5(c): item 4 with the right digest: lifted, and the next run succeeds.
    right = dict(main="ap", frames=["home", "home", "a2", "a3"], mapping=h_map, return_home=ID_H)
    right_config = r.config("E5c corrected digest", root4, **right)
    code, report = suspended("E5c corrected digest", new_runtime, right_config)
    lifted = (report or {}).get("lifted") or []
    check("E5c.corrected_digest_lifts", code == 0 and len(lifted) == 1 and lifted[0].get("lifted_by") == "layer:1",
          json.dumps(report)[:1200])
    r.go("E5c restart with the corrected digest", root4, **right)
    events = r.events("E5c", root4)
    found = describe_executions("E5c", events)
    check("E5c.next_run_succeeds", len(found) == 3 and found[2].get("kind") == "succeeded", json.dumps([e.get("kind") for e in found]))
    code, report = suspended("E5c after", new_runtime, right_config)
    check("E5c.nothing_listed", code == 0 and not (report or {}).get("suspended") and not (report or {}).get("lifted"),
          json.dumps(report)[:1200])

    # 5(d): a v0.9.0 (885947c9) suspension of a page-graph task with its original code.
    root5d = r.new_root("E5d")
    pause_first = {"action": "pause", "retry_limit": 0, "retry_backoff_ms": 1000, "escalation_threshold": 2}
    pg_frames = ["home", "a2"] + ["a2"] * 24
    r.go("E5d v0.9.0 page-graph failure", root5d, runtime=old_runtime, main="pgf", frames=pg_frames, on_failure=pause_first)
    events = r.events("E5d old", root5d)
    found = describe_executions("E5d old", events)
    old_code = (found[0].get("failure") or {}).get("error_code") if found else None
    check("E5d.old_suspension", len(found) == 1 and (found[0].get("failure") or {}).get("disposition") == "paused_task"
          and identity(old_code) is None, json.dumps(old_code))
    same_config = r.config("E5d same digest", root5d, main="pgf", frames=pg_frames, on_failure=pause_first)
    code, report = suspended("E5d same digest", new_runtime, same_config)
    check("E5d.same_digest_suspended", code == 0 and len((report or {}).get("suspended") or []) == 1
          and not (report or {}).get("lifted"), json.dumps(report)[:1200])
    r.go("E5d restart, same digest", root5d, main="pgf", frames=pg_frames, on_failure=pause_first)
    events = r.events("E5d same", root5d)
    check("E5d.same_digest_no_dispatch", len(intents(events)) == 1, str(len(intents(events))))
    new_pg = dict(main="pgf2", frames=pg_frames, on_failure=pause_first)
    new_pg_config = r.config("E5d new digest", root5d, **new_pg)
    code, report = suspended("E5d new digest", new_runtime, new_pg_config)
    lifted = (report or {}).get("lifted") or []
    check("E5d.new_digest_lifts", code == 0 and len(lifted) == 1 and lifted[0].get("lifted_by") == "main_digest",
          json.dumps(report)[:1200])
    r.go("E5d restart, new digest", root5d, **new_pg)
    events = r.events("E5d new", root5d)
    found = describe_executions("E5d new", events)
    last = (found[-1].get("failure") or {}) if found else {}
    check("E5d.same_code_pauses_again_at_once", len(found) == 2 and last.get("error_code") == old_code
          and last.get("consecutive_same_error") == 2 and last.get("disposition") == "paused_task", json.dumps(last)[:600])

    # 8. The page-graph task's failure is unchanged: 885947c9 and this build.
    sequences, codes = {}, {}
    for build, runtime_dir in (("885947c9", old_runtime), ("product", new_runtime)):
        root = r.new_root(f"E8 {build}")
        r.go(f"E8 {build} page-graph failure", root, runtime=runtime_dir, main="pgf", frames=pg_frames)
        events = r.events(f"E8 {build}", root)
        found = describe_executions(f"E8 {build}", events)
        codes[build] = [(execution.get("failure") or {}).get("error_code") for execution in found]
        compared = []
        for event in events:
            kind = event.get("event_type")
            if kind == "policy.execution_recorded":
                compared.append(normalize({"event_type": kind, "payload": event.get("payload")}))
                break
            if str(kind).startswith("task."):
                compared.append(normalize({"event_type": kind, "payload": event.get("payload")}))
        sequences[build] = compared
    check("E8.original_code_on_both", codes["885947c9"] == codes["product"] and len(codes["product"]) == 1
          and identity(codes["product"][0]) is None, json.dumps(codes))
    compare("E8", sequences, (("885947c9", "product"),))

    # 9. A sensitive task is paused at its first failure, with no rerun.
    root9 = r.new_root("E9")
    r.go("E9 sensitive", root9, main="ap", frames=["home", "a2", "x", "x", "x", "x"], sensitive=True)
    events = r.events("E9", root9)
    found = describe_executions("E9", events)
    check("E9.paused_at_first_failure", len(found) == 1 and (found[0].get("failure") or {}).get("disposition") == "paused_task"
          and len(intents(events)) == 1, json.dumps([(e.get("failure") or {}).get("disposition") for e in found]))

    # 7. Replay: this build restarts on every ledger; 885947c9 starts and stops on copies.
    replay_cases = {
        "E1": dict(main="ap2", frames=["home"] * 32, mapping=h_map, return_home=ID_H),
        "E2": dict(main="ap", frames=["home"] * 32),
        "E3": dict(main="ap", frames=["z"] * 32, mapping=h_map, return_home=ID_H, window_limit=2),
        "E4": dict(main="ap", frames=["home"] * 32, mapping=h_map, return_home=ID_H),
        "E5b": dict(main="ap", frames=["home"] * 32, mapping=[(ID_H, "h2", "h2")], return_home=ID_H),
        "E5d": dict(main="pgf2", frames=["home"] * 32, on_failure=pause_first),
        "E8 product": dict(main="pgf", frames=["home"] * 32),
        "E9": dict(main="ap", frames=["home"] * 32, sensitive=True),
    }
    for label, root in list(r.roots):
        case = replay_cases.get(label)
        if case is None:
            continue
        # The instance is unavailable to the evaluator: the start replays the ledger and dispatches
        # nothing.
        case = dict(case, available=False)
        r.go(f"E7 product restart {label}", root, align=False, settle_s=3, **case)
        copy = root + "-old-replay"
        shutil.copytree(root, copy)
        old_case = {key: value for key, value in case.items() if key not in ("mapping", "return_home")}
        r.go(f"E7 885947c9 start {label}", copy, runtime=old_runtime, align=False, settle_s=3, **old_case)

    # 10. The v0.9.0 actingledger reads every ledger; the copies' existing files stay unchanged.
    for label, root in r.roots:
        copy = root + "-read-copy"
        shutil.copytree(root, copy)
        before = tree_hashes(copy)
        results = {}
        for name, args in (("open", ["open"]), ("events", ["events"]),
                           ("export --task-evidence", ["export", "--task-evidence"])):
            code, out, err = run_exe([r.old_ledger, "--state-root", copy, *args])
            corrupt = "corrupt_ledger_record" in out or "corrupt_ledger_record" in err
            results[name] = [code, corrupt]
            say("E10", label, name, "exit", code, "stdout bytes", len(out), "stderr", short(err, 300))
        after = tree_hashes(copy)
        changed = sorted(name for name in before if before.get(name) != after.get(name))
        added = sorted(name for name in after if name not in before)
        check(f"E10.old_reads.{label.replace(' ', '_')}", all(code == 0 and not corrupt for code, corrupt in results.values()),
              json.dumps(results))
        check(f"E10.existing_files_unchanged.{label.replace(' ', '_')}", not changed,
              json.dumps({"files": len(before), "changed": changed, "added": added}))
        events_old, failure = ledger_events(r.old_ledger, copy)
        events_new, _ = ledger_events(r.new_ledger, root)
        check(f"E10.old_pages_all_events.{label.replace(' ', '_')}", events_old is not None and events_new is not None
              and len(events_old) == len(events_new) and len(events_new) > 0,
              f"{len(events_old or [])} vs {len(events_new or [])} {failure}")

    say("RESULT", "failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


def tree_hashes(root):
    hashes = {}
    for folder, _dirs, names in os.walk(root):
        for name in names:
            path = os.path.join(folder, name)
            with open(path, "rb") as handle:
                hashes[os.path.relpath(path, root)] = hashlib.sha256(handle.read()).hexdigest()
    return hashes


VOLATILE = re.compile(r"(^|_)(id|ids|at|ms|us|sequence|timestamp|time|elapsed|monotonic|budget|remaining|duration)$")


def normalize(value):
    if isinstance(value, dict):
        return {key: ("<volatile>" if ((VOLATILE.search(key) and key != "target_id")
                                       or key in ("links", "task_timing", "sampling", "perf_context"))
                      else sampled_action(item) if key == "action" else normalize(item))
                for key, item in sorted(value.items())}
    if isinstance(value, list):
        return [normalize(item) for item in value]
    if isinstance(value, str):
        value = re.sub(r"(?<![0-9a-z_])[a-z]+_[0-9a-f]{32}(?![0-9a-z_])", "<identifier>", value)
        value = re.sub(r"decision:[0-9a-f]{64}", "<decision>", value)
        return re.sub(r"(?<![0-9a-f])[0-9a-f]{64}(?![0-9a-f])", "<sha256>", value)
    return value


def sampled_action(value):
    if isinstance(value, dict):
        return {key: ("<sampled>" if key in ("x", "y", "x1", "y1", "x2", "y2") else normalize(item))
                for key, item in sorted(value.items())}
    return normalize(value)


def collapse(sequence):
    sequence = list(sequence)
    changed = True
    while changed:
        changed = False
        for size in range(1, 41):
            index = 0
            while index + 2 * size <= len(sequence):
                if sequence[index:index + size] == sequence[index + size:index + 2 * size]:
                    del sequence[index + size:index + 2 * size]
                    changed = True
                else:
                    index += 1
    return sequence


def compare(label, sequences, pairs):
    for left, right in pairs:
        old, new = collapse(sequences[left]), collapse(sequences[right])
        difference = next((index for index, (a, b) in enumerate(zip(old, new)) if a != b), None)
        say(label, left, len(old), right, len(new), "first difference", difference)
        if difference is not None:
            say(label, left, "at difference", short(json.dumps(old[difference]), 2500))
            say(label, right, "at difference", short(json.dumps(new[difference]), 2500))
        check(f"{label}.identical.{left}_vs_{right}", len(old) == len(new) and difference is None and len(old) > 0,
              f"{len(old)} vs {len(new)}")


if __name__ == "__main__":
    command = sys.argv[1]
    if command == "static":
        sys.exit(static(sys.argv[2], sys.argv[3], sys.argv[4]))
    if command == "prepare":
        sys.exit(prepare(sys.argv[2]))
    if command == "frames":
        sys.exit(frames(sys.argv[2]))
    if command == "frames-check":
        sys.exit(frames_check(sys.argv[2], sys.argv[3]))
    sys.exit(run(*sys.argv[2:8]))
