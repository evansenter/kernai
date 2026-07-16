"""`make raycast` — play the C raycaster payload and prove it's deterministic.

Boots the kernel (built with the `cpayloads` feature), sends `c` to run the
craycast payload, and renders each `frame` event as ASCII in the terminal — a
fixed-point raycaster written in C, compiled with clang, running fully
sandboxed as a kernai U-mode payload. Then it boots a second time and checks
the per-frame checksums are identical: same input → same frames, byte for byte
(P9), even for a non-Rust workload.
"""

import json
import sys
import time

from .qemu import QemuKernel

FAST = "--fast" in sys.argv


def collect_frames(q, animate=False):
    """Send `c`, render/collect every `frame` event through `suite_done`."""
    q.send(b"c")
    frames = []
    tty = sys.stdout.isatty()
    while True:
        raw = q.stream.next_frame(timeout=60)
        assert raw is not None, "kernel exited before suite_done"
        e = json.loads(raw)
        if e["type"] == "frame":
            frames.append(e)
            if animate:
                if tty:
                    sys.stdout.write("\033[H")  # cursor home — animate in place
                print(f"  kernai · craycast — a C raycaster, sandboxed  "
                      f"[frame {len(frames):2d}  checksum {e['checksum']}]   ")
                for row in e["rows"]:
                    print("  │" + row + "│")
                sys.stdout.flush()
                if not FAST and tty:
                    time.sleep(0.05)
        elif e["type"] == "suite_done":
            return frames


def main():
    if sys.stdout.isatty():
        sys.stdout.write("\033[2J\033[H")  # clear screen
    print("Booting kernai, running a C raycaster payload...\n")

    with QemuKernel() as q:
        assert json.loads(q.stream.next_frame(timeout=60))["type"] == "hello"
        frames = collect_frames(q, animate=True)

    assert frames, "no frames rendered — is the kernel built with --features cpayloads?"
    checks = [e["checksum"] for e in frames]
    print(f"\n  Rendered {len(frames)} frames from a clang-compiled C payload, "
          "sandboxed by kernai.")

    # Determinism: a second boot must produce byte-identical frame checksums.
    with QemuKernel() as q:
        assert json.loads(q.stream.next_frame(timeout=60))["type"] == "hello"
        checks2 = [e["checksum"] for e in collect_frames(q, animate=False)]

    ok = checks == checks2 and len(checks) > 0
    print(f"  Determinism: {len(checks)} frame checksums, two boots "
          f"{'IDENTICAL' if ok else 'DIVERGED'} — a C workload replays exactly (P9).")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
