# Handoff

Rewritten at the end of every session. Assume the reader has zero context
beyond this repo (we dogfood E3 on ourselves).

## Current state (2026-07-14, session 2)

**M0–M9 complete and green.** `make test` from a fresh clone runs thirteen
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
11. `m9` — rich diagnostic frames + the surface-classic twin (P6/E1): the
    agentic fault frame carries the full 31-register file + a P12 `caused_by`
    causal parent (the `payload_start` it descends from, threaded through the
    lifecycle events); the same fault on the classic surface (toggled via the
    `set_surface` tool) renders as one printf `[FAULT] …` console line
12. `determinism` — two input-free boots byte-identical (P9 / E6 seed)
13. `demo` — the narrated `make demo` (now 11 acts, incl. MCP + two-surface)

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

Single command bytes: `r` ring · `x` crash · `p`/`m`/`i`/`f` the M3–M6 suites.
Or drive it structured: a `0xAA`-led length-prefixed frame carrying JSON-RPC
(MCP) — `initialize`, `tools/list`, `tools/call {run_suite|crash|ring_read|
set_surface}`, `resources/list`, `resources/read {trap_ring|processes|spec|
surface}`. See `harness/mcp.py` for the client and `runner.py::m8`/`m9` for full
sessions.

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

**M10: delegation/attenuation hardening (P10).** Per the RFC ladder. The
attenuation lattice (`granted = requested & parent_caps & image_ceiling`) is
enforced in `payload::on_spawn`, and the M4 audit already covered the direct
spawn path. M10 pushes on it adversarially:

1. Deeper chains: today `spawner → child` is one level. Add a fixture that
   spawns a grandchild (a child that itself spawns), and assert caps can only
   ever shrink along the whole chain — never re-widen at any hop.
2. The MCP surface as an attenuation vector: can a control-plane caller grant
   caps a payload couldn't grant itself? (Today `spawn` is payload-only; if M10
   adds a `spawn` *tool*, it must respect the same lattice, and probably a
   control-plane ceiling.) Decide and log whether the operator is “root” or is
   itself capability-bounded.
3. Adversarial audit (this is a security milestone — follow the M4/M5/M8
   pattern): a fixture battery that tries to widen caps via spawn ordering,
   integer tricks on the caps bitmask, reused/forged pids, and the snapshot/
   fork path (does a forked continuation inherit exactly the parent’s CapSet,
   no more?). 
4. Acceptance: an `m10` check proving a multi-hop delegation chain monotonically
   attenuates and every widening attempt is refused. Add to `make test`.

Then M11 (autonomy dial + P3 token-budgeted event summaries — `?budget=Ntok`
returns a coalesced digest, not the firehose) and M12 (the E1–E8 eval suite:
wire the M9 two-surface toggle into an actual A/B harness with a seeded-bug
stimulus set, plus E2/E3/E5). After the ladder, expand per the RFC's spirit —
the eval suite as a public benchmark, the MCP surface published as the spec.

Then M10 (delegation/attenuation hardening — an adversarial pass on the P10
lattice and the MCP surface), M11 (autonomy dial + P3 event budgets:
`?budget=Ntok` summaries), M12 (the E1–E8 eval suite + demos). After the
ladder, expand per the RFC's spirit (E-series evals as a public benchmark, the
protocol as the durable artifact).
