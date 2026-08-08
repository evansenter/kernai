"""E1 — diagnostic sufficiency, the RFC's headline experiment (M12).

The full E1 puts a fixed operator *agent* in front of each surface and measures
localization rate/time/tokens. That needs model access (and is non-deterministic),
so it lives outside CI — see `make eval` and the note at the bottom. What runs in
CI here is a deterministic **surface-content proxy**: for a set of seeded faults,
score how many of the *localization facts* an operator needs are machine-
recoverable from each surface's output. It measures the ceiling of what any agent
could extract — i.e. it tests P6 directly ("the frame alone is sufficient to
localize the bug"), isolating the surface from the agent exactly as E1 intends.

Each seeded fault defines a checklist of diagnostic facts. Some are on both
surfaces (cause, faulting PC/address); the ones that localize the *root cause* —
the decoded offending instruction, the page-table permission that was violated,
the causal parent, the register file — are on the structured (agentic) surface
only. The proxy score is facts-recovered / facts-needed per surface.
"""

import json

from .mcp import Mcp
from .qemu import QemuKernel


# Seeded faults (the existing fixtures double as E1's stimulus set) and, per
# fault, the diagnostic facts an operator needs to localize it. Each fact has a
# checker for the structured frame and one for the classic printf line.
def _classic_stval_mod4(ln):
    """The classic line carries stval=0x…; misalignment is computable from it."""
    for tok in ln.split():
        if tok.startswith("stval=0x"):
            try:
                return int(tok[6:], 16) % 4 != 0
            except ValueError:
                return False
    return False


def _has(d, *path):
    for k in path:
        if not isinstance(d, dict) or k not in d:
            return False
        d = d[k]
    return d not in (None, "", {}, [])


BUGS = {
    "crasher": {  # illegal instruction: a U-mode write to the sstatus CSR
        "cause_name": "illegal_instruction",
        "facts": [
            ("cause classified",
             lambda f: f.get("cause_name") == "illegal_instruction",
             lambda ln: "illegal_instruction" in ln),
            ("faulting PC",
             lambda f: _has(f, "sepc"),
             lambda ln: "pc=0x" in ln),
            ("offending CSR decoded (root cause)",
             lambda f: f.get("insn", {}).get("csr") == "0x100",
             lambda ln: False),
            ("causal parent (which run)",
             lambda f: _has(f, "caused_by") and _has(f, "pid"),
             lambda ln: False),
            ("full register file",
             lambda f: len(f.get("regs", {})) == 31,
             lambda ln: False),
        ],
    },
    "wild": {  # load page fault: reading kernel memory from U-mode
        "cause_name": "load_page_fault",
        "facts": [
            ("cause classified",
             lambda f: f.get("cause_name") == "load_page_fault",
             lambda ln: "load_page_fault" in ln),
            ("faulting address",
             lambda f: _has(f, "stval"),
             lambda ln: "stval=0x" in ln),
            ("permission violated (root cause: supervisor-only page)",
             lambda f: any(e.get("u") == 0 for e in (f.get("pagewalk") or [])),
             lambda ln: False),
            ("causal parent (which run)",
             lambda f: _has(f, "caused_by") and _has(f, "pid"),
             lambda ln: False),
        ],
    },
    "wxviol": {  # store page fault: writing an executable (W^X) page
        "cause_name": "store_page_fault",
        "facts": [
            ("cause classified",
             lambda f: f.get("cause_name") == "store_page_fault",
             lambda ln: "store_page_fault" in ln),
            ("faulting address",
             lambda f: _has(f, "stval"),
             lambda ln: "stval=0x" in ln),
            ("permission violated (root cause: W^X, x=1 w=0)",
             lambda f: any(e.get("x") == 1 and e.get("w") == 0 for e in (f.get("pagewalk") or [])),
             lambda ln: False),
            ("full register file",
             lambda f: len(f.get("regs", {})) == 31,
             lambda ln: False),
        ],
    },
    "badjump": {  # instruction page fault: fetching from an unmapped address
        "cause_name": "instruction_page_fault",
        "facts": [
            ("cause classified",
             lambda f: f.get("cause_name") == "instruction_page_fault",
             lambda ln: "instruction_page_fault" in ln),
            ("faulting PC",
             lambda f: _has(f, "sepc"),
             lambda ln: "pc=0x" in ln),
            ("no mapping (root cause: page not present, v=0)",
             lambda f: any(e.get("v") == 0 for e in (f.get("pagewalk") or [])),
             lambda ln: False),
            ("causal parent (which run)",
             lambda f: _has(f, "caused_by") and _has(f, "pid"),
             lambda ln: False),
        ],
    },
    "breaker": {  # breakpoint: a bare ebreak instruction
        "cause_name": "breakpoint",
        "facts": [
            ("cause classified",
             lambda f: f.get("cause_name") == "breakpoint",
             lambda ln: "breakpoint" in ln),
            ("faulting PC",
             lambda f: _has(f, "sepc"),
             lambda ln: "pc=0x" in ln),
            ("causal parent (which run)",
             lambda f: _has(f, "caused_by") and _has(f, "pid"),
             lambda ln: False),
            ("full register file",
             lambda f: len(f.get("regs", {})) == 31,
             lambda ln: False),
        ],
    },
    "misalign": {  # misaligned AMO: amoadd.w on addr%4 == 2 (QEMU reports the
        # AMO's read phase, so the cause is load_address_misaligned)
        "cause_name": "load_address_misaligned",
        "facts": [
            ("cause classified",
             lambda f: f.get("cause_name") == "load_address_misaligned",
             lambda ln: "load_address_misaligned" in ln),
            ("faulting address",
             lambda f: _has(f, "stval"),
             lambda ln: "stval=0x" in ln),
            ("misalignment root cause (stval % 4 != 0)",
             lambda f: _has(f, "stval") and int(f["stval"], 16) % 4 != 0,
             lambda ln: _classic_stval_mod4(ln)),
            ("pointer register identified (register file)",
             lambda f: len(f.get("regs", {})) == 31,
             lambda ln: False),
            ("causal parent (which run)",
             lambda f: _has(f, "caused_by") and _has(f, "pid"),
             lambda ln: False),
        ],
    },
    "nullread": {  # load page fault at VA 0: the classic null deref
        "cause_name": "load_page_fault",
        "facts": [
            ("cause classified",
             lambda f: f.get("cause_name") == "load_page_fault",
             lambda ln: "load_page_fault" in ln),
            ("null address (stval == 0)",
             lambda f: f.get("stval") == "0x0",
             lambda ln: " stval=0x0 " in ln + " "),
            ("nothing mapped at 0 (root cause: v=0)",
             lambda f: any(e.get("v") == 0 for e in (f.get("pagewalk") or [])),
             lambda ln: False),
            ("causal parent (which run)",
             lambda f: _has(f, "caused_by") and _has(f, "pid"),
             lambda ln: False),
        ],
    },
    "execdata": {  # instruction page fault: fetch from a LIVE data page
        "cause_name": "instruction_page_fault",
        "facts": [
            ("cause classified",
             lambda f: f.get("cause_name") == "instruction_page_fault",
             lambda ln: "instruction_page_fault" in ln),
            ("faulting PC (the data address)",
             lambda f: _has(f, "sepc"),
             lambda ln: "pc=0x" in ln),
            ("mapped but not executable (root cause: v=1, x=0 — NOT badjump's v=0)",
             lambda f: any(e.get("v") == 1 and e.get("x") == 0
                           for e in (f.get("pagewalk") or []))
                       and not any(e.get("v") == 0 for e in (f.get("pagewalk") or [])),
             lambda ln: False),
            ("jump site identified (ra in the register file)",
             lambda f: _has(f, "regs", "ra"),
             lambda ln: False),
            ("causal parent (which run)",
             lambda f: _has(f, "caused_by") and _has(f, "pid"),
             lambda ln: False),
        ],
    },
    "stackover": {  # store page fault from stack exhaustion
        "cause_name": "store_page_fault",
        "facts": [
            ("cause classified",
             lambda f: f.get("cause_name") == "store_page_fault",
             lambda ln: "store_page_fault" in ln),
            ("faulting address",
             lambda f: _has(f, "stval"),
             lambda ln: "stval=0x" in ln),
            ("sp corroborates exhaustion (|sp - stval| < 4K, via regs)",
             lambda f: _has(f, "regs", "sp") and _has(f, "stval")
                       and abs(int(f["regs"]["sp"], 16) - int(f["stval"], 16)) < 4096,
             lambda ln: False),
            ("landed on a read-only data page (v=1, w=0, x=0 — NOT wxviol's x=1)",
             lambda f: any(e.get("v") == 1 and e.get("w") == 0 and e.get("x") == 0
                           for e in (f.get("pagewalk") or [])),
             lambda ln: False),
            ("causal parent (which run)",
             lambda f: _has(f, "caused_by") and _has(f, "pid"),
             lambda ln: False),
        ],
    },
}

# One curated suite runs the whole stimulus set (kernel-side `seed_suite_eval`).
SUITES = ["e"]


def _collect(q, mcp, classic):
    """Run the stimulus suite once; return faults IN SEEDING ORDER (several
    stimuli share a cause_name, so order — pid on the agentic surface, line
    order on the classic one — is the only sound join key). On the agentic
    surface a fault is a JSON frame; on the classic surface it's a printf line
    captured from the console noise channel."""
    frames_by_pid = {}
    noise_before = len(q.stream.decoder.noise)
    for suite in SUITES:
        mcp.result("tools/call", {"name": "run_suite", "arguments": {"suite": suite}})
        while True:
            f = q.stream.next_frame(timeout=60)
            assert f is not None, "kernel exited during eval suite"
            e = json.loads(f)
            if e["type"] == "fault":
                frames_by_pid[e["pid"]] = e
            if e["type"] == "suite_done":
                break
    if not classic:
        return [frames_by_pid.get(pid) for pid in range(len(BUGS))]
    # Classic surface: faults are raw lines in the decoder's noise channel,
    # in payload order (sequential execution). Only lines from THIS run.
    noise = bytes(q.stream.decoder.noise[noise_before:]).decode(errors="replace")
    return [ln for ln in noise.splitlines() if "[FAULT]" in ln]


def run_eval():
    """Score both surfaces over the seeded-fault set. Returns a scorecard dict."""
    scorecard = {"agentic": {}, "classic": {}}
    with QemuKernel() as q:
        assert json.loads(q.stream.next_frame(timeout=60))["type"] == "hello"
        mcp = Mcp(q)
        agentic = _collect(q, mcp, classic=False)
        mcp.result("tools/call", {"name": "set_surface", "arguments": {"mode": "classic"}})
        classic = _collect(q, mcp, classic=True)

    assert len(classic) == len(BUGS), \
        f"classic surface produced {len(classic)} fault lines for {len(BUGS)} stimuli"
    for idx, (name, bug) in enumerate(BUGS.items()):
        frame = agentic[idx]
        line = classic[idx]
        assert frame is not None, f"agentic surface produced no fault for {name}"
        assert bug["cause_name"] == frame.get("cause_name"), \
            f"{name}: expected {bug['cause_name']}, got {frame.get('cause_name')}"
        assert bug["cause_name"] in line, \
            f"{name}: classic line out of order or wrong cause: {line!r}"
        a_hits = [label for (label, af, _cf) in bug["facts"] if af(frame)]
        c_hits = [label for (label, _af, cf) in bug["facts"] if cf(line)]
        total = len(bug["facts"])
        scorecard["agentic"][name] = (len(a_hits), total, a_hits)
        scorecard["classic"][name] = (len(c_hits), total, c_hits)
    return scorecard


def summarize(scorecard):
    a = sum(v[0] for v in scorecard["agentic"].values())
    c = sum(v[0] for v in scorecard["classic"].values())
    total = sum(v[1] for v in scorecard["agentic"].values())
    return a, c, total


def main():
    sc = run_eval()
    a, c, total = summarize(sc)
    print("E1 (surface-content proxy) — diagnostic facts recoverable per surface\n")
    print(f"{'seeded fault':<12} {'agentic':>18} {'classic':>18}")
    print("-" * 50)
    for name in BUGS:
        ah, at, _ = sc["agentic"][name]
        ch, ct, _ = sc["classic"][name]
        print(f"{name:<12} {f'{ah}/{at} facts':>18} {f'{ch}/{ct} facts':>18}")
    print("-" * 50)
    print(f"{'TOTAL':<12} {f'{a}/{total}':>18} {f'{c}/{total}':>18}")
    print(f"\nThe structured surface recovers {a}/{total} localization facts; the "
          f"classic\nprintf surface recovers {c}/{total}. The gap is the root-cause "
          "detail —\ndecoded instruction, violated page permission, causal parent, "
          "registers —\nthat P6 says an agent needs and a log line drops. That gap "
          "IS the E1 thesis:\nthe surface, not the agent, decides debuggability.")
    print("\nNote: this is the deterministic surface-content proxy (measures the "
          "ceiling\nof what any agent could extract). The full agent-in-the-loop E1 "
          "(localization\nrate/tokens with a real model) needs model access and is "
          "future work.")


if __name__ == "__main__":
    main()
