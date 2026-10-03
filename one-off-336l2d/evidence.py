# One-off (to be reverted), Workflow #336 L2d evidence: the stuck-recovery ladder and the page-graph
# home entry use the configured return-home package (R16-R20 amendment 5955585460 "L2d", ruling
# R23 5955680563, R24 amendment 5958084341 "L2d additions", R25 clarification 5961093808). Every
# printed line starts with "L2D|". Usage:
#   evidence.py static <repo> <base sha> <product sha>
#   evidence.py prepare <work>
#   evidence.py run <work> <new runtime> <new tools> <base runtime> <catalog dir>
#   evidence.py same <product host log> <base host log>
import hashlib
import json
import os
import re
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
}
MARKER = (250, 250, 250)
RESOURCES = {"schema_version": "1.0", "resources": [], "resource_count": 0}
BINDING_MISSING = "contained_task_home_recovery_binding_missing"
INCOMPATIBLE = "contained_task_home_recovery_package_incompatible"
TERMINAL_NON_HOME = "contained_task_home_recovery_terminal_non_home"
FAILURES = []

ID_PG = "fixture.l2d.pg"
ID_H = "fixture.l2d.h"
ID_HL = "fixture.l2d.hl"


def say(*parts):
    print("L2D|" + "|".join(str(part) for part in parts), flush=True)


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
    for y in range(9, 12):
        for x in range(9, 12):
            image.putpixel((x, y), COLORS[name])
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


def source_pack(task_id, package_id, spec, mode, any_of_pages=(), outcome=True):
    """spec: (from, to) per click operation. With `outcome`, a `no_designated_effect`
    scheduling_outcome on the last page, as `record stop` writes it."""
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
            "purpose": f"Return-home fixture step {index + 1}",
            "from": frm,
            "to": to,
            "expect_after": {"page_id": to, "timeout_ms": 500, "interval_ms": 100},
            "post_delay_ms": 50,
            "click": {"kind": "rect", "x": 8, "y": 8, "width": 5, "height": 5},
            "guard": guard(frm),
        })
    probes = [probe(state) for state in states]
    if any_of_pages:
        probes.extend([MARKER_PROBE, ALT_PROBE])
    rules = {}
    for page in pages:
        rule = {"required": [f"state/{state_of(page)}"]}
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
    return {
        "control.json": pretty(control),
        "resources/operations/resources.json": pretty(RESOURCES),
        f"resources/operations/{task_id}/task.json": pretty(task),
    }


def packs():
    return {
        # The page-graph home entry package (required home page through any_of), as in L2c.
        "pg_home": source_pack("rh_pg", ID_PG, [("home", "step_02_a2")], "navigable_route", any_of_pages=("home",)),
        # (a) A page-graph return-home package with a no_designated_effect scheduling_outcome.
        "h": source_pack("rh_h", ID_H, [("x", "home")], "navigable_route"),
        # (b) A Lab-shaped click-only linear return-home package ending on .../step_02_home.
        "hl": source_pack("rh_hl", ID_HL, [("step_01_x", "step_02_home")], "linear_steps"),
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
    for path in ("crates/actingcommand-contract", "crates/execution-kernel", "crates/ledger", "crates/ledger-forensics",
                 "crates/policy", "crates/scheduler", "crates/lab", "crates/resource-tooling", "apps",
                 "tools/actinglab-architecture", "crates/runtime-host/src/tests",
                 "crates/runtime-host/src/host/startup_package.rs", "crates/runtime-host/src/host/policy_outcome.rs",
                 "crates/runtime-host/src/host/policy_dispatch.rs", "crates/runtime-host/src/host.rs"):
        stat = git(repo, "diff", "--stat", base, product, "--", path).strip()
        check(f"STATIC.unchanged.{path}", not stat, stat)
    for path, names in (
            ("crates/runtime-host/src/host/contained_task.rs",
             ("fail_contained_task_entry", "record_geometry_triggered_recovery_failure",
              "record_geometry_triggered_prerequisite_failure", "prepare_contained_task",
              "recover_contained_task", "run_contained_task", "run_scheduled_contained_task",
              "run_startup_package", "begin_contained_run")),
            ("crates/runtime-host/src/host/prerequisite.rs",
             ("return_home_fallback", "resolve_prerequisite_chain", "run_linear_gated", "gate_layer",
              "fail_recognition", "fail_gate", "close_open", "record_closed")),
            ("crates/runtime-host/src/host/recovery_ladder.rs",
             ("admit_recovery_ladder", "climb_recovery_ladder", "recovery_application_restart",
              "recovery_emulator_restart", "run_recovery_rung_package"))):
        before, after = git(repo, "show", f"{base}:{path}"), git(repo, "show", f"{product}:{path}")
        for name in names:
            old, new = function_span(before, name), function_span(after, name)
            check(f"STATIC.function_identical.{os.path.basename(path)}.{name}", old is not None and old == new,
                  "missing" if old is None else "")
    kernel = git(repo, "show", f"{product}:crates/execution-kernel/src/contained_task.rs")
    for name in ("is_entry_recovery_compatible", "is_prerequisite_compatible", "prerequisite_incompatibility",
                 "run_entry_recovery", "run_as_prerequisite", "terminal_matches_required_home"):
        for line in function_span(kernel, name).splitlines():
            say("STATIC", f"CT {name} (unchanged)", line)
    ladder = git(repo, "show", f"{product}:crates/actingcommand-contract/src/event/payload/recovery_ladder.rs")
    for line in function_span(ladder, "is_stuck_recovery_trigger").splitlines():
        say("STATIC", "RLC is_stuck_recovery_trigger (unchanged)", line)
    for path in ("crates/runtime-host/src/host/recovery_ladder.rs", "crates/runtime-host/src/host/contained_task.rs",
                 "crates/runtime-host/src/host/prerequisite.rs"):
        for line in git(repo, "diff", "-U3", base, product, "--", path).splitlines():
            say("STATIC", f"diff {os.path.basename(path)}", line)
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
    for name in COLORS:
        path = os.path.join(work, "frames", name + ".png")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        make_frame(name).save(path, format="PNG")
        state["frames"][name] = path
    save_state(work, state)
    say("prepare", "done", work)
    return 0


# ---------------------------------------------------------------------------------------------
# run: scheduled fixture dispatches (a fixture instance takes no direct task-run and runs no
# stuck-recovery ladder).

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
                 return_home):
    os.makedirs(os.path.join(config_dir, "policy"))
    for name, document in catalog_documents(catalog_dir, 60000).items():
        with open(os.path.join(config_dir, "policy", name + ".json"), "w", encoding="utf-8") as handle:
            json.dump(document, handle, indent=2)
    now = int(time.time() * 1000)
    config = {
        "schema_version": "actingcommand.actingd.config.v1",
        "state_root": state_root,
        "bind_host": "127.0.0.1",
        "bind_port": 0,
        "secret_fingerprint_salt": "oneoff-336-l2d-fixture-salt-value",
        "policy": {
            "facts": {
                "ledger_position": 0, "fact_snapshot_id": "snapshot:oneoff-336-l2d",
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


def daemon(label, runtime_dir, config, state_root, settle_s):
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
    check(f"{label}.daemon_clean", ready and code == 0 and exit_code == 0, f"ready {ready} shutdown {code} exit {exit_code}")
    if exit_code != 0:
        for stream in ("out", "err"):
            with open(os.path.join(run_dir, f"actingd-{stamp}.{stream}"), "rb") as handle:
                say(label, "actingd " + stream, short(handle.read().decode("utf-8", "replace"), 1500))


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


def ref_sha(value):
    if isinstance(value, dict):
        return value.get("sha256")
    if isinstance(value, str):
        return value.split(":", 1)[-1]
    return value


def describe(fact):
    kind = fact["kind"]
    if kind == "terminal_committed":
        return {key: fact.get(key) for key in ("outcome", "final_page", "executed_steps", "failure_code",
                                               "failure_severity")}
    if kind == "package_admitted":
        return {"package_label": fact.get("package_label"), "package_sha256": ref_sha(fact.get("package_sha256"))}
    if kind in ("step_started", "step_finished", "effect_intent", "effect_completed"):
        return {key: fact.get(key) for key in ("step_index", "operation_label", "from_page", "page_label") if key in fact}
    if kind in ("recognition_started", "recognition_completed"):
        return {key: fact.get(key) for key in ("candidate_pages", "matched_page") if key in fact}
    if kind in ("geometry_observed", "evidence_indexed", "capture_completed"):
        return {}
    detail = {key: value for key, value in fact.items() if key != "kind"}
    if "package_sha256" in detail:
        detail["package_sha256"] = ref_sha(detail["package_sha256"])
    return detail


def print_facts(label, facts):
    for sequence, event_type, fact in facts:
        say(label, "fact", sequence, event_type, fact["kind"], short(json.dumps(describe(fact), ensure_ascii=False), 1500))


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
        check(f"{label}.identical.{left}_vs_{right}".replace(" ", "_"),
              len(old) == len(new) and difference is None and len(old) > 0, f"{len(old)} vs {len(new)}")


def run_case(work, label, runtime_dir, ledger, main, frame_names, catalog_dir, return_home_pack):
    """One scheduled dispatch of `main` on the fixture backend; `return_home_pack` (a pack name)
    is the return-home package configured for the fixture game and server, or None."""
    state = load_state(work)
    run_dir = os.path.join(work, "runs", re.sub(r"[^A-Za-z0-9_.-]", "_", label))
    state_root = os.path.join(run_dir, "state")
    os.makedirs(state_root)
    frames = [state["frames"][name] for name in frame_names]
    package_path = state["packs"][main]["path"]
    package_digest = {"schema_version": SCHEMA_DIR, "sha256": state["packs"][main]["digest"]}
    prerequisites, homes = None, None
    if return_home_pack is not None:
        package_id = {"h": ID_H, "hl": ID_HL}[return_home_pack]
        prerequisites = [{"package_id": package_id, "package_path": state["packs"][return_home_pack]["path"],
                          "package_digest": {"schema_version": SCHEMA_DIR,
                                             "sha256": state["packs"][return_home_pack]["digest"]}}]
        homes = [{"game": GAME, "server": SERVER, "package_id": package_id}]
    config = write_config(os.path.join(run_dir, "config"), state_root, package_path, package_digest, frames,
                          catalog_dir, prerequisites, homes)
    say(label, "main", main, "frames", json.dumps(frame_names), "prerequisite_packages", json.dumps(prerequisites),
        "return_home_packages", json.dumps(homes))
    daemon(label, runtime_dir, config, state_root, 20)
    events, failure = ledger_events(ledger, state_root)
    if events is None:
        say(label, "events", "failed", failure)
        check(f"{label}.ledger_readable", False, failure)
        events = []
    all_facts = facts_of(events)
    facts = first_run(all_facts)
    say(label, "ledger events", len(events), "task facts", len(all_facts),
        "terminal facts (runs)", len(kinds(all_facts, "terminal_committed")))
    print_facts(label, facts)
    text = json.dumps(events, ensure_ascii=False)
    for event in events:
        if event.get("event_type") in ("runtime.failed", "policy.execution_recorded", "task.failed",
                                       "runtime.resource_declaration_rejected"):
            say(label, event["event_type"], event["sequence"], short(json.dumps(event.get("payload"), ensure_ascii=False), 1500))
    return facts, text


def run(work, new_runtime, new_tools, base_runtime, catalog_dir):
    state = load_state(work)
    digests = {name: entry["digest"] for name, entry in state["packs"].items()}
    ledger = os.path.join(new_tools, "actingledger.exe")
    home = f"{GAME}/home"
    from_x = ["x", "x", "home", "home", "home", "a2", "a2", "a2"]

    def case(label, runtime_dir, frames, return_home_pack):
        return run_case(work, label, runtime_dir, ledger, "pg_home", frames, catalog_dir, return_home_pack)

    def expected(pack, final_page):
        return [
            ["entry_recognition", "initial", home, False], ["entry_recovery_decision", True],
            ["entry_recovery_package_admitted", digests[pack]],
            ["step_started", 0, "step_01_click"], ["step_finished", 0, "step_01_click"],
            ["entry_recovery_completed", digests[pack], final_page, 1],
            ["entry_recognition", "post_recovery", home, True], ["entry_target_disposition", "started", None],
            ["package_admitted", ID_PG, digests["pg_home"]],
            ["entry_recognition", "initial", home, True],
            ["step_started", 1, "step_01_click"], ["step_finished", 1, "step_01_click"],
            ["terminal_committed", "success", 2, None]]

    # Change 2 / 2': the page-graph home entry package from X, scheduled path, no request binding.
    for item, pack, final_page in (("D-a", "h", home), ("D-b", "hl", f"{GAME}/step_02_home")):
        kind = "page-graph return-home with no_designated_effect outcome" if pack == "h" \
            else "Lab-shaped click-only linear return-home ending on step_02_home"
        facts, text = case(f"{item} product pg_home from X, {kind}", new_runtime, from_x, pack)
        rows = entry_sequence(facts)
        for row in rows:
            say(item, "product row", json.dumps(row, ensure_ascii=False))
        check(f"{item}.product.fact_sequence", rows == expected(pack, final_page),
              json.dumps({"expected": expected(pack, final_page), "actual": rows}, ensure_ascii=False))
        completed = next((index for index, row in enumerate(rows) if row[0] == "entry_recovery_completed"), None)
        check(f"{item}.product.completed_then_post_recovery_true_then_started", completed is not None
              and rows[completed + 1:completed + 3] == [["entry_recognition", "post_recovery", home, True],
                                                        ["entry_target_disposition", "started", None]],
              json.dumps(rows[completed:completed + 3] if completed is not None else None))
        check(f"{item}.product.terminal_success", rows[-1:] == [["terminal_committed", "success", 2, None]],
              json.dumps(rows[-1:]))
        for code in (INCOMPATIBLE, TERMINAL_NON_HOME, BINDING_MISSING):
            check(f"{item}.product.no_{code}", code not in text, "")
        # The same dispatch and configuration on the L2c tip: the page-graph home entry did not use
        # the return-home package there.
        facts, text = case(f"{item} 89e57ae3 pg_home from X, {kind}", base_runtime, from_x, pack)
        rows = entry_sequence(facts)
        for row in rows:
            say(item, "89e57ae3 row", json.dumps(row, ensure_ascii=False))
        check(f"{item}.89e57ae3.binding_missing", rows == [
            ["entry_recognition", "initial", home, False], ["entry_recovery_decision", True],
            ["entry_target_disposition", "fail_closed", BINDING_MISSING],
            ["terminal_committed", "failure", 0, BINDING_MISSING]], json.dumps(rows))

    # Unchanged: from HOME (no recovery needed) with the return-home package configured, and from
    # X with none configured, on 89e57ae3 and on the product.
    for scenario, frames, pack, wanted in (
            ("from HOME, h configured", ["home", "home", "a2", "a2"], "h", ("success", None)),
            ("from X, nothing configured", ["x"], None, ("failure", BINDING_MISSING))):
        sequences = {}
        for build, runtime_dir in (("89e57ae3", base_runtime), ("product", new_runtime)):
            facts, _ = case(f"U {build} pg_home {scenario}", runtime_dir, frames, pack)
            sequences[build] = [normalize({"event_type": event_type, "fact": fact}) for _, event_type, fact in facts]
        compare(f"U {scenario}", sequences, (("89e57ae3", "product"),))
        last = sequences["product"][-1]["fact"] if sequences["product"] else {}
        check(f"U.{scenario.replace(' ', '_').replace(',', '')}.terminal",
              (last.get("outcome"), last.get("failure_code")) == wanted,
              json.dumps([last.get("outcome"), last.get("failure_code")]))

    say("RESULT", "failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


# ---------------------------------------------------------------------------------------------
# same: the in-process request-bound home entry runs on the L2c tip and on the product.

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
                elif kind == "counts":
                    scenarios.setdefault(scenario + " counts", []).append(payload)
        return scenarios

    product, base = load(product_log), load(base_log)
    say("SAME", "scenarios product", json.dumps(sorted(product)), "89e57ae3", json.dumps(sorted(base)))
    check("SAME.scenarios", sorted(product) == sorted(base) and len(product) == 18, f"{len(product)} vs {len(base)}")
    for scenario in sorted(set(product) | set(base)):
        old, new = base.get(scenario, []), product.get(scenario, [])
        if scenario.endswith(" counts"):
            # Capture counts of a timed wait loop vary with timing; inputs do not.
            say("SAME", scenario, "89e57ae3", json.dumps(old), "product", json.dumps(new))
            inputs = [re.sub(r" capture_count=\d+", "", line) for line in old + new]
            check(f"SAME.identical_inputs.{scenario.replace(' ', '_').replace(',', '')}",
                  len(old) == len(new) == 1 and inputs[0] == inputs[1], json.dumps(inputs))
            continue
        say("SAME", scenario, "events before collapsing repeated wait iterations", "89e57ae3", len(old), "product", len(new))
        old, new = collapse(old), collapse(new)
        difference = next((index for index, (a, b) in enumerate(zip(old, new)) if a != b), None)
        say("SAME", scenario, "89e57ae3", len(old), "product", len(new), "first difference", difference)
        if difference is not None:
            say("SAME", scenario, "89e57ae3 at difference", short(json.dumps(old[difference]), 2500))
            say("SAME", scenario, "product at difference", short(json.dumps(new[difference]), 2500))
        check(f"SAME.identical.{scenario.replace(' ', '_').replace(',', '')}",
              len(old) == len(new) and difference is None and len(old) > 0, f"{len(old)} vs {len(new)}")
        if not scenario.endswith(" receipt"):
            rows = [[entry["event_type"], entry["fact"].get("kind"), entry["fact"].get("failure_code"),
                     entry["fact"].get("disposition"), entry["fact"].get("phase"), entry["fact"].get("matched")]
                    for entry in new if isinstance(entry.get("fact"), dict)
                    and str(entry["fact"].get("kind", "")).startswith(("entry_", "terminal_"))]
            for row in rows:
                say("SAME", scenario, "product entry/terminal fact", json.dumps(row))
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
    sys.exit(run(*sys.argv[2:7]))
