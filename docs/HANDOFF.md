# Handoff

Rewritten at the end of every session. Assume the reader has zero context
beyond this repo (we dogfood E3 on ourselves).

## Current state (2026-07-14, session 2)

**M0–M5 complete and green.** `make test` from a fresh clone runs nine
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
   (isolation), a payload writing its own code faults (W^X), each fault
   frame carries the page-table walk, kernel survives both
7. `hardening` — 200 ticks contiguous under garbage serial input
8. `determinism` — two input-free boots byte-identical (P9 / E6 seed)
9. `demo` — the narrated `make demo` completes

CI (`.github/workflows/ci.yml`) runs the same gate + `ci/unsafe_budget.sh`
on every push. **Unsafe budget: 71/200 lines in 4/4 hal files** — the file
cap is fully used; new hal code must extend `hal/csr.rs` / `hal/boot.rs` /
`hal/trap.rs`, never add a 5th unsafe file.

M3+M4 were adversarially audited by five parallel reviewers; the one HIGH
(an `sscratch` desync race in `enter_user`) and several LOW findings were
fixed. M5 (paging) is new this session and has NOT yet had a dedicated
adversarial audit — that is a priority follow-up (the page-table code, the
satp-switch window in `enter_user`, and the frame reaper are the sensitive
spots).

Toolchain: nightly-2026-07-14 (rust-toolchain.toml), QEMU 8.2.2
(`qemu-system-misc`), gdb-multiarch 15.1. `make build` builds the payload
workspace first (the kernel embeds their ELFs), then the kernel.

## What the operator can do (single serial command bytes)

`r` dump trap ring · `x` crash the kernel (illegal instr → shutdown) ·
`p` M3 payload suite · `m` M4 sandbox suite · `i` M5 isolation suite.

## Known-broken / caveats

- Nothing known-broken.
- Payloads still run sequentially (run-to-completion); `yield` is a no-op
  reschedule and `spawn`'d children run after the parent. Address spaces are
  isolated now, but concurrent scheduling waits for the full-frame
  suspend/resume switch that M6's checkpoint machinery builds.
- `enter_user` does not scrub the register file, so a fresh payload could
  read a stale kernel register value. Low risk (no secrets yet); scrub when
  M8/M9 add sensitive state.
- Deadlines are in timebase units (deterministic instruction proxy under
  -icount), not exact retired-instruction counts — see DECISIONS.md.
- The budget script's SAFETY-comment walk is looser than clippy's; clippy's
  `undocumented_unsafe_blocks = deny` remains authoritative.

## Exact next step

**M6: checkpoint/restore + speculative fork (P8).** Per the RFC ladder. The
per-payload address space (M5) makes this tractable — the whole state is the
page table + the trap frame + the CapSet.

1. `snapshot(pid) -> blob`: serialize a payload's address space (walk its
   page table, copy each user frame + its VA/perms), register file (the saved
   trap frame), and CapSet into a versioned blob. A payload is snapshottable
   only when parked at a syscall boundary (its frame is saved) — add a
   `sys_checkpoint` or an operator command that parks it.
2. `restore(blob) -> pid`: allocate a fresh address space, re-map the frames
   from the blob, install the register file, resume.
3. `fork_from(blob)`: restore into a NEW pid so two continuations can run
   from the same checkpoint (the what-if verb, RFC demo 2).
4. Acceptance: snapshot a payload mid-run, restore it, and the restored
   payload produces the identical continuation (byte-identical events under
   -icount); fork produces two independent pids.

Blob format: postcard/CBOR would be self-describing (P5) but adds a
dependency; a raw versioned header keeps zero-dep. Decide and log.
Before starting: M5 deserves a dedicated adversarial audit first (page-table
code, satp window, frame reaper) — do that, then M6.
