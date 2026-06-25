#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Build a Lab-1y input.zip from a real AK bundle for acceptance testing."""
import os, json, shutil, zipfile, sys

AK = r"C:\.ClaudeCode\ActingCommand\ActingCommand-Resources-Arknights"
BASE = r"C:\.ClaudeCode\ActingCommand\runs\actinglab\labpkg"
ENTRY = sys.argv[1] if len(sys.argv) > 1 else "open_terminal"
MODE = sys.argv[2] if len(sys.argv) > 2 else "navigable_route"

pkg = os.path.join(BASE, "pkg")
if os.path.exists(pkg):
    shutil.rmtree(pkg)
os.makedirs(pkg)
res = os.path.join(pkg, "resources")
for sub in ("operations", "recognition", "navigation"):
    shutil.copytree(os.path.join(AK, sub), os.path.join(res, sub))
# drop the clean/safety resource dicts (not needed; Lab must not read safety)
for f in ("resources.json", "resources.safety.json"):
    p = os.path.join(res, "operations", f)
    if os.path.exists(p):
        os.remove(p)

json.dump({"game": "arknights", "server": "cn", "entry_task_id": ENTRY, "version": "acceptance"},
          open(os.path.join(res, "manifest.json"), "w", encoding="utf-8"))

control = {
    "schema_version": "Lab-1y.control.v1", "package_id": "ak." + ENTRY,
    "execution_mode": MODE, "game": "arknights", "server": "cn",
    "resolution": {"width": 1280, "height": 720}, "entry_task_id": ENTRY,
    "capture_interval_ms": 300, "step_timeout_ms": 4000, "max_steps": 8, "stop_on_error": True,
}
json.dump(control, open(os.path.join(pkg, "control.json"), "w", encoding="utf-8"), ensure_ascii=False, indent=2)

zpath = os.path.join(BASE, "in_%s.zip" % ENTRY)
with zipfile.ZipFile(zpath, "w", zipfile.ZIP_DEFLATED) as z:
    for root, _, files in os.walk(pkg):
        for f in files:
            full = os.path.join(root, f)
            arc = os.path.relpath(full, pkg).replace("\\", "/")
            z.write(full, arc)
print("built:", zpath, os.path.getsize(zpath), "bytes; entry=%s mode=%s" % (ENTRY, MODE))
