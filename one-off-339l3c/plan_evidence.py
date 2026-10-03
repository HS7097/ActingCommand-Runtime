# One-off (to be reverted), Workflow #339 L3c: session app --record refuses to land on an
# optional step while planning, before any Runtime request. Every printed line starts with
# "L3CP|". Usage:
#   plan_evidence.py prepare <work> <umbrella standard package zip>
#   plan_evidence.py run <work> <new runtime dir> <new tools dir> <repo> <product sha>
import collections
import hashlib
import io
import json
import os
import subprocess
import sys
import time
import zipfile

from PIL import Image

SCHEMA_DIR = "actingcommand.package.content-directory.v1"
SOURCE_PACKAGE_ID = "bluearchive.jp.battle_auto_enable"
FIXTURE_ALIAS = "node.a"
FIXTURE_INSTANCE_ID = "instance_00000000000000000000000000000339"
PREVIOUS_HEAD = "463497bfb6373b6be482d3b235ecd9a4c92969f0"
FAILURES = []
ENV = dict(os.environ)
BASELINE = {}
WATCHED = ("command.received", "command.validated", "command.rejected", "application.intent", "application.completed",
           "application.failed", "runtime.failed")


def say(*parts):
    print("L3CP|" + "|".join(str(part) for part in parts), flush=True)


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

    def frame(state):
        image = Image.new("RGB", (1280, 720), (32, 32, 32))
        for x in range(0, 1280, 8):
            for y in range(0, 640, 8):
                image.putpixel((x, y), (32 + (x // 8) % 16, 32 + (y // 8) % 16, 40))
        image.paste(images["auto_off.png" if state == "off" else "auto_on.png"], (1180, 664))
        image.paste(images["battle_cost_label.png"], (778, 649))
        image.putpixel((1173, 680), (224, 225, 227) if state == "off" else (255, 229, 26))
        return image

    def popup():
        image = frame("on").point(lambda value: value // 2)
        for x in range(300, 980):
            for y in range(160, 560):
                image.putpixel((x, y), (235, 235, 240))
        image.paste(images["battle_cost_label.png"], (600, 480))
        return image

    frames = os.path.join(work, "frames")
    save_png(frame("off"), os.path.join(frames, "title.png"))
    save_png(popup(), os.path.join(frames, "popup.png"))
    sizes = {name: list(image.size) for name, image in images.items()}
    with open(os.path.join(work, "cases.json"), "w", encoding="utf-8") as handle:
        json.dump({"sizes": sizes}, handle, indent=2)
    say("prepare", "frames", sorted(os.listdir(frames)))


def run_exe(args, timeout=300, env=None):
    try:
        result = subprocess.run(args, capture_output=True, timeout=timeout, env=env or ENV)
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


def color_frame(state):
    image = Image.new("RGB", (64, 36), (32, 32, 32))
    image.putpixel((10, 10), (224, 225, 227) if state == "off" else (255, 229, 26))
    return image


def fixture_config(config_dir, state_root):
    os.makedirs(config_dir, exist_ok=False)
    frames = [color_frame("off"), color_frame("on")]
    config = {
        "schema_version": "actingcommand.actingd.config.v1", "state_root": state_root, "bind_host": "127.0.0.1", "bind_port": 0,
        "secret_fingerprint_salt": "oneoff-339-l3c-fixture-salt-value",
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


def watched_counts(events):
    types = collections.Counter(event.get("event_type") for event in events)
    return {name: types.get(name, 0) for name in WATCHED}


def ledger_report(label, ledger, root):
    events, failure = ledger_events(ledger, root)
    if events is None:
        check(label + ".ledger_readable", False, failure)
        return []
    types = collections.Counter(event.get("event_type") for event in events)
    say(label, "ledger events", len(events), "types", json.dumps(dict(sorted(types.items()))))
    return events


def baseline(new_runtime, new_ledger, work):
    say("P0", "baseline: a fixture actingd started and stopped without any command")
    daemon = Daemon(new_runtime, os.path.join(work, "p0-baseline"), "P0")
    if daemon.start():
        daemon.stop()
        BASELINE["watched"] = watched_counts(ledger_report("P0", new_ledger, daemon.runtime_root))
        say("P0", "watched event counts", json.dumps(BASELINE["watched"]))


def session(new_runtime, new_lab, new_ledger, work, sizes, config_env, label, clear_first):
    w, h = sizes["auto_off.png"]
    bw, bh = sizes["battle_cost_label.png"]
    frames = os.path.join(work, "frames")
    daemon = Daemon(new_runtime, os.path.join(work, label.lower()), label)
    if not daemon.start():
        return
    env = daemon.env(config_env)
    base = ["--instance", FIXTURE_ALIAS]
    lab(new_lab, base + ["record", "start", "--task-id", "optional_restart", "--locale", "en-US"], f"{label} record start", env=env)
    lab(new_lab, base + ["record", "mark", "--frame", os.path.join(frames, "title.png"), "--template", f"ui/title=1180,664,{w},{h}",
                         "--page", "title", "--click-from", "ui/title"], f"{label} record mark title frame and click", env=env)
    code, value, _ = lab(new_lab, base + ["record", "mark", "--frame", os.path.join(frames, "popup.png"), "--template", f"notice/close=600,480,{bw},{bh}",
                                          "--color", "notice/panel=320,180,8,8", "--page", "notice", "--optional"],
                         f"{label} record mark pop-up frame, marks, --optional, no effect", env=env)
    state = data_of(value).get("step_state") or {}
    check(f"{label}.open_optional_step_2", code == 0 and data_of(value).get("step") == 2 and state.get("optional") == {"settle_ms": 2000}
          and state.get("effect") == "none" and state.get("closed") is False, json.dumps(state))
    if clear_first:
        code, value, _ = lab(new_lab, base + ["record", "mark", "--step", "2", "--not-optional"], f"{label} record mark --step 2 --not-optional", env=env)
        check(f"{label}.cleared", code == 0 and (data_of(value).get("step_state") or {}).get("optional") is None, "")
    path = recording_file(daemon.session_root)
    before = file_sha(path)
    say(label, "recording.json sha256 before", before)
    results = []
    for args in (["session", "app", "restart", "--record"], ["session", "instance", "app", "restart", "--record"]):
        code, value, error = lab(new_lab, base + args, f"{label} {' '.join(args)}", env=env)
        results.append((code, error or {}))
    after = file_sha(path)
    say(label, "recording.json sha256 after", after)
    check(f"{label}.recording_unchanged", before == after, after)
    code, value, _ = lab(new_lab, base + ["record", "status"], f"{label} record status", env=env, limit=3000)
    steps = (data_of(value).get("lab") or {}).get("steps") or []
    check(f"{label}.no_application_recorded", code == 0 and all(step.get("application") is None for step in steps), json.dumps([step.get("optional") for step in steps]))
    daemon.stop()
    events = ledger_report(label, new_ledger, daemon.runtime_root)
    watched = watched_counts(events)
    say(label, "watched event counts", json.dumps(watched), "baseline", json.dumps(BASELINE.get("watched")))
    check(f"{label}.no_command_received_no_application_events", watched["command.received"] == 0
          and not any(str(event.get("event_type", "")).startswith("application.") for event in events)
          and watched == BASELINE.get("watched"), json.dumps(watched))
    return results


def run(work, new_runtime, new_tools, repo, product_sha):
    with open(os.path.join(work, "cases.json"), encoding="utf-8") as handle:
        sizes = json.load(handle)["sizes"]
    new_lab = os.path.join(new_tools, "actinglab.exe")
    new_ledger = os.path.join(new_tools, "actingledger.exe")
    config_path = os.path.join(work, "lab-config.json")
    with open(config_path, "w", encoding="utf-8") as handle:
        json.dump({"instances": {FIXTURE_ALIAS: {}}}, handle)
    config_env = {"ACTINGLAB_CONFIG_PATH": config_path}
    ENV.update(config_env)
    try:
        baseline(new_runtime, new_ledger, work)
        say("P1", "open step 2 with marks and --optional, no effect: session app restart --record and session instance app restart --record")
        results = session(new_runtime, new_lab, new_ledger, work, sizes, config_env, "P1", clear_first=False) or []
        for (code, error), command in zip(results, ("session app", "session instance app")):
            check(f"P1.{command}.record_optional_application", code == 3 and error.get("code") == "record_optional_application"
                  and (error.get("details") or {}).get("step") == 2 and (error.get("details") or {}).get("application") == "restart",
                  f"exit {code} code {error.get('code')} details {json.dumps(error.get('details'))}")
        say("P2", "control: the same step after --not-optional; the request reaches the Runtime, which denies it on the fixture")
        results = session(new_runtime, new_lab, new_ledger, work, sizes, config_env, "P2", clear_first=True) or []
        for (code, error), command in zip(results, ("session app", "session instance app")):
            check(f"P2.{command}.runtime_denied", code == 4 and error.get("code") == "device_error"
                  and "fixture_execution_scope_forbidden" in (error.get("message") or ""), f"exit {code} code {error.get('code')}")
        say("P3", "diff from the previous head", PREVIOUS_HEAD, "to", product_sha)
        diff = subprocess.run(["git", "-C", repo, "diff", PREVIOUS_HEAD, product_sha], capture_output=True, check=True).stdout.decode("utf-8")
        for line in diff.splitlines():
            say("P3", "diff", line)
    except Exception as error:
        say("run", "exception", repr(error))
        FAILURES.append("exception")
    say("RESULT", "failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


if __name__ == "__main__":
    if sys.argv[1] == "prepare":
        prepare(sys.argv[2], sys.argv[3])
        say("RESULT", "prepare failures", len(FAILURES), json.dumps(FAILURES))
        sys.exit(1 if FAILURES else 0)
    sys.exit(run(*sys.argv[2:7]))
