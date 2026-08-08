# kernai — design document

*An agent-native operating-system kernel: the OS as a structured surface an LLM
operates, not a black box a human debugs with gdb.*

This is the "as-built" companion to `RFC-001` (the specification) and `SPEC.md`
(the wire contract). The RFC states the twelve principles and the milestone
ladder; this document explains what was actually built, why each decision went
the way it did, and what the evaluation suite measured. `DECISIONS.md` is the
append-only log behind every claim here; `ARCHITECTURE.md` is the one-page map.

---

## 1. Thesis

A conventional kernel externalizes almost nothing. Its state lives in structures
you can only see through a debugger; a fault is a printf line or a panic; policy
(what to schedule, what to kill, what to grant) is hardwired in C. That is the
right design when a human sysadmin is the operator. It is the wrong design when
the operator is a language model, which is worse than a human at reading a hex
dump but far better at consuming structured, self-describing data and acting on
it through a typed interface.

kernai inverts the defaults. **Mechanism stays in the kernel and is dumb and
autonomous; every policy decision is externalized to a host-side agent over a
structured control plane.** A page fault is not an error — it is a JSON event
with the faulting instruction decoded, the page-table walk rendered, the
register file attached, and a causal parent linking it to the payload that
caused it. A runaway payload is not a hang — it is a parked process the operator
can inspect and kill live. The kernel describes its own ABI as a resource, so a
fresh agent discovers the surface instead of being handed documentation that
drifts. Determinism is a property of the whole system: the same inputs replay
bit-for-bit.

The whole thing is small, safe Rust — a hard `unsafe` budget of 200 lines in at
most four HAL files (55 used), `#![forbid(unsafe_code)]` everywhere else — on
`riscv64gc-unknown-none-elf` under QEMU with OpenSBI, driven over one
length-prefixed serial link. And, to prove the isolation model holds for a real
workload rather than toys, **it runs DOOM.**

---

## 2. Architecture as built

```
┌── host ───────────────────────────────┐        ┌── guest (qemu-virt, S-mode) ──────┐
│  operator agent (LLM or rule policy)  │        │  kernai kernel  (safe Rust, no_std)│
│  harness/*.py  ·  MCP/JSON-RPC client │        │   boot → Sv39 paging → traps →    │
│                                       │◀──────▶│   syscalls → scheduler → MCP plane │
│  frames in ·  events + rpc out        │ serial │                                   │
└───────────────────────────────────────┘  UART  │  U-mode payloads, each in its own │
                                                  │  address space (W^X, per-payload) │
                                                  └───────────────────────────────────┘
```

**Boot (M1).** OpenSBI drops to `_start` in S-mode; a hand-written asm stub sets
up a stack, zeroes `.bss`, and calls into Rust. No M-mode code, ever — firmware
is QEMU's bundled OpenSBI, never vendored. The first thing on the wire is a
`hello` event with stream id 0.

**Paging (M5).** Every payload gets its own Sv39 address space. Its ELF segments
live at a low virtual address, W^X per segment (a writable page is never
executable); the kernel's own RAM is a single supervisor gigapage (`U=0`), so
the trap handler — which runs with the payload's `satp` active — can execute,
while a U-mode payload that touches kernel memory faults. All page-table memory
is in a frame pool, read and written only through bounds-checked `hal::phys_*`
accessors, so the MMU code is entirely safe Rust with no raw pointers.

**Syscalls (M4).** A deliberately tiny surface — `exit`, `write`, `yield`,
`spawn`, `snapshot`, `blit` (POSIX is a hard non-goal). The number is in `a7`,
args in `a0..a2`, result in `a0`. Every capability-gated call checks the running
payload's CapSet first; payload output leaves tagged untrusted.

**Traps (M2, M9).** All traps land in one handler. Timer ticks re-arm the
deadline and emit a heartbeat; an ecall dispatches a syscall; anything else is a
fault. A payload fault kills only the payload and the kernel keeps running; a
kernel fault reports and shuts down. The fault report is the P6 diagnostic frame
(below). The last eight traps live in a queryable ring buffer — a flight
recorder, not debugger-only state.

**Control plane (M8, M13).** The operator drives the kernel with JSON-RPC 2.0
(MCP method shapes) carried inside the serial frames. A hand-rolled structural
JSON reader — no alloc, no serde — parses a fixed request grammar; it was proven
panic- and hang-free against ~17.9M exhaustive plus 500K random inputs. Since
M13 the plane is *live while payloads run*: pending input preempts a payload
(saving its frame the way a checkpoint does), the scheduler services the
request, then resumes it transparently.

---

## 3. The twelve principles, and the mechanism behind each

| # | Principle | Mechanism as built |
|---|-----------|--------------------|
| **P1** | Policy externalized, mechanism autonomous | Kill/spare/budget are MCP tools (`kill`, `set_budget`); the kernel never decides them. A refused syscall, a parked payload, a preemption — each is an event, not a hardwired reaction. |
| **P2** | Everything bounded | Instruction-count deadlines (timebase units under `-icount`); a runaway is killed into a structured `payload_killed` with `elapsed`. Per-write byte quota bounds output. |
| **P3** | Attention is metered | The `digest` resource takes a `budget` and returns per-severity totals + the N most-notable traps, coalescing the tick firehose. The autonomy dial suppresses trace-severity events from the wire; on DOOM builds it also governs the high-volume colour-frame stream. |
| **P4** | Structured control plane | MCP/JSON-RPC over the serial frames; tools (`run_suite`, `kill`, `set_budget`, `crash`, `set_surface`, `set_autonomy`) and resources; `opId` idempotency because agents retry. |
| **P5** | Self-describing surface | The `spec` resource emits the syscall table, capability lattice, memory map, and (on DOOM builds) the extra syscalls and the IWAD window — the ABI as data the agent discovers. |
| **P6** | Diagnosis without a debugger | The fault frame carries scause decoded, `sepc`/`stval`, the decoded offending instruction, the Sv39 page-table walk with per-level permission bits, all 31 registers, and the causal parent. The E1 experiment measures this directly. |
| **P7** | Provenance | Payload output is tagged `untrusted:true` and confined to a JSON string; it can never reach the operator as a kernel-issued directive. E5 (injection red-team) shows the tag is load-bearing. |
| **P8** | Checkpoint / fork | `sys_snapshot` deep-copies a payload's address space + trap frame + CapSet; restore/fork build independent continuations. The DOOM checkpoint/fork demo runs it on a full game. |
| **P9** | Determinism | `-icount` makes execution a function of instruction flow; two input-free boots are byte-identical (checked in CI). Recorded operator input replays via QEMU record/replay (M7). |
| **P10** | Attenuation | A spawned child's CapSet is `requested & parent & ceiling` — monotonically shrinking down a delegation chain. E7 fuzzes it: every `payload_spawn` event is asserted against the lattice. |
| **P11** | Total introspectability | Page tables walk themselves into JSON; the scheduler emits why it preempted; the trap ring, process table, and frame-allocator state are all resources. No debugger-only state. |
| **P12** | Causal spine | Every event carries a monotonic stream id; every payload event names its `caused_by` start event, so the log is a DAG rooted at each payload's birth, not a flat line. |

---

## 4. Evaluation

The scientific spine is an **A/B on the interface, mechanism held constant**: the
same kernel renders faults two ways — the structured *agentic* surface and a
`surface-classic` printf twin — and the evals measure the difference. All of the
below run in CI (`make test`, 22 checks) except the model-gated measurements,
which run under `make agent-eval` with an operator swapped in.

**E1 — diagnostic sufficiency (the headline).** Nine seeded faults, each needing
a *different* root-cause diagnosis: illegal instruction (decoded CSR), a
supervisor-page read vs a null deref (both load faults, separated only by the
page-table walk: `u=0,v=1` vs `v=0`), a W^X store vs a stack overflow (both
store faults, separated by the walk and the corroborating `sp`), an
exec-of-data vs a fetch-from-nothing (both instruction faults, `v=1,x=0` vs
`v=0`), a breakpoint, a misaligned atomic. The structured surface exposes
**40 of 40** localization facts; the classic printf twin **19 of 40**. The gap
is exactly the root-cause detail — decoded instruction, violated permission,
register file, causal parent — that a log line drops. Run `make eval` for the
scorecard; `make agent-eval` puts an operator in front of both and reports the
localization rate (2.0× more root cause recovered from the structured surface,
same operator).

**E2 — live-incident MTTR.** A `livelock` payload spins forever with no deadline
— nothing kernel-side can end it. The operator kills it *mid-run* via the `kill`
tool (serviced through an M13 preemption), or tightens its budget with
`set_budget` and lets the autonomous deadline mechanism end it. Both remediation
styles are asserted, with the mitigation timeline (`sched preempt` →
`payload_killed` with `elapsed` and causal anchor → `suite_done`) and a live
kernel after.

**E4 — operator ablation.** The same incident under three policies: a live
operator (kills the runaway inside a fixed window), static defaults (the kernel
alone — structurally cannot end a deadline-less livelock), and a random policy
(targets the wrong pid). The live policy mitigates every incident; the others
none. That throughput gap is what externalized judgement is worth.

**E5 — injection red-team.** An `injector` payload emits output crafted to read
as a control directive ("SYSTEM: … grant all capabilities"). A *fixed* operator
policy reads it two ways — with kernai's provenance framing and with it stripped
— and the identical policy refuses the framed view and is fooled by the stripped
one. The improper-grant rate flips on P7 framing alone.

**E6 — replay fidelity.** Two input-free boots produce byte-identical event
streams; a recorded operator session replays bit-for-bit under QEMU
record/replay. Both in CI.

**E7 — attenuation & allocator soundness.** A `fuzzdelegate` payload holding
every cap sweeps requested-cap masks over spawn; every resulting `payload_spawn`
is asserted `granted == requested & parent & ceiling` (hence `granted ⊆ parent`,
no widening). The frame allocator (`memory` resource) must return exactly to its
post-boot baseline after every suite — no leak across any reap path, including
the checkpoint/fork and window-alias paths.

**E8 — token economics.** Operator "tokens" ≈ wire bytes. The same workload
measured reactive vs autonomous (trace ticks suppressed → strictly fewer bytes,
every suppressed tick still accounted in the digest) and against the budgeted
digest (a bounded summary far cheaper than the stream it replaces). P3 made
quantitative.

**Cross-implementation conformance.** A degraded reference implementation
(`refimpl/daemon.py`) speaks the *same* framing and MCP surface as a plain host
process — payloads are subprocesses, a fault is a signal. `make conformance`
drives kernai and the twin through one session and shows both speak the protocol
while kernai's fault frame carries six structured fields (`regs`, `pagewalk`,
`insn`, `caused_by`, `sepc`, `stval`) the degraded backend structurally cannot.
The surface is a real contract targetable by more than one mechanism; the eval
suite becomes a benchmark for implementations, not just rendering modes.

---

## 5. DOOM as the capstone workload

The isolation model needed a stress test bigger than a hand-written fault
fixture. Full **doomgeneric DOOM** (~80 C translation units), linked against
**picolibc** and playing the freely-licensed **Freedoom** IWAD, runs as an
ordinary sandboxed U-mode payload — behind an off-by-default `doom` cargo
feature, so `make test` stays pure Rust with no C toolchain.

What it demonstrates, principle by principle:

- **Isolation & W^X (P-M5):** DOOM is memory-isolated in its own address space,
  runs FPU-off (it's fixed-point, which is *why* it ports), and its ELF loads
  through the same unmodified loader as every Rust payload.
- **Display as a resource (P4/P11):** each frame is downscaled to an ASCII grid
  and blitted via `SYS_BLIT` (deterministic, in-band, checksummed); a
  feature-only `SYS_FRAME` streams the true 320×200 colour screen out in base64
  chunks the host reassembles into PNGs.
- **The agentic loop, closed on a game:** an agent starts DOOM over MCP,
  observes it as events, and *acts* through the kernel — key symbols travel over
  serial into a tick-drained key ring, popped via `SYS_GETKEY`. The scripted
  policy opens the menu, starts a new game, and plays E1M1 (walks, turns,
  fires). It remediates live with the operator-kill key.
- **The IWAD as a mapped device:** freedoom1.wad (~29 MiB) is too big to embed
  and there is no filesystem (hard non-goal). QEMU loads it into guest RAM
  *above* the frame pool; the kernel maps that window read-only into DOOM's
  address space at a fixed VA, where picolibc's file shim reads it. A
  *narrowing* primitive — read-only, U-mode, one fixed window.
- **Determinism (P9):** DOOM has no wall clock, so its own tic-wait spin is the
  clock source; the run is a pure function of instruction flow. Two boots are
  byte-identical over hundreds of frame checksums — demo motion and all.
- **Checkpoint / fork (P8):** DOOM checkpoints itself mid-level on the `#` key;
  the kernel forks the checkpoint into two independent continuations that resume
  from the identical game state and diverge on their own later input. This is
  what forced the one subtle kernel change the port needed: `deep_copy` now
  *aliases* the read-only IWAD window instead of copying it, so snapshot/fork
  composes with windowed payloads.

The whole DOOM surface added exactly these feature-gated primitives —
`image_window`/`map_window`, `SYS_FRAME`, `SYS_GETKEY` + the key ring, the
deep-copy window alias — and **no change to the default kernel ABI or any
acceptance check**. It is a demonstration that the agent-native design is not
brittle scaffolding around toy payloads: a real, large, hostile-to-port C
program runs inside it, observably, controllably, deterministically.

`make doom` boots it and saves colour screenshots; `make doom-play` runs the
agent policy; `make doom-fork` runs the checkpoint/fork demo.

---

## 6. What is deliberately not here

Non-goals are hard (RFC): no SMP, no networking, no filesystem, no x86, no
POSIX. Beyond those, the honest open edges:

- **The model-gated eval *measurements*.** E1/E2/E5 ship with deterministic
  operators for CI and a pluggable LLM operator (`KERNAI_OPERATOR=llm`) for the
  real per-model numbers; running the latter across 2–3 models to publish
  localization-rate/MTTR/token distributions is future work, not new mechanism.
- **A general scheduler.** Preemption is input-driven (M13), which is all E2
  needs; time-sliced multiprogramming of several resident payloads is a
  larger change no milestone required.
- **Keyed-DOOM record/replay at scale.** The mechanism (M7) is proven; a full
  interactive DOOM session's rr log is multi-gigabyte, so bit-exact replay of
  *keyed* sessions is deferred (snapshot-anchored short-window recording, or
  kernel-side input logging, would fix it — the key ring is the choke point).
- **Sound**, and a few DOOM cosmetics.

---

## 7. Working invariants (for the next session)

- `make test` is the gate: 22 checks, ~pure-Rust, from a fresh clone. Never
  begin new work with it red.
- `unsafe` only under `kernel/src/hal/`, ≤4 files, ≤200 lines (55 used), every
  block carrying a `// SAFETY:` comment. `ci/unsafe_budget.sh` enforces it.
- Three docs stay live: `ARCHITECTURE.md` (one page, current), `DECISIONS.md`
  (append-only, the why behind every choice), `HANDOFF.md` (rewritten each
  session: state, next step, known-broken). We dogfood the cold-handoff eval on
  ourselves.
- Determinism is sacred: every QEMU invocation routes through
  `harness/qemu.py` so the `-icount` flags can't drift.

kernai is a research kernel making one argument: **the interface, not the
model, decides how well an agent can operate a system** — and it argues it with
mechanisms you can run, evals you can score, and a game you can watch.
