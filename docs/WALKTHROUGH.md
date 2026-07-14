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
{"id": 3, "type": "tick", "seq": 3, "time": 248217}
```

Two things worth staring at:

- **`time` is not wall-clock time.** QEMU runs with `-icount`, which derives
  the virtual clock from *how many instructions have executed*. Ticks fire
  every 500,000 instructions, exactly. Run it twice, get identical numbers —
  `make test` literally asserts two boots produce byte-identical streams.
  (Principle P9: agents debug by re-running, so nothing may be random.)
- **`id` increases across all event types.** The event stream is the
  kernel's diary, and the ids are its page numbers.

## The ring buffer: a kernel you can question

The kernel keeps its last 8 traps in a ring buffer, and you can ask for it
at any time by sending one byte, `r`, over the serial port:

```json
{"id": 5, "type": "trap_ring", "count": 4, "entries": [
  {"id": 1, "cause": "timer", "sepc": "0x8020020e"},
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
{"id": 7, "type": "fault",
 "cause": "0x2", "cause_name": "illegal_instruction",
 "sepc": "0x80200aa2",          // address of the guilty instruction
 "stval": "0xc0001073",         // its raw bits, straight from the CPU
 "ra": "0x80200528",            // where it was called from
 "sp": "0x802122a0",            // the stack pointer at that moment
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

## Try it yourself

```sh
make demo    # the narrated version of everything above
make run     # raw boot: OpenSBI banner, then framed events; press r / x
make test    # the full acceptance gate (framing, boot, traps, hardening,
             # determinism, demo) — ~2 seconds, 8 QEMU boots
make debug   # boot frozen at the first instruction, gdb stub listening
make gdb     # (second terminal) attach; try: break kernai::kmain, continue
```

## Glossary

- **RISC-V** — an open CPU instruction set; `rv64` = its 64-bit variant.
- **S-mode / M-mode** — CPU privilege levels. OpenSBI runs in M(achine)
  mode, the kernel in S(upervisor) mode; user programs (from M3 on) will run
  in U(ser) mode. Each level can't touch the ones above it.
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
