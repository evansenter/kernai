"""`make conformance` — drive kernai and the degraded reference implementation
(`refimpl/daemon.py`) through the SAME MCP session, and report where each
surface can and cannot answer.

This is what the reference implementation buys (RFC eval plan): the surface
(`docs/SPEC.md`) is a real contract, targetable by more than one mechanism, so
the evals become a benchmark for *implementations*, not just for kernai. Both
targets speak the identical framing + JSON-RPC; the diff is the diagnostic
richness — which is the E1/E6/P6/P11 thesis made concrete across two backends,
not just two rendering modes of one.
"""

import json
import subprocess
import sys

from .framing import FrameDecoder, encode_frame
from .mcp import Mcp
from .qemu import QemuKernel


class RefimplTarget:
    """A QemuKernel-shaped adapter over the refimpl subprocess, so the same Mcp
    client and the same driving code work against both."""

    def __init__(self):
        self._proc = subprocess.Popen(
            [sys.executable, "-m", "refimpl.daemon"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        self.stream = _DecoderStream(self._proc.stdout)

    def send(self, data):
        self._proc.stdin.write(data)
        self._proc.stdin.flush()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        if self._proc.poll() is None:
            self._proc.terminate()
        try:
            self._proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self._proc.kill()


class _DecoderStream:
    def __init__(self, fileobj):
        self._f = fileobj
        self._dec = FrameDecoder()
        self._q = []

    def next_frame(self, timeout=30):
        while not self._q:
            chunk = self._f.read(1)
            if not chunk:
                return None
            self._q.extend(self._dec.feed(chunk))
        return self._q.pop(0)


def drive(target):
    """Run the shared session against `target`; return what the surface answered."""
    m = Mcp(target)
    init = m.result("initialize")
    tools = {t["name"] for t in m.result("tools/list")["tools"]}
    spec = m.result("resources/read", {"uri": "spec"})
    # Run the seeded-fault workload; capture the fault event's fields.
    m.result("tools/call", {"name": "run_suite", "arguments": {"suite": "e"}})
    fault = None
    while True:
        e = json.loads(target.stream.next_frame(timeout=60))
        if e["type"] == "fault" and fault is None:
            fault = e
        if e["type"] == "suite_done":
            break
    return {
        "server": init["serverInfo"]["name"],
        "tools": tools,
        "spec_arch": spec.get("arch"),
        "fault_fields": set(fault.keys()) if fault else set(),
        "fault": fault,
    }


def main():
    print("Conformance: kernai vs the degraded reference impl, same surface\n")

    with QemuKernel() as q:
        assert json.loads(q.stream.next_frame(timeout=60))["type"] == "hello"
        kernai = drive(q)
    with RefimplTarget() as r:
        assert json.loads(r.stream.next_frame(timeout=30))["type"] == "hello"
        ref = drive(r)

    # Both must speak the protocol: initialize, tools/list, resources/read, and
    # a fault event — the SPEC's structural contract.
    assert "run_suite" in kernai["tools"] and "run_suite" in ref["tools"]
    assert kernai["fault_fields"] and ref["fault_fields"]
    print(f"  protocol conformance: kernai={kernai['server']}, "
          f"refimpl={ref['server']} — both speak framing + MCP ✓\n")

    # The diagnostic-richness gap: fields kernai's fault frame carries that the
    # degraded impl structurally cannot.
    STRUCT = {"regs", "pagewalk", "insn", "caused_by", "sepc", "stval"}
    kernai_has = STRUCT & kernai["fault_fields"]
    ref_has = STRUCT & ref["fault_fields"]
    missing = kernai_has - ref_has
    print("  fault-frame diagnostic fields (P6/P11):")
    print(f"    kernai : {sorted(kernai_has)}")
    print(f"    refimpl: {sorted(ref_has)}  (degraded: {ref['fault'].get('degraded')})")
    print(f"  → kernai's surface exposes {len(missing)} root-cause fields the "
          f"degraded backend can't:\n    {sorted(missing)}")

    ok = "run_suite" in ref["tools"] and len(missing) >= 4 and ref["fault"].get("degraded")
    print("\n" + ("PASS — one surface, two mechanisms; the degradation is exactly "
                  "the structured diagnostic detail the evals measure."
                  if ok else "INCOMPLETE — see above."))
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
