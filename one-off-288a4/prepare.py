# One-off (to be reverted), Workflow #288 A4 evidence: the inputs shared by the merge-base and
# PR-head runs, prepared once from the public umbrella 536f048a BA bundle.
#
#   zips/<package_id>.zip       each published pack ZIP, byte for byte as in the bundle
#   dirs/<package_id>/          the same pack as a content directory: the ZIP's entries
#                               without its six derived JSON documents
#   git/packs/<package_id>/     the same content directories committed to one Git repository
#                               whose origin is the BA resource repository
#   refs/git/<package_id>.json  the GitSourceTree reference of that commit and tree
#   refs/fact/<package_id>.json a PackageAdmitted semantic fact recording that reference
#   scene.png                   a blank 1280x720 frame for the Lab observe call
import hashlib
import io
import json
import os
import re
import struct
import subprocess
import sys
import zipfile
import zlib

UMBRELLA, WORK = sys.argv[1], sys.argv[2]
BUNDLE = os.path.join(UMBRELLA, "bundles", "bluearchive-bundle-3ff697b.zip")
BUNDLE_SHA256 = "df524eac6290c4c0b435f3872a89e12dbe4f49e37555fa102bf96dd71b696af8"
REPOSITORY = "https://github.com/HS7097/ActingCommand-Resources-BlueArchive"
DERIVED = re.compile(
    r"^resources/(manifest\.json"
    r"|recognition/[^/]+\.(pack|pages)\.json"
    r"|navigation/[^/]+\.navigation\.json"
    r"|operations/operations\.(index|primitives)\.json)$"
)


def fail(message):
    print(f"PREP FAIL {message}", flush=True)
    sys.exit(1)


def write(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "xb") as handle:
        handle.write(data)


def png(width, height):
    raw = b"".join(b"\x00" + b"\x00" * (width * 3) for _ in range(height))

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


with open(BUNDLE, "rb") as handle:
    bundle = handle.read()
digest = hashlib.sha256(bundle).hexdigest()
print(f"PREP bundle {os.path.basename(BUNDLE)} sha256={digest} bytes={len(bundle)}")
if digest != BUNDLE_SHA256:
    fail(f"bundle sha256 {digest} != {BUNDLE_SHA256}")
outer = zipfile.ZipFile(io.BytesIO(bundle))
index = json.loads(outer.read("bundle.json"))
print(f"PREP bundle.json schema={index['schema_version']} source_sha={index['source_sha']} packs={len(index['packs'])}")
git_root = os.path.join(WORK, "git")
packs = []
for entry in index["packs"]:
    data = outer.read(entry["path"])
    actual = hashlib.sha256(data).hexdigest()
    if actual != entry["sha256"] or len(data) != entry["byte_count"]:
        fail(f"{entry['path']} sha256={actual} bytes={len(data)} differs from bundle.json")
    package_id = entry["package_id"]
    write(os.path.join(WORK, "zips", package_id + ".zip"), data)
    inner = zipfile.ZipFile(io.BytesIO(data))
    kept, stripped = [], []
    for info in inner.infolist():
        if info.is_dir():
            continue
        name = info.filename
        if DERIVED.match(name):
            stripped.append(name)
            continue
        kept.append(name)
        content = inner.read(name)
        for root in (os.path.join(WORK, "dirs", package_id), os.path.join(git_root, "packs", package_id)):
            write(os.path.join(root, *name.split("/")), content)
    if len(stripped) != 6:
        fail(f"{package_id}: {len(stripped)} derived documents instead of 6: {stripped}")
    print(f"PREP pack {package_id} zip_sha256={actual} source_files={len(kept)} derived_removed={' '.join(sorted(stripped))}")
    packs.append({"package_id": package_id, "entry_task_id": entry["entry_task_id"], "sha256": actual})

env = dict(
    os.environ,
    GIT_AUTHOR_NAME="one-off",
    GIT_AUTHOR_EMAIL="one-off@example.invalid",
    GIT_COMMITTER_NAME="one-off",
    GIT_COMMITTER_EMAIL="one-off@example.invalid",
    GIT_AUTHOR_DATE="2026-10-02T00:00:00+0000",
    GIT_COMMITTER_DATE="2026-10-02T00:00:00+0000",
)


def git(*args):
    result = subprocess.run(["git", "-C", git_root, *args], env=env, capture_output=True, text=True)
    if result.returncode != 0:
        fail(f"git {' '.join(args)} exited {result.returncode}: {result.stderr.strip()}")
    return result.stdout.strip()


git("init", "-q", "-b", "main")
git("config", "core.autocrlf", "false")
git("config", "commit.gpgsign", "false")
git("add", "-A")
git("commit", "-q", "-m", "BA pack sources of the umbrella 536f048a bundle")
git("remote", "add", "origin", REPOSITORY)
commit = git("rev-parse", "HEAD")
print(f"PREP git commit={commit} object_format={git('rev-parse', '--show-object-format=storage')} origin={git('config', '--get', 'remote.origin.url')}")
for pack in packs:
    package_id = pack["package_id"]
    tree = git("rev-parse", f"HEAD:packs/{package_id}")
    reference = {
        "schema_version": "actingcommand.package.git-source-tree.v1",
        "repository": REPOSITORY,
        "commit": {"algorithm": "sha1", "hex": commit},
        "bundle_path": f"packs/{package_id}",
        "tree": {"algorithm": "sha1", "hex": tree},
    }
    text = json.dumps(reference, separators=(",", ":"))
    write(os.path.join(WORK, "refs", "git", package_id + ".json"), text.encode("utf-8"))
    fact = {
        "kind": "package_admitted",
        "package_label": package_id,
        "task_label": pack["entry_task_id"],
        "package_sha256": reference,
    }
    write(
        os.path.join(WORK, "refs", "fact", package_id + ".json"),
        json.dumps(fact, separators=(",", ":")).encode("utf-8"),
    )
    print(f"PREP git_ref {package_id} {text}")
write(os.path.join(WORK, "packs.json"), json.dumps(packs).encode("utf-8"))
write(os.path.join(WORK, "scene.png"), png(1280, 720))
print("PREP done")
