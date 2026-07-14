# A guided tour of kernai (no kernel background required)

This walks through everything that exists after milestones M0–M2, assuming
you know roughly what a program is but nothing about operating systems.
The interactive version is `make demo`.

## The one-paragraph idea

An operating-system kernel is the first real program a computer runs. It
owns the hardware and supervises every other program. Kernels normally talk
to humans: log lines, cryptic hex dumps, debuggers. **kernai** is an
experiment in building a kernel whose operator is an AI agent instead — so
everything it says is structured JSON, everything it does is reproducible
bit-for-bit, and its internal state is queryable instead of hidden. The full
pitch lives in `RFC-001-agent-native-kernel.md`; the twelve principles it
defines (P1–P12) are referenced all over this repo.

## The cast of characters

| Who | What it is | Analogy |
|-----|-----------|---------|
| **QEMU** | A program that emulates an entire RISC-V computer — CPU, memory, serial port — on your machine. | A computer inside your computer. |
| **OpenSBI** | Firmware that QEMU loads first. It initializes the machine, then hands control to our kernel and keeps serving low-level requests (like "print this byte"). | The BIOS. |
| **the kernel** (`kernel/`) | ~600 lines of Rust, no operating system underneath, no libraries. The only program running on the emulated machine. | The subject of the experiment. |
| **the harness** (`harness/`) | Python on your real machine. Boots QEMU, reads the kernel's events, sends it commands, asserts things. | The lab equipment. |
| **the serial port** | The single wire connecting the two worlds. Bytes in, bytes out — nothing else. | A phone line. |

## What happens when you run `make run`

1. QEMU creates the fake computer and loads two things into its memory:
   OpenSBI and our kernel (at address `0x80200000` — the address is baked
   into `kernel/link.ld`, the "floor plan" for where the kernel lives).
2. OpenSBI initializes the machine, prints its ASCII-art banner over the
   serial port, and jumps to the kernel's first instruction.
3. That first instruction is hand-written assembly (`kernel/src/hal/boot.rs`):
   it zeroes the kernel's uninitialized memory, points the stack somewhere
   sane, and only *then* calls the first Rust function, `kmain`. (Rust code
   can't run without a stack, and expects zeroed statics — someone has to
   bootstrap that, and it can only be assembly.)
4. `kmain` announces itself. This is the **hello frame**, event #0:

   ```json
   {"id": 0, "type": "hello", "name": "kernai", "proto": 0}
   ```

   Every kernel event is one JSON object wrapped in a tiny binary envelope
   (2 magic bytes + a length — `harness/framing.py` and
   `kernel/src/console.rs` are mirror images of each other). The magic
   bytes exist only so the harness can find the first frame after OpenSBI's
   banner noise.

5. The kernel asks the hardware for a **timer interrupt** every 10,000
   timebase units, then parks. From here on it is purely event-driven.

## Ticks: the heartbeat

An **interrupt** is the hardware tapping the CPU on the shoulder: the CPU
suspends whatever it was executing and jumps to the kernel's **trap
handler** ("trap" is the umbrella term for interrupts and faults). The
handler's first job is bookkeeping: it saves all 31 CPU registers so the
interrupted code can be resumed exactly as it was (`kernel/src/hal/trap.rs`
— this is the "trap frame save/restore" you'll see in commit messages).

Our handler services the timer, records the trap, and emits:

```json
{"id": 3, "type": "tick", "seq": 3, "time": 248228}
```

Two things worth staring at:

- **`time` is not wall-clock time.** QEMU runs with `-icount`, which derives
  the virtual clock from *how many instructions have executed*. The kernel
  re-arms each tick from the previous deadline, so ticks land exactly 10,000
  timebase units — 500,000 instructions — apart. Run it twice, get identical
  numbers —
  `make test` literally asserts two boots produce byte-identical streams.
  (Principle P9: agents debug by re-running, so nothing may be random.)
- **`id` increases across all event types.** The event stream is the
  kernel's diary, and the ids are its page numbers.

## The ring buffer: a kernel you can question

The kernel keeps its last 8 traps in a ring buffer, and you can ask for it
at any time by sending one byte, `r`, over the serial port:

```json
{"id": 6, "type": "trap_ring", "count": 5, "entries": [
  {"id": 1, "cause": "timer", "sepc": "0x80200232"},
  ...
]}
```

`sepc` is the address of the instruction that was interrupted. This is
deliberately the opposite of how kernels usually work, where seeing this
would require attaching a debugger (principle P11: *no kernel state
observable only via a debugger*). Today it's one ring buffer; the plan is
that all kernel state becomes queryable resources like this.

## The crash: a fault report designed for an agent

Send `x` and the kernel deliberately executes a forbidden instruction —
writing to the CPU's read-only cycle counter. The CPU refuses and traps.
Instead of hanging or dumping hex, the handler emits the project's first
**diagnostic frame** (principle P6: *errors are prompts*):

```json
{"id": 8, "type": "fault",
 "cause": "0x2", "cause_name": "illegal_instruction",
 "sepc": "0x80200568",          // address of the guilty instruction
 "stval": "0xc0001073",         // its raw bits, straight from the CPU
 "ra": "0x8020045c",            // where it was called from
 "sp": "0x80211b40",            // the stack pointer at that moment
 "insn": {                      // the bits, decoded for you:
   "bits": "0xc0001073",
   "opcode": "0x73",            //   a SYSTEM instruction
   "funct3": 1, "rd": 0, "rs1": 0, "rs2": 0, "funct7": "0x60",
   "csr": "0xc00"               //   targeting CSR 0xc00 = the read-only
 },                             //   cycle counter — i.e. HERE'S WHY
 "ring": [ ...last 8 traps... ] // the flight recorder
}
```

The design goal (measured properly at milestone M9/E1): an agent reading
this frame — and nothing else — should be able to localize the bug. Then the
kernel shuts the machine down cleanly. A fault must never be a hang.

## Running programs *under* the kernel (M3)

Everything so far was the kernel itself. Now for the point of an operating
system: running other programs. Send `p` and the kernel loads two small
**user programs** — separately compiled binaries under `payloads/` — and runs
each in **user mode**, the CPU's unprivileged level. A user-mode program
can't touch hardware; to do anything real it makes a **system call**, asking
the kernel (via the `ecall` instruction, the same mechanism the kernel uses
to call OpenSBI).

```json
{"id":7,"type":"payload_start","pid":0,"name":"hello","entry":"0x80400000","caps":["write"]}
{"id":9,"type":"payload_output","pid":0,"untrusted":true,"len":20,"data":"hello from userspace"}
{"id":11,"type":"payload_exit","pid":0,"code":0}
```

Three things:

- **The kernel loaded an ELF file** (the standard executable format) into a
  dedicated region of memory (the "arena", at `0x80400000`), then dropped to
  user mode to run it. `payload_start` announces this; `caps` is the program's
  **capability set** — the exact list of privileged operations it's allowed
  to request. This one may `write`, nothing else. (Isolation is at three
  levels: *privilege* — a payload can't run privileged instructions;
  *fault* — a crash is contained; and, since M5, *memory* — the MMU gives
  each payload its own page table so it physically cannot read the kernel's
  or another payload's memory. See the M5 section below.)
- **Output is tagged `"untrusted": true`.** Bytes coming *from* a workload are
  data, never instructions — the kernel confines them to a JSON string and
  never lets them be read as commands by itself or its operator (principle P7:
  the kernel defends its operator from being social-engineered by the
  workload). There's also a length cap so a hostile program can't flood the
  operator's attention.
- **`sys_exit` ends the program.** `pid` (process id) distinguishes programs.

The second program (`crasher`) deliberately executes an instruction only the
kernel is allowed to run. It faults — and the kernel emits the *same*
structured report as before, but with `"origin": "payload"`, and **kills only
that program**. The kernel keeps running. A user program crashing is an event,
not a catastrophe (principle P1). `suite_done` then summarizes.

## The sandbox: capabilities, delegation, deadlines (M4)

Send `m` for four programs that show the kernel enforcing policy on its own,
with no human in the loop:

- **A muzzled program** (empty capability set) tries to `write`. Refused:
  `{"type":"syscall_denied","syscall":"write","reason":"missing_cap"}`. The
  refusal is a *structured event* the operator can see — not a silent error
  the program could hide. A program can only do what it was granted.
- **A spawner** creates a child program and tries to hand it the `write` and
  `yield` capabilities. But the spawner itself doesn't hold `yield`, so the
  kernel **attenuates** the grant — the child gets only `write`:
  `{"type":"payload_spawn","requested":["write","yield"],"granted":["write"],"attenuated":true}`.
  A program can never delegate power it doesn't have; a chain of hand-offs can
  only ever shrink (principle P10), and the kernel enforces this, not
  convention.
- **A runaway** loops forever on purpose. The kernel gave it an instruction
  budget when it started; when the timer notices it has blown past that
  budget, it kills it: `{"type":"payload_killed","reason":"deadline"}`. A
  workload can't wedge the machine by spinning. Because the budget is measured
  in *instructions*, not wall-clock time, the kill lands at the exact same
  point every run.

That is the whole thesis in miniature: the mechanism (traps, privilege,
memory fencing) is boring and autonomous; every *policy* decision — grant,
deny, attenuate, kill — is an observable structured event, which is exactly
what an AI operator needs to supervise the system.

## Memory isolation (M5)

Send `i`. The CPU has a **memory management unit** (MMU) that can translate
every address a program uses through a **page table** — a per-program map
from "virtual" addresses the program sees to real physical memory. The
kernel now builds a private page table for each payload: its own code and
data, plus the kernel's memory marked *supervisor-only*. Two fixtures probe
the walls:

- **`wild`** reads kernel address `0x80200000`. It gets a `load_page_fault`,
  and the fault frame includes a **page-table walk**: the kernel's page is
  present but `"u": 0` (supervisor-only), so a user program touching it
  faults. Before M5 there was no MMU and this read would have succeeded —
  that was the memory-isolation gap M5 closes.
- **`wxviol`** tries to overwrite its own code. It gets a `store_page_fault`;
  the walk's leaf shows the code page is `"x": 1` (executable) but `"w": 0`
  (not writable). This is **W^X** (write-xor-execute): code can't be
  rewritten, data can't be executed — a whole class of exploits made
  structurally impossible.

Both faults kill only the offending payload; a clean payload runs afterward
in its own fresh address space. The page-table walk in the fault frame is
the beginning of the RFC's "errors are prompts" goal (P6): the report tells
the operator not just *that* it faulted but *why*, down to the permission
bit.

## Try it yourself

```sh
make demo    # the narrated version of everything above (M0-M4)
make run     # raw boot; then press: r (ring) x (crash) p (M3) m (M4) i (M5)
make test    # the full acceptance gate (framing, boot, traps, payloads,
             # sandbox, hardening, determinism, demo) — a few seconds
make debug   # boot frozen at the first instruction, gdb stub listening
make gdb     # (second terminal) attach; try: break kernai::kmain, continue
```

## Glossary

- **RISC-V** — an open CPU instruction set; `rv64` = its 64-bit variant.
- **S-mode / M-mode / U-mode** — CPU privilege levels. OpenSBI runs in
  M(achine) mode, the kernel in S(upervisor) mode, user programs (payloads)
  in U(ser) mode. Each level can't touch the ones above it — that boundary is
  what makes a crashing payload survivable.
- **payload** — a user program the kernel runs (in `payloads/`). A **capability**
  is a token granting one privileged operation (`write`/`yield`/`spawn`); a
  payload's **CapSet** is all it holds. **Attenuation** = granting a subset.
- **syscall** — a payload's request to the kernel (`ecall`): exit, write,
  yield, spawn. The kernel's side is `kernel/src/syscall.rs`.
- **SBI** — the "system call" interface the kernel uses to ask OpenSBI for
  things (print a byte, set the timer, shut down). Made via the `ecall`
  instruction.
- **trap** — any transfer of control into the kernel: interrupts (timer) and
  exceptions (illegal instruction, page fault...).
- **CSR** — control-and-status register; the CPU's own settings registers.
  `scause`/`sepc`/`stval` are CSRs that describe the current trap.
- **ELF** — the executable file format; `kernel/target/.../kernai` is one.
- **`no_std` Rust** — Rust without its standard library: no heap, no files,
  no threads. Everything the kernel uses, it builds itself.
- **unsafe budget** — Rust's `unsafe` keyword disables safety checking;
  here it's quarantined in `kernel/src/hal/` and capped (≤4 files, ≤200
  lines, every use justified) by `ci/unsafe_budget.sh`.
