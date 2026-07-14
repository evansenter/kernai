"""Milestone acceptance runner: `python3 -m harness.runner <milestone>|all`.

Each milestone check is a function returning None on success and raising
AssertionError (with a readable message) on failure. `make test` runs `all`.
"""

import subprocess
import sys

MILESTONES = {}


def milestone(name):
    def register(fn):
        MILESTONES[name] = fn
        return fn
    return register


@milestone("m0")
def m0_framing():
    """Framing layer round-trips frames over a real pipe (loopback stub)."""
    proc = subprocess.run(
        [sys.executable, "-m", "unittest", "discover", "-s", "harness", "-t", "."],
        capture_output=True, text=True)
    if proc.returncode != 0:
        raise AssertionError(f"framing unit tests failed:\n{proc.stderr}")


@milestone("m1")
def m1_boot_hello():
    """Kernel boots; hello frame arrives over the SBI console (acceptance 1)."""
    from .qemu import KERNEL_ELF, QemuKernel
    assert KERNEL_ELF.exists(), f"{KERNEL_ELF} missing — run `make build`"
    with QemuKernel() as q:
        evt = q.next_event(timeout=60)
        assert evt is not None, f"EOF before any frame; stderr: {q.stderr_tail()}"
        assert evt.get("type") == "hello", f"first event is not hello: {evt}"
        assert evt.get("id") == 0, f"hello must be event 0: {evt}"
        assert evt.get("proto") == 0, f"unknown protocol: {evt}"


def _await(q, evt_type, seen, timeout=60):
    """Read events until one of `evt_type` arrives; every event is appended
    to `seen` so callers can assert stream-wide invariants afterwards."""
    while True:
        evt = q.next_event(timeout)
        assert evt is not None, f"EOF while waiting for {evt_type}; stderr: {q.stderr_tail()}"
        seen.append(evt)
        if evt["type"] == evt_type:
            return evt


@milestone("m2")
def m2_traps_timer_fault():
    """Monotonic timer ticks (acceptance 2); ring query answered; illegal
    instruction yields a structured fault report, then exit (acceptance 3)."""
    import subprocess

    from .qemu import QemuKernel
    with QemuKernel() as q:
        seen = []
        hello = _await(q, "hello", seen)
        assert hello["id"] == 0, f"hello must be event 0: {hello}"

        # Acceptance 2: N timer ticks, monotonically increasing event ids.
        ticks = []
        while len(ticks) < 5:
            ticks.append(_await(q, "tick", seen))
        for a, b in zip(ticks, ticks[1:]):
            assert a["id"] < b["id"], f"tick ids not monotonic: {a} -> {b}"
            assert a["seq"] + 1 == b["seq"], f"tick seq skipped: {a} -> {b}"
            assert a["time"] < b["time"], f"tick time not monotonic: {a} -> {b}"

        # P11 seed: the trap ring answers a serial query.
        q.send(b"r")
        ring = _await(q, "trap_ring", seen)
        assert ring["count"] >= 5, f"ring count below observed ticks: {ring}"
        assert ring["entries"], f"ring dump empty: {ring}"
        assert all(e["cause"] == "timer" for e in ring["entries"]), \
            f"unexpected causes in ring: {ring}"

        # Acceptance 3: deliberate illegal instruction -> structured fault
        # report (cause, sepc, decoded fields), then shutdown — not a hang.
        q.send(b"x")
        fault = _await(q, "fault", seen)
        assert fault["cause"] == "0x2", f"wrong cause: {fault}"
        assert fault["cause_name"] == "illegal_instruction", f"wrong cause_name: {fault}"
        assert int(fault["sepc"], 16) > 0x8020_0000, f"implausible sepc: {fault}"
        assert fault["stval"] == "0xc0001073", f"wrong stval: {fault}"
        insn = fault["insn"]
        assert insn["bits"] == fault["stval"], f"insn bits != stval: {fault}"
        assert insn["opcode"] == "0x73", f"not a SYSTEM opcode: {fault}"
        assert insn["csr"] == "0xc00", f"CSR not decoded: {fault}"
        assert fault["ring"][-1]["cause"] == "illegal_instruction", \
            f"fault missing from its own ring history: {fault}"

        # Stream-wide invariant: event ids strictly increase in emission order.
        ids = [e["id"] for e in seen]
        assert ids == sorted(set(ids)), f"event ids not strictly monotonic: {ids}"

        try:
            q.wait_exit(30)
        except subprocess.TimeoutExpired:
            raise AssertionError("kernel hung after fault report instead of shutting down")


@milestone("determinism")
def determinism_two_boots():
    """P9 seed (E6): two input-free boots yield byte-identical event streams."""
    from .qemu import QemuKernel

    def capture():
        frames = []
        with QemuKernel() as q:
            while len(frames) < 11:  # hello + 10 ticks
                frame = q.stream.next_frame(timeout=60)
                assert frame is not None, f"EOF during capture; stderr: {q.stderr_tail()}"
                frames.append(frame)
        return frames

    a, b = capture(), capture()
    for i, (fa, fb) in enumerate(zip(a, b)):
        assert fa == fb, f"boot diverged at frame {i}: {fa!r} != {fb!r}"


def main(argv):
    which = argv[1] if len(argv) > 1 else "all"
    names = list(MILESTONES) if which == "all" else [which]
    for name in names:
        check = MILESTONES.get(name)
        if check is None:
            print(f"unknown milestone {name!r}; have: {', '.join(MILESTONES)}")
            return 2
        try:
            check()
        except AssertionError as e:
            print(f"[FAIL] {name}: {e}")
            return 1
        print(f"[ OK ] {name}: {check.__doc__.strip().splitlines()[0]}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
