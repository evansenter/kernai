# Architecture (current: M4)

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
│ demo.py     narrated tour     │────────►│ ┌ hal/ (unsafe island, 56/200) ─┐ │
└──────────────────────────────┘ r x p m │ │ boot.rs entry, bss, arena i/o │ │
                                          │ │ sbi.rs  ecall wrappers        │ │
 frames: AA 99 | u32 LE len | JSON        │ │ csr.rs  CSRs, sscratch, irq   │ │
 events: {"id":N,...} id == stream order  │ │ trap.rs vector, enter_user    │ │
                                          │ └───────────────────────────────┘ │
 payload arena: 0x80400000, 2 MiB,        │ traps.rs dispatch, ring, faults   │
 one resident payload (no paging yet)     │ elf.rs   ELF64 loader (safe)      │
                                          │ syscall.rs ABI v0 dispatch (safe) │
                                          │ payload.rs proc table + scheduler │
                                          │ console.rs FrameBuf  events.rs id │
                                          │ main.rs  kmain + idle command loop│
                                          └───────────────────────────────────┘
```

## Control flow

Boot → `kmain`: hello (event 0), assert kernel image ends below the arena,
install trap vector, arm timer, enter `idle()`. `idle` serves single-byte
operator commands (P1 seed): `r` ring dump, `x` kernel illegal-instruction,
`p` M3 payload suite, `m` M4 payload suite. This keeps the boot event stream
identical to M2 — payloads only run when asked.

**Payload lifecycle** (sequential; one resident pre-paging): the scheduler
loads a pending image's ELF into the arena (`elf.rs`, safe, via bounds-checked
`hal::arena_*`), records a start timestamp + deadline, emits `payload_start`,
and `hal::enter_user` drops to U-mode. Traps from U-mode land on a dedicated
kernel trap stack (sscratch swap in the vector). On `ecall`, `syscall::dispatch`
runs (`exit`/`write`/`yield`/`spawn`, capability-checked); on fault, the P6
frame is emitted tagged `origin:"payload"` and only the payload dies; on
timer, an over-budget payload is killed. A departing payload is handed back to
the scheduler by **rewriting its trap frame** to resume in S-mode on the boot
stack (`redirect_to_scheduler`) — the normal trap-restore path does the
context switch, so no separate switch routine is needed. Queue drains →
`suite_done` → `idle`.

## Invariants worth defending

- **Emission is atomic**: every frame is emitted with interrupts masked
  (trap context for tick/fault/syscall; explicit `without_interrupts` for
  ring dump, panic, and all payload events). Ids strictly increase in stream
  order; asserted stream-wide.
- **Determinism (P9)**: one QEMU command line (`harness/qemu.py`). Input-free
  boots are byte-identical; the deadline kill lands at the same instruction
  every run under identical input timing.
- **Unsafe island**: `hal/` only, 56/200 budget lines, 4/4 files (at the file
  cap — M5 must extend existing hal files). ELF loading, the process table,
  the scheduler, and all policy are safe code. Arena access is safe:
  `hal::arena_{read,write,zero}` bounds-check every access into the fixed
  arena, which boot asserts is disjoint from the kernel image.
- **Attenuation (P10)**: `granted = requested & parent_caps & image_ceiling`,
  so a delegation chain can only shrink. Enforced in `payload::on_spawn`.
- **The serial layer stays dumb**: length-prefix + magic, single-byte commands.

## Attachment points for later principles

- **P4 (MCP, M8)**: `FrameStream` reads any fd; virtio-serial replaces the
  UART beneath it, JSON-RPC rides inside the same frames. Control ops map to
  today's command bytes; `trap_ring`/process table become resources.
- **P6 (M9)**: the `fault` frame (payload + kernel) grows a page-table walk,
  richer registers, and a `cause` parent id (P12) — same event shape.
- **P7 (now → M9)**: payload output is already tagged `untrusted` and confined
  to a JSON string; the write quota bounds a hostile payload's context flood.
- **M5 (next)**: per-payload page tables (satp) let multiple payloads be
  resident; the scheduler's single-resident assumption and the arena become
  per-address-space. `hal/` gains paging; the process table gains an satp
  root per slot. Real cooperative `yield`/preemptive switch lands here.
