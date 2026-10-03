# One-off (to be reverted), Workflow #336 L5: one offline Lab recording on two synthetic 64x36
# frames, then `record stop --dry-run`, printing catalog_on_failure_example and the
# binding_requires item about on_failure. Every printed line starts with "L5|DRYRUN|".
# Usage: dry_run.py <actinglab.exe> <work dir>
import json
import os
import struct
import subprocess
import sys
import zlib

W, H = 64, 36
BG = (32, 32, 32)
FAILURES = []


def say(*parts):
    print("L5|DRYRUN|" + " ".join(str(part) for part in parts), flush=True)


def check(label, condition, detail=""):
    say("CHECK", "PASS" if condition else "FAIL", label, detail)
    if not condition:
        FAILURES.append(label)


def png(path, block, color):
    rows = []
    for y in range(H):
        row = bytearray([0])
        for x in range(W):
            inside = block[0] <= x < block[0] + block[2] and block[1] <= y < block[1] + block[3]
            row.extend(color if inside else BG)
        rows.append(bytes(row))
    raw = b"".join(rows)

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    header = struct.pack(">IIBBBBB", W, H, 8, 2, 0, 0, 0)
    data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")
    with open(path, "wb") as handle:
        handle.write(data)


def lab(exe, args, label):
    result = subprocess.run([exe, "--json", *args], capture_output=True, timeout=600)
    out = result.stdout.decode("utf-8", "replace")
    try:
        value = json.loads(out)
    except ValueError:
        value = None
    if value is None:
        say(label, "exit", result.returncode, "stdout", out[:800], "stderr", result.stderr.decode("utf-8", "replace")[:800])
    else:
        say(label, "exit", result.returncode, "ok", value.get("ok"), "error", json.dumps(value.get("error")))
    return result.returncode, value


def main():
    exe, work = sys.argv[1], sys.argv[2]
    os.makedirs(work, exist_ok=False)
    state = os.path.join(work, "state")
    first = os.path.join(work, "f1.png")
    second = os.path.join(work, "f2.png")
    png(first, (8, 8, 16, 12), (200, 40, 40))
    png(second, (40, 8, 16, 12), (40, 40, 200))
    base = ["--instance", "emu-l5"]
    code, value = lab(exe, base + ["record", "start", "--task-id", "l5_dry_run", "--game", "fixture-game-a",
                                   "--server", "fixture-server-a", "--locale", "en", "--state-dir", state], "record start")
    check("record start", code == 0, f"exit {code}")
    code, value = lab(exe, base + ["record", "mark", "--state-dir", state, "--frame", first, "--page", "start",
                                   "--color", "state/start=8,8,16,12", "--click", "8,8,16,12"], "record mark step 1")
    check("record mark step 1", code == 0, f"exit {code}")
    code, value = lab(exe, base + ["record", "mark", "--state-dir", state, "--close-step"], "record mark --close-step")
    check("record mark --close-step", code == 0, f"exit {code}")
    code, value = lab(exe, base + ["record", "mark", "--state-dir", state, "--frame", second, "--page", "done",
                                   "--color", "state/done=40,8,16,12"], "record mark step 2")
    check("record mark step 2", code == 0, f"exit {code}")
    code, value = lab(exe, base + ["record", "stop", "--state-dir", state, "--dry-run"], "record stop --dry-run")
    data = (value or {}).get("data") or {}
    lab_data = data.get("lab") or {}
    say("status", json.dumps(data.get("status")), "lab.status", json.dumps(lab_data.get("status")),
        "digest", lab_data.get("digest"), "warnings", json.dumps(lab_data.get("warnings")))
    example = lab_data.get("catalog_on_failure_example")
    say("catalog_on_failure_example", json.dumps(example, sort_keys=True))
    items = [item for item in lab_data.get("binding_requires") or [] if "on_failure" in item]
    for item in items:
        say("binding_requires on_failure item:", item)
    check("dry run validated", code == 0 and data.get("status") == "validated" and lab_data.get("status") == "validated",
          f"exit {code}")
    check("retry_backoff_ms is 0", example == {"action": "pause", "retry_limit": 1, "retry_backoff_ms": 0,
                                               "escalation_threshold": 2}, json.dumps(example))
    check("binding_requires says immediately (R22)", len(items) == 1 and "The rerun happens immediately (R22)" in items[0]
          and "next period of a clock trigger" not in items[0], f"{len(items)} item(s)")
    say("failures", len(FAILURES), json.dumps(FAILURES))
    sys.exit(1 if FAILURES else 0)


main()
