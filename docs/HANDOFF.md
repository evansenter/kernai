# Handoff

Rewritten at the end of every session. Assume the reader has zero context
beyond this repo (we dogfood E3 on ourselves).

## Current state (2026-07-14, session 2)

**M0–M7 complete and green.** `make test` from a fresh clone runs eleven
checks in ~5 seconds (fmt + clippy + build + unsafe budget first):

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
9. `m7` — deterministic replay of a full operator session (E6): record a
   live session (all four suites p/m/i/f, then the `x` crash) to a QEMU
   record/replay log; replay it with NO live input; the two raw event
   streams are byte-identical (every serial byte re-injected at its exact
   recorded instruction count)
10. `determinism` — two input-free boots byte-identical (P9 / E6 seed)
11. `demo` — the narrated `make demo` completes

CI (`.github/workflows/ci.yml`) runs the same gate + `ci/unsafe_budget.sh`
on every push. **Unsafe budget: 55/200 lines in 4/4 hal files** — the file
cap is fully used; new hal code must extend `hal/csr.rs` / `hal/boot.rs` /
`hal/trap.rs`, never add a 5th unsafe file.

Audits so far: M3+M4 (five reviewers) fixed one HIGH (`sscratch` desync) +
LOWs. M5 paging (five reviewers) fixed one HIGH (a confused-deputy leak in
the write syscall) and one LOW (register scrub), both in the M6 commit. M6
(checkpoint) — adversarial audit came back **clean** (deep-copy independence
across 40 runs, no cross-continuation frame sharing, register scrub holds,
confused-deputy holds against a 12-address battery, graceful exhaustion).
M7 is harness-only (QEMU record/replay); no new unsafe, nothing to audit.

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
  suspend/resume switch EXISTS (`hal::resume_user`, used by M6 restore), so
  real cooperative/preemptive scheduling is a small follow-up: save the
  running payload's frame on yield/preempt and resume another slot. Not wired
  yet because no milestone required it.
- Deadlines are in timebase units (deterministic instruction proxy under
  -icount), not exact retired-instruction counts — see DECISIONS.md.
- The budget script's SAFETY-comment walk is looser than clippy's; clippy's
  `undocumented_unsafe_blocks = deny` remains authoritative.

## Exact next step

**M8: MCP control plane + resources (P4).** Per the RFC ladder. Today the
control plane is single command bytes in / JSON event frames out. M8 makes the
inbound side structured too — JSON-RPC (MCP shape) rides *inside* the existing
length-prefixed frames, so the serial layer stays dumb (P-serial rule holds).

Plan:
1. Inbound frames: the harness sends length-prefixed JSON-RPC requests instead
   of bare bytes. The kernel grows a tiny request reader on the serial input
   path (parse a frame, dispatch a method). Keep the single-byte commands
   working as a fallback so `make demo` and the existing checks don't break, or
   migrate them — decide and log in DECISIONS.md.
2. Methods map to today's verbs: `tools/call run_suite{p|m|i|f}`, `crash`,
   `ring/read`. Responses carry a request id so calls correlate (P12 parent id
   is the seed for this).
3. Resources (MCP `resources/*`): expose `trap_ring` and the process table as
   readable resources — `resources/read trap_ring` returns the ring, the
   process table lists live payloads + caps. This is the natural home for the
   snapshot handles M6 deferred (snapshot/restore/fork become control-plane
   verbs returning resource handles).
4. Acceptance: a harness check that drives the kernel purely over JSON-RPC —
   call a tool, read a resource, assert the structured response. Add to
   `make test`. Keep it deterministic (goes through the same `qemu.py`).

No new unsafe expected — this is a parser + dispatcher in safe kernel code plus
harness work. A JSON parser in `no_std` with no alloc is the one real cost:
either a tiny hand-rolled scanner for the fixed request shapes (recommended —
the request grammar is small and fixed) or a `serde`/`nanoserde` no_std path.
Log the choice in DECISIONS.md.

After M8: M9 (rich diagnostic frames + the `surface-classic` printf twin for
the E1 A/B), M10 (delegation/attenuation hardening), M11 (autonomy dial), M12
(the E1–E8 eval suite). Then expand past the ladder per the RFC's spirit.
