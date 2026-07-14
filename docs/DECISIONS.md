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
