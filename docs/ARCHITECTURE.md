# Architecture (current: M12 — ladder complete)

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
└──────────────────────────────┘ cmds     │ │ boot.rs entry, bss, phys i/o  │ │
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
dump, `x` kernel illegal-instruction, `p`/`m`/`i`/`f`/`d`/`e` the payload
suites (M3–M6, M10, and the M12 eval stimulus) — and a leading `0xAA` byte
switches into the MCP/JSON-RPC request
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

## How each principle is realized (P1–P12 all landed)

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
- **P7 (done)**: payload output is tagged `untrusted` and confined to a JSON
  string (`payload_output`), and the per-write byte quota bounds a hostile
  payload's context flood — the provenance seam that keeps workload bytes from
  ever being read by the operator as kernel-issued instructions.
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
- **E1/E3/E6 (M12, done)**: `harness/eval.py` scores the two surfaces over a
  seeded-fault set — the structured surface recovers 17/17 localization facts,
  the classic printf twin 8/17 (`make eval`; the `e1` check asserts the gap).
  `e3` reconstructs state from resources alone (cold handoff); `e6` is M7's
  replay. The MCP surface is published as `docs/SPEC.md` v0.1 — the RFC's
  durable artifact. Remaining evals (full agent-loop E1, E4/E5/E7/E8, and the
  full E2 A/B measurement) are future work; the M0–M12 ladder is complete.
- **P1/P2/E2 (M13, done)**: the control plane is live while payloads run. A
  timer tick that finds serial input pending (for a payload that hasn't opted
  into keyboard input) suspends the payload — its register file saved the same
  way M6 checkpoints save one — emits a `sched preempt` event (P11), services
  the plane in the scheduler (`payload::service_console`: JSON-RPC frames + the
  ring-dump byte; suite-seeding/crash verbs answer `busy` mid-run), and resumes
  it silently: no new `payload_start`, deadline budget not refilled, causal
  anchor unchanged. Preemption is input-driven only, so input-free runs are
  byte-identical to before (P9/m7 unaffected). On top of it: the `kill {pid}`
  tool (opId-idempotent) emits `payload_killed reason:"operator"` with
  `elapsed` — the time-to-mitigation fact — and the `e2` acceptance check
  remediates a deadline-less `livelock` payload live, control-plane only.

## Beyond the ladder: C payloads and DOOM (optional features)

The kernel runs arbitrary freestanding C, not just Rust — proven by two
opt-in payloads that touch no core code path. Both are cargo features, **off by
default**, so `make test` and CI stay pure-Rust with no C toolchain.

- **`cpayloads`** (`make raycast`): a fixed-point raycaster in freestanding C
  (clang, `-march=rv64imac -mabi=lp64`), linked with the shared `link.ld`, run
  through the unmodified ELF loader. It renders via `SYS_BLIT`.
- **`doom`** (`make doom`): full **doomgeneric DOOM** (~80 C units) linked
  against **picolibc**, playing the **Freedoom** IWAD. It boots, shows the title,
  and plays its attract-mode demo (first-person 3-D, HUD, enemies) as a sandboxed
  U-mode payload — FPU-off (fixed-point), memory-isolated, deterministic under
  `-icount` (two boots byte-identical, P9). See DECISIONS.md (2026-07-19) for the
  full rationale. The port added a handful of feature-gated primitives and no
  change to the default ABI:
  - **`image_window` / `map_window`** — a payload image may declare one
    read-only physical window mapped into its address space on top of its ELF.
    DOOM uses it for the IWAD: QEMU loads freedoom1.wad into guest RAM *above*
    POOL_END (`-m 256M -device loader,…,addr=0x88000000`) so the frame allocator
    never touches it, and the kernel maps that window R+U at a fixed VA where the
    payload's picolibc file shim reads it. A *narrowing* primitive (read-only,
    U-mode, one fixed window); `frames::free` already ignores the out-of-pool
    leaves, so `destroy` reaps the payload with no allocator change. `deep_copy`
    aliases (never copies) out-of-pool user leaves, so snapshot/fork (P8)
    composes with windowed payloads.
  - **`SYS_FRAME`** (`doom` build only) — streams the true 320×200 colour screen
    out in base64 `fbchunk` events the host reassembles into PNGs, reading the
    payload framebuffer *through its page table* (bad pointer → EFAULT), the same
    confused-deputy defense as `write`/`blit`. `SYS_BLIT`'s ASCII+checksum frame
    remains the deterministic in-band surface; `SYS_FRAME` is the richer
    "display as a resource" seam.
  - **`SYS_GETKEY` + the key ring** (`doom` build only) — the input half of the
    agentic loop. While an *input-wanting* payload runs (`image_wants_input`,
    i.e. DOOM only), the timer tick drains serial into a small ring the payload
    pops via `getkey`. Every key event is the two-byte escape sequence
    `0xA5 <key>` (low 7 bits = symbol, bit 7 = release; the payload owns the
    symbol→key mapping), and `0xA5 0x03` is the **operator kill** — the payload
    is marked KILLED with a structured `payload_killed reason:"operator"` event
    (P1/P2, the E2 seed). The escape prefix keeps the shared serial line
    unambiguous (audit-hardened): bare bytes mid-run are discarded — never
    misread as keys or kills, never leaked to the untrusted payload — and a
    pair straddling payload termination is swallowed by the idle loop via
    shared pending state instead of executing as a command. No capability
    gates `getkey`: input is a grant by construction (the operator chose to
    feed this payload), unlike output, which stays cap-gated.
  - **Agent-plane parity (P4/P5)** — `run_suite` accepts `doom`/`craycast` on
    feature builds, and the `spec` resource self-describes the doom ABI
    (syscalls `frame`/`getkey`, the IWAD window). `make doom-play`
    (`harness/doom_play.py`) closes the loop end-to-end: start DOOM over MCP,
    observe `frame`/`fbchunk` events, walk the menu and play E1M1 via key
    symbols, kill it mid-run, read `processes` for the structured post-mortem.
    Interactive input is host-timed, so keyed sessions replay via QEMU
    record/replay (M7); input-free runs stay boot-for-boot byte-identical.
