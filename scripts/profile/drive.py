#!/usr/bin/env python3
"""Drive rthid through a pty proxy and measure panning throughput.

Runs inside a real terminal (for example an Apple Terminal window opened by run.py).
It forks rthid on a pty with the same window size as its own tty, copies every
byte the child writes to its own stdout unchanged (so the real terminal still has
to parse and paint it), and injects keys and SGR mouse drag events into the child.

Sequence:
  1. wait until rthid enabled SGR mouse reporting (ESC[?1006h), then --startup seconds
  2. press Space if --mode dith (rthid starts in Original)
  3. press '+' until the debug log reports Zoom In(k) (when --debug-log is on),
     or exactly --plus N times (or the count stored by a calibration run, --plus-from)
  4. drag horizontally inside the preview for --drag-secs at --rate events/s,
     1 cell per event, sweeping +-amplitude around the preview centre
  5. optionally run `sample <pid> --sample-secs` during the drag
  6. release, press 'q', wait for exit

Outputs in --out-dir (tag defaults to <mode>_k<k>[_nolog]):
  summary_<tag>.json   bytes, read chunks, frames (count of ESC[?25l, emitted once
                       per ratatui draw), duration, events injected, settle time
  stream_<tag>.bin     raw child output (only with --save-stream)
  log_<tag>.txt        RTHID_DEBUG_LOG (only with --debug-log)
  sample_<tag>.txt     macOS sample output (only with --sample-secs > 0)

Example:
  python3 drive.py --bin ~/.cache/cargo-target/release/rthid \
      --image tests/fixtures/IMG_7139.PNG --mode orig --zoom 1 --out-dir /tmp/rthid-prof
"""
import argparse
import fcntl
import json
import os
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import termios
import threading
import time
import tty

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
MARKER = b"\x1b[?25l"


def parse_args():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--bin", default=os.path.expanduser("~/.cache/cargo-target/release/rthid"))
    p.add_argument("--image", default=os.path.join(REPO, "tests/fixtures/IMG_7139.PNG"))
    p.add_argument("--mode", choices=["orig", "dith"], required=True)
    p.add_argument("--zoom", type=int, default=1, help="target Zoom::In(k)")
    p.add_argument("--plus", type=int, help="press '+' exactly N times instead of reading the log")
    p.add_argument("--plus-from", help="read plus_presses from an earlier summary json (calibration run)")
    p.add_argument("--debug-log", action="store_true", help="set RTHID_DEBUG_LOG (adds per frame minimap logging)")
    p.add_argument("--drag-secs", type=float, default=6.0)
    p.add_argument("--rate", type=float, default=120.0, help="drag events per second")
    p.add_argument("--amplitude", type=int, default=40, help="sweep half width in cells")
    p.add_argument("--sample-secs", type=int, default=4, help="0 disables sample(1)")
    p.add_argument("--startup", type=float, default=1.0, help="extra wait after rthid is ready")
    p.add_argument("--config", help="rthid --config path (default <out-dir>/config.toml)")
    p.add_argument("--out-dir", default="/tmp/rthid-prof")
    p.add_argument("--tag")
    p.add_argument("--save-stream", action="store_true")
    p.add_argument("--extra-arg", action="append", default=[], help="extra rthid CLI arg (repeatable)")
    return p.parse_args()


class Copier(threading.Thread):
    """Copies child output to stdout and records byte, chunk and frame marker times."""

    def __init__(self, fd, stream):
        super().__init__(daemon=True)
        self.fd, self.stream = fd, stream
        self.events = []  # (time, nbytes, markers)
        self.lock = threading.Lock()
        self.last_output = 0.0
        self.frames = 0
        self.ready = threading.Event()  # set once SGR mouse reporting is enabled
        self.watch = None  # bytes pattern to look for, set by the main thread
        self.watch_hit = threading.Event()

    def run(self):
        out = sys.stdout.buffer.raw if hasattr(sys.stdout.buffer, "raw") else sys.stdout.buffer
        tail = b""
        while True:
            try:
                data = os.read(self.fd, 1 << 16)
            except OSError:
                break
            if not data:
                break
            now = time.monotonic()
            buf = tail + data
            # The tail is shorter than MARKER, so no marker is counted twice.
            markers = (tail[-(len(MARKER) - 1):] + data).count(MARKER)
            if not self.ready.is_set() and b"\x1b[?1006h" in buf:
                self.ready.set()
            if self.watch and self.watch in buf:
                self.watch_hit.set()
            tail = buf[-15:]
            with self.lock:
                self.events.append((now, len(data), markers))
                self.last_output = now
                self.frames += markers
            if self.stream:
                self.stream.write(data)
            view = memoryview(data)
            while view:
                n = os.write(1, view)
                view = view[n:]

    def window(self, t0, t1):
        with self.lock:
            ev = [e for e in self.events if t0 <= e[0] < t1]
        return {
            "bytes": sum(e[1] for e in ev),
            "chunks": len(ev),
            "frames": sum(e[2] for e in ev),
        }


def last_zoom(log_path):
    try:
        with open(log_path, "rb") as f:
            f.seek(0, 2)
            size = f.tell()
            f.seek(max(0, size - 400_000))
            text = f.read().decode("utf-8", "replace")
    except OSError:
        return None
    found = re.findall(r"\[minimap\] frame=\d+ grid=(\d+)x(\d+) view=Viewport \{ zoom: (\w+(?:\(\d+\))?)", text)
    return found[-1] if found else None


def main():
    a = parse_args()
    os.makedirs(a.out_dir, exist_ok=True)
    tag = a.tag or f"{a.mode}_k{a.zoom}" + ("" if a.debug_log else "_nolog")
    config = a.config or os.path.join(a.out_dir, "config.toml")
    log_path = os.path.join(a.out_dir, f"log_{tag}.txt")
    if a.debug_log and os.path.exists(log_path):
        os.remove(log_path)
    if a.plus is None and a.plus_from:
        with open(a.plus_from) as f:
            a.plus = json.load(f)["plus_presses"]
    if not a.debug_log and a.plus is None:
        sys.exit("--plus is required without --debug-log")

    winsz = fcntl.ioctl(1, termios.TIOCGWINSZ, b"\0" * 8)
    rows, cols = struct.unpack("HHHH", winsz)[:2]
    old = termios.tcgetattr(0) if os.isatty(0) else None
    if old:
        tty.setraw(0)

    env = dict(os.environ)
    env.pop("RTHID_DEBUG_LOG", None)
    if a.debug_log:
        env["RTHID_DEBUG_LOG"] = log_path
    pid, fd = pty.fork()
    if pid == 0:
        os.execve(a.bin, [a.bin, a.image, "--config", config, *a.extra_arg], env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, winsz)
    os.kill(pid, signal.SIGWINCH)

    stream = open(os.path.join(a.out_dir, f"stream_{tag}.bin"), "wb") if a.save_stream else None
    copier = Copier(fd, stream)
    copier.start()

    def send(b):
        os.write(fd, b)

    def press(key, timeout=3.0):
        """Sends a key and waits until at least one more frame was drawn."""
        before = copier.frames
        send(key)
        deadline = time.monotonic() + timeout
        while copier.frames == before and time.monotonic() < deadline:
            time.sleep(0.01)
        time.sleep(0.15)

    summary = {"tag": tag, "mode": a.mode, "zoom": a.zoom, "cols": cols, "rows": rows, "debug_log": a.debug_log}
    zoom_seen = None
    try:
        if not copier.ready.wait(30):
            raise RuntimeError("rthid did not enable mouse reporting within 30 s")
        time.sleep(a.startup)
        if a.mode == "dith":
            copier.watch = b"Dithered"
            press(b" ")
            summary["dithered_seen"] = copier.watch_hit.wait(3)
            time.sleep(0.5)
        target = f"In({a.zoom})"
        presses = 0
        if a.plus is not None:
            for _ in range(a.plus):
                press(b"+")
                presses += 1
            time.sleep(0.5)
            zoom_seen = last_zoom(log_path) if a.debug_log else None
        else:
            for _ in range(10):
                press(b"+")
                presses += 1
                time.sleep(0.2)
                zoom_seen = last_zoom(log_path)
                if zoom_seen and zoom_seen[2] == target:
                    break
            time.sleep(0.5)
        summary["plus_presses"] = presses
        summary["zoom_seen"] = zoom_seen

        # Preview spans columns 1..cols-31 and rows 1..rows-4 (0 based), see ui::layout.
        pw, ph = cols - 31, rows - 4
        cx = 1 + pw // 2 + 1  # 1 based for SGR
        cy = 1 + ph // 2 + 1
        amp = max(1, min(a.amplitude, pw // 2 - 4))
        summary["drag"] = {"cx": cx, "cy": cy, "amplitude": amp, "rate": a.rate}
        x = cx
        direction = 1
        send(f"\x1b[<0;{x};{cy}M".encode())
        time.sleep(0.05)
        n = int(a.drag_secs * a.rate)
        sampler = None
        t0 = time.monotonic()
        for i in range(n):
            target_t = t0 + i / a.rate
            delay = target_t - time.monotonic()
            if delay > 0:
                time.sleep(delay)
            if i == int(0.5 * a.rate) and a.sample_secs > 0:
                sampler = subprocess.Popen(
                    ["sample", str(pid), str(a.sample_secs), "-file", os.path.join(a.out_dir, f"sample_{tag}.txt")],
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            x += direction
            if x >= cx + amp or x <= cx - amp:
                direction = -direction
            send(f"\x1b[<32;{x};{cy}M".encode())
        t1 = time.monotonic()
        send(f"\x1b[<0;{x};{cy}m".encode())
        # Wait for the backlog to drain: output quiet for 0.5 s.
        while time.monotonic() - copier.last_output < 0.5:
            time.sleep(0.05)
        settle_end = copier.last_output
        w = copier.window(t0, t1)
        w_all = copier.window(t0, settle_end + 0.001)
        summary.update({
            "events": n,
            "drag_secs": t1 - t0,
            "drag_window": w,
            "until_settled": dict(w_all, secs=settle_end - t0),
            "settle_after_release_secs": settle_end - t1,
            "fps": w["frames"] / (t1 - t0),
            "bytes_per_frame": (w["bytes"] / w["frames"]) if w["frames"] else None,
            "bytes_per_sec": w["bytes"] / (t1 - t0),
            "chunk_avg_bytes": (w["bytes"] / w["chunks"]) if w["chunks"] else None,
        })
        if sampler:
            sampler.wait(timeout=30)
        time.sleep(0.3)
        send(b"q")
        deadline = time.monotonic() + 10
        status = None
        while time.monotonic() < deadline:
            done, status = os.waitpid(pid, os.WNOHANG)
            if done:
                break
            time.sleep(0.05)
        else:
            os.kill(pid, signal.SIGKILL)
            os.waitpid(pid, 0)
            status = "killed"
        summary["exit_status"] = status
    finally:
        copier.join(timeout=2)
        if stream:
            stream.close()
        if old:
            termios.tcsetattr(0, termios.TCSAFLUSH, old)
        with open(os.path.join(a.out_dir, f"summary_{tag}.json"), "w") as f:
            json.dump(summary, f, indent=2)


if __name__ == "__main__":
    main()
