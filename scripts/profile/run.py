#!/usr/bin/env python3
"""Run the rthid pan profiling matrix in a real Apple Terminal window.

Opens a new Terminal window with osascript, resizes it, runs inside it:
  1. the cat replay test: /usr/bin/time -p cat <frames-dir>/<name>.bin for every
     *.bin in --frames-dir (or --replay names), --cat-repeats times each
  2. one short calibration run per (mode, zoom) with RTHID_DEBUG_LOG, which finds
     how many '+' presses reach Zoom In(k) and checks that the ESC[?25l frame
     marker count matches the logged frame count
  3. the measured runs (no debug log, sample(1) during the drag) via drive.py
then waits for a done marker, closes the window and runs analyze.py.

Requires macOS Automation permission for the calling app to control Terminal.
Build with symbols first so sample(1) shows function names:
  CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo build --release
Keep RTHID_DEBUG_LOG off for measured runs: its per frame minimap log dominates the
main thread. The cat replay runs only when --frames-dir holds *.bin frame streams.

Usage:
  python3 scripts/profile/run.py [--out-dir /tmp/rthid-prof/run] [--cols 200 --rows 60]
      [--image PATH] [--bin PATH] [--modes orig,dith] [--zooms 1,2] [--drag-secs 6]
      [--rate 120] [--sample-secs 4] [--repeats 1] [--frames-dir /tmp/rthid-frames]
      [--cat-repeats 2] [--skip-cat] [--skip-app] [--keep-window] [--timeout 900]
"""
import argparse
import glob
import os
import shlex
import subprocess
import sys
import time

from drive import DEFAULT_BIN, DEFAULT_IMAGE, REPO

HERE = os.path.join(REPO, "scripts", "profile")


def osa(script):
    r = subprocess.run(["osascript", "-e", script], capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(f"osascript failed ({r.returncode}): {r.stderr.strip()}")
    return r.stdout.strip()


def build_matrix(a):
    q = shlex.quote
    out = a.out_dir
    py = sys.executable
    drive = os.path.join(HERE, "drive.py")
    common = ["--bin", a.bin, "--image", a.image, "--out-dir", out, "--rate", str(a.rate)]
    def drive_cmd(m, k, *extra):
        return " ".join(q(x) for x in [py, drive, *common, "--mode", m, "--zoom", str(k), *extra])

    lines = [
        "#!/bin/bash",
        f"cd {q(out)}",
        f"while [ ! -f {q(out)}/go ]; do sleep 0.2; done",
        f"stty size > {q(out)}/stty_size.txt",
        f"echo \"$TERM_PROGRAM $TERM_PROGRAM_VERSION\" > {q(out)}/term_program.txt",
    ]
    if not a.skip_cat:
        names = a.replay.split(",") if a.replay else sorted(
            os.path.basename(f)[:-4] for f in glob.glob(os.path.join(a.frames_dir, "*.bin")))
        for n in names:
            lines.append(f": > {q(out)}/cat_{n}.txt")
        for _ in range(a.cat_repeats):
            for n in names:
                lines.append("printf '\\033[0m\\033[2J\\033[H'")
                lines.append(f"/usr/bin/time -p cat {q(os.path.join(a.frames_dir, n + '.bin'))} 2>> {q(out)}/cat_{n}.txt")
        lines.append("printf '\\033[0m\\033[2J\\033[H'")
    if not a.skip_app:
        cases = [(m, int(k)) for m in a.modes.split(",") for k in a.zooms.split(",")]
        for m, k in cases:
            lines.append(drive_cmd(m, k, "--debug-log", "--drag-secs", "1.5", "--sample-secs", "0",
                                   "--save-stream", "--tag", f"calib_{m}_k{k}"))
        for r in range(a.repeats):
            for m, k in cases:
                tag = f"{m}_k{k}" + (f"_r{r + 1}" if a.repeats > 1 else "")
                lines.append(drive_cmd(m, k, "--plus-from", f"{out}/summary_calib_{m}_k{k}.json",
                                       "--drag-secs", str(a.drag_secs), "--sample-secs", str(a.sample_secs),
                                       "--tag", tag))
    lines.append(f"touch {q(out)}/done")
    path = os.path.join(out, "matrix.sh")
    with open(path, "w") as f:
        f.write("\n".join(lines) + "\n")
    os.chmod(path, 0o755)
    return path


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--out-dir", default="/tmp/rthid-prof/run")
    p.add_argument("--cols", type=int, default=200)
    p.add_argument("--rows", type=int, default=60)
    p.add_argument("--image", default=DEFAULT_IMAGE)
    p.add_argument("--bin", default=DEFAULT_BIN)
    p.add_argument("--modes", default="orig,dith")
    p.add_argument("--zooms", default="1,2")
    p.add_argument("--drag-secs", type=float, default=6.0)
    p.add_argument("--rate", type=float, default=120.0)
    p.add_argument("--sample-secs", type=int, default=4)
    p.add_argument("--repeats", type=int, default=1)
    p.add_argument("--frames-dir", default="/tmp/rthid-frames")
    p.add_argument("--replay", help="comma separated replay names (default: every *.bin in --frames-dir)")
    p.add_argument("--cat-repeats", type=int, default=2)
    p.add_argument("--skip-cat", action="store_true")
    p.add_argument("--skip-app", action="store_true")
    p.add_argument("--keep-window", action="store_true")
    p.add_argument("--timeout", type=float, default=900)
    a = p.parse_args()
    a.out_dir = os.path.abspath(a.out_dir)
    os.makedirs(a.out_dir, exist_ok=True)
    for stale in ("go", "done"):
        if os.path.exists(os.path.join(a.out_dir, stale)):
            os.remove(os.path.join(a.out_dir, stale))
    for stale in glob.glob(os.path.join(a.out_dir, "*_*.txt")) + glob.glob(os.path.join(a.out_dir, "summary_*.json")):
        os.remove(stale)
    matrix = build_matrix(a)

    win = osa(f'''tell application "Terminal"
  set t to do script "bash {matrix}; exit"
  delay 0.5
  set w to first window whose selected tab is t
  set number of columns of t to {a.cols}
  set number of rows of t to {a.rows}
  set index of w to 1
  activate
  return id of w
end tell''')
    print(f"Terminal window id {win}")
    time.sleep(1.0)
    size = osa(f'tell application "Terminal" to get {{number of columns, number of rows}} of selected tab of window id {win}')
    print(f"tab size (cols, rows): {size}")
    open(os.path.join(a.out_dir, "go"), "w").close()
    deadline = time.monotonic() + a.timeout
    while not os.path.exists(os.path.join(a.out_dir, "done")):
        if time.monotonic() > deadline:
            print("timeout waiting for done marker", file=sys.stderr)
            break
        time.sleep(1)
    if not a.keep_window:
        time.sleep(1)
        try:
            osa(f'tell application "Terminal" to close (every window whose id is {win})')
        except RuntimeError as e:
            print(e, file=sys.stderr)
    subprocess.run([sys.executable, os.path.join(HERE, "analyze.py"), "--out-dir", a.out_dir])


if __name__ == "__main__":
    main()
