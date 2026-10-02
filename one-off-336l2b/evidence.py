# One-off (to be reverted), Workflow #336 L2b evidence: prerequisite packages and the entry gate
# of linear_steps packages (R14/R15 amendment 5954419623 "L2b" items 1-11, R24 amendment
# 5958084341 "L2b" items 1-2). Every printed line starts with "L2B|". Usage:
#   evidence.py static <repo> <base sha> <product sha>
#   evidence.py prepare <work>
#   evidence.py run <work> <new runtime> <new tools> <base runtime> <old runtime> <old tools> <catalog dir>
#   evidence.py same <product host log> <8e0ac191 host log>
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
GAME = "fixture-game-a"
SERVER = "fixture-server-a"
ALIAS = "node.a"
INSTANCE_ID = "instance_00000000000000000000000000000336"
W, H = 64, 36
BG = (32, 32, 32)
COLORS = {
    "x": (200, 40, 40),
    "home": (200, 40, 200),
    "a2": (40, 200, 40),
    "m": (40, 40, 200),
    "c2": (120, 120, 40),
}
MARKER = (250, 250, 250)
RESOURCES = {"schema_version": "1.0", "resources": [], "resource_count": 0}
APPLICATION_REFUSAL = "application_effect_requires_assigned_application"
UNMATCHED = "contained_task_prerequisite_entry_unmatched"
FAILURES = []

ID_A = "fixture.prereq.a"
ID_H = "fixture.prereq.h"
ID_B = "fixture.prereq.b"
ID_C = "fixture.prereq.c"


def say(*parts):
    print("L2B|" + "|".join(str(part) for part in parts), flush=True)


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


def make_frame(name):
    image = Image.new("RGB", (W, H), BG)
    state = "home" if name == "homep" else name
    if state in COLORS:
        for y in range(9, 12):
            for x in range(9, 12):
                image.putpixel((x, y), COLORS[state])
    if name == "home":
        for y in range(19, 22):
            for x in range(19, 22):
                image.putpixel((x, y), MARKER)
    return image


def probe(state):
    return {"id": f"state/{state}", "region": {"mode": "rect", "rect": {"x": 10, "y": 10, "width": 1, "height": 1}},
            "expected": list(COLORS[state])}


MARKER_PROBE = {"id": "state/marker", "region": {"mode": "rect", "rect": {"x": 20, "y": 20, "width": 1, "height": 1}},
                "expected": list(MARKER)}
ALT_PROBE = {"id": "state/alt", "region": {"mode": "rect", "rect": {"x": 30, "y": 30, "width": 1, "height": 1}},
             "expected": [1, 2, 3]}


def state_of(page):
    return page.rsplit("_", 1)[-1]


def guard(page):
    return {"page_id": page, "target_id": f"state/{state_of(page)}",
            "expected_rect": {"x": 10, "y": 10, "width": 1, "height": 1}, "color_probe": f"state/{state_of(page)}"}


def source_pack(task_id, package_id, spec, mode="linear_steps", prerequisite=None, marker_pages=(), any_of_pages=(),
                max_steps=None, outcome=True, resolution=(W, H), entry_page=None):
    """spec: (from, to, effect) per operation; effect is "click" or an application action."""
    pages = []
    for frm, to, _ in spec:
        for page in (frm, to):
            if page != "any" and page not in pages:
                pages.append(page)
    states = sorted({state_of(page) for page in pages})
    operations = []
    for index, (frm, to, effect) in enumerate(spec):
        operation = {
            "id": f"step_{index + 1:02d}_{'click' if effect == 'click' else 'app'}",
            "purpose": f"Prerequisite fixture step {index + 1}",
            "from": frm,
            "to": to,
            "expect_after": {"page_id": to, "timeout_ms": 500, "interval_ms": 100},
            "post_delay_ms": 50,
        }
        if effect == "click":
            operation["click"] = {"kind": "rect", "x": 8, "y": 8, "width": 5, "height": 5}
            operation["guard"] = guard(frm)
        else:
            operation["application"] = {"action": effect}
        operations.append(operation)
    steps = max_steps or len(operations)
    probes = [probe(state) for state in states]
    if marker_pages or any_of_pages:
        probes.append(MARKER_PROBE)
    if any_of_pages:
        probes.append(ALT_PROBE)
    rules = {}
    for page in pages:
        rule = {"required": [f"state/{state_of(page)}"]}
        if page in marker_pages:
            rule["required"].append("state/marker")
        if page in any_of_pages:
            rule["any_of"] = [["state/marker", "state/alt"]]
        rules[page] = rule
    task = {
        "schema_version": "0.9",
        "task_id": task_id,
        "game": GAME,
        "server_scope": [SERVER],
        "locale": "en-US",
        "goal": f"Prerequisite fixture {task_id}",
        "coordinate_space": {"width": resolution[0], "height": resolution[1]},
        "defaults": {"template_threshold": 0.95, "color_max_distance": 20, "match_metric": "ccoeff_normed"},
        "timeout_ms": 30000,
        "max_steps": steps,
        "entry_page": entry_page or spec[0][0],
        "target_page": spec[-1][1],
        "color_probes": probes,
        "page_rules": rules,
        "operations": operations,
    }
    if outcome:
        task["scheduling_outcome"] = {"mappings": [{
            "outcome_key": f"{task_id}_done", "effect": "no_designated_effect", "terminal_pages": [spec[-1][1]]}]}
    control = {
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": mode,
        "game": GAME,
        "server": SERVER,
        "resolution": {"width": resolution[0], "height": resolution[1]},
        "entry_task_id": task_id,
        "timeout_ms": 30000,
        "step_timeout_ms": 500,
        "capture_interval_ms": 100,
        "max_steps": steps,
    }
    if prerequisite is not None:
        control["prerequisite_package_id"] = prerequisite
    return {
        "control.json": pretty(control),
        "resources/operations/resources.json": pretty(RESOURCES),
        f"resources/operations/{task_id}/task.json": pretty(task),
    }


def legacy_zip(task_id, package_id, prerequisite):
    """The compiled ZIP layout a sha256: digest admits (no source compile): A, home -> step_02_a2."""
    target = f"{GAME}/step_02_a2"
    control = {
        "schema_version": "Lab-1y.control.v2", "package_id": package_id, "execution_mode": "linear_steps",
        "game": GAME, "server": SERVER, "resolution": {"width": W, "height": H}, "entry_task_id": task_id,
        "timeout_ms": 30000, "step_timeout_ms": 500, "capture_interval_ms": 100, "max_steps": 1,
        "prerequisite_package_id": prerequisite,
    }
    task = {
        "schema_version": "0.9", "task_id": task_id, "game": GAME, "server_scope": [SERVER],
        "coordinate_space": {"width": W, "height": H}, "timeout_ms": 30000, "max_steps": 1,
        "entry_page": "home", "target_page": "step_02_a2",
        "scheduling_outcome": {"mappings": [{"outcome_key": f"{task_id}_done", "effect": "no_designated_effect",
                                             "terminal_pages": ["step_02_a2"]}]},
        "operations": [{
            "id": "step_01_click", "from": "home", "to": "step_02_a2",
            "click": {"kind": "point", "x": 10, "y": 10}, "unguarded_trusted_coordinate": True,
            "expect_after": {"page_id": "step_02_a2", "timeout_ms": 500, "interval_ms": 100}, "post_delay_ms": 50,
        }],
    }
    recognition = {
        "schema_version": "0.3", "game": GAME, "server": SERVER, "coordinate_space": {"width": W, "height": H},
        "defaults": {"color_max_distance": 20.0},
        "targets": [
            {"type": "color", "id": "page/home", "region": {"x": 10, "y": 10, "width": 1, "height": 1},
             "expected": list(COLORS["home"])},
            {"type": "color", "id": "page/a2", "region": {"x": 10, "y": 10, "width": 1, "height": 1},
             "expected": list(COLORS["a2"])},
        ],
    }
    pages = {"schema_version": "0.3", "pages": [
        {"id": f"{GAME}/home", "required": ["page/home"], "optional": [], "forbidden": []},
        {"id": target, "required": ["page/a2"], "optional": [], "forbidden": []},
    ]}
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", zipfile.ZIP_STORED) as archive:
        for path, value in (
                ("control.json", control),
                ("resources/manifest.json", {"schema_version": "0.3", "entry_task_id": task_id}),
                (f"resources/operations/{task_id}/task.json", task),
                (f"resources/recognition/{GAME}.{SERVER}.pack.json", recognition),
                (f"resources/recognition/{GAME}.{SERVER}.pages.json", pages)):
            info = zipfile.ZipInfo(path, date_time=(1980, 1, 1, 0, 0, 0))
            archive.writestr(info, json.dumps(value, separators=(",", ":")).encode("utf-8"))
    return buffer.getvalue()


A_SPEC = [("home", "step_02_a2", "click")]
H_SPEC = [("x", "home", "click")]
B_SPEC = [("home", "step_02_m", "click")]
C_SPEC = [("step_01_m", "step_02_c2", "click")]


def packs():
    return {
        "a": source_pack("pre_a", ID_A, A_SPEC, prerequisite=ID_H),
        "a_marker": source_pack("pre_a", ID_A, A_SPEC, prerequisite=ID_H, marker_pages=("home",)),
        "h": source_pack("pre_h", ID_H, H_SPEC, mode="navigable_route"),
        "b": source_pack("pre_b", ID_B, B_SPEC, prerequisite=ID_H),
        "b_marker": source_pack("pre_b", ID_B, B_SPEC, prerequisite=ID_H, marker_pages=("home",)),
        "c": source_pack("pre_c", ID_C, C_SPEC, prerequisite=ID_B),
        # Refusals at preparation.
        "p": source_pack("pre_p", "fixture.prereq.p", A_SPEC, prerequisite="fixture.prereq.q"),
        "q": source_pack("pre_q", "fixture.prereq.q", B_SPEC, prerequisite="fixture.prereq.p"),
        "l0": source_pack("pre_l0", "fixture.prereq.l0", A_SPEC, prerequisite="fixture.prereq.l1"),
        "l1": source_pack("pre_l1", "fixture.prereq.l1", A_SPEC, prerequisite="fixture.prereq.l2"),
        "l2": source_pack("pre_l2", "fixture.prereq.l2", A_SPEC, prerequisite="fixture.prereq.l3"),
        "l3": source_pack("pre_l3", "fixture.prereq.l3", A_SPEC, prerequisite="fixture.prereq.l4"),
        "l4": source_pack("pre_l4", "fixture.prereq.l4", A_SPEC),
        "a_r": source_pack("pre_a", ID_A, A_SPEC, prerequisite="fixture.prereq.r"),
        "r": source_pack("pre_r", "fixture.prereq.r", H_SPEC, mode="recognize_only", outcome=False),
        "a_res": source_pack("pre_a", ID_A, A_SPEC, prerequisite="fixture.prereq.hres"),
        "h_res": source_pack("pre_hres", "fixture.prereq.hres", H_SPEC, mode="navigable_route", resolution=(32, 18)),
        "a_big": source_pack("pre_a", ID_A, A_SPEC, prerequisite="fixture.prereq.hbig"),
        "h_big": source_pack("pre_hbig", "fixture.prereq.hbig", H_SPEC, mode="navigable_route", max_steps=1000),
        "a_self": source_pack("pre_a", ID_A, A_SPEC, prerequisite=ID_A),
        "a_badid": source_pack("pre_a", ID_A, A_SPEC, prerequisite=""),
        "h_field": source_pack("pre_h", ID_H, H_SPEC, mode="navigable_route", prerequisite=ID_A),
        # R24: an application entry with a prerequisite; an application entry as the prerequisite.
        "x1_declares": source_pack("pre_x1", "fixture.prereq.x1", [("any", "home", "restart")], prerequisite=ID_H),
        "x1": source_pack("pre_x1", "fixture.prereq.x1", [("any", "home", "restart")]),
        "a_x1": source_pack("pre_a", ID_A, A_SPEC, prerequisite="fixture.prereq.x1"),
        # The existing page-graph home entry (required home page through any_of), no prerequisite.
        "pg_home": source_pack("pre_pg", "fixture.prereq.pg", A_SPEC, mode="navigable_route", any_of_pages=("home",)),
    }


def load_state(work):
    with open(os.path.join(work, "state.json"), encoding="utf-8") as handle:
        return json.load(handle)


def save_state(work, state):
    with open(os.path.join(work, "state.json"), "w", encoding="utf-8") as handle:
        json.dump(state, handle, indent=2)


# ---------------------------------------------------------------------------------------------
# static: what the product changes and what it leaves alone.

def git(repo, *args):
    return subprocess.run(["git", "-C", repo, *args], capture_output=True, check=True).stdout.decode("utf-8")


def function_span(source, signature):
    match = re.search(r"\n([ \t]*)(pub(\([a-z]+\))? )?(const )?fn " + re.escape(signature) + r"\b", source)
    if not match:
        return None
    start = match.start() + 1
    depth, index, in_string, escaped, in_comment = 0, source.index("{", match.end()), False, False, False
    while index < len(source):
        char = source[index]
        if in_comment:
            if char == "\n":
                in_comment = False
        elif in_string:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                in_string = False
        elif char == "/" and source[index:index + 2] == "//":
            in_comment = True
        elif char == '"':
            in_string = True
        elif char == "'" and source[index:index + 3] in ("'{'", "'}'"):
            index += 2
        elif char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return source[start:index + 1]
        index += 1
    return None


def static(repo, base, product):
    say("STATIC", "base", base[:8], "product", product[:8])
    for line in git(repo, "diff", "--stat", base, product).splitlines():
        say("STATIC", "diff --stat", line)
    for path in ("crates/actingcommand-contract", "crates/execution-kernel/src/run.rs",
                 "crates/execution-kernel/src/offline.rs", "crates/runtime-host/src/host/recovery_ladder.rs",
                 "crates/runtime-host/src/host/startup_package.rs", "crates/ledger", "crates/ledger-forensics",
                 "apps/actingctl", "crates/lab", "crates/resource-tooling"):
        stat = git(repo, "diff", "--stat", base, product, "--", path).strip()
        check(f"STATIC.unchanged.{path}", not stat, stat)
    for path, names in (
            ("crates/runtime-host/src/host/contained_task.rs",
             ("run_preflighted_contained_task", "record_geometry_triggered_recovery_failure",
              "fail_contained_task_entry", "startup_package_admission_failure")),
            ("crates/execution-kernel/src/contained_task.rs",
             ("recognize_required_home", "run_entry_recovery", "run_with_options", "run_with_collector",
              "required_home_entry_page", "capture_page", "capture_until_page", "terminal_matches_required_home"))):
        before, after = git(repo, "show", f"{base}:{path}"), git(repo, "show", f"{product}:{path}")
        for name in names:
            old, new = function_span(before, name), function_span(after, name)
            check(f"STATIC.function_identical.{os.path.basename(path)}.{name}", old is not None and old == new,
                  "missing" if old is None else "")
    ladder = git(repo, "show", f"{product}:crates/actingcommand-contract/src/event/payload/recovery_ladder.rs")
    for line in function_span(ladder, "is_stuck_recovery_trigger").splitlines():
        say("STATIC", "RLC is_stuck_recovery_trigger", line)
    host = git(repo, "show", f"{product}:crates/runtime-host/src/host/contained_task.rs")
    execute = function_span(host, "execute_contained_task_with_lease")
    branch = execute[execute.index("} else if !prerequisites.is_empty()"):execute.index("let execution = prepared.run(&mut runtime);")]
    for line in branch.splitlines():
        say("STATIC", "execute branch", line)
    say("RESULT", "static failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


# ---------------------------------------------------------------------------------------------
# prepare: packages, frames, kernel cases.

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
    zip_bytes = legacy_zip("pre_a", ID_A, ID_H)
    zip_path = os.path.join(work, "packs", "a_zip", "a_zip.zip")
    os.makedirs(os.path.dirname(zip_path))
    with open(zip_path, "wb") as handle:
        handle.write(zip_bytes)
    state["zip"] = {"path": zip_path, "sha256": hashlib.sha256(zip_bytes).hexdigest()}
    say("prepare", "legacy zip", zip_path, "sha256", state["zip"]["sha256"], "bytes", len(zip_bytes))
    for name in ("x", "home", "homep", "a2", "m", "c2", "y"):
        path = os.path.join(work, "frames", name + ".png")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        make_frame(name).save(path, format="PNG")
        state["frames"][name] = path

    def case(name, **fields):
        pack = state["packs"][name]
        return {"name": name, "locator": pack["path"],
                "reference": json.dumps({"schema_version": SCHEMA_DIR, "sha256": pack["digest"]}), **fields}

    linear_invalid = "contained_task_linear_invalid"
    control_invalid = "contained_task_control_invalid"
    admission = [
        case("a", expect="ok", expect_detail=f'linear_entry_page=Some("{GAME}/home") prerequisite=Some("{ID_H}") compatible=true'),
        case("b", expect="ok", expect_detail=f'linear_entry_page=Some("{GAME}/home") prerequisite=Some("{ID_H}")'),
        case("c", expect="ok", expect_detail=f'linear_entry_page=Some("{GAME}/step_01_m") prerequisite=Some("{ID_B}")'),
        case("h", expect="ok", expect_detail="linear_entry_page=None prerequisite=None compatible=true incompatibility=None"),
        case("x1", expect="ok", expect_detail="linear_entry_page=None prerequisite=None compatible=true"),
        case("r", expect="ok", expect_detail='compatible=false incompatibility=Some("recognize_only")'),
        case("pg_home", expect="ok", expect_detail=f'required_home_entry_page=Some("{GAME}/home") linear_entry_page=None prerequisite=None'),
        case("a_self", expect=control_invalid, expect_detail="reason=prerequisite_self"),
        case("a_badid", expect=control_invalid, expect_detail="reason=prerequisite_id_invalid"),
        case("h_field", expect=control_invalid, expect_detail="reason=prerequisite_requires_linear_steps"),
        case("x1_declares", expect=linear_invalid, expect_detail="reason=prerequisite_with_application_entry operation=step_01_app"),
    ]
    with open(os.path.join(work, "cases.json"), "w", encoding="utf-8") as handle:
        json.dump({"admission": admission, "runs": []}, handle, indent=2)
    save_state(work, state)
    say("prepare", "done", work)
    return 0


# ---------------------------------------------------------------------------------------------
# run: scheduled fixture dispatches (a fixture instance takes no direct task-run).

def run_exe(args, timeout=300, cwd=None):
    try:
        result = subprocess.run(args, capture_output=True, timeout=timeout, cwd=cwd)
    except subprocess.TimeoutExpired:
        return None, "", "timeout"
    return result.returncode, result.stdout.decode("utf-8", "replace"), result.stderr.decode("utf-8", "replace")


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


def write_config(config_dir, state_root, package_path, package_digest, frames, catalog_dir, prerequisites):
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
        "secret_fingerprint_salt": "oneoff-336-l2b-fixture-salt-value",
        "policy": {
            "facts": {
                "ledger_position": 0, "fact_snapshot_id": "snapshot:oneoff-336-l2b",
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
                "package_digest": package_digest,
                "operation_id": "operation.observe",
                "yield_points": ["after_observation"],
                "scheduled_execution": {"mode": "fixture_simulation", "package_path": package_path},
            }],
        },
        "instances": [{
            "alias": ALIAS,
            "instance_id": INSTANCE_ID,
            "fixture_backend": {
                "frames": [{"width": W, "height": H, "rgb": list(Image.open(frame).convert("RGB").tobytes())}
                           for frame in frames],
                "max_inputs": 4,
            },
        }],
    }
    if prerequisites is not None:
        config["prerequisite_packages"] = prerequisites
    path = os.path.join(config_dir, "actingd.json")
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(config, handle, separators=(",", ":"))
    return path


def wait_for_minute_window():
    second = time.time() % 60
    if second < 2:
        time.sleep(2 - second)
    elif second > 20:
        time.sleep(62 - second)


def prerequisite_entries(state, mapping):
    """mapping: list of (package id, pack name)."""
    return [{"package_id": package_id, "package_path": state["packs"][name]["path"],
             "package_digest": {"schema_version": SCHEMA_DIR, "sha256": state["packs"][name]["digest"]}}
            for package_id, name in mapping]


def scheduled_run(work, label, runtime_dir, main, frame_names, catalog_dir, mapping, settle_s=20):
    state = load_state(work)
    run_dir = os.path.join(work, "runs", re.sub(r"[^A-Za-z0-9_.-]", "_", label))
    state_root = os.path.join(run_dir, "state")
    os.makedirs(state_root)
    frames = [state["frames"][name] for name in frame_names]
    if main == "a_zip":
        package_path, package_digest = state["zip"]["path"], "sha256:" + state["zip"]["sha256"]
    else:
        package_path = state["packs"][main]["path"]
        package_digest = {"schema_version": SCHEMA_DIR, "sha256": state["packs"][main]["digest"]}
    prerequisites = None if mapping is None else prerequisite_entries(state, mapping)
    config = write_config(os.path.join(run_dir, "config"), state_root, package_path, package_digest, frames,
                          catalog_dir, prerequisites)
    say(label, "main", main, "frames", json.dumps(frame_names), "prerequisite_packages",
        json.dumps(None if mapping is None else [[package_id, name, state["packs"][name]["digest"]] for package_id, name in mapping]))
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
    return state_root, config


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


def kinds(facts, kind):
    return [fact for _, _, fact in facts if fact["kind"] == kind]


def terminal(facts):
    found = kinds(facts, "terminal_committed")
    return found[-1] if found else {}


def ref_sha(value):
    if isinstance(value, dict):
        return value.get("sha256")
    if isinstance(value, str):
        return value.split(":", 1)[-1]
    return value


def describe(fact):
    kind = fact["kind"]
    if kind == "terminal_committed":
        timing = fact.get("task_timing") or {}
        return {key: fact.get(key) for key in ("outcome", "final_page", "executed_steps", "failure_code",
                                               "failure_severity")} | {"task_failure": timing.get("task_failure")}
    if kind == "package_admitted":
        return {"package_label": fact.get("package_label"), "package_sha256": ref_sha(fact.get("package_sha256"))}
    if kind in ("step_started", "step_finished", "effect_intent", "effect_completed"):
        return {key: fact.get(key) for key in ("step_index", "operation_label", "from_page", "page_label") if key in fact}
    if kind in ("recognition_started", "recognition_completed"):
        return {key: fact.get(key) for key in ("candidate_pages", "matched_page") if key in fact}
    detail = {key: value for key, value in fact.items() if key != "kind"}
    if "package_sha256" in detail:
        detail["package_sha256"] = ref_sha(detail["package_sha256"])
    return detail


def print_facts(label, facts):
    for sequence, event_type, fact in facts:
        say(label, "fact", sequence, event_type, fact["kind"], short(json.dumps(describe(fact), ensure_ascii=False), 1500))


def run_case(work, label, runtime_dir, ledger, main, frame_names, catalog_dir, mapping):
    root, config = scheduled_run(work, label, runtime_dir, main, frame_names, catalog_dir, mapping)
    events, failure = ledger_events(ledger, root)
    if events is None:
        say(label, "events", "failed", failure)
        events = []
    all_facts = facts_of(events)
    facts = first_run(all_facts)
    say(label, "ledger events", len(events), "task facts", len(all_facts),
        "terminal facts (runs)", len(kinds(all_facts, "terminal_committed")))
    print_facts(label, facts)
    types = [event.get("event_type") for event in events]
    say(label, "event types", json.dumps(types))
    for event in events:
        if event.get("event_type") in ("runtime.failed", "command.rejected", "policy.execution_recorded",
                                       "runtime.resource_declaration_rejected", "task.failed"):
            say(label, event["event_type"], event["sequence"], short(json.dumps(event.get("payload"), ensure_ascii=False), 2500))
    return facts, root, events, types, json.dumps(events, ensure_ascii=False), config


ENTRY_KINDS = ("entry_recognition", "entry_recovery_decision", "entry_recovery_package_admitted",
               "entry_recovery_completed", "entry_recovery_failed", "entry_target_disposition")


def entry_sequence(facts):
    """The entry facts, PackageAdmitted and the terminal, as compact tuples."""
    rows = []
    for _, _, fact in facts:
        kind = fact["kind"]
        if kind == "entry_recognition":
            rows.append((kind, fact.get("phase"), fact.get("required_page"), fact.get("matched")))
        elif kind == "entry_recovery_decision":
            rows.append((kind, fact.get("required")))
        elif kind == "entry_recovery_package_admitted":
            rows.append((kind, ref_sha(fact.get("package_sha256"))))
        elif kind == "entry_recovery_completed":
            rows.append((kind, ref_sha(fact.get("package_sha256")), fact.get("final_page"), fact.get("executed_steps")))
        elif kind == "entry_recovery_failed":
            rows.append((kind, ref_sha(fact.get("package_sha256")), fact.get("failure_code")))
        elif kind == "entry_target_disposition":
            rows.append((kind, fact.get("disposition"), fact.get("failure_code")))
        elif kind == "package_admitted":
            rows.append((kind, fact.get("package_label"), ref_sha(fact.get("package_sha256"))))
        elif kind in ("step_started", "step_finished"):
            rows.append((kind, fact.get("step_index"), fact.get("operation_label")))
        elif kind == "terminal_committed":
            rows.append((kind, fact.get("outcome"), fact.get("executed_steps"), fact.get("failure_code")))
    return [list(row) for row in rows]


def ledger_reads(label, ledger, root, commands):
    results = {}
    for name, args in commands:
        code, out, err = run_exe([ledger, "--state-root", root, *args])
        corrupt = "corrupt_ledger_record" in out or "corrupt_ledger_record" in err
        say(label, name, "exit", code, "stdout bytes", len(out), "corrupt_ledger_record", corrupt, "stderr", short(err, 300))
        results[name] = (code, corrupt, out)
    return results


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
                                       or key in ("links", "task_timing", "sampling")) else normalize(item))
                for key, item in sorted(value.items())}
    if isinstance(value, list):
        return [normalize(item) for item in value]
    if isinstance(value, str):
        value = re.sub(r"(?<![0-9a-z_])[a-z]+_[0-9a-f]{32}(?![0-9a-z_])", "<identifier>", value)
        return re.sub(r"(?<![0-9a-f])[0-9a-f]{64}(?![0-9a-f])", "<sha256>", value)
    return value


def expect_rows(label, facts, expected):
    rows = entry_sequence(facts)
    for row in rows:
        say(label, "row", json.dumps(row, ensure_ascii=False))
    check(f"{label}.fact_sequence", rows == expected,
          json.dumps({"expected": expected, "actual": rows}, ensure_ascii=False))
    return rows


def run(work, new_runtime, new_tools, base_runtime, old_runtime, old_tools, catalog_dir):
    state = load_state(work)
    digests = {name: entry["digest"] for name, entry in state["packs"].items()}
    new_ledger = os.path.join(new_tools, "actingledger.exe")
    old_ledger = os.path.join(old_tools, "actingledger.exe")
    produced = []
    home, a2_page, m_page, c2_page = f"{GAME}/home", f"{GAME}/step_02_a2", f"{GAME}/step_01_m", f"{GAME}/step_02_c2"
    h_map = [(ID_H, "h")]

    def case(label, main, frames, mapping, runtime=new_runtime, ledger=new_ledger, keep=True):
        facts, root, events, types, text, config = run_case(work, label, runtime, ledger, main, frames, catalog_dir, mapping)
        if keep:
            produced.append((label, root))
        return facts, root, events, types, text, config

    # 1. Already on the first step: no prerequisite package runs.
    facts, *_ = case("E1 A from HOME", "a", ["home", "home", "a2"], h_map)
    expect_rows("E1", facts, [
        ["entry_recognition", "initial", home, True], ["entry_recovery_decision", False],
        ["entry_target_disposition", "started", None], ["package_admitted", ID_A, digests["a"]],
        ["step_started", 0, "step_01_click"], ["step_finished", 0, "step_01_click"],
        ["terminal_committed", "success", 1, None]])
    check("E1.effect_completed_once", len(kinds(facts, "effect_completed")) == 1, "")

    # 2 / 6. From elsewhere, scheduled (no request binding): H runs inside the same run.
    facts, _root, events, types, *_ = case("E2 A from X", "a", ["x", "x", "home", "home", "home", "a2"], h_map)
    e2_rows = expect_rows("E2", facts, [
        ["entry_recognition", "initial", home, False], ["entry_recovery_decision", True],
        ["entry_recovery_package_admitted", digests["h"]],
        ["step_started", 0, "step_01_click"], ["step_finished", 0, "step_01_click"],
        ["entry_recovery_completed", digests["h"], home, 1],
        ["entry_recognition", "post_recovery", home, True], ["entry_target_disposition", "started", None],
        ["package_admitted", ID_A, digests["a"]],
        ["step_started", 1, "step_01_click"], ["step_finished", 1, "step_01_click"],
        ["terminal_committed", "success", 2, None]])
    check("E2.one_task_requested", types.count("task.requested") == 1, f"task.requested {types.count('task.requested')}")
    with open(os.path.join(work, "e2-rows.json"), "w", encoding="utf-8") as handle:
        json.dump(e2_rows, handle)

    # 3. Two-level chain C -> B -> H, from X and from HOME.
    facts, root, events, types, *_ = case("E3 C from X", "c", ["x", "x", "x", "home", "home", "home", "m", "m", "m", "c2"],
                                          [(ID_H, "h"), (ID_B, "b")])
    expect_rows("E3", facts, [
        ["entry_recognition", "initial", m_page, False], ["entry_recovery_decision", True],
        ["entry_recovery_package_admitted", digests["b"]],
        ["entry_recognition", "initial", home, False], ["entry_recovery_decision", True],
        ["entry_recovery_package_admitted", digests["h"]],
        ["step_started", 0, "step_01_click"], ["step_finished", 0, "step_01_click"],
        ["entry_recovery_completed", digests["h"], home, 1],
        ["entry_recognition", "post_recovery", home, True],
        ["step_started", 1, "step_01_click"], ["step_finished", 1, "step_01_click"],
        ["entry_recovery_completed", digests["b"], f"{GAME}/step_02_m", 1],
        ["entry_recognition", "post_recovery", m_page, True], ["entry_target_disposition", "started", None],
        ["package_admitted", ID_C, digests["c"]],
        ["step_started", 2, "step_01_click"], ["step_finished", 2, "step_01_click"],
        ["terminal_committed", "success", 3, None]])
    # 8. Five effective configuration records, no limit failure.
    code, out, err = run_exe([new_ledger, "--state-root", root, "export"])
    section = out.split("effective_configuration:", 1)[-1] if "effective_configuration:" in out else ""
    records = [json.loads(line[2:]) for line in section.splitlines() if line.startswith("- {")]
    summary = [(record["configuration"]["facts"].get("phase"), ref_sha(record["configuration"]["facts"].get("package_sha256")))
               for record in records]
    say("E8", "actingledger export exit", code, "effective configuration records", len(records), json.dumps(summary))
    check("E8.five_configuration_records", code == 0 and len(records) == 5
          and [kind for kind, _ in summary].count("entry_recovery") == 2,
          json.dumps(summary))
    check("E8.no_limit_failure", "effective_configuration_limit_exceeded" not in json.dumps(events), "")
    facts, *_ = case("E3 C from HOME", "c", ["home", "home", "home", "m", "m", "m", "c2"], [(ID_H, "h"), (ID_B, "b")])
    expect_rows("E3b", facts, [
        ["entry_recognition", "initial", m_page, False], ["entry_recovery_decision", True],
        ["entry_recovery_package_admitted", digests["b"]],
        ["entry_recognition", "initial", home, True], ["entry_recovery_decision", False],
        ["step_started", 0, "step_01_click"], ["step_finished", 0, "step_01_click"],
        ["entry_recovery_completed", digests["b"], f"{GAME}/step_02_m", 1],
        ["entry_recognition", "post_recovery", m_page, True], ["entry_target_disposition", "started", None],
        ["package_admitted", ID_C, digests["c"]],
        ["step_started", 1, "step_01_click"], ["step_finished", 1, "step_01_click"],
        ["terminal_committed", "success", 2, None]])

    # 4. The prerequisite package does not reach the first step.
    facts, *_ = case("E4 A' from X", "a_marker", ["x", "x"] + ["homep"] * 12, h_map)
    expect_rows("E4", facts, [
        ["entry_recognition", "initial", home, False], ["entry_recovery_decision", True],
        ["entry_recovery_package_admitted", digests["h"]],
        ["step_started", 0, "step_01_click"], ["step_finished", 0, "step_01_click"],
        ["entry_recovery_completed", digests["h"], home, 1],
        ["entry_recognition", "post_recovery", home, False], ["entry_target_disposition", "fail_closed", UNMATCHED],
        ["terminal_committed", "failure", 1, UNMATCHED]])
    failure = (terminal(facts).get("task_timing") or {}).get("task_failure") or {}
    timing = failure.get("timing") or {}
    check("E4.timing", timing.get("scope") == "page_recognition" and timing.get("stage") == "entry_recognition"
          and timing.get("limit_ms") == 500, json.dumps(failure))
    facts, *_ = case("E4 C with B' from X", "c", ["x", "x", "x", "homep"] + ["homep"] * 10,
                     [(ID_H, "h"), (ID_B, "b_marker")])
    expect_rows("E4b", facts, [
        ["entry_recognition", "initial", m_page, False], ["entry_recovery_decision", True],
        ["entry_recovery_package_admitted", digests["b_marker"]],
        ["entry_recognition", "initial", home, False], ["entry_recovery_decision", True],
        ["entry_recovery_package_admitted", digests["h"]],
        ["step_started", 0, "step_01_click"], ["step_finished", 0, "step_01_click"],
        ["entry_recovery_completed", digests["h"], home, 1],
        ["entry_recognition", "post_recovery", home, False],
        ["entry_recovery_failed", digests["b_marker"], UNMATCHED],
        ["entry_target_disposition", "fail_closed", UNMATCHED],
        ["terminal_committed", "failure", 1, UNMATCHED]])
    facts, *_ = case("E4 A from unknown page", "a", ["y"] * 14, h_map)
    expect_rows("E4c", facts, [
        ["entry_recognition", "initial", home, False], ["entry_recovery_decision", True],
        ["entry_recovery_package_admitted", digests["h"]],
        ["entry_recovery_failed", digests["h"], "contained_task_page_unknown"],
        ["entry_target_disposition", "fail_closed", "contained_task_page_unknown"],
        ["terminal_committed", "failure", 0, "contained_task_page_unknown"]])

    # 5. Refusals at preparation (scheduled path): no task.* record, the policy lease released.
    refusals = [
        ("E5 cycle", "p", [("fixture.prereq.p", "p"), ("fixture.prereq.q", "q")], "contained_task_prerequisite_cycle"),
        ("E5 depth", "l0", [(f"fixture.prereq.l{i}", f"l{i}") for i in range(1, 5)], "contained_task_prerequisite_depth_exceeded"),
        ("E5 unbound", "a", [], "contained_task_prerequisite_unbound"),
        ("E5 mismatch", "a", [(ID_H, "b")], "contained_task_prerequisite_mismatch"),
        ("E5 incompatible recognize_only", "a_r", [("fixture.prereq.r", "r")], "contained_task_prerequisite_incompatible"),
        ("E5 incompatible resolution", "a_res", [("fixture.prereq.hres", "h_res")], "contained_task_prerequisite_incompatible"),
        ("E5 step limit", "a_big", [("fixture.prereq.hbig", "h_big")], "contained_task_prerequisite_step_limit"),
        ("E5 self", "a_self", [], "contained_task_control_invalid"),
        ("E5 page-graph declares", "h_field", [], "contained_task_control_invalid"),
        ("R24-1 application entry declares", "x1_declares", h_map, "contained_task_linear_invalid"),
    ]
    for label, main, mapping, code in refusals:
        facts, _root, events, types, text, _ = case(label, main, ["x", "home"], mapping)
        task_events = [event_type for event_type in types if str(event_type).startswith("task.")]
        check(f"{label}.refused_before_any_task_record", code in text and not task_events,
              json.dumps({"code_in_ledger": code in text, "task_events": task_events}))
        if label == "E5 cycle":
            for wanted in ("lease.granted", "lease.released", "runtime.failed", "policy.execution_recorded"):
                check(f"E5.cycle.{wanted}", wanted in types, json.dumps(sorted(set(types))))
        details = sorted(set(re.findall(r"layer=\d+ package_id=[A-Za-z0-9_.]+(?: [a-z_]+=[A-Za-z0-9_.]+)?", text)))
        say(label, "refusal details in ledger", json.dumps(details))
    facts, *_ = case("E5 legacy ZIP main, directory prerequisite", "a_zip", ["x", "x", "home", "home", "home", "a2"], h_map)
    rows = entry_sequence(facts)
    for row in rows:
        say("E5 legacy", "row", json.dumps(row))
    check("E5.legacy_zip_main_directory_prerequisite", rows[-1] == ["terminal_committed", "success", 2, None]
          and ["entry_recovery_package_admitted", digests["h"]] in rows, json.dumps(rows[-1:]))

    # R24 item 2: an application entry package as the prerequisite, on the fixture backend.
    facts, *_ = case("R24-2 application entry prerequisite", "a_x1", ["x"], [("fixture.prereq.x1", "x1")])
    expect_rows("R24-2", facts, [
        ["entry_recognition", "initial", home, False], ["entry_recovery_decision", True],
        ["entry_recovery_package_admitted", digests["x1"]],
        ["entry_recovery_failed", digests["x1"], APPLICATION_REFUSAL],
        ["entry_target_disposition", "fail_closed", APPLICATION_REFUSAL],
        ["terminal_committed", "failure", 0, APPLICATION_REFUSAL]])

    # 7. The existing page-graph home entry: the same normalized facts on v0.9.0, the L2e base and this build.
    for scenario, frames in (("from X", ["x"]), ("from HOME", ["home", "home", "a2", "a2"])):
        sequences = {}
        for build, runtime_dir in (("885947c9", old_runtime), ("fe358b6b", base_runtime), ("product", new_runtime)):
            label = f"E7 {build} page-graph home entry {scenario}"
            facts, *_ = case(label, "pg_home", frames, None, runtime=runtime_dir, keep=build == "product")
            sequences[build] = [normalize({"event_type": event_type, "fact": fact}) for _, event_type, fact in facts]
        for build in ("fe358b6b", "product"):
            same = sequences["885947c9"] == sequences[build] and len(sequences[build]) > 0
            say("E7", scenario, "885947c9", len(sequences["885947c9"]), build, len(sequences[build]))
            check(f"E7.identical.{scenario.replace(' ', '_')}.885947c9_vs_{build}", same, "")
        for index, entry in enumerate(sequences["product"]):
            say("E7", scenario, "normalized fact", index, short(json.dumps(entry), 600))
        last = sequences["product"][-1]["fact"] if sequences["product"] else {}
        wanted = ("success", None) if scenario == "from HOME" else ("failure", "contained_task_home_recovery_binding_missing")
        check(f"E7.product.{scenario.replace(' ', '_')}.terminal", (last.get("outcome"), last.get("failure_code")) == wanted,
              json.dumps([last.get("outcome"), last.get("failure_code")]))

    # 11. Older builds refuse the declaration before PackageAdmitted, and the configuration field.
    for build, runtime_dir in (("885947c9", old_runtime), ("fe358b6b", base_runtime)):
        label = f"E11 {build} declaration"
        facts, _root, events, types, text, _ = case(label, "a", ["home", "home", "a2"], None, runtime=runtime_dir, keep=False)
        admitted = kinds(facts_of(events), "package_admitted")
        check(f"E11.{build}.resource_declaration_invalid", "resource_declaration_invalid" in text
              and "/prerequisite_package_id" in text and not admitted,
              json.dumps({"code": "resource_declaration_invalid" in text, "pointer": "/prerequisite_package_id" in text,
                          "package_admitted": len(admitted)}))
    check_dir = os.path.join(work, "check-config")
    good = write_config(os.path.join(check_dir, "good"), os.path.join(check_dir, "good-state"),
                        state["packs"]["a"]["path"], {"schema_version": SCHEMA_DIR, "sha256": digests["a"]},
                        [state["frames"]["home"]], catalog_dir, prerequisite_entries(state, h_map))
    for build, runtime_dir, wanted in (("885947c9", old_runtime, "config_decode_failed"),
                                       ("fe358b6b", base_runtime, "config_decode_failed"),
                                       ("product", new_runtime, "ok")):
        code, out, err = run_exe([os.path.join(runtime_dir, "actingcommand-actingd.exe"), "check-config", "--config", good])
        say("E11", build, "check-config exit", code, short(out, 600), short(err, 300))
        check(f"E11.{build}.check_config", (wanted == "ok" and code == 0 and '"status":"ok"' in out)
              or (wanted != "ok" and code != 0 and wanted in out), "")
    bad_entries = {
        "prerequisite_package_duplicate": prerequisite_entries(state, [(ID_H, "h"), (ID_H, "h")]),
        "prerequisite_package_id_invalid": [{**prerequisite_entries(state, h_map)[0], "package_id": ""}],
        "prerequisite_package_unavailable": [{**prerequisite_entries(state, h_map)[0],
                                              "package_path": os.path.join(work, "missing", digests["h"])}],
    }
    for wanted, entries in bad_entries.items():
        path = write_config(os.path.join(check_dir, wanted), os.path.join(check_dir, wanted + "-state"),
                            state["packs"]["a"]["path"], {"schema_version": SCHEMA_DIR, "sha256": digests["a"]},
                            [state["frames"]["home"]], catalog_dir, entries)
        code, out, err = run_exe([os.path.join(new_runtime, "actingcommand-actingd.exe"), "check-config", "--config", path])
        say("E11", "product check-config", wanted, "exit", code, short(out, 400))
        check(f"E11.product.check_config.{wanted}", code != 0 and f'"code":"{wanted}"' in out, "")

    # 10. The v0.9.0 actingledger reads every product ledger; the copies stay unchanged.
    for label, root in produced:
        copy = root + "-read-copy"
        shutil.copytree(root, copy)
        before = tree_hashes(copy)
        results = ledger_reads(f"E10 v0.9.0 actingledger on {label}", old_ledger, copy,
                               (("open", ["open"]), ("events", ["events"]),
                                ("export --task-evidence", ["export", "--task-evidence"]), ("export", ["export"])))
        after = tree_hashes(copy)
        slug = label.replace(" ", "_")
        check(f"E10.old_reads.{slug}", all(code == 0 and not corrupt for code, corrupt, _ in results.values()),
              json.dumps({name: [code, corrupt] for name, (code, corrupt, _) in results.items()}))
        check(f"E10.copy_unchanged.{slug}", before == after, f"{len(before)} files")
        events_old, failure = ledger_events(old_ledger, copy)
        events_new, _ = ledger_events(new_ledger, root)
        check(f"E10.old_pages_all_events.{slug}", events_old is not None and events_new is not None
              and len(events_old) == len(events_new) and len(events_new) > 0,
              f"{len(events_old or [])} vs {len(events_new or [])} {failure}")
        if label == "E3 C from X":
            out = results["export"][2]
            section = out.split("effective_configuration:", 1)[-1] if "effective_configuration:" in out else ""
            lines = [line for line in section.splitlines() if line.startswith("- {")]
            check("E10.old_export_validates_five_configuration_records", len(lines) == 5, f"{len(lines)} lines")

    say("RESULT", "failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


# ---------------------------------------------------------------------------------------------
# same: the in-process existing entry recovery runs on 8e0ac191 and on the product.

def same(product_log, base_log):
    def load(path):
        scenarios = {}
        with open(path, encoding="utf-8", errors="replace") as handle:
            for line in handle:
                line = line.strip()
                if not line.startswith("SAME|"):
                    continue
                _, scenario, kind, payload = line.split("|", 3)
                if kind == "event":
                    scenarios.setdefault(scenario, []).append(normalize(json.loads(payload)))
                elif kind == "receipt":
                    scenarios.setdefault(scenario + " receipt", []).append(normalize(json.loads(payload)))
        return scenarios

    product, base = load(product_log), load(base_log)
    say("SAME", "scenarios product", json.dumps(sorted(product)), "8e0ac191", json.dumps(sorted(base)))
    check("SAME.scenarios", sorted(product) == sorted(base) and len(product) >= 8, "")
    for scenario in sorted(set(product) | set(base)):
        old, new = base.get(scenario, []), product.get(scenario, [])
        difference = next((index for index, (a, b) in enumerate(zip(old, new)) if a != b), None)
        say("SAME", scenario, "8e0ac191", len(old), "product", len(new), "first difference", difference)
        if difference is not None:
            say("SAME", scenario, "8e0ac191 at difference", short(json.dumps(old[difference]), 2500))
            say("SAME", scenario, "product at difference", short(json.dumps(new[difference]), 2500))
        check(f"SAME.identical.{scenario.replace(' ', '_')}", len(old) == len(new) and difference is None and len(old) > 0,
              f"{len(old)} vs {len(new)}")
    say("RESULT", "same failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


if __name__ == "__main__":
    command = sys.argv[1]
    if command == "static":
        sys.exit(static(sys.argv[2], sys.argv[3], sys.argv[4]))
    if command == "prepare":
        sys.exit(prepare(sys.argv[2]))
    if command == "same":
        sys.exit(same(sys.argv[2], sys.argv[3]))
    sys.exit(run(*sys.argv[2:9]))
