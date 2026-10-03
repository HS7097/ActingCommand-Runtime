# One-off (to be reverted), Workflow #336 L2c evidence: the return-home fallback and the failure
# detail of linear_steps packages (R16 amendment 5955585460 "L2c" items 1-10, R24 amendment
# 5958084341 "L2c additions" items 1-2). Every printed line starts with "L2C|". Usage:
#   evidence.py static <repo> <base sha> <product sha>
#   evidence.py prepare <work>
#   evidence.py run <work> <new runtime> <new tools> <base runtime> <old runtime> <old tools> <catalog dir>
#   evidence.py same <product host log> <base host log>
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time

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
    "a3": (40, 200, 200),
    "m": (40, 40, 200),
    "c2": (120, 120, 40),
    "n3": (200, 120, 40),
    "n2": (40, 120, 200),
    "n1": (120, 40, 200),
    "fin": (200, 200, 40),
}
MARKER = (250, 250, 250)
RESOURCES = {"schema_version": "1.0", "resources": [], "resource_count": 0}
APPLICATION_REFUSAL = "application_effect_requires_assigned_application"
RETURN_HOME_UNMATCHED = "contained_task_return_home_entry_unmatched"
LINEAR_UNMATCHED = "contained_task_linear_entry_unmatched"
FAILURES = []

ID_A = "fixture.l2c.a"
ID_H = "fixture.l2c.h"
ID_B = "fixture.l2c.b"
ID_C = "fixture.l2c.c"
ID_A3 = "fixture.l2c.a3"
ID_PGF = "fixture.l2c.pgf"
ID_X = ["fixture.l2c.x0", "fixture.l2c.x1", "fixture.l2c.x2", "fixture.l2c.x3"]
ID_APP = "fixture.l2c.app"
ID_HR = "fixture.l2c.hr"
ID_PG = "fixture.l2c.pg"


def say(*parts):
    print("L2C|" + "|".join(str(part) for part in parts), flush=True)


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
                outcome=True):
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
            "purpose": f"Return-home fixture step {index + 1}",
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
        "goal": f"Return-home fixture {task_id}",
        "coordinate_space": {"width": W, "height": H},
        "defaults": {"template_threshold": 0.95, "color_max_distance": 20, "match_metric": "ccoeff_normed"},
        "timeout_ms": 30000,
        "max_steps": len(operations),
        "entry_page": spec[0][0],
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
        "resolution": {"width": W, "height": H},
        "entry_task_id": task_id,
        "timeout_ms": 30000,
        "step_timeout_ms": 500,
        "capture_interval_ms": 100,
        "max_steps": len(operations),
    }
    if prerequisite is not None:
        control["prerequisite_package_id"] = prerequisite
    return {
        "control.json": pretty(control),
        "resources/operations/resources.json": pretty(RESOURCES),
        f"resources/operations/{task_id}/task.json": pretty(task),
    }


A_SPEC = [("home", "step_02_a2", "click")]
A3_SPEC = [("home", "step_02_a2", "click"), ("step_02_a2", "step_03_a3", "click")]
H_SPEC = [("x", "home", "click")]
B_SPEC = [("home", "step_02_m", "click")]
C_SPEC = [("step_01_m", "step_02_c2", "click")]
RESTART_SPEC = [("any", "home", "restart")]


def packs():
    return {
        "a": source_pack("rh_a", ID_A, A_SPEC),
        "a_marker": source_pack("rh_a", ID_A, A_SPEC, marker_pages=("home",)),
        "h": source_pack("rh_h", ID_H, H_SPEC, mode="navigable_route"),
        "b": source_pack("rh_b", ID_B, B_SPEC),
        "c": source_pack("rh_c", ID_C, C_SPEC, prerequisite=ID_B),
        "a3": source_pack("rh_a3", ID_A3, A3_SPEC),
        "pg_fail": source_pack("rh_pgf", ID_PGF, A3_SPEC, mode="navigable_route"),
        # Item 7: three declared layers X1-X3 and the return-home layer below X3.
        "x0": source_pack("rh_x0", ID_X[0], [("step_01_n1", "step_02_fin", "click")], prerequisite=ID_X[1]),
        "x1": source_pack("rh_x1", ID_X[1], [("step_01_n2", "step_02_n1", "click")], prerequisite=ID_X[2]),
        "x2": source_pack("rh_x2", ID_X[2], [("step_01_n3", "step_02_n2", "click")], prerequisite=ID_X[3]),
        "x3": source_pack("rh_x3", ID_X[3], [("home", "step_02_n3", "click")]),
        # R24: an application entry main package; a restart-style return-home package.
        "app": source_pack("rh_app", ID_APP, RESTART_SPEC),
        "hr": source_pack("rh_hr", ID_HR, RESTART_SPEC),
        # The existing page-graph home entry (required home page through any_of).
        "pg_home": source_pack("rh_pg", ID_PG, A_SPEC, mode="navigable_route", any_of_pages=("home",)),
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
                 "crates/execution-kernel/src/offline.rs", "crates/execution-kernel/src/contained_task/linear.rs",
                 "crates/runtime-host/src/host/recovery_ladder.rs", "crates/runtime-host/src/host/startup_package.rs",
                 "crates/runtime-host/src/host/policy_outcome.rs", "crates/runtime-host/src/host/policy_dispatch.rs",
                 "crates/policy", "crates/ledger", "crates/ledger-forensics", "apps/actingctl", "crates/lab",
                 "crates/resource-tooling", "apps/actingd/src/config/manifest.rs", "apps/actingd/src/check_config.rs",
                 "tools/actinglab-architecture"):
        stat = git(repo, "diff", "--stat", base, product, "--", path).strip()
        check(f"STATIC.unchanged.{path}", not stat, stat)
    for path, names in (
            ("crates/runtime-host/src/host/contained_task.rs",
             ("run_preflighted_contained_task", "record_geometry_triggered_recovery_failure",
              "record_geometry_triggered_prerequisite_failure", "fail_contained_task_entry",
              "startup_package_admission_failure", "resource_reading_failure_detail")),
            ("crates/runtime-host/src/host/prerequisite.rs",
             ("run_linear_gated", "fail_recognition", "fail_gate", "close_open", "record_closed",
              "prerequisite_refusal", "prerequisite_admission_failure")),
            ("crates/execution-kernel/src/contained_task.rs",
             ("recognize_required_home", "run_entry_recovery", "run_with_options", "run_with_collector",
              "required_home_entry_page", "is_entry_recovery_compatible", "is_prerequisite_compatible",
              "prerequisite_incompatibility", "linear_entry_page", "recognize_linear_entry",
              "run_as_prerequisite", "recognize_entry_frame", "capture_page", "capture_until_page",
              "terminal_matches_required_home"))):
        before, after = git(repo, "show", f"{base}:{path}"), git(repo, "show", f"{product}:{path}")
        for name in names:
            old, new = function_span(before, name), function_span(after, name)
            check(f"STATIC.function_identical.{path.split('/')[1]}/{os.path.basename(path)}.{name}", old is not None and old == new,
                  "missing" if old is None else "")
    ladder = git(repo, "show", f"{product}:crates/actingcommand-contract/src/event/payload/recovery_ladder.rs")
    for line in function_span(ladder, "is_stuck_recovery_trigger").splitlines():
        say("STATIC", "RLC is_stuck_recovery_trigger", line)
    check("STATIC.return_home_code_is_no_ladder_trigger",
          not RETURN_HOME_UNMATCHED.startswith(("contained_task_recovery_", "contained_task_home_recovery_"))
          and RETURN_HOME_UNMATCHED != "contained_task_page_unknown", RETURN_HOME_UNMATCHED)
    host = git(repo, "show", f"{product}:crates/runtime-host/src/host/contained_task.rs")
    execute = function_span(host, "execute_contained_task_with_lease")
    branch = execute[execute.index("if let Some(detail) = resource_reading_failure_detail"):]
    branch = branch[:branch.index("let mut failure = RequestFailure::request(")]
    for line in branch.splitlines():
        say("STATIC", "task failure detail branch", line)
    for line in git(repo, "diff", "-U2", base, product, "--", "crates/runtime-host/src/host/contained_task.rs").splitlines():
        say("STATIC", "HCT diff", line)
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
    for name in ("x", "home", "homep") + tuple(state_name for state_name in COLORS if state_name not in ("x", "home")):
        path = os.path.join(work, "frames", name + ".png")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        make_frame(name).save(path, format="PNG")
        state["frames"][name] = path
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


def write_config(config_dir, state_root, package_path, package_digest, frames, catalog_dir, prerequisites,
                 return_home, overwrite=False):
    os.makedirs(os.path.join(config_dir, "policy"), exist_ok=overwrite)
    for name, document in catalog_documents(catalog_dir, 60000).items():
        with open(os.path.join(config_dir, "policy", name + ".json"), "w", encoding="utf-8") as handle:
            json.dump(document, handle, indent=2)
    now = int(time.time() * 1000)
    config = {
        "schema_version": "actingcommand.actingd.config.v1",
        "state_root": state_root,
        "bind_host": "127.0.0.1",
        "bind_port": 0,
        "secret_fingerprint_salt": "oneoff-336-l2c-fixture-salt-value",
        "policy": {
            "facts": {
                "ledger_position": 0, "fact_snapshot_id": "snapshot:oneoff-336-l2c",
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
                "max_inputs": 8,
            },
        }],
    }
    if prerequisites is not None:
        config["prerequisite_packages"] = prerequisites
    if return_home is not None:
        config["return_home_packages"] = return_home
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


def return_home_entries(package_id):
    return [{"game": GAME, "server": SERVER, "package_id": package_id}]


def daemon(label, runtime_dir, config, state_root, settle_s, during=None):
    actingd = os.path.join(runtime_dir, "actingcommand-actingd.exe")
    actingctl = os.path.join(runtime_dir, "actingctl.exe")
    run_dir = os.path.dirname(os.path.dirname(config))
    wait_for_minute_window()
    stamp = str(int(time.time() * 1000))
    with open(os.path.join(run_dir, f"actingd-{stamp}.out"), "wb") as out, \
            open(os.path.join(run_dir, f"actingd-{stamp}.err"), "wb") as err:
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
        result = None
        if ready:
            if during is not None:
                result = during(actingctl, state_root)
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
            with open(os.path.join(run_dir, f"actingd-{stamp}.{stream}"), "rb") as handle:
                say(label, "actingd " + stream, short(handle.read().decode("utf-8", "replace"), 1500))
    return result


def scheduled_run(work, label, runtime_dir, main, frame_names, catalog_dir, mapping, return_home, settle_s=20):
    state = load_state(work)
    run_dir = os.path.join(work, "runs", re.sub(r"[^A-Za-z0-9_.-]", "_", label))
    state_root = os.path.join(run_dir, "state")
    os.makedirs(state_root)
    frames = [state["frames"][name] for name in frame_names]
    package_path = state["packs"][main]["path"]
    package_digest = {"schema_version": SCHEMA_DIR, "sha256": state["packs"][main]["digest"]}
    prerequisites = None if mapping is None else prerequisite_entries(state, mapping)
    homes = None if return_home is None else return_home_entries(return_home)
    config = write_config(os.path.join(run_dir, "config"), state_root, package_path, package_digest, frames,
                          catalog_dir, prerequisites, homes)
    say(label, "main", main, "frames", json.dumps(frame_names), "prerequisite_packages",
        json.dumps(None if mapping is None else [[package_id, name, state["packs"][name]["digest"]] for package_id, name in mapping]),
        "return_home_packages", json.dumps(homes))
    daemon(label, runtime_dir, config, state_root, settle_s)
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


def first_settlement_events(events):
    """The events up to the first dispatch's execution record (its runtime lifecycle failure
    record comes before it)."""
    for index, event in enumerate(events):
        if event.get("event_type") == "policy.execution_recorded":
            return events[:index + 1]
    return events


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


def run_case(work, label, runtime_dir, ledger, main, frame_names, catalog_dir, mapping, return_home):
    root, config = scheduled_run(work, label, runtime_dir, main, frame_names, catalog_dir, mapping, return_home)
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


def lifecycle_details(events):
    """The native detail texts of the runtime lifecycle failure records."""
    found = []

    def walk(value):
        if isinstance(value, dict):
            for key, item in value.items():
                if key == "native_detail" and item is not None:
                    found.append(item if isinstance(item, str) else json.dumps(item, ensure_ascii=False))
                else:
                    walk(item)
        elif isinstance(value, list):
            for item in value:
                walk(item)

    for event in events:
        if event.get("event_type") == "runtime.failed":
            walk(event.get("payload"))
    return found


def mentions(events, needle):
    """The sequence and type of every event whose JSON contains `needle`."""
    return [[event.get("sequence"), event.get("event_type")] for event in events
            if needle in json.dumps(event, ensure_ascii=False)]


def entry_sequence(facts):
    """The entry facts, PackageAdmitted, steps and the terminal, as compact rows."""
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
                                       or key in ("links", "task_timing", "sampling"))
                      else sampled_action(item) if key == "action" else normalize(item))
                for key, item in sorted(value.items())}
    if isinstance(value, list):
        return [normalize(item) for item in value]
    if isinstance(value, str):
        value = re.sub(r"(?<![0-9a-z_])[a-z]+_[0-9a-f]{32}(?![0-9a-z_])", "<identifier>", value)
        return re.sub(r"(?<![0-9a-f])[0-9a-f]{64}(?![0-9a-f])", "<sha256>", value)
    return value


def sampled_action(value):
    """An input action's position is sampled from the click rectangle with a run-seeded
    generator; its kind stays."""
    if isinstance(value, dict):
        return {key: ("<sampled>" if key in ("x", "y", "x1", "y1", "x2", "y2") else normalize(item))
                for key, item in sorted(value.items())}
    return normalize(value)


def collapse(sequence):
    """Removes immediately repeated blocks (a wait loop's iterations), so that runs whose loops
    took a different number of iterations compare by their distinct steps."""
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


def expect_rows(label, facts, expected):
    rows = entry_sequence(facts)
    for row in rows:
        say(label, "row", json.dumps(row, ensure_ascii=False))
    check(f"{label}.fact_sequence", rows == expected,
          json.dumps({"expected": expected, "actual": rows}, ensure_ascii=False))
    return rows


def compare(label, sequences, pairs):
    for left, right in pairs:
        old, new = collapse(sequences[left]), collapse(sequences[right])
        difference = next((index for index, (a, b) in enumerate(zip(old, new)) if a != b), None)
        say(label, left, len(old), right, len(new), "first difference", difference)
        if difference is not None:
            say(label, left, "at difference", short(json.dumps(old[difference]), 2500))
            say(label, right, "at difference", short(json.dumps(new[difference]), 2500))
        check(f"{label}.identical.{left}_vs_{right}".replace(" ", "_"),
              len(old) == len(new) and difference is None and len(old) > 0, f"{len(old)} vs {len(new)}")


def export_records(ledger, root):
    code, out, _ = run_exe([ledger, "--state-root", root, "export"])
    section = out.split("effective_configuration:", 1)[-1] if "effective_configuration:" in out else ""
    records = [json.loads(line[2:]) for line in section.splitlines() if line.startswith("- {")]
    summary = [(record["configuration"]["facts"].get("phase"),
                ref_sha(record["configuration"]["facts"].get("package_sha256"))) for record in records]
    return code, summary


def config_fact_records(actingctl, state_root):
    code, out, err = run_exe([actingctl, "facts", "--program", "--state-root", state_root], timeout=60)
    found = {}

    def walk(value):
        if isinstance(value, dict):
            if value.get("key") in ("config.subsystems", "config.parameters"):
                found[value["key"]] = value
            for item in value.values():
                walk(item)
        elif isinstance(value, list):
            for item in value:
                walk(item)

    try:
        walk(json.loads(out))
    except ValueError:
        pass
    return code, found, short(err, 300)


def run(work, new_runtime, new_tools, base_runtime, old_runtime, old_tools, catalog_dir):
    state = load_state(work)
    digests = {name: entry["digest"] for name, entry in state["packs"].items()}
    new_ledger = os.path.join(new_tools, "actingledger.exe")
    old_ledger = os.path.join(old_tools, "actingledger.exe")
    produced = []
    home, a2_page, m_page = f"{GAME}/home", f"{GAME}/step_02_a2", f"{GAME}/step_01_m"
    h_map = [(ID_H, "h")]

    def case(label, main, frames, mapping, return_home, runtime=new_runtime, ledger=new_ledger, keep=True):
        result = run_case(work, label, runtime, ledger, main, frames, catalog_dir, mapping, return_home)
        if keep:
            produced.append((label, result[1]))
        return result

    # 1. A declares nothing and starts elsewhere: H runs from the configuration, then A.
    facts, _root, events, types, *_ = case("E1 A from X", "a", ["x", "x", "home", "home", "home", "a2"], h_map, ID_H)
    expect_rows("E1", facts, [
        ["entry_recognition", "initial", home, False], ["entry_recovery_decision", True],
        ["entry_recovery_package_admitted", digests["h"]],
        ["step_started", 0, "step_01_click"], ["step_finished", 0, "step_01_click"],
        ["entry_recovery_completed", digests["h"], home, 1],
        ["entry_recognition", "post_recovery", home, True], ["entry_target_disposition", "started", None],
        ["package_admitted", ID_A, digests["a"]],
        ["step_started", 1, "step_01_click"], ["step_finished", 1, "step_01_click"],
        ["terminal_committed", "success", 2, None]])
    check("E1.one_task_requested", types.count("task.requested") == 1, f"task.requested {types.count('task.requested')}")

    # 2. A already on HOME: H does not run.
    facts, *_ = case("E2 A from HOME", "a", ["home", "home", "a2"], h_map, ID_H)
    expect_rows("E2", facts, [
        ["entry_recognition", "initial", home, True], ["entry_recovery_decision", False],
        ["entry_target_disposition", "started", None], ["package_admitted", ID_A, digests["a"]],
        ["step_started", 0, "step_01_click"], ["step_finished", 0, "step_01_click"],
        ["terminal_committed", "success", 1, None]])
    check("E2.no_return_home_package_opened", not kinds(facts, "entry_recovery_package_admitted"), "")

    # 3. H ran but A's first step (an extra marker) never passes.
    facts, _root, events, types, *_ = case("E3 A' from X", "a_marker", ["x", "x"] + ["homep"] * 12, h_map, ID_H)
    expect_rows("E3", facts, [
        ["entry_recognition", "initial", home, False], ["entry_recovery_decision", True],
        ["entry_recovery_package_admitted", digests["h"]],
        ["step_started", 0, "step_01_click"], ["step_finished", 0, "step_01_click"],
        ["entry_recovery_completed", digests["h"], home, 1],
        ["entry_recognition", "post_recovery", home, False],
        ["entry_target_disposition", "fail_closed", RETURN_HOME_UNMATCHED],
        ["terminal_committed", "failure", 1, RETURN_HOME_UNMATCHED]])
    failure = (terminal(facts).get("task_timing") or {}).get("task_failure") or {}
    timing = failure.get("timing") or {}
    check("E3.timing", timing.get("scope") == "page_recognition" and timing.get("stage") == "entry_recognition"
          and timing.get("limit_ms") == 500, json.dumps(failure))
    details = lifecycle_details(events)
    say("E3", "lifecycle native details", json.dumps(details, ensure_ascii=False))
    say("E3", "events mentioning return_home=", json.dumps(mentions(events, "return_home=")))
    wanted = f"layer=0 package_id={ID_A} required_page={home} return_home={ID_H}"
    check("E3.lifecycle_native_detail_names_return_home", any(wanted in detail for detail in details), wanted)
    check("E3.no_ladder", "recovery_ladder" not in json.dumps(types), json.dumps(sorted(set(types))))

    # 4. The chain end falls back: C declares B, B declares nothing -> H, B, C.
    facts, *_ = case("E4 C from X", "c", ["x", "x", "x", "home", "home", "home", "m", "m", "m", "c2"],
                     [(ID_H, "h"), (ID_B, "b")], ID_H)
    expect_rows("E4", facts, [
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

    # 5. No return_home_packages: A from X fails as in v2, on this build and on the L2b tip.
    sequences = {}
    for build, runtime_dir in (("3c009797", base_runtime), ("product", new_runtime)):
        facts, *_ = case(f"E5 {build} A from X without return_home_packages", "a", ["x"] * 12, h_map, None,
                         runtime=runtime_dir, keep=build == "product")
        rows = entry_sequence(facts)
        say("E5", build, "rows", json.dumps(rows))
        check(f"E5.{build}.linear_entry_unmatched", rows and rows[-1][:2] == ["terminal_committed", "failure"]
              and rows[-1][3] == LINEAR_UNMATCHED and not any(row[0].startswith("entry_") for row in rows),
              json.dumps(rows[-1:]))
        sequences[build] = [normalize({"event_type": event_type, "fact": fact}) for _, event_type, fact in facts]
    compare("E5", sequences, (("3c009797", "product"),))

    # 6. Configuration refusals (check-config).
    check_dir = os.path.join(work, "check-config")

    def check_config(name, runtime_dir, mapping, homes, prerequisites_present=True):
        path = write_config(os.path.join(check_dir, name), os.path.join(check_dir, name + "-state"),
                            state["packs"]["a"]["path"], {"schema_version": SCHEMA_DIR, "sha256": digests["a"]},
                            [state["frames"]["home"]], catalog_dir,
                            prerequisite_entries(state, mapping) if prerequisites_present else None, homes)
        code, out, err = run_exe([os.path.join(runtime_dir, "actingcommand-actingd.exe"), "check-config", "--config", path])
        say("E6", name, "check-config exit", code, short(out, 600), short(err, 300))
        return code, out

    code, out = check_config("product-good", new_runtime, h_map, return_home_entries(ID_H))
    check("E6.product.good", code == 0 and '"status":"ok"' in out, "")
    for name, homes, wanted in (
            ("product-unbound", return_home_entries("fixture.l2c.unmapped"), "return_home_package_unbound"),
            ("product-duplicate", return_home_entries(ID_H) * 2, "return_home_package_duplicate"),
            ("product-key-invalid", [{"game": "", "server": SERVER, "package_id": ID_H}], "return_home_package_key_invalid")):
        code, out = check_config(name, new_runtime, h_map, homes)
        check(f"E6.product.{wanted}", code != 0 and f'"code":"{wanted}"' in out and '"stage":"assemble"' in out, "")
    code, out = check_config("3c009797-return-home", base_runtime, h_map, return_home_entries(ID_H))
    check("E6.3c009797.config_decode_failed", code != 0 and '"code":"config_decode_failed"' in out, "")
    code, out = check_config("3c009797-without", base_runtime, h_map, None)
    check("E6.3c009797.same_config_without_the_field_ok", code == 0 and '"status":"ok"' in out, "")
    code, out = check_config("885947c9-return-home", old_runtime, h_map, return_home_entries(ID_H),
                             prerequisites_present=False)
    check("E6.885947c9.config_decode_failed", code != 0 and '"code":"config_decode_failed"' in out, "")

    # 7. The full chain: three declared layers and the return-home layer, seven configuration records.
    chain_frames = ["x"] * 4 + ["x", "home"] + ["home"] + ["home", "n3"] + ["n3"] + ["n3", "n2"] + ["n2"] \
        + ["n2", "n1"] + ["n1"] + ["n1", "fin"]
    mapping = [(ID_X[1], "x1"), (ID_X[2], "x2"), (ID_X[3], "x3"), (ID_H, "h")]
    facts, root, events, types, text, _ = case("E7 X0 full chain from X", "x0", chain_frames, mapping, ID_H)
    n = {name: f"{GAME}/{name}" for name in ("step_01_n1", "step_01_n2", "step_01_n3", "step_02_n1", "step_02_n2",
                                             "step_02_n3")}
    expect_rows("E7", facts, [
        ["entry_recognition", "initial", n["step_01_n1"], False], ["entry_recovery_decision", True],
        ["entry_recovery_package_admitted", digests["x1"]],
        ["entry_recognition", "initial", n["step_01_n2"], False], ["entry_recovery_decision", True],
        ["entry_recovery_package_admitted", digests["x2"]],
        ["entry_recognition", "initial", n["step_01_n3"], False], ["entry_recovery_decision", True],
        ["entry_recovery_package_admitted", digests["x3"]],
        ["entry_recognition", "initial", home, False], ["entry_recovery_decision", True],
        ["entry_recovery_package_admitted", digests["h"]],
        ["step_started", 0, "step_01_click"], ["step_finished", 0, "step_01_click"],
        ["entry_recovery_completed", digests["h"], home, 1],
        ["entry_recognition", "post_recovery", home, True],
        ["step_started", 1, "step_01_click"], ["step_finished", 1, "step_01_click"],
        ["entry_recovery_completed", digests["x3"], n["step_02_n3"], 1],
        ["entry_recognition", "post_recovery", n["step_01_n3"], True],
        ["step_started", 2, "step_01_click"], ["step_finished", 2, "step_01_click"],
        ["entry_recovery_completed", digests["x2"], n["step_02_n2"], 1],
        ["entry_recognition", "post_recovery", n["step_01_n2"], True],
        ["step_started", 3, "step_01_click"], ["step_finished", 3, "step_01_click"],
        ["entry_recovery_completed", digests["x1"], n["step_02_n1"], 1],
        ["entry_recognition", "post_recovery", n["step_01_n1"], True], ["entry_target_disposition", "started", None],
        ["package_admitted", ID_X[0], digests["x0"]],
        ["step_started", 4, "step_01_click"], ["step_finished", 4, "step_01_click"],
        ["terminal_committed", "success", 5, None]])
    code, summary = export_records(new_ledger, root)
    say("E7", "actingledger export exit", code, "effective configuration records", len(summary), json.dumps(summary))
    check("E7.seven_configuration_records", code == 0 and len(summary) == 7
          and [sha for kind, sha in summary if kind == "entry_recovery"]
          == [digests["h"], digests["x3"], digests["x2"], digests["x1"]], json.dumps(summary))
    check("E7.no_limit_failure", "effective_configuration_limit_exceeded" not in text, "")

    # 8. The configuration facts are the same with and without return_home_packages.
    run_dir = os.path.join(work, "runs", "E8_config_facts")
    state_root = os.path.join(run_dir, "state")
    os.makedirs(state_root)
    config_dir = os.path.join(run_dir, "config")
    records = {}
    for variant, homes in (("without", None), ("with", return_home_entries(ID_H))):
        config = write_config(config_dir, state_root, state["packs"]["a"]["path"],
                              {"schema_version": SCHEMA_DIR, "sha256": digests["a"]},
                              [state["frames"][name] for name in ("home", "home", "a2", "a2")], catalog_dir,
                              prerequisite_entries(state, h_map), homes, overwrite=True)
        found = daemon(f"E8 {variant} return_home_packages", new_runtime, config, state_root, 5,
                       during=config_fact_records)
        code, facts_found, err = found if found else (None, {}, "")
        say("E8", variant, "actingctl facts --program exit", code, "records", json.dumps(sorted(facts_found)), err)
        for key, record in sorted(facts_found.items()):
            say("E8", variant, key, short(json.dumps(record, ensure_ascii=False, sort_keys=True), 6000))
        records[variant] = facts_found
    for key in ("config.subsystems", "config.parameters"):
        left, right = records["without"].get(key), records["with"].get(key)
        same_value = left is not None and right is not None and json.dumps(left.get("value"), sort_keys=True) \
            == json.dumps(right.get("value"), sort_keys=True)
        rest = lambda record: {k: v for k, v in (record or {}).items() if k not in ("observed_at_unix_ms",)}
        check(f"E8.{key}.value_bytes_identical", same_value, "")
        check(f"E8.{key}.record_identical_but_observation_time", left is not None and rest(left) == rest(right), "")
    produced.append(("E8 config facts", state_root))

    # 9. The failure detail: A3's second step never passes.
    for build, runtime_dir in (("product", new_runtime), ("3c009797", base_runtime)):
        facts, _root, events, types, *_ = case(f"E9 {build} A3 second step unconfirmed", "a3",
                                               ["home", "a2"] + ["a2"] * 14, None, None,
                                               runtime=runtime_dir, keep=build == "product")
        rows = entry_sequence(facts)
        say("E9", build, "terminal", json.dumps(rows[-1:]))
        details = lifecycle_details(events)
        say("E9", build, "lifecycle native details", json.dumps(details, ensure_ascii=False))
        say("E9", build, "events mentioning after_page=", json.dumps(mentions(events, "after_page=")))
        has = any("operation=step_02_click" in detail and "after_page=" in detail for detail in details)
        check(f"E9.{build}.page_confirmation_failed", rows and rows[-1][3] == "page_confirmation_failed",
              json.dumps(rows[-1:]))
        check(f"E9.{build}.detail_{'carried' if build == 'product' else 'absent'}",
              has if build == "product" else not has, json.dumps(details))
    sequences = {}
    for build, runtime_dir, mapping, homes in (("885947c9", old_runtime, None, None),
                                               ("product", new_runtime, h_map, ID_H)):
        facts, _root, events, types, *_ = case(f"E9 {build} page-graph same scenario", "pg_fail",
                                               ["home", "a2"] + ["a2"] * 24, mapping, homes,
                                               runtime=runtime_dir, keep=build == "product")
        say("E9 page-graph", build, "terminal", json.dumps(entry_sequence(facts)[-1:]))
        compared = []
        for event in first_settlement_events(events):
            kind = event.get("event_type")
            if str(kind).startswith("task.") or kind == "runtime.failed":
                compared.append(normalize({"event_type": kind, "payload": event.get("payload")}))
            elif kind == "policy.execution_recorded":
                compared.append(normalize({"event_type": kind, "payload": event.get("payload")}))
        sequences[build] = compared
        say("E9 page-graph", build, "compared events", len(compared), json.dumps([entry["event_type"] for entry in compared]))
    compare("E9 page-graph", sequences, (("885947c9", "product"),))

    # R24 1. An application entry main package with return_home_packages: no entry fact at all.
    facts, *_ = case("R24-1 application entry main", "app", ["x"], h_map, ID_H)
    rows = entry_sequence(facts)
    for row in rows:
        say("R24-1", "row", json.dumps(row))
    check("R24-1.no_entry_fact", not any(row[0].startswith("entry_") for row in rows), json.dumps(rows))
    check("R24-1.fixture_refusal", rows and rows[-1][:2] == ["terminal_committed", "failure"]
          and rows[-1][3] == APPLICATION_REFUSAL and rows[0][:2] == ["package_admitted", ID_APP], json.dumps(rows))
    check("R24-1.no_capture", not kinds(facts, "capture_completed") and not kinds(facts, "recognition_started"), "")

    # R24 2. A click-first main package with a restart-style return-home package.
    facts, *_ = case("R24-2 restart-style return-home", "a", ["x"], [(ID_HR, "hr")], ID_HR)
    expect_rows("R24-2", facts, [
        ["entry_recognition", "initial", home, False], ["entry_recovery_decision", True],
        ["entry_recovery_package_admitted", digests["hr"]],
        ["entry_recovery_failed", digests["hr"], APPLICATION_REFUSAL],
        ["entry_target_disposition", "fail_closed", APPLICATION_REFUSAL],
        ["terminal_committed", "failure", 0, APPLICATION_REFUSAL]])

    # Same facts: the existing page-graph home entry on v0.9.0, the L2b tip and this build (with
    # return_home_packages configured on this build).
    for scenario, frames in (("from X", ["x"]), ("from HOME", ["home", "home", "a2", "a2"])):
        sequences = {}
        for build, runtime_dir, mapping, homes in (("885947c9", old_runtime, None, None),
                                                   ("3c009797", base_runtime, None, None),
                                                   ("product", new_runtime, h_map, ID_H)):
            facts, *_ = case(f"PG {build} page-graph home entry {scenario}", "pg_home", frames, mapping, homes,
                             runtime=runtime_dir, keep=build == "product")
            sequences[build] = [normalize({"event_type": event_type, "fact": fact}) for _, event_type, fact in facts]
        compare(f"PG {scenario}", sequences, (("885947c9", "product"), ("3c009797", "product")))
        last = sequences["product"][-1]["fact"] if sequences["product"] else {}
        wanted = ("success", None) if scenario == "from HOME" else ("failure", "contained_task_home_recovery_binding_missing")
        check(f"PG.product.{scenario.replace(' ', '_')}.terminal", (last.get("outcome"), last.get("failure_code")) == wanted,
              json.dumps([last.get("outcome"), last.get("failure_code")]))

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
        if label == "E7 X0 full chain from X":
            out = results["export"][2]
            section = out.split("effective_configuration:", 1)[-1] if "effective_configuration:" in out else ""
            lines = [line for line in section.splitlines() if line.startswith("- {")]
            check("E10.old_export_validates_seven_configuration_records", len(lines) == 7, f"{len(lines)} lines")

    say("RESULT", "failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


# ---------------------------------------------------------------------------------------------
# same: the in-process existing entry recovery runs on the L2b tip and on the product.

def same(product_log, base_log):
    def load(path):
        scenarios = {}
        with open(path, encoding="utf-8", errors="replace") as handle:
            for line in handle:
                line = line.strip()
                if "SAME|" not in line:
                    continue
                _, scenario, kind, payload = line[line.index("SAME|"):].split("|", 3)
                if kind == "event":
                    scenarios.setdefault(scenario, []).append(normalize(json.loads(payload)))
                elif kind == "receipt":
                    scenarios.setdefault(scenario + " receipt", []).append(normalize(json.loads(payload)))
        return scenarios

    product, base = load(product_log), load(base_log)
    say("SAME", "scenarios product", json.dumps(sorted(product)), "3c009797", json.dumps(sorted(base)))
    check("SAME.scenarios", sorted(product) == sorted(base) and len(product) >= 8, "")
    for scenario in sorted(set(product) | set(base)):
        old, new = base.get(scenario, []), product.get(scenario, [])
        say("SAME", scenario, "events before collapsing repeated wait iterations", "3c009797", len(old), "product", len(new))
        old, new = collapse(old), collapse(new)
        difference = next((index for index, (a, b) in enumerate(zip(old, new)) if a != b), None)
        say("SAME", scenario, "3c009797", len(old), "product", len(new), "first difference", difference)
        if difference is not None:
            say("SAME", scenario, "3c009797 at difference", short(json.dumps(old[difference]), 2500))
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
