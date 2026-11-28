#!/usr/bin/env python3
"""Replay iTerm2 QA scenarios for rthid and save screenshots to check by eye.

Scenarios:
  fit-zoom-fit  Original at Fit, '+' to the first zoom step, '-' back to Fit.
                The image must be visible in the first and last shots.
  narrow        Dithered 1x1 image, then drop a 40x3000 image (one cell column
                at Fit). No bar from the previous square may stay on the first row
                (#30).
  resize        Resize the window three times, quit, and count rthid lines left
                in the shell scrollback. Prints `leaked N`; N must be 0 (#29).

Every scenario runs in one new iTerm2 window with its own config file, so the
user's config is untouched. See iterm2.py for the required permissions.

Usage:
  cargo build --release
  python3 scripts/qa/scenarios.py [--out-dir /tmp/rthid-qa] [--bin PATH] [--at X,Y]
      [--size 1280x760] [--keep-window] [SCENARIO ...]
"""
import argparse
import json
import os
import re
import struct
import subprocess
import sys
import time
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import iterm2  # noqa: E402

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
SAMPLE = os.path.join(REPO, 'sample', 'pratt.PNG')
# Lines only rthid draws: help line, status line and panel borders.
LEAK = re.compile(r'o: open \||Fit \((?:sextant|quadrant)\)|┌rthid|┌Parameters|┌Brush|┌Layers')


def write_png(path, width, height, rgb):
    rows = b''.join(b'\0' + b''.join(bytes(rgb(x, y)) for x in range(width)) for y in range(height))
    chunk = lambda kind, data: (struct.pack('>I', len(data)) + kind + data
                                + struct.pack('>I', zlib.crc32(kind + data)))
    with open(path, 'wb') as f:
        f.write(b'\x89PNG\r\n\x1a\n'
                + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 2, 0, 0, 0))
                + chunk(b'IDAT', zlib.compress(rows)) + chunk(b'IEND', b''))


def default_bin():
    meta = subprocess.run(['cargo', 'metadata', '--format-version', '1', '--no-deps'],
                          cwd=REPO, capture_output=True, text=True, check=True)
    return os.path.join(json.loads(meta.stdout)['target_directory'], 'release', 'rthid')


class Run:
    def __init__(self, args):
        self.out, self.bin = args.out_dir, args.bin
        self.width, self.height = args.size
        self.wid = iterm2.open_window(self.width, self.height, f"cd '{self.out}'", args.at)
        time.sleep(1)

    def rthid(self, image):
        # Clear the screen and scrollback first so leak counts start from zero.
        iterm2.send(self.wid, "clear; printf '\\e[3J'\r")
        time.sleep(1)
        iterm2.send(self.wid, f"'{self.bin}' '{image}' --config '{self.out}/config.toml'\r")
        time.sleep(3)

    def keys(self, text, wait=1.5):
        iterm2.send(self.wid, text)
        time.sleep(wait)

    def shot(self, name):
        print(iterm2.capture(self.wid, os.path.join(self.out, f'{name}.png')))

    def quit(self):
        self.keys('q')


def fit_zoom_fit(run):
    run.rthid(SAMPLE)
    run.shot('fit-zoom-fit-1-fit')
    run.keys('+')
    run.shot('fit-zoom-fit-2-zoomed')
    run.keys('-')
    run.shot('fit-zoom-fit-3-fit')
    run.quit()


def narrow(run):
    tiny, tall = (os.path.join(run.out, name) for name in ('tiny1x1.png', 'narrow40x3000.png'))
    # White dithers to white with the default palette, so a leftover bar stands out.
    write_png(tiny, 1, 1, lambda x, y: (255, 255, 255))
    write_png(tall, 40, 3000, lambda x, y: (x * 6, y % 256, 64))
    run.rthid(tiny)
    run.keys(' ')
    run.keys('{paste ' + tall + '}', wait=2.5)
    run.shot('narrow-after-drop')
    run.quit()


def resize(run):
    run.rthid(SAMPLE)
    for width, height in [(800, 500), (300, 150)] * 3:
        iterm2.resize(run.wid, width, height)
        time.sleep(0.5)
    iterm2.resize(run.wid, run.width, run.height)
    time.sleep(1.5)
    run.quit()
    leaked = [line for line in iterm2.contents(run.wid).splitlines() if LEAK.search(line)]
    print(f'leaked {len(leaked)}')
    for line in leaked[:5]:
        print('  ' + line.rstrip()[:120])


SCENARIOS = {'fit-zoom-fit': fit_zoom_fit, 'narrow': narrow, 'resize': resize}


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('scenarios', nargs='*', metavar='SCENARIO', help=', '.join(SCENARIOS))
    p.add_argument('--out-dir', default='/tmp/rthid-qa')
    p.add_argument('--bin')
    p.add_argument('--at', default='0,25', type=lambda s: tuple(map(int, s.split(','))),
                   help='top left corner of the window in global screen points')
    p.add_argument('--size', default='1280x760', type=lambda s: tuple(map(int, s.split('x'))),
                   help='window size in points')
    p.add_argument('--keep-window', action='store_true')
    args = p.parse_args()
    unknown = set(args.scenarios) - set(SCENARIOS)
    if unknown:
        p.error(f'unknown scenario: {", ".join(sorted(unknown))}')
    args.out_dir = os.path.abspath(args.out_dir)
    args.bin = os.path.abspath(args.bin or default_bin())
    os.makedirs(args.out_dir, exist_ok=True)
    run = Run(args)
    try:
        for name in args.scenarios or SCENARIOS:
            print(f'== {name}')
            SCENARIOS[name](run)
    finally:
        if not args.keep_window:
            iterm2.send(run.wid, 'exit\r')


if __name__ == '__main__':
    main()
