# Handoff

Rewritten at the end of every session. Assume the reader has zero context
beyond this repo (we dogfood E3 on ourselves).

## Current state (2026-07-14, session 2)

**M0–M12 complete and green — the RFC ladder is finished.** `make test` from a
fresh clone runs seventeen checks in ~11 seconds (fmt + clippy + build + unsafe
budget first):

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
11. `m9` — rich diagnostic frames + the surface-classic twin (P6/E1): the
    agentic fault frame carries the full 31-register file + a P12 `caused_by`
    causal parent (the `payload_start` it descends from, threaded through the
    lifecycle events); the same fault on the classic surface (toggled via the
    `set_surface` tool) renders as one printf `[FAULT] …` console line
12. `m10` — multi-hop delegation attenuation (P10): a two-hop chain
    (`delegator{write,spawn,yield} → redelegator{write,spawn} → worker{write}`)
    where every hop greedily requests all caps still shrinks monotonically;
    `granted ⊆ parent` at each hop, never re-widening
13. `m11` — autonomy dial + P3 token-budgeted digest: the `digest` resource
    takes a `budget` and returns per-severity totals (ticks coalesced into a
    count) + at most `budget` notable traps, preserving the high-severity ones;
    the `set_autonomy` dial suppresses trace-severity tick frames at autonomous
    while still counting them (and checking deadlines)
14. `e1` — the RFC headline (surface-content proxy): over the seeded-fault set,
    the structured surface recovers every localization fact (17/17) and the
    classic printf twin far fewer (8/17) — the gap is the root-cause detail P6
    says decides debuggability (`make eval` prints the scorecard)
15. `e3` — cold handoff: a fresh reader reconstructs state from the
    spec/processes/digest resources alone (P5/P11/P12)
16. `determinism` — two input-free boots byte-identical (P9 / E6 seed)
17. `demo` — the narrated `make demo` (now 11 acts, incl. MCP + two-surface)

CI (`.github/workflows/ci.yml`) runs the same gate + `ci/unsafe_budget.sh`
on every push. **Unsafe budget: 55/200 lines in 4/4 hal files** — the file
cap is fully used; new hal code must extend `hal/csr.rs` / `hal/boot.rs` /
`hal/trap.rs`, never add a 5th unsafe file. M8 and M9 added no unsafe (`rpc.rs`
is `#![forbid(unsafe_code)]`; M9 is more JSON fields + a runtime toggle).

Audits so far: M3+M4 (five reviewers) fixed one HIGH (`sscratch` desync). M5
paging (five reviewers) fixed one HIGH (confused-deputy leak in write) + a LOW.
M6 (checkpoint) audit returned **clean**. M8 (the JSON-RPC reader) got a
three-reviewer audit: the parser was proven panic/hang-free (~17.9M exhaustive +
500K random inputs, zero panics), and three real issues were fixed in the
M8-hardening commit — a response-overflow silent hang, a command-eating desync,
and an invalid-numeric-id echo (see DECISIONS.md). M9 is additive diagnostic
emission (more JSON fields + a runtime surface toggle) with no untrusted parsing
and no new unsafe, so it was not separately audited; if anything, re-check that
the enriched fault frame (full register file + pagewalk + ring) still fits the
2 KiB FrameBuf on the worst-case payload fault (it does today, ~1.6 KiB).

Toolchain: nightly-2026-07-14 (rust-toolchain.toml), QEMU 8.2.2
(`qemu-system-misc`), gdb-multiarch 15.1. `make build` builds the payload
workspace first (the kernel embeds their ELFs), then the kernel.

**Note on session portability:** the remote container was reclaimed and
re-cloned mid-session and landed on a *stale* local checkout (M4) while origin
already had M7. `git fetch && git reset --hard origin/<branch>` restored it —
the pushed commits were the source of truth. Commit and push at every milestone;
the local working tree is not durable.

## What the operator can do

Single command bytes: `r` ring · `x` crash · `p`/`m`/`i`/`f`/`d`/`e` the
M3–M6/M10 suites + the M12 eval stimulus. Or drive it structured: a `0xAA`-led
length-prefixed frame
carrying JSON-RPC (MCP) — `initialize`, `tools/list`, `tools/call {run_suite|
crash|ring_read|set_surface|set_autonomy}`, `resources/list`, `resources/read
{trap_ring|processes|spec|surface|autonomy|digest}`. `run_suite` takes
`{suite: p|m|i|f|d|e}`; `digest` takes `{budget: N}`. See `harness/mcp.py` for the
client and `runner.py::m8`…`m11` for full sessions.

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

**The M0–M12 ladder is complete and green.** All twelve milestones and their
acceptance checks are in `make test`; the twelve principles P1–P12 each have a
concrete, tested mechanism; the MCP surface is published (`docs/SPEC.md`).

Expansion directions, in the RFC's spirit (pick by value; none is blocking):

1. **Grow the E1 stimulus set toward N≥20** (RFC E1). Add seeded faults with
   distinct diagnosis needs (unmapped-address load, misaligned access, a stack
   overflow, a divide trap) — each a tiny payload fixture — so the surface
   benchmark is broader. `harness/eval.py::BUGS` is the extension point.
2. **Full agent-in-the-loop E1.** The current `e1` is a deterministic
   surface-content proxy (facts-recoverable). The real experiment drives an LLM
   operator over each surface and measures localization rate/time/tokens; it
   needs model access, so it belongs in `make eval`/a separate harness, not CI.
3. **E2 (live-incident MTTR)** needs operator-initiated remediation (kill a
   payload, set a budget) *while it runs* — which needs preemptive scheduling.
   The suspend/resume primitive exists (`hal::resume_user`); wiring a real
   scheduler (save the running frame on preempt, resume another slot) unlocks
   E2 and a `kill`/`set_budget` control-plane tool (RFC P4's named tools).
4. **A degraded reference implementation** (a Linux daemon speaking the same
   `docs/SPEC.md` surface — no true checkpoint/fork, but real) so the eval suite
   becomes a public benchmark for *both* kernel surfaces and operator agents.
5. **Remaining evals** E4/E5/E7/E8 as the RFC defines them.

Nothing is known-broken. Any of the above is a fresh, self-contained change;
start from a green `make test` and keep each milestone's check green.
