"""`make agent-eval` — the agent-in-the-loop E1/E2 measurement (the real
experiments the CI proxies stand in for).

CI's `e1`/`e2` checks are deterministic *proxies*: e1 scores what any operator
COULD extract from each surface; e2 asserts the remediation MECHANISM works.
This harness closes the actual loop — an operator observes the surface, decides,
and acts — and reports the numbers the RFC's experiments call for:

  E1  localization rate per surface (agentic vs classic), operator held fixed.
  E2  live-incident mean-time-to-remediation (elapsed instruction units) + the
      number of observe→act rounds.

The operator is pluggable (harness/operator.py): the default is the deterministic
RuleOperator, so `make agent-eval` runs with no model and no network and is
reproducible; `KERNAI_OPERATOR=llm ANTHROPIC_API_KEY=… make agent-eval` swaps in
a real Claude model for the true, per-model measurement across the same surfaces.
"""

import json
import sys

from .eval import BUGS
from .mcp import Mcp
from .operator import make_operator
from .qemu import QemuKernel


def e1_session(op):
    """Put `op` in front of both surfaces over the seeded-fault set; return
    per-surface localization rate (root-cause facts recovered / needed)."""
    # The facts each fault's root cause actually needs (the union the structured
    # surface can supply; the classic line supplies only the shallow ones).
    needed = {}
    for name, bug in BUGS.items():
        needed[bug["cause_name"]] = {lbl for (lbl, _a, _c) in bug["facts"]}

    def collect(classic):
        faults = []
        with QemuKernel() as q:
            assert json.loads(q.stream.next_frame(timeout=60))["type"] == "hello"
            m = Mcp(q)
            m.result("initialize")
            if classic:
                m.result("tools/call", {"name": "set_surface", "arguments": {"mode": "classic"}})
            nb = len(q.stream.decoder.noise)
            m.result("tools/call", {"name": "run_suite", "arguments": {"suite": "e"}})
            while True:
                e = json.loads(q.stream.next_frame(timeout=60))
                if not classic and e["type"] == "fault":
                    faults.append(e)
                if e["type"] == "suite_done":
                    break
            if classic:
                noise = bytes(q.stream.decoder.noise[nb:]).decode(errors="replace")
                faults = [ln for ln in noise.splitlines() if "[FAULT]" in ln]
        return faults

    scores = {}
    for surface, classic in (("agentic", False), ("classic", True)):
        obs = collect(classic)
        recovered = total = 0
        # Localization facts are a proxy count; map each observation to its bug.
        for i, (name, bug) in enumerate(BUGS.items()):
            o = obs[i] if i < len(obs) else ""
            got = op.localize(o)
            # Count against a canonical root-cause fact set per fault (5 facts:
            # cause, pc/addr, decoded-root, registers-or-walk, causal-parent).
            canon = _canon_facts(bug)
            recovered += len(got & canon)
            total += len(canon)
        scores[surface] = (recovered, total)
    return scores


def _canon_facts(bug):
    """The root-cause fact labels for a bug, normalized to what `localize`
    emits (so agentic and classic are scored on the same yardstick)."""
    facts = {f"cause={bug['cause_name']}"}
    labels = {lbl for (lbl, _a, _c) in bug["facts"]}
    if any("PC" in l or "pc" in l for l in labels):
        facts.add("faulting-pc")
    if any("address" in l for l in labels):
        facts.add("faulting-addr")
    if any("root cause" in l or "root" in l for l in labels):
        # the structured-only root fact — mapped to whichever localize emits
        facts |= {"decoded-instruction", "root:not-mapped", "root:supervisor-only",
                  "root:w^x"} & _possible_roots(bug)
    if any("register" in l for l in labels):
        facts.add("register-file")
    if any("causal" in l or "parent" in l for l in labels):
        facts.add("causal-parent")
    return facts


def _possible_roots(bug):
    c = bug["cause_name"]
    return {
        "illegal_instruction": {"decoded-instruction"},
        "load_page_fault": {"root:not-mapped", "root:supervisor-only"},
        "store_page_fault": {"root:w^x"},
        "instruction_page_fault": {"root:not-mapped"},
    }.get(c, set())


def e2_session(op):
    """Live-incident remediation: measure MTTR (elapsed instruction units in the
    kill event) and the observe→act round count."""
    with QemuKernel() as q:
        assert json.loads(q.stream.next_frame(timeout=60))["type"] == "hello"
        m = Mcp(q)
        m.result("initialize")
        m.result("tools/call", {"name": "run_suite", "arguments": {"suite": "e2"}})
        rounds = 0
        live_pid = None
        while True:
            e = json.loads(q.stream.next_frame(timeout=30))
            if e["type"] == "payload_start" and e["name"] == "livelock":
                live_pid = e["pid"]
            elif e["type"] == "tick" and live_pid is not None:
                rounds += 1
                # The operator observes an incident and decides to remediate.
                action = op.remediate({"runaway_pid": live_pid})
                if action:
                    r = m.result("tools/call", action)
                    return {"mttr_units": r.get("elapsed"), "rounds": rounds,
                            "status": r.get("status")}


def main():
    op = make_operator()
    print(f"agent-in-the-loop eval — operator: {op.name}"
          + ("" if op.name == "rule" else " (real model)") + "\n")

    e1 = e1_session(op)
    print("E1 — localization rate (root-cause facts recovered / needed):")
    for surface, (r, t) in e1.items():
        pct = 100 * r / t if t else 0
        print(f"  {surface:<8} {r:>3}/{t:<3} ({pct:4.0f}%)")
    ag = e1["agentic"][0] / e1["agentic"][1]
    cl = e1["classic"][0] / e1["classic"][1]
    print(f"  → the agentic surface lets the SAME operator localize "
          f"{ag / cl:.1f}x more root cause.\n")

    e2 = e2_session(op)
    print("E2 — live-incident remediation:")
    print(f"  mitigated in {e2['rounds']} observe→act round(s); "
          f"MTTR = {e2['mttr_units']} instruction units ({e2['status']}).")

    # Rule operator is deterministic, so these are stable; assert the headline.
    ok = ag > cl and e2["status"] == "killed"
    print("\n" + ("PASS — the loop closes on both surfaces; the structured "
                  "surface wins E1 and the operator remediates E2."
                  if ok else "INCOMPLETE — see above."))
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
