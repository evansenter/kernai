# Handoff

Rewritten at the end of every session. Assume the reader has zero context
beyond this repo (we dogfood E3 on ourselves).

## Current state (2026-07-14, session 2)

**M0–M8 complete and green.** `make test` from a fresh clone runs twelve
checks in ~8 seconds (fmt + clippy + build + unsafe budget first):

1. `m0` — framing round-trips over a real pipe (loopback stub)
2. `m1` — boot to hello frame over the SBI console (RFC acceptance 1)
3. `m2` — monotonic timer ticks (acceptance 2); trap-ring query; deliberate
   kernel illegal instruction → structured fault → clean shutdown (acc. 3)
4. `m3` — a U-mode payload ELF runs to `sys_exit`; output tagged untrusted;
   a payload fault yields a structured `origin:"payload"` report, kernel lives
5. `m4` — capability enforcement (ENOCAP + `syscall_denied`), spawn with
   attenuation (P10), instruction-count deadline kill, post-suite liveness
6. `m5` — per-payload Sv39 paging: kernel-read fault (isolation), self-write
   fault (W^X), a `leaker` refused a kernel pointer (confused-deputy), each
   fault frame carries the page-table walk
7. `m6` — checkpoint/restore/fork (P8): a payload checkpoints itself mid-run;
   the kernel forks independent continuations that resume from the checkpoint
8. `hardening` — 200 ticks contiguous under garbage serial input (now incl.
   the frame magic `AA 99` as noise — the M8 reader drops it silently)
9. `m7` — deterministic replay of a full operator session (E6): record a live
   session to a QEMU record/replay log, replay with no live input, the raw
   event streams are byte-identical
10. `m8` — MCP control plane (P4): the kernel is driven purely over JSON-RPC
    2.0 inside the frames — `initialize`, `tools/list`, `resources/list`/`read`
    (incl. the P5 self-describing `spec`), an async `tools/call run_suite`
    whose events stream to `suite_done`, client-`opId` idempotency (a replayed
    mutating call runs once), structured errors, all with stream ids strictly
    monotonic across control + event frames
11. `determinism` — two input-free boots byte-identical (P9 / E6 seed)
12. `demo` — the narrated `make demo` (now 10 acts, incl. the MCP act)

CI (`.github/workflows/ci.yml`) runs the same gate + `ci/unsafe_budget.sh`
on every push. **Unsafe budget: 55/200 lines in 4/4 hal files** — the file
cap is fully used; new hal code must extend `hal/csr.rs` / `hal/boot.rs` /
`hal/trap.rs`, never add a 5th unsafe file. M8 added no unsafe (`rpc.rs` is
`#![forbid(unsafe_code)]`: a hand-rolled structural JSON reader, no serde).

Audits so far: M3+M4 (five reviewers) fixed one HIGH (`sscratch` desync). M5
paging (five reviewers) fixed one HIGH (confused-deputy leak in write) + a LOW.
M6 (checkpoint) audit returned **clean**. **M8 (the JSON-RPC reader) is new and
should be audited next** — it parses untrusted host input; scrutinize the
structural scanner (`skip_container`/`object_get` bounds, unbalanced/nested/
truncated inputs), the frame-length/desync handling (a bogus length must never
hang or over-read), the id-echo escaping (no structure injection through the
correlation id), and the idempotency window.

Toolchain: nightly-2026-07-14 (rust-toolchain.toml), QEMU 8.2.2
(`qemu-system-misc`), gdb-multiarch 15.1. `make build` builds the payload
workspace first (the kernel embeds their ELFs), then the kernel.

**Note on session portability:** the remote container was reclaimed and
re-cloned mid-session and landed on a *stale* local checkout (M4) while origin
already had M7. `git fetch && git reset --hard origin/<branch>` restored it —
the pushed commits were the source of truth. Commit and push at every milestone;
the local working tree is not durable.

## What the operator can do

Single command bytes: `r` ring · `x` crash · `p`/`m`/`i`/`f` the M3–M6 suites.
Or drive it structured: a `0xAA`-led length-prefixed frame carrying JSON-RPC
(MCP) — `initialize`, `tools/list`, `tools/call {run_suite|crash|ring_read}`,
`resources/list`, `resources/read {trap_ring|processes|spec}`. See
`harness/mcp.py` for the client and `runner.py::m8` for a full session.

## Known-broken / caveats

- Nothing known-broken.
- Payloads still run sequentially (run-to-completion). The full-frame
  suspend/resume switch exists (`hal::resume_user`, M6); real cooperative/
  preemptive scheduling is a small follow-up, not yet wired (no milestone
  required it). This is why `tools/call run_suite` is async (acknowledge, then
  the scheduler `run()` diverges and streams events) rather than returning the
  suite result synchronously.
- `resources/read` returns the resource JSON as the result directly, not MCP's
  `contents[].text` string-wrapping (DECISIONS.md — avoids stringifying a whole
  document in no_std; a strict-MCP shim can re-wrap host-side).
- opId idempotency window is 8 entries (a small FIFO of FNV-1a hashes); a
  production window would be larger and possibly response-caching.
- Deadlines are in timebase units (deterministic instruction proxy under
  -icount), not exact retired-instruction counts — see DECISIONS.md.

## Exact next step

**M9: rich diagnostic frames + the `surface-classic` twin (P6, E1).** Per the
RFC ladder. The fault frame is already the v0 P6 diagnostic (cause, sepc,
decoded instruction, ring history, page-table walk). M9:

1. Grow the frame: the full register file (not just ra/sp), a `cause` parent
   event id (P12 — link the fault to the syscall/tick that preceded it), and
   any symbolization the payload ELF affords (map sepc → segment/offset).
2. Build the `surface-classic` twin: the SAME kernel able to emit a
   traditional unstructured printf-style log line for the same fault, behind a
   build flag or an MCP toggle. This is the A/B substrate for **E1** (does an
   agent localize a bug faster from structured frames than a log dump?).
3. Acceptance: an M9 check asserting the enriched frame's new fields and that
   the classic twin carries the same underlying facts in prose. Add to
   `make test`.

Then M10 (delegation/attenuation hardening — an adversarial pass on the P10
lattice and the MCP surface), M11 (autonomy dial + P3 event budgets:
`?budget=Ntok` summaries), M12 (the E1–E8 eval suite + demos). After the
ladder, expand per the RFC's spirit (E-series evals as a public benchmark, the
protocol as the durable artifact).
