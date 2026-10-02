# One-off (to be reverted), Workflow #336 L2e evidence (R24 amendment 5958084341, "L2e" items 1-8,
# plus the R25 admission refusal). Every printed line starts with "L2E|". Usage:
#   evidence.py static <repo> <l2a sha> <product sha>
#   evidence.py prepare <work>
#   evidence.py run <work> <new runtime> <new tools> <l2a runtime> <old runtime> <old tools> <catalog dir>
import hashlib
import io
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
    "a": (200, 40, 40),
    "b": (40, 200, 40),
    "c": (40, 40, 200),
    "home": (200, 40, 200),
    "launcher": (40, 200, 200),
}
RESOURCES = {"schema_version": "1.0", "resources": [], "resource_count": 0}
APPLICATION_REFUSAL = "application_effect_requires_assigned_application"
FAILURES = []


def say(*parts):
    print("L2E|" + "|".join(str(part) for part in parts), flush=True)


def check(label, condition, detail=""):
    say("CHECK", label, "PASS" if condition else "FAIL", detail)
    if not condition:
        FAILURES.append(label)


def short(text, limit=700):
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


def reference(sha256):
    return json.dumps({"schema_version": SCHEMA_DIR, "sha256": sha256}, separators=(",", ":"))


def write_tree(root, files):
    for path, data in files.items():
        target = os.path.join(root, *path.split("/"))
        os.makedirs(os.path.dirname(target), exist_ok=True)
        with open(target, "xb") as handle:
            handle.write(data)


def make_frame(state):
    image = Image.new("RGB", (W, H), BG)
    if state in COLORS:
        for y in range(9, 12):
            for x in range(9, 12):
                image.putpixel((x, y), COLORS[state])
    return image


def probe(state):
    return {
        "id": f"state/{state}",
        "region": {"mode": "rect", "rect": {"x": 10, "y": 10, "width": 1, "height": 1}},
        "expected": list(COLORS[state]),
    }


def state_of(page):
    return page.rsplit("_", 1)[-1]


def guard(page):
    return {
        "page_id": page,
        "target_id": f"state/{state_of(page)}",
        "expected_rect": {"x": 10, "y": 10, "width": 1, "height": 1},
        "color_probe": f"state/{state_of(page)}",
    }


def app_pack(task_id, spec, mode="linear_steps", mutate=None):
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
            "purpose": f"Application fixture step {index + 1}",
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
    task = {
        "schema_version": "0.9",
        "task_id": task_id,
        "game": GAME,
        "server_scope": [SERVER],
        "locale": "en-US",
        "goal": f"Application fixture {task_id}",
        "coordinate_space": {"width": W, "height": H},
        "defaults": {"template_threshold": 0.95, "color_max_distance": 20, "match_metric": "ccoeff_normed"},
        "timeout_ms": 30000,
        "max_steps": len(operations),
        "entry_page": spec[0][0],
        "target_page": spec[-1][1],
        "color_probes": [probe(state) for state in states],
        "page_rules": {page: {"required": [f"state/{state_of(page)}"]} for page in pages},
        "scheduling_outcome": {"mappings": [{
            "outcome_key": f"{task_id}_done", "effect": "no_designated_effect", "terminal_pages": [spec[-1][1]]}]},
        "operations": operations,
    }
    control = {
        "schema_version": "Lab-1y.control.v2",
        "package_id": f"fixture.application.{task_id}",
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
    extra = {}
    if mutate:
        extra = mutate(task, control) or {}
    files = {
        "control.json": pretty(control),
        "resources/operations/resources.json": pretty(RESOURCES),
        f"resources/operations/{task_id}/task.json": pretty(task),
    }
    for path, data in extra.items():
        files[f"resources/operations/{task_id}/{path}"] = data
    return files


P1 = [("any", "home", "restart"), ("home", "step_02_c", "click")]
P2 = [("step_01_a", "step_02_b", "click"), ("step_02_b", "home", "launch")]
P3 = [("step_01_a", "step_02_b", "click"), ("step_02_b", "step_03_launcher", "stop"), ("step_03_launcher", "home", "launch")]

POLICY = {
    "schema_version": "actingcommand.selection-policy.v1",
    "policy_id": "policy-a",
    "applies_to": {
        "candidate_layout_id": "layout-a",
        "outcome_keys": {
            "selected": "outcome-selected", "empty": "outcome-empty", "insufficient": "outcome-insufficient",
            "ambiguous": "outcome-ambiguous", "unknown": "outcome-unknown",
        },
    },
    "fields": [{"name": "open", "value_type": {"type": "boolean"}}],
    "facts": [],
    "gates": [],
    "scoring": [],
    "selection": {"mode": "top_k", "required_count": 1},
    "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}],
}


def packs():
    def window(task, _control):
        task["operations"][0]["transition"] = {"kind": "window", "min_ms": 2000, "max_ms": 30000}

    def trusted_point(task, _control):
        operation = task["operations"][0]
        operation.pop("guard")
        operation["click"] = {"kind": "point", "x": 10, "y": 10}
        operation["unguarded_trusted_coordinate"] = True

    def retry(task, _control):
        task["operations"][0].update({"retryable": True, "max_attempts": 2, "retry_interval_ms": 100})

    def entry_home(task, _control):
        task["entry_page"] = "home"

    def app_guard(task, _control):
        task["operations"][0]["guard"] = guard("home")

    def select(task, _control):
        policy = json.dumps(POLICY, separators=(",", ":")).encode("utf-8")
        operation = task["operations"][0]
        operation.pop("click")
        operation["select"] = {"layout_id": "layout-a",
                               "policy": {"path": "policies/slots.json", "sha256": hashlib.sha256(policy).hexdigest()}}
        task["candidate_layouts"] = [{
            "id": "layout-a", "page_id": "step_01_a", "kind": "fixed_slots",
            "features": [{"name": "open", "value": "passed"}],
            "slots": [{"rect": {"x": 8, "y": 8, "width": 5, "height": 5},
                       "click": {"x": 8, "y": 8, "width": 5, "height": 5},
                       "targets": {"open": "state/a"}}],
        }]
        return {"policies/slots.json": policy}

    return {
        "p1": app_pack("app_p1", P1),
        "p2": app_pack("app_p2", P2),
        "p3": app_pack("app_p3", P3),
        "p4": app_pack("app_p4", P1, mutate=window),
        "click_only": app_pack("click_only", [("step_01_a", "step_02_b", "click"), ("step_02_b", "step_03_c", "click")]),
        "page_graph_start": app_pack("page_graph_start", [("any", "home", "restart")], mode="navigable_route"),
        "r25_no_home": app_pack("r25_no_home", [("any", "step_02_b", "restart"), ("step_02_b", "step_03_c", "click")]),
        "any_click": app_pack("any_click", [("any", "home", "click"), ("home", "step_02_c", "click")], mutate=trusted_point),
        "any_later": app_pack("any_later", [("step_01_a", "step_02_b", "click"), ("any", "home", "launch")]),
        "app_retry": app_pack("app_retry", P1, mutate=retry),
        "stop_click": app_pack("stop_click", [("step_01_a", "step_02_b", "click"), ("step_02_b", "step_03_launcher", "stop"),
                                              ("step_03_launcher", "step_04_c", "click")]),
        "entry_mismatch": app_pack("entry_mismatch", P1, mutate=entry_home),
        "app_guard": app_pack("app_guard", P1, mutate=app_guard),
        "select_step": app_pack("select_step", [("step_01_a", "step_02_b", "click")], mutate=select),
    }


REFUSALS = [
    # name, expected code(s), expected detail
    ("any_click", "contained_task_linear_invalid", "reason=any_requires_application operation=step_01_click"),
    ("any_later", "contained_task_linear_invalid", "reason=any_from operation=step_02_app"),
    ("app_retry", "contained_task_linear_invalid", "reason=application_retry operation=step_01_app"),
    ("stop_click", "contained_task_linear_invalid", "reason=input_after_application_stop operation=step_03_click"),
    ("entry_mismatch", "contained_task_linear_invalid", "reason=entry_page operation=step_01_app"),
    # Either the source compile or the kernel's operation check refuses it; the code is reported as found.
    ("app_guard", "!ok", ""),
    ("select_step", "contained_task_linear_invalid", "reason=operation_effect operation=step_01_click"),
    ("r25_no_home", "contained_task_linear_invalid", "reason=application_without_home operation=step_01_app"),
]


def load_state(work):
    with open(os.path.join(work, "state.json"), encoding="utf-8") as handle:
        return json.load(handle)


def save_state(work, state):
    with open(os.path.join(work, "state.json"), "w", encoding="utf-8") as handle:
        json.dump(state, handle, indent=2)


# ---------------------------------------------------------------------------------------------
# static: items 6 (zero change, the branch below the capability check) and 7 (linear_application).

def git(repo, *args):
    return subprocess.run(["git", "-C", repo, *args], capture_output=True, check=True).stdout.decode("utf-8")


def function_span(source, signature):
    """The text of the first function whose line contains `fn <signature>`, by brace matching."""
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


def static(repo, l2a, product):
    say("E6", "base", l2a[:8], "product", product[:8])
    for line in git(repo, "diff", "--stat", l2a, product).splitlines():
        say("E6", "diff --stat L2a..product", line)
    for path in ("crates/runtime-host/src/host/contained_task.rs", "crates/runtime-host/src/host/input.rs",
                 "crates/execution-kernel/src/session.rs", "crates/runtime-host/src/provider.rs",
                 "crates/execution-kernel/src/offline.rs", "crates/execution-kernel/src/run.rs",
                 "crates/runtime-host/src/host/foreground_gate.rs"):
        stat = git(repo, "diff", "--stat", l2a, product, "--", path).strip()
        say("E6", "diff", path, "unchanged" if not stat else stat)
        check(f"E6.file_unchanged.{os.path.basename(path)}", not stat, path)
    ct = "crates/execution-kernel/src/contained_task.rs"
    before_source, after_source = git(repo, "show", f"{l2a}:{ct}"), git(repo, "show", f"{product}:{ct}")
    for line in git(repo, "diff", l2a, product, "--", ct).splitlines():
        if line.startswith(("+", "-", "@@")) and not line.startswith(("+++", "---")):
            say("E6", "contained_task.rs diff", line)
    before, after = function_span(before_source, "run_with_collector"), function_span(after_source, "run_with_collector")
    check("E6.run_with_collector_same_lines", sorted(before.splitlines()) == sorted(after.splitlines()),
          "the L2a..product change of run_with_collector only moves lines")
    capability = after.index('"application_effect_requires_assigned_application"')
    branch = after.index("linear::LINEAR_STEPS")
    initial = after.index("let initial_application")
    say("E6", "product run_with_collector offsets", "capability check", capability, "linear branch", branch,
        "initial_application", initial)
    check("E6.linear_branch_after_capability_check_before_initial_application", capability < branch < initial, "")
    removed_text = before_source.replace(before, "")
    check("E6.rest_of_contained_task_identical", removed_text == after_source.replace(after, ""),
          "contained_task.rs outside run_with_collector")
    for name in ("capture_until_page", "capture_page", "capture_frame", "await_postcondition", "finish_success",
                 "task_timeout_error", "wait_post_input_delay", "retry_policy", "guard_outcome", "run_with_options",
                 "run_entry_recovery", "resolve_page_reference", "validate"):
        same = function_span(before_source, name) == function_span(after_source, name)
        check(f"E6.function_identical.{name}", same, name)
    linear = git(repo, "show", f"{product}:crates/execution-kernel/src/contained_task/linear.rs")
    application = function_span(linear, "linear_application")
    for line in application.splitlines():
        say("E7", "linear_application", line)
    completed = function_span(linear, "linear_effect_completed")
    for line in completed.splitlines():
        say("E7", "linear_effect_completed", line)
    check("E7.calls_control_application", ".control_application(action)" in application, "")
    check("E7.writes_effect_completed_only", "EffectCompleted" in completed and "EffectIntent" not in application
          and "EffectIntent" not in completed and "linear_effect_completed" in application, "")
    run = function_span(linear, "run_linear_steps")
    for line in run.splitlines()[:40]:
        say("E7", "run_linear_steps", line)
    any_arm = run.index("LinearFrom::Any => None")
    wait = run.index(".linear_wait(")
    check("E7.entry_any_skips_linear_wait", any_arm < wait and "LinearFrom::Page(page) =>" in run[any_arm:wait], "")
    say("RESULT", "static failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


# ---------------------------------------------------------------------------------------------
# prepare: packages, frames, the kernel harness cases.

def prepare(work):
    os.makedirs(work, exist_ok=False)
    state = {"packs": {}, "frames": {}}
    for name, files in packs().items():
        sha = digest(files)
        path = os.path.join(work, "packs", name, sha)
        write_tree(path, files)
        state["packs"][name] = {"digest": sha, "path": path}
        say("prepare", "pack", name, "digest", sha, "files", len(files))
        for file_path in sorted(files):
            if file_path.endswith("task.json") or file_path == "control.json":
                say("prepare", "pack file", name, file_path, short(files[file_path].decode("utf-8"), 4000))
    for name in ("a", "b", "c", "home", "launcher", "x"):
        path = os.path.join(work, "frames", name + ".png")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        make_frame(name).save(path, format="PNG")
        state["frames"][name] = path
    frames = state["frames"]

    def case(name, **fields):
        pack = state["packs"][name]
        return {"name": name, "locator": pack["path"], "reference": reference(pack["digest"]), **fields}

    refused = {"expect_decision": APPLICATION_REFUSAL, "expect_capture_count": 0}
    admission = [
        case("p1", expect="ok", expect_detail="mode=linear_steps", simulate=[[frames["home"]], []], **refused),
        case("p2", expect="ok", expect_detail="mode=linear_steps", simulate=[[frames["a"]], []], **refused),
        case("p3", expect="ok", expect_detail="mode=linear_steps", simulate=[[frames["a"]]], **refused),
        case("p4", expect="ok", expect_detail="mode=linear_steps", simulate=[[frames["home"]]], **refused),
        case("click_only", expect="ok", expect_detail="mode=linear_steps", simulate=[[frames["a"]]],
             expect_decision="step_01_click", expect_capture_count=1),
        case("page_graph_start", expect="ok", expect_detail="mode=navigable_route", simulate=[[frames["home"]]], **refused),
    ]
    for name, code, detail in REFUSALS:
        admission.append(case(name, expect=code, expect_detail=detail))
    runs = [
        case("p1", expect="success", expect_detail='final_page=Some("fixture-game-a/step_02_c") executed_steps=2',
             frames=[frames["home"], frames["c"]], expect_inputs=1, expect_applications=1),
        case("p1", expect="contained_task_linear_application_unconfirmed",
             expect_detail="operation=step_01_app application=restart attempts=1 transition=none intermediate_seen=false",
             frames=[frames["x"]] * 12, expect_inputs=0, expect_applications=1),
        case("p1", expect="operation_error", expect_detail="injected application failure",
             frames=[frames["home"], frames["c"]], fail_application=True, expect_inputs=0, expect_applications=1),
        case("p1", expect=APPLICATION_REFUSAL, expect_detail="", frames=[frames["home"], frames["c"]],
             supports_application=False, expect_inputs=0, expect_applications=0),
        case("p2", expect="success", expect_detail='final_page=Some("fixture-game-a/home") executed_steps=2',
             frames=[frames["a"], frames["b"], frames["home"]], expect_inputs=1, expect_applications=1),
        case("p3", expect="success", expect_detail='final_page=Some("fixture-game-a/home") executed_steps=3',
             frames=[frames["a"], frames["b"], frames["launcher"], frames["home"]], expect_inputs=1, expect_applications=2),
        case("p4", expect="success", expect_detail='final_page=Some("fixture-game-a/step_02_c") executed_steps=2',
             frames=[frames["home"], frames["c"]], expect_inputs=1, expect_applications=1),
        case("click_only", expect="success", expect_detail='final_page=Some("fixture-game-a/step_03_c") executed_steps=2',
             frames=[frames["a"], frames["b"], frames["c"]], expect_inputs=2, expect_applications=0),
    ]
    for index, run in enumerate(runs):
        run["name"] = f"{run['name']}#{index + 1}"
    with open(os.path.join(work, "cases.json"), "w", encoding="utf-8") as handle:
        json.dump({"admission": admission, "runs": runs}, handle, indent=2)
    # The L2a source (97959309) on the same packages: its admission codes and details.
    l2a = [
        case("p1", expect="contained_task_page_set_invalid", expect_detail="page=any detector_matches=2"),
        case("p2", expect="contained_task_linear_invalid", expect_detail="reason=operation_effect operation=step_02_app"),
        case("click_only", expect="ok", expect_detail="mode=linear_steps", simulate=[[frames["a"]]],
             expect_decision="step_01_click", expect_capture_count=1),
    ]
    with open(os.path.join(work, "cases-l2a.json"), "w", encoding="utf-8") as handle:
        json.dump({"admission": l2a, "runs": []}, handle, indent=2)
    save_state(work, state)
    say("prepare", "done", work)
    return 0


# ---------------------------------------------------------------------------------------------
# run: scheduled runs on the fixture backend (a fixture instance takes no direct task-run), and
# the ledgers both ways.

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


def write_config(config_dir, state_root, package_path, package_digest, frames, catalog_dir):
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
        "secret_fingerprint_salt": "oneoff-336-l2e-fixture-salt-value",
        "policy": {
            "facts": {
                "ledger_position": 0,
                "fact_snapshot_id": "snapshot:oneoff-336-l2e",
                "facts": [], "outcomes": [], "tasks": [],
                "instances": [{
                    "instance_id": ALIAS, "server_id": SERVER, "game_id": GAME,
                    "host_id": "fixture-host-a", "available": True,
                    "capability_operation_ids": ["operation.observe"], "preferred_task_ids": [],
                }],
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
                "frames": [{"width": W, "height": H, "rgb": list(Image.open(frame).convert("RGB").tobytes())} for frame in frames],
                "max_inputs": 4,
            },
        }],
    }
    path = os.path.join(config_dir, "actingd.json")
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(config, handle, separators=(",", ":"))
    return path


def wait_for_minute_window():
    """The interval trigger fires on whole minutes; one run per daemon needs a start early in a
    minute and a shutdown before the next one."""
    second = time.time() % 60
    if second < 2:
        time.sleep(2 - second)
    elif second > 20:
        time.sleep(62 - second)


def scheduled_run(work, label, runtime_dir, package, package_digest, frame_names, catalog_dir, settle_s=20):
    state = load_state(work)
    run_dir = os.path.join(work, "runs", re.sub(r"[^A-Za-z0-9_.-]", "_", label))
    state_root = os.path.join(run_dir, "state")
    os.makedirs(state_root)
    frames = [state["frames"][name] for name in frame_names]
    config = write_config(os.path.join(run_dir, "config"), state_root, package, package_digest, frames, catalog_dir)
    say(label, "frames", json.dumps(frame_names), "package", package, "package_digest", json.dumps(package_digest))
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
        say(label, "daemon ready", ready, "after_s", round(time.time() - started, 1), "second of minute", round(started % 60, 1))
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
                say(label, "actingd " + stream, short(handle.read().decode("utf-8", "replace"), 1200))
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


def kinds(facts, kind):
    return [fact for _, _, fact in facts if fact["kind"] == kind]


def terminal(facts):
    found = kinds(facts, "terminal_committed")
    return found[-1] if found else {}


def print_facts(label, facts):
    for sequence, event_type, fact in facts:
        kind = fact["kind"]
        if kind == "terminal_committed":
            detail = {key: fact.get(key) for key in ("outcome", "final_page", "executed_steps", "failure_code",
                                                      "failure_severity", "scheduling_disposition")}
        elif kind == "package_admitted":
            detail = {key: fact.get(key) for key in ("package_label", "task_label", "package_sha256")}
        elif kind in ("step_started", "step_finished", "effect_intent", "effect_completed",
                      "recognition_started", "recognition_completed"):
            detail = {key: fact.get(key) for key in ("step_index", "operation_label", "from_page", "page_label",
                                                      "candidate_pages", "matched_page") if key in fact}
        else:
            detail = {key: value for key, value in fact.items() if key != "kind"}
        say(label, "fact", sequence, event_type, kind, short(json.dumps(detail, ensure_ascii=False), 1200))


def run_case(work, label, runtime_dir, ledger, pack, frame_names, catalog_dir):
    state = load_state(work)
    entry = state["packs"][pack]
    content = {"schema_version": SCHEMA_DIR, "sha256": entry["digest"]}
    root = scheduled_run(work, label, runtime_dir, entry["path"], content, frame_names, catalog_dir)
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
        if event.get("event_type") in ("task.failed", "task.completed", "runtime.failed", "command.rejected",
                                       "policy.execution_recorded"):
            say(label, event["event_type"], event["sequence"], short(json.dumps(event.get("payload"), ensure_ascii=False), 1600))
    text = json.dumps(events, ensure_ascii=False)
    return facts, root, events, types, text


def ledger_reads(label, ledger, root):
    results = {}
    for name, args in (("open", ["open"]), ("events", ["events"]), ("export --task-evidence", ["export", "--task-evidence"])):
        code, out, err = run_exe([ledger, "--state-root", root, *args])
        corrupt = "corrupt_ledger_record" in out or "corrupt_ledger_record" in err
        say(label, name, "exit", code, "stdout bytes", len(out), "corrupt_ledger_record", corrupt, "stderr", short(err, 300))
        results[name] = (code, corrupt)
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
        return {key: ("<volatile>" if ((VOLATILE.search(key) and key != "target_id") or key in ("links", "task_timing", "sampling")) else normalize(item))
                for key, item in sorted(value.items())}
    if isinstance(value, list):
        return [normalize(item) for item in value]
    if isinstance(value, str):
        return re.sub(r"(?<![0-9a-z_])[a-z]+_[0-9a-f]{32}(?![0-9a-z_])", "<identifier>", value)
    return value


def run(work, new_runtime, new_tools, l2a_runtime, old_runtime, old_tools, catalog_dir):
    new_ledger = os.path.join(new_tools, "actingledger.exe")
    old_ledger = os.path.join(old_tools, "actingledger.exe")
    produced = []

    # 1. P1-P4 admitted, then refused on the fixture backend before any capture.
    for name, frames in (("p1", ["home", "c"]), ("p2", ["a", "b", "home"]), ("p3", ["a", "b", "launcher", "home"]),
                         ("p4", ["home", "c"])):
        label = f"E1 {name}"
        facts, root, events, types, _ = run_case(work, label, new_runtime, new_ledger, name, frames, catalog_dir)
        produced.append((label, root))
        fact_kinds = [fact["kind"] for _, _, fact in facts]
        say(label, "fact kinds of the first run", json.dumps(fact_kinds))
        last = terminal(facts)
        ordered = ("package_admitted" in fact_kinds and "run_started" in fact_kinds
                   and fact_kinds.index("package_admitted") < fact_kinds.index("run_started"))
        check(f"E1.{name}.admitted_then_refused", ordered and last.get("failure_code") == APPLICATION_REFUSAL,
              json.dumps([fact_kinds, last.get("failure_code")]))
        # capture.summary_committed is the terminal's capture summary, written for every run; it
        # is printed so that its counts show that no frame was captured.
        for event in events:
            if event.get("event_type") == "capture.summary_committed":
                say(label, "capture.summary_committed", event["sequence"],
                    short(json.dumps(event.get("payload"), ensure_ascii=False), 1600))
        captures = [t for t in types if str(t).startswith("capture.") and t != "capture.summary_committed"]
        applications = [t for t in types if str(t).startswith("application.")]
        check(f"E1.{name}.no_capture_no_step_no_application",
              not any(kind in ("capture_completed", "step_started", "recognition_started") for kind in fact_kinds)
              and not captures and not applications,
              json.dumps({"capture events": captures, "application events": applications,
                          "all event types": sorted(set(types))}))
    if os.environ.get("L2E_ONLY") == "E1":
        say("RESULT", "failures", len(FAILURES), json.dumps(FAILURES))
        return 1 if FAILURES else 0

    # 2. Admission refusals: no PackageAdmitted.
    for name, code, _detail in REFUSALS:
        label = f"E2 {name}"
        facts, root, events, types, text = run_case(work, label, new_runtime, new_ledger, name, ["a", "home"], catalog_dir)
        produced.append((label, root))
        admitted = kinds(facts_of(events), "package_admitted")
        if code == "!ok":
            found = sorted(set(re.findall(r'"(?:failure_code|code|host_code)":\s*"([a-z0-9_]+)"', text)))
        else:
            found = [candidate for candidate in code.split("|") if candidate in text]
        check(f"E2.{name}.refused_before_package_admitted", bool(found) and not admitted,
              f"codes in ledger {found}; package_admitted facts {len(admitted)}")

    # 4. Older builds refuse before PackageAdmitted.
    for label, runtime_dir, name, expected in (
            ("E4 885947c9 p1", old_runtime, "p1", "contained_task_control_invalid"),
            ("E4 L2a p1", l2a_runtime, "p1", "contained_task_page_set_invalid"),
            ("E4 L2a p2", l2a_runtime, "p2", "contained_task_linear_invalid")):
        facts, root, events, types, text = run_case(work, label, runtime_dir, new_ledger, name, ["a", "home"], catalog_dir)
        produced.append((label, root))
        admitted = kinds(facts_of(events), "package_admitted")
        check(f"{label.replace(' ', '_')}.refused_before_package_admitted", expected in text and not admitted,
              f"{expected} in ledger: {expected in text}; package_admitted facts {len(admitted)}")

    # 6. The minimal page-graph application package: the same normalized facts on all three builds.
    sequences, task_sequences = {}, {}
    for build_label, runtime_dir in (("885947c9", old_runtime), ("L2a", l2a_runtime), ("product", new_runtime)):
        label = f"E6 {build_label} page-graph start"
        facts, root, events, types, _ = run_case(work, label, runtime_dir, new_ledger, "page_graph_start", ["home"], catalog_dir)
        check(f"E6.{build_label}.refused", terminal(facts).get("failure_code") == APPLICATION_REFUSAL,
              json.dumps(terminal(facts).get("failure_code")))
        sequences[build_label] = [normalize({"event_type": event_type, "fact": fact}) for _, event_type, fact in facts]
        task_events = [event for event in events if str(event.get("event_type", "")).startswith("task.")]
        task_sequences[build_label] = [normalize({"event_type": event["event_type"], "payload": event.get("payload")})
                                       for event in task_events]
    for name, collection in (("semantic facts of the run", sequences), ("all task.* events", task_sequences)):
        for build_label in ("L2a", "product"):
            old_seq, new_seq = collection["885947c9"], collection[build_label]
            difference = next((index for index, (a, b) in enumerate(zip(old_seq, new_seq)) if a != b), None)
            same = len(old_seq) == len(new_seq) and difference is None and len(old_seq) > 0
            say("E6", name, "885947c9", len(old_seq), build_label, len(new_seq), "first difference", difference)
            if difference is not None:
                say("E6", name, "885947c9 at difference", short(json.dumps(old_seq[difference]), 2500))
                say("E6", name, f"{build_label} at difference", short(json.dumps(new_seq[difference]), 2500))
            check(f"E6.identical.{name.replace(' ', '_')}.885947c9_vs_{build_label}", same, f"{len(old_seq)} vs {len(new_seq)}")
    for index, entry in enumerate(sequences["product"]):
        say("E6", "normalized fact", index, short(json.dumps(entry), 700))

    # 5. 885947c9 actingledger reads the ledgers of items 1, 2 and 4.
    for label, root in produced:
        copy = root + "-read-copy"
        shutil.copytree(root, copy)
        before = tree_hashes(copy)
        results = ledger_reads(f"E5 v0.9.0 actingledger on {label}", old_ledger, copy)
        after = tree_hashes(copy)
        slug = label.replace(" ", "_")
        check(f"E5.old_reads.{slug}", all(code == 0 and not corrupt for code, corrupt in results.values()), json.dumps(results))
        check(f"E5.copy_unchanged.{slug}", before == after, f"{len(before)} files")
        events_old, failure = ledger_events(old_ledger, copy)
        events_new, _ = ledger_events(new_ledger, root)
        check(f"E5.old_pages_all_events.{slug}", events_old is not None and events_new is not None
              and len(events_old) == len(events_new) and len(events_new) > 0,
              f"{len(events_old or [])} vs {len(events_new or [])} {failure}")

    say("RESULT", "failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


if __name__ == "__main__":
    command = sys.argv[1]
    if command == "static":
        sys.exit(static(sys.argv[2], sys.argv[3], sys.argv[4]))
    if command == "prepare":
        sys.exit(prepare(sys.argv[2]))
    sys.exit(run(*sys.argv[2:9]))
