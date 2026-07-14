# Architecture (current: M2)

One page, always accurate. Principles P1–P12 referenced here are defined in
`RFC-001-agent-native-kernel.md`. For a beginner-level narrative of the same
material, see `WALKTHROUGH.md`.

## What exists

```
host (Python, stdlib only)                guest (qemu -machine virt, -icount)
┌──────────────────────────────┐          ┌──────────────────────────────────┐
│ framing.py  encode/decode    │          │ OpenSBI (QEMU's, M-mode)         │
│ transport.py FrameStream     │  frames  │   ▲ ecall: putchar getchar       │
│ qemu.py     THE qemu cmdline │◄─────────│   │        set_timer SRST        │
│ runner.py   acceptance gates │  'r','x' │ kernel (S-mode, 0x80200000)      │
│ demo.py     narrated tour    │─────────►│ ┌─ hal/ (the unsafe island) ───┐ │
└──────────────────────────────┘  UART/   │ │ boot.rs  entry asm, bss, sp  │ │
                                  stdio   │ │ sbi.rs   ecall wrappers      │ │
 frames: AA 99 | u32 LE len | JSON        │ │ csr.rs   CSRs, irq on/off    │ │
 events: {"id":N,"type":...}  id is       │ │ trap.rs  vector save/restore │ │
 globally monotonic == stream order       │ └──────────────────────────────┘ │
                                          │ traps.rs   dispatch, tick, ring, │
                                          │            fault frames (safe)   │
                                          │ console.rs FrameBuf (safe)       │
                                          │ events.rs  id counter (safe)     │
                                          │ main.rs    kmain + command loop  │
                                          └──────────────────────────────────┘
```

## Control flow after boot

`_start` (asm: zero bss, set 64K stack) → `kmain`: emit hello (event 0),
install `stvec` → full 31-GPR+sepc+sstatus save/restore vector, arm the SBI
timer, then loop: poll `console_getchar` for command bytes, `wfi` otherwise.

- **Timer trap** (every 10_000 timebase units = 500k instructions under
  `-icount shift=1` — an instruction-count cadence, P9): re-arm, record in
  ring, emit `tick{seq,time}`.
- **`'r'` command**: emit `trap_ring` — the last 8 traps (id, cause, sepc).
  First sliver of P11; the future MCP resource `/trace/traps` (P4 seam) is
  this same data behind a real protocol.
- **`'x'` command**: deliberately execute `csrrw x0, cycle, x0` (illegal:
  cycle is read-only). Any non-timer trap → emit the v0 P6 diagnostic frame
  (cause, sepc, stval, decoded instruction incl. offending CSR, ra/sp,
  last-8 trap history) → SBI shutdown. **Faults are never hangs.**
- **Panic** → structured `panic` frame → shutdown. Same discipline.

## Invariants worth defending

- **Emission is atomic**: frame bytes + id allocation happen with interrupts
  disabled (`without_interrupts`), so frames can't interleave on the wire
  and event ids strictly increase in stream order. The acceptance suite
  asserts this stream-wide.
- **Determinism**: the QEMU command line exists in exactly one place
  (`harness/qemu.py`; `-icount shift=1,sleep=off -rtc clock=vm`). `make
  test` asserts two input-free boots are byte-identical (E6 seed).
- **Unsafe island**: `hal/` only — currently 26/200 budget lines across
  4/4 files (at the file cap; M3 must extend existing hal files, not add).
  Ring buffer stays safe code via per-field atomics; consistency comes from
  single-hart execution + irq-off critical sections, not locks.
- **The serial layer stays dumb**: length-prefix + magic, no checksums, no
  retransmit, single-byte commands.

## Planned seams (deliberately visible, not built)

- **P4 (MCP, M8)**: `FrameStream` reads any fd — virtio-serial will replace
  the UART beneath it; JSON-RPC rides inside the same frames; `trap_ring`
  becomes a resource, `'x'`-style pokes become tools.
- **P6 (M9)**: `fault`/`panic` frames grow page-table walks, richer register
  files, and a `cause` field pointing at a parent event id (P12) — same
  event shape, more fields.
- **M3 (next)**: U-mode entry. `hal/trap.rs` gains an sscratch stack swap
  (seam noted in the file); payloads/ gets its first ELF.
