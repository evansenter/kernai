# Architecture (current: M1)

One page, always accurate. Principles P1–P12 referenced here are defined in
`RFC-001-agent-native-kernel.md`.

## What exists

```
host (Python harness)                     guest (qemu -machine virt, -icount)
┌──────────────────────────────┐          ┌──────────────────────────────────┐
│ harness/framing.py           │          │ OpenSBI (QEMU's, M-mode)         │
│   encode_frame/FrameDecoder  │  frames  │   ▲ ecall (legacy putchar, SRST) │
│ harness/transport.py         │◄─────────│ kernel (S-mode, 0x80200000)      │
│   FrameStream (fd + timeout) │  UART/   │   hal/boot.rs  asm entry, bss,   │
│ harness/qemu.py              │  stdio   │                stack → kmain     │
│   canonical QEMU invocation  │          │   hal/sbi.rs   ecall wrappers    │
│ harness/runner.py            │          │   console.rs   FrameBuf (safe)   │
│   m0 framing / m1 boot gates │          │   events.rs    monotonic ids     │
└──────────────────────────────┘          │   main.rs      kmain, panic frame│
                                          └──────────────────────────────────┘
```

- **Frames**: `0xAA 0x99 | u32 LE len | UTF-8 JSON payload`, identical
  constants in `harness/framing.py` and `kernel/src/console.rs`. The magic
  exists solely to resync past OpenSBI's ASCII banner on the shared UART.
- **Events**: every frame is a JSON object with a globally monotonic `id`
  (`events.rs`, AtomicU64). First frame is always `{"id":0,"type":"hello",…}`.
- **Boot path**: OpenSBI jumps to `_start` at 0x80200000 in S-mode →
  zero `.bss`, `sp = __stack_top` (64 KiB) → `kmain(hartid, dtb)`.
- **Unsafe island**: `kernel/src/hal/` only (boot asm shim + SBI ecalls);
  everything else `#![forbid(unsafe_code)]`. Budget: see `make unsafe-budget`.
- **Determinism (P9)**: the QEMU command line lives in ONE place,
  `harness/qemu.py` (`-icount shift=1,sleep=off -rtc clock=vm`); `make
  run/debug/test` all route through it. Host-side timeouts are watchdogs
  only, never guest-visible.
- **Panic = frame, not silence**: the panic handler emits a structured
  `panic` event (location + escaped message), then SBI shutdown.

## Planned seams (not built yet)

- **P4 (MCP control plane, M8)**: framing is transport-agnostic;
  virtio-serial replaces the SBI-console UART underneath it, and JSON-RPC
  rides inside the same frames. Nothing above `FrameStream`/`FrameBuf`
  changes.
- **P6 (diagnostic frames, M9)**: the `panic` event is the seed. Fault
  frames will extend the same event shape — `id` now, `cause` (P12 parent)
  and decoded machine state later.
