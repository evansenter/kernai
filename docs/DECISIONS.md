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

**2026-07-14 · M2 · Corrections + fixes from the adversarial review pass.**
Six parallel reviewers (asm, kernel logic, harness, budget script, compliance,
docs) audited the tree; confirmed findings were fixed rather than argued with:
- *Budget scanner hardened*: it didn't know Rust raw strings — a crafted
  `r#"..."#` could desync it and hide real `unsafe` (verified exploit); the
  `#![forbid(unsafe_code)]` check also accepted the attribute inside a
  comment/string. Both fixed and covered by attack fixtures during review.
- *Harness watchdogs were per-frame, not total*: a kernel that kept ticking
  but never answered a query would hang `make test` forever. `_await`, the
  demo's `get`, and the demo subprocess now carry whole-wait deadlines.
- *FrameBuf grew 1 KiB → 2 KiB*: the worst-case fault frame (~1.1 KiB) could
  exceed 1 KiB and poison itself into silence — the one failure P6 forbids.
- *Panic handler* now masks interrupts around emission (no tick bytes
  interleaved into a dying kernel's last words), guards against recursive
  panic, and JSON-escapes the location.
- *Tick cadence made exact*: re-arm from the previous deadline instead of
  "now", so ticks land exactly TICK_INTERVAL apart (was +1–3 units of
  handler-latency drift per tick). Clamped forward to prevent catch-up
  storms.
- *asm contracts*: `wfi` and the deliberate illegal instruction dropped
  `nomem` (the trap handler they lead into mutates memory); the latter is
  `options(noreturn)` now.
- *Record corrections*: the M0 entry's "Makefile QEMU_BASE variable" never
  existed — the single home of the QEMU flags is `harness/qemu.py::qemu_args()`,
  which every make target routes through. The M2 entry's "~66 ecalls" was
  wrong: a tick frame is 50 wire bytes (≈50 putchar ecalls), a full ring dump
  ~420, a fault frame ~600 — all still ≪ the 500k-instruction tick interval.
  And "without_interrupts wraps every frame" overstated: hello is pre-enable
  and tick/fault are trap-context; only ring-dump/panic take the wrapper.
- *README bootstrap* gained `gcc` and `curl` (fresh Ubuntu lacks a host `cc`;
  CI runners had masked it). Harness pinned back to python ≥ 3.9 via
  `from __future__ import annotations`.

**2026-07-14 · M2 · PROVISIONAL · Kernel floating point is forbidden.**
The riscv64gc target has a hard-float ABI, but the kernel never enables
sstatus.FS, so OpenSBI's FS=Off means any FP instruction traps as
illegal_instruction — loudly, through our own structured fault path. That is
the enforcement: no FP in kernel code (none exists today; verified by
disassembly during review). If FP is ever wanted, the trap frame must first
grow F/D register save/restore and FS management. Revisit no earlier than M5.

**2026-07-14 · M3 · PROVISIONAL · Payloads are a separate cargo workspace, not part of the kernel crate.**
`payloads/` is its own workspace of U-mode fixtures (sys runtime + hello,
crasher, and the M4 set). They are userspace, not kernel code, so the hal
unsafe budget does not apply to them (ci/unsafe_budget.sh scans kernel/src
only); sys keeps its unsafe minimal regardless (entry asm + ecall shim). The
kernel embeds their ELFs via include_bytes! (paths from build.rs env vars),
so `make build` builds payloads first. Alternative — payloads inside the
kernel crate — would blur the unsafe boundary and the privilege boundary.

**2026-07-14 · M3 · Fixed identity-mapped payload arena at 0x80400000 (2 MiB) until paging.**
No paging until M5, so one payload is resident at a time at a fixed physical
address; payloads link there (payloads/link.ld) and the kernel asserts at
boot that its own image ends below it. Arena access is safe code:
hal::arena_{read,write,zero} bounds-check every access. This is the honest
pre-paging model — sequential execution, a run queue of pending images —
and it is enough to demonstrate P1/P7/P10 seeds without paging. Concurrent
resident payloads need per-address-space page tables (M5).

**2026-07-14 · M3 · Departing payloads return to the kernel by trap-frame rewrite, not a context switch.**
When a payload exits/faults/is-killed, the trap handler rewrites its saved
frame to resume in S-mode on the boot stack at the scheduler entry
(redirect_to_scheduler), and the normal trap-restore path performs the
switch. This needs no separate save/restore routine and keeps the only entry
into a payload a single enter_user (sret). Works because pre-paging no
payload is ever suspended-and-resumed — every payload runs start→terminal,
and preemption only kills. When M5 adds true suspend/resume for multiple
residents, a full-frame switch routine joins hal/trap.rs (global_asm, no new
inline-unsafe budget).

**2026-07-14 · M3/M4 · PROVISIONAL · Payloads are operator-triggered ('p','m'), not auto-run at boot.**
Boot drops straight to the idle command loop exactly as in M2, so the boot
event stream and the trap ring stay all-timer until the operator acts — the
M2 acceptance assertions remain valid unchanged. 'p' runs the M3 suite
(hello, crasher), 'm' the M4 suite (muzzled, spawner→child, runaway). This
also matches P1 (the operator decides when to spawn) and the DECISIONS
precedent that fault/ring are operator-triggered bytes.

**2026-07-14 · M4 · Capability denial and deadline kill are structured events, not just errnos.**
A refused syscall returns ENOCAP AND emits a `syscall_denied` event; an
over-budget payload emits `payload_killed`. The errno alone could be swallowed
by the payload; the event cannot. This is the P1/P6 stance — every policy
decision the kernel makes is observable to the operator — applied to the
sandbox surface.

**2026-07-14 · M4 · PROVISIONAL · Deadlines measured in timebase units via rdtime, not rdinstret.**
CLAUDE.md/P9 want instruction-count deadlines. Under -icount shift=1 the
timebase (rdtime) is a deterministic function of retired instructions
(1 unit ≈ 50 instructions), and rdtime is already proven to work from S-mode
here, so deadlines are expressed in timebase units and are a deterministic
instruction proxy. Verified: the runaway is killed at the identical point
every run under identical input timing. If an exact retired-instruction count
is ever needed, add rdinstret to hal/csr.rs (guarded by scounteren).

**2026-07-14 · M4 · PROVISIONAL · Cooperative yield is a no-op reschedule pre-paging.**
yield is capability-gated and emits a payload_yield event, but with one
resident payload there is nothing to switch to, so it resumes the caller.
Real rescheduling needs multiple resident payloads (M5). The syscall and its
cap gate are wired now so the ABI is stable; only the scheduler behaviour
changes at M5.

**2026-07-14 · M3/M4 · Fixes from the five-reviewer adversarial audit of the U-mode surface.**
Five parallel reviewers (trap/privilege asm, ELF loader, capability soundness,
hal unsafe/budget, acceptance rigor) audited the M3/M4 diff. Confirmed
findings were fixed:
- *HIGH — sscratch desync in enter_user*: enter_user armed sscratch while in
  S-mode with interrupts enabled (the scheduler runs with SIE=1). A timer in
  the window between the sscratch write and the sret was misclassified as
  from-U; __kernai_trap then reset sscratch to 0, so the payload entered with
  a desynced sscratch and its NEXT trap would build the kernel trap frame on
  the payload's own stack — a privilege-boundary break (reviewer reproduced a
  wedge with a hostile-sp payload). Fixed: enter_user now clears sstatus.SIE
  (`csrci sstatus, 2`) before arming sscratch; the sret restores SIE from
  SPIE=1, re-enabling interrupts atomically on entry to U-mode. Window closed.
- *LOW — trap vector misclassified a U-mode trap with user sp==0 as from-S*
  because it overloaded sscratch==0 as the from-S sentinel. Fixed: the vector
  now reconstructs the interrupted sp by testing sstatus.SPP (the definitive
  trapped-privilege bit), not the sscratch value.
- *LOW — stale CURRENT window*: after a redirect the scheduler runs with
  interrupts on while CURRENT still named the departed payload, so a timer's
  on_tick could spuriously kill an already-terminal slot. Fixed two ways:
  redirect_to_scheduler clears CURRENT in trap context before the sret, and
  on_tick ignores any slot not in RUNNING.
- *LOW — ELF loader overflow*: header-field arithmetic used unchecked +/*, so
  a crafted image panics under debug overflow-checks (the design intent is
  that adversarial images become structured events, not panics — M9 feeds
  this hostile input). Fixed: all offset math is checked; the program-header
  table is bounds-validated up front.
- *LOW — doc honesty*: README said M3 payloads run "with isolation" and the
  walkthrough said "fenced-off" memory. Pre-paging there is no MMU/PMP, so a
  U-mode payload can address all RAM; isolation today is privilege-level and
  fault-level, not memory-level (that lands at M5). Both reworded.
- *LOW — harness failure legibility*: _await now converts a silent-guest
  TimeoutError to a clean assertion; the runner catches KeyError/
  StopIteration/IndexError so a missing event reads as [FAIL], not a
  traceback; the demo M4 loop guards EOF. The payload_spawn event now carries
  the parent's CapSet and m4 asserts granted ⊆ parent directly (P10), instead
  of inferring it from the fixtures.
Reviewers confirmed no capability-widening path, sound arena bounds-checking,
correct budget accounting (now 59/200, 4/4 files), and zero flakiness
(m3 15/15, m4 15/15, deterministic deadline kill).
