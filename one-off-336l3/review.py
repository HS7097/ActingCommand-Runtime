# One-off (to be reverted), Workflow #336 L3 review evidence for PR #589 (findings F1, F2, F5).
# Every printed line starts with "L3|". Usage:
#   review.py run <work prepared by evidence.py> <new tools dir>
import json
import os
import sys

import evidence
from evidence import FAILURES, check, lab, say


def main(work, new_tools):
    with open(os.path.join(work, "cases.json"), encoding="utf-8") as handle:
        sizes = json.load(handle)["sizes"]
    new_lab = os.path.join(new_tools, "actinglab.exe")
    evidence.ENV["ACTINGLAB_SESSION_STATE_DIR"] = os.path.join(work, "default-session")
    frames = os.path.join(work, "frames")
    w, h = sizes["auto_off.png"]
    state = os.path.join(work, "review-state")
    base = ["--instance", "emu-review"]

    def mark(label, args, limit=4000):
        return lab(new_lab, base + ["record", "mark", "--state-dir", state, *args], "R " + label, limit=limit)

    lab(new_lab, base + ["record", "start", "--task-id", "review_case", "--locale", "en-US", "--state-dir", state], "R start")

    # (b) frame + marks + click, then the byte-identical frame with one more mark.
    code, value, error = mark("frame f1 + marks + click", ["--frame", os.path.join(frames, "f1.png"),
                                                           "--template", f"ui/a=1180,664,{w},{h}", "--click-from", "ui/a"])
    data = (value or {}).get("data") or {}
    check("B.first", code == 0 and data.get("step") == 1 and data.get("step_opened") is True, f"exit {code}")
    code, value, error = mark("same frame f1 + one more mark", ["--frame", os.path.join(frames, "f1.png"),
                                                                "--color", "state/a=1173,680,1,1"])
    data = (value or {}).get("data") or {}
    say("B", "step", data.get("step"), "step_opened", data.get("step_opened"), "closed_step", json.dumps(data.get("closed_step")),
        "frame", json.dumps(data.get("frame")), "step_state", json.dumps(data.get("step_state")))
    check("B.identical_frame_targets_open_step", code == 0 and data.get("step") == 1 and data.get("step_opened") is False
          and data.get("closed_step") is None and data.get("frame") is None
          and (data.get("step_state") or {}).get("marks") == 2 and (data.get("step_state") or {}).get("frames") == 1,
          json.dumps(data.get("step_state")))

    # (a) two --sample in one call get distinct frame ids.
    code, value, error = mark("two samples in one call", ["--sample", os.path.join(frames, "f1b.png"),
                                                          "--sample", os.path.join(frames, "f1.png")])
    samples = [frame["frame_id"] for frame in ((value or {}).get("data") or {}).get("samples", [])]
    say("A", "sample frame ids", json.dumps(samples))
    check("A.two_samples_distinct", code == 0 and len(samples) == 2 and len(set(samples)) == 2 and "f0001" not in samples, json.dumps(samples))

    # (a) a page transition frame with one --sample get distinct frame ids.
    code, value, error = mark("page transition frame + one sample", ["--step", "1", "--transition", "page",
                                                                     "--frame", os.path.join(frames, "loading.png"),
                                                                     "--sample", os.path.join(frames, "loading.png"),
                                                                     "--color", "load/bar=600,358,8,4"])
    transition = ((value or {}).get("data") or {}).get("transition") or {}
    ids = [frame["frame_id"] for frame in transition.get("frames") or []]
    roles = [frame["role"] for frame in transition.get("frames") or []]
    say("A", "transition frame ids", json.dumps(ids), "roles", json.dumps(roles))
    check("A.transition_frames_distinct", code == 0 and len(ids) == 2 and len(set(ids)) == 2 and not set(ids) & set(samples) and "f0001" not in ids,
          json.dumps(ids))
    code, value, error = lab(new_lab, base + ["record", "status", "--state-dir", state], "R status", limit=8000)
    steps = ((((value or {}).get("data") or {}).get("lab") or {}).get("steps")) or []
    all_ids = [frame["frame_id"] for step in steps for frame in step.get("frames", [])]
    all_ids += [frame["frame_id"] for step in steps for frame in ((step.get("transition") or {}).get("frames") or [])]
    say("A", "all frame ids on record", json.dumps(all_ids), "click", json.dumps((steps[0] if steps else {}).get("click")))
    check("A.all_ids_unique", len(all_ids) == len(set(all_ids)) == 5, json.dumps(all_ids))
    check("F3.click_outcome_in_status", ((steps[0] if steps else {}).get("click") or {}).get("outcome") == "declared",
          json.dumps((steps[0] if steps else {}).get("click")))

    # (c) a page transition carrying "sample" (unknown field) is refused.
    request = {"schema_version": "actingcommand.lab-record-mark.v1", "step": 1,
               "transition": {"kind": "page", "frame": os.path.join(frames, "loading.png"), "sample": [os.path.join(frames, "loading.png")],
                              "add": [{"id": "load/x", "family": "color", "region": {"x": 600, "y": 358, "width": 8, "height": 4}}]},
               "replace_transition": True}
    code, value, error = mark("page transition with an unknown field sample", ["--request-json", json.dumps(request)])
    check("C.unknown_transition_field", code == 2 and (error or {}).get("code") == "validation_failed" and "sample" in (error or {}).get("message", ""),
          json.dumps(error)[:400])
    say("RESULT", "failures", len(FAILURES), json.dumps(FAILURES))
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[2], sys.argv[3]))
