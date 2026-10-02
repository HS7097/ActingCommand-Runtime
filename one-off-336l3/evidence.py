# One-off (to be reverted), Workflow #336 L3 evidence (frozen model section 8 L3 items 1-5 and
# the R16-R20 amendment "L3 增项" items 1-6). Every printed line starts with "L3|". Usage:
#   evidence.py prepare <work> <umbrella BA standard package zip>
#   evidence.py run <work> <new runtime dir> <new tools dir> <old tools dir> <lock holder exe> <base sha> <product sha> <repo>
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

from PIL import Image, ImageEnhance

SCHEMA_DIR = "actingcommand.package.content-directory.v1"
BA_PACKAGE_ID = "bluearchive.jp.battle_auto_enable"
FIXTURE_ALIAS = "node.a"
FIXTURE_INSTANCE_ID = "instance_00000000000000000000000000000337"
FAILURES = []
ENV = dict(os.environ)


def say(*parts):
    print("L3|" + "|".join(str(part) for part in parts), flush=True)


def check(label, condition, detail=""):
    say("CHECK", label, "PASS" if condition else "FAIL", detail)
    if not condition:
        FAILURES.append(label)


def digest(files):
    hasher = hashlib.sha256()
    hasher.update((SCHEMA_DIR + "\n").encode())
    for path in sorted(files, key=lambda item: item.encode("utf-8")):
        hasher.update(f"{hashlib.sha256(files[path]).hexdigest()}  {path}\n".encode("utf-8"))
    return hasher.hexdigest()


def reference(sha256):
    return {"schema_version": SCHEMA_DIR, "sha256": sha256}


def pretty(value):
    return (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode("utf-8")


def write_tree(root, files):
    for path, data in files.items():
        target = os.path.join(root, *path.split("/"))
        os.makedirs(os.path.dirname(target), exist_ok=True)
        with open(target, "xb") as handle:
            handle.write(data)


def save_png(image, path):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    image.save(path, format="PNG")


def color_pack():
    """A 64x36 color-only carrier package (one task, two pages told apart by one pixel)."""
    control = {
        "schema_version": "Lab-1y.control.v2", "package_id": "fixture.toggle", "execution_mode": "navigable_route",
        "game": "fixture-game-a", "server": "fixture-server-a", "resolution": {"width": 64, "height": 36},
        "entry_task_id": "toggle", "timeout_ms": 30000, "max_steps": 1,
    }
    task = {
        "schema_version": "0.9", "task_id": "toggle", "game": "fixture-game-a", "server_scope": ["fixture-server-a"],
        "locale": "en-US", "coordinate_space": {"width": 64, "height": 36},
        "defaults": {"template_threshold": 0.97, "color_max_distance": 20, "match_metric": "ccoeff_normed"},
        "goal": "Toggle the fixture state once.", "timeout_ms": 30000, "max_steps": 1,
        "entry_page": "toggle_off", "target_page": "toggle_on",
        "color_probes": [
            {"id": "state/off", "region": {"mode": "rect", "rect": {"x": 10, "y": 10, "width": 1, "height": 1}}, "expected": [224, 225, 227]},
            {"id": "state/on", "region": {"mode": "rect", "rect": {"x": 10, "y": 10, "width": 1, "height": 1}}, "expected": [255, 229, 26]},
        ],
        "page_rules": {"toggle_off": {"required": ["state/off"], "forbidden": ["state/on"]},
                       "toggle_on": {"required": ["state/on"], "forbidden": ["state/off"]}},
        "scheduling_outcome": {"mappings": [{"outcome_key": "toggle_done", "effect": "no_designated_effect", "terminal_pages": ["toggle_on"]}]},
        "operations": [{
            "id": "toggle_once", "purpose": "Press once from the off state, then require the on state.",
            "from": "toggle_off", "to": "toggle_on", "click": {"kind": "point", "x": 10, "y": 10},
            "guard": {"page_id": "toggle_off", "target_id": "state/off", "expected_rect": {"x": 10, "y": 10, "width": 1, "height": 1}, "color_probe": "state/off"},
            "expect_after": {"page_id": "toggle_on", "timeout_ms": 15000, "interval_ms": 500},
            "retryable": False, "max_attempts": 1, "retry_interval_ms": 1, "post_delay_ms": 200,
        }],
    }
    resources = {"schema_version": "1.0", "resources": [], "resource_count": 0}
    return {"control.json": pretty(control), "resources/operations/resources.json": pretty(resources),
            "resources/operations/toggle/task.json": pretty(task)}


def color_frame(state):
    image = Image.new("RGB", (64, 36), (32, 32, 32))
    image.putpixel((10, 10), (224, 225, 227) if state == "off" else (255, 229, 26))
    return image


def prepare(work, bundle_zip):
    os.makedirs(work, exist_ok=False)
    with zipfile.ZipFile(bundle_zip) as bundle:
        names = bundle.namelist()
        index_name = next(name for name in names if name.endswith("bundle.json"))
        index = json.loads(bundle.read(index_name))
        packs = [pack for pack in index["packs"] if pack["package_id"] == BA_PACKAGE_ID]
        prefix = index_name[: -len("bundle.json")] + packs[0]["path"] + "/"
        ba = {name[len(prefix):]: bundle.read(name) for name in names if name.startswith(prefix) and not name.endswith("/")}
    say("prepare", "ba pack", BA_PACKAGE_ID, "files", len(ba), "digest", digest(ba), "bundle digest", packs[0]["digest"])
    check("prepare.ba_digest", digest(ba) == packs[0]["digest"], digest(ba))
    assets = "resources/operations/battle_auto_enable/assets/"
    images = {name: Image.open(io.BytesIO(ba[assets + name])).convert("RGB") for name in ("auto_off.png", "auto_on.png", "battle_cost_label.png")}
    for name, image in images.items():
        say("prepare", "asset", name, "size", image.size, "sha256", hashlib.sha256(ba[assets + name]).hexdigest())

    def frame(state, band=False, brightness=None, size=(1280, 720)):
        image = Image.new("RGB", (1280, 720), (32, 32, 32))
        # A dim gradient so the empty background is not perfectly flat outside the probe areas.
        for x in range(0, 1280, 8):
            for y in range(0, 640, 8):
                image.putpixel((x, y), (32 + (x // 8) % 16, 32 + (y // 8) % 16, 40))
        image.paste(images["auto_off.png" if state == "off" else "auto_on.png"], (1180, 664))
        image.paste(images["battle_cost_label.png"], (778, 649))
        image.putpixel((1173, 680), (224, 225, 227) if state == "off" else (255, 229, 26))
        if band:
            for x in range(200, 1080):
                for y in range(356, 364):
                    image.putpixel((x, y), (90, 200, 255))
        if brightness is not None:
            image = ImageEnhance.Brightness(image).enhance(brightness)
        if size != (1280, 720):
            image = image.crop((0, 0) + size)
        return image

    frames = os.path.join(work, "frames")
    save_png(frame("off"), os.path.join(frames, "f1.png"))
    save_png(frame("off", brightness=1.02), os.path.join(frames, "f1b.png"))
    save_png(frame("off", band=True), os.path.join(frames, "loading.png"))
    save_png(frame("on"), os.path.join(frames, "f2.png"))
    save_png(frame("off", size=(1280, 719)), os.path.join(frames, "small.png"))
    save_png(color_frame("off"), os.path.join(frames, "color-off.png"))
    pack = color_pack()
    pack_digest = digest(pack)
    write_tree(os.path.join(work, "carrier", pack_digest), pack)
    sizes = {name: list(image.size) for name, image in images.items()}
    with open(os.path.join(work, "cases.json"), "w", encoding="utf-8") as handle:
        json.dump({"sizes": sizes, "carrier": pack_digest}, handle, indent=2)
    say("prepare", "frames", sorted(os.listdir(frames)), "carrier", pack_digest)


def run_exe(args, timeout=300, env=None, stdin=None):
    try:
        result = subprocess.run(args, capture_output=True, timeout=timeout, env=env or ENV, input=stdin)
    except subprocess.TimeoutExpired:
        return None, "", "timeout"
    return result.returncode, result.stdout.decode("utf-8", "replace"), result.stderr.decode("utf-8", "replace")


def short(text, limit=900):
    text = text.strip().replace("\r", "").replace("\n", " ")
    return text if len(text) <= limit else text[:limit] + "..."


def lab(exe, args, label, env=None, limit=1600):
    code, out, err = run_exe([exe, "--json", *args], env=env)
    try:
        value = json.loads(out)
    except ValueError:
        value = None
    error = (value or {}).get("error")
    say(label, "exit", code, "envelope", short(out, limit) if out else short(err, 600))
    return code, value, error


def file_sha(path):
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def expect_error(label, code, error, exit_code, error_code):
    got = (error or {}).get("code")
    check(label, code == exit_code and got == error_code, f"exit {code} code {got}")


def offline_session(lab_exe, work, sizes):
    say("E1", "offline session on synthetic frames from the real assets")
    state = os.path.join(work, "e1-state")
    frames = os.path.join(work, "frames")
    w, h = sizes["auto_off.png"]
    base = ["--instance", "emu-a"]
    code, value, error = lab(lab_exe, base + ["record", "start", "--task-id", "battle_auto", "--locale", "en-US", "--state-dir", state], "E1 start")
    lab_recording = ((value or {}).get("data") or {}).get("lab_recording") or {}
    check("E1.start", code == 0 and lab_recording.get("status") == "active", json.dumps(lab_recording))
    request = {
        "schema_version": "actingcommand.lab-record-mark.v1",
        "frame": os.path.join(frames, "f1.png"), "page": "auto_off",
        "add": [
            {"id": "ui/auto_off", "family": "template", "region": {"x": 1180, "y": 664, "width": w, "height": h}},
            {"id": "state/auto_off", "family": "color", "region": {"x": 1173, "y": 680, "width": 1, "height": 1}},
            {"id": "hud/strip", "family": "color_digest", "region": {"x": 0, "y": 640, "width": 1280, "height": 80},
             "columns": 16, "rows": 2, "max_mean_milli": 3000, "max_cell": 30},
            {"id": "text/auto", "family": "ocr", "region": {"x": 1180, "y": 664, "width": w, "height": h},
             "languages": ["en"], "timeout_ms": 1000, "match_mode": "contains", "expected": ["AUTO"],
             "case_sensitive": False, "minimum_confidence": 0.8, "model_ref": "fixture-model", "model_sha256": "0" * 64},
            {"id": "check/hud_off", "family": "check", "all_of": ["ui/auto_off", "state/auto_off"]},
        ],
        "click": {"from": "ui/auto_off"},
    }
    code, value, error = lab(lab_exe, base + ["record", "mark", "--state-dir", state, "--request-json", json.dumps(request)], "E1 mark --frame f1 (five families, click-from)", limit=6000)
    data = (value or {}).get("data") or {}
    tests = {mark["id"]: mark["self_test"] for mark in data.get("marks", [])}
    for mark_id, test in tests.items():
        say("E1", "self_test", mark_id, json.dumps(test))
    check("E1.mark_f1", code == 0 and data.get("status") == "marks_recorded" and data.get("step") == 1
          and tests.get("text/auto", {}).get("status") == "not_evaluated"
          and all(tests[key]["status"] == "passed" for key in ("ui/auto_off", "state/auto_off", "hud/strip", "check/hud_off"))
          and (data.get("click") or {}).get("source") == "from_mark", json.dumps(data.get("step_state")))
    code, value, error = lab(lab_exe, base + ["record", "mark", "--state-dir", state, "--sample", os.path.join(frames, "f1b.png")], "E1 mark --sample f1b", limit=6000)
    data = (value or {}).get("data") or {}
    check("E1.sample", code == 0 and (data.get("step_state") or {}).get("frames") == 2, json.dumps(data.get("step_state")))
    for mark in ((value or {}).get("data") or {}).get("marks", []):
        say("E1", "after sample", mark["id"], json.dumps(mark["self_test"]))
    code, value, error = lab(lab_exe, base + ["record", "mark", "--state-dir", state, "--frame", os.path.join(frames, "loading.png"), "--color", "load/bar=600,358,8,4"], "E1 mark --frame loading", limit=4000)
    data = (value or {}).get("data") or {}
    check("E1.loading_opens_2", code == 0 and data.get("step") == 2 and data.get("step_opened") is True and data.get("closed_step") == 1, f"step {data.get('step')} closed {data.get('closed_step')}")
    code, value, error = lab(lab_exe, base + ["record", "mark", "--state-dir", state, "--to-transition", "2"], "E1 mark --to-transition 2", limit=4000)
    data = (value or {}).get("data") or {}
    check("E1.to_transition", code == 0 and data.get("status") == "step_converted_to_transition", data.get("status"))
    code, value, error = lab(lab_exe, base + ["record", "mark", "--state-dir", state, "--frame", os.path.join(frames, "f2.png"),
                                              "--template", f"ui/auto_on=1180,664,{w},{h}", "--color", "state/auto_on=1173,680,1,1"], "E1 mark --frame f2", limit=4000)
    data = (value or {}).get("data") or {}
    check("E1.f2_opens_3", code == 0 and data.get("step") == 3 and data.get("step_opened") is True, f"step {data.get('step')}")
    code, value, error = lab(lab_exe, base + ["record", "mark", "--state-dir", state, "--step", "3", "--transition", "window", "--min-ms", "1000", "--max-ms", "3000"], "E1 mark --step 3 --transition window")
    expect_error("E1.transition_without_click", code, error, 3, "record_transition_without_click")
    code, value, error = lab(lab_exe, base + ["record", "status", "--state-dir", state], "E1 status", limit=12000)
    steps = ((((value or {}).get("data") or {}).get("lab") or {}).get("steps")) or []
    for step in steps:
        say("E1", "status step", json.dumps({key: step.get(key) for key in ("index", "artifact_step", "page", "dropped", "converted_to_transition", "closed", "closed_by")}),
            "transition", json.dumps((step.get("transition") or {}).get("kind")), "marks", json.dumps([[m["id"], m["self_test"]["status"], m.get("margin")] for m in step.get("marks", [])]))
    numbering = [(step["index"], step["artifact_step"], step["converted_to_transition"]) for step in steps]
    check("E1.status_numbering", numbering == [(1, 1, False), (2, None, True), (3, 2, False)]
          and (steps[0].get("transition") or {}).get("kind") == "page", json.dumps(numbering))


def refusals(lab_exe, work, sizes, config_env):
    say("E2", "refusal envelopes and exit codes")
    state = os.path.join(work, "e2-state")
    frames = os.path.join(work, "frames")
    w, h = sizes["auto_off.png"]
    base = ["--instance", "emu-a"]

    def mark(label, args, exit_code=None, error_code=None, reason=None):
        code, value, error = lab(lab_exe, base + ["record", "mark", "--state-dir", state, *args], "E2 " + label, limit=2500)
        if error_code:
            expect_error("E2." + label, code, error, exit_code, error_code)
            if reason:
                reasons = [entry.get("reason") for entry in ((error or {}).get("details") or {}).get("marks", [])]
                check("E2." + label + ".reason", reason in reasons, json.dumps(reasons))
        else:
            check("E2." + label, code == 0, f"exit {code}")
        return code, value, error

    lab(lab_exe, base + ["record", "start", "--task-id", "refusals", "--locale", "en-US", "--state-dir", state], "E2 start")
    mark("setup step 1", ["--frame", os.path.join(frames, "f1.png"), "--template", f"ui/a=1180,664,{w},{h}", "--color", "state/a=1173,680,1,1", "--click-from", "ui/a"])
    mark("id conflict", ["--template", f"ui/a=1180,664,{w},{h}"], 3, "record_mark_id_conflict")
    mark("reserved id", ["--template", f"page/x=1180,664,{w},{h}"], 3, "record_mark_id_reserved")
    mark("frame size mismatch", ["--sample", os.path.join(frames, "small.png")], 3, "record_frame_size_mismatch")
    mark("region outside frame", ["--color", "out/x=1278,718,5,5"], 3, "record_mark_rejected", "region_outside_frame")
    mark("flat template", ["--template", "flat/bg=1100,700,16,16"], 3, "record_mark_rejected")
    mark("sample self-test fails", ["--sample", os.path.join(frames, "f2.png")], 3, "record_mark_rejected", "self_test_failed")
    mark("second click", ["--click", "100,100,10,10"], 3, "record_click_exists")
    check_a = {"schema_version": "actingcommand.lab-record-mark.v1", "add": [{"id": "check/a", "family": "check", "all_of": ["ui/a", "state/a"]}]}
    mark("check ok", ["--request-json", json.dumps(check_a)])
    check_b = {"schema_version": "actingcommand.lab-record-mark.v1", "add": [{"id": "check/b", "family": "check", "all_of": ["check/a", "ui/a"]}]}
    mark("check nested in check", ["--request-json", json.dumps(check_b)], 3, "record_mark_rejected", "check_member_invalid")
    mark("window invalid", ["--step", "1", "--transition", "window", "--min-ms", "3000", "--max-ms", "1000"], 2, "record_transition_window_invalid")
    mark("transition with click", ["--step", "1", "--transition", "page", "--frame", os.path.join(frames, "loading.png"), "--color", "load/x=600,358,8,4", "--click", "1,1,5,5"], 2, "record_transition_has_click")
    mark("window ok", ["--step", "1", "--transition", "window", "--min-ms", "1000", "--max-ms", "3000"])
    mark("transition exists", ["--step", "1", "--transition", "window", "--min-ms", "1000", "--max-ms", "2000"], 3, "record_transition_exists")
    mark("to-transition on a click step", ["--to-transition", "1"], 3, "record_to_transition_invalid")
    mark("offline frame closes step 1", ["--frame", os.path.join(frames, "f2.png"), "--color", "state/b=1173,680,1,1"])
    mark("frame onto marks without click", ["--frame", os.path.join(frames, "f1.png")], 3, "record_step_click_missing")
    mark("drop a non-last step", ["--drop-step", "1"], 3, "record_step_not_last")
    mark("transition on a step without click", ["--step", "2", "--transition", "window", "--min-ms", "1000", "--max-ms", "3000"], 3, "record_transition_without_click")
    mark("unknown flag", ["--templat", "x=1,1,1,1"], 2, "validation_failed")
    for label, args, code_expected, error_code in (
        ("tap --record", ["--instance", "emu-a", "tap", "10", "10", "--record"], 2, "record_flag_unsupported"),
        ("--record with a value", ["--instance", "emu-a", "capture", "--record", "yes"], 2, "record_flag_takes_no_value"),
        ("--record with --state-dir", ["--instance", "emu-a", "capture", "--record", "--state-dir", state], 2, "record_state_dir_unsupported"),
        ("--tap-rect without --record", ["--instance", "emu-a", "do", "--capture", "--tap-rect", "1,2,3,4"], 2, "validation_failed"),
        ("observe --scene --record", ["--instance", "emu-a", "observe", "--scene", os.path.join(frames, "f1.png"), "--record"], 2, "record_flag_unsupported"),
        ("do --dry-run --record", ["--instance", "emu-a", "do", "--capture", "--dry-run", "--record"], 2, "record_flag_unsupported"),
    ):
        code, value, error = lab(lab_exe, args, "E2 " + label)
        expect_error("E2." + label, code, error, code_expected, error_code)
    # Instance mismatch: the configuration resolves the record instance to emu-a, observe
    # without --instance targets "default"; refused before any package or capture.
    env = dict(ENV)
    env.update(config_env)
    env["ACTINGLAB_SESSION_STATE_DIR"] = state
    carrier = json.load(open(os.path.join(work, "cases.json"), encoding="utf-8"))["carrier"]
    code, out, err = run_exe([lab_exe, "--json", "observe", "--capture", "--record", "--package", os.path.join(work, "carrier", carrier),
                              "--package-ref", json.dumps(reference(carrier))], env=env)
    value = json.loads(out) if out.strip().startswith("{") else {}
    say("E2 instance mismatch", "exit", code, "envelope", short(out))
    expect_error("E2.instance mismatch", code, value.get("error"), 3, "record_instance_mismatch")


def old_compatibility(new_lab, old_lab, work, sizes):
    say("E3", "old record compatibility with the v0.9.0 actinglab")
    state = os.path.join(work, "e3-state")
    frames = os.path.join(work, "frames")
    w, h = sizes["auto_off.png"]
    base = ["--instance", "emu-old"]
    code, value, error = lab(old_lab, base + ["record", "start", "--task-id", "daily_open", "--state-dir", state], "E3 v0.9.0 start")
    record_id = (((value or {}).get("data") or {}).get("record") or {}).get("record_id")
    lab(old_lab, base + ["record", "step", "--state-dir", state, "--kind", "anchor", "--step-id", "home-anchor", "--id", "page/home",
                         "--region", f"1180,664,{w},{h}", "--frame", os.path.join(frames, "f1.png")], "E3 v0.9.0 step anchor")
    lab(old_lab, base + ["record", "step", "--state-dir", state, "--kind", "color-probe", "--step-id", "home-color", "--id", "color/home",
                         "--region", "1173,680,1,1", "--frame", os.path.join(frames, "f1.png")], "E3 v0.9.0 step color-probe")
    record_file = os.path.join(state, "record-emu-old.json")
    before = file_sha(record_file)
    _, old_status, _ = lab(old_lab, base + ["record", "status", "--state-dir", state], "E3 v0.9.0 status")
    _, new_status, _ = lab(new_lab, base + ["record", "status", "--state-dir", state], "E3 new status")
    new_data = dict((new_status or {}).get("data") or {})
    lab_field = new_data.pop("lab", "absent")
    check("E3.status_same", (old_status or {}).get("data") == new_data and lab_field is None, f"lab field {json.dumps(lab_field)}")
    build = ["record", "build-task", "--state-dir", state, "--dry-run", "--out", os.path.join(work, "e3-out"), "--game", "fixture-game-a",
             "--server", "fixture-server-a", "--locale", "en-US"]
    old_code, old_build, _ = lab(old_lab, base + build, "E3 v0.9.0 build-task --dry-run", limit=1500)
    new_code, new_build, _ = lab(new_lab, base + build, "E3 new build-task --dry-run", limit=1500)
    old_body = json.dumps((old_build or {}).get("data") or (old_build or {}).get("error"), sort_keys=True)
    new_body = json.dumps((new_build or {}).get("data") or (new_build or {}).get("error"), sort_keys=True)
    say("E3", "build-task data sha256", hashlib.sha256(old_body.encode()).hexdigest(), hashlib.sha256(new_body.encode()).hexdigest())
    check("E3.build_task_same", old_code == new_code and old_body == new_body, f"exit {old_code}/{new_code}")
    check("E3.record_file_unchanged_by_reads", file_sha(record_file) == before, before)
    code, value, error = lab(new_lab, base + ["record", "mark", "--state-dir", state, "--frame", os.path.join(frames, "f1.png"),
                                              "--template", f"ui/home=1180,664,{w},{h}"], "E3 new record mark on the v0.9.0 session", limit=2500)
    after = file_sha(record_file)
    say("E3", "record file sha256 before", before, "after record mark", after)
    check("E3.mark_lazily_creates_lab", code == 0 and ((value or {}).get("data") or {}).get("step") == 1, f"exit {code}")
    check("E3.record_file_sha256_unchanged", before == after, after)
    code, value, error = lab(new_lab, base + ["record", "start", "--task-id", "daily_open", "--record-id", record_id, "--force", "--state-dir", state], "E3 start --record-id X --force")
    lab_recording = ((value or {}).get("data") or {}).get("lab_recording") or {}
    check("E3.force_reuse_unavailable", code == 0 and lab_recording.get("status") == "unavailable" and lab_recording.get("reason") == "record_id_reused", json.dumps(lab_recording))
    code, value, error = lab(new_lab, base + ["record", "mark", "--state-dir", state, "--frame", os.path.join(frames, "f1.png")], "E3 mark after the forced reuse")
    expect_error("E3.mark_after_reuse", code, error, 3, "record_lab_unavailable")
    code, value, error = lab(new_lab, base + ["record", "status", "--state-dir", state], "E3 status after the forced reuse")
    say("E3", "lab after reuse", json.dumps(((value or {}).get("data") or {}).get("lab")))


def fixture_config(config_dir, state_root, frames):
    os.makedirs(config_dir, exist_ok=False)
    config = {
        "schema_version": "actingcommand.actingd.config.v1", "state_root": state_root, "bind_host": "127.0.0.1", "bind_port": 0,
        "secret_fingerprint_salt": "oneoff-336-l3-fixture-salt-value",
        "instances": [{"alias": FIXTURE_ALIAS, "instance_id": FIXTURE_INSTANCE_ID,
                       "fixture_backend": {"frames": [{"width": 64, "height": 36, "rgb": list(frame.tobytes())} for frame in frames], "max_inputs": 4}}],
    }
    path = os.path.join(config_dir, "actingd.json")
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(config, handle)
    say("E4", "fixture config bytes", os.path.getsize(path), "frames", len(frames))
    return path


def record_pipeline(new_runtime, new_lab, work):
    say("E4", "--record pipeline on the actingd fixture backend")
    e4 = os.path.join(work, "e4")
    runtime_root = os.path.join(e4, "runtime-state")
    session_root = os.path.join(e4, "session-state")
    os.makedirs(runtime_root)
    os.makedirs(session_root)
    frames = [color_frame("off")] + [color_frame("on"), color_frame("off")] * 7
    config = fixture_config(os.path.join(e4, "config"), runtime_root, frames)
    actingd = os.path.join(new_runtime, "actingcommand-actingd.exe")
    actingctl = os.path.join(new_runtime, "actingctl.exe")
    env = dict(ENV)
    env["ACTINGCOMMAND_RUNTIME_STATE_ROOT"] = runtime_root
    env["ACTINGLAB_SESSION_STATE_DIR"] = session_root
    carrier = json.load(open(os.path.join(work, "cases.json"), encoding="utf-8"))["carrier"]
    package = ["--package", os.path.join(work, "carrier", carrier), "--package-ref", json.dumps(reference(carrier))]
    logs = os.path.join(e4, "logs")
    os.makedirs(logs)
    base = ["--instance", FIXTURE_ALIAS]
    with open(os.path.join(logs, "actingd.out"), "wb") as out, open(os.path.join(logs, "actingd.err"), "wb") as err:
        process = subprocess.Popen([actingd, "--config", config], stdout=out, stderr=err, cwd=os.path.dirname(config))
        ready = False
        started = time.time()
        while time.time() - started < 90 and process.poll() is None:
            code, _, _ = run_exe([actingctl, "status", "--state-root", runtime_root], timeout=30)
            if code == 0:
                ready = True
                break
            time.sleep(1)
        say("E4", "actingd ready", ready, "after_s", round(time.time() - started, 1))
        check("E4.actingd_ready", ready, "")
        if ready:
            steps = [
                ("start", ["record", "start", "--task-id", "toggle_rec", "--locale", "en-US"]),
                ("capture --record", ["capture", "--record"]),
                ("mark state/off", ["record", "mark", "--color", "state/off=10,10,1,1"]),
                ("do --capture --tap-rect --record", ["do", "--capture", "--tap-rect", "8,8,5,5", "--record", *package]),
                ("capture --record (opens step 2)", ["capture", "--record"]),
                ("drop step 2", ["record", "mark", "--drop-step", "2"]),
                ("reopen step 1", ["record", "mark", "--reopen-step", "1"]),
                ("do --capture --record again", ["do", "--capture", "--record", *package]),
                ("capture --record after the redo", ["capture", "--record"]),
                ("status", ["record", "status"]),
            ]
            results = {}
            for label, args in steps:
                code, out, err_text = run_exe([new_lab, "--json", *base, *args], env=env, timeout=180)
                say("E4", label, "exit", code, "envelope", short(out, 5000) if out else short(err_text, 800))
                try:
                    results[label] = (code, json.loads(out))
                except ValueError:
                    results[label] = (code, {})
            start = results["start"][1].get("data") or {}
            check("E4.start_reachable", start.get("record_flag_reachable") is True, json.dumps(start.get("record_flag_reachable")))
            capture = (results["capture --record"][1].get("data") or {}).get("record") or {}
            check("E4.capture_record", results["capture --record"][0] == 0 and capture.get("status") == "frame_recorded" and capture.get("step") == 1, json.dumps(capture)[:300])
            do_code, do_value = results["do --capture --tap-rect --record"]
            do_record = ((do_value.get("data") or {}).get("record")) or (((do_value.get("error") or {}).get("details") or {}).get("record")) or {}
            say("E4", "do record", json.dumps(do_record)[:3000])
            check("E4.do_record", do_code == 0 and do_record.get("status") == "click_recorded" and do_record.get("step_closed") is True, f"exit {do_code}")
            status = ((results["status"][1].get("data") or {}).get("lab")) or {}
            for step in status.get("steps", []):
                say("E4", "status step", json.dumps({key: step.get(key) for key in ("index", "artifact_step", "dropped", "closed", "closed_by", "click")}))
            first = next((step for step in status.get("steps", []) if step.get("index") == 1), {})
            check("E4.redo_attempts", (first.get("click") or {}).get("attempts") == 1 and (first.get("click") or {}).get("executed") is True, json.dumps(first.get("click")))
        code, out, err_text = run_exe([actingctl, "request-shutdown", "--state-root", runtime_root, "--wait", "60"], timeout=120)
        say("E4", "request-shutdown exit", code, short(out, 300), short(err_text, 300))
        try:
            process.wait(timeout=120)
        except subprocess.TimeoutExpired:
            process.kill()
    for stream in ("out", "err"):
        with open(os.path.join(logs, "actingd." + stream), "rb") as handle:
            say("E4", "actingd " + stream, short(handle.read().decode("utf-8", "replace"), 1500))
    recording = os.path.join(session_root, "record-artifacts")
    for folder, _dirs, names in os.walk(recording):
        for name in names:
            if name == "recording.json":
                with open(os.path.join(folder, name), encoding="utf-8") as handle:
                    say("E4", "recording.json", short(handle.read(), 6000))


def lock_cases(new_lab, holder_exe, work, sizes):
    say("L", "recording lock (R20)")
    state = os.path.join(work, "lock-state")
    frames = os.path.join(work, "frames")
    w, h = sizes["auto_off.png"]
    base = ["--instance", "emu-lock"]
    lab(new_lab, base + ["record", "start", "--task-id", "lock_case", "--locale", "en-US", "--state-dir", state], "L start")
    lab(new_lab, base + ["record", "step", "--state-dir", state, "--kind", "anchor", "--step-id", "auto-anchor", "--id", "page/auto",
                         "--region", "auto", "--frame", os.path.join(frames, "f1.png")], "L old step (auto region) before the holder")

    def holder():
        process = subprocess.Popen([holder_exe, state, "emu-lock"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        line = process.stdout.readline().decode("utf-8", "replace").strip()
        say("L", "holder", line)
        return process, line

    process, line = holder()
    pid = re.search(r"pid=(\d+)", line)
    holder_pid = int(pid.group(1)) if pid else None
    check("L.holder_locked", line.startswith("LOCKED") and holder_pid == process.pid, line)
    code, value, error = lab(new_lab, base + ["record", "mark", "--state-dir", state, "--frame", os.path.join(frames, "f1.png")], "L1 record mark while held")
    details = (error or {}).get("details") or {}
    expect_error("L1.record_busy", code, error, 3, "record_busy")
    check("L1.holder_pid", (details.get("holder") or {}).get("pid") == holder_pid and "record status" in (error or {}).get("message", ""), json.dumps(details))
    check("L1.blocked_by", (error or {}).get("blocked_by") == ["record_lock"], json.dumps((error or {}).get("blocked_by")))
    code, value, error = lab(new_lab, base + ["record", "step", "--state-dir", state, "--kind", "color-probe", "--step-id", "c1", "--id", "color/c1",
                                              "--region", "1173,680,1,1", "--frame", os.path.join(frames, "f1.png")], "L2 old record step while held")
    expect_error("L2.old_step_busy", code, error, 3, "record_busy")
    code, value, error = lab(new_lab, base + ["record", "status", "--state-dir", state], "L2 record status while held")
    check("L2.status_runs", code == 0, f"exit {code}")
    code, value, error = lab(new_lab, base + ["record", "candidates", "--state-dir", state, "--step-id", "auto-anchor"], "L2 record candidates while held")
    check("L2.candidates_runs", code == 0, f"exit {code} {json.dumps(error)}")
    process.stdin.write(b"\n")
    process.stdin.flush()
    rest = process.stdout.read().decode("utf-8", "replace").strip()
    exit_code = process.wait(timeout=60)
    say("L3", "holder exited normally", exit_code, rest)
    code, value, error = lab(new_lab, base + ["record", "mark", "--state-dir", state, "--frame", os.path.join(frames, "f1.png"),
                                              "--template", f"ui/auto=1180,664,{w},{h}"], "L3 record mark after a normal exit")
    check("L3.mark_after_exit", exit_code == 0 and code == 0, f"holder {exit_code} mark {code}")
    process, line = holder()
    check("L4.holder_locked_again", line.startswith("LOCKED"), line)
    process.kill()
    killed = process.wait(timeout=60)
    say("L4", "holder killed", killed)
    started = time.time()
    code, value, error = lab(new_lab, base + ["record", "mark", "--state-dir", state, "--color", "state/auto=1173,680,1,1"], "L4 record mark right after the kill")
    check("L4.no_stale_lock", code == 0, f"exit {code} after {round(time.time() - started, 2)} s")
    say("L4", "lock files", sorted(name for name in os.listdir(state) if ".lock" in name))
    denied = os.path.join(work, "lock-denied")
    os.makedirs(denied)
    code, out, err = run_exe(["icacls", denied, "/deny", "*S-1-1-0:(W)"])
    say("L5", "icacls deny write", code, short(out, 300), short(err, 300))
    code, value, error = lab(new_lab, ["--instance", "emu-denied", "record", "mark", "--state-dir", denied, "--frame", os.path.join(frames, "f1.png")], "L5 record mark in a directory without write access")
    expect_error("L5.record_lock_failed", code, error, 5, "record_lock_failed")


def r11(repo, base_sha, product_sha):
    say("R11", "old record actions unchanged", "base", base_sha, "product", product_sha)
    path = "apps/actinglab/src/commands/session_record.rs"
    old = subprocess.run(["git", "-C", repo, "show", f"{base_sha}:{path}"], capture_output=True, check=True).stdout.decode("utf-8")
    new = subprocess.run(["git", "-C", repo, "show", f"{product_sha}:{path}"], capture_output=True, check=True).stdout.decode("utf-8")

    def items(text):
        chunks, current, name = {}, [], None
        for line in text.splitlines():
            match = re.match(r"^(?:pub\(crate\) |pub\(super\) )?fn ([a-z0-9_]+)", line)
            if match or re.match(r"^(?:#\[|pub\(crate\) (?:struct|enum)|struct |enum |impl |const |use |//)", line):
                if name:
                    chunks[name] = "\n".join(current).rstrip()
                name = match.group(1) if match else None
                current = []
            if name:
                current.append(line)
        if name:
            chunks[name] = "\n".join(current).rstrip()
        return chunks

    old_items, new_items = items(old), items(new)
    changed = sorted(name for name in old_items if old_items[name] != new_items.get(name))
    added = sorted(name for name in new_items if name not in old_items)
    say("R11", "functions in v0.9.0 file", len(old_items), "changed", json.dumps(changed), "added", json.dumps(added))
    check("R11.only_the_dispatcher_changed", changed == ["run_session_record_inner"], json.dumps(changed))

    def arms(text):
        start = text.index('        "step" => {')
        end = text.index("        other => Err(CliError::usage(format!(", start)
        return text[start:end]

    same_arms = arms(old) == arms(new)
    say("R11", "step/amend/candidates/build-task/promote arms identical", same_arms, "bytes", len(arms(old)))
    check("R11.old_arms_identical", same_arms, "")
    diff = subprocess.run(["git", "-C", repo, "diff", "--unified=2", base_sha, product_sha, "--", path], capture_output=True, check=True).stdout.decode("utf-8")
    for line in diff.splitlines()[:140]:
        say("R11", "diff", line)


def run(work, new_runtime, new_tools, old_tools, holder_exe, base_sha, product_sha, repo):
    with open(os.path.join(work, "cases.json"), encoding="utf-8") as handle:
        cases = json.load(handle)
    sizes = cases["sizes"]
    new_lab = os.path.join(new_tools, "actinglab.exe")
    old_lab = os.path.join(old_tools, "actinglab.exe")
    config_path = os.path.join(work, "lab-config.json")
    with open(config_path, "w", encoding="utf-8") as handle:
        json.dump({"instances": {"emu-a": {"game": "fixture-game-a", "server": "fixture-server-a"}}}, handle)
    config_env = {"ACTINGLAB_CONFIG_PATH": config_path}
    ENV.update(config_env)
    ENV["ACTINGLAB_SESSION_STATE_DIR"] = os.path.join(work, "default-session")
    for name, step in (("E1", lambda: offline_session(new_lab, work, sizes)),
                       ("E2", lambda: refusals(new_lab, work, sizes, config_env)),
                       ("E3", lambda: old_compatibility(new_lab, old_lab, work, sizes)),
                       ("E4", lambda: record_pipeline(new_runtime, new_lab, work)),
                       ("L", lambda: lock_cases(new_lab, holder_exe, work, sizes)),
                       ("R11", lambda: r11(repo, base_sha, product_sha))):
        try:
            step()
        except Exception as error:  # report and continue with the next item
            say(name, "exception", repr(error))
            FAILURES.append(name + ".exception")
    say("RESULT", "failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


if __name__ == "__main__":
    if sys.argv[1] == "prepare":
        prepare(sys.argv[2], sys.argv[3])
        say("RESULT", "prepare failures", len(FAILURES), json.dumps(FAILURES))
        sys.exit(1 if FAILURES else 0)
    sys.exit(run(*sys.argv[2:10]))
