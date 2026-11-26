#!/usr/bin/env python3
"""Summarize a rthid profiling run directory produced by run.py.

Reads, from --out-dir:
  cat_<name>.txt       /usr/bin/time -p output of the replay test (one block per repeat)
  summary_<tag>.json   drive.py summaries
  sample_<tag>.txt     macOS sample(1) call graphs

Prints a table and writes results.json. Main thread samples are attributed by their
self (leaf) frame plus ancestry into: stdout_write (write syscalls under std Stdout),
debug_log (RTHID_DEBUG_LOG formatting and file writes), idle (kevent/poll waiting
for input), diff (ratatui BufferDiff), encode (crossterm commands, Stdout buffering,
ratatui backend draw), render (rthid cells/ui/app/grid and widgets), other.

Usage: python3 analyze.py --out-dir /tmp/rthid-prof/run [--frames 120]
"""
import argparse
import glob
import json
import os
import re

HEADER = re.compile(r"^    (\d+) Thread_\d+")
LINE = re.compile(r"^([\s+!:|]*?)(\d+) (.*)$")


def main_thread_leaves(path):
    """Yields (self_count, ancestry names) for the main thread of a sample file."""
    lines = open(path, errors="replace").read().splitlines()
    start = None
    for i, line in enumerate(lines):
        if HEADER.match(line) and "main-thread" in line:
            start = i
            break
    if start is None:
        return 0, []
    total = int(HEADER.match(lines[start]).group(1))
    stack = []  # (indent, name, count, child_sum)
    leaves = []

    def pop_to(indent):
        while stack and stack[-1][0] >= indent:
            ind, name, count, child = stack.pop()
            selfc = count - child
            if selfc > 0:
                leaves.append((selfc, [s[1] for s in stack] + [name]))

    for line in lines[start + 1:]:
        if HEADER.match(line) or not line.strip() or line.startswith("Total number"):
            break
        m = LINE.match(line)
        if not m:
            break
        indent = len(m.group(1))
        count = int(m.group(2))
        name = m.group(3)
        pop_to(indent)
        if stack:
            stack[-1][3] += count
        stack.append([indent, name, count, 0])
    pop_to(-1)
    return total, leaves


OWNERS = [
    ("diff", ("6buffer4diff",)),
    ("encode", ("9crossterm", "17ratatui_crossterm", "2io5stdio", "bufwriter")),
    ("render", ("5rthid5cells", "5rthid2ui", "5rthid3app", "5rthid4grid", "5rthid8viewport",
                "15ratatui_widgets", "rthid_dither")),
]


def owner(name):
    """Category whose keyword appears earliest in a mangled name (the impl path, not type params)."""
    best = None
    for cat, keys in OWNERS:
        for k in keys:
            i = name.find(k)
            if i >= 0 and (best is None or i < best[0]):
                best = (i, cat)
    return best[1] if best else None


def classify(path):
    leaf = path[-1]
    joined = " ".join(path)
    syscall_write = re.match(r"(write|__write_nocancel|writev)\s+\(in libsystem_kernel", leaf)
    if "log_minimap" in joined or "std2fs4File" in joined:
        return "debug_log"
    if syscall_write:
        return "stdout_write"
    if re.match(r"(kevent|poll|select|__select|__psynch_cvwait|mach_msg)", leaf):
        return "idle"
    for name in reversed(path):
        cat = owner(name.split("  (in ")[0])
        if cat:
            return cat
    return "other"


def cat_times(path):
    return [float(x) for x in re.findall(r"^real\s+([\d.]+)", open(path).read(), re.M)]


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--out-dir", default="/tmp/rthid-prof/run")
    p.add_argument("--frames", type=int, default=120, help="frames per replay file")
    a = p.parse_args()
    out = {"cat": {}, "runs": {}}
    for f in sorted(glob.glob(os.path.join(a.out_dir, "cat_*.txt"))):
        name = os.path.basename(f)[4:-4]
        t = cat_times(f)
        out["cat"][name] = {"real": t, "fps": [round(a.frames / x, 1) for x in t if x > 0],
                            "bytes": None}
    for f in sorted(glob.glob(os.path.join(a.out_dir, "summary_*.json"))):
        s = json.load(open(f))
        tag = s["tag"]
        r = {k: s.get(k) for k in ("mode", "zoom", "debug_log", "plus_presses", "zoom_seen", "events", "fps",
                                   "bytes_per_frame", "bytes_per_sec", "chunk_avg_bytes",
                                   "settle_after_release_secs")}
        r["frames"] = s.get("drag_window", {}).get("frames")
        r["dithered_seen"] = s.get("dithered_seen")
        stream = os.path.join(a.out_dir, f"stream_{tag}.bin")
        log = os.path.join(a.out_dir, f"log_{tag}.txt")
        if os.path.exists(stream) and os.path.exists(log):
            # Validates the frame proxy: ESC[?25l count vs logged frames.
            r["marker_check"] = {
                "stream_markers": open(stream, "rb").read().count(b"\x1b[?25l"),
                "logged_frames": sum(1 for l in open(log, errors="replace") if l.startswith("[minimap] frame=")),
            }
        sp = os.path.join(a.out_dir, f"sample_{tag}.txt")
        if os.path.exists(sp):
            total, leaves = main_thread_leaves(sp)
            cats = {}
            for c, path in leaves:
                k = classify(path)
                cats[k] = cats.get(k, 0) + c
            r["sample_total"] = total
            r["main_pct"] = {k: round(100 * v / total, 1) for k, v in sorted(cats.items(), key=lambda kv: -kv[1])}
        out["runs"][tag] = r
    with open(os.path.join(a.out_dir, "results.json"), "w") as f:
        json.dump(out, f, indent=2)

    print("cat replay (fps = frames / real seconds):")
    for name, c in out["cat"].items():
        print(f"  {name:10s} real={c['real']}  fps={c['fps']}")
    print()
    cols = ["stdout_write", "encode", "diff", "render", "idle", "debug_log", "other"]
    print(f"{'run':16s} {'fps':>6s} {'frames':>6s} {'KB/frame':>9s} {'MB/s':>6s} " + " ".join(f"{c:>12s}" for c in cols))
    for tag, r in out["runs"].items():
        pct = r.get("main_pct", {})
        bpf = r["bytes_per_frame"]
        print(f"{tag:16s} {r['fps'] or 0:6.1f} {r['frames'] or 0:6d} {(bpf or 0) / 1024:9.1f} {(r['bytes_per_sec'] or 0) / 1e6:6.2f} "
              + " ".join(f"{pct.get(c, 0):11.1f}%" for c in cols))


if __name__ == "__main__":
    main()
