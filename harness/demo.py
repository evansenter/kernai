"""`make demo` — a narrated tour of everything the kernel can do so far,
written for someone who has never touched a kernel. Run with --fast to skip
the dramatic pauses (the acceptance suite does).
"""

import json
import subprocess
import sys
import time

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

    say("[1/5] Booting an emulated RISC-V computer...")
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

        say("[2/5] Watching the kernel's heartbeat (timer interrupts)...")
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

        say("[3/5] Asking the kernel what happened recently (introspection)...")
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

        say("[4/7] Running a user program under the kernel...")
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

        say("[5/7] The sandbox: capabilities, delegation, and runaway control...")
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

        say("[6/7] Deliberately crashing the kernel itself (the good part)...")
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

    say("[7/7] Proving determinism (run it again, get identical bytes)...")
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
That's M0-M4: a kernel that boots, narrates itself in structured events,
runs sandboxed user programs, enforces capabilities and delegation, kills
runaways on an instruction budget, and reproduces byte-for-byte every run.
Next up (M5): giving each program its own private memory map (paging), so
several can be resident at once. See docs/WALKTHROUGH.md for the full guided
tour and docs/RFC-001-agent-native-kernel.md for where this is going.
""")


if __name__ == "__main__":
    if not KERNEL_ELF.exists():
        print("kernel not built — run `make build` first", file=sys.stderr)
        sys.exit(1)
    main()
