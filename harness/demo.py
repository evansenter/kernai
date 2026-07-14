"""`make demo` — a narrated tour of everything the kernel can do so far,
written for someone who has never touched a kernel. Run with --fast to skip
the dramatic pauses (the acceptance suite does).
"""

import json
import subprocess
import sys
import time

from .mcp import Mcp
from .qemu import KERNEL_ELF, QemuKernel

FAST = "--fast" in sys.argv
TTY = sys.stdout.isatty()

BOLD = "\033[1m" if TTY else ""
DIM = "\033[2m" if TTY else ""
CYAN = "\033[36m" if TTY else ""
RESET = "\033[0m" if TTY else ""


def say(text=""):
    print(f"{BOLD}{text}{RESET}")
    sys.stdout.flush()
    if not FAST:
        time.sleep(0.06 * min(len(text), 20))


def show(evt):
    print(f"{CYAN}    {json.dumps(evt)}{RESET}")
    sys.stdout.flush()


def note(text):
    for line in text.strip("\n").splitlines():
        print(f"{DIM}    {line}{RESET}")
    sys.stdout.flush()
    if not FAST:
        time.sleep(0.8)


def get(q, evt_type, timeout=60):
    deadline = time.monotonic() + timeout  # bounds the whole wait
    while True:
        remaining = deadline - time.monotonic()
        assert remaining > 0, f"no {evt_type} event within {timeout}s"
        evt = q.next_event(remaining)
        assert evt is not None, "kernel exited unexpectedly"
        if evt["type"] == evt_type:
            return evt


def main():
    say()
    say("kernai demo — a tiny operating-system kernel you can talk to")
    note("""
A kernel is the first real program a computer runs: it owns the hardware and
supervises everything else. This one is deliberately tiny, runs on an
emulated RISC-V computer (QEMU), and reports everything it does as
machine-readable JSON events instead of human log lines — because its
intended operator is an AI agent, not a person at a terminal.
""")

    say("[1/11] Booting an emulated RISC-V computer...")
    note("""
QEMU emulates the whole machine: CPU, memory, a serial port (think: a wire
for bytes). A small piece of firmware called OpenSBI (the machine's
"BIOS") starts first, prints a text banner, and then hands control to our
kernel. The harness skips the banner and locks onto the kernel's framed
events.
""")
    with QemuKernel() as q:
        hello = get(q, "hello")
        say("The kernel's first words — event #0:")
        show(hello)
        note("""
Every event is JSON with a globally increasing "id". The hello frame is the
kernel saying "I'm alive, I speak protocol 0".
""")

        say("[2/11] Watching the kernel's heartbeat (timer interrupts)...")
        note("""
The kernel asked the hardware for a timer that fires every 10,000 timebase
units — under deterministic emulation that is exactly every 500,000 CPU
instructions, never wall-clock time. Each time it fires, the CPU drops
whatever it's doing and jumps into the kernel's trap handler ("trap" =
any event that interrupts normal execution), which records it and emits a
tick event.
""")
        ticks = [get(q, "tick") for _ in range(3)]
        for t in ticks:
            show(t)
        note("""
"seq" counts ticks; "time" is the timebase clock. Note the ids and times
increase in lockstep — the event stream is the kernel's diary.
""")

        say("[3/11] Asking the kernel what happened recently (introspection)...")
        note("""
Classic kernels keep their internal state hidden — you attach a debugger to
see it. This kernel's design principle P11 says: no state observable only
via debugger. So it keeps a ring buffer of the last 8 traps, and we can
query it live by sending a single byte, 'r', over the serial port:
""")
        q.send(b"r")
        ring = get(q, "trap_ring")
        show(ring)
        note("""
"count" is every trap since boot; "entries" are the most recent ones —
all timer ticks so far, each with the program counter ("sepc") where the
CPU was interrupted. This is the seed of the kernel-as-queryable-database
idea the whole project is built around.
""")

        say("[4/11] Running a user program under the kernel...")
        note("""
So far everything has been the kernel itself. Now we send 'p' to load two
small *user* programs — separate compiled binaries — and run them in
"user mode", the CPU's unprivileged level where a program can't touch the
hardware directly. It has to ask the kernel, via a "system call". The first
program prints a line and exits; the second deliberately misbehaves.
""")
        q.send(b"p")
        p_start = get(q, "payload_start")
        show(p_start)
        note("""
payload_start: the kernel loaded the "hello" program from an ELF file (the
standard executable format), placed it in a fenced-off memory region, and
dropped to user mode to run it. "caps" is its capability set — the exact
list of privileged things it's allowed to ask for. This one may "write".
""")
        show(get(q, "payload_output"))
        note("""
The program asked the kernel to print for it. Crucially the output is tagged
"untrusted": true. The kernel treats bytes coming FROM a workload as data,
never as instructions to itself or its operator (principle P7 — the kernel
defends its operator from being social-engineered by the workload).
""")
        show(get(q, "payload_exit"))
        # The second payload faults; catch its report.
        get(q, "payload_start")
        get(q, "payload_output")
        pfault = get(q, "fault")
        show(pfault)
        note("""
The second program tried to do something only the kernel is allowed to do
(touch a privileged register). Watch what happened: origin is "payload", so
the kernel produced the SAME structured fault report as before — but instead
of shutting down, it killed just that one program and kept running. A
user program crashing is an event, not a catastrophe (principle P1).
""")
        show(get(q, "suite_done"))
        note('suite_done: one program exited cleanly, one faulted. The kernel is fine.')

        say("[5/11] The sandbox: capabilities, delegation, and runaway control...")
        note("""
Send 'm' for four more programs that show the kernel enforcing policy:
  • a "muzzled" program with NO capabilities tries to print — denied
  • a "spawner" tries to create a child program, handing it capabilities
  • a "runaway" program loops forever on purpose
Watch the kernel handle each without a human intervening.
""")
        q.send(b"m")
        events = []
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            e = q.next_event(60)
            assert e is not None, "kernel exited during M4 suite"
            if e["type"] == "tick":
                continue
            events.append(e)
            if e["type"] == "suite_done":
                break
        denied = next(e for e in events if e["type"] == "syscall_denied" and e["syscall"] == "write")
        show(denied)
        note("""
The muzzled program's print was refused: ENOCAP. The refusal is itself a
structured event ("syscall_denied") the operator can see — not a silent
error code the program could hide. Capabilities are the security substrate:
a program can only do what it was explicitly granted.
""")
        spawn = next(e for e in events if e["type"] == "payload_spawn")
        show(spawn)
        note("""
The spawner asked to give its child the "write" AND "yield" capabilities —
but the spawner itself doesn't hold "yield". So the kernel *attenuated* the
grant: the child got only "write" ("attenuated": true). A program can never
hand out power it doesn't have; a chain of delegations can only ever shrink
(principle P10). This is enforced by the kernel, not by good manners.
""")
        killed = next(e for e in events if e["type"] == "payload_killed")
        show(killed)
        note("""
And the runaway that looped forever? The kernel gave it an instruction
budget when it started; when the timer noticed it had blown past that
budget, it killed it — "reason": "deadline". A workload can't wedge the
machine by spinning. Because time here is measured in instructions, not
the wall clock, this kill happens at the EXACT same point every single run.
""")

        say("[6/11] Memory isolation: each program gets its own private memory...")
        note("""
Send 'i'. Until now, nothing physically stopped a user program from reaching
into the kernel's memory — there was no memory management unit (MMU) turned
on. Now each program runs in its own "address space": a private map (a page
table) the CPU's MMU enforces. Two programs:
  • "wild" tries to read the kernel's memory
  • "wxviol" tries to overwrite its own code
""")
        q.send(b"i")
        iso = []
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            e = q.next_event(30)
            assert e is not None, "kernel exited during M5 suite"
            if e["type"] == "tick":
                continue
            iso.append(e)
            if e["type"] == "suite_done":
                break
        wild = next(e for e in iso if e["type"] == "fault" and e["pid"] == 0)
        show({k: wild[k] for k in ("type", "origin", "cause_name", "stval", "pagewalk")})
        note("""
"wild" reached for kernel address 0x80200000 and got a load_page_fault. Look
at "pagewalk" — the kernel's page is present but "u": 0, meaning
supervisor-only. A user program touching it faults. The kernel is now
genuinely walled off from the programs it runs (the isolation the earlier
milestones deferred to here).
""")
        wx = next(e for e in iso if e["type"] == "fault" and e["pid"] == 1)
        show({k: wx[k] for k in ("type", "cause_name", "pagewalk")})
        note("""
"wxviol" tried to write to its own code and got a store_page_fault. The
pagewalk's last entry shows the code page is "x": 1 (executable) but "w": 0
(not writable) — "W^X", write-XOR-execute. Code can't be rewritten and data
can't be run; a whole class of exploits is structurally impossible. Both
faults killed only the offending program; the kernel ran a clean program
right after, in its own fresh address space.
""")

        say("[7/11] Checkpoint and fork: saving and branching a running program...")
        note("""
Send 'f'. A program called "forker" prints a line, then asks the kernel to
*checkpoint* it — freeze its entire state (memory + registers). It keeps
running (the "original" branch) and exits. Then the kernel does something a
normal OS can't do cheaply: it *forks* that checkpoint into two brand-new
programs, each resuming from the frozen point — like save-scumming a video
game, or exploring two futures from one decision.
""")
        q.send(b"f")
        chk = []
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            e = q.next_event(30)
            assert e is not None, "kernel exited during M6 suite"
            if e["type"] == "tick":
                continue
            chk.append(e)
            if e["type"] == "suite_done":
                break
        show(next(e for e in chk if e["type"] == "snapshot"))
        note("""
"snapshot": the kernel deep-copied the program's private memory and saved its
registers. Now watch the outputs. The line "before the checkpoint" was
printed once — by the original, before the snapshot. Each forked continuation
resumes AFTER that point, so it never reprints it:
""")
        for o in chk:
            if o["type"] == "payload_output":
                tag = "restored" if any(
                    s["pid"] == o["pid"] and s.get("restored")
                    for s in chk if s["type"] == "payload_start") else "original"
                show({"pid": o["pid"], "branch": tag, "data": o["data"]})
        note("""
Two independent continuations (pid 1 and pid 2) each resumed from the exact
checkpoint and ran the post-snapshot code — but "before the checkpoint"
appears only once. The snapshot call returned a non-zero id to the original
and zero to each fork, so a program can tell which future it's in (exactly
like Unix fork()). This is the substrate for an agent doing tree search:
check point, try a branch, and if it's bad, fork the checkpoint again and
try another. It's cheap here because a unikernel owns the whole memory map.
""")

        say("[8/11] Driving the kernel as an MCP server (structured control)...")
        note("""
Everything so far used single command bytes. But the kernel's real control
plane (principle P4) speaks MCP — the same JSON-RPC protocol AI agents already
use for tools — carried inside those same framed messages. Any agent SDK can
drive it with zero glue. We do a normal MCP handshake, then ask it to describe
itself and to report what has run.
""")
        mcp = Mcp(q)
        init = mcp.result("initialize")
        show({"serverInfo": init["serverInfo"], "protocolVersion": init["protocolVersion"]})
        note("""
initialize: the standard MCP handshake. The kernel identifies as an MCP server
named "kernai". From here a generic MCP client knows how to talk to it.
""")
        tools = mcp.result("tools/list")
        show({"tools": [t["name"] for t in tools["tools"]]})
        note("""
tools/list: the control operations, discoverable — not documented in a manual
that drifts. "run_suite" runs a workload set, "crash" faults the kernel,
"ring_read" reads the flight recorder. An agent learns the surface by asking.
""")
        spec = mcp.result("resources/read", {"uri": "spec"})
        show({"syscalls": [s["name"] for s in spec["syscalls"]],
              "caps": [c["name"] for c in spec["caps"]], "memory": spec["memory"]})
        note("""
The "spec" resource is the kernel describing its own ABI (principle P5): its
system-call table, its capability lattice, its memory map. No SPEC.md to fall
out of sync — the contract is queryable, live, from the kernel itself.
""")
        procs = mcp.result("resources/read", {"uri": "processes"})
        show({"processes": [{"name": p["name"], "state": p["state"]} for p in procs["processes"]]})
        note("""
And the "processes" resource is the kernel's own view of every workload from
the earlier acts and how each ended — exited, faulted, or killed. The kernel's
internal state IS the operator's API (principle P11): no debugger required.
""")

        say("[9/11] The same fault, two surfaces (why the format matters)...")
        note("""
The whole thesis is that a kernel's *output format* — not the agent — decides
how debuggable it is (principle P6). To make that measurable, the kernel can
render a fault two ways, and you flip between them over the control plane. First
the "agentic" surface: run the crashing program again and look at the fault.
""")

        def run_suite_collect(suite):
            mcp.result("tools/call", {"name": "run_suite", "arguments": {"suite": suite}})
            evs, flt = [], None
            while True:
                e = q.next_event(30)
                assert e is not None, "kernel exited during suite"
                evs.append(e)
                if e["type"] == "fault":
                    flt = e
                if e["type"] == "suite_done":
                    return evs, flt

        evs, flt = run_suite_collect("p")
        crasher = next(e for e in evs if e["type"] == "payload_start" and e["name"] == "crasher")
        show({"type": "fault", "cause_name": flt["cause_name"], "caused_by": flt["caused_by"],
              "regs(sample)": {k: flt["regs"][k] for k in ("ra", "sp", "a0", "a7")},
              "regs(count)": len(flt["regs"])})
        note(f"""
The agentic fault frame carries the FULL register file (all 31 of them, ABI
named) and a "caused_by": {flt['caused_by']} — the id of the payload_start event
({crasher['id']}) this fault descends from. That "caused_by" edge (principle
P12) turns the event log into a causal graph: an agent can walk from the fault
back to exactly which program run, spawned by which parent, triggered it — no
guessing, no debugger.
""")
        mcp.result("tools/call", {"name": "set_surface", "arguments": {"mode": "classic"}})
        note("""
Now flip to the "classic" surface — the way a traditional kernel talks: one
dense printf line, no structure. Same fault, same facts, worse format. Run the
crashing program once more:
""")
        run_suite_collect("p")
        noise = bytes(q.stream.decoder.noise).decode(errors="replace")
        classic = [ln for ln in noise.splitlines() if "[FAULT]" in ln][-1]
        print(f"{CYAN}    {classic}{RESET}")
        note("""
That's it — no register file, no page-table walk, no ring history, no causal
parent, not even a machine-readable frame (it arrives as raw console noise, like
a real kernel log). This is the A/B for the project's headline experiment (E1):
give a fixed agent only the frames on one arm and only lines like this on the
other, and measure how much faster it localizes the bug. The kernel, not the
agent, is the variable.
""")
        mcp.result("tools/call", {"name": "set_surface", "arguments": {"mode": "agentic"}})

        say("[10/11] Deliberately crashing the kernel itself (the good part)...")
        note("""
Now we send 'x', which tells the kernel to execute an instruction that is
forbidden by the CPU spec: writing to the read-only 'cycle' counter
register. A normal kernel would print a hex dump and hang. Ours must emit
a structured fault report designed so that an AI agent can localize the
bug from the report alone (principle P6) — then shut down cleanly.
""")
        q.send(b"x")
        fault = get(q, "fault")
        show(fault)
        note("""
Reading it like the kernel does:
  cause/cause_name  what kind of trap: an illegal instruction
  sepc              the exact address of the offending instruction
  stval             the instruction's raw bits, straight from the CPU
  insn              those bits decoded: opcode 0x73 = a SYSTEM instruction,
                    and csr 0xc00 is the read-only cycle counter — i.e. the
                    report doesn't just say "illegal", it shows you WHY
  ra/sp             return address + stack pointer at the moment of the trap
  ring              the last 8 traps before death — the flight recorder
""")
        try:
            q.wait_exit(30)
            say("The kernel reported the fault and shut the machine down — no hang.")
        except subprocess.TimeoutExpired:
            raise AssertionError("kernel hung after fault report")

    say("[11/11] Proving determinism (run it again, get identical bytes)...")
    note("""
The emulator is configured so virtual time is computed from the instruction
count (-icount), not the host clock. Same program + same inputs = the same
events, byte for byte, every single run. That's principle P9: any bug an
agent ever sees is exactly reproducible.
""")
    def capture():
        frames = []
        with QemuKernel() as q2:
            while len(frames) < 4:
                frame = q2.stream.next_frame(timeout=60)
                assert frame is not None, f"EOF during capture; stderr: {q2.stderr_tail()}"
                frames.append(frame)
        return frames

    a, b = capture(), capture()
    for fa, fb in zip(a, b):
        assert fa == fb, "boots diverged — determinism broken!"
        show(json.loads(fa))
    say(f"Two fresh boots produced identical streams ({len(a)} frames compared).")
    note("""
That's M0-M10: a kernel that boots, narrates itself in structured events, runs
sandboxed user programs in private address spaces (paging + W^X), enforces
capabilities and multi-hop delegation (grants only ever shrink down a chain),
kills runaways on an instruction budget,
checkpoints and forks a running program, replays a recorded session
byte-for-byte, is driven as a self-describing MCP server, and reports faults as
rich causal diagnostic frames with a classic printf twin for measuring the
difference. Next up (M10-M12): hardening delegation, a token-budgeted autonomy
dial, and the E1-E8 evaluation suite that scores kernel surfaces against a
fixed agent. See docs/WALKTHROUGH.md for the full guided tour and
docs/RFC-001-agent-native-kernel.md for where this is going.
""")


if __name__ == "__main__":
    if not KERNEL_ELF.exists():
        print("kernel not built — run `make build` first", file=sys.stderr)
        sys.exit(1)
    main()
