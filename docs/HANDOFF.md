# Handoff

Rewritten at the end of every session. Assume the reader has zero context
beyond this repo (we dogfood E3 on ourselves).

## Current state (2026-07-14, session 1)

**M0–M4 complete and green.** `make test` from a fresh clone runs eight
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
6. `hardening` — 200 ticks contiguous under garbage serial input
7. `determinism` — two input-free boots byte-identical (P9 / E6 seed)
8. `demo` — the narrated `make demo` (now M0–M4) completes

CI (`.github/workflows/ci.yml`) runs the same gate + `ci/unsafe_budget.sh`
on every push. **Unsafe budget: 56/200 lines in 4/4 hal files** — the file
cap is fully used; M5's paging code must extend `hal/csr.rs` / `hal/boot.rs`
/ `hal/trap.rs`, never add a 5th unsafe file.

Toolchain: nightly-2026-07-14 (rust-toolchain.toml), QEMU 8.2.2
(`qemu-system-misc`), gdb-multiarch 15.1. `make build` builds the payload
workspace first (the kernel embeds their ELFs), then the kernel.

## What the operator can do (single serial command bytes)

`r` dump trap ring · `x` crash the kernel (illegal instr → shutdown) ·
`p` run the M3 payload suite · `m` run the M4 sandbox suite.

## Known-broken / caveats

- Nothing known-broken.
- One payload is resident at a time (no paging); `yield` is a no-op
  reschedule and `spawn`'d children run after the parent (sequential). All
  three go away at M5 with per-payload address spaces.
- Deadlines are in timebase units (deterministic instruction proxy under
  -icount), not exact retired-instruction counts — see DECISIONS.md.
- The budget script's SAFETY-comment walk is looser than clippy's; clippy's
  `undocumented_unsafe_blocks = deny` remains authoritative.

## Exact next step

**M5: paging + isolation suite.** Per the RFC ladder. Concretely:

1. `hal/` gains Sv39 page-table types and a `satp` switch (extend
   `hal/csr.rs` + a new safe `mm.rs`/`paging` module above hal for the safe
   table-building logic; keep the raw satp write and TLB flush in hal). A
   `FrameAllocator` (safe, plain-data — P11) hands out physical frames.
2. Each process slot gains a page-table root; the loader maps the payload's
   segments into its own address space instead of the shared arena, so
   multiple payloads can be resident. The scheduler's single-resident
   assumption (payload.rs) and `enter_user`/`redirect_to_scheduler` grow a
   real suspend/resume switch (full-frame, in `hal/trap.rs` global_asm — no
   new inline-unsafe budget).
3. Isolation suite: a payload that reads/writes outside its map faults
   (store/load page fault, `origin:"payload"`); W^X enforced; one payload
   cannot see another's memory. Acceptance: the fault frame's page-table walk
   (the M9 P6 growth can start here) shows the bad access.

Before starting: re-read CLAUDE.md's milestone gate — M4 must stay green in
CI on every push, and the unsafe file cap (4) is already reached, so plan
M5's unsafe as edits to the existing hal files.
