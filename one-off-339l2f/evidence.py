# One-off (to be reverted), Workflow #339 L2f evidence (frozen model 5959674074 section 6.4, E1-E14).
# Every printed line starts with "L2F|". Usage:
#   evidence.py static <repo> <l2e sha> <product sha>
#   evidence.py prepare <work>
#   evidence.py run <work> <new runtime> <new tools> <l2e runtime> <l2a runtime> <v091 runtime> <v091 tools> <v090 tools> <catalog dir>
#   evidence.py kernel <work> <product kernel jsonl> <l2e kernel jsonl>
import concurrent.futures
import difflib
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
W, H = 64, 36
BG = (32, 32, 32)
COLORS = {
    # The L2a and L2e evidence packs (unchanged values).
    "a": (200, 40, 40),
    "b": (40, 200, 40),
    "c": (40, 40, 200),
    "load": (200, 200, 40),
    "off": (224, 225, 227),
    "on": (255, 229, 26),
    "home": (200, 40, 200),
    "launcher": (40, 200, 200),
    "title": (120, 120, 40),
    "homepage": (40, 120, 120),
    # The #339 packs.
    "s01": (250, 120, 0),
    "o02": (0, 250, 120),
    "s03": (120, 0, 250),
    "oa": (250, 0, 120),
    "ob": (120, 250, 0),
    "s04": (0, 120, 250),
    "p": (100, 100, 100),
    "s02": (250, 180, 180),
    "o03": (180, 180, 250),
    "s05": (180, 250, 180),
}
PAGE_STATE = {"p1": "p", "p2": "p"}
RESOURCES = {"schema_version": "1.0", "resources": [], "resource_count": 0}
APPLICATION_REFUSAL = "application_effect_requires_assigned_application"
OPTIONAL = {"optional": {"settle_ms": 500}}
RETRY = {"retryable": True, "max_attempts": 2, "retry_interval_ms": 100}
FAILURES = []


def say(*parts):
    print("L2F|" + "|".join(str(part) for part in parts), flush=True)


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


def page_state(page):
    return PAGE_STATE.get(page, page)


def guard_for(page, state):
    return {
        "page_id": page,
        "target_id": f"state/{state}",
        "expected_rect": {"x": 10, "y": 10, "width": 1, "height": 1},
        "color_probe": f"state/{state}",
    }


def control_for(package_id, task_id, mode, count, step_timeout_ms, capture_interval_ms=100):
    return {
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": mode,
        "game": GAME,
        "server": SERVER,
        "resolution": {"width": W, "height": H},
        "entry_task_id": task_id,
        "timeout_ms": 30000,
        "step_timeout_ms": step_timeout_ms,
        "capture_interval_ms": capture_interval_ms,
        "max_steps": count,
    }


def files_for(task_id, task, control, extra=None):
    files = {
        "control.json": pretty(control),
        "resources/operations/resources.json": pretty(RESOURCES),
        f"resources/operations/{task_id}/task.json": pretty(task),
    }
    for path, data in (extra or {}).items():
        files[f"resources/operations/{task_id}/{path}"] = data
    return files


# ---------------------------------------------------------------------------------------------
# The #339 packs: 64x36, step_timeout_ms 1000, capture_interval_ms 100, settle_ms 500.

def opt_pack(task_id, spec, mode="linear_steps", mutate=None):
    """spec: (from, to, effect, extra) per operation; effect is "click" or an application action."""
    pages = []
    for frm, to, _effect, _extra in spec:
        for page in (frm, to):
            if page != "any" and page not in pages:
                pages.append(page)
    states = sorted({page_state(page) for page in pages})
    operations = []
    for index, (frm, to, effect, extra) in enumerate(spec):
        operation = {
            "id": f"step_{index + 1:02d}_{'click' if effect == 'click' else 'app'}",
            "purpose": f"Optional-step fixture step {index + 1}",
            "from": frm,
            "to": to,
            "expect_after": {"page_id": to, "timeout_ms": 1000, "interval_ms": 100},
            "post_delay_ms": 50,
        }
        if effect == "click":
            operation["click"] = {"kind": "rect", "x": 8, "y": 8, "width": 5, "height": 5}
            operation["guard"] = guard_for(frm, page_state(frm))
        else:
            operation["application"] = {"action": effect}
        operation.update(json.loads(json.dumps(extra)))
        operations.append(operation)
    task = {
        "schema_version": "0.9",
        "task_id": task_id,
        "game": GAME,
        "server_scope": [SERVER],
        "locale": "en-US",
        "goal": f"Optional-step fixture {task_id}",
        "coordinate_space": {"width": W, "height": H},
        "defaults": {"template_threshold": 0.95, "color_max_distance": 20, "match_metric": "ccoeff_normed"},
        "timeout_ms": 30000,
        "max_steps": len(operations),
        "entry_page": spec[0][0],
        "target_page": spec[-1][1],
        "color_probes": [probe(state) for state in states],
        "page_rules": {page: {"required": [f"state/{page_state(page)}"]} for page in pages},
        "scheduling_outcome": {"mappings": [{
            "outcome_key": f"{task_id}_done", "effect": "no_designated_effect", "terminal_pages": [spec[-1][1]]}]},
        "operations": operations,
    }
    control = control_for(f"fixture.optional.{task_id}", task_id, mode, len(operations), 1000)
    if mutate:
        mutate(task, control)
    return files_for(task_id, task, control)


OPT1 = [("s01", "o02", "click", {}), ("o02", "s03", "click", OPTIONAL)]
OPT1P = [("s01", "o02", "click", RETRY), ("o02", "s03", "click", OPTIONAL)]
OPT1PP = [("s01", "o02", "click", {}), ("o02", "s03", "click", {**OPTIONAL, **RETRY})]
OPT2 = [("s01", "oa", "click", {}), ("oa", "ob", "click", OPTIONAL), ("ob", "s04", "click", OPTIONAL)]
OPT3 = [("s01", "p1", "click", {}), ("p1", "p2", "click", OPTIONAL), ("p2", "s04", "click", OPTIONAL)]


def set_optional(index, value):
    def mutate(task, _control):
        task["operations"][index]["optional"] = value
    return mutate


def opt_packs():
    def transition_on_skip_target(task, _control):
        task["operations"][0]["transition"] = {"kind": "page", "page_id": "s03"}

    def first_optional(task, _control):
        task["operations"][0]["optional"] = {"settle_ms": 500}

    return {
        "opt1": opt_pack("opt1", OPT1),
        "opt1p": opt_pack("opt1p", OPT1P),
        "opt1pp": opt_pack("opt1pp", OPT1PP),
        "opt2": opt_pack("opt2", OPT2),
        "opt3": opt_pack("opt3", OPT3),
        # E10 refusals.
        "first_step": opt_pack("first_step", OPT1, mutate=first_optional),
        "settle": opt_pack("settle", OPT1, mutate=set_optional(1, {"settle_ms": 60001})),
        "candidates": opt_pack("candidates", [("s01", "o02", "click", {}), ("o02", "s01", "click", OPTIONAL),
                                              ("s01", "s03", "click", {})]),
        "transition_page": opt_pack("transition_page", OPT1, mutate=transition_on_skip_target),
        "page_graph": opt_pack("page_graph", OPT1, mode="navigable_route"),
        "shape_true": opt_pack("shape_true", OPT1, mutate=set_optional(1, True)),
        "shape_string": opt_pack("shape_string", OPT1, mutate=set_optional(1, {"settle_ms": "2000"})),
        "shape_empty": opt_pack("shape_empty", OPT1, mutate=set_optional(1, {})),
        "shape_extra": opt_pack("shape_extra", OPT1, mutate=set_optional(1, {"settle_ms": 500, "late_ms": 1})),
        "application": opt_pack("application", [("s01", "s02", "click", {}), ("s02", "s03", "launch", OPTIONAL),
                                                ("s03", "home", "click", {})]),
        "segment_end": opt_pack("segment_end", [("any", "s02", "restart", {}), ("s02", "home", "click", {}),
                                                ("home", "s05", "click", OPTIONAL)]),
        "segment_ok": opt_pack("segment_ok", [("any", "s02", "restart", {}), ("s02", "o03", "click", {}),
                                              ("o03", "home", "click", OPTIONAL)]),
    }


LINEAR_INVALID = "contained_task_linear_invalid"
OPT_REFUSALS = [
    # name, code, detail (kernel detail or declaration issue)
    ("first_step", LINEAR_INVALID, "reason=optional_first_step operation=step_01_click"),
    ("settle", LINEAR_INVALID, "reason=optional_settle operation=step_02_click"),
    ("candidates", LINEAR_INVALID, "reason=optional_candidates operation=step_01_click"),
    ("transition_page", LINEAR_INVALID, "reason=transition_page operation=step_01_click"),
    ("page_graph", "contained_task_operation_invalid", "operation=step_02_click optional requires linear_steps"),
    ("shape_true", "resource_declaration_invalid", '"field_path":"/operations/1/optional" && "reason":"invalid_type"'),
    ("shape_string", "resource_declaration_invalid", '"field_path":"/operations/1/optional/settle_ms" && "reason":"invalid_type"'),
    ("shape_empty", "resource_declaration_invalid", '"field_path":"/operations/1/optional/settle_ms" && "reason":"missing_field"'),
    ("shape_extra", "resource_declaration_invalid", '"field_path":"/operations/1/optional/late_ms" && "reason":"unknown_field"'),
    ("application", LINEAR_INVALID, "reason=optional_application operation=step_02_app"),
    ("segment_end", LINEAR_INVALID, "reason=optional_restart_segment_end operation=step_03_click"),
]


# ---------------------------------------------------------------------------------------------
# The L2a evidence packs (one-off 409ffce6) and the L2e ones (one-off eab0c833), same content.

def l2a_page_name(index, state):
    return f"step_{index + 1:02d}_{state}"


def l2a_pack(task_id, states, transition=None, template=False, retry=False, mode="linear_steps", mutate=None):
    pages = [l2a_page_name(index, state) for index, state in enumerate(states)]
    used = sorted(set(states) | ({"load"} if transition and transition.get("kind") == "page" else set()))
    page_rules = {}
    for index, (page, state) in enumerate(zip(pages, states)):
        page_rules[page] = {"required": (["ui/mark"] if template and index == 0 else []) + [f"state/{state}"]}
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
            "guard": guard_for(pages[index], states[index]),
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
    control = control_for(f"fixture.linear.{task_id}", task_id, mode, len(operations), 500)
    if mutate:
        mutate(task, control)
    return files_for(task_id, task, control, {"assets/mark.png": MARK_PNG} if template else None)


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
        "color_probes": [probe("off"), probe("on")],
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
    return files_for("toggle", task, control)


def app_pack(task_id, spec, mode="linear_steps", mutate=None):
    """The L2e builder: (from, to, effect) per operation, effect "click" or an application action."""
    def state_of(page):
        return page.rsplit("_", 1)[-1]

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
            operation["guard"] = guard_for(frm, state_of(frm))
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
    control = control_for(f"fixture.application.{task_id}", task_id, mode, len(operations), 500)
    extra = {}
    if mutate:
        extra = mutate(task, control) or {}
    return files_for(task_id, task, control, extra)


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


def l2a_l2e_packs():
    def loop(task, _control):
        task["target_page"] = "step_01_a"
        task["scheduling_outcome"]["mappings"][0]["terminal_pages"] = ["step_01_a"]
        task["operations"][0]["to"] = "step_01_a"
        task["operations"][0]["expect_after"]["page_id"] = "step_01_a"
        del task["page_rules"]["step_02_b"]

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
        task["operations"][0]["guard"] = guard_for("home", "home")

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
        # L2a
        "l2a_three": l2a_pack("linear_three", ["a", "b", "c"]),
        "l2a_page": l2a_pack("linear_page", ["a", "b"], transition={"kind": "page", "page_id": "transition_01"}, template=True),
        "l2a_window": l2a_pack("linear_window", ["a", "b"], transition={"kind": "window", "min_ms": 1000, "max_ms": 3000}),
        "l2a_retry": l2a_pack("linear_retry", ["a", "b"], retry=True),
        "l2a_retry_page": l2a_pack("linear_retry_page", ["a", "b"], transition={"kind": "page", "page_id": "transition_01"}, retry=True),
        "l2a_to_equals_from": l2a_pack("linear_loop", ["a", "b"], mutate=loop),
        "l2a_nav_transition": l2a_pack("nav_transition", ["a", "b"], transition={"kind": "page", "page_id": "transition_01"}, mode="navigable_route"),
        "l2a_window_min_above_max": l2a_pack("linear_bad_window", ["a", "b"], transition={"kind": "window", "min_ms": 3000, "max_ms": 1000}),
        "l2a_transition_kind_unknown": l2a_pack("linear_bad_kind", ["a", "b"], transition={"kind": "fade", "min_ms": 1000}),
        "toggle": toggle_pack(),
        # L2e
        "l2e_p1": app_pack("app_p1", P1),
        "l2e_p2": app_pack("app_p2", P2),
        "l2e_p3": app_pack("app_p3", P3),
        "l2e_p4": app_pack("app_p4", P1, mutate=window),
        "l2e_click_only": app_pack("click_only", [("step_01_a", "step_02_b", "click"), ("step_02_b", "step_03_c", "click")]),
        "l2e_page_graph_start": app_pack("page_graph_start", [("any", "home", "restart")], mode="navigable_route"),
        "l2e_r25_no_home": app_pack("r25_no_home", [("any", "step_02_b", "restart"), ("step_02_b", "step_03_c", "click")]),
        "l2e_lab_shaped": app_pack("lab_shaped", [("any", "step_02_title", "restart"), ("step_02_title", "step_03_home", "click")]),
        "l2e_homepage": app_pack("homepage", [("any", "step_02_title", "restart"), ("step_02_title", "step_03_homepage", "click")]),
        "l2e_step_xx_home": app_pack("step_xx_home", [("any", "step_02_title", "restart"), ("step_02_title", "step_xx_home", "click")]),
        "l2e_any_click": app_pack("any_click", [("any", "home", "click"), ("home", "step_02_c", "click")], mutate=trusted_point),
        "l2e_any_later": app_pack("any_later", [("step_01_a", "step_02_b", "click"), ("any", "home", "launch")]),
        "l2e_app_retry": app_pack("app_retry", P1, mutate=retry),
        "l2e_stop_click": app_pack("stop_click", [("step_01_a", "step_02_b", "click"), ("step_02_b", "step_03_launcher", "stop"),
                                                  ("step_03_launcher", "step_04_c", "click")]),
        "l2e_entry_mismatch": app_pack("entry_mismatch", P1, mutate=entry_home),
        "l2e_app_guard": app_pack("app_guard", P1, mutate=app_guard),
        "l2e_select_step": app_pack("select_step", [("step_01_a", "step_02_b", "click")], mutate=select),
    }


# The fixture runs of the L2a and L2e evidence (pack, frames).
L2A_RUNS = [
    ("l2a_three", ["a", "b", "c"]),
    ("l2a_page", ["a_mark", "load", "b"]),
    ("l2a_window", ["a", "b"]),
    ("l2a_three", ["x"] * 12),
    ("l2a_page", ["a_mark"] + ["b"] * 10),
    ("l2a_three", ["a"] * 12),
    ("l2a_retry", ["a"] * 6 + ["b"] * 4),
    ("l2a_retry_page", ["a"] + ["load"] * 8),
    ("l2a_three", ["a"] + ["b"] * 7),
    ("l2a_to_equals_from", ["a", "b"]),
    ("l2a_nav_transition", ["a", "b"]),
    ("l2a_window_min_above_max", ["a", "b"]),
    ("l2a_transition_kind_unknown", ["a", "b"]),
]
L2E_RUNS = [
    ("l2e_p1", ["home", "c"]),
    ("l2e_p2", ["a", "b", "home"]),
    ("l2e_p3", ["a", "b", "launcher", "home"]),
    ("l2e_p4", ["home", "c"]),
    ("l2e_click_only", ["a", "b", "c"]),
    ("l2e_lab_shaped", ["title", "home"]),
] + [(name, ["a", "home"]) for name in (
    "l2e_page_graph_start", "l2e_r25_no_home", "l2e_homepage", "l2e_step_xx_home", "l2e_any_click", "l2e_any_later",
    "l2e_app_retry", "l2e_stop_click", "l2e_entry_mismatch", "l2e_app_guard", "l2e_select_step")]


def load_state(work):
    with open(os.path.join(work, "state.json"), encoding="utf-8") as handle:
        return json.load(handle)


def save_state(work, state):
    with open(os.path.join(work, "state.json"), "w", encoding="utf-8") as handle:
        json.dump(state, handle, indent=2)


# ---------------------------------------------------------------------------------------------
# static

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


def static(repo, l2e, product):
    say("E11", "base", l2e[:8], "product", product[:8])
    names = sorted(line for line in git(repo, "diff", "--name-only", l2e, product).splitlines() if line)
    for line in git(repo, "diff", "--stat", l2e, product).splitlines():
        say("SCOPE", "diff --stat L2e..product", line)
    check("SCOPE.changed_files", names == sorted([
        "contracts/README.md", "contracts/linear-steps.md", "crates/execution-kernel/src/contained_task.rs",
        "crates/execution-kernel/src/contained_task/linear.rs", "crates/pack-containment/src/source/declarations.rs"]),
        json.dumps(names))
    for path in ("crates/runtime-host/src/host/contained_task.rs", "crates/execution-kernel/src/run.rs",
                 "crates/execution-kernel/src/offline.rs", "crates/page-detector/src/lib.rs",
                 "crates/pack-containment/src/source/mod.rs", "crates/execution-kernel/src/contained_task/timing.rs",
                 "crates/actingcommand-contract/src/event/payload.rs"):
        stat = git(repo, "diff", "--stat", l2e, product, "--", path).strip()
        say("SCOPE", "diff", path, "unchanged" if not stat else stat)
        check(f"SCOPE.file_unchanged.{os.path.basename(path)}", not stat, path)
    path = "crates/execution-kernel/src/contained_task/linear.rs"
    before, after = git(repo, "show", f"{l2e}:{path}"), git(repo, "show", f"{product}:{path}")
    for name in ("linear_wait", "linear_observe"):
        old, new = function_span(before, name), function_span(after, name)
        diff = list(difflib.unified_diff(old.splitlines(), new.splitlines(), f"{l2e[:8]}:{name}", f"{product[:8]}:{name}", lineterm=""))
        say("E11", f"fn {name}", "L2e lines", old.count("\n") + 1, "sha256", hashlib.sha256(old.encode()).hexdigest(),
            "product lines", new.count("\n") + 1, "sha256", hashlib.sha256(new.encode()).hexdigest())
        say("E11", f"diff of fn {name} L2e..product", "empty" if not diff else json.dumps(diff))
        check(f"E11.function_unchanged.{name}", old is not None and old == new, name)
    wait_set = function_span(after, "linear_wait_set")
    for line in wait_set.splitlines():
        say("SETTLE", "linear_wait_set", line)
    check("SETTLE.every_sleep_reports_a_boundary",
          wait_set.count("thread::sleep(") == 1 and wait_set.count("runtime.observe_task_boundary(") == 1
          and "let boundary = purpose.boundary();" in wait_set and "LinearWaitPurpose::AfterInput" in wait_set, "")
    check("SETTLE.settle_bounds_sleep_only_while_it_lasts", ".filter(|left| !left.is_zero())" in wait_set
          and "settle_left.map_or(sleep, |left| sleep.min(left))" in wait_set, "")
    awaited = function_span(after, "awaited")
    for line in awaited.splitlines():
        say("DETAIL", "awaited", line)
    check("DETAIL.awaited_empty_for_one_candidate", "if self.pages.len() < 2 {\n            return String::new();" in awaited, "")
    ct = "crates/execution-kernel/src/contained_task.rs"
    for line in git(repo, "diff", l2e, product, "--", ct, "crates/pack-containment/src/source/declarations.rs").splitlines():
        if line.startswith(("+", "-", "@@")) and not line.startswith(("+++", "---")):
            say("SCOPE", "kernel/declaration diff", line)
    say("RESULT", "static failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


# ---------------------------------------------------------------------------------------------
# prepare: packages, frames, kernel harness cases.

FRAME_STATES = {
    "a": ("a", False), "a_mark": ("a", True), "b": ("b", False), "c": ("c", False), "load": ("load", False),
    "x": (None, False), "off": ("off", False), "on": ("on", False), "home": ("home", False),
    "launcher": ("launcher", False), "title": ("title", False), "homepage": ("homepage", False),
    "s01": ("s01", False), "o02": ("o02", False), "s03": ("s03", False), "oa": ("oa", False), "ob": ("ob", False),
    "s04": ("s04", False), "p": ("p", False), "s02": ("s02", False), "o03": ("o03", False), "s05": ("s05", False),
}

# E1-E9: (label, pack, frames, expect code, expect detail, inputs, executed steps)
OPT_RUNS = [
    ("E1", "opt1", ["s01", "o02", "s03", "s03"], "success", 'final_page=Some("fixture-game-a/s03") executed_steps=2', 2),
    ("E2", "opt1", ["s01"] + ["s03"] * 12, "success", 'final_page=Some("fixture-game-a/s03") executed_steps=1', 1),
    ("E3", "opt1", ["s01", "s03", "s03", "o02", "s03", "s03"], "success", 'final_page=Some("fixture-game-a/s03") executed_steps=2', 2),
    ("E4a", "opt2", ["s01", "oa", "ob", "s04", "s04"], "success", 'final_page=Some("fixture-game-a/s04") executed_steps=3', 3),
    ("E4b", "opt2", ["s01", "ob", "oa", "s04", "s04"], "success", 'final_page=Some("fixture-game-a/s04") executed_steps=3', 3),
    ("E4c", "opt2", ["s01", "ob"] + ["s04"] * 12, "success", 'final_page=Some("fixture-game-a/s04") executed_steps=2', 2),
    ("E4d", "opt2", ["s01"] + ["s04"] * 12, "success", 'final_page=Some("fixture-game-a/s04") executed_steps=1', 1),
    ("E5a", "opt3", ["s01", "p", "p", "s04", "s04"], "success", 'final_page=Some("fixture-game-a/s04") executed_steps=3', 3),
    ("E5b", "opt3", ["s01", "p"] + ["s04"] * 12, "success", 'final_page=Some("fixture-game-a/s04") executed_steps=2', 2),
    ("E6", "opt1", ["s01", "s03"] + ["x"] * 20, "page_confirmation_failed",
     "operation=step_01_click attempts=1 after_page=<unrecognized> hit_error_page=false transition=none"
     " awaited=fixture-game-a/o02,fixture-game-a/s03 skip_target_seen=true", 1),
    ("E7", "opt1p", ["s01"] + ["s01"] * 14 + ["o02", "s03"], "success", 'final_page=Some("fixture-game-a/s03") executed_steps=2', 3),
    ("E8", "opt1p", ["s01", "s03"] + ["x"] * 20, "page_confirmation_failed",
     "operation=step_01_click attempts=1 after_page=<unrecognized> hit_error_page=false transition=none"
     " awaited=fixture-game-a/o02,fixture-game-a/s03 skip_target_seen=true", 1),
    ("E9", "opt1pp", ["s01", "o02"] + ["o02"] * 14 + ["s03"], "success", 'final_page=Some("fixture-game-a/s03") executed_steps=2', 3),
]

# The L2a and L2e in-process runs (code, detail, inputs), on both sources.
def l2_kernel_runs(case, frames):
    return [
        case("l2a_three", "l2a-dir-none-success", frames=[frames[n] for n in ["a", "b", "c"]], expect="success",
             expect_detail='executed_steps=2', expect_inputs=2),
        case("l2a_three", "l2a-entry-unmatched", frames=[frames["x"]] * 12, expect="contained_task_linear_entry_unmatched",
             expect_detail="page=fixture-game-a/step_01_a", expect_inputs=0),
        case("l2a_page", "l2a-intermediate-unobserved", frames=[frames["a_mark"]] + [frames["b"]] * 10,
             expect="contained_task_linear_intermediate_unobserved", expect_detail="intermediate_page=fixture-game-a/transition_01", expect_inputs=1),
        case("l2a_retry_page", "l2a-stuck-loading-with-retry", frames=[frames["a"]] + [frames["load"]] * 8,
             expect="page_confirmation_failed", expect_detail="transition=page intermediate_seen=true", expect_inputs=1),
        case("l2a_window", "l2a-window-next-page-never", frames=[frames["a"]] * 24, expect="page_confirmation_failed",
             expect_detail="transition=window min_ms=1000 max_ms=3000", expect_inputs=1),
        case("l2a_retry", "l2a-swallowed-click-retried", frames=[frames["a"]] * 6 + [frames["b"]] * 4, expect="success",
             expect_detail="executed_steps=1", expect_inputs=2),
        case("l2a_three", "l2a-next-page-never", frames=[frames["a"]] * 12, expect="page_confirmation_failed",
             expect_detail="operation=step_01_click attempts=1", expect_inputs=1),
        case("l2a_three", "l2a-fails-at-step-2", frames=[frames["a"]] + [frames["b"]] * 7, expect="page_confirmation_failed",
             expect_detail="operation=step_02_click attempts=1", expect_inputs=2),
        case("l2e_p1", "l2e-p1-success", frames=[frames["home"], frames["c"]], expect="success",
             expect_detail='executed_steps=2', expect_inputs=1),
        case("l2e_p1", "l2e-p1-unconfirmed", frames=[frames["x"]] * 12, expect="contained_task_linear_application_unconfirmed",
             expect_detail="operation=step_01_app application=restart attempts=1 transition=none intermediate_seen=false", expect_inputs=0),
        case("l2e_p2", "l2e-p2", frames=[frames["a"], frames["b"], frames["home"]], expect="success", expect_detail="executed_steps=2", expect_inputs=1),
        case("l2e_p3", "l2e-p3", frames=[frames["a"], frames["b"], frames["launcher"], frames["home"]], expect="success",
             expect_detail="executed_steps=3", expect_inputs=1),
        case("l2e_p4", "l2e-p4", frames=[frames["home"], frames["c"]], expect="success", expect_detail="executed_steps=2", expect_inputs=1),
        case("l2e_click_only", "l2e-click-only", frames=[frames["a"], frames["b"], frames["c"]], expect="success",
             expect_detail="executed_steps=2", expect_inputs=2),
        case("l2e_lab_shaped", "l2e-lab-shaped", frames=[frames["title"], frames["home"]], expect="success",
             expect_detail="executed_steps=2", expect_inputs=1),
    ]


L2_ADMISSION = [
    # pack, expect, detail; on both sources
    ("l2a_three", "ok", "mode=linear_steps"),
    ("l2a_page", "ok", "mode=linear_steps"),
    ("l2a_window", "ok", "mode=linear_steps"),
    ("l2a_to_equals_from", LINEAR_INVALID, "reason=to_equals_from operation=step_01_click"),
    ("l2a_nav_transition", "contained_task_operation_invalid", "transition requires linear_steps"),
    ("l2a_window_min_above_max", LINEAR_INVALID, "reason=transition_window"),
    ("l2a_transition_kind_unknown", "resource_declaration_invalid", "/operations/0/transition/kind"),
    ("l2e_p1", "ok", "mode=linear_steps"),
    ("l2e_page_graph_start", "ok", "mode=navigable_route"),
    ("l2e_any_click", LINEAR_INVALID, "reason=any_requires_application operation=step_01_click"),
    ("l2e_any_later", LINEAR_INVALID, "reason=any_from operation=step_02_app"),
    ("l2e_app_retry", LINEAR_INVALID, "reason=application_retry operation=step_01_app"),
    ("l2e_stop_click", LINEAR_INVALID, "reason=input_after_application_stop operation=step_03_click"),
    ("l2e_entry_mismatch", LINEAR_INVALID, "reason=entry_page operation=step_01_app"),
    ("l2e_select_step", LINEAR_INVALID, "reason=operation_effect operation=step_01_click"),
    ("l2e_r25_no_home", LINEAR_INVALID, "reason=application_without_home operation=step_01_app"),
    ("l2e_homepage", LINEAR_INVALID, "reason=application_without_home operation=step_01_app"),
    ("l2e_step_xx_home", LINEAR_INVALID, "reason=application_without_home operation=step_01_app"),
]


# The digests the L2a one-off (run 37038935428) and the L2e one-off (run 37063544445) printed
# for the same packs.
EARLIER_DIGESTS = {
    "l2a_nav_transition": "89029daee724d89c4965d106463ff3ee7b5122f1f2296f21112af82eb906c53e",
    "l2a_page": "961a14ae9b8b7c4523b0aa9002b4e2e37316945b8b068e82c2ea4760482c7441",
    "l2a_retry_page": "c9f5dee238d99b969eda0ab349177321f9ac670948be5cbc6ada084b32399efa",
    "l2a_retry": "a859924a769d8689f87a6cb10b1aa4d7ca95a6c47f2a014776bbfe9f706d6475",
    "l2a_three": "fb4a8a7c37c69d693d6aaad0979c340555d9cc5865da8c8c2caab81cb6d6d487",
    "l2a_to_equals_from": "56c35b3f404f9e1212dcb025ce47a79e73508daee8a77d35215a0ebf6fbabb74",
    "toggle": "100f07b95bea8316828b04fa1c090e12d937cbc733c1158bc55e73fc02dbbdc8",
    "l2a_transition_kind_unknown": "4eb91fe495f2bc12eb895d12bd6033224ce970665854606cc0aeaff26240c266",
    "l2a_window_min_above_max": "5574328b015febfbda17f9d4d4457edd49b18ca701a489344aabb99889d4fe1a",
    "l2a_window": "fdd2e9c30b9de21114817041f615b398c5ae554b8344bf058afe31200f2560ac",
    "l2e_p1": "8ba12514c536c6d0a9bd20e3cb08c4a901399b220a125d5d0075b0b87400df1f",
    "l2e_p2": "1a7a1c00fbb408dabfa09802aef724cbd99e074240876b2073b914f6a863512c",
    "l2e_p3": "45b45f9b775764aca75c2e282e512145f9c356cb91d45bd5db5cc10da6b89cec",
    "l2e_p4": "515c7133817e019a5a0c5f5b4352b0a65e1f3401aa4db494e55d6a0a7aa66a42",
    "l2e_click_only": "0adfda6681e29649fcbdea48827db8d267eefc62ebc791814415e2ebc521cc33",
    "l2e_page_graph_start": "7f0ffb4b559891a290c309c0a0b40119924480f3a3bd5c5e5920805f3039c27a",
    "l2e_r25_no_home": "60eb877839a397eea75cae94648732852c07de3b2987d3cee28bc5b38adaf753",
    "l2e_lab_shaped": "3ed8db699bff7c27268ebd7ea55ce85846c208d6df70f6a4b184688880fb694b",
    "l2e_homepage": "be6ffe7f591cb1acb49b7c42687dae5838812892d6e88e24a60a174a0cbc21d3",
    "l2e_step_xx_home": "5757913d8d17c05521ce6e485b71603fa5962e6ce95282c7c8a395ceb4498c1e",
    "l2e_any_click": "673eae6883068d1db465144d1f773ce88ceae14e50ee642bb00802a7d90a6859",
    "l2e_any_later": "687b393f1f55feebca185bbfd45d37ef935a5911aa4715cc43efe5d4f1d6e4c9",
    "l2e_app_retry": "dcf13ccb163af6984d8e79e9eeb78e552361dfbd5628d320e3c797e5d8036d8a",
    "l2e_stop_click": "7fc4f2d31630f90839f5b6344d87596e9c2bc89fdea06706b21e009c465b388a",
    "l2e_entry_mismatch": "4141a2b64e6c4e0bad086cbe2cfaa2d336943b75eb8668aaee92a5a26c433a34",
    "l2e_app_guard": "c1f4af39cc75ccd524ccb3d6fd581fc075f649283f94b12bc4be863249347f1a",
    "l2e_select_step": "e3e0314e7932d6f3cfc93b017185eecda505ce670c1091d78282a328f862226d",
}


def prepare(work):
    os.makedirs(work, exist_ok=False)
    state = {"packs": {}, "frames": {}}
    for name, files in {**opt_packs(), **l2a_l2e_packs()}.items():
        sha = digest(files)
        if name in EARLIER_DIGESTS:
            check(f"E11.pack_identical_to_earlier_evidence.{name}", sha == EARLIER_DIGESTS[name], sha)
        path = os.path.join(work, "packs", name, sha)
        write_tree(path, files)
        state["packs"][name] = {"digest": sha, "path": path}
        say("prepare", "pack", name, "digest", sha, "files", len(files))
        if name in ("opt1", "opt1p", "opt1pp", "opt2", "opt3", "segment_ok") or name in {item[0] for item in OPT_REFUSALS}:
            for file_path in sorted(files):
                if file_path.endswith("task.json") or file_path == "control.json":
                    say("prepare", "pack file", name, file_path, short(files[file_path].decode("utf-8"), 6000))
    for name, (frame_state, mark) in FRAME_STATES.items():
        path = os.path.join(work, "frames", name + ".png")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        make_frame(frame_state, mark).save(path, format="PNG")
        state["frames"][name] = path
    frames = state["frames"]

    def case(pack, name=None, **fields):
        entry = state["packs"][pack]
        return {"name": name or pack, "locator": entry["path"], "reference": reference(entry["digest"]), **fields}

    # The product harness: E1-E10, E14, and the L2a/L2e cases of E11.
    admission = [case("opt1", expect="ok", expect_detail="mode=linear_steps", simulate=[[frames["s01"]]],
                      expect_decision='"status":"would_click","operation_label":"step_01_click"')]
    for pack in ("opt1p", "opt1pp", "opt2", "opt3"):
        admission.append(case(pack, expect="ok", expect_detail="mode=linear_steps"))
    admission.append(case("segment_ok", expect="ok", expect_detail="mode=linear_steps", simulate=[[frames["s02"]]],
                          expect_decision=APPLICATION_REFUSAL))
    for name, code, detail in OPT_REFUSALS:
        admission.append(case(name, expect=code, expect_detail=detail))
    l2_admission = [case(pack, f"e11-{pack}", expect=code, expect_detail=detail) for pack, code, detail in L2_ADMISSION]
    runs = [case(pack, f"{label}-{pack}", frames=[frames[n] for n in names], expect=code, expect_detail=detail,
                 expect_inputs=inputs) for label, pack, names, code, detail, inputs in OPT_RUNS]
    l2_runs = l2_kernel_runs(case, frames)
    with open(os.path.join(work, "cases.json"), "w", encoding="utf-8") as handle:
        json.dump({"admission": admission + l2_admission, "runs": runs + l2_runs}, handle, indent=2)
    with open(os.path.join(work, "cases-l2e.json"), "w", encoding="utf-8") as handle:
        json.dump({"admission": l2_admission, "runs": l2_runs}, handle, indent=2)
    # E12: the older loaders on OPT1.
    with open(os.path.join(work, "cases-old.json"), "w", encoding="utf-8") as handle:
        json.dump({"admission": [case("opt1", "opt1-on-an-older-build", expect="resource_declaration_invalid",
                                      expect_detail='"field_path":"/operations/1/optional" && "reason":"unknown_field"')]},
                  handle, indent=2)
    save_state(work, state)
    say("prepare", "done", work)
    return 0


# ---------------------------------------------------------------------------------------------
# run: scheduled runs on the fixture backend (a fixture instance takes no direct task-run).

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


def write_config(config_dir, state_root, package_path, package_digest, frames, catalog_dir, instance_number):
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
        "secret_fingerprint_salt": "oneoff-339-l2f-fixture-salt-value",
        "policy": {
            "facts": {
                "ledger_position": 0,
                "fact_snapshot_id": "snapshot:oneoff-339-l2f",
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
            "instance_id": f"instance_{instance_number:032x}",
            "fixture_backend": {
                "frames": [{"width": W, "height": H, "rgb": list(Image.open(frame).convert("RGB").tobytes())} for frame in frames],
                "max_inputs": 8,
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
    elif second > 12:
        time.sleep(62 - second)


JOB_COUNTER = [0]


def scheduled_batch(work, jobs, settle_s=20):
    """jobs: (label, runtime dir, pack name, frame names); all daemons of a batch run at once,
    each with its own state root and instance id. Returns {label: state root}."""
    state = load_state(work)
    prepared = []
    for label, runtime_dir, pack, frame_names in jobs:
        JOB_COUNTER[0] += 1
        run_dir = os.path.join(work, "runs", f"{JOB_COUNTER[0]:03d}-" + re.sub(r"[^A-Za-z0-9_.-]", "_", label))
        state_root = os.path.join(run_dir, "state")
        os.makedirs(state_root)
        entry = state["packs"][pack]
        content = {"schema_version": SCHEMA_DIR, "sha256": entry["digest"]}
        frames = [state["frames"][name] for name in frame_names]
        config = write_config(os.path.join(run_dir, "config"), state_root, entry["path"], content, frames,
                              os.environ["CATALOG_DIR"], 0x339000 + JOB_COUNTER[0])
        say(label, "frames", json.dumps(frame_names), "pack", pack, "digest", entry["digest"], "run dir", run_dir)
        prepared.append((label, runtime_dir, run_dir, state_root, config))
    wait_for_minute_window()
    processes = []
    for label, runtime_dir, run_dir, state_root, config in prepared:
        out = open(os.path.join(run_dir, "actingd.out"), "wb")
        err = open(os.path.join(run_dir, "actingd.err"), "wb")
        process = subprocess.Popen([os.path.join(runtime_dir, "actingcommand-actingd.exe"), "--config", config],
                                   stdout=out, stderr=err, cwd=os.path.dirname(config))
        processes.append((label, runtime_dir, run_dir, state_root, process, out, err, time.time()))

    def ready(item):
        label, runtime_dir, _run_dir, state_root, process, _out, _err, started = item
        actingctl = os.path.join(runtime_dir, "actingctl.exe")
        while time.time() - started < 30 and process.poll() is None:
            code, _, _ = run_exe([actingctl, "status", "--state-root", state_root], timeout=30)
            if code == 0:
                return label, True, round(time.time() - started, 1), round(started % 60, 1)
            time.sleep(0.5)
        return label, False, round(time.time() - started, 1), round(started % 60, 1)

    def shutdown(item):
        label, runtime_dir, run_dir, state_root, process, out, err, _started = item
        actingctl = os.path.join(runtime_dir, "actingctl.exe")
        code, _, shutdown_err = run_exe([actingctl, "request-shutdown", "--state-root", state_root, "--wait", "60"], timeout=120)
        try:
            exit_code = process.wait(timeout=120)
        except subprocess.TimeoutExpired:
            process.kill()
            exit_code = "killed"
        out.close()
        err.close()
        return label, code, short(shutdown_err, 300), exit_code, run_dir

    with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, len(processes))) as pool:
        for label, is_ready, after_s, second in pool.map(ready, processes):
            say(label, "daemon ready", is_ready, "after_s", after_s, "second of minute", second)
            if not is_ready:
                FAILURES.append(f"{label}.daemon_ready")
        time.sleep(settle_s)
        for label, code, shutdown_err, exit_code, run_dir in pool.map(shutdown, processes):
            say(label, "request-shutdown exit", code, shutdown_err, "actingd exit", exit_code)
            if exit_code != 0:
                for stream in ("out", "err"):
                    with open(os.path.join(run_dir, f"actingd.{stream}"), "rb") as handle:
                        say(label, "actingd " + stream, short(handle.read().decode("utf-8", "replace"), 1200))
    return {item[0]: item[3] for item in prepared}


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


def find_key(value, key):
    if isinstance(value, dict):
        for name, item in value.items():
            if name == key:
                yield item
            yield from find_key(item, key)
    elif isinstance(value, list):
        for item in value:
            yield from find_key(item, key)


def kinds(facts, kind):
    return [item[2] for item in facts if item[2]["kind"] == kind]


def terminal(facts):
    found = kinds(facts, "terminal_committed")
    return found[-1] if found else {}


def print_facts(label, facts):
    for sequence, event_type, fact, stamp in facts:
        kind = fact["kind"]
        if kind in ("recognition_started", "recognition_completed"):
            detail = {"candidate_pages": fact.get("candidate_pages"), "matched_page": fact.get("matched_page")}
        elif kind in ("step_started", "step_finished", "effect_intent", "effect_completed"):
            detail = {key: fact.get(key) for key in ("step_index", "operation_label", "from_page", "page_label") if key in fact}
        elif kind == "terminal_committed":
            detail = {key: fact.get(key) for key in ("outcome", "final_page", "executed_steps", "failure_code",
                                                      "failure_severity", "scheduling_disposition")}
            timing = fact.get("task_timing") or {}
            detail["task_failure"] = timing.get("task_failure")
            for boundary in ("post_input_wait", "postcondition_wait", "page_recognition_wait", "retry_wait", "capture_page"):
                detail[boundary] = [{key: summary.get(key) for key in ("status", "attempts", "count", "total_us", "max_us")}
                                    for summary in find_key(timing, boundary) if isinstance(summary, dict)]
        elif kind == "package_admitted":
            detail = {key: fact.get(key) for key in ("package_label", "task_label")}
        else:
            detail = {key: value for key, value in fact.items() if key != "kind"}
        say(label, "fact", sequence, stamp, event_type, kind, short(json.dumps(detail, ensure_ascii=False), 1500))


def read_case(label, ledger, root):
    events, failure = ledger_events(ledger, root)
    if events is None:
        say(label, "events", "failed", failure)
        FAILURES.append(f"{label}.ledger_events")
        events = []
    all_facts = facts_of(events)
    facts = first_run(all_facts)
    say(label, "ledger events", len(events), "task facts", len(all_facts),
        "terminal facts (runs)", len(kinds(all_facts, "terminal_committed")))
    print_facts(label, facts)
    for event in events:
        if event.get("event_type") in ("task.failed", "task.completed", "runtime.failed", "command.rejected",
                                       "policy.execution_recorded"):
            say(label, event["event_type"], event["sequence"], short(json.dumps(event.get("payload"), ensure_ascii=False), 2000))
    text = json.dumps(events, ensure_ascii=False)
    return facts, events, text


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


# Facts that are not part of a capture and its recognition.
STRUCTURAL_KINDS = ("package_admitted", "run_started", "entry_recognition", "step_started", "step_finished", "effect_intent",
                    "effect_completed", "selection_evaluated", "finalizing", "terminal_committed")


def fold(entries, is_capture, is_miss_end):
    """Folds consecutive identical unmatched-recognition units (with their capture and timing
    entries) into one: a unit runs from a capture entry to the recognition that ends it."""
    units, current = [], []
    for entry in entries:
        if is_capture(entry):
            current.append(entry)
            if is_miss_end(entry) is not None:
                units.append(("miss" if is_miss_end(entry) else "unit", tuple(json.dumps(item, sort_keys=True) for item in current)))
                current = []
        else:
            if current:
                units.append(("unit", tuple(json.dumps(item, sort_keys=True) for item in current)))
                current = []
            units.append(("unit", (json.dumps(entry, sort_keys=True),)))
    if current:
        units.append(("unit", tuple(json.dumps(item, sort_keys=True) for item in current)))
    folded = []
    for unit in units:
        if unit[0] == "miss" and folded and folded[-1] == unit:
            continue
        folded.append(unit)
    return [item for _, unit in folded for item in unit]


def sampled(entry):
    """A rect click is tapped at a point sampled from the run's action seed, which differs per run:
    the point is replaced by its rect membership (every pack clicks the rect x 8..12, y 8..12)."""
    fact = entry["fact"]
    action = fact.get("action")
    if fact["kind"] == "effect_intent" and isinstance(action, dict) and action.get("kind") == "tap":
        inside = 8 <= action.get("x", -1) <= 12 and 8 <= action.get("y", -1) <= 12
        fact["action"] = {"kind": "tap", "point": "<sampled inside the click rect>" if inside else action}
    return entry


def fold_facts(facts):
    entries = [sampled(normalize({"event_type": event_type, "fact": fact})) for _, event_type, fact, _ in facts]
    return fold(entries,
                lambda entry: entry["fact"]["kind"] not in STRUCTURAL_KINDS,
                lambda entry: (entry["fact"].get("matched_page") is None) if entry["fact"]["kind"] == "recognition_completed" else None)


def fold_trace(trace):
    return fold(trace,
                lambda line: line == "capture" or line.startswith(("recognition", "wait ")),
                lambda line: (" -> None " in line) if line.startswith("recognition ") else None)


def steps(facts, kind):
    return [[fact.get("step_index"), fact.get("operation_label"), fact.get("from_page") if kind == "step_started" else fact.get("page_label")]
            for fact in kinds(facts, kind)]


def g(page):
    return f"{GAME}/{page}"


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


def success(facts, executed_steps, final_page):
    last = terminal(facts)
    return (last.get("outcome") == "success" and last.get("executed_steps") == executed_steps
            and last.get("final_page") == final_page)


FAILURE_CODE = re.compile(r"(contained_task_[a-z0-9_]+|resource_declaration_invalid|page_confirmation_failed"
                          r"|application_effect_requires_assigned_application)")


def failure_codes(text):
    return sorted(set(FAILURE_CODE.findall(text)))


def check_opt_run(label, facts, events, text):
    last = terminal(facts)
    starts, finishes = steps(facts, "step_started"), steps(facts, "step_finished")
    intents = len(kinds(facts, "effect_intent"))
    recognitions = [[fact.get("candidate_pages"), fact.get("matched_page")] for fact in kinds(facts, "recognition_completed")]
    captures = len(kinds(facts, "evidence_indexed"))
    say(label, "summary", json.dumps({"starts": starts, "finishes": finishes, "intents": intents, "captures": captures,
                                      "terminal": {k: last.get(k) for k in ("outcome", "final_page", "executed_steps", "failure_code")}}))
    no_outcome_failure = "contained_task_outcome_" not in text and "fixture capture exhausted" not in text
    timing = last.get("task_timing") or {}
    retry_waits = [item for item in find_key(timing, "retry_wait") if isinstance(item, dict)]
    limits = list(find_key(last, "limit_ms"))
    s01, o02, s03 = g("s01"), g("o02"), g("s03")
    oa, ob, s04, p1, p2 = g("oa"), g("ob"), g("s04"), g("p1"), g("p2")
    if label == "E1":
        check("E1.events", starts == [[0, "step_01_click", s01], [1, "step_02_click", o02]]
              and finishes == [[0, "step_01_click", o02], [1, "step_02_click", s03]]
              and recognitions[1] == [[o02, s03], o02] and recognitions[2] == [[s03], s03], json.dumps([starts, finishes, recognitions]))
        check("E1.executed_2_inputs_2_scheduled_success", success(facts, 2, s03) and intents == 2 and no_outcome_failure, "")
    elif label == "E2":
        check("E2.skip_writes_nothing", starts == [[0, "step_01_click", s01]] and finishes == [[0, "step_01_click", s03]]
              and "step_02_click" not in json.dumps([fact for _, _, fact, _ in facts]), json.dumps([starts, finishes]))
        check("E2.executed_1_inputs_1_scheduled_success", success(facts, 1, s03) and intents == 1 and no_outcome_failure, "")
        matched = [(stamp, fact.get("matched_page")) for _, _, fact, stamp in facts if fact["kind"] == "recognition_completed"]
        s03_stamps = [stamp for stamp, page in matched if page == s03]
        say("E2", "ledger append times of the s03 recognitions (ms)", json.dumps(s03_stamps),
            "first to deciding", (s03_stamps[-1] - s03_stamps[0]) if len(s03_stamps) > 1 else None)
        postcondition = [item for item in find_key(timing, "postcondition_wait") if isinstance(item, dict)]
        say("E2", "postcondition_wait summary (settle sleeps)", json.dumps(postcondition))
    elif label == "E3":
        check("E3.late_popup_run", finishes == [[0, "step_01_click", o02], [1, "step_02_click", s03]]
              and [r[1] for r in recognitions[1:4]] == [s03, s03, o02], json.dumps([finishes, recognitions]))
        check("E3.executed_2", success(facts, 2, s03) and intents == 2 and no_outcome_failure, "")
    elif label == "E4a":
        check("E4a.order", [s[1] for s in starts] == ["step_01_click", "step_02_click", "step_03_click"]
              and [s[0] for s in starts] == [0, 1, 2], json.dumps(starts))
        check("E4a.executed_3", success(facts, 3, s04) and intents == 3 and no_outcome_failure, "")
    elif label == "E4b":
        check("E4b.order", [s[1] for s in starts] == ["step_01_click", "step_03_click", "step_02_click"]
              and [s[0] for s in starts] == [0, 1, 2], json.dumps(starts))
        after_ob = [fact.get("candidate_pages") for fact in kinds(facts, "recognition_started")]
        check("E4b.candidates_after_ob", after_ob[1] == [oa, ob, s04] and after_ob[2] == [oa, s04] and after_ob[3] == [s04],
              json.dumps(after_ob))
        check("E4b.executed_3", success(facts, 3, s04) and intents == 3 and no_outcome_failure, "")
    elif label == "E4c":
        check("E4c.executed_2", success(facts, 2, s04) and intents == 2 and no_outcome_failure
              and [s[1] for s in starts] == ["step_01_click", "step_03_click"], json.dumps(starts))
    elif label == "E4d":
        check("E4d.executed_1", success(facts, 1, s04) and intents == 1 and no_outcome_failure
              and [s[1] for s in starts] == ["step_01_click"], json.dumps(starts))
    elif label == "E5a":
        check("E5a.executed_3_inputs_3", success(facts, 3, s04) and intents == 3 and no_outcome_failure
              and [f[2] for f in finishes] == [p1, p2, s04], json.dumps(finishes))
    elif label == "E5b":
        check("E5b.executed_2_inputs_2", success(facts, 2, s04) and intents == 2 and no_outcome_failure
              and [f[2] for f in finishes] == [p1, s04], json.dumps(finishes))
    elif label in ("E6", "E8"):
        check(f"{label}.page_confirmation_failed", last.get("failure_code") == "page_confirmation_failed"
              and last.get("executed_steps") == 1 and finishes == [[0, "step_01_click", "<unrecognized>"]]
              and starts == [[0, "step_01_click", s01]] and intents == 1, json.dumps([last.get("failure_code"), starts, finishes]))
        say(label, "limit_ms in the terminal", json.dumps(limits), "task_failure", json.dumps(timing.get("task_failure")))
        if limits:
            check(f"{label}.ledger_limit_about_settle_plus_budget", any(isinstance(v, int) and 1500 <= v <= 1700 for v in limits),
                  json.dumps(limits))
        check(f"{label}.frames_not_exhausted", 0 < captures < 22 and "fixture capture exhausted" not in text, f"captures {captures} of 22")
        if label == "E8":
            check("E8.no_retry_wait", not retry_waits, json.dumps(retry_waits))
    elif label == "E7":
        check("E7.swallowed_then_reached", starts == [[0, "step_01_click", s01], [0, "step_01_click", s01], [1, "step_02_click", o02]]
              and finishes == [[0, "step_01_click", s01], [0, "step_01_click", o02], [1, "step_02_click", s03]], json.dumps([starts, finishes]))
        check("E7.inputs_3_executed_2", success(facts, 2, s03) and intents == 3 and no_outcome_failure, "")
        say("E7", "retry_wait summary", json.dumps(retry_waits))
    elif label == "E9":
        check("E9.optional_close_swallowed_then_retried", starts == [[0, "step_01_click", s01], [1, "step_02_click", o02], [1, "step_02_click", o02]]
              and finishes == [[0, "step_01_click", o02], [1, "step_02_click", o02], [1, "step_02_click", s03]], json.dumps([starts, finishes]))
        check("E9.inputs_3_executed_2", success(facts, 2, s03) and intents == 3 and no_outcome_failure, "")


def run(work, new_runtime, new_tools, l2e_runtime, l2a_runtime, v091_runtime, v091_tools, v090_tools, catalog_dir):
    os.environ["CATALOG_DIR"] = catalog_dir
    new_ledger = os.path.join(new_tools, "actingledger.exe")
    produced = []

    # E1-E9 on the product build, three daemons at a time.
    jobs = [(label, new_runtime, pack, names) for label, pack, names, _code, _detail, _inputs in OPT_RUNS]
    roots = {}
    for start in range(0, len(jobs), 3):
        roots.update(scheduled_batch(work, jobs[start:start + 3]))
    for label, _pack, _names, _code, _detail, _inputs in OPT_RUNS:
        facts, events, text = read_case(label, new_ledger, roots[label])
        produced.append((label, roots[label]))
        check_opt_run(label, facts, events, text)

    # E10 on the product build: refusals before PackageAdmitted, and the admitted restart pack.
    jobs = [(f"E10 {name}", new_runtime, name, ["s01", "s03"]) for name, _code, _detail in OPT_REFUSALS]
    jobs.append(("E10 segment_ok", new_runtime, "segment_ok", ["s02", "o03", "home"]))
    roots = {}
    for start in range(0, len(jobs), 8):
        roots.update(scheduled_batch(work, jobs[start:start + 8]))
    for name, code, _detail in OPT_REFUSALS:
        label = f"E10 {name}"
        facts, events, text = read_case(label, new_ledger, roots[label])
        produced.append((label, roots[label]))
        admitted = kinds(facts_of(events), "package_admitted")
        check(f"E10.{name}.refused_before_package_admitted", code in text and not admitted,
              f"{code} in ledger: {code in text}; package_admitted facts {len(admitted)}")
    facts, events, text = read_case("E10 segment_ok", new_ledger, roots["E10 segment_ok"])
    produced.append(("E10 segment_ok", roots["E10 segment_ok"]))
    fact_kinds = [item[2]["kind"] for item in facts]
    capture_events = [event.get("event_type") for event in events
                      if str(event.get("event_type", "")).startswith("capture.") and event.get("event_type") != "capture.summary_committed"]
    check("E10.segment_ok.admitted_then_refused_before_any_capture",
          "package_admitted" in fact_kinds and terminal(facts).get("failure_code") == APPLICATION_REFUSAL
          and "evidence_indexed" not in fact_kinds and "recognition_started" not in fact_kinds and not capture_events,
          json.dumps([fact_kinds, capture_events]))

    # E11: the L2a and L2e evidence packs on the L2e and product builds; the page-graph pack on
    # v0.9.1 and the product build. Semantic fact subsequences with misses folded.
    jobs, pairs = [], []
    for index, (pack, names) in enumerate(L2A_RUNS + L2E_RUNS):
        for build, runtime_dir in (("L2e", l2e_runtime), ("product", new_runtime)):
            jobs.append((f"E11 {build} {index:02d} {pack}", runtime_dir, pack, names))
        pairs.append((f"E11 L2e {index:02d} {pack}", f"E11 product {index:02d} {pack}", f"{index:02d} {pack}"))
    for build, runtime_dir in (("v0.9.1", v091_runtime), ("product", new_runtime)):
        jobs.append((f"E11 {build} navigable_route toggle", runtime_dir, "toggle", ["off", "on", "on", "on"]))
    pairs.append(("E11 v0.9.1 navigable_route toggle", "E11 product navigable_route toggle", "navigable_route toggle"))
    roots = {}
    for start in range(0, len(jobs), 8):
        roots.update(scheduled_batch(work, jobs[start:start + 8]))
    for old_label, new_label, name in pairs:
        old_facts, _, old_text = read_case(old_label, new_ledger, roots[old_label])
        new_facts, _, new_text = read_case(new_label, new_ledger, roots[new_label])
        produced.extend([(old_label, roots[old_label]), (new_label, roots[new_label])])
        old_seq, new_seq = fold_facts(old_facts), fold_facts(new_facts)
        difference = next((i for i, (a, b) in enumerate(zip(old_seq, new_seq)) if a != b), None)
        # A package refused before admission writes no task fact: its failure codes are compared.
        old_codes, new_codes = failure_codes(old_text), failure_codes(new_text)
        same = (len(old_seq) == len(new_seq) and difference is None
                and (len(old_seq) > 0 or (old_codes == new_codes and len(old_codes) > 0)))
        say("E11", name, "folded facts", len(old_seq), len(new_seq), "first difference", difference,
            "terminal", json.dumps(terminal(old_facts).get("failure_code")), json.dumps(terminal(new_facts).get("failure_code")),
            "codes", json.dumps(old_codes), json.dumps(new_codes))
        if difference is not None or len(old_seq) != len(new_seq):
            for i in range(max(len(old_seq), len(new_seq))):
                say("E11", name, i, short(old_seq[i] if i < len(old_seq) else "-", 600), short(new_seq[i] if i < len(new_seq) else "-", 600))
        check(f"E11.identical_folded_facts.{name.replace(' ', '_')}", same, f"{len(old_seq)} vs {len(new_seq)}")

    # E12: older builds refuse OPT1 before PackageAdmitted.
    jobs = [(f"E12 {build}", runtime_dir, "opt1", ["s01", "o02", "s03"])
            for build, runtime_dir in (("v0.9.1", v091_runtime), ("L2a", l2a_runtime), ("L2e", l2e_runtime))]
    roots = scheduled_batch(work, jobs)
    for label, _runtime_dir, _pack, _names in jobs:
        facts, events, text = read_case(label, new_ledger, roots[label])
        admitted = kinds(facts_of(events), "package_admitted")
        check(f"{label.replace(' ', '_')}.resource_declaration_invalid_before_package_admitted",
              "resource_declaration_invalid" in text and not admitted, f"package_admitted facts {len(admitted)}")
        say(label, "field path /operations/1/optional in the ledger", "/operations/1/optional" in text,
            "UnknownField in the ledger", "UnknownField" in text)

    # E13: the v0.9.1 and v0.9.0 actingledger read every ledger of E1-E11.
    for build, tools in (("v0.9.1", v091_tools), ("v0.9.0", v090_tools)):
        old_ledger = os.path.join(tools, "actingledger.exe")
        for label, root in produced:
            copy = root + f"-read-copy-{build}"
            shutil.copytree(root, copy)
            before = tree_hashes(copy)
            results = ledger_reads(f"E13 {build} actingledger on {label}", old_ledger, copy)
            after = tree_hashes(copy)
            slug = f"{build}.{label.replace(' ', '_')}"
            check(f"E13.reads.{slug}", all(code == 0 and not corrupt for code, corrupt in results.values()), json.dumps(results))
            check(f"E13.copy_unchanged.{slug}", before == after, f"{len(before)} files")
    say("RESULT", "failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


# ---------------------------------------------------------------------------------------------
# kernel: the in-process results of the product and L2e harnesses.

def load_lines(path):
    with open(path, encoding="utf-8") as handle:
        return [json.loads(line) for line in handle if line.strip()]


def kernel(work, product_path, l2e_path):
    product, l2e = load_lines(product_path), load_lines(l2e_path)
    by_name = {(item["kind"], item["name"]): item for item in product}
    runs = {item["name"]: item for item in product if item["kind"] == "run"}
    # E2: the frame that decided s03 was captured at least the settle after s03 first passed.
    for name in ("E2-opt1", "E4c-opt2", "E4d-opt2", "E5b-opt3"):
        item = runs[name]
        times = []
        for line in item["timeline"]:
            match = re.match(r"recognition \[.*\] -> Some\(\"([^\"]+)\"\).* \(capture at (\d+)us\)$", line)
            if match:
                times.append((match.group(1), int(match.group(2))))
        target = g("s03") if name.startswith("E2") else g("s04")
        skip = [t for page, t in times if page == target]
        say("E2", name, "capture times of the skip-target frames (us)", json.dumps(skip),
            "first to deciding (us)", skip[-1] - skip[0] if len(skip) > 1 else None)
        # The runtime's capture call starts microseconds after the kernel's capture start.
        check(f"E2.settle_held.{name}", len(skip) >= 2 and skip[-1] - skip[0] >= 499_900, json.dumps(skip))
    # E6/E8: no retry wait, no back-to-back captures once the settle is over, limit S + T.
    for name in ("E6-opt1", "E8-opt1p"):
        item = runs[name]
        check(f"{name}.no_retry_wait_boundary", "RetryWait" not in item["boundaries"], json.dumps(item["boundaries"]))
        check(f"{name}.frames_not_exhausted", item["captures"] < item["frames_given"], f"{item['captures']} of {item['frames_given']}")
        limit = re.search(r"limit_ms: (\d+)", item["timing"])
        say(name, "timing", item["timing"])
        check(f"{name}.limit_about_settle_plus_budget", limit is not None and 1500 <= int(limit.group(1)) <= 1700, item["timing"])
        captures = [int(m.group(1)) for m in (re.match(r"capture #\d+ at (\d+)us$", line) for line in item["timeline"]) if m]
        gaps = [b - a for a, b in zip(captures, captures[1:])]
        say(name, "capture gaps after the first (us)", json.dumps(gaps))
        check(f"{name}.no_back_to_back_captures", all(gap >= 50_000 for gap in gaps[1:]), json.dumps(gaps))
    check("E7.retry_wait_boundary", "RetryWait" in runs["E7-opt1p"]["boundaries"], json.dumps(runs["E7-opt1p"]["boundaries"]))
    check("E2.settle_sleeps_report_postcondition_wait", runs["E2-opt1"]["boundaries"].get("PostconditionWait", 0) >= 4,
          json.dumps(runs["E2-opt1"]["boundaries"]))
    for name in ("E1-opt1", "E4a-opt2", "E4b-opt2", "E9-opt1pp"):
        say(name, "progress (update_run_progress)", json.dumps(runs[name]["progress"]))
    # The run start writes progress 0 (as on L2e); each dispatch then writes d + 1.
    check("E4b.progress_is_dispatch_count", runs["E4b-opt2"]["progress"] == [0, 1, 2, 3], json.dumps(runs["E4b-opt2"]["progress"]))
    # E11: the L2a/L2e cases on both sources.
    for item in l2e:
        mine = by_name.get((item["kind"], item["name"]))
        if mine is None:
            check(f"E11.kernel.present.{item['name']}", False, "missing in the product output")
            continue
        if item["kind"] == "admission":
            same = mine["code"] == item["code"] and mine["detail"] == item["detail"] and mine["issue"] == item["issue"]
            say("E11", "kernel admission", item["name"], "L2e", item["code"], item["detail"], "product", mine["code"], mine["detail"])
        elif item["kind"] == "offline":
            same = mine["decision"] == item["decision"]
        else:
            same_detail = mine["code"] == item["code"] and mine["detail"] == item["detail"] and mine["inputs"] == item["inputs"]
            old_trace, new_trace = fold_trace(item["trace"]), fold_trace(mine["trace"])
            same = same_detail and old_trace == new_trace
            say("E11", "kernel run", item["name"], "code", item["code"], mine["code"], "detail byte-identical",
                mine["detail"] == item["detail"], "detail", json.dumps(mine["detail"]), "folded trace identical",
                old_trace == new_trace, len(old_trace), len(new_trace))
            if old_trace != new_trace:
                for index in range(max(len(old_trace), len(new_trace))):
                    say("E11", item["name"], index, old_trace[index] if index < len(old_trace) else "-",
                        new_trace[index] if index < len(new_trace) else "-")
        check(f"E11.kernel.{item['kind']}.{item['name']}", same, "")
    say("RESULT", "kernel comparison failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


if __name__ == "__main__":
    command = sys.argv[1]
    if command == "static":
        sys.exit(static(sys.argv[2], sys.argv[3], sys.argv[4]))
    if command == "prepare":
        sys.exit(prepare(sys.argv[2]))
    if command == "kernel":
        sys.exit(kernel(sys.argv[2], sys.argv[3], sys.argv[4]))
    sys.exit(run(*sys.argv[2:11]))
