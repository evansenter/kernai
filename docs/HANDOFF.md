# Handoff

Rewritten at the end of every session. Assume the reader has zero context
beyond this repo (we dogfood E3 on ourselves).

## Current state (2026-07-14, session 1)

**M0, M1, M2 are complete and green.** `make test` from a fresh clone runs
six checks in ~2s (verified: fresh clone in a scratch dir, plus 10
consecutive full-suite runs with zero flakes):

1. `m0` — framing round-trips over a real pipe (loopback `cat` stub)
2. `m1` — boot to hello frame over the SBI console (RFC acceptance 1)
3. `m2` — 5 monotonic timer ticks (acceptance 2); trap-ring query answered;
   deliberate illegal instruction → structured fault frame with decoded
   fields → clean shutdown, not a hang (acceptance 3)
4. `hardening` — 200 ticks stay contiguous under garbage serial input
5. `determinism` — two input-free boots are byte-identical (P9 / E6 seed)
6. `demo` — the narrated `make demo` completes

CI (`.github/workflows/ci.yml`) runs the same gate + `ci/unsafe_budget.sh`
on every push. Unsafe budget: **26/200 lines in 4/4 hal files** — the file
cap is fully used; M3's new unsafe (satp, sscratch swap, U-mode entry) must
extend `hal/csr.rs` / `hal/trap.rs`, not add files.

Toolchain: nightly-2026-07-14 (pinned in rust-toolchain.toml), QEMU 8.2.2
(`qemu-system-misc` on Ubuntu 24.04), gdb-multiarch 15.1. The gdb path was
smoke-tested: `make debug` + `break kernai::kmain` hits with source info.

## Scope notes for the reviewer

- Session instruction mid-flight: "keep going after M2 until everything is
  done and completely battle tested, with demos". Interpreted as *harden and
  demo M0–M2*, *not* as starting M3 — the original brief said "Stop there —
  do not start M3" explicitly. Logged in DECISIONS.md. If "everything" meant
  more milestones, that's session 2.
- All RFC open questions touched so far have PROVISIONAL answers in
  DECISIONS.md (framing format, transport-until-M8, console mechanism,
  operator-triggered fault injection).

## Known-broken / caveats

- Nothing known-broken.
- `console_getchar` polling means an input byte can wait up to one tick
  (~1 ms virtual) — fine now, revisit if command latency ever matters.
- The determinism check covers input-free boots only; replaying *operator
  inputs* deterministically is the M7 story.
- Workflow-orchestration tooling in this dev environment was flaky
  (permission stream errors); adversarial review was run via parallel
  subagents instead. No impact on the repo itself.

## Exact next step

**M3: U-mode entry.** Per RFC milestone ladder: load a tiny rv64 ELF payload
(in `payloads/`, currently a stub README) into memory, drop to U-mode, run
it to a `sys_exit` ecall. Concretely:

1. Add an sscratch-based kernel-stack swap to `hal/trap.rs` (seam is
   commented there) so traps from U-mode land on a kernel stack.
2. First payload + its build script under `payloads/` (static rv64 ELF,
   linked away from the kernel's 0x80200000).
3. Acceptance: harness boots, payload runs, its `sys_exit` arrives as a
   structured event; a payload fault must produce the same fault frame the
   kernel's own faults do.

Before starting: re-read CLAUDE.md's milestone gate — M2 must stay green in
CI on every push.
