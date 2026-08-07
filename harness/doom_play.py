"""`make doom-play` — an agent plays DOOM through the kernel's own surfaces.

This closes the RFC's agentic loop end-to-end on a real workload:

  start:   the session begins over MCP — `tools/call run_suite {suite:"doom"}`
           (P4: the same structured plane every suite uses; no side channel)
  observe: `frame` events (the deterministic ASCII surface) + `fbchunk`
           color keyframes (SYS_FRAME), reassembled into PNG screenshots
  decide:  a small frame-indexed policy: leave the title, walk the menu
           (New Game → episode → skill), then move and shoot in E1M1
  act:     key events over serial → the kernel's key ring → SYS_GETKEY →
           DG_GetKey. Wire format: each event is the two-byte escape sequence
           0xA5 <key> (low 7 bits symbol, bit 7 release) — the prefix keeps
           the shared serial line unambiguous (a bare byte mid-run is
           discarded by the kernel, never misread as a key or a kill)
  end:     the operator kill (0xA5 0x03) — a live, structured remediation
           (`payload_killed reason:"operator"`, the E2 seed) — and then a
           post-mortem `resources/read processes` showing the killed payload.

The policy here is scripted, so the demo is reproducible without a model in
the loop, but the seam it exercises is exactly the one an LLM agent would use:
every observation is a structured event, every action is a byte on the same
serial channel, and a recorded session replays deterministically under QEMU
record/replay (M7).
"""

import base64
import json
import os
import sys

from .doom import COLOR_H, COLOR_W, boot, find_wad, write_png
from .mcp import Mcp

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

KEY_PREFIX = 0xA5  # every key event is the pair 0xA5 <key>
OP_KILL = 0x03     # 0xA5 0x03 = operator kill (never queued as a key)


def press(q, sym):
    q.send(bytes([KEY_PREFIX, ord(sym) & 0x7F]))


def release(q, sym):
    q.send(bytes([KEY_PREFIX, (ord(sym) & 0x7F) | 0x80]))


def tap(q, sym):
    press(q, sym)
    release(q, sym)


# The policy: (frame_number, action, argument). Actions fire when the agent
# OBSERVES that frame — reaction, not scheduling, so it tolerates a couple of
# frames of pipeline lag. Any keydown at the title opens the menu; three
# ENTERs walk New Game → episode → skill; then a movement/fire script plays
# E1M1; finally the operator kill ends the session mid-run.
SCRIPT = [
    (10, "tap", "e"),     # ESC: title → main menu
    (26, "tap", "n"),     # ENTER: New Game → episode select
    (40, "tap", "n"),     # ENTER: Episode 1 → skill select
    (54, "tap", "n"),     # ENTER: Hurt Me Plenty → E1M1 loads
    (85, "press", "u"),   # run forward into the level
    (135, "release", "u"),
    (138, "press", "r"),  # turn right
    (150, "release", "r"),
    (153, "press", "u"),  # forward again
    (168, "tap", "f"),    # fire!
    (180, "tap", "f"),
    (192, "tap", "f"),
    (205, "release", "u"),
    (208, "press", "l"),  # turn back left
    (220, "release", "l"),
    (223, "press", "u"),
    (260, "release", "u"),
    (285, "kill", None),  # operator kill: end the session mid-run (E2 seed)
]


def main():
    out_dir = os.path.join(REPO_ROOT, "harness", "doom_frames")
    os.makedirs(out_dir, exist_ok=True)
    print("Agent-plays-DOOM: booting kernai, starting the suite over MCP...\n")

    frames = 0
    pngs = []
    color = {}
    checksums = []  # per-frame blit checksums: the input-efficacy evidence
    script = sorted(SCRIPT)
    done = []
    killed = False

    with boot(wad=find_wad()) as q:
        hello = q.next_event(timeout=60)
        assert hello and hello["type"] == "hello", f"no hello frame: {hello!r}"

        # Start DOOM on the structured plane (P4) — no single-byte fallback.
        m = Mcp(q)
        m.result("initialize")
        acc = m.result("tools/call",
                       {"name": "run_suite", "arguments": {"suite": "doom"}})
        assert acc.get("status") == "accepted", f"run_suite not accepted: {acc}"
        print(f"  MCP: run_suite doom accepted → {acc}\n")

        def save_color():
            if not color:
                return
            rgb = b"".join(color[s] for s in sorted(color))
            color.clear()
            need = COLOR_W * COLOR_H * 3
            if len(rgb) >= need:
                path = os.path.join(out_dir, f"play_{len(pngs):03d}.png")
                write_png(path, COLOR_W, COLOR_H, rgb[:need], scale=2)
                pngs.append(path)

        pending = b""
        while True:
            e = q.next_event(timeout=240)
            if e is None:
                print("  (kernel exited)")
                break
            t = e["type"]
            if t == "payload_output":
                pending += e["data"].encode("latin1")
                while b"\n" in pending:
                    line, pending = pending.split(b"\n", 1)
                    print("    doom │ " + line.decode("latin1"))
            elif t == "fbchunk":
                color[e["seq"]] = base64.b64decode(e["data"])
            elif t == "frame":
                save_color()
                frames += 1
                checksums.append(e["checksum"])
                # The agent acts on what it has observed.
                while script and script[0][0] <= frames:
                    _, action, arg = script.pop(0)
                    done.append((frames, action, arg))
                    if action == "tap":
                        tap(q, arg)
                    elif action == "press":
                        press(q, arg)
                    elif action == "release":
                        release(q, arg)
                    elif action == "kill":
                        print(f"    ✂ frame {frames}: operator kill (0xA5 0x03)")
                        q.send(bytes([KEY_PREFIX, OP_KILL]))
                    if action != "kill":
                        print(f"    ⌨ frame {frames}: {action} {arg!r}")
            elif t == "payload_killed":
                killed = True
                print(f"    ☠ payload_killed: {json.dumps(e)}")
            elif t == "suite_done":
                # Post-mortem on the same plane: the process table shows the
                # killed payload as a first-class, structured fact (P11).
                procs = m.result("resources/read", {"uri": "processes"})
                print(f"\n  post-mortem processes: {json.dumps(procs)}")
                break
            elif t in ("fault", "panic", "payload_load_fault"):
                print(f"    {t.upper()}: {json.dumps(e)}")
                break

    print(f"\n  Agent session: {frames} frames observed, "
          f"{len(done)} actions sent, {len(pngs)} screenshots → {out_dir}")

    # Input efficacy: without input, DOOM's title screen is STATIC for ~170
    # frames (identical blit checksums until the attract demo starts). The
    # agent's ESC lands around frame 10-12, so the screen must diverge from
    # the title baseline long before frame 100 — this fails if SYS_GETKEY, the
    # key ring, or the symbol mapping silently regress (a kill-only run would
    # otherwise still "pass").
    input_worked = False
    diverged_at = None
    if len(checksums) >= 100:
        baseline = checksums[8]  # title screen, pre-ESC
        for i, c in enumerate(checksums[9:100], start=10):
            if c != baseline:
                diverged_at = i
                input_worked = True
                break
    print(f"  Input efficacy: screen diverged from the title at frame "
          f"{diverged_at} (attract-only runs hold static until ~170) → "
          f"{'OK' if input_worked else 'NO EFFECT — input path broken?'}")

    ok = killed and frames >= 285 and len(pngs) > 5 and input_worked
    print("  " + ("PASS — the agent started DOOM over MCP, played it via "
                  "getkey, and killed it mid-run." if ok else
                  "INCOMPLETE — see events above."))
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
