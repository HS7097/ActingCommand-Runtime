# One-off (to be reverted), Workflow #339 L3c evidence (frozen model, board comment 5959674074,
# section 6.4 "L3c evidence"). Every printed line starts with "L3C|". Usage:
#   evidence.py prepare <work> <umbrella standard package zip>
#   evidence.py run <work> <new tools dir> <L3b tools dir> <L3 tools dir> <roundtrip exe> <repo> <product sha>
import hashlib
import io
import json
import os
import re
import shutil
import subprocess
import sys
import zipfile

from PIL import Image

SCHEMA_DIR = "actingcommand.package.content-directory.v1"
SOURCE_PACKAGE_ID = "bluearchive.jp.battle_auto_enable"
L3B_SHA = "7a45d62519be33c253bb056fbfca2dab1273384d"
MARK_SCHEMA = "actingcommand.lab-record-mark.v1"
FAILURES = []
ENV = dict(os.environ)


def say(*parts):
    print("L3C|" + "|".join(str(part) for part in parts), flush=True)


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


def save_png(image, path):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    image.save(path, format="PNG")


def prepare(work, bundle_zip):
    os.makedirs(work, exist_ok=False)
    with zipfile.ZipFile(bundle_zip) as bundle:
        names = bundle.namelist()
        index_name = next(name for name in names if name.endswith("bundle.json"))
        index = json.loads(bundle.read(index_name))
        packs = [pack for pack in index["packs"] if pack["package_id"] == SOURCE_PACKAGE_ID]
        prefix = index_name[: -len("bundle.json")] + packs[0]["path"] + "/"
        files = {name[len(prefix):]: bundle.read(name) for name in names if name.startswith(prefix) and not name.endswith("/")}
    check("prepare.source_digest", digest(files) == packs[0]["digest"], digest(files))
    assets = "resources/operations/battle_auto_enable/assets/"
    images = {name: Image.open(io.BytesIO(files[assets + name])).convert("RGB") for name in ("auto_off.png", "auto_on.png", "battle_cost_label.png")}

    def frame(state, band=False):
        image = Image.new("RGB", (1280, 720), (32, 32, 32))
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
        return image

    def popup():
        # A notice pop-up over the dimmed home screen: a light panel with a close button.
        image = frame("on").point(lambda value: value // 2)
        for x in range(300, 980):
            for y in range(160, 560):
                image.putpixel((x, y), (235, 235, 240))
        image.paste(images["battle_cost_label.png"], (600, 480))
        return image

    frames = os.path.join(work, "frames")
    save_png(frame("off"), os.path.join(frames, "title.png"))
    save_png(frame("off", band=True), os.path.join(frames, "loading.png"))
    save_png(frame("on"), os.path.join(frames, "home.png"))
    save_png(popup(), os.path.join(frames, "popup.png"))
    sizes = {name: list(image.size) for name, image in images.items()}
    with open(os.path.join(work, "cases.json"), "w", encoding="utf-8") as handle:
        json.dump({"sizes": sizes}, handle, indent=2)
    say("prepare", "frames", sorted(os.listdir(frames)), "asset sizes", json.dumps(sizes))


def run_exe(args, timeout=300, env=None):
    try:
        result = subprocess.run(args, capture_output=True, timeout=timeout, env=env or ENV)
    except subprocess.TimeoutExpired:
        return None, "", "timeout"
    return result.returncode, result.stdout.decode("utf-8", "replace"), result.stderr.decode("utf-8", "replace")


def short(text, limit=900):
    text = text.strip().replace("\r", "").replace("\n", " ")
    return text if len(text) <= limit else text[:limit] + "..."


def lab(exe, args, label, env=None, limit=1800, timeout=300):
    code, out, err = run_exe([exe, "--json", *args], env=env, timeout=timeout)
    try:
        value = json.loads(out)
    except ValueError:
        value = None
    error = (value or {}).get("error")
    say(label, "exit", code, "envelope", short(out, limit) if out else short(err, 600))
    return code, value, error


def data_of(value):
    return (value or {}).get("data") or {}


def file_sha(path):
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def recording_file(state):
    for folder, _dirs, names in os.walk(os.path.join(state, "record-artifacts")):
        if "recording.json" in names:
            return os.path.join(folder, "recording.json")
    return None


def load(state):
    with open(recording_file(state), encoding="utf-8") as handle:
        return json.load(handle)


def state_of(data):
    return data.get("step_state") or {}


class Session:
    def __init__(self, exe, work, name, task, record_id=None, tag="E"):
        self.exe = exe
        self.state = os.path.join(work, name)
        self.tag = tag
        self.base = ["--instance", "emu-a"]
        args = ["record", "start", "--task-id", task, "--locale", "en-US", "--state-dir", self.state]
        if record_id:
            args += ["--record-id", record_id]
        code, value, _ = lab(exe, self.base + args, f"{tag} record start ({name})")
        check(f"{tag}.{name}.start", code == 0 and (data_of(value).get("lab_recording") or {}).get("status") == "active", f"exit {code}")

    def mark(self, label, args, exit_code=0, error_code=None, exe=None, limit=1800):
        code, value, error = lab(exe or self.exe, self.base + ["record", "mark", "--state-dir", self.state, *args], f"{self.tag} {label}", limit=limit)
        got = (error or {}).get("code")
        if error_code:
            check(f"{self.tag}.{label}", code == exit_code and got == error_code, f"exit {code} code {got}")
        else:
            check(f"{self.tag}.{label}", code == exit_code, f"exit {code} code {got}")
        return code, value, error

    def status(self, label, exe=None):
        return lab(exe or self.exe, self.base + ["record", "status", "--state-dir", self.state], f"{self.tag} {label}", limit=6000)

    def sha(self):
        return file_sha(recording_file(self.state))


def marks(sizes):
    w, h = sizes["auto_off.png"]
    bw, bh = sizes["battle_cost_label.png"]
    return {
        "title": ["--template", f"ui/title=1180,664,{w},{h}", "--color", "state/title=1173,680,1,1", "--page", "title", "--click-from", "ui/title"],
        "popup": ["--template", f"notice/close=600,480,{bw},{bh}", "--color", "notice/panel=320,180,8,8", "--page", "notice", "--click-from", "notice/close"],
        "home": ["--template", f"ui/home=1180,664,{w},{h}", "--color", "state/home=1173,680,1,1", "--page", "home"],
    }


# E1. The offline session of the frozen model: start, a frame and its click, the pop-up frame
# with its click marked --optional --settle-ms 1500, --close-step, the next frame, status.
def offline_session(new_lab, work, sizes, frames):
    say("E1", "offline session: title + click; pop-up + click + --optional --settle-ms 1500; --close-step; home; status")
    m = marks(sizes)
    s = Session(new_lab, work, "e1", "daily_notice", tag="E1")
    code, value, _ = s.mark("mark title frame and click", ["--frame", os.path.join(frames, "title.png"), *m["title"]])
    data = data_of(value)
    check("E1.title_step_1_not_optional", data.get("step") == 1 and state_of(data).get("optional") is None and "optional" in state_of(data), json.dumps(state_of(data)))
    code, value, _ = s.mark("mark pop-up frame, click, --optional --settle-ms 1500",
                            ["--frame", os.path.join(frames, "popup.png"), *m["popup"], "--optional", "--settle-ms", "1500"])
    data = data_of(value)
    check("E1.popup_step_2_optional", data.get("status") == "marks_recorded" and data.get("step") == 2 and data.get("step_opened") is True
          and data.get("closed_step") == 1 and state_of(data).get("optional") == {"settle_ms": 1500}
          and state_of(data).get("effect") == "click_declared", json.dumps({"step": data.get("step"), "closed_step": data.get("closed_step"), "step_state": state_of(data)}))
    env = dict(ENV)
    env["ACTINGLAB_SESSION_STATE_DIR"] = s.state
    before = s.sha()
    code, value, error = lab(new_lab, s.base + ["capture", "--record"], "E1 capture --record before --close-step", env=env)
    check("E1.capture_refused_before_close", code == 3 and (error or {}).get("code") == "record_step_click_not_executed", f"exit {code} code {(error or {}).get('code')}")
    check("E1.capture_refusal_wrote_nothing", s.sha() == before, before)
    code, value, _ = s.mark("--close-step", ["--close-step"])
    data = data_of(value)
    check("E1.close_step_2", data.get("status") == "step_closed" and data.get("step") == 2 and state_of(data).get("closed") is True
          and state_of(data).get("optional") == {"settle_ms": 1500}, json.dumps(state_of(data)))
    before = s.sha()
    code, value, error = lab(new_lab, s.base + ["capture", "--record"], "E1 capture --record after --close-step (no Runtime or device in CI)", env=env, timeout=180)
    check("E1.capture_passes_record_preflight_after_close", (error or {}).get("code") != "record_step_click_not_executed", f"exit {code} code {(error or {}).get('code')}")
    check("E1.capture_after_close_wrote_nothing", s.sha() == before, before)
    code, value, _ = s.mark("mark home frame (next frame)", ["--frame", os.path.join(frames, "home.png"), *m["home"]])
    data = data_of(value)
    check("E1.home_opens_step_3", data.get("step") == 3 and data.get("step_opened") is True and data.get("closed_step") is None
          and state_of(data).get("optional") is None, json.dumps({"step": data.get("step"), "closed_step": data.get("closed_step")}))
    code, value, _ = s.status("record status")
    steps = (data_of(value).get("lab") or {}).get("steps") or []
    for step in steps:
        say("E1", "status step", json.dumps({key: step.get(key) for key in ("index", "artifact_step", "page", "optional", "closed", "closed_by")}))
    check("E1.status_optional", [step.get("optional") for step in steps] == [None, {"settle_ms": 1500}, None]
          and [step.get("closed_by") for step in steps] == ["offline_frame", "author", None], json.dumps([step.get("optional") for step in steps]))
    stored = load(s.state)
    say("E1", "recording.json steps", json.dumps([{key: step.get(key) for key in ("index", "page", "optional", "closed", "closed_by")} for step in stored["steps"]]))
    optional = stored["steps"][1].get("optional") or {}
    check("E1.recording_json_optional", set(optional) == {"settle_ms", "marked_at_unix_ms"} and optional.get("settle_ms") == 1500
          and isinstance(optional.get("marked_at_unix_ms"), int) and optional["marked_at_unix_ms"] > 0
          and "optional" not in stored["steps"][0] and "optional" not in stored["steps"][2], json.dumps(optional))

    say("E2", "settle defaults, clearing, the request form and --dry-run on the same recording")
    s.tag = "E2"
    marked_at = optional.get("marked_at_unix_ms")
    code, value, _ = s.mark("--step 2 --optional keeps 1500", ["--step", "2", "--optional"])
    after = load(s.state)["steps"][1].get("optional") or {}
    check("E2.keeps_value", state_of(data_of(value)).get("optional") == {"settle_ms": 1500} and after == optional, json.dumps(after))
    code, value, _ = s.mark("--step 2 --optional --settle-ms 3000", ["--step", "2", "--optional", "--settle-ms", "3000"])
    after = load(s.state)["steps"][1].get("optional") or {}
    check("E2.new_settle", state_of(data_of(value)).get("optional") == {"settle_ms": 3000} and after.get("settle_ms") == 3000
          and after.get("marked_at_unix_ms", 0) >= marked_at, json.dumps(after))
    code, value, _ = s.mark("--step 2 --not-optional", ["--step", "2", "--not-optional"])
    stored = load(s.state)
    check("E2.cleared", state_of(data_of(value)).get("optional") is None and all("optional" not in step for step in stored["steps"]), json.dumps(state_of(data_of(value))))
    code, value, _ = s.mark("--step 2 --optional (default 2000)", ["--step", "2", "--optional"])
    check("E2.default_2000", state_of(data_of(value)).get("optional") == {"settle_ms": 2000}
          and load(s.state)["steps"][1].get("optional", {}).get("settle_ms") == 2000, json.dumps(state_of(data_of(value))))
    request = {"schema_version": MARK_SCHEMA, "step": 2, "optional": True, "optional_settle_ms": 1500}
    code, value, _ = s.mark("--request-json optional true, settle 1500", ["--request-json", json.dumps(request)])
    check("E2.request_form", state_of(data_of(value)).get("optional") == {"settle_ms": 1500}, json.dumps(state_of(data_of(value))))
    before = s.sha()
    code, value, _ = s.mark("--step 3 --optional --dry-run", ["--step", "3", "--optional", "--dry-run"])
    check("E2.dry_run", data_of(value).get("status") == "marks_validated" and state_of(data_of(value)).get("optional") == {"settle_ms": 2000}
          and s.sha() == before, json.dumps(state_of(data_of(value))))
    return s


# E3. A second copy of the same pop-up from the same PNG: without --close-step the frame targets
# the open step; after it, the frame opens a new step that reuses the first copy's marks.
def second_copy(new_lab, work, sizes, frames):
    say("E3", "the same pop-up PNG twice: --close-step between the copies")
    m = marks(sizes)
    s = Session(new_lab, work, "e3", "reward_cards", tag="E3")
    s.mark("title frame and click", ["--frame", os.path.join(frames, "title.png"), *m["title"]])
    s.mark("pop-up copy 1 --optional", ["--frame", os.path.join(frames, "popup.png"), *m["popup"], "--optional"])
    code, value, _ = s.mark("same PNG again without --close-step", ["--frame", os.path.join(frames, "popup.png"), "--optional"])
    data = data_of(value)
    check("E3.same_png_targets_open_step", data.get("step") == 2 and data.get("step_opened") is False and data.get("closed_step") is None,
          json.dumps({"step": data.get("step"), "step_opened": data.get("step_opened")}))
    s.mark("--close-step", ["--close-step"])
    code, value, _ = s.mark("pop-up copy 2 (--reuse, same click) --optional",
                            ["--frame", os.path.join(frames, "popup.png"), "--reuse", "notice/close", "--reuse", "notice/panel",
                             "--click-from", "notice/close", "--optional"])
    data = data_of(value)
    check("E3.copy_2_opens_step_3", data.get("step") == 3 and data.get("step_opened") is True and state_of(data).get("optional") == {"settle_ms": 2000}
          and (data.get("click") or {}).get("rect") == (load(s.state)["steps"][1].get("click") or {}).get("rect"), json.dumps(state_of(data)))
    s.mark("--close-step", ["--close-step"])
    s.mark("home frame", ["--frame", os.path.join(frames, "home.png"), *m["home"]])
    code, value, _ = s.status("record status")
    steps = (data_of(value).get("lab") or {}).get("steps") or []
    check("E3.status", [step.get("optional") for step in steps] == [None, {"settle_ms": 2000}, {"settle_ms": 2000}, None]
          and [step.get("reused") for step in steps][2] == ["notice/close", "notice/panel"], json.dumps([step.get("optional") for step in steps]))


# E4. One realistic case per new refusal; every refusal leaves recording.json unchanged.
def refusals(new_lab, work, sizes, frames):
    say("E4", "refusals")
    m = marks(sizes)

    def refused(s, label, args, exit_code, error_code, reason=None):
        before = s.sha()
        code, value, error = s.mark(label, args, exit_code, error_code)
        if reason:
            got = ((error or {}).get("details") or {}).get("reason")
            check(f"E4.{label}.reason", got == reason, json.dumps((error or {}).get("details")))
        check(f"E4.{label}.unchanged", s.sha() == before, before)
        return error

    s = Session(new_lab, work, "e4a", "notice_first", tag="E4")
    say("E4a", "a recording that starts on a pop-up that only sometimes shows")
    refused(s, "first frame is a pop-up marked --optional", ["--frame", os.path.join(frames, "popup.png"), *m["popup"], "--optional"], 3, "record_optional_first_step")
    refused(s, "restart entry step --optional", ["--application", "restart", "--optional"], 3, "record_optional_first_step")

    s = Session(new_lab, work, "e4b", "restart_if_home", tag="E4")
    say("E4b", "a conditional restart: an application step marked --optional")
    s.mark("title frame and click", ["--frame", os.path.join(frames, "title.png"), *m["title"]])
    refused(s, "home frame, --application restart --optional", ["--frame", os.path.join(frames, "home.png"), *m["home"], "--application", "restart", "--optional"],
            3, "record_optional_application")
    s.mark("pop-up --optional", ["--frame", os.path.join(frames, "popup.png"), *m["popup"], "--optional"])
    refused(s, "optional step 2 gets --application restart --replace-click", ["--step", "2", "--application", "restart", "--replace-click"], 3, "record_optional_application")
    error = refused(s, "--settle-ms 90000", ["--step", "2", "--optional", "--settle-ms", "90000"], 2, "record_optional_settle_invalid")
    check("E4.settle_details", (error or {}).get("details") == {"settle_ms": 90000, "max_settle_ms": 60000}, json.dumps((error or {}).get("details")))
    refused(s, "--settle-ms without --optional", ["--step", "2", "--settle-ms", "1500"], 2, "validation_failed")
    refused(s, "--optional with --not-optional", ["--step", "2", "--optional", "--not-optional"], 2, "validation_failed")
    refused(s, "--optional with --close-step", ["--optional", "--close-step"], 2, "validation_failed")
    refused(s, "--optional with --transition window", ["--step", "2", "--optional", "--transition", "window", "--min-ms", "500", "--max-ms", "3000"], 2, "validation_failed")
    request = {"schema_version": MARK_SCHEMA, "step": 2, "optional": False, "optional_settle_ms": 100}
    refused(s, "request optional false with a settle", ["--request-json", json.dumps(request)], 2, "validation_failed")

    s = Session(new_lab, work, "e4c", "loading_optional", tag="E4")
    say("E4c", "a loading screen marked --optional, then --to-transition")
    s.mark("title frame and click", ["--frame", os.path.join(frames, "title.png"), *m["title"]])
    s.mark("loading frame, mark, --optional", ["--frame", os.path.join(frames, "loading.png"), "--color", "load/bar=600,358,8,4", "--optional"])
    refused(s, "--to-transition 2 on the optional step", ["--to-transition", "2"], 3, "record_to_transition_invalid", reason="optional")
    s.mark("--step 2 --not-optional", ["--step", "2", "--not-optional"])
    code, value, _ = s.mark("--to-transition 2 after clearing", ["--to-transition", "2"])
    check("E4.to_transition_after_clear", data_of(value).get("status") == "step_converted_to_transition", data_of(value).get("status"))


def normalized(state):
    with open(recording_file(state), encoding="utf-8") as handle:
        text = handle.read()
    text = text.replace(json.dumps(state)[1:-1], "<state>")
    return re.sub(r'("[a-z_]*_unix_ms": )\d+', r"\g<1>0", text)


def roundtrip(exe, paths, label):
    code, out, err = run_exe([exe, *paths])
    for line in out.splitlines():
        say(label, line)
    lines = [line for line in out.splitlines() if line.startswith("ROUNDTRIP|")]
    check(label + ".roundtrip_identical", code == 0 and len(lines) == len(paths) and all("|identical=true|" in line for line in lines), f"exit {code} {short(err, 300)}")


# E5-E7. Old builds, old recordings and recordings without optional steps.
def compatibility(new_lab, l3b_lab, l3_lab, roundtrip_exe, work, sizes, frames, e1_state):
    m = marks(sizes)
    say("E5", "the same recording without optional steps by the L3b build and by this build, and by the L3 build and by this build")
    sequence_b = [
        ("restart entry step", ["--application", "restart"]),
        ("title frame and click", ["--frame", os.path.join(frames, "title.png"), *m["title"]]),
        ("pop-up frame and click", ["--frame", os.path.join(frames, "popup.png"), *m["popup"]]),
        ("--close-step", ["--close-step"]),
        ("home frame", ["--frame", os.path.join(frames, "home.png"), *m["home"]]),
    ]
    sessions = {}
    for name, exe, sequence in (("e5-l3b", l3b_lab, sequence_b), ("e5-new-b", new_lab, sequence_b),
                                ("e5-l3", l3_lab, sequence_b[1:]), ("e5-new-l3", new_lab, sequence_b[1:])):
        s = Session(exe, work, name, "cold_start", record_id="l3c-same-sequence", tag="E5")
        for label, args in sequence:
            s.mark(f"{name} {label}", args)
        sessions[name] = s
        say("E5", name, "recording.json sha256", s.sha(), "bytes", os.path.getsize(recording_file(s.state)))
    for old, new in (("e5-l3b", "e5-new-b"), ("e5-l3", "e5-new-l3")):
        same = normalized(sessions[old].state) == normalized(sessions[new].state)
        check(f"E5.{old}_equals_{new}_except_times_and_state_dir", same, "")
        check(f"E5.{new}_has_no_optional_key", '"optional"' not in open(recording_file(sessions[new].state), encoding="utf-8").read(), "")
    roundtrip(roundtrip_exe, [recording_file(sessions[name].state) for name in ("e5-l3b", "e5-new-b", "e5-l3", "e5-new-l3")] + [recording_file(e1_state)], "E5")

    say("E6", "the L3b and L3 builds refuse the new flags and the request field; recording.json unchanged")
    for name, exe in (("e5-l3b", l3b_lab), ("e5-l3", l3_lab)):
        s = sessions[name]
        s.tag = "E6"
        before = s.sha()
        code, value, error = s.mark(f"{name} build: --optional --settle-ms 1500", ["--step", "3" if name == "e5-l3b" else "2", "--optional", "--settle-ms", "1500"], 2, "validation_failed", exe=exe)
        check(f"E6.{name}.flag_message", "record mark does not accept: --optional" in ((error or {}).get("message") or ""), (error or {}).get("message"))
        code, value, error = s.mark(f"{name} build: --not-optional", ["--step", "2", "--not-optional"], 2, "validation_failed", exe=exe)
        request = {"schema_version": MARK_SCHEMA, "step": 2, "optional": True}
        code, value, error = s.mark(f"{name} build: request with optional", ["--request-json", json.dumps(request)], 2, "validation_failed", exe=exe)
        check(f"E6.{name}.request_unknown_field", "unknown field `optional`" in ((error or {}).get("message") or ""), (error or {}).get("message"))
        after = s.sha()
        say("E6", name, "recording.json sha256 before", before, "after", after)
        check(f"E6.{name}.sha256_unchanged", before == after, after)

    say("E7", "this build reads the L3b and L3 recordings and marks an optional step on them")
    for name, step in (("e5-l3b", "3"), ("e5-l3", "2")):
        s = sessions[name]
        s.tag = "E7"
        code, value, _ = s.status(f"{name} recording, this build: record status", exe=new_lab)
        steps = (data_of(value).get("lab") or {}).get("steps") or []
        check(f"E7.{name}.readable", code == 0 and steps and all(item.get("optional") is None for item in steps), f"exit {code} steps {len(steps)}")
        before = load(s.state)
        code, value, _ = s.mark(f"{name} recording, this build: --step {step} --optional --settle-ms 1500", ["--step", step, "--optional", "--settle-ms", "1500"], exe=new_lab)
        after = load(s.state)
        index = int(step) - 1
        added = after["steps"][index].pop("optional", None)
        for item in (before, after):
            item.pop("updated_at_unix_ms", None)
        check(f"E7.{name}.only_the_step_gained_optional", code == 0 and (added or {}).get("settle_ms") == 1500 and before == after, json.dumps(added))
        before = s.sha()
        # The L3 build already cannot read the application step of the L3b recording.
        for old_name, exe in (("L3b", l3b_lab),) + ((("L3", l3_lab),) if name == "e5-l3" else ()):
            code, value, error = s.status(f"{name} recording with an optional step, {old_name} build: record status", exe=exe)
            check(f"E7.{name}.{old_name}_refuses_reading", code == 3 and (error or {}).get("code") == "record_lab_unavailable"
                  and "optional" in ((error or {}).get("message") or ""), f"exit {code} code {(error or {}).get('code')}")
            s.mark(f"{name} recording with an optional step, {old_name} build: record mark --step 2 --page x", ["--step", "2", "--page", "x"], 3, "record_lab_unavailable", exe=exe)
        check(f"E7.{name}.old_builds_wrote_nothing", s.sha() == before, before)


def git(repo, *args):
    return subprocess.run(["git", "-C", repo, *args], capture_output=True, check=True).stdout.decode("utf-8")


def static(repo, product_sha):
    say("E8", "files changed from the L3b tip", L3B_SHA, "to", product_sha)
    names = [line for line in git(repo, "diff", "--name-only", L3B_SHA, product_sha).splitlines() if line]
    for line in git(repo, "diff", "--stat", L3B_SHA, product_sha).splitlines():
        say("E8", "stat", line)
    forbidden = [name for name in names if name.startswith(("crates/execution-kernel/", "crates/runtime-host/", "crates/actingcommand-contract/", "crates/ledger/"))]
    check("E8.no_kernel_host_contract_ledger_change", not forbidden, json.dumps(forbidden))
    tests = [line for line in git(repo, "diff", L3B_SHA, product_sha).splitlines() if line.startswith("+") and "#[test]" in line]
    check("E8.no_new_tests", not tests, json.dumps(tests))


def run(work, new_tools, l3b_tools, l3_tools, roundtrip_exe, repo, product_sha):
    with open(os.path.join(work, "cases.json"), encoding="utf-8") as handle:
        sizes = json.load(handle)["sizes"]
    frames = os.path.join(work, "frames")
    new_lab = os.path.join(new_tools, "actinglab.exe")
    l3b_lab = os.path.join(l3b_tools, "actinglab.exe")
    l3_lab = os.path.join(l3_tools, "actinglab.exe")
    config_path = os.path.join(work, "lab-config.json")
    with open(config_path, "w", encoding="utf-8") as handle:
        json.dump({"instances": {"emu-a": {"game": "fixture-game-a", "server": "fixture-server-a"}}}, handle)
    ENV["ACTINGLAB_CONFIG_PATH"] = config_path
    ENV["ACTINGLAB_SESSION_STATE_DIR"] = os.path.join(work, "default-session")
    e1 = {}
    for name, step in (("E1", lambda: e1.setdefault("session", offline_session(new_lab, work, sizes, frames))),
                       ("E3", lambda: second_copy(new_lab, work, sizes, frames)),
                       ("E4", lambda: refusals(new_lab, work, sizes, frames)),
                       ("E5", lambda: compatibility(new_lab, l3b_lab, l3_lab, roundtrip_exe, work, sizes, frames, e1["session"].state)),
                       ("E8", lambda: static(repo, product_sha))):
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
    sys.exit(run(*sys.argv[2:9]))
