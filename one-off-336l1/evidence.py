# One-off (to be reverted), Workflow #336 L1 evidence (frozen model section 8 L1, items 1-5).
# Every printed line starts with "L1|". Usage:
#   evidence.py prepare <work> <umbrella BA standard package zip> <catalog-a dir>
#   evidence.py run <work> <new runtime dir> <new tools dir> <old runtime dir> <old tools dir> <catalog-a dir>
import hashlib
import io
import json
import os
import shutil
import struct
import subprocess
import sys
import time
import zipfile
import zlib

from PIL import Image

SCHEMA_DIR = "actingcommand.package.content-directory.v1"
SCHEMA_JSON = "actingcommand.package.content-json.v1"
BA_PACKAGE_ID = "bluearchive.jp.battle_auto_enable"
BA_DIGEST = "352dc9a0810aac3b6aafd0dbc74862362e5ba478337d756f2bd50523618b00a7"
BASH = r"C:\Program Files\Git\bin\bash.exe"
ALIAS = "node.a"
INSTANCE_ID = "instance_00000000000000000000000000000336"
FAILURES = []


def say(*parts):
    print("L1|" + "|".join(str(part) for part in parts), flush=True)


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


def zip_bytes(entries, compression, date_time=(1980, 1, 1, 0, 0, 0), directories=False):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        written = set()
        for path, data in entries:
            if directories:
                parts = path.split("/")[:-1]
                for depth in range(1, len(parts) + 1):
                    folder = "/".join(parts[:depth]) + "/"
                    if folder not in written:
                        written.add(folder)
                        archive.writestr(zipfile.ZipInfo(folder, date_time), b"")
            info = zipfile.ZipInfo(path, date_time)
            info.compress_type = compression
            if compression == zipfile.ZIP_DEFLATED:
                archive.writestr(info, data, compresslevel=9)
            else:
                archive.writestr(info, data)
    return buffer.getvalue()


def raw_zip(entries):
    """A stored ZIP whose names are the given raw bytes, with no flag bit set (no UTF-8 flag)."""
    out = io.BytesIO()
    central = []
    for name, data in entries:
        offset = out.tell()
        crc = zlib.crc32(data) & 0xFFFFFFFF
        out.write(struct.pack("<IHHHHHIIIHH", 0x04034B50, 20, 0, 0, 0, 0x21, crc, len(data), len(data), len(name), 0))
        out.write(name)
        out.write(data)
        central.append(
            struct.pack("<IHHHHHHIIIHHHHHII", 0x02014B50, 20, 20, 0, 0, 0, 0x21, crc, len(data), len(data), len(name), 0, 0, 0, 0, 0, offset)
            + name
        )
    start = out.tell()
    directory = b"".join(central)
    out.write(directory)
    out.write(struct.pack("<IHHHHIIH", 0x06054B50, 0, 0, len(entries), len(entries), len(directory), start, 0))
    return out.getvalue()


def json_container(files, order, ensure_ascii, indent=None, escape_slash=False, schema_first=True):
    ordered = {path: files[path].decode("utf-8") for path in order}
    document = {"schema_version": SCHEMA_JSON, "files": ordered} if schema_first else {"files": ordered, "schema_version": SCHEMA_JSON}
    text = json.dumps(document, ensure_ascii=ensure_ascii, indent=indent)
    if escape_slash:
        # Every '/' of a JSON text is inside a string; '\/' is its standard escape.
        text = text.replace("/", "\\/")
    return text.encode("utf-8")


def pretty(value):
    return (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode("utf-8")


def color_pack():
    """The synthetic color-only pack: one task, two pages told apart by one pixel, 64x36."""
    control = {
        "schema_version": "Lab-1y.control.v2",
        "package_id": "fixture.toggle",
        "execution_mode": "navigable_route",
        "game": "fixture-game-a",
        "server": "fixture-server-a",
        "resolution": {"width": 64, "height": 36},
        "entry_task_id": "toggle",
        "timeout_ms": 30000,
        "max_steps": 1,
    }
    task = {
        "schema_version": "0.9",
        "task_id": "toggle",
        "game": "fixture-game-a",
        "server_scope": ["fixture-server-a"],
        "locale": "en-US",
        "coordinate_space": {"width": 64, "height": 36},
        "defaults": {"template_threshold": 0.97, "color_max_distance": 20, "match_metric": "ccoeff_normed"},
        "goal": "Toggle the fixture state once (caf\u00e9, \u00fc, \u2014, \u2713).",
        "timeout_ms": 30000,
        "max_steps": 1,
        "entry_page": "toggle_off",
        "target_page": "toggle_on",
        "color_probes": [
            {"id": "state/off", "region": {"mode": "rect", "rect": {"x": 10, "y": 10, "width": 1, "height": 1}}, "expected": [224, 225, 227]},
            {"id": "state/on", "region": {"mode": "rect", "rect": {"x": 10, "y": 10, "width": 1, "height": 1}}, "expected": [255, 229, 26]},
        ],
        "page_rules": {
            "toggle_off": {"required": ["state/off"], "forbidden": ["state/on"]},
            "toggle_on": {"required": ["state/on"], "forbidden": ["state/off"]},
        },
        "scheduling_outcome": {
            "mappings": [{"outcome_key": "toggle_done", "effect": "no_designated_effect", "terminal_pages": ["toggle_on"]}]
        },
        "operations": [{
            "id": "toggle_once",
            "purpose": "Press once from the off state, then require the on state.",
            "from": "toggle_off",
            "to": "toggle_on",
            "click": {"kind": "point", "x": 10, "y": 10},
            "guard": {
                "page_id": "toggle_off",
                "target_id": "state/off",
                "expected_rect": {"x": 10, "y": 10, "width": 1, "height": 1},
                "color_probe": "state/off",
            },
            "expect_after": {"page_id": "toggle_on", "timeout_ms": 15000, "interval_ms": 500},
            "retryable": False,
            "max_attempts": 1,
            "retry_interval_ms": 1,
            "post_delay_ms": 200,
        }],
    }
    resources = {"schema_version": "1.0", "resources": [], "resource_count": 0}
    return {
        "control.json": pretty(control),
        "resources/operations/resources.json": pretty(resources),
        "resources/operations/toggle/task.json": pretty(task),
    }


def color_frame(state):
    image = Image.new("RGB", (64, 36), (32, 32, 32))
    image.putpixel((10, 10), (224, 225, 227) if state == "off" else (255, 229, 26))
    return image


def ba_frame(ba_files, state):
    image = Image.new("RGB", (1280, 720), (32, 32, 32))
    assets = "resources/operations/battle_auto_enable/assets/"

    def paste(name, x, y):
        image.paste(Image.open(io.BytesIO(ba_files[assets + name])).convert("RGB"), (x, y))

    paste("auto_off.png" if state == "off" else "auto_on.png", 1180, 664)
    paste("battle_cost_label.png", 778, 649)
    image.putpixel((1173, 680), (224, 225, 227) if state == "off" else (255, 229, 26))
    return image


def save_png(image, path):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    image.save(path, format="PNG")


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


def write_config(config_dir, state_root, package_path, sha256, frames, catalog_dir):
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
        "secret_fingerprint_salt": "oneoff-336-l1-fixture-salt-value",
        "policy": {
            "facts": {
                "ledger_position": 0,
                "fact_snapshot_id": "snapshot:oneoff-336-l1",
                "facts": [], "outcomes": [], "tasks": [],
                "instances": [{
                    "instance_id": ALIAS, "server_id": "fixture-server-a", "game_id": "fixture-game-a",
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
                "package_digest": reference(sha256),
                "operation_id": "operation.observe",
                "yield_points": ["after_observation"],
                "scheduled_execution": {"mode": "fixture_simulation", "package_path": package_path},
            }],
        },
        "instances": [{
            "alias": ALIAS,
            "instance_id": INSTANCE_ID,
            "fixture_backend": {
                "frames": [{"width": 64, "height": 36, "rgb": list(frame.tobytes())} for frame in frames],
                "max_inputs": 2,
            },
        }],
    }
    path = os.path.join(config_dir, "actingd.json")
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(config, handle)
    return path


def prepare(work, bundle_zip, catalog_dir):
    os.makedirs(work, exist_ok=False)
    # The public umbrella v0.9.0 standard package of BA@193cfdd: the battle_auto_enable pack.
    with zipfile.ZipFile(bundle_zip) as bundle:
        names = bundle.namelist()
        index_name = next(name for name in names if name.endswith("bundle.json"))
        index = json.loads(bundle.read(index_name))
        packs = [pack for pack in index["packs"] if pack["package_id"] == BA_PACKAGE_ID]
        say("prepare", "bundle", os.path.basename(bundle_zip), "index", index_name, "packs", len(index["packs"]),
            "battle_auto_enable", json.dumps(packs))
        prefix = index_name[: -len("bundle.json")] + packs[0]["path"] + "/"
        ba = {name[len(prefix):]: bundle.read(name) for name in names if name.startswith(prefix) and not name.endswith("/")}
    ba_digest = digest(ba)
    say("prepare", "ba", "files", len(ba), "digest", ba_digest, "bundle", packs[0]["digest"], "expected", BA_DIGEST)
    check("prepare.ba_digest", ba_digest == BA_DIGEST == packs[0]["digest"], ba_digest)
    for path in sorted(ba):
        say("prepare", "ba file", path, len(ba[path]), hashlib.sha256(ba[path]).hexdigest())

    ba_sorted = sorted(ba)
    write_tree(os.path.join(work, "ba", "dir", BA_DIGEST), ba)
    write_bytes(os.path.join(work, "ba", "zip-a", BA_DIGEST + ".zip"),
                zip_bytes([(path, ba[path]) for path in ba_sorted], zipfile.ZIP_DEFLATED, directories=True))
    write_bytes(os.path.join(work, "ba", "zip-b", BA_DIGEST + ".zip"),
                zip_bytes([(path, ba[path]) for path in reversed(ba_sorted)], zipfile.ZIP_STORED, (2001, 2, 3, 4, 5, 6)))

    color = color_pack()
    color_digest = digest(color)
    color_sorted = sorted(color)
    say("prepare", "color", "files", len(color), "digest", color_digest)
    write_tree(os.path.join(work, "color", "dir", color_digest), color)
    json_a = json_container(color, color_sorted, ensure_ascii=False)
    json_b = json_container(color, list(reversed(color_sorted)), ensure_ascii=True, indent=2, escape_slash=True, schema_first=False)
    write_bytes(os.path.join(work, "color", "json-a", color_digest + ".json"), json_a)
    write_bytes(os.path.join(work, "color", "json-b", color_digest + ".json"), json_b)
    write_bytes(os.path.join(work, "color", "zip", color_digest + ".zip"),
                zip_bytes([(path, color[path]) for path in color_sorted], zipfile.ZIP_DEFLATED))
    write_bytes(os.path.join(work, "color", "txt", color_digest + ".txt"), json_a)
    say("prepare", "color json-a head", json_a[:160].decode("utf-8"))
    say("prepare", "color json-b head", json_b[:160].decode("utf-8"))

    # A path that is not ASCII: a directory, a ZIP with the UTF-8 flag, a ZIP without it.
    utf8 = dict(color)
    utf8["notes/caf\u00e9-\u00fc.txt"] = "non-ASCII path\n".encode("utf-8")
    utf8_digest = digest(utf8)
    cp437 = {path.encode("utf-8").decode("cp437"): data for path, data in utf8.items()}
    say("prepare", "utf8", "digest", utf8_digest, "cp437 reading digest", digest(cp437))
    write_tree(os.path.join(work, "utf8", "dir", utf8_digest), utf8)
    write_bytes(os.path.join(work, "utf8", "zip-flag", utf8_digest + ".zip"),
                zip_bytes([(path, utf8[path]) for path in sorted(utf8)], zipfile.ZIP_DEFLATED))
    write_bytes(os.path.join(work, "utf8", "zip-noflag", utf8_digest + ".zip"),
                raw_zip([(path.encode("utf-8"), utf8[path]) for path in sorted(utf8)]))

    # Refusals.
    zip_a = open(os.path.join(work, "ba", "zip-a", BA_DIGEST + ".zip"), "rb").read()
    write_bytes(os.path.join(work, "refuse", "name", "0" * 64 + ".zip"), zip_a)
    git_entries = [(path, ba[path]) for path in ba_sorted] + [(".git/config", b"[core]\n")]
    write_bytes(os.path.join(work, "refuse", "git", "git-entry.zip"), zip_bytes(git_entries, zipfile.ZIP_DEFLATED))
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        for path in ba_sorted:
            archive.writestr(path, ba[path])
        link = zipfile.ZipInfo("resources/link.png", (1980, 1, 1, 0, 0, 0))
        link.create_system = 3
        link.external_attr = 0o120777 << 16
        archive.writestr(link, "operations/battle_auto_enable/assets/auto_off.png")
    write_bytes(os.path.join(work, "refuse", "symlink", "symlink-entry.zip"), buffer.getvalue())
    write_bytes(os.path.join(work, "refuse", "nonutf8", "nonutf8-entry.zip"),
                raw_zip([(path.encode("utf-8"), ba[path]) for path in ba_sorted] + [(b"notes/bad\xff.txt", b"x\n")]))
    write_bytes(os.path.join(work, "refuse", "backslash", "backslash-entry.zip"),
                raw_zip([(path.encode("utf-8"), ba[path]) for path in ba_sorted] + [(b"notes\\win.txt", b"x\n")]))
    write_bytes(os.path.join(work, "refuse", "truncated", "truncated.zip"), zip_a[: len(zip_a) // 2])
    pairs = [json.dumps(path) + ":" + json.dumps(color[path].decode("utf-8")) for path in color_sorted]
    duplicate = '{"schema_version":"%s","files":{%s}}' % (SCHEMA_JSON, ",".join(pairs + pairs[:1]))
    write_bytes(os.path.join(work, "refuse", "duplicate", "duplicate-path.json"), duplicate.encode("utf-8"))
    not_string = '{"schema_version":"%s","files":{"control.json":{"inline":true}}}' % SCHEMA_JSON
    write_bytes(os.path.join(work, "refuse", "notstring", "not-string.json"), not_string.encode("utf-8"))
    write_bytes(os.path.join(work, "refuse", "bom", "bom.json"), b"\xef\xbb\xbf" + json_a)

    # Synthetic frames: the real BA assets at their declared coordinates; the color pack's pixel.
    for state in ("off", "on"):
        save_png(ba_frame(ba, state), os.path.join(work, "frames", f"ba-{state}.png"))
        save_png(color_frame(state), os.path.join(work, "frames", f"color-{state}.png"))

    cases = {
        "ba": BA_DIGEST, "color": color_digest, "utf8": utf8_digest,
        "load": [
            {"name": "ba-dir", "locator": os.path.join(work, "ba", "dir", BA_DIGEST), "sha256": BA_DIGEST, "expect": "ok"},
            {"name": "ba-zip-a", "locator": os.path.join(work, "ba", "zip-a", BA_DIGEST + ".zip"), "sha256": BA_DIGEST, "expect": "ok"},
            {"name": "ba-zip-b", "locator": os.path.join(work, "ba", "zip-b", BA_DIGEST + ".zip"), "sha256": BA_DIGEST, "expect": "ok"},
            {"name": "color-json-a", "locator": os.path.join(work, "color", "json-a", color_digest + ".json"), "sha256": color_digest, "expect": "ok"},
            {"name": "color-json-b", "locator": os.path.join(work, "color", "json-b", color_digest + ".json"), "sha256": color_digest, "expect": "ok"},
            {"name": "utf8-zip-noflag", "locator": os.path.join(work, "utf8", "zip-noflag", utf8_digest + ".zip"), "sha256": utf8_digest, "expect": "ok"},
            {"name": "name-mismatch", "locator": os.path.join(work, "refuse", "name", "0" * 64 + ".zip"), "sha256": BA_DIGEST, "expect": "content_directory_name_mismatch"},
            {"name": "txt", "locator": os.path.join(work, "color", "txt", color_digest + ".txt"), "sha256": color_digest, "expect": "content_container_unsupported"},
            {"name": "git-entry", "locator": os.path.join(work, "refuse", "git", "git-entry.zip"), "sha256": BA_DIGEST, "expect": "content_directory_path_invalid"},
            {"name": "symlink-entry", "locator": os.path.join(work, "refuse", "symlink", "symlink-entry.zip"), "sha256": BA_DIGEST, "expect": "content_zip_entry_invalid"},
            {"name": "nonutf8-entry", "locator": os.path.join(work, "refuse", "nonutf8", "nonutf8-entry.zip"), "sha256": BA_DIGEST, "expect": "content_zip_entry_invalid"},
            {"name": "backslash-entry", "locator": os.path.join(work, "refuse", "backslash", "backslash-entry.zip"), "sha256": BA_DIGEST, "expect": "content_zip_entry_invalid"},
            {"name": "truncated-zip", "locator": os.path.join(work, "refuse", "truncated", "truncated.zip"), "sha256": BA_DIGEST, "expect": "content_zip_invalid"},
            {"name": "duplicate-path", "locator": os.path.join(work, "refuse", "duplicate", "duplicate-path.json"), "sha256": color_digest, "expect": "content_json_duplicate_path"},
            {"name": "not-string", "locator": os.path.join(work, "refuse", "notstring", "not-string.json"), "sha256": color_digest, "expect": "content_json_file_not_string"},
            {"name": "bom", "locator": os.path.join(work, "refuse", "bom", "bom.json"), "sha256": color_digest, "expect": "content_json_invalid"},
        ],
        "entries": [
            {"name": "ba-zip-a", "file": os.path.join(work, "ba", "zip-a", BA_DIGEST + ".zip"), "kind": "zip", "sha256": BA_DIGEST, "expect": "ok"},
            {"name": "color-json-b", "file": os.path.join(work, "color", "json-b", color_digest + ".json"), "kind": "json", "sha256": color_digest, "expect": "ok"},
            {"name": "color-json-b-other-reference", "file": os.path.join(work, "color", "json-b", color_digest + ".json"), "kind": "json", "sha256": BA_DIGEST, "expect": "content_directory_digest_mismatch"},
        ],
    }
    with open(os.path.join(work, "cases.json"), "w", encoding="utf-8") as handle:
        json.dump(cases, handle, indent=2)
    say("prepare", "done", work)


def run_exe(args, timeout=300, cwd=None):
    try:
        result = subprocess.run(args, capture_output=True, timeout=timeout, cwd=cwd)
    except subprocess.TimeoutExpired:
        return None, "", "timeout"
    return result.returncode, result.stdout.decode("utf-8", "replace"), result.stderr.decode("utf-8", "replace")


def short(text, limit=700):
    text = text.strip().replace("\r", "").replace("\n", " ")
    return text if len(text) <= limit else text[:limit] + "..."


def lab_json(exe, args):
    code, out, err = run_exe([exe, "--json", *args])
    try:
        value = json.loads(out)
    except ValueError:
        value = None
    return code, value, out, err


def tree_hashes(root):
    hashes = {}
    for folder, _dirs, names in os.walk(root):
        for name in names:
            path = os.path.join(folder, name)
            with open(path, "rb") as handle:
                hashes[os.path.relpath(path, root)] = hashlib.sha256(handle.read()).hexdigest()
    return hashes


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


def summarize_runs(label, events):
    from collections import Counter
    counts = Counter(event["event_type"] for event in events)
    say(label, "events", len(events), "types", json.dumps(dict(sorted(counts.items()))))
    admitted = []
    for event in events:
        if event["event_type"] == "task.requested":
            fact = (((event.get("payload") or {}).get("payload") or {}).get("data") or {}).get("fact") or {}
            if fact.get("kind") == "package_admitted":
                admitted.append(fact.get("package_sha256"))
                say(label, "package_admitted", event["sequence"], json.dumps(fact, ensure_ascii=False))
        if event["event_type"] in ("task.completed", "task.failed", "policy.execution_recorded", "policy.dispatch_completed"):
            say(label, event["event_type"], event["sequence"], short(json.dumps(event["payload"], ensure_ascii=False), 900))
    return counts, admitted


def ledger_reads(label, ledger, root):
    results = {}
    for name, args in (("open", ["open"]), ("events", ["events"]), ("export --task-evidence", ["export", "--task-evidence"])):
        code, out, err = run_exe([ledger, "--state-root", root, *args])
        corrupt = "corrupt_ledger_record" in out or "corrupt_ledger_record" in err
        say(label, name, "exit", code, "stdout bytes", len(out), "sha256", hashlib.sha256(out.encode("utf-8")).hexdigest(),
            "corrupt_ledger_record", corrupt, "stderr", short(err, 300))
        say(label, name, "stdout head", short(out, 500))
        results[name] = (code, corrupt)
    return results


def daemon_run(label, runtime_dir, config, state_root, logs, settle_s=25):
    actingd = os.path.join(runtime_dir, "actingcommand-actingd.exe")
    actingctl = os.path.join(runtime_dir, "actingctl.exe")
    os.makedirs(logs, exist_ok=True)
    with open(os.path.join(logs, label + ".out"), "wb") as out, open(os.path.join(logs, label + ".err"), "wb") as err:
        process = subprocess.Popen([actingd, "--config", config], stdout=out, stderr=err, cwd=os.path.dirname(config))
        started = time.time()
        ready = False
        while time.time() - started < 60 and process.poll() is None:
            code, status_out, _ = run_exe([actingctl, "status", "--state-root", state_root], timeout=30)
            if code == 0:
                ready = True
                break
            time.sleep(1)
        say(label, "daemon ready", ready, "after_s", round(time.time() - started, 1), "alive", process.poll() is None)
        if ready:
            time.sleep(settle_s)
        code, shutdown_out, shutdown_err = run_exe([actingctl, "request-shutdown", "--state-root", state_root, "--wait", "60"], timeout=120)
        say(label, "request-shutdown exit", code, short(shutdown_out, 400), short(shutdown_err, 300))
        try:
            exit_code = process.wait(timeout=120)
        except subprocess.TimeoutExpired:
            process.kill()
            exit_code = "killed"
    for stream in ("out", "err"):
        with open(os.path.join(logs, f"{label}.{stream}"), "rb") as handle:
            say(label, "actingd " + stream, short(handle.read().decode("utf-8", "replace"), 600))
    say(label, "actingd exit", exit_code)
    return ready and exit_code == 0


def run(work, new_runtime, new_tools, old_runtime, old_tools, catalog_dir):
    with open(os.path.join(work, "cases.json"), encoding="utf-8") as handle:
        cases = json.load(handle)
    ba, color, utf8 = cases["ba"], cases["color"], cases["utf8"]
    new_lab = os.path.join(new_tools, "actinglab.exe")
    old_lab = os.path.join(old_tools, "actinglab.exe")
    new_ledger = os.path.join(new_tools, "actingledger.exe")
    old_ledger = os.path.join(old_tools, "actingledger.exe")
    new_actingd = os.path.join(new_runtime, "actingcommand-actingd.exe")
    old_actingd = os.path.join(old_runtime, "actingcommand-actingd.exe")

    # 1. Same digest across containers.
    groups = {
        "ba": (ba, [os.path.join(work, "ba", "dir", ba), os.path.join(work, "ba", "zip-a", ba + ".zip"), os.path.join(work, "ba", "zip-b", ba + ".zip")]),
        "color": (color, [os.path.join(work, "color", "dir", color), os.path.join(work, "color", "json-a", color + ".json"), os.path.join(work, "color", "json-b", color + ".json")]),
        "utf8": (utf8, [os.path.join(work, "utf8", "dir", utf8), os.path.join(work, "utf8", "zip-flag", utf8 + ".zip"), os.path.join(work, "utf8", "zip-noflag", utf8 + ".zip")]),
    }
    for group, (expected, paths) in groups.items():
        for path in paths:
            code, value, out, err = lab_json(new_lab, ["package", "digest", "--package", path])
            data = (value or {}).get("data") or {}
            got = (data.get("reference") or {}).get("sha256")
            say("E1", group, os.path.relpath(path, work), "exit", code, "reference", json.dumps(data.get("reference")),
                "package_id", data.get("package_id"), "file_count", data.get("file_count"), "byte_count", data.get("byte_count"),
                "error", short(json.dumps((value or {}).get("error")) if value else err, 400))
            check(f"E1.{group}.{os.path.basename(os.path.dirname(path))}", code == 0 and got == expected, got)
    posix = os.path.join(work, "ba", "dir", ba).replace("\\", "/")
    formula = ("cd '%s' && { printf 'actingcommand.package.content-directory.v1\\n'; find . -type f -printf '%%P\\0' | "
               "LC_ALL=C sort -z | xargs -0 sha256sum -b | sed 's/ \\*/  /'; } | sha256sum -b | cut -c1-64") % posix
    code, out, err = run_exe([BASH, "-c", formula])
    say("E1", "coreutils formula", "ba dir", "exit", code, "digest", out.strip(), short(err, 200))
    check("E1.coreutils", out.strip() == ba, out.strip())

    # 2. observe passes for each container (synthetic frames).
    frames = os.path.join(work, "frames")
    for name, path, sha256, frame, page in (
        ("ba-dir", os.path.join(work, "ba", "dir", ba), ba, "ba-off.png", "battle_auto_off"),
        ("ba-zip-a", os.path.join(work, "ba", "zip-a", ba + ".zip"), ba, "ba-off.png", "battle_auto_off"),
        ("ba-zip-b", os.path.join(work, "ba", "zip-b", ba + ".zip"), ba, "ba-on.png", "battle_auto_on"),
        ("color-dir", os.path.join(work, "color", "dir", color), color, "color-off.png", "toggle_off"),
        ("color-json-a", os.path.join(work, "color", "json-a", color + ".json"), color, "color-off.png", "toggle_off"),
        ("color-json-b", os.path.join(work, "color", "json-b", color + ".json"), color, "color-on.png", "toggle_on"),
    ):
        code, value, out, err = lab_json(new_lab, ["observe", "--scene", os.path.join(frames, frame), "--package", path,
                                                   "--package-ref", json.dumps(reference(sha256))])
        data = (value or {}).get("data") or {}
        say("E2", name, "exit", code, "state", data.get("state"), "page", data.get("page"), "matched", data.get("matched"),
            "frame", frame, "error", short(json.dumps((value or {}).get("error")) if value else err, 400))
        check(f"E2.{name}", code == 0 and data.get("matched") is True and str(data.get("page", "")).endswith(page), data.get("page"))

    # 3. Refusals keep their own codes (the CLI message; the kernel codes are in the KERNEL lines).
    for case in cases["load"]:
        if case["expect"] == "ok":
            continue
        frame = "ba-off.png" if case["sha256"] == ba else "color-off.png"
        code, value, out, err = lab_json(new_lab, ["observe", "--scene", os.path.join(frames, frame), "--package", case["locator"],
                                                   "--package-ref", json.dumps(reference(case["sha256"]))])
        error = (value or {}).get("error") or {}
        message = json.dumps(error, ensure_ascii=False) if error else err
        say("E3", case["name"], "exit", code, "expect", case["expect"], "error", short(message, 500))
        check(f"E3.{case['name']}", code != 0 and case["expect"] in message and "contained_task_admission_failed" not in message, case["expect"])

    # 4. The v0.9.0 build refuses the new containers; check-config before and after.
    for name, path, sha256, frame in (
        ("ba-zip-a", os.path.join(work, "ba", "zip-a", ba + ".zip"), ba, "ba-off.png"),
        ("color-json-a", os.path.join(work, "color", "json-a", color + ".json"), color, "color-off.png"),
    ):
        for command in (["observe", "--scene", os.path.join(frames, frame), "--package", path, "--package-ref", json.dumps(reference(sha256))],
                        ["package", "digest", "--package", path]):
            code, value, out, err = lab_json(old_lab, command)
            error = (value or {}).get("error") or {}
            message = json.dumps(error, ensure_ascii=False) if error else err
            say("E4", "v0.9.0 actinglab", command[0], name, "exit", code, "error", short(message, 400))
            check(f"E4.old_lab.{command[0]}.{name}", code != 0 and "content_directory_not_directory" in message, "content_directory_not_directory")
    off = color_frame("off")
    on = color_frame("on")
    check_root = os.path.join(work, "e4", "unused-state-root")
    configs = {}
    for name, package_path in (
        ("dir", os.path.join(work, "color", "dir", color)),
        ("zip", os.path.join(work, "color", "zip", color + ".zip")),
        ("json", os.path.join(work, "color", "json-a", color + ".json")),
        ("txt", os.path.join(work, "color", "txt", color + ".txt")),
    ):
        configs[name] = write_config(os.path.join(work, "e4", "config-" + name), check_root, package_path, color, [off, on, on, on], catalog_dir)
    for build, actingd in (("v0.9.0", old_actingd), ("new", new_actingd)):
        for name, config in configs.items():
            code, out, err = run_exe([actingd, "check-config", "--config", config])
            try:
                value = json.loads(out)
            except ValueError:
                value = {}
            say("E4", build, "check-config", name, "exit", code, "status", value.get("status"), "error", json.dumps(value.get("error")),
                "policy_configured", value.get("policy_configured"), short(err, 200))
            if build == "v0.9.0" and name in ("zip", "json"):
                check(f"E4.old_check_config.{name}", (value.get("error") or {}).get("code") == "procedure_package_not_regular", json.dumps(value.get("error")))
            if build == "new" and name in ("dir", "zip", "json"):
                check(f"E4.new_check_config.{name}", code == 0 and value.get("status") == "ok", value.get("status"))
            if build == "new" and name == "txt":
                check("E4.new_check_config.txt", (value.get("error") or {}).get("code") == "procedure_package_container_unsupported", json.dumps(value.get("error")))
            if build == "v0.9.0" and name == "dir":
                check("E4.old_check_config.dir", code == 0 and value.get("status") == "ok", value.get("status"))

    # 5. Ledger both ways.
    e5 = os.path.join(work, "e5")
    logs = os.path.join(e5, "logs")
    l0 = os.path.join(e5, "l0-state")
    os.makedirs(l0)
    config_l0 = write_config(os.path.join(e5, "config-l0"), l0, os.path.join(work, "color", "dir", color), color, [off, on, on, on], catalog_dir)
    check("E5.l0_daemon", daemon_run("E5 v0.9.0 actingd L0 directory", old_runtime, config_l0, l0, logs), "")
    events, failure = ledger_events(old_ledger, l0)
    if events is None:
        say("E5", "L0 events (v0.9.0 actingledger)", "failed", failure)
        check("E5.l0_events", False, failure)
        events = []
    counts, admitted = summarize_runs("E5 L0", events)
    check("E5.l0_admitted_directory", reference(color) in admitted and counts.get("task.completed", 0) >= 1, json.dumps(admitted))
    read_copy = os.path.join(e5, "l0-read-copy")
    append_copy = os.path.join(e5, "l0-append-copy")
    shutil.copytree(l0, read_copy)
    shutil.copytree(l0, append_copy)
    before = tree_hashes(read_copy)
    say("E5", "read copy files", json.dumps(before, sort_keys=True))
    results = ledger_reads("E5 new actingledger on L0 copy", new_ledger, read_copy)
    after = tree_hashes(read_copy)
    say("E5", "read copy unchanged", before == after, "changed", json.dumps({k: [before.get(k), after.get(k)] for k in set(before) | set(after) if before.get(k) != after.get(k)}))
    check("E5.new_reads_l0", all(code == 0 and not corrupt for code, corrupt in results.values()), json.dumps(results))
    check("E5.read_copy_sha256_unchanged", before == after, "")

    # The trigger is a 60 s interval: start after the next occurrence so the copy runs once more.
    now = time.time()
    wait = 60 - (now % 60) + 3
    say("E5", "waiting for the next interval occurrence", round(wait, 1), "s")
    time.sleep(wait)
    config_append = write_config(os.path.join(e5, "config-append"), append_copy, os.path.join(work, "color", "zip", color + ".zip"), color, [off, on, on, on], catalog_dir)
    check("E5.append_daemon", daemon_run("E5 new actingd append ZIP", new_runtime, config_append, append_copy, logs), "")
    events_new, failure = ledger_events(new_ledger, append_copy)
    if events_new is None:
        say("E5", "appended events (new actingledger)", "failed", failure)
        events_new = []
    counts_new, admitted_new = summarize_runs("E5 appended", events_new)
    check("E5.appended_zip_run", len(events_new) > len(events) and admitted_new.count(reference(color)) > admitted.count(reference(color)),
          f"{len(events)} -> {len(events_new)} events; admitted {len(admitted)} -> {len(admitted_new)}")
    results = ledger_reads("E5 v0.9.0 actingledger on appended copy", old_ledger, append_copy)
    check("E5.old_reads_appended", all(code == 0 and not corrupt for code, corrupt in results.values()), json.dumps(results))
    events_old, failure = ledger_events(old_ledger, append_copy)
    say("E5", "v0.9.0 actingledger paged events of the appended copy", len(events_old or []), failure)
    check("E5.old_pages_all_events", events_old is not None and len(events_old) == len(events_new), f"{len(events_old or [])} vs {len(events_new)}")

    say("RESULT", "failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


if __name__ == "__main__":
    if sys.argv[1] == "prepare":
        prepare(sys.argv[2], sys.argv[3], sys.argv[4])
        say("RESULT", "prepare failures", len(FAILURES), json.dumps(FAILURES))
        sys.exit(1 if FAILURES else 0)
    sys.exit(run(*sys.argv[2:8]))
