# Architecture (current: M0)

One page, always accurate. Principles P1–P12 referenced here are defined in
`RFC-001-agent-native-kernel.md`.

## What exists

```
host (Python harness)                        guest (none yet)
┌──────────────────────────────┐
│ harness/framing.py           │   frames:  0xAA 0x99 | u32 LE len | payload
│   encode_frame/FrameDecoder  │   payloads: UTF-8 JSON event objects
│ harness/transport.py         │
│   FrameStream (fd + timeout) │◄── M0: loopback `cat` subprocess
│   Loopback (M0 stub)         │    M1+: QEMU serial (stdio)
│ harness/runner.py            │
│   milestone acceptance gates │
└──────────────────────────────┘
```

- **Framing** is deliberately dumb (length-prefixed; 2 magic bytes solely to
  resync past OpenSBI's boot banner on the shared UART). No checksum, no
  retransmit: QEMU's virtual serial is lossless.
- **FrameStream** watchdog timeouts are host-side hygiene only; all
  guest-visible time will be icount-derived (P9). Every QEMU invocation in
  the Makefile uses `-icount shift=1,sleep=off`.
- **Unsafe budget** (`ci/unsafe_budget.sh`) is static analysis over
  `kernel/src`: unsafe only in `hal/`, ≤4 files, ≤200 lines, `// SAFETY:` on
  every occurrence, `#![forbid(unsafe_code)]` elsewhere. Runs toolchain-free
  so CI enforces it independently of the build.

## Planned seams (not built yet)

- **P4 (MCP control plane, M8):** the framing layer is transport-agnostic;
  virtio-serial replaces the SBI-console UART underneath it without touching
  frame or event formats.
- **P6 (diagnostic frames, M9):** kernel events are JSON objects with a
  monotonic `id` from day one, so richer fault frames extend — not replace —
  the event shape.
