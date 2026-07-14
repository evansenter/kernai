# Decisions

Append-only. Format: date, milestone, decision, alternatives, why.
`PROVISIONAL` marks answers to RFC open questions logged to unblock work —
revisit deliberately, don't drift.

---

**2026-07-14 · M0 · PROVISIONAL · Wire framing: magic + length-prefix, JSON payloads.**
RFC open question: newline-delimited JSON-RPC vs length-prefixed. Chose
length-prefixed (`0xAA 0x99 | u32 LE len | payload`) with a 2-byte non-ASCII
magic whose only job is resyncing past OpenSBI banner noise on the shared
UART. Alternatives: NDJSON (fragile if a payload ever embeds a newline or the
UART carries non-event noise), CRC-protected frames (cleverness; QEMU virtual
serial is lossless). Payloads are UTF-8 JSON events. Revisit when MCP lands
(M8) — JSON-RPC messages ride inside these frames unchanged.

**2026-07-14 · M0 · Make, not just.**
CLAUDE.md allows either. `make` is preinstalled on dev boxes and CI runners;
`just` would be one more bootstrap step for zero current benefit.

**2026-07-14 · M0 · Toolchain pinned to nightly-2026-07-14.**
Latest nightly at project start; prebuilt `core` for
riscv64gc-unknown-none-elf (no build-std). Components: rust-src, clippy,
rustfmt. Bump deliberately, never implicitly.

**2026-07-14 · M0 · Harness is stdlib-only Python.**
No pip, no venv, no third-party deps. `make test` from a fresh clone needs
only python3 ≥ 3.9 — one fewer flake source in the layer that must be boring.

**2026-07-14 · M0 · icount shift=1,sleep=off everywhere.**
P9. shift=1 (2ns of virtual time per instruction) keeps virtual time close to
the 10 MHz timebase granularity while running fast. All Makefile QEMU targets
share one QEMU_BASE variable so no invocation can drift from it.

**2026-07-14 · M0 · unsafe budget enforced by static parse, not cargo geiger.**
`ci/unsafe_budget.sh` embeds a small Python scanner (comments/strings
stripped, brace-matched spans) so it runs toolchain-free before any build.
clippy's `undocumented_unsafe_blocks = deny` (kernel Cargo.toml) remains the
authoritative SAFETY-comment check; the RFC's `cargo geiger` suggestion adds
a dependency for little over this and can be revisited at M5+.

**2026-07-14 · M1 · PROVISIONAL · SBI legacy console putchar, not DBCN.**
Both exist in QEMU's OpenSBI 1.3. Legacy putchar (EID 0x01) is one
byte per ecall — slow but universal and impossible to misuse; DBCN batching
is an optimization the dumb serial layer doesn't need yet. Revisit when
frame volume grows (M8), behind the same `console_putchar` seam.

**2026-07-14 · M1 · PROVISIONAL · Frames ride the SBI console UART until M8.**
The RFC says "MCP over virtio-serial"; a virtio driver is real work that
buys nothing before the MCP layer exists. The harness framing is
transport-agnostic (FrameStream reads any fd), so swapping the UART for
virtio-serial at M8 touches no framing or event code. Until then `-serial
stdio` is the wire.

**2026-07-14 · M1 · Kernel has zero crate dependencies.**
core only — no riscv/sbi/spin crates. The unsafe they'd wrap is exactly the
unsafe we're budgeting in hal/, and vendored abstractions would hide it from
ci/unsafe_budget.sh. Costs us ~40 lines of hand-written ecall/CSR wrappers.

**2026-07-14 · M1 · FrameBuf overflow poisons the frame instead of truncating.**
A truncated JSON diagnostic parses as garbage or, worse, parses clean with
missing fields; an absent frame is unambiguous. Buffer is 1 KiB; events are
designed small (P3 — operator attention is metered).

**2026-07-14 · M2 · PROVISIONAL · Fault injection and ring queries are operator-triggered bytes ('x', 'r'), not tick-count-triggered.**
Alternatives: self-inject after N ticks (deterministic but couples acceptance
timing to host stdin delivery — under icount the guest outruns wall clock, so
any fixed N races the harness), or no query path at all. Single command bytes
over the existing serial line keep the guest's liveness independent of host
timing, exercise a real input path, and are the embryo of P1's external
control plane. Replay of operator inputs (P9 full story) lands at M7.

**2026-07-14 · M2 · Frame emission runs entirely under interrupts-disabled.**
`without_interrupts` wraps id allocation + byte output for every frame. Cost:
tick latency can stretch by one frame emission (~66 ecalls ≪ the 500k-insn
tick interval). Buys two load-bearing invariants: frames never interleave on
the wire, and event ids strictly increase in stream order — asserted
stream-wide by the acceptance suite, and the substrate P12's causal graph
will stand on.

**2026-07-14 · M2 · Trap ring is atomics-per-field, not a lock.**
A proper Mutex needs UnsafeCell (unsafe outside hal) or a dependency. Per-field
relaxed atomics are safe code; consistency is structural: the only writer runs
in the trap handler (interrupts hardware-disabled), readers snapshot under
without_interrupts, single hart (SMP is a non-goal). If SMP ever stopped
being a non-goal this is the first thing to revisit.

**2026-07-14 · M2 · Deliberate illegal instruction is `csrrw x0, cycle, x0` (0xc0001073).**
Alternatives: `.word 0` or all-ones (guaranteed illegal but decode to
nothing instructive). Writing the read-only cycle counter is illegal per the
privileged spec AND decodes into meaningful fields — the fault report can
show opcode=SYSTEM, csr=0xc00 and thereby *why* it trapped. Better P6 demo,
same one instruction.

**2026-07-14 · M2 · Battle-testing scope: hardening + determinism + demo in `make test`.**
Session instruction "completely battle tested, with demos" interpreted as:
harden and demo M0–M2, do NOT start M3 (original brief was explicit:
"Stop there"). Added to the permanent gate: 200-tick monotonicity under
garbage input, byte-identical double-boot (E6 seed), and the narrated demo
itself. The full gate stays under ~5s so nobody is tempted to skip it.
