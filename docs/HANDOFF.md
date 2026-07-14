# Handoff

Rewritten at the end of every session. Assume the reader has zero context
beyond this repo (we dogfood E3 on ourselves).

## Current state (2026-07-14, session 2)

**M0–M6 complete and green.** `make test` from a fresh clone runs ten
checks in a few seconds:

1. `m0` — framing round-trips over a real pipe (loopback stub)
2. `m1` — boot to hello frame over the SBI console (RFC acceptance 1)
3. `m2` — monotonic timer ticks (acceptance 2); trap-ring query; deliberate
   kernel illegal instruction → structured fault → clean shutdown (acc. 3)
4. `m3` — a U-mode payload ELF runs to `sys_exit`; output tagged untrusted;
   a payload fault yields a structured `origin:"payload"` report and kills
   only the payload — the kernel survives and keeps answering
5. `m4` — capability enforcement (ENOCAP + `syscall_denied` event), spawn
   with attenuation (`granted ⊆ parent`, P10), instruction-count deadline
   kill of a runaway (`payload_killed`), post-suite liveness
6. `m5` — per-payload Sv39 paging: a payload reading kernel memory faults
   (isolation), a payload writing its own code faults (W^X), a `leaker`
   handing the kernel a kernel pointer via write() is refused (confused-
   deputy defense), each fault frame carries the page-table walk
7. `m6` — checkpoint/restore/fork (P8): a payload checkpoints itself
   mid-run; the kernel forks that checkpoint into independent continuations
   that each resume from the checkpoint point, not the top
8. `hardening` — 200 ticks contiguous under garbage serial input
9. `determinism` — two input-free boots byte-identical (P9 / E6 seed)
10. `demo` — the narrated `make demo` completes

CI (`.github/workflows/ci.yml`) runs the same gate + `ci/unsafe_budget.sh`
on every push. **Unsafe budget: 55/200 lines in 4/4 hal files** — the file
cap is fully used; new hal code must extend `hal/csr.rs` / `hal/boot.rs` /
`hal/trap.rs`, never add a 5th unsafe file.

Audits so far: M3+M4 (five reviewers) fixed one HIGH (`sscratch` desync) +
LOWs. M5 paging (five reviewers) fixed one HIGH (a confused-deputy leak in
the write syscall — the kernel would follow a payload's kernel pointer) and
one LOW (register scrub), both in the M6 commit. **M6 (checkpoint) is new and
not yet audited** — the deep-copy walk, __resume_user frame layout, and
snapshot/reap lifecycle are the spots to scrutinize next.

Toolchain: nightly-2026-07-14 (rust-toolchain.toml), QEMU 8.2.2
(`qemu-system-misc`), gdb-multiarch 15.1. `make build` builds the payload
workspace first (the kernel embeds their ELFs), then the kernel.

## What the operator can do (single serial command bytes)

`r` dump trap ring · `x` crash the kernel (illegal instr → shutdown) ·
`p` M3 payload suite · `m` M4 sandbox suite · `i` M5 isolation suite ·
`f` M6 checkpoint/fork suite.

## Known-broken / caveats

- Nothing known-broken.
- Payloads still run sequentially (run-to-completion); `yield` is a no-op
  reschedule and `spawn`'d children run after the parent. The full-frame
  suspend/resume switch now EXISTS (`hal::resume_user`, used by M6 restore),
  so real cooperative/preemptive scheduling is a small follow-up: save the
  running payload's frame on yield/preempt and resume another slot. Not wired
  yet because no milestone required it.
- Deadlines are in timebase units (deterministic instruction proxy under
  -icount), not exact retired-instruction counts — see DECISIONS.md.
- The budget script's SAFETY-comment walk is looser than clippy's; clippy's
  `undocumented_unsafe_blocks = deny` remains authoritative.

## Exact next step

**M7: deterministic replay green in CI (E6).** Per the RFC ladder. The kernel
is already deterministic under -icount (the `determinism` check proves two
input-free boots are byte-identical). M7 extends that to *operator input*:

1. Record: every control-plane input (the command bytes r/x/p/m/i/f, and
   later MCP requests) is logged with the icount/timebase at which it was
   consumed. The harness already sends these; capture them host-side with the
   guest's `time` for each, or have the kernel echo an `input` event.
2. Replay mode: a harness runner that re-feeds the recorded inputs and
   asserts the event stream is byte-identical to the recording — including
   the payload suites (which today aren't in the determinism check because
   command timing is host-paced). The subtlety: input delivery must be tied
   to instruction count, not wall clock, so replays line up. Options: drive
   input at deterministic icount points via the gdb stub, or make the kernel
   poll input only at fixed tick boundaries so delivery is quantized.
3. Acceptance (E6): record a full session (e.g. boot + `f` suite), replay it,
   diff the two event streams — must be identical. Add to `make test`.

This is mostly harness work + a small kernel `input` event; no new unsafe.
Before starting: **M6 deserves a dedicated adversarial audit** (deep-copy
walk, __resume_user, snapshot/reap lifecycle) — do that first.
