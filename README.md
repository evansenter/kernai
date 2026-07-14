# kernai

An agent-native RISC-V unikernel. See `docs/RFC-001-agent-native-kernel.md` for
the thesis and `CLAUDE.md` for working conventions. New here — or new to
kernels entirely? Start with `docs/WALKTHROUGH.md`, then run `make demo`.

Current state: **M9** — boot, traps, SBI timer, trap ring, structured fault
reports (M0–M2); U-mode payloads running to `sys_exit`, a payload crash kills
only the payload (M3); a capability-gated syscall surface with spawn
attenuation and instruction-count deadline kill (M4); per-payload Sv39 paging
giving real memory isolation and W^X, with page-table walks in fault reports
(M5); checkpoint/restore and speculative fork — a payload checkpoints itself
and the kernel forks independent continuations (M6, P8); deterministic replay
of a full operator session — record the serial input, replay it with none, get
a bit-identical event stream (M7, E6); an MCP/JSON-RPC control plane inside the
same frames — tools, resources, a self-describing `spec`, and idempotent
mutating calls (M8, P4/P5); rich P6 diagnostic frames — full register file plus
a P12 causal parent — and a runtime-toggled `surface-classic` printf twin for
the E1 A/B (M9); a multi-hop delegation chain proving capabilities only ever
attenuate, never re-widen, even under a greedy "request everything" at each hop
(M10, P10); an autonomy dial plus a token-budgeted `digest` resource that
coalesces the event firehose into a bounded severity-ranked summary (M11, P3).
See `docs/HANDOFF.md` for the exact next step
(M12: the E1–E8 evaluation suite).

## Bootstrap (Ubuntu 24.04 or similar)

```sh
# 1. QEMU with RISC-V system emulation (bundles OpenSBI), cross gdb, and a
#    host C toolchain (rustc needs `cc` to link build scripts)
sudo apt-get install -y qemu-system-misc gdb-multiarch python3 make gcc curl

# 2. Rust via rustup (https://rustup.rs). The pinned nightly + riscv target
#    are declared in rust-toolchain.toml; this installs them:
rustup toolchain install

# 3. Verify
qemu-system-riscv64 --version   # needs -machine virt (any recent version; CI uses 8.2)
gdb-multiarch --version
rustup target list --installed --toolchain nightly-2026-07-14 | grep riscv64gc
```

## Use

```sh
make build   # build payload workspace, then the kernel ELF (release)
make run     # boot it under QEMU (-icount, deterministic) with serial on stdio
make demo    # narrated tour: boot, ticks, ring, payloads, sandbox, fault, determinism
make test    # full acceptance suite: framing (M0), boot (M1), traps/timer/fault
             # (M2), U-mode payloads (M3), caps/spawn/deadline (M4), paging/
             # isolation (M5), checkpoint/fork (M6), deterministic replay (M7),
             # MCP control plane (M8), diagnostic frames + classic twin (M9),
             # delegation attenuation (M10), autonomy + event budgets (M11),
             # input hardening, determinism, demo — plus unsafe budget; CI runs this
make debug   # boot QEMU halted with a gdb stub on :1234
make gdb     # attach gdb-multiarch to a running `make debug`
```

Once booted (`make run`), the kernel serves single-byte operator commands on
the serial line: `r` dumps the trap ring, `x` crashes the kernel on purpose,
`p` runs the M3 payload suite, `m` the M4 sandbox suite, `i` the M5 isolation
suite, `f` the M6 checkpoint/fork suite, `d` the M10 delegation-chain suite. A
leading `0xAA` byte instead begins
an MCP/JSON-RPC request frame (M8): the structured control plane those bytes
are a stand-in for — `initialize`, `tools/list`, `tools/call`,
`resources/read` (`trap_ring`, `processes`, and the self-describing `spec`).

`make test` must pass from a fresh clone after the bootstrap above; if it
doesn't, that is a bug.

## Determinism

Every QEMU invocation uses `-icount shift=1,sleep=off` (P9): virtual time is
derived from the instruction count, never the host clock. Timer cadence and
deadlines are therefore exact instruction counts and identical across runs.
Determinism extends to *operator input* (M7/E6): QEMU's record/replay
(`-icount rr=record`, swapping in `rr=replay`) logs each serial byte at the
instruction it was consumed and re-injects it there, so a recorded session
replays to a bit-identical event stream — see `harness/runner.py::m7`.

## Layout

```
kernel/    no_std kernel; src/hal/ is the only unsafe island (budget-enforced)
payloads/  cargo workspace of tiny rv64 U-mode payloads (sys runtime + fixtures)
harness/   host-side Python driver: framing, QEMU transport, milestone runner
ci/        unsafe budget check, GitHub Actions helpers
docs/      RFC-001, ARCHITECTURE.md, DECISIONS.md, HANDOFF.md
```
