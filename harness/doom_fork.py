"""`make doom-fork` — checkpoint a live DOOM game and fork two divergent
what-if continuations (P8, on a real workload).

The agent starts DOOM over MCP, plays into E1M1, then sends the `#` checkpoint
key — DOOM calls kernai_snapshot(), the kernel deep-copies its whole address
space (the IWAD window aliased, not copied — the deep_copy path the window work
added). It kills DOOM; the kernel then forks the checkpoint into two independent
continuations that resume from the IDENTICAL game state. The harness feeds each
a DIFFERENT input timeline (one turns and walks left, the other right), captures
a colour screenshot from each, and shows they diverge — one saved instant, two
futures. This is the P8 "speculative fork / what-if" story the RFC describes,
running on a full game engine rather than a toy.
"""

import base64
import json
import os
import sys

from .doom import COLOR_H, COLOR_W, boot, find_wad, write_png
from .mcp import Mcp

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
KEY_PREFIX = 0xA5
OP_KILL = 0x03


def key(q, sym, release=False):
    q.send(bytes([KEY_PREFIX, (ord(sym) & 0x7F) | (0x80 if release else 0)]))


def tap(q, sym):
    key(q, sym)
    key(q, sym, release=True)


class Frames:
    """Accumulate fbchunk color keyframes; hand back the newest complete one."""
    def __init__(self):
        self.cur = {}
        self.last = None

    def chunk(self, e):
        self.cur[e["seq"]] = base64.b64decode(e["data"])

    def commit(self):
        if self.cur:
            rgb = b"".join(self.cur[s] for s in sorted(self.cur))
            self.cur = {}
            need = COLOR_W * COLOR_H * 3
            if len(rgb) >= need:
                self.last = rgb[:need]


def play(q, frames, until, actions=None):
    """Read events until `until(frame_count)` is true; apply `actions[fc]` when
    a frame arrives. Returns the frame count reached."""
    actions = actions or {}
    fc = 0
    while True:
        e = json.loads(q.stream.next_frame(timeout=120))
        t = e["type"]
        if t == "fbchunk":
            frames.chunk(e)
        elif t == "frame":
            frames.commit()
            fc += 1
            if fc in actions:
                actions[fc](q)
            if until(fc):
                return fc
        elif t in ("payload_killed", "payload_exit", "suite_done"):
            return fc


def save(frames, name, label):
    path = os.path.join(REPO_ROOT, "harness", "doom_frames", name)
    write_png(path, COLOR_W, COLOR_H, frames.last, scale=2)
    print(f"    ● {label}: {path}")
    return frames.last


def main():
    out = os.path.join(REPO_ROOT, "harness", "doom_frames")
    os.makedirs(out, exist_ok=True)
    print("Checkpoint/fork DOOM: one saved instant, two futures (P8)...\n")

    with boot(wad=find_wad()) as q:
        assert json.loads(q.stream.next_frame(timeout=60))["type"] == "hello"
        m = Mcp(q)
        m.result("initialize")
        acc = m.result("tools/call",
                       {"name": "run_suite", "arguments": {"suite": "doomfork"}})
        assert acc["status"] == "accepted", acc
        frames = Frames()

        # Get into the game: title → menu → new game → E1M1, then walk in a bit.
        menu = {10: lambda q: tap(q, "e"), 26: lambda q: tap(q, "n"),
                40: lambda q: tap(q, "n"), 54: lambda q: tap(q, "n")}
        play(q, frames, until=lambda fc: fc >= 70, actions=menu)
        # Walk forward so there's motion to diverge from.
        key(q, "u")
        play(q, frames, until=lambda fc: fc >= 95)
        key(q, "u", release=True)

        # CHECKPOINT the live game.
        print("    ⑃ frame ~95: sending the checkpoint key (#)")
        tap(q, "#")
        play(q, frames, until=lambda fc: fc >= 105)
        base = save(frames, "fork_base.png", "checkpoint state")

        # Kill the original so the kernel forks the checkpoint.
        q.send(bytes([KEY_PREFIX, OP_KILL]))

        # ---- Continuation A: turn LEFT, walk ----
        contA = []
        while True:
            e = json.loads(q.stream.next_frame(timeout=120))
            if e["type"] == "payload_start" and e.get("restored"):
                break
            assert e["type"] != "suite_done", "no first continuation forked"
        print("    ⑃ continuation A resumed from the checkpoint — turning LEFT")
        fA = Frames()
        key(q, "l")
        play(q, fA, until=lambda fc: fc >= 20)
        key(q, "l", release=True)
        key(q, "u")
        play(q, fA, until=lambda fc: fc >= 45)
        left = save(fA, "fork_A_left.png", "continuation A (left)")
        q.send(bytes([KEY_PREFIX, OP_KILL]))

        # ---- Continuation B: turn RIGHT, walk ----
        while True:
            e = json.loads(q.stream.next_frame(timeout=120))
            if e["type"] == "payload_start" and e.get("restored"):
                break
            assert e["type"] != "suite_done", "no second continuation forked"
        print("    ⑃ continuation B resumed from the SAME checkpoint — turning RIGHT")
        fB = Frames()
        key(q, "r")
        play(q, fB, until=lambda fc: fc >= 20)
        key(q, "r", release=True)
        key(q, "u")
        play(q, fB, until=lambda fc: fc >= 45)
        right = save(fB, "fork_B_right.png", "continuation B (right)")
        q.send(bytes([KEY_PREFIX, OP_KILL]))

    diverged = left != right and left is not None and right is not None
    print(f"\n  Two continuations from one checkpoint; screenshots "
          f"{'DIVERGE' if diverged else 'match'} — "
          f"{'a real game forked into two futures (P8).' if diverged else 'no divergence?'}")
    sys.exit(0 if diverged else 1)


if __name__ == "__main__":
    main()
