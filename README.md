# kernai

An agent-native RISC-V unikernel. See `docs/RFC-001-agent-native-kernel.md` for
the thesis and `CLAUDE.md` for working conventions. New here — or new to
kernels entirely? Start with `docs/WALKTHROUGH.md`, then run `make demo`.

Current state: **M2** (boot, traps, SBI timer, trap ring buffer, structured
fault reports). See `docs/HANDOFF.md` for the exact next step.

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
make build   # build the kernel ELF (release)
make run     # boot it under QEMU (-icount, deterministic) with serial on stdio
make demo    # narrated tour: boot, ticks, ring query, fault report, determinism
make test    # full acceptance suite: framing (M0), boot (M1), traps/timer/fault
             # (M2), input hardening, determinism, demo — plus unsafe budget;
             # this is what CI runs
make debug   # boot QEMU halted with a gdb stub on :1234
make gdb     # attach gdb-multiarch to a running `make debug`
```

`make test` must pass from a fresh clone after the bootstrap above; if it
doesn't, that is a bug.

## Determinism

Every QEMU invocation uses `-icount shift=1,sleep=off` (P9): virtual time is
derived from the instruction count, never the host clock. Timer cadence and
deadlines are therefore exact instruction counts and identical across runs.

## Layout

```
kernel/    no_std kernel; src/hal/ is the only unsafe island (budget-enforced)
payloads/  tiny rv64 acceptance-test ELFs + build script (populated at M3)
harness/   host-side Python driver: framing, QEMU transport, milestone runner
ci/        unsafe budget check, GitHub Actions helpers
docs/      RFC-001, ARCHITECTURE.md, DECISIONS.md, HANDOFF.md
```
