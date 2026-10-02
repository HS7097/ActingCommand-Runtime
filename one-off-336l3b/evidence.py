# One-off (to be reverted), Workflow #336 L3b evidence (R24 amendment, board comment 5958084341,
# section "L3b", evidence items 1-7). Every printed line starts with "L3B|". Usage:
#   evidence.py prepare <work> <umbrella standard package zip>
#   evidence.py run <work> <new runtime dir> <new tools dir> <L3 tools dir> <lock holder exe> <repo> <product sha>
import collections
import hashlib
import io
import json
import os
import re
import subprocess
import sys
import time
import zipfile

from PIL import Image

SCHEMA_DIR = "actingcommand.package.content-directory.v1"
SOURCE_PACKAGE_ID = "bluearchive.jp.battle_auto_enable"
FIXTURE_ALIAS = "node.a"
FIXTURE_INSTANCE_ID = "instance_00000000000000000000000000000338"
L3_SHA = "ef00cc8ea02651ccd74bf5a59f55a3eaa728cda5"
MAIN_BASE_SHA = "8e0ac191ea1aad4c9c8e61898cae5d139c585ee1"
OLD_SHA = "885947c90dd6561d6e5c9b7bae3fe736ec36f7c8"
FAILURES = []
ENV = dict(os.environ)
BASELINE = {}


def say(*parts):
    print("L3B|" + "|".join(str(part) for part in parts), flush=True)


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
        packs = [pack for pack in index["packs"] if pack["package_id"] == SOURCE_PACKAGE_ID]
        prefix = index_name[: -len("bundle.json")] + packs[0]["path"] + "/"
        files = {name[len(prefix):]: bundle.read(name) for name in names if name.startswith(prefix) and not name.endswith("/")}
    say("prepare", "source pack", SOURCE_PACKAGE_ID, "files", len(files), "digest", digest(files), "bundle digest", packs[0]["digest"])
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

    frames = os.path.join(work, "frames")
    # title: the first screen every cold start shows; splash: a loading screen; home: the next page.
    save_png(frame("off"), os.path.join(frames, "title.png"))
    save_png(frame("off", band=True), os.path.join(frames, "splash.png"))
    save_png(frame("on"), os.path.join(frames, "home.png"))
    sizes = {name: list(image.size) for name, image in images.items()}
    with open(os.path.join(work, "cases.json"), "w", encoding="utf-8") as handle:
        json.dump({"sizes": sizes}, handle, indent=2)
    say("prepare", "frames", sorted(os.listdir(frames)))


def run_exe(args, timeout=300, env=None, stdin=None):
    try:
        result = subprocess.run(args, capture_output=True, timeout=timeout, env=env or ENV, input=stdin)
    except subprocess.TimeoutExpired:
        return None, "", "timeout"
    return result.returncode, result.stdout.decode("utf-8", "replace"), result.stderr.decode("utf-8", "replace")


def short(text, limit=900):
    text = text.strip().replace("\r", "").replace("\n", " ")
    return text if len(text) <= limit else text[:limit] + "..."


def lab(exe, args, label, env=None, limit=2500):
    code, out, err = run_exe([exe, "--json", *args], env=env)
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


def expect_error(label, code, error, exit_code, error_code):
    got = (error or {}).get("code")
    check(label, code == exit_code and got == error_code, f"exit {code} code {got}")


def recording_file(state):
    for folder, _dirs, names in os.walk(os.path.join(state, "record-artifacts")):
        if "recording.json" in names:
            return os.path.join(folder, "recording.json")
    return None


def step_summary(step):
    keys = ("index", "artifact_step", "entry", "page", "closed", "closed_by", "converted_to_transition")
    summary = {key: step.get(key) for key in keys}
    summary["application"] = step.get("application")
    summary["click"] = step.get("click")
    summary["transition"] = (step.get("transition") or {}).get("kind")
    summary["frames"] = len(step.get("frames", []))
    summary["marks"] = [mark["id"] for mark in step.get("marks", [])]
    return summary


# 1. Offline session: the application entry step, the title screen, then the main interface
# (R25: a pack with a restart reaches a step marked --page home).
def offline_session(new_lab, work, sizes):
    say("E1", "offline session: record mark --application restart, the title frame, then the home frame (--page home)")
    frames = os.path.join(work, "frames")
    w, h = sizes["auto_off.png"]
    state = os.path.join(work, "e1-state")
    base = ["--instance", "emu-a"]
    code, value, _ = lab(new_lab, base + ["record", "start", "--task-id", "restart_home", "--locale", "en-US", "--state-dir", state], "E1 record start")
    check("E1.start", code == 0 and (data_of(value).get("lab_recording") or {}).get("status") == "active", f"exit {code}")
    code, value, _ = lab(new_lab, base + ["record", "mark", "--state-dir", state, "--application", "restart"], "E1 record mark --application restart")
    data = data_of(value)
    application = data.get("application") or {}
    check("E1.entry_step_declared", code == 0 and data.get("status") == "marks_recorded" and data.get("step") == 1
          and data.get("step_opened") is True and data.get("closed_step") is None
          and application.get("action") == "restart" and application.get("source") == "declared"
          and application.get("executed") is None and (data.get("step_state") or {}).get("effect") == "application_declared"
          and (data.get("step_state") or {}).get("frames") == 0, json.dumps({"application": application, "step_state": data.get("step_state")}))
    code, value, _ = lab(new_lab, base + ["record", "mark", "--state-dir", state, "--frame", os.path.join(frames, "title.png"),
                                          "--template", f"ui/title=1180,664,{w},{h}", "--color", "state/title=1173,680,1,1", "--page", "title",
                                          "--click-from", "ui/title"],
                         "E1 record mark --frame title.png (marks, page title, click)")
    data = data_of(value)
    check("E1.title_opens_step_2", code == 0 and data.get("step") == 2 and data.get("step_opened") is True
          and data.get("closed_step") == 1 and (data.get("step_state") or {}).get("effect") == "click_declared",
          json.dumps({"step": data.get("step"), "closed_step": data.get("closed_step"), "step_state": data.get("step_state")}))
    code, value, _ = lab(new_lab, base + ["record", "mark", "--state-dir", state, "--frame", os.path.join(frames, "home.png"),
                                          "--template", f"ui/home=1180,664,{w},{h}", "--color", "state/home=1173,680,1,1", "--page", "home"],
                         "E1 record mark --frame home.png (marks, --page home)")
    data = data_of(value)
    check("E1.home_opens_step_3", code == 0 and data.get("step") == 3 and data.get("closed_step") == 2, json.dumps(data.get("step_state")))
    code, value, _ = lab(new_lab, base + ["record", "status", "--state-dir", state], "E1 record status", limit=8000)
    steps = (data_of(value).get("lab") or {}).get("steps") or []
    for step in steps:
        say("E1", "status step", json.dumps(step_summary(step), ensure_ascii=False))
    first = steps[0] if steps else {}
    second = steps[1] if len(steps) > 1 else {}
    third = steps[2] if len(steps) > 2 else {}
    check("E1.status_entry_any", first.get("entry") == "any" and first.get("artifact_step") == 1
          and (first.get("application") or {}).get("action") == "restart" and first.get("frames") == []
          and first.get("closed") is True and first.get("closed_by") == "offline_frame", json.dumps(step_summary(first)))
    check("E1.status_title_page", second.get("entry") == "page" and second.get("artifact_step") == 2 and second.get("page") == "title"
          and second.get("application") is None and (second.get("click") or {}).get("outcome") == "declared", json.dumps(step_summary(second)))
    check("E1.status_home_page", third.get("entry") == "page" and third.get("artifact_step") == 3 and third.get("page") == "home"
          and third.get("closed") is False, json.dumps(step_summary(third)))
    path = recording_file(state)
    with open(path, encoding="utf-8") as handle:
        stored = json.load(handle)
    say("E1", "recording.json steps", short(json.dumps([{key: step.get(key) for key in ("index", "page", "frames", "click", "application", "closed", "closed_by")} for step in stored["steps"]]), 3000))
    check("E1.recording_json_application", stored["steps"][0].get("application", {}).get("action") == "restart"
          and all("application" not in step for step in stored["steps"][1:]) and stored["coordinate_space"] == {"width": 1280, "height": 720}, "")

    say("E1b", "force-stop is recorded as stop; the request form declares a non-first application step")
    state = os.path.join(work, "e1b-state")
    lab(new_lab, base + ["record", "start", "--task-id", "stop_case", "--locale", "en-US", "--state-dir", state], "E1b record start")
    code, value, _ = lab(new_lab, base + ["record", "mark", "--state-dir", state, "--application", "force-stop"], "E1b record mark --application force-stop")
    application = data_of(value).get("application") or {}
    check("E1b.force_stop_as_stop", code == 0 and application.get("action") == "stop" and application.get("cli_verb") == "force-stop", json.dumps(application))
    state = os.path.join(work, "e1c-state")
    lab(new_lab, base + ["record", "start", "--task-id", "launch_case", "--locale", "en-US", "--state-dir", state], "E1c record start")
    request = {"schema_version": "actingcommand.lab-record-mark.v1", "frame": os.path.join(frames, "title.png"), "page": "title",
               "add": [{"id": "ui/title", "family": "template", "region": {"x": 1180, "y": 664, "width": w, "height": h}}],
               "application": {"action": "launch"}}
    code, value, _ = lab(new_lab, base + ["record", "mark", "--state-dir", state, "--request-json", json.dumps(request)], "E1c record mark --request-json (frame, mark, application launch)")
    data = data_of(value)
    check("E1c.framed_application_step", code == 0 and data.get("step") == 1 and (data.get("application") or {}).get("action") == "launch"
          and (data.get("step_state") or {}).get("effect") == "application_declared" and (data.get("step_state") or {}).get("frames") == 1,
          json.dumps(data.get("step_state")))
    code, value, _ = lab(new_lab, base + ["record", "status", "--state-dir", state], "E1c record status", limit=4000)
    steps = (data_of(value).get("lab") or {}).get("steps") or []
    check("E1c.entry_page", steps and steps[0].get("entry") == "page", json.dumps([step_summary(step) for step in steps]))


# 2. Refusal envelopes of the new codes, and transitions on an application step.
def refusals(new_lab, work, sizes, config_env):
    say("E2", "refusal envelopes")
    frames = os.path.join(work, "frames")
    w, h = sizes["auto_off.png"]
    base = ["--instance", "emu-a"]

    def mark(state, label, args, exit_code=None, error_code=None, limit=2500):
        code, value, error = lab(new_lab, base + ["record", "mark", "--state-dir", state, *args], "E2 " + label, limit=limit)
        if error_code:
            expect_error("E2." + label, code, error, exit_code, error_code)
        else:
            check("E2." + label, code == 0, f"exit {code} {json.dumps(error)}")
        return code, value, error

    state = os.path.join(work, "e2a-state")
    lab(new_lab, base + ["record", "start", "--task-id", "refusals_a", "--locale", "en-US", "--state-dir", state], "E2a record start")
    before = file_sha(recording_file(state))
    mark(state, "action outside the four", ["--application", "reboot"], 2, "record_application_action_invalid")
    mark(state, "application with a click", ["--frame", os.path.join(frames, "title.png"), "--template", f"ui/a=1180,664,{w},{h}",
                                             "--click-from", "ui/a", "--application", "restart"], 2, "record_application_with_click")
    mark(state, "application with a click retry", ["--application", "restart", "--click-retry", "3"], 2, "record_application_with_click")
    mark(state, "launch from an unmarked frame", ["--frame", os.path.join(frames, "title.png"), "--application", "launch"], 3, "record_application_step_marks_missing")
    check("E2a.refusals_wrote_nothing", file_sha(recording_file(state)) == before, before)
    mark(state, "setup: title frame, marks, declared click", ["--frame", os.path.join(frames, "title.png"), "--template", f"ui/a=1180,664,{w},{h}",
                                                             "--color", "state/a=1173,680,1,1", "--click-from", "ui/a"])
    mark(state, "application on a step with a click", ["--application", "launch"], 3, "record_step_effect_exists")
    code, value, _ = mark(state, "--replace-click replaces the click with the application", ["--application", "launch", "--replace-click"])
    data = data_of(value)
    check("E2a.replaced", data.get("click") is None and (data.get("application") or {}).get("action") == "launch"
          and (data.get("step_state") or {}).get("effect") == "application_declared", json.dumps(data.get("step_state")))
    mark(state, "click on an application step", ["--click", "100,100,10,10"], 3, "record_step_effect_exists")
    mark(state, "click guard on an application step", ["--click-guard", "ui/a"], 2, "record_application_with_click")
    mark(state, "setup: home frame closes step 1 offline", ["--frame", os.path.join(frames, "home.png"), "--template", f"ui/b=1180,664,{w},{h}",
                                                           "--color", "state/b=1173,680,1,1", "--click-from", "ui/b"])
    mark(state, "setup: close step 2", ["--close-step"])
    mark(state, "application entry after a closed step", ["--application", "restart"], 3, "record_application_entry_invalid")

    say("E2b", "the entry step has no frame; a window transition on it; device commands on a declared application step")
    state = os.path.join(work, "e2b-state")
    lab(new_lab, base + ["record", "start", "--task-id", "refusals_b", "--locale", "en-US", "--state-dir", state], "E2b record start")
    mark(state, "setup: entry step", ["--application", "restart"])
    mark(state, "page on the entry step", ["--step", "1", "--page", "title"], 3, "record_step_frame_missing")
    mark(state, "mark on the entry step", ["--step", "1", "--color", "x/y=1173,680,1,1"], 3, "record_step_frame_missing")
    code, value, _ = mark(state, "window transition on the application step", ["--step", "1", "--transition", "window", "--min-ms", "2000", "--max-ms", "30000"])
    check("E2b.window_accepted", (data_of(value).get("transition") or {}).get("kind") == "window", json.dumps(data_of(value).get("transition")))
    env = dict(ENV)
    env.update(config_env)
    env["ACTINGLAB_SESSION_STATE_DIR"] = state
    before = file_sha(recording_file(state))
    code, value, error = lab(new_lab, base + ["capture", "--record"], "E2b capture --record on the declared application step", env=env)
    expect_error("E2b.capture_refused", code, error, 3, "record_step_click_not_executed")
    code, value, error = lab(new_lab, base + ["do", "--capture", "--tap-rect", "1,1,5,5", "--record"], "E2b do --capture --tap-rect --record on the application step", env=env)
    expect_error("E2b.do_refused", code, error, 3, "record_step_effect_exists")
    check("E2b.device_refusals_wrote_nothing", file_sha(recording_file(state)) == before, before)

    say("E2c", "a splash screen becomes the transition of the application step (--to-transition)")
    state = os.path.join(work, "e2c-state")
    lab(new_lab, base + ["record", "start", "--task-id", "refusals_c", "--locale", "en-US", "--state-dir", state], "E2c record start")
    mark(state, "setup: entry step", ["--application", "restart"])
    code, value, _ = mark(state, "splash frame with a mark", ["--frame", os.path.join(frames, "splash.png"), "--color", "load/bar=600,358,8,4"])
    check("E2c.splash_opens_2", data_of(value).get("step") == 2 and data_of(value).get("closed_step") == 1, json.dumps(data_of(value).get("step")))
    code, value, _ = mark(state, "--to-transition 2", ["--to-transition", "2"])
    check("E2c.converted", data_of(value).get("status") == "step_converted_to_transition", data_of(value).get("status"))
    mark(state, "title frame", ["--frame", os.path.join(frames, "title.png"), "--template", f"ui/title=1180,664,{w},{h}", "--page", "title"])
    code, value, _ = lab(new_lab, base + ["record", "status", "--state-dir", state], "E2c record status", limit=8000)
    steps = (data_of(value).get("lab") or {}).get("steps") or []
    for step in steps:
        say("E2c", "status step", json.dumps(step_summary(step), ensure_ascii=False))
    numbering = [(step["index"], step["artifact_step"], step["entry"], step["converted_to_transition"]) for step in steps]
    first_transition = (steps[0].get("transition") or {}) if steps else {}
    check("E2c.transition_on_application_step", numbering == [(1, 1, "any", False), (2, None, "page", True), (3, 2, "page", False)]
          and first_transition.get("kind") == "page" and first_transition.get("source") == "converted_from_step", json.dumps(numbering))


def fixture_config(config_dir, state_root):
    os.makedirs(config_dir, exist_ok=False)
    frames = [color_frame("off"), color_frame("on")]
    config = {
        "schema_version": "actingcommand.actingd.config.v1", "state_root": state_root, "bind_host": "127.0.0.1", "bind_port": 0,
        "secret_fingerprint_salt": "oneoff-336-l3b-fixture-salt-value",
        "instances": [{"alias": FIXTURE_ALIAS, "instance_id": FIXTURE_INSTANCE_ID,
                       "fixture_backend": {"frames": [{"width": 64, "height": 36, "rgb": list(frame.tobytes())} for frame in frames], "max_inputs": 4}}],
    }
    path = os.path.join(config_dir, "actingd.json")
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(config, handle)
    return path


class Daemon:
    def __init__(self, runtime_dir, root, label):
        self.label = label
        self.root = root
        self.runtime_root = os.path.join(root, "runtime-state")
        self.session_root = os.path.join(root, "session-state")
        os.makedirs(self.runtime_root)
        os.makedirs(self.session_root)
        self.config = fixture_config(os.path.join(root, "config"), self.runtime_root)
        self.actingd = os.path.join(runtime_dir, "actingcommand-actingd.exe")
        self.actingctl = os.path.join(runtime_dir, "actingctl.exe")
        self.process = None

    def env(self, config_env):
        env = dict(ENV)
        env.update(config_env)
        env["ACTINGCOMMAND_RUNTIME_STATE_ROOT"] = self.runtime_root
        env["ACTINGLAB_SESSION_STATE_DIR"] = self.session_root
        return env

    def start(self):
        self.out = open(os.path.join(self.root, "actingd.out"), "wb")
        self.err = open(os.path.join(self.root, "actingd.err"), "wb")
        self.process = subprocess.Popen([self.actingd, "--config", self.config], stdout=self.out, stderr=self.err, cwd=os.path.dirname(self.config))
        started = time.time()
        ready = False
        while time.time() - started < 90 and self.process.poll() is None:
            code, _, _ = run_exe([self.actingctl, "status", "--state-root", self.runtime_root], timeout=30)
            if code == 0:
                ready = True
                break
            time.sleep(1)
        say(self.label, "actingd ready", ready, "after_s", round(time.time() - started, 1))
        check(self.label + ".actingd_ready", ready, "")
        return ready

    def stop(self):
        code, out, err = run_exe([self.actingctl, "request-shutdown", "--state-root", self.runtime_root, "--wait", "60"], timeout=120)
        say(self.label, "request-shutdown exit", code, short(out, 300), short(err, 300))
        try:
            exit_code = self.process.wait(timeout=120)
        except subprocess.TimeoutExpired:
            self.process.kill()
            exit_code = "killed"
        self.out.close()
        self.err.close()
        say(self.label, "actingd exit", exit_code)


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


WATCHED = ("command.received", "command.validated", "command.rejected", "application.intent", "application.completed",
           "application.failed", "runtime.failed")


def watched_counts(events):
    types = collections.Counter(event.get("event_type") for event in events)
    return {name: types.get(name, 0) for name in WATCHED}


# A fixture actingd started (one successful actingctl status) and stopped without any command:
# the ledger every later run is compared with.
def baseline(new_runtime, new_ledger, work):
    say("E0", "baseline: a fixture actingd started and stopped without any command")
    daemon = Daemon(new_runtime, os.path.join(work, "e0-baseline"), "E0")
    if daemon.start():
        daemon.stop()
        events = ledger_report("E0", new_ledger, daemon.runtime_root)
        BASELINE["types"] = collections.Counter(event.get("event_type") for event in events)
        BASELINE["watched"] = watched_counts(events)


def ledger_report(label, ledger, root):
    events, failure = ledger_events(ledger, root)
    if events is None:
        say(label, "ledger events", "failed", failure)
        check(label + ".ledger_readable", False, failure)
        return []
    types = collections.Counter(event.get("event_type") for event in events)
    say(label, "ledger events", len(events), "types", json.dumps(dict(sorted(types.items()))))
    for event in events:
        if event.get("event_type") in WATCHED:
            say(label, "event", event.get("sequence"), event.get("event_type"), short(json.dumps(event.get("payload"), ensure_ascii=False), 900))
    return events


def holder_cycle(holder_exe, state, instance, label):
    process = subprocess.Popen([holder_exe, state, instance], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    line = process.stdout.readline().decode("utf-8", "replace").strip()
    say(label, "holder", line)
    return process, line


def release(process, label):
    process.stdin.write(b"\n")
    process.stdin.flush()
    rest = process.stdout.read().decode("utf-8", "replace").strip()
    code = process.wait(timeout=60)
    say(label, "holder released", code, rest)
    return code


# 3. Fixture: the Runtime denies the operation; the error is carried through, nothing recorded.
def fixture_denial(new_runtime, new_lab, new_ledger, holder_exe, work, config_env):
    say("E3", "fixture instance: session app restart --record is denied by the Runtime")
    daemon = Daemon(new_runtime, os.path.join(work, "e3"), "E3")
    base = ["--instance", FIXTURE_ALIAS]
    if daemon.start():
        env = daemon.env(config_env)
        code, value, _ = lab(new_lab, base + ["record", "start", "--task-id", "restart_home", "--locale", "en-US"], "E3 record start", env=env)
        check("E3.start_reachable", data_of(value).get("record_flag_reachable") is True, json.dumps(data_of(value).get("record_flag_reachable")))
        path = recording_file(daemon.session_root)
        before = file_sha(path)
        say("E3", "recording.json sha256 before", before)
        code, value, error = lab(new_lab, base + ["session", "app", "restart", "--record"], "E3 session app restart --record", env=env)
        message = (error or {}).get("message") or ""
        check("E3.denied_error_carried", code == 4 and (error or {}).get("code") == "device_error"
              and "fixture_execution_scope_forbidden" in message and "state=Denied" in message, short(message, 400))
        check("E3.not_indeterminate", (error or {}).get("code") != "record_application_indeterminate", (error or {}).get("code"))
        after = file_sha(path)
        say("E3", "recording.json sha256 after", after)
        check("E3.recording_unchanged", before == after, after)
        process, line = holder_cycle(holder_exe, daemon.session_root, FIXTURE_ALIAS, "E3")
        check("E3.lock_released", line.startswith("LOCKED"), line)
        release(process, "E3")
        code2, value2, error2 = lab(new_lab, base + ["session", "app", "restart"], "E3 plain session app restart (no --record)", env=env)
        check("E3.same_error_as_plain_session_app", code2 == code and (error2 or {}).get("code") == (error or {}).get("code")
              and (error2 or {}).get("message") == message, short((error2 or {}).get("message") or "", 300))
        code, value, error = lab(new_lab, base + ["session", "instance", "app", "force-stop", "--record"], "E3 session instance app force-stop --record", env=env)
        check("E3.instance_app_denied", code == 4 and "fixture_execution_scope_forbidden" in ((error or {}).get("message") or ""), (error or {}).get("code"))
        check("E3.recording_unchanged_2", file_sha(path) == before, file_sha(path))
        code, value, _ = lab(new_lab, base + ["record", "status"], "E3 record status", env=env)
        lab_status = data_of(value).get("lab") or {}
        check("E3.no_step_recorded", lab_status.get("steps") == [] and lab_status.get("open_step") is None, json.dumps(lab_status.get("steps")))
        daemon.stop()
        events = ledger_report("E3", new_ledger, daemon.runtime_root)
        application = [event for event in events if str(event.get("event_type", "")).startswith("application.")]
        check("E3.no_application_events", not application, json.dumps([event.get("event_type") for event in application]))
        say("E3", "watched event counts", json.dumps(watched_counts(events)), "baseline", json.dumps(BASELINE.get("watched")))


# 4 and 5. Gate refusals (the L3 build, dry run, value, state dir) and the busy lock: nothing
# reaches the Runtime.
def gate_and_lock(new_runtime, new_lab, l3_lab, new_ledger, holder_exe, work, config_env):
    say("E4", "gate refusals and the busy lock on a fixture actingd")
    daemon = Daemon(new_runtime, os.path.join(work, "e4"), "E4")
    base = ["--instance", FIXTURE_ALIAS]
    if not daemon.start():
        return
    env = daemon.env(config_env)
    lab(new_lab, base + ["record", "start", "--task-id", "gate_case", "--locale", "en-US"], "E4 record start", env=env)
    path = recording_file(daemon.session_root)
    before = file_sha(path)
    cases = [
        ("L3 build: session app restart --record", l3_lab, ["session", "app", "restart", "--record"], 2, "record_flag_unsupported"),
        ("L3 build: session instance app restart --record", l3_lab, ["session", "instance", "app", "restart", "--record"], 2, "record_flag_unsupported"),
        ("session app restart --record x", new_lab, ["session", "app", "restart", "--record", "x"], 2, "record_flag_takes_no_value"),
        ("session app restart --record --state-dir", new_lab, ["session", "app", "restart", "--record", "--state-dir", daemon.session_root], 2, "record_state_dir_unsupported"),
        ("session app restart --record --dry-run", new_lab, ["session", "app", "restart", "--record", "--dry-run"], 2, "record_flag_unsupported"),
        ("--dry-run session app restart --record", new_lab, ["--dry-run", "session", "app", "restart", "--record"], 2, "record_flag_unsupported"),
        ("session instance app restart --record --dry-run", new_lab, ["session", "instance", "app", "restart", "--record", "--dry-run"], 2, "record_flag_unsupported"),
        ("session app reboot --record", new_lab, ["session", "app", "reboot", "--record"], 2, "record_flag_unsupported"),
    ]
    for label, exe, args, exit_code, error_code in cases:
        code, value, error = lab(exe, base + args, "E4 " + label, env=env)
        expect_error("E4." + label, code, error, exit_code, error_code)
    say("E5", "the recording lock is held by a one-off program calling RecordingLock::acquire")
    process, line = holder_cycle(holder_exe, daemon.session_root, FIXTURE_ALIAS, "E5")
    pid = re.search(r"pid=(\d+)", line)
    holder_pid = int(pid.group(1)) if pid else None
    check("E5.holder_locked", line.startswith("LOCKED") and holder_pid == process.pid, line)
    code, value, error = lab(new_lab, base + ["session", "app", "restart", "--record"], "E5 session app restart --record while the lock is held", env=env)
    expect_error("E5.record_busy", code, error, 3, "record_busy")
    check("E5.holder_named", (((error or {}).get("details") or {}).get("holder") or {}).get("pid") == holder_pid, json.dumps((error or {}).get("details")))
    release(process, "E5")
    check("E4.recording_unchanged", file_sha(path) == before, before)
    daemon.stop()
    events = ledger_report("E4", new_ledger, daemon.runtime_root)
    types = collections.Counter(event.get("event_type") for event in events)
    watched = watched_counts(events)
    check("E4.watched_events_as_baseline", "watched" in BASELINE and watched == BASELINE["watched"]
          and not any(str(name).startswith("application.") for name in types),
          json.dumps({"gate": watched, "baseline": BASELINE.get("watched")}))
    say("E4", "all event types equal to the baseline", types == BASELINE.get("types"),
        json.dumps({"gate": dict(sorted(types.items())), "baseline": dict(sorted((BASELINE.get("types") or {}).items()))}))


def git(repo, *args):
    return subprocess.run(["git", "-C", repo, *args], capture_output=True, check=True).stdout.decode("utf-8")


def function_text(source, name):
    match = re.search(r"^(?:pub\(super\) |pub\(crate\) |pub )?fn " + re.escape(name) + r"\b.*?^}\n", source, re.S | re.M)
    return match.group(0) if match else None


# 6. Zero change to run_session_app and the old record actions; 7. the branch code.
def static(repo, product_sha):
    say("E6", "run_session_app and the old record actions are unchanged", "L3", L3_SHA, "main base", MAIN_BASE_SHA, "v0.9.0", OLD_SHA, "product", product_sha)
    path = "apps/actinglab/src/session_management.rs"
    bodies = {sha: function_text(git(repo, "show", f"{sha}:{path}"), "run_session_app") for sha in (OLD_SHA, MAIN_BASE_SHA, L3_SHA, product_sha)}
    for sha, body in bodies.items():
        say("E6", "run_session_app at", sha[:8], "lines", body.count("\n") if body else None,
            "sha256", hashlib.sha256(body.encode()).hexdigest() if body else None)
    check("E6.run_session_app_identical", all(body is not None for body in bodies.values()) and len(set(bodies.values())) == 1, "")
    record = "apps/actinglab/src/commands/session_record.rs"

    def arms(text):
        start = text.index('        "step" => {')
        end = text.index("        other => Err(CliError::usage(format!(", start)
        return text[start:end]

    old_arms = {sha: arms(git(repo, "show", f"{sha}:{record}")) for sha in (OLD_SHA, L3_SHA, product_sha)}
    for sha, text in old_arms.items():
        say("E6", "old record arms (step, candidates, amend, build-task, promote) at", sha[:8], "bytes", len(text), "sha256", hashlib.sha256(text.encode()).hexdigest())
    check("E6.old_record_arms_identical", len(set(old_arms.values())) == 1, "")
    for line in git(repo, "diff", "--unified=1", L3_SHA, product_sha, "--", path, record).splitlines():
        say("E6", "diff", line)
    say("E7", "branch code of run_session_app_recorded and the Indeterminate rows")
    source = git(repo, "show", f"{product_sha}:{path}")
    for name in ("run_session_app_recorded", "application_not_completed"):
        for line in (function_text(source, name) or "missing\n").splitlines():
            say("E7", name, line)
    steps = git(repo, "show", f"{product_sha}:crates/lab/src/recording/steps.rs")
    for name in ("plan_application", "commit_application"):
        for line in (function_text(steps, name) or "missing\n").splitlines():
            say("E7", name, line)


def run(work, new_runtime, new_tools, l3_tools, holder_exe, repo, product_sha):
    with open(os.path.join(work, "cases.json"), encoding="utf-8") as handle:
        sizes = json.load(handle)["sizes"]
    new_lab = os.path.join(new_tools, "actinglab.exe")
    new_ledger = os.path.join(new_tools, "actingledger.exe")
    l3_lab = os.path.join(l3_tools, "actinglab.exe")
    config_path = os.path.join(work, "lab-config.json")
    with open(config_path, "w", encoding="utf-8") as handle:
        json.dump({"instances": {"emu-a": {"game": "fixture-game-a", "server": "fixture-server-a"}, FIXTURE_ALIAS: {}}}, handle)
    config_env = {"ACTINGLAB_CONFIG_PATH": config_path}
    ENV.update(config_env)
    ENV["ACTINGLAB_SESSION_STATE_DIR"] = os.path.join(work, "default-session")
    for name, step in (("E0", lambda: baseline(new_runtime, new_ledger, work)),
                       ("E1", lambda: offline_session(new_lab, work, sizes)),
                       ("E2", lambda: refusals(new_lab, work, sizes, config_env)),
                       ("E3", lambda: fixture_denial(new_runtime, new_lab, new_ledger, holder_exe, work, config_env)),
                       ("E4", lambda: gate_and_lock(new_runtime, new_lab, l3_lab, new_ledger, holder_exe, work, config_env)),
                       ("E6", lambda: static(repo, product_sha))):
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
