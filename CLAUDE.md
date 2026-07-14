# kernai — agent-native kernel

Read `docs/RFC-001-agent-native-kernel.md` before writing any code. It is the spec: the twelve principles (P1–P12), milestone ladder (M0–M12), and evaluation plan (E1–E8) defined there are canonical. This file covers working conventions only.

## Environment

- Target: `riscv64gc-unknown-none-elf`, Rust nightly pinned in `rust-toolchain.toml`
- Runs under `qemu-system-riscv64 -machine virt` with QEMU's bundled OpenSBI — never vendor firmware, never write M-mode code
- Determinism from day one (P9): all QEMU invocations use `-icount`; deadlines are instruction counts, never wall time
- Debug path: `qemu -s -S` + `gdb-multiarch`; a `just debug` (or `make debug`) target must exist by M2
- Verify toolchain before assuming it (`rustup target list --installed`, `qemu-system-riscv64 --version`, `gdb-multiarch --version`) and keep exact bootstrap steps current in README

## Hard rules

- `unsafe` only under `kernel/src/hal/` — max 4 files, max 200 lines total inside `unsafe` blocks/fns. `#![forbid(unsafe_code)]` in every other crate/module. Every `unsafe` carries a `// SAFETY:` invariant comment (`clippy::undocumented_unsafe_blocks = deny`). `ci/unsafe_budget.sh` enforces all of this; never weaken it, never launder through transmute helpers.
- Never make a failing acceptance test pass by weakening its assertion. If a test is wrong, say so in DECISIONS.md and fix the test in its own commit.
- The serial layer stays dumb: length-prefixed frames, no cleverness. Flakiness here contaminates every downstream measurement.
- Non-goals are hard: no SMP, no networking, no filesystems, no x86, no POSIX.

## Workflow

- Milestone-gated. One milestone = its acceptance check green + docs updated, before starting the next. Never begin M(n+1) with M(n) red.
- `make test` must pass from a fresh clone on a machine with the documented bootstrap.
- Maintain three docs:
  - `ARCHITECTURE.md` — current design, one page, always accurate
  - `DECISIONS.md` — append-only: decision, alternatives considered, why. RFC open questions get provisional answers logged here (tagged `PROVISIONAL`) rather than blocking work.
  - `HANDOFF.md` — rewritten at the end of every session: current state, exact next step, anything known-broken
- Assume the next session starts with zero context beyond this repo. We dogfood the RFC's cold-handoff eval (E3) on ourselves from day one.
- Commits are small and milestone-labeled (`M2: trap frame save/restore`). CI (build + test + unsafe budget) must be green on every push to main.

## Layout (target)

```
kernel/    no_std kernel; src/hal/ is the only unsafe island
payloads/  tiny rv64 acceptance-test ELFs + their build script
harness/   host-side Python driver (framing, milestone runner)
ci/        unsafe budget check, clippy config, GH Actions helpers
docs/      RFC-001, ARCHITECTURE.md, DECISIONS.md, HANDOFF.md
```
