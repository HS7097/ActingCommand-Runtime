# One-off (to be reverted), Workflow #288 A4 evidence: the actinglab CLI of one build.
#   (d) `actinglab --json capabilities` and `actinglab --json schema package`: package references
#   (b) `actinglab --json observe` offline on a GitSourceTree reference of its Git worktree
import json
import os
import subprocess
import sys

LABEL, EXE, WORK = sys.argv[1], sys.argv[2], sys.argv[3]
PACK = "bluearchive.jp.notice_home"


def run(args):
    result = subprocess.run(
        [EXE, *args], capture_output=True, text=True, encoding="utf-8", errors="replace"
    )
    return result.returncode, result.stdout, result.stderr


def data_of(text):
    value = json.loads(text)
    return value.get("data", value) if isinstance(value, dict) else value


code, out, err = run(["--json", "capabilities"])
print(f"D|{LABEL}|capabilities exit={code}")
if code != 0:
    print(f"D|{LABEL}|capabilities stderr={err.strip()[:400]}")
    sys.exit(1)
data = data_of(out)
domains = data["schema_domains"]["package_reference"]
lab2 = data["lab2_cli"]["schema_versions"]["package_reference"]
print(f"D|{LABEL}|capabilities schema_domains.package_reference keys={sorted(domains)}")
print(f"D|{LABEL}|capabilities schema_domains.package_reference={json.dumps(domains, sort_keys=True)}")
print(f"D|{LABEL}|capabilities lab2_cli.schema_versions.package_reference={json.dumps(lab2, sort_keys=True)}")
print(f"D|{LABEL}|capabilities git_source_tree_listed={'git_source_tree' in domains or 'git_source_tree' in lab2}")

code, out, err = run(["--json", "schema", "package"])
print(f"D|{LABEL}|schema package exit={code}")
if code != 0:
    print(f"D|{LABEL}|schema package stderr={err.strip()[:400]}")
    sys.exit(1)
schema = data_of(out)
print(f"D|{LABEL}|schema package package_reference={json.dumps(schema['package_reference'], sort_keys=True)}")

with open(os.path.join(WORK, "refs", "git", PACK + ".json"), encoding="utf-8") as handle:
    reference = handle.read()
package = os.path.join(WORK, "git", "packs", PACK)
scene = os.path.join(WORK, "scene.png")
code, out, err = run(
    ["--json", "observe", "--package", package, "--package-ref", reference, "--scene", scene]
)
print(f"B|{LABEL}|cli observe {PACK} git_source_tree exit={code}")
try:
    value = json.loads(out)
    summary = {key: value.get(key) for key in ("ok", "status", "error") if key in value}
    body = value.get("data") or {}
    if isinstance(body, dict):
        summary.update({key: body.get(key) for key in ("state", "page", "matched") if key in body})
    print(f"B|{LABEL}|cli observe result={json.dumps(summary, sort_keys=True)[:900]}")
except ValueError:
    print(f"B|{LABEL}|cli observe stdout={out.strip()[:900]}")
if err.strip():
    print(f"B|{LABEL}|cli observe stderr={err.strip()[:900]}")
