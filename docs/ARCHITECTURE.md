# Architecture (current: M11)

One page, always accurate. Principles P1–P12 are defined in
`RFC-001-agent-native-kernel.md`. Beginner-level narrative: `WALKTHROUGH.md`.

## What exists

```
host (Python, stdlib only)                guest (qemu -machine virt, -icount)
┌──────────────────────────────┐         ┌───────────────────────────────────┐
│ framing.py  encode/decode     │         │ OpenSBI (QEMU's, M-mode)          │
│ transport.py FrameStream      │ frames  │   ▲ ecall: putchar getchar        │
│ qemu.py     THE qemu cmdline  │◄────────│   │        set_timer SRST         │
│ runner.py   acceptance gates  │ cmds    │ kernel (S-mode, 0x80200000)       │
│ demo.py     narrated tour     │────────►│ ┌ hal/ (unsafe island, 55/200) ─┐ │
└──────────────────────────────┘ rxpmi   │ │ boot.rs entry, bss, phys i/o  │ │
                                          │ │ sbi.rs  ecall wrappers        │ │
 frames: AA 99 | u32 LE len | JSON        │ │ csr.rs  CSRs, satp, sscratch  │ │
 events: {"id":N,...} id == stream order  │ │ trap.rs vector, enter_user    │ │
                                          │ └───────────────────────────────┘ │
 frame pool: 0x80400000..0x88000000       │ traps.rs dispatch, ring, faults   │
 per-payload Sv39 address spaces;         │ frames.rs bitmap frame allocator  │
 kernel = supervisor gigapage (U=0) in    │ mm.rs    Sv39 page tables (safe)  │
 every payload table                      │ elf.rs   ELF64 loader → mapping   │
                                          │ syscall.rs ABI v0 dispatch (safe) │
                                          │ payload.rs proc table + scheduler │
                                          │ rpc.rs   MCP/JSON-RPC dispatch    │
                                          │ console.rs FrameBuf  events.rs id │
                                          │ main.rs  kmain + idle command loop│
                                          └───────────────────────────────────┘
```

## Control flow

Boot → `kmain`: hello (event 0), assert kernel image ends below the frame
pool, **enable Sv39 paging** (kernel identity gigapage; the kernel runs
translated but transparently), install trap vector, arm timer, enter
`idle()`. `idle` serves single-byte operator commands (P1 seed): `r` ring
dump, `x` kernel illegal-instruction, `p`/`m`/`i`/`f` the M3–M6 payload
suites — and a leading `0xAA` byte switches into the MCP/JSON-RPC request
reader (`rpc.rs`, M8/P4), the structured control plane the bytes were always a
stand-in for. The boot event stream stays identical to M2 — payloads only run
when asked.

**Payload lifecycle** (sequential run-to-completion): the scheduler builds a
fresh address space (`mm.rs`: kernel gigapage + the payload's ELF segments
mapped W^X from allocated frames via `elf.rs`, all safe over `hal::phys_*`),
records a start timestamp + deadline, switches `satp` and emits
`payload_start`,
and `hal::enter_user` switches `satp` and drops to U-mode. Traps from U-mode
land on a dedicated kernel trap stack (sscratch swap in the vector), still
under the payload's `satp` (the kernel gigapage keeps the handler mapped). On
`ecall`, `syscall::dispatch` runs (`exit`/`write`/`yield`/`spawn`,
capability-checked; `write` reads user bytes through the payload's page table
so a bad pointer is EFAULT, never a kernel read); on fault — including a page
fault from touching kernel memory or writing a text page (W^X) — the P6 frame
is emitted tagged `origin:"payload"` with a page-table walk, and only the
payload dies; on timer, an over-budget payload is killed. A departing payload
is handed back to the scheduler by **rewriting its trap frame** to resume in
S-mode on the boot stack (`redirect_to_scheduler`). `run` then switches to the
kernel `satp` and **reaps** the terminated payload's frames before picking the
next. Queue drains → `suite_done` → `idle`.

## Invariants worth defending

- **Emission is atomic**: every frame is emitted with interrupts masked
  (trap context for tick/fault/syscall; explicit `without_interrupts` for
  ring dump, panic, and all payload events). Ids strictly increase in stream
  order; asserted stream-wide.
- **Determinism (P9)**: one QEMU command line (`harness/qemu.py`). Input-free
  boots are byte-identical; the deadline kill lands at the same instruction
  every run under identical input timing.
- **Unsafe island**: `hal/` only, 55/200 budget lines, 4/4 files (at the file
  cap — new hal code extends existing files). ELF loading, page tables, the
  frame allocator, the process table, the scheduler, and all policy are safe
  code. Physical-frame access is safe: `hal::phys_*` bounds-check every access
  into the pool, which boot asserts is disjoint from the kernel image.
- **Memory isolation (M5)**: each payload has its own Sv39 table; kernel RAM
  is a supervisor gigapage (U=0) so a U-mode payload faults on it, and text
  pages are mapped R|X (never W). A payload cannot address another's frames
  (no PTE) or the kernel's.
- **Attenuation (P10)**: `granted = requested & parent_caps & image_ceiling`,
  so a delegation chain can only shrink — proven over a two-hop chain where
  every hop greedily requests all caps (M10: `delegator{write,spawn,yield} →
  redelegator{write,spawn} → worker{write}`). Enforced in `payload::on_spawn`;
  `enqueue` re-clamps to the ceiling as defense in depth. `spawn` is
  payload-only and `CAP_SPAWN`-gated — the control plane is not a capability
  source.
- **The serial layer stays dumb**: length-prefix + magic, single-byte commands.

## Attachment points for later principles

- **P4/P5 (MCP, M8, done)**: JSON-RPC 2.0 (MCP method shapes) rides *inside*
  the existing length-prefixed frames — a leading `0xAA` in the operator input
  switches that byte into `rpc::read_request`, which reads the frame body and
  dispatches (`rpc.rs`, all safe: a hand-rolled structural JSON reader over a
  fixed grammar). Tools (`run_suite`, `crash`, `ring_read`) map to the command
  bytes; resources (`trap_ring`, `processes`, and the P5 self-describing
  `spec` — syscall table, cap lattice, memory map) expose kernel state.
  Responses wrap in a `{"id":<stream>,"type":"rpc","rpc":{…}}` envelope so the
  monotonic-stream-id invariant holds for control traffic. Mutating calls are
  idempotent via a client `opId` (an 8-slot FIFO of seen hashes → a replay is
  answered `duplicate`, never re-run). Transport junk is silently dropped and
  resynced (P-serial). Single command bytes remain as a compat/fallback plane.
- **P6 (M9)**: the payload `fault` frame already carries the page-table walk;
  M9 grows it further (richer registers, a `cause` parent id — P12) and adds
  the `surface-classic` printf twin for the E1 A/B.
- **P7 (now → M9)**: payload output is already tagged `untrusted` and confined
  to a JSON string; the write quota bounds a hostile payload's context flood.
- **P8 (M6, done)**: `sys_snapshot` deep-copies a payload's address space +
  saves its trap frame + CapSet (`mm::deep_copy`, `payload::Snapshot`);
  restore/fork rebuild an independent continuation and `hal::resume_user`
  (a full-frame trampoline) resumes it. snapshot returns a positive id in the
  original and 0 in each continuation (fork()-style). `resume_user` is also
  the suspend/resume primitive real scheduling will use.
- **P9/E6 (M7, done)**: the byte-identical determinism guarantee now covers
  recorded *operator input*, via QEMU's own record/replay (`-icount
  rr=record/replay`, `harness/qemu.py`). A live session (all four suites, then
  the crash) is logged to an `rrfile`; replaying with no live input reproduces
  every event frame bit-for-bit (`runner.py::m7`). No kernel change — QEMU logs
  each input at the instruction count it was consumed and re-injects it there.
- **P6/P12/E1 (M9, done)**: the `fault` frame now carries the full register
  file (`regs`, 31 ABI-named GPRs) and a P12 causal parent (`caused_by` — the
  `payload_start` it descends from; threaded through every payload lifecycle
  event so the log is a DAG). The `surface-classic` twin is a runtime toggle
  (`set_surface` tool / `surface` resource): the same fault renders as one
  printf `[FAULT] …` console line (`traps::emit_fault_classic` via `RawConsole`,
  unframed → the host noise channel) instead of the rich frame — the A/B
  substrate for E1. Scope: M9 twins the diagnostic path; a fully classic event
  stream is M12 eval machinery.
- **P3 (M11, done)**: the `digest` resource takes a `budget` param and returns
  per-severity totals (ticks coalesced into a count) + at most `budget` of the
  most-recent notable traps, newest first, with an `elided` count — a summary,
  not the firehose. The autonomy dial (`set_autonomy` tool) makes the kernel
  self-manage attention: at `autonomous` it suppresses trace-severity tick
  *frames* from the wire (still counting them + checking deadlines), leaving the
  digest to pull on demand. Both are `traps.rs` state + `rpc.rs` verbs.
- **E1–E8 (M12, next)**: wire the M9 two-surface toggle into an actual A/B eval
  harness over a seeded-bug stimulus set (E1 headline), plus E2/E3/E5. The eval
  suite ships as the public benchmark; the MCP surface as the published spec.
