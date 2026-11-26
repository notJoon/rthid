#!/usr/bin/env python3
"""Drive one iTerm2 window for visual QA: type keys, mouse and pastes, capture it.

Requires macOS, iTerm2, Automation permission to control iTerm2 and Screen
Recording permission for the calling app. A window on another Space cannot be
captured, so keep it on a visible display (--at picks its top left corner in
global screen points).

Usage:
  python3 scripts/qa/iterm2.py open [--at X,Y] W H CMD   new W x H point window running CMD, prints its id
  python3 scripts/qa/iterm2.py send ID TEXT              type TEXT into the window
  python3 scripts/qa/iterm2.py cap ID PATH [PX]          capture the window to PATH, longest side PX (default 1400)
  python3 scripts/qa/iterm2.py size ID                   print columns, rows
  python3 scripts/qa/iterm2.py resize ID W H             resize, keeping the top left corner
  python3 scripts/qa/iterm2.py contents ID               print the session text, including scrollback
  python3 scripts/qa/iterm2.py close ID                  type `exit` into the shell

TEXT takes Python escapes (\\x1b, \\r, \\t, \\x7f) and these helpers, with 1-based
cell coordinates and SGR mouse encoding:
  {click X Y}  {drag X1 Y1 X2 Y2}  {wheel up|down|left|right X Y}  {paste TEXT}

Example:
  W=$(python3 scripts/qa/iterm2.py open 1280 760 'rthid sample/pratt.PNG')
  python3 scripts/qa/iterm2.py send $W '+{drag 60 20 80 30}'
  python3 scripts/qa/iterm2.py cap $W /tmp/rthid-qa/drag.png
"""
import re
import subprocess
import sys
import time


def osa(script):
    return subprocess.run(
        ['osascript', '-e', script], capture_output=True, text=True, check=True
    ).stdout.strip()


def applescript_string(text):
    """AppleScript expression for text, with quotes and control characters spelled out."""
    parts, plain = [], ''
    for ch in text:
        if ch in '"\\' or ord(ch) < 32 or ord(ch) == 127:
            if plain:
                parts.append(f'"{plain}"')
                plain = ''
            parts.append(f'(ASCII character {ord(ch)})')
        else:
            plain += ch
    if plain:
        parts.append(f'"{plain}"')
    return ' & '.join(parts) or '""'


def expand(text):
    """Python escapes plus the mouse and paste helpers, as raw terminal input."""
    text = text.encode('latin-1', 'backslashreplace').decode('unicode_escape')

    def helper(m):
        kind, _, rest = m.group(1).partition(' ')
        if kind == 'paste':
            return f'\x1b[200~{rest}\x1b[201~'
        args = rest.split()
        if kind == 'click':
            x, y = args
            return f'\x1b[<0;{x};{y}M\x1b[<0;{x};{y}m'
        if kind == 'wheel':
            direction, x, y = args
            code = {'up': 64, 'down': 65, 'left': 66, 'right': 67}[direction]
            return f'\x1b[<{code};{x};{y}M'
        if kind == 'drag':
            x1, y1, x2, y2 = map(int, args)
            steps = max(abs(x2 - x1), abs(y2 - y1), 1)
            out = f'\x1b[<0;{x1};{y1}M'
            for i in range(1, steps + 1):
                out += f'\x1b[<32;{x1 + (x2 - x1) * i // steps};{y1 + (y2 - y1) * i // steps}M'
            return out + f'\x1b[<0;{x2};{y2}m'
        raise ValueError(f'unknown helper {kind}')

    return re.sub(r'\{((?:click|drag|wheel|paste)\b[^}]*)\}', helper, text)


def session(wid):
    return f'current session of window id {wid}'


def open_window(width, height, command, at=(0, 25)):
    x, y = at
    return osa(
        'tell application "iTerm"\n'
        'set w to (create window with default profile)\n'
        f'set bounds of w to {{{x}, {y}, {x + width}, {y + height}}}\n'
        'delay 1\n'
        f'tell current session of w to write text {applescript_string(command)}\n'
        'return id of w\nend tell'
    )


def send(wid, text):
    osa(f'tell application "iTerm" to tell {session(wid)} to write text '
        f'{applescript_string(expand(text))} newline NO')


def capture(wid, path, px=1400):
    # A window that was just created or resized can briefly refuse capture.
    for attempt in range(3):
        if subprocess.run(['screencapture', '-x', '-o', '-l', str(wid), path]).returncode == 0:
            subprocess.run(['sips', '-Z', str(px), path], capture_output=True, check=True)
            return path
        time.sleep(1)
    raise SystemExit(f'could not capture window {wid}; is it on a visible Space?')


def size(wid):
    return osa(f'tell application "iTerm" to tell {session(wid)} to get {{columns, rows}}')


def resize(wid, width, height):
    x, y = map(int, osa(f'tell application "iTerm" to get bounds of window id {wid}').split(', ')[:2])
    osa(f'tell application "iTerm" to set bounds of window id {wid} to {{{x}, {y}, {x + width}, {y + height}}}')


def contents(wid):
    return osa(f'tell application "iTerm" to tell {session(wid)} to get contents')


def main(argv):
    if not argv:
        raise SystemExit(__doc__)
    cmd, *args = argv
    if cmd == 'open':
        at = (0, 25)
        if args[0] == '--at':
            at = tuple(map(int, args[1].split(',')))
            args = args[2:]
        print(open_window(int(args[0]), int(args[1]), args[2], at))
    elif cmd == 'send':
        send(args[0], args[1])
    elif cmd == 'cap':
        print(capture(args[0], args[1], *args[2:]))
    elif cmd == 'size':
        print(size(args[0]))
    elif cmd == 'resize':
        resize(args[0], int(args[1]), int(args[2]))
    elif cmd == 'contents':
        print(contents(args[0]))
    elif cmd == 'close':
        send(args[0], 'exit\r')
    else:
        raise SystemExit(__doc__)


if __name__ == '__main__':
    main(sys.argv[1:])
