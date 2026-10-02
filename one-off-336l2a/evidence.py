# One-off (to be reverted), Workflow #336 L2a evidence (frozen model v2 section 8 L2, items 1-7).
# Every printed line starts with "L2|". Usage:
#   evidence.py static <repo> <old sha> <product sha>
#   evidence.py prepare <work>
#   evidence.py build <work> <new tools dir>
#   evidence.py run <work> <new runtime dir> <new tools dir> <old runtime dir> <old tools dir>
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
SCHEMA_JSON = "actingcommand.package.content-json.v1"
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
    "load": (200, 200, 40),
    "off": (224, 225, 227),
    "on": (255, 229, 26),
}
RESOURCES = {"schema_version": "1.0", "resources": [], "resource_count": 0}
FAILURES = []


def say(*parts):
    print("L2|" + "|".join(str(part) for part in parts), flush=True)


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


def write_bytes(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "xb") as handle:
        handle.write(data)


def zip_bytes(files):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        for path in sorted(files, key=lambda item: item.encode("utf-8")):
            info = zipfile.ZipInfo(path, (1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, files[path], compresslevel=9)
    return buffer.getvalue()


def json_container(files):
    document = {
        "schema_version": SCHEMA_JSON,
        "files": {path: files[path].decode("utf-8") for path in sorted(files)},
    }
    return (json.dumps(document, ensure_ascii=False, indent=2) + "\n").encode("utf-8")


def checker():
    image = Image.new("RGB", (6, 6))
    for y in range(6):
        for x in range(6):
            image.putpixel((x, y), (240, 240, 240) if (x // 2 + y // 2) % 2 == 0 else (16, 16, 16))
    return image


def png_bytes(image):
    buffer = io.BytesIO()
    image.save(buffer, format="PNG")
    return buffer.getvalue()


MARK_PNG = png_bytes(checker())


def make_frame(state, mark=False):
    image = Image.new("RGB", (W, H), BG)
    if state in COLORS:
        for y in range(9, 12):
            for x in range(9, 12):
                image.putpixel((x, y), COLORS[state])
    if mark:
        image.paste(checker(), (40, 20))
    return image


def probe(state):
    return {
        "id": f"state/{state}",
        "region": {"mode": "rect", "rect": {"x": 10, "y": 10, "width": 1, "height": 1}},
        "expected": list(COLORS[state]),
    }


def page_name(index, state):
    return f"step_{index + 1:02d}_{state}"


def linear_pack(task_id, states, transition=None, template=False, retry=False, mode="linear_steps", mutate=None):
    pages = [page_name(index, state) for index, state in enumerate(states)]
    used = sorted(set(states) | ({"load"} if transition and transition.get("kind") == "page" else set()))
    page_rules = {}
    for index, (page, state) in enumerate(zip(pages, states)):
        page_rules[page] = {"required": ([ "ui/mark"] if template and index == 0 else []) + [f"state/{state}"]}
    if transition and transition.get("kind") == "page":
        page_rules["transition_01"] = {"required": ["state/load"]}
    operations = []
    for index in range(len(pages) - 1):
        operation = {
            "id": f"step_{index + 1:02d}_click",
            "purpose": f"Linear fixture step {index + 1}",
            "from": pages[index],
            "to": pages[index + 1],
            "click": {"kind": "rect", "x": 8, "y": 8, "width": 5, "height": 5},
            "guard": {
                "page_id": pages[index],
                "target_id": f"state/{states[index]}",
                "expected_rect": {"x": 10, "y": 10, "width": 1, "height": 1},
                "color_probe": f"state/{states[index]}",
            },
            "expect_after": {"page_id": pages[index + 1], "timeout_ms": 500, "interval_ms": 200},
            "post_delay_ms": 50,
        }
        if retry:
            operation.update({"retryable": True, "max_attempts": 2, "retry_interval_ms": 100})
        if transition and index == 0:
            operation["transition"] = transition
        operations.append(operation)
    task = {
        "schema_version": "0.9",
        "task_id": task_id,
        "game": GAME,
        "server_scope": [SERVER],
        "locale": "en-US",
        "goal": f"Linear fixture {task_id}",
        "coordinate_space": {"width": W, "height": H},
        "defaults": {"template_threshold": 0.95, "color_max_distance": 20, "match_metric": "ccoeff_normed"},
        "timeout_ms": 30000,
        "max_steps": len(operations),
        "entry_page": pages[0],
        "target_page": pages[-1],
        "color_probes": [probe(state) for state in used],
        "page_rules": page_rules,
        "scheduling_outcome": {"mappings": [{
            "outcome_key": f"{task_id}_done", "effect": "no_designated_effect", "terminal_pages": [pages[-1]]}]},
        "operations": operations,
    }
    if template:
        task["verify_templates"] = [{
            "id": "ui/mark", "template": "assets/mark.png",
            "region": {"mode": "rect", "rect": {"x": 36, "y": 16, "width": 14, "height": 14}}, "threshold": 0.9}]
    control = {
        "schema_version": "Lab-1y.control.v2",
        "package_id": f"fixture.linear.{task_id}",
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
    if mutate:
        mutate(task, control)
    files = {
        "control.json": pretty(control),
        "resources/operations/resources.json": pretty(RESOURCES),
        f"resources/operations/{task_id}/task.json": pretty(task),
    }
    if template:
        files[f"resources/operations/{task_id}/assets/mark.png"] = MARK_PNG
    return files, task, control


def toggle_pack():
    """The existing-path pack: L1's navigable_route color pack (two pages, one click)."""
    control = {
        "schema_version": "Lab-1y.control.v2", "package_id": "fixture.toggle", "execution_mode": "navigable_route",
        "game": GAME, "server": SERVER, "resolution": {"width": W, "height": H}, "entry_task_id": "toggle",
        "timeout_ms": 30000, "max_steps": 1,
    }
    task = {
        "schema_version": "0.9", "task_id": "toggle", "game": GAME, "server_scope": [SERVER], "locale": "en-US",
        "coordinate_space": {"width": W, "height": H},
        "defaults": {"template_threshold": 0.97, "color_max_distance": 20, "match_metric": "ccoeff_normed"},
        "goal": "Toggle the fixture state once.", "timeout_ms": 30000, "max_steps": 1,
        "entry_page": "toggle_off", "target_page": "toggle_on",
        "color_probes": [
            {"id": "state/off", "region": {"mode": "rect", "rect": {"x": 10, "y": 10, "width": 1, "height": 1}}, "expected": list(COLORS["off"])},
            {"id": "state/on", "region": {"mode": "rect", "rect": {"x": 10, "y": 10, "width": 1, "height": 1}}, "expected": list(COLORS["on"])},
        ],
        "page_rules": {
            "toggle_off": {"required": ["state/off"], "forbidden": ["state/on"]},
            "toggle_on": {"required": ["state/on"], "forbidden": ["state/off"]},
        },
        "scheduling_outcome": {"mappings": [{"outcome_key": "toggle_done", "effect": "no_designated_effect", "terminal_pages": ["toggle_on"]}]},
        "operations": [{
            "id": "toggle_once", "purpose": "Press once from the off state, then require the on state.",
            "from": "toggle_off", "to": "toggle_on", "click": {"kind": "point", "x": 10, "y": 10},
            "guard": {"page_id": "toggle_off", "target_id": "state/off", "expected_rect": {"x": 10, "y": 10, "width": 1, "height": 1}, "color_probe": "state/off"},
            "expect_after": {"page_id": "toggle_on", "timeout_ms": 15000, "interval_ms": 500},
            "retryable": False, "max_attempts": 1, "retry_interval_ms": 1, "post_delay_ms": 200,
        }],
    }
    return {
        "control.json": pretty(control),
        "resources/operations/resources.json": pretty(RESOURCES),
        "resources/operations/toggle/task.json": pretty(task),
    }


def load_state(work):
    with open(os.path.join(work, "state.json"), encoding="utf-8") as handle:
        return json.load(handle)


def save_state(work, state):
    with open(os.path.join(work, "state.json"), "w", encoding="utf-8") as handle:
        json.dump(state, handle, indent=2)


# ---------------------------------------------------------------------------------------------
# static: items 1 (untouched functions) and 4 (input frame context), from the git history.

def git_show(repo, rev, path):
    return subprocess.run(["git", "-C", repo, "show", f"{rev}:{path}"], capture_output=True, check=True).stdout.decode("utf-8")


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


def static(repo, old, product):
    ct = "crates/execution-kernel/src/contained_task.rs"
    for path in ("crates/execution-kernel/src/run.rs", "crates/execution-kernel/src/offline.rs",
                 "crates/execution-kernel/src/contained_task/selection.rs",
                 "crates/execution-kernel/src/contained_task/timing.rs",
                 "crates/runtime-host/src/host/contained_task.rs"):
        result = subprocess.run(["git", "-C", repo, "diff", "--stat", old, product, "--", path], capture_output=True, text=True)
        say("E1", "diff", old[:8], product[:8], path, "unchanged" if not result.stdout.strip() else short(result.stdout, 300))
        check(f"E1.file_unchanged.{os.path.basename(path)}", result.returncode == 0 and not result.stdout.strip(), path)
    old_source = git_show(repo, old, ct)
    new_source = git_show(repo, product, ct)
    for name in ("capture_until_page", "capture_page", "capture_frame", "await_postcondition",
                 "complete_successful_step", "finish_stability_termination", "finish_success",
                 "finish_effect_attempt", "task_timeout_error", "wait_post_input_delay", "retry_policy",
                 "failure_decision", "guard_outcome", "run_with_options", "run_entry_recovery",
                 "recognize_required_home", "validate_recovery", "validate_page_references",
                 "resolve_page_reference", "recognized_page_targets", "input_action"):
        before, after = function_span(old_source, name), function_span(new_source, name)
        same = before is not None and before == after
        say("E1", "function", name, "885947c9 lines", None if before is None else before.count("\n") + 1,
            "sha256", None if before is None else hashlib.sha256(before.encode()).hexdigest()[:16],
            "product sha256", None if after is None else hashlib.sha256(after.encode()).hexdigest()[:16],
            "identical" if same else "DIFFERENT")
        check(f"E1.function_identical.{name}", same, name)
    before, after = function_span(old_source, "run_with_collector"), function_span(new_source, "run_with_collector")
    lines_before, lines_after = before.splitlines(), after.splitlines()
    inserted = [line for line in lines_after if line not in lines_before]
    removed = [line for line in lines_before if line not in lines_after]
    say("E1", "run_with_collector", "removed lines", len(removed), "inserted lines", len(inserted))
    for line in inserted:
        say("E1", "run_with_collector +", line)
    check("E1.run_with_collector_only_inserted_branch", not removed and len(inserted) <= 20, f"{len(removed)} removed")
    diff = subprocess.run(["git", "-C", repo, "diff", old, product, "--", ct], capture_output=True, text=True).stdout
    for line in diff.splitlines():
        if line.startswith(("+", "-", "@@")) and not line.startswith(("+++", "---")):
            say("E1", "contained_task.rs diff", line)
    linear = git_show(repo, product, "crates/execution-kernel/src/contained_task/linear.rs")
    for number, line in enumerate(linear.splitlines(), 1):
        if any(token in line for token in ("committed_input_frame", "input_context", ".input(action", "frame.input_reference")):
            say("E4", "linear.rs", number, line.strip())
    check("E4.committed_input_frame_called", "runtime\n                    .committed_input_frame(reference)" in linear or ".committed_input_frame(reference)" in linear, "")
    check("E4.input_context_passed", ".input(action, observation.input_context.clone())" in linear, "")
    say("RESULT", "static failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


# ---------------------------------------------------------------------------------------------
# prepare: packages, containers, frames, the package-build source repo.

def prepare(work):
    os.makedirs(work, exist_ok=False)
    state = {"packs": {}, "frames": {}}

    def emit(name, files, containers):
        sha = digest(files)
        entry = {"digest": sha, "paths": {}}
        for container in containers:
            if container == "dir":
                path = os.path.join(work, "packs", name, "dir", sha)
                write_tree(path, files)
            elif container == "zip":
                path = os.path.join(work, "packs", name, "zip", sha + ".zip")
                write_bytes(path, zip_bytes(files))
            else:
                path = os.path.join(work, "packs", name, "json", sha + ".json")
                write_bytes(path, json_container(files))
            entry["paths"][container] = path
        state["packs"][name] = entry
        say("prepare", "pack", name, "digest", sha, "containers", ",".join(containers), "files", len(files))
        for path in sorted(files):
            if path.endswith(".json"):
                say("prepare", "pack file", name, path, short(files[path].decode("utf-8"), 4000))
        return sha

    emit("toggle", toggle_pack(), ["dir"])
    three, _, _ = linear_pack("linear_three", ["a", "b", "c"])
    emit("three", three, ["dir"])
    page, page_task, _ = linear_pack("linear_page", ["a", "b"], transition={"kind": "page", "page_id": "transition_01"}, template=True)
    emit("page", page, ["zip", "dir"])
    window, _, _ = linear_pack("linear_window", ["a", "b"], transition={"kind": "window", "min_ms": 1000, "max_ms": 3000})
    emit("window", window, ["json"])
    retry, _, _ = linear_pack("linear_retry", ["a", "b"], retry=True)
    emit("retry", retry, ["dir"])
    retry_page, _, _ = linear_pack("linear_retry_page", ["a", "b"], transition={"kind": "page", "page_id": "transition_01"}, retry=True)
    emit("retry_page", retry_page, ["dir"])

    def loop(task, control):
        task["target_page"] = "step_01_a"
        task["scheduling_outcome"]["mappings"][0]["terminal_pages"] = ["step_01_a"]
        task["operations"][0]["to"] = "step_01_a"
        task["operations"][0]["expect_after"]["page_id"] = "step_01_a"
        del task["page_rules"]["step_02_b"]
    to_from, _, _ = linear_pack("linear_loop", ["a", "b"], mutate=loop)
    emit("to_equals_from", to_from, ["dir"])
    nav, _, _ = linear_pack("nav_transition", ["a", "b"], transition={"kind": "page", "page_id": "transition_01"}, mode="navigable_route")
    emit("navigable_with_transition", nav, ["dir"])
    bad_window, _, _ = linear_pack("linear_bad_window", ["a", "b"], transition={"kind": "window", "min_ms": 3000, "max_ms": 1000})
    emit("window_min_above_max", bad_window, ["json"])
    bad_kind, _, _ = linear_pack("linear_bad_kind", ["a", "b"], transition={"kind": "fade", "min_ms": 1000})
    emit("transition_kind_unknown", bad_kind, ["json"])

    # The package-build source repo: the same task as the ZIP page pack, plus an unrelated task the
    # selected build leaves out.
    repo = os.path.join(work, "repo")
    write_bytes(os.path.join(repo, "operations", "resources.json"), pretty(RESOURCES))
    write_bytes(os.path.join(repo, "operations", "linear_page", "task.json"), pretty(page_task))
    write_bytes(os.path.join(repo, "operations", "linear_page", "assets", "mark.png"), MARK_PNG)
    _, other_task, _ = linear_pack("other_task", ["c", "off"])
    write_bytes(os.path.join(repo, "operations", "other_task", "task.json"), pretty(other_task))
    write_bytes(os.path.join(repo, "navigation", f"{GAME}.{SERVER}.navigation.json"),
                pretty({"schema_version": "0.3", "control_points": [{"name": "home", "point": [1, 1]}]}))
    state["repo"] = repo

    for name, (frame_state, mark) in {
        "a": ("a", False), "a_mark": ("a", True), "b": ("b", False), "c": ("c", False),
        "load": ("load", False), "x": (None, False), "off": ("off", False), "on": ("on", False),
    }.items():
        path = os.path.join(work, "frames", name + ".png")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        make_frame(frame_state, mark).save(path, format="PNG")
        state["frames"][name] = path
    save_state(work, state)
    say("prepare", "done", work)
    return 0


# ---------------------------------------------------------------------------------------------
# helpers for binaries

def run_exe(args, timeout=300, cwd=None):
    try:
        result = subprocess.run(args, capture_output=True, timeout=timeout, cwd=cwd)
    except subprocess.TimeoutExpired:
        return None, "", "timeout"
    return result.returncode, result.stdout.decode("utf-8", "replace"), result.stderr.decode("utf-8", "replace")


def lab_json(exe, args):
    code, out, err = run_exe([exe, "--json", *args])
    try:
        value = json.loads(out)
    except ValueError:
        value = None
    return code, value, out, err


# ---------------------------------------------------------------------------------------------
# build: item 7 (package build --execution-mode linear_steps) and the game-prefixed legacy ZIP.

def build_task(lab, repo, task_id, out_dir, label):
    """Builds one task; returns the published ZIP copied to a stable path, or None."""
    os.makedirs(out_dir, exist_ok=True)
    out = os.path.join(out_dir, task_id + ".zip")
    code, value, raw, err = lab_json(lab, ["package", "build-task", "--repo", repo, "--task", task_id,
                                           "--game", GAME, "--server", SERVER, "--locale", "en-US",
                                           "--execution-mode", "linear_steps", "--out", out])
    data = (value or {}).get("data") or {}
    say(label, "package build-task --execution-mode linear_steps", task_id, "exit", code, "status", data.get("status"),
        "execution_mode", data.get("execution_mode"), "included_tasks", data.get("included_tasks"), "out", data.get("out"),
        "validation.control", json.dumps((data.get("validation") or {}).get("control")),
        "error", short(json.dumps((value or {}).get("error")) if value else (raw or err), 900))
    check(f"{label}.build_written.{task_id}", code == 0 and data.get("status") == "written", data.get("status"))
    # The logical output is published through the package publication state; the bytes are the
    # newest ZIP under the output directory.
    found = []
    for folder, _dirs, names in os.walk(out_dir):
        for name in names:
            if name.endswith(".zip"):
                path = os.path.join(folder, name)
                found.append((os.path.getmtime(path), path))
    say(label, "ZIP files under the output directory", json.dumps([path for _, path in sorted(found)]))
    if not found:
        return None
    published = sorted(found)[-1][1]
    stable = os.path.join(os.path.dirname(out_dir), task_id + "-published.zip")
    shutil.copyfile(published, stable)
    return stable


def build(work, new_tools):
    state = load_state(work)
    lab = os.path.join(new_tools, "actinglab.exe")
    out = build_task(lab, state["repo"], "linear_page", os.path.join(work, "built", "page"), "E7")
    if out is None:
        check("E7.published_zip_found", False, "")
    else:
        with zipfile.ZipFile(out) as archive:
            names = archive.namelist()
            say("E7", "built ZIP entries", json.dumps(names))
            control = json.loads(archive.read("control.json"))
            say("E7", "built control.json", json.dumps(control))
            check("E7.control_linear_steps", control.get("execution_mode") == "linear_steps", control.get("execution_mode"))
            task_name = next(name for name in names if name.endswith("linear_page/task.json"))
            task = json.loads(archive.read(task_name))
            say("E7", "built task.json page_rules", json.dumps(task.get("page_rules")))
            say("E7", "built task.json operation transition", json.dumps([op.get("transition") for op in task["operations"]]))
            check("E7.selected_task_keeps_transition_page_rule", "transition_01" in (task.get("page_rules") or {}),
                  json.dumps(sorted(task.get("page_rules") or {})))
            page_sets = 0
            for name in names:
                if name.endswith(".json") and "task.json" not in name and "page" in name.rsplit("/", 1)[-1].lower():
                    page_sets += 1
                    pages = json.loads(archive.read(name))
                    ids = [page.get("id") for page in pages.get("pages", [])] if isinstance(pages, dict) else None
                    say("E7", "built page set", name, short(json.dumps(pages), 2500))
                    if ids is not None:
                        check("E7.page_set_has_transition_page_and_only_the_selected_task",
                              f"{GAME}/transition_01" in ids and not any("step_01_c" in str(i) or "step_02_off" in str(i) for i in ids),
                              json.dumps(ids))
            check("E7.page_set_found", page_sets > 0, str(page_sets))
        built_sha = hashlib.sha256(open(out, "rb").read()).hexdigest()
        state["built"] = {"path": out, "sha256": built_sha}
        say("E7", "built ZIP sha256", built_sha)

    # A three-step package whose first `to` is written with the game prefix and whose second `from`
    # is not: the content-directory source parser derives page ids from the declared names, so the
    # case is a built (legacy ZIP) package, edited after the build and re-sealed in its manifest.
    three_repo = os.path.join(work, "repo-three")
    _, three_task, _ = linear_pack("linear_three", ["a", "b", "c"])
    write_bytes(os.path.join(three_repo, "operations", "resources.json"), pretty(RESOURCES))
    write_bytes(os.path.join(three_repo, "operations", "linear_three", "task.json"), pretty(three_task))
    write_bytes(os.path.join(three_repo, "navigation", f"{GAME}.{SERVER}.navigation.json"),
                pretty({"schema_version": "0.3", "control_points": [{"name": "home", "point": [1, 1]}]}))
    built_three = build_task(lab, three_repo, "linear_three", os.path.join(work, "built", "three"), "E3")
    if built_three is not None:
        with zipfile.ZipFile(built_three) as archive:
            entries = {info.filename: archive.read(info.filename) for info in archive.infolist() if not info.filename.endswith("/")}
        task_name = next(name for name in entries if name.endswith("linear_three/task.json"))
        task = json.loads(entries[task_name])
        before = json.dumps(task["operations"][0].get("to"))
        task["operations"][0]["to"] = f"{GAME}/step_02_b"
        task["operations"][0]["expect_after"]["page_id"] = f"{GAME}/step_02_b"
        say("E3", "prefixed edit", task_name, "op0.to", before, "->", json.dumps(task["operations"][0]["to"]),
            "op1.from", json.dumps(task["operations"][1]["from"]))
        entries[task_name] = (json.dumps(task, indent=2) + "\n").encode("utf-8")
        manifest = json.loads(entries["resources/manifest.json"])
        relative = task_name[len("resources/"):]
        resealed = 0
        for item in manifest["files"]:
            if item["path"] == relative:
                item["sha256"] = "sha256:" + hashlib.sha256(entries[task_name]).hexdigest()
                resealed += 1
        check("E3.prefixed_manifest_resealed", resealed == 1, str(resealed))
        entries["resources/manifest.json"] = (json.dumps(manifest, indent=2) + "\n").encode("utf-8")
        prefixed = os.path.join(work, "built", "linear_three_prefixed.zip")
        write_bytes(prefixed, zip_bytes(entries))
        state["prefixed"] = {"path": prefixed, "sha256": hashlib.sha256(open(prefixed, "rb").read()).hexdigest()}

    packs, frames = state["packs"], state["frames"]
    admission = [
        {"name": "three-dir", "locator": packs["three"]["paths"]["dir"], "reference": reference(packs["three"]["digest"]),
         "expect": "ok", "expect_detail": "mode=linear_steps", "first_frame": frames["a"], "expect_decision": "step_01_click"},
        {"name": "page-zip", "locator": packs["page"]["paths"]["zip"], "reference": reference(packs["page"]["digest"]),
         "expect": "ok", "expect_detail": "mode=linear_steps", "first_frame": frames["a_mark"], "expect_decision": "step_01_click"},
        {"name": "window-json", "locator": packs["window"]["paths"]["json"], "reference": reference(packs["window"]["digest"]),
         "expect": "ok", "expect_detail": "required_home_entry_page=None", "first_frame": frames["a"], "expect_decision": "step_01_click"},
        {"name": "to-equals-from", "locator": packs["to_equals_from"]["paths"]["dir"], "reference": reference(packs["to_equals_from"]["digest"]),
         "expect": "contained_task_linear_invalid", "expect_detail": "reason=to_equals_from operation=step_01_click"},
        {"name": "navigable-route-with-transition", "locator": packs["navigable_with_transition"]["paths"]["dir"],
         "reference": reference(packs["navigable_with_transition"]["digest"]),
         "expect": "contained_task_operation_invalid", "expect_detail": "transition requires linear_steps"},
        {"name": "window-min-above-max", "locator": packs["window_min_above_max"]["paths"]["json"],
         "reference": reference(packs["window_min_above_max"]["digest"]),
         "expect": "contained_task_linear_invalid", "expect_detail": "reason=transition_window"},
        {"name": "transition-kind-unknown", "locator": packs["transition_kind_unknown"]["paths"]["json"],
         "reference": reference(packs["transition_kind_unknown"]["digest"]),
         "expect": "resource_declaration_invalid", "expect_detail": "/operations/0/transition/kind"},
    ]
    if "built" in state:
        admission.append({"name": "built-linear-page", "locator": state["built"]["path"], "reference": state["built"]["sha256"],
                          "expect": "ok", "expect_detail": "mode=linear_steps", "first_frame": frames["a_mark"], "expect_decision": "step_01_click"})
    if "prefixed" in state:
        admission.append({"name": "prefixed-to-legacy-zip", "locator": state["prefixed"]["path"], "reference": state["prefixed"]["sha256"],
                          "expect": "ok", "expect_detail": "mode=linear_steps", "first_frame": frames["a"], "expect_decision": "step_01_click"})
    with open(os.path.join(work, "cases.json"), "w", encoding="utf-8") as handle:
        json.dump({"admission": admission}, handle, indent=2)
    save_state(work, state)
    say("RESULT", "build failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


# ---------------------------------------------------------------------------------------------
# run: scheduled runs on the fixture backend (a fixture instance takes no direct task-run), and
# the ledgers both ways.

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


def write_config(config_dir, state_root, package_path, package_digest, frames, max_inputs, catalog_dir):
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
        "secret_fingerprint_salt": "oneoff-336-l2a-fixture-salt-value",
        "policy": {
            "facts": {
                "ledger_position": 0,
                "fact_snapshot_id": "snapshot:oneoff-336-l2a",
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
                "max_inputs": max_inputs,
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


def scheduled_run(work, label, runtime_dir, package, package_digest, frame_names, max_inputs, catalog_dir, settle_s=20):
    state = load_state(work)
    run_dir = os.path.join(work, "runs", re.sub(r"[^A-Za-z0-9_.-]", "_", label))
    state_root = os.path.join(run_dir, "state")
    os.makedirs(state_root)
    frames = [state["frames"][name] for name in frame_names]
    config = write_config(os.path.join(run_dir, "config"), state_root, package, package_digest, frames, max_inputs, catalog_dir)
    say(label, "config bytes", os.path.getsize(config), "frames", len(frames), json.dumps(frame_names), "max_inputs", max_inputs,
        "package", package, "package_digest", json.dumps(package_digest))
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
        code, shutdown_out, shutdown_err = run_exe([actingctl, "request-shutdown", "--state-root", state_root, "--wait", "60"], timeout=120)
        say(label, "request-shutdown exit", code, short(shutdown_err, 300))
        try:
            exit_code = process.wait(timeout=120)
        except subprocess.TimeoutExpired:
            process.kill()
            exit_code = "killed"
    say(label, "actingd exit", exit_code, "second of minute at exit", round(time.time() % 60, 1))
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
    """The facts up to and including the first terminal fact: one dispatched run."""
    for index, (_, _, fact) in enumerate(facts):
        if fact["kind"] == "terminal_committed":
            return facts[:index + 1]
    return facts


def find_key(value, key):
    if isinstance(value, dict):
        for name, item in value.items():
            if name == key:
                yield item
            yield from find_key(item, key)
    elif isinstance(value, list):
        for item in value:
            yield from find_key(item, key)


def print_facts(label, facts):
    for sequence, event_type, fact in facts:
        kind = fact["kind"]
        if kind in ("recognition_started", "recognition_completed"):
            detail = {"candidate_pages": fact.get("candidate_pages"), "matched_page": fact.get("matched_page")}
        elif kind in ("step_started", "step_finished", "effect_intent", "effect_completed"):
            detail = {key: fact.get(key) for key in ("step_index", "operation_label", "from_page", "page_label", "phase") if key in fact}
            if kind == "effect_intent":
                detail["action"] = fact.get("action")
        elif kind == "terminal_committed":
            detail = {key: fact.get(key) for key in ("outcome", "final_page", "executed_steps", "failure_code", "scheduling_disposition")}
            timing = fact.get("task_timing") or {}
            detail["task_failure"] = timing.get("task_failure")
            for boundary in ("post_input_wait", "postcondition_wait", "page_recognition_wait", "retry_wait"):
                detail[boundary] = [{key: summary.get(key) for key in ("status", "attempts", "total_us", "max_us")}
                                    for summary in find_key(timing, boundary) if isinstance(summary, dict)]
        elif kind == "package_admitted":
            detail = {key: fact.get(key) for key in ("package_label", "task_label", "package_sha256")}
        else:
            detail = {key: value for key, value in fact.items() if key != "kind"}
        say(label, "fact", sequence, event_type, kind, short(json.dumps(detail, ensure_ascii=False), 1500))


def kinds(facts, kind):
    return [fact for _, _, fact in facts if fact["kind"] == kind]


def terminal(facts):
    found = kinds(facts, "terminal_committed")
    return found[-1] if found else {}


def run_case(work, label, runtime_dir, ledger, package, package_digest, frame_names, max_inputs, catalog_dir):
    root = scheduled_run(work, label, runtime_dir, package, package_digest, frame_names, max_inputs, catalog_dir)
    events, failure = ledger_events(ledger, root)
    if events is None:
        say(label, "events", "failed", failure)
        events = []
    all_facts = facts_of(events)
    runs = len(kinds(all_facts, "terminal_committed"))
    facts = first_run(all_facts)
    say(label, "ledger events", len(events), "task facts", len(all_facts), "terminal facts (runs)", runs)
    print_facts(label, facts)
    for event in events:
        if event.get("event_type") in ("task.completed", "task.failed", "policy.execution_recorded"):
            say(label, event["event_type"], event["sequence"], short(json.dumps(event.get("payload"), ensure_ascii=False), 1500))
    code = terminal(facts).get("failure_code")
    text = json.dumps(events, ensure_ascii=False)
    export_code, export_out, export_err = run_exe([ledger, "--state-root", root, "export", "--task-evidence"])
    if code:
        for line in export_out.splitlines():
            if code in line:
                say(label, "task evidence naming the failure", short(line, 1500))
    return facts, root, events, text + export_out


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
        return {key: ("<volatile>" if (VOLATILE.search(key) or key in ("links", "task_timing", "sampling")) else normalize(item))
                for key, item in sorted(value.items())}
    if isinstance(value, list):
        return [normalize(item) for item in value]
    if isinstance(value, str):
        return re.sub(r"(?<![0-9a-z_])[a-z]+_[0-9a-f]{32}(?![0-9a-z_])", "<identifier>", value)
    return value


def completed(facts, executed_steps, final_page=None):
    last = terminal(facts)
    return (last.get("outcome") == "success" and last.get("executed_steps") == executed_steps
            and (final_page is None or last.get("final_page") == final_page))


def run(work, new_runtime, new_tools, old_runtime, old_tools, catalog_dir):
    state = load_state(work)
    packs = state["packs"]
    new_ledger = os.path.join(new_tools, "actingledger.exe")
    old_ledger = os.path.join(old_tools, "actingledger.exe")
    produced = []

    def content(name):
        return {"schema_version": SCHEMA_DIR, "sha256": packs[name]["digest"]}

    # 1. The existing navigable_route pack: the same facts on 885947c9 and on the product build.
    toggle = packs["toggle"]
    sequences, task_sequences = {}, {}
    for build_label, runtime_dir in (("885947c9", old_runtime), ("product", new_runtime)):
        facts, root, events, _ = run_case(work, f"E1 {build_label} navigable_route", runtime_dir, new_ledger,
                                          toggle["paths"]["dir"], content("toggle"), ["off", "on", "on", "on"], 2, catalog_dir)
        check(f"E1.{build_label}.completed", completed(facts, 1, f"{GAME}/toggle_on"), json.dumps(terminal(facts).get("outcome")))
        sequences[build_label] = [normalize({"event_type": event_type, "fact": fact}) for _, event_type, fact in facts]
        task_events = [event for event in events if str(event.get("event_type", "")).startswith("task.")]
        task_sequences[build_label] = [normalize({"event_type": event["event_type"], "payload": event.get("payload")}) for event in task_events]
        if build_label == "product":
            produced.append(("E1 product navigable_route", root))
    for name, collection in (("semantic facts of the run", sequences), ("all task.* events", task_sequences)):
        old_seq, new_seq = collection["885947c9"], collection["product"]
        first_difference = next((index for index, (a, b) in enumerate(zip(old_seq, new_seq)) if a != b), None)
        say("E1", name, "885947c9", len(old_seq), "product", len(new_seq), "first difference", first_difference)
        if first_difference is not None:
            say("E1", name, "885947c9 at difference", short(json.dumps(old_seq[first_difference]), 2500))
            say("E1", name, "product at difference", short(json.dumps(new_seq[first_difference]), 2500))
        if name == "semantic facts of the run":
            check("E1.identical_normalized_facts", len(old_seq) == len(new_seq) and first_difference is None and len(old_seq) > 0,
                  f"{len(old_seq)} vs {len(new_seq)}")
            for index, entry in enumerate(new_seq):
                say("E1", "normalized fact", index, short(json.dumps(entry), 700))
        else:
            say("E1", "all task.* events identical after normalization", len(old_seq) == len(new_seq) and first_difference is None)

    # 2. Three containers x three intermediate kinds.
    facts, root, _, _ = run_case(work, "E2 dir none", new_runtime, new_ledger, packs["three"]["paths"]["dir"], content("three"),
                                 ["a", "b", "c"], 4, catalog_dir)
    produced.append(("E2 dir none", root))
    check("E2.dir_none.completed", completed(facts, 2, f"{GAME}/step_03_c"), json.dumps(terminal(facts)))
    starts = kinds(facts, "step_started")
    check("E2.dir_none.from_pages_are_detector_ids", [s.get("from_page") for s in starts] == [f"{GAME}/step_01_a", f"{GAME}/step_02_b"],
          json.dumps(starts))
    candidates = [fact.get("candidate_pages") for fact in kinds(facts, "recognition_started")]
    check("E2.dir_none.single_candidates", candidates == [[f"{GAME}/step_01_a"], [f"{GAME}/step_02_b"], [f"{GAME}/step_03_c"]],
          json.dumps(candidates))

    facts, root, _, _ = run_case(work, "E2 zip page", new_runtime, new_ledger, packs["page"]["paths"]["zip"], content("page"),
                                 ["a_mark", "load", "b"], 4, catalog_dir)
    produced.append(("E2 zip page", root))
    check("E2.zip_page.completed", completed(facts, 1, f"{GAME}/step_02_b"), json.dumps(terminal(facts)))
    candidates = [fact.get("candidate_pages") for fact in kinds(facts, "recognition_completed")]
    matched = [fact.get("matched_page") for fact in kinds(facts, "recognition_completed")]
    check("E2.zip_page.intermediate_seen", candidates == [[f"{GAME}/step_01_a"], [f"{GAME}/transition_01"], [f"{GAME}/step_02_b"]]
          and matched == [f"{GAME}/step_01_a", f"{GAME}/transition_01", f"{GAME}/step_02_b"], json.dumps([candidates, matched]))

    facts, root, _, _ = run_case(work, "E2 json window", new_runtime, new_ledger, packs["window"]["paths"]["json"], content("window"),
                                 ["a", "b"], 4, catalog_dir)
    produced.append(("E2 json window", root))
    check("E2.json_window.completed", completed(facts, 1, f"{GAME}/step_02_b"), json.dumps(terminal(facts)))
    summaries = [summary for summary in find_key(terminal(facts).get("task_timing") or {}, "post_input_wait") if isinstance(summary, dict)]
    numbers = [summary.get(key) for summary in summaries for key in ("max_us", "total_us")]
    say("E2", "json window post_input_wait summaries", json.dumps(summaries))
    check("E2.json_window.post_input_wait_at_least_1000ms", any(isinstance(n, int) and n >= 1_000_000 for n in numbers), json.dumps(numbers))

    # 3. Failures and retries.
    facts, root, _, _ = run_case(work, "E3 entry unmatched", new_runtime, new_ledger, packs["three"]["paths"]["dir"], content("three"),
                                 ["x"] * 12, 4, catalog_dir)
    produced.append(("E3 entry unmatched", root))
    check("E3.entry_unmatched", terminal(facts).get("failure_code") == "contained_task_linear_entry_unmatched"
          and terminal(facts).get("executed_steps") == 0 and not kinds(facts, "effect_intent"), json.dumps(terminal(facts).get("failure_code")))

    facts, root, _, _ = run_case(work, "E3 intermediate unobserved", new_runtime, new_ledger, packs["page"]["paths"]["zip"], content("page"),
                                 ["a_mark"] + ["b"] * 10, 4, catalog_dir)
    produced.append(("E3 intermediate unobserved", root))
    check("E3.intermediate_unobserved", terminal(facts).get("failure_code") == "contained_task_linear_intermediate_unobserved"
          and len(kinds(facts, "effect_intent")) == 1, json.dumps(terminal(facts).get("failure_code")))

    facts, root, _, _ = run_case(work, "E3 next page never", new_runtime, new_ledger, packs["three"]["paths"]["dir"], content("three"),
                                 ["a"] * 12, 4, catalog_dir)
    produced.append(("E3 next page never", root))
    check("E3.page_confirmation_failed", terminal(facts).get("failure_code") == "page_confirmation_failed"
          and terminal(facts).get("executed_steps") == 1, json.dumps(terminal(facts).get("failure_code")))

    facts, root, _, _ = run_case(work, "E3 swallowed click retried", new_runtime, new_ledger, packs["retry"]["paths"]["dir"], content("retry"),
                                 ["a"] * 6 + ["b"] * 4, 4, catalog_dir)
    produced.append(("E3 swallowed click retried", root))
    starts = [fact.get("step_index") for fact in kinds(facts, "step_started")]
    finishes = [[fact.get("step_index"), fact.get("page_label")] for fact in kinds(facts, "step_finished")]
    check("E3.retry.two_attempts_one_step", starts == [0, 0] and finishes == [[0, f"{GAME}/step_01_a"], [0, f"{GAME}/step_02_b"]]
          and len(kinds(facts, "effect_intent")) == 2, json.dumps([starts, finishes]))
    check("E3.retry.completed_executed_steps_1", completed(facts, 1, f"{GAME}/step_02_b"), json.dumps(terminal(facts)))

    facts, root, _, text = run_case(work, "E3 stuck loading", new_runtime, new_ledger, packs["retry_page"]["paths"]["dir"], content("retry_page"),
                                    ["a"] + ["load"] * 8, 4, catalog_dir)
    produced.append(("E3 stuck loading", root))
    check("E3.stuck_loading", terminal(facts).get("failure_code") == "page_confirmation_failed"
          and len(kinds(facts, "effect_intent")) == 1, json.dumps(terminal(facts).get("failure_code")))
    say("E3", "stuck loading detail intermediate_seen=true found in ledger or task evidence", "intermediate_seen=true" in text)

    facts, root, _, _ = run_case(work, "E3 three-step fails at step 2", new_runtime, new_ledger, packs["three"]["paths"]["dir"], content("three"),
                                 ["a"] + ["b"] * 7, 4, catalog_dir)
    produced.append(("E3 three-step fails at step 2", root))
    check("E3.fail_step_2_executed_2", terminal(facts).get("failure_code") == "page_confirmation_failed"
          and terminal(facts).get("executed_steps") == 2, json.dumps(terminal(facts)))

    if "prefixed" in state:
        facts, root, _, _ = run_case(work, "E3 prefixed to", new_runtime, new_ledger, state["prefixed"]["path"],
                                     "sha256:" + state["prefixed"]["sha256"], ["a", "b", "c"], 4, catalog_dir)
        produced.append(("E3 prefixed to", root))
        check("E3.prefixed_to.completed", completed(facts, 2, f"{GAME}/step_03_c"), json.dumps(terminal(facts)))

    # 7. The built package runs.
    if "built" in state:
        facts, root, _, _ = run_case(work, "E7 built linear_page", new_runtime, new_ledger, state["built"]["path"],
                                     "sha256:" + state["built"]["sha256"], ["a_mark", "load", "b"], 4, catalog_dir)
        produced.append(("E7 built linear_page", root))
        check("E7.built_runs", completed(facts, 1, f"{GAME}/step_02_b"), json.dumps(terminal(facts)))

    # 6. The v0.9.0 build refuses linear packages before PackageAdmitted.
    for label, name, expected in (("E6 v0.9.0 linear dir", "three", "contained_task_control_invalid"),
                                  ("E6 v0.9.0 linear dir with transition", "retry_page", "resource_declaration_invalid")):
        facts, root, events, text = run_case(work, label, old_runtime, old_ledger, packs[name]["paths"]["dir"], content(name),
                                             ["a", "b"], 2, catalog_dir)
        admitted = kinds(facts_of(events), "package_admitted")
        check(f"{label.replace(' ', '_')}.refused_before_package_admitted", expected in text and not admitted,
              f"{expected} in ledger: {expected in text}; package_admitted facts {len(admitted)}")

    # 5. 885947c9 actingledger reads every ledger the product build wrote.
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
              and len(events_old) == len(events_new) and len(events_new) > 20, f"{len(events_old or [])} vs {len(events_new or [])} {failure}")

    say("RESULT", "failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


if __name__ == "__main__":
    command = sys.argv[1]
    if command == "static":
        sys.exit(static(sys.argv[2], sys.argv[3], sys.argv[4]))
    if command == "prepare":
        sys.exit(prepare(sys.argv[2]))
    if command == "build":
        sys.exit(build(sys.argv[2], sys.argv[3]))
    sys.exit(run(*sys.argv[2:8]))
