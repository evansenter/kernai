# Architecture (current: M6)

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
│ demo.py     narrated tour     │────────►│ ┌ hal/ (unsafe island, 71/200) ─┐ │
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
                                          │ console.rs FrameBuf  events.rs id │
                                          │ main.rs  kmain + idle command loop│
                                          └───────────────────────────────────┘
```

## Control flow

Boot → `kmain`: hello (event 0), assert kernel image ends below the frame
pool, **enable Sv39 paging** (kernel identity gigapage; the kernel runs
translated but transparently), install trap vector, arm timer, enter
`idle()`. `idle` serves single-byte operator commands (P1 seed): `r` ring
dump, `x` kernel illegal-instruction, `p`/`m`/`i` the M3/M4/M5 payload
suites. The boot event stream stays identical to M2 — payloads only run
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
- **Unsafe island**: `hal/` only, 71/200 budget lines, 4/4 files (at the file
  cap — new hal code extends existing files). ELF loading, page tables, the
  frame allocator, the process table, the scheduler, and all policy are safe
  code. Physical-frame access is safe: `hal::phys_*` bounds-check every access
  into the pool, which boot asserts is disjoint from the kernel image.
- **Memory isolation (M5)**: each payload has its own Sv39 table; kernel RAM
  is a supervisor gigapage (U=0) so a U-mode payload faults on it, and text
  pages are mapped R|X (never W). A payload cannot address another's frames
  (no PTE) or the kernel's.
- **Attenuation (P10)**: `granted = requested & parent_caps & image_ceiling`,
  so a delegation chain can only shrink. Enforced in `payload::on_spawn`.
- **The serial layer stays dumb**: length-prefix + magic, single-byte commands.

## Attachment points for later principles

- **P4 (MCP, M8)**: `FrameStream` reads any fd; virtio-serial replaces the
  UART beneath it, JSON-RPC rides inside the same frames. Control ops map to
  today's command bytes; `trap_ring`/process table become resources.
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
- **P9/E6 (M7, next)**: extend the byte-identical determinism guarantee to
  recorded *operator input*, with a replay mode that re-feeds a session and
  diffs the event stream. Mostly harness work + an `input` event.
