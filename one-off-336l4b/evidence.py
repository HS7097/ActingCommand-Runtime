# One-off (to be reverted), Workflow #336 L4 fix round evidence: record stop on a recording that
# already generated its package refuses a generation option with another value
# (record_stop_option_conflict) before anything is written, and accepts an equal value. Every
# printed line starts with "L4B|". Usage:
#   evidence.py prepare <work> <umbrella standard package zip>
#   evidence.py run <work> <new tools>
import hashlib
import io
import json
import os
import subprocess
import sys
import zipfile

from PIL import Image

SCHEMA_DIR = "actingcommand.package.content-directory.v1"
FAILURES = []
INSTANCE = "emu-a"


def say(*parts):
    print("L4B|" + "|".join(str(part) for part in parts), flush=True)


def check(label, condition, detail=""):
    say("CHECK", label, "PASS" if condition else "FAIL", detail)
    if not condition:
        FAILURES.append(label)


def short(text, limit=1500):
    text = text.strip().replace("\r", "").replace("\n", " ")
    return text if len(text) <= limit else text[:limit] + "..."


def digest(files):
    hasher = hashlib.sha256()
    hasher.update((SCHEMA_DIR + "\n").encode())
    for path in sorted(files, key=lambda item: item.encode("utf-8")):
        hasher.update(f"{hashlib.sha256(files[path]).hexdigest()}  {path}\n".encode("utf-8"))
    return hasher.hexdigest()


def file_sha(path):
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def tree(root):
    found = {}
    for folder, _dirs, names in os.walk(root):
        for name in names:
            path = os.path.join(folder, name)
            found[os.path.relpath(path, root)] = file_sha(path)
    return found


def prepare(work, bundle_zip):
    os.makedirs(work, exist_ok=False)
    with zipfile.ZipFile(bundle_zip) as bundle:
        names = bundle.namelist()
        index_name = next(name for name in names if name.endswith("bundle.json"))
        index = json.loads(bundle.read(index_name))
        pack = next(pack for pack in index["packs"] if pack["package_id"] == "bluearchive.jp.battle_auto_enable")
        prefix = index_name[: -len("bundle.json")] + pack["path"] + "/"
        files = {name[len(prefix):]: bundle.read(name) for name in names if name.startswith(prefix) and not name.endswith("/")}
    check("prepare.source_digest", digest(files) == pack["digest"], digest(files))
    assets = "resources/operations/battle_auto_enable/assets/"
    images = {name: Image.open(io.BytesIO(files[assets + name])).convert("RGB") for name in ("auto_off.png", "auto_on.png", "battle_cost_label.png")}
    frames = os.path.join(work, "frames")
    os.makedirs(frames)
    for state in ("off", "on"):
        image = Image.new("RGB", (1280, 720), (32, 32, 32))
        for x in range(0, 1280, 8):
            for y in range(0, 640, 8):
                image.putpixel((x, y), (32 + (x // 8) % 16, 32 + (y // 8) % 16, 40))
        image.paste(images["auto_off.png" if state == "off" else "auto_on.png"], (1180, 664))
        image.paste(images["battle_cost_label.png"], (778, 649))
        image.putpixel((1173, 680), (224, 225, 227) if state == "off" else (255, 229, 26))
        image.save(os.path.join(frames, state + ".png"), format="PNG")
    say("prepare", "frames", sorted(os.listdir(frames)))
    return 1 if FAILURES else 0


def lab(exe, args, label):
    result = subprocess.run([exe, "--json", *args], capture_output=True, timeout=600)
    out = result.stdout.decode("utf-8", "replace")
    try:
        value = json.loads(out)
    except ValueError:
        value = None
    say(label, "exit", result.returncode, "envelope", short(out) if out else short(result.stderr.decode("utf-8", "replace"), 800))
    return result.returncode, value, (value or {}).get("error")


def run(work, new_tools):
    exe = os.path.join(new_tools, "actinglab.exe")
    frames = os.path.join(work, "frames")
    state = os.path.join(work, "sessions", "option_case")
    out = os.path.join(work, "out", "packages")
    os.makedirs(out)
    base = ["--instance", INSTANCE]

    def mark(label, args):
        code, value, _ = lab(exe, base + ["record", "mark", "--state-dir", state, *args], label)
        check(label, code == 0, f"exit {code}")

    def stop(label, args):
        return lab(exe, base + ["record", "stop", "--state-dir", state, *args], label)

    def recording_sha():
        for folder, _dirs, names in os.walk(os.path.join(state, "record-artifacts")):
            if "recording.json" in names:
                return file_sha(os.path.join(folder, "recording.json"))
        return None

    def statuses():
        code, value, _ = lab(exe, base + ["record", "status", "--state-dir", state], "record status")
        data = (value or {}).get("data") or {}
        return (data.get("record") or {}).get("status"), (data.get("lab") or {}).get("status")

    code, value, _ = lab(exe, base + ["record", "start", "--task-id", "option_case", "--game", "bluearchive", "--server", "jp",
                                      "--locale", "ja-JP", "--state-dir", state], "record start")
    check("start", code == 0, f"exit {code}")
    mark("mark off + click", ["--frame", os.path.join(frames, "off.png"), "--page", "off", "--color", "state/off=1173,680,1,1",
                              "--click", "600,300,80,40"])
    mark("mark on", ["--frame", os.path.join(frames, "on.png"), "--page", "on", "--color", "state/on=1173,680,1,1"])
    first_dir = os.path.join(out, "first")
    code, value, _ = stop("record stop --lab-dir first (generate once)", ["--lab-dir", first_dir])
    lab_data = ((value or {}).get("data") or {}).get("lab") or {}
    timeouts = lab_data.get("timeouts") or {}
    check("generated", code == 0 and lab_data.get("status") == "generated", json.dumps({"digest": lab_data.get("digest"),
                                                                                         "package_id": lab_data.get("package_id"),
                                                                                         "timeouts": timeouts}))
    recorded_id = lab_data.get("package_id")
    recorded_timeout = timeouts.get("timeout_ms")
    for folder, _dirs, names in os.walk(os.path.join(state, "record-artifacts")):
        if "recording.json" in names:
            with open(os.path.join(folder, "recording.json"), encoding="utf-8") as handle:
                say("recording.json artifact", json.dumps(json.load(handle).get("artifact")))

    def refused(label, args, field, given):
        before_sha, before_out = recording_sha(), tree(out)
        target = os.path.join(out, "second")
        code, value, error = stop(label, [*args, "--lab-dir", target])
        details = (error or {}).get("details") or {}
        check(label, code == 3 and (error or {}).get("code") == "record_stop_option_conflict"
              and details.get("field") == field and details.get("given") == given
              and recording_sha() == before_sha and tree(out) == before_out and not os.path.exists(target)
              and statuses() == ("stopped", "stopped"), json.dumps(details))

    def accepted(label, args):
        before_sha = recording_sha()
        code, value, _ = stop(label, args)
        lab_data = ((value or {}).get("data") or {}).get("lab") or {}
        check(label, code == 0 and lab_data.get("status") == "already_generated" and recording_sha() == before_sha,
              json.dumps({"lab_dir_status": lab_data.get("lab_dir_status")}))
        return lab_data

    refused("record stop --package-id <other> (conflict)", ["--package-id", "bluearchive.jp.other_case"], "package_id",
            "bluearchive.jp.other_case")
    refused("record stop --dry-run --package-id <other> (conflict)", ["--dry-run", "--package-id", "bluearchive.jp.other_case"],
            "package_id", "bluearchive.jp.other_case")
    refused("record stop --timeout-ms <other> (conflict)", ["--timeout-ms", str(recorded_timeout + 1)], "timeout_ms", recorded_timeout + 1)
    refused("record stop --game <other> (conflict)", ["--game", "othergame"], "game", "othergame")
    refused("record stop --application-arrival-timeout-ms 1000 (conflict)", ["--application-arrival-timeout-ms", "1000"],
            "application_arrival_timeout_ms", 1000)
    code, value, error = stop("record stop --requires x (record_requires_conflict unchanged)", ["--requires", "bluearchive.jp.x"])
    check("requires_conflict_unchanged", code == 3 and (error or {}).get("code") == "record_requires_conflict", "")
    second = accepted("record stop --package-id <recorded> --lab-dir second (equal, accepted)",
                      ["--package-id", recorded_id, "--lab-dir", os.path.join(out, "second")])
    check("equal_value_written", second.get("lab_dir_status") == "written"
          and os.path.isfile(os.path.join(out, "second", f"{lab_data.get('digest')}.{lab_data.get('container')}")), "")
    accepted("record stop with every option equal", ["--package-id", recorded_id, "--game", "bluearchive", "--server", "jp",
                                                    "--locale", "ja-JP", "--timeout-ms", str(recorded_timeout),
                                                    "--arrival-timeout-ms", "15000", "--application-arrival-timeout-ms", "90000"])
    say("RESULT", "failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


if __name__ == "__main__":
    if sys.argv[1] == "prepare":
        sys.exit(prepare(sys.argv[2], sys.argv[3]))
    sys.exit(run(sys.argv[2], sys.argv[3]))
