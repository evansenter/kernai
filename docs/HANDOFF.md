# Handoff

Rewritten at the end of every session. Assume the reader has zero context
beyond this repo (we dogfood E3 on ourselves).

## Current state (2026-08-08, session 4)

**M0–M12 complete and green, and — beyond the ladder — full DOOM runs as a
sandboxed payload — and, since M13, the control plane is live while payloads
run.** `make test` from a fresh clone runs eighteen checks in
~11 seconds (fmt + clippy + build + unsafe budget first) and stays **pure Rust**:
the DOOM/C work is entirely behind off-by-default cargo features, so nothing
below changed and CI needs no C toolchain.

**DOOM (the `doom` feature, `make doom`).** Full doomgeneric DOOM (~80 C units,
picolibc, Freedoom IWAD) boots in U-mode, reads the IWAD from a kernel-mapped
window, shows the Freedoom title, and plays its attract-mode demo — real
first-person 3-D gameplay, HUD, enemies — rendered both as live ASCII frames and
as reassembled colour PNG screenshots (`harness/doom_frames/`). Memory-isolated,
FPU-off (fixed-point), and deterministic: two boots are byte-identical over
hundreds of frame checksums (P9), demo motion and all. See DECISIONS.md
(2026-07-19, two entries) and ARCHITECTURE.md for the design; the port files
live in `payloads/doom/` (`sys_kernai.c`, `doomgeneric_kernai.c`, `doom.ld`,
`build_doom.sh`) and `harness/doom.py`.

**And DOOM is agent-operable (`make doom-play`).** The RFC's agentic loop is
closed end-to-end on it: an agent starts DOOM **over MCP** (`tools/call
run_suite {suite:"doom"}`), **observes** it as structured events (`frame` ASCII
+ `fbchunk` colour keyframes via `SYS_FRAME`), **acts** through the kernel
(key bytes → the tick-drained key ring → `SYS_GETKEY` → `DG_GetKey` — the
scripted policy in `harness/doom_play.py` opens the menu, starts a new game,
and plays E1M1: walks, turns, fires), and **remediates** live (byte `0x03` =
operator kill → `payload_killed reason:"operator"`, the E2 seed), finishing
with a `resources/read processes` post-mortem that shows the killed payload.
The doom build's ABI self-describes in the `spec` resource (P5), and
`deep_copy` aliases the IWAD window so snapshot/fork (P8) composes with
windowed payloads. All of it is feature-gated: **no change to the default
kernel ABI or any acceptance check**.

The eighteen acceptance checks:

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
15. `e2` — live-incident MTTR mechanism (M13): a deadline-less `livelock`
    payload is killed WHILE IT RUNS via the MCP `kill` tool, serviced through
    an input-driven preemption; asserts the `sched preempt` event, the
    causally-anchored `payload_killed` (reason, `elapsed` — the
    time-to-mitigation fact), `suite_done killed:1`, and a live kernel after
16. `e3` — cold handoff: a fresh reader reconstructs state from the
    spec/processes/digest resources alone (P5/P11/P12)
17. `determinism` — two input-free boots byte-identical (P9 / E6 seed)
18. `demo` — the narrated `make demo` (now 11 acts, incl. MCP + two-surface)

CI (`.github/workflows/ci.yml`) runs the same gate + `ci/unsafe_budget.sh`
on every push. **Unsafe budget: 55/200 lines in 4/4 hal files** — the file
cap is fully used; new hal code must extend `hal/csr.rs` / `hal/boot.rs` /
`hal/trap.rs`, never add a 5th unsafe file. M8 and M9 added no unsafe (`rpc.rs`
is `#![forbid(unsafe_code)]`; M9 is more JSON fields + a runtime toggle).

Audits so far (each adversarial, via parallel reviewer subagents): M3+M4 fixed
one HIGH (`sscratch` desync). M5 paging fixed one HIGH (confused-deputy leak in
write) + a LOW. M6 (checkpoint) returned **clean**. M8 (the JSON-RPC reader):
the parser was proven panic/hang-free (~17.9M exhaustive + 500K random inputs,
zero panics); three real issues fixed in the M8-hardening commit — a
response-overflow silent hang, a command-eating desync, an invalid-numeric-id
echo. M10 (the P10 lattice) returned **clean** (no capability-widening path) + a
defense-in-depth re-clamp on restore. A final holistic pass (kernel correctness
+ docs/harness consistency) over the whole M0–M12 tree found **no HIGH bugs**;
its fixes (the `emit_fault` overflow fallback, an `elf.rs` checked_add, and a
batch of doc/SPEC accuracy fixes) are in the "Final holistic audit" commit. The
enriched fault frame's reachable worst case is ~1.9 KiB (a page fault) vs the
2 KiB cap, and `emit_fault` now falls back to a minimal frame on overflow, so a
fault report can never be silent (the one failure P6 forbids).

Toolchain: nightly-2026-07-14 (rust-toolchain.toml), QEMU 8.2.2
(`qemu-system-misc`), gdb-multiarch 15.1. `make build` builds the payload
workspace first (the kernel embeds their ELFs), then the kernel. The optional
`doom` feature additionally needs `riscv64-unknown-elf-gcc` + picolibc
(`--specs=…/picolibc.specs`) and a Doom IWAD (`apt-get install freedoom`, or set
`DOOM_WAD`); `cpayloads` needs clang + lld. Neither is required for `make test`.

**Note on session portability:** the remote container was reclaimed and
re-cloned mid-session and landed on a *stale* local checkout (M4) while origin
already had M7. `git fetch && git reset --hard origin/<branch>` restored it —
the pushed commits were the source of truth. Commit and push at every milestone;
the local working tree is not durable.

## What the operator can do

Single command bytes: `r` ring · `x` crash · `p`/`m`/`i`/`f`/`d`/`e` the
M3–M6/M10 suites + the M12 eval stimulus. With `--features cpayloads`, `c` runs
the C raycaster; with `--features doom`, `D` runs full DOOM (needs `-m 256M` +
the IWAD loaded as a device — `make doom` / `harness/doom.py` wires this). Or
drive it structured: a `0xAA`-led
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

**The M0–M13 ladder is complete and green, and full DOOM runs on it.** All
milestones and their acceptance checks are in `make test`; the twelve
principles P1–P12 each have a concrete, tested mechanism; the MCP surface is
published (`docs/SPEC.md`); DOOM (the `doom` feature) demonstrates the
isolation model on a large real-world C workload; and M13's input-driven
preemption makes the control plane live against running payloads — the `e2`
check remediates a livelocked payload mid-run via the `kill` tool, control
plane only.

Expansion directions, in the RFC's spirit (pick by value; none is blocking):

0. **DOOM polish (optional).** Input, kill, MCP-start, and P8-compatibility are
   DONE (`make doom-play`). Still open, all non-blocking: sound is dropped; the
   `fbchunk` keyframe stream is throttled by `COLOR_EVERY` but not governed by
   the P3 budget machinery (the autonomy dial governs trap frames only); DOOM
   itself stays un-preemptible while running (it owns the serial line for
   keystrokes — a deliberate M13 property, not a gap; every other payload gets
   the live plane); and a DOOM-checkpoint demo (fork a game mid-level into
   what-if continuations — the mm groundwork is in) would be a strong P8
   showcase.

1. **Grow the E1 stimulus set toward N≥20** (RFC E1). Add seeded faults with
   distinct diagnosis needs (unmapped-address load, misaligned access, a stack
   overflow, a divide trap) — each a tiny payload fixture — so the surface
   benchmark is broader. `harness/eval.py::BUGS` is the extension point.
2. **Full agent-in-the-loop E1.** The current `e1` is a deterministic
   surface-content proxy (facts-recoverable). The real experiment drives an LLM
   operator over each surface and measures localization rate/time/tokens; it
   needs model access, so it belongs in `make eval`/a separate harness, not CI.
3. **E2 mechanism: DONE (M13).** Input-driven preemption, the `kill` AND
   `set_budget` tools (both P4 named verbs; the `e2` check remediates the
   livelock both ways — direct kill, and budget-tightening where the kernel's
   own deadline mechanism does the ending), and the `livelock` pathology are
   in. What remains of E2 proper is the *measurement*: the A/B across surfaces
   with an LLM operator (same model-access dependency as item 2), plus a
   richer pathology set (runaway spawn loop, snapshot hog — new fixtures only,
   the verbs exist).
4. **A degraded reference implementation** (a Linux daemon speaking the same
   `docs/SPEC.md` surface — no true checkpoint/fork, but real) so the eval suite
   becomes a public benchmark for *both* kernel surfaces and operator agents.
5. **Remaining evals** E4/E5/E7/E8 as the RFC defines them.

Nothing is known-broken. Any of the above is a fresh, self-contained change;
start from a green `make test` and keep each milestone's check green.
