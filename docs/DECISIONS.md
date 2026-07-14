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

**2026-07-14 · M5 · PROVISIONAL · Per-payload Sv39 address spaces; kernel is a supervisor gigapage in every table.**
Each payload gets its own root page table mapping its ELF segments at a low
user VA (0x10000) with W^X from the program headers, plus a single 1 GiB
supervisor gigapage identity-mapping kernel RAM (0x8000_0000, U=0). Rationale:
a trap doesn't switch satp, so the handler runs under the payload's table and
the kernel must be mapped in it — a supervisor (U=0) gigapage is one PTE, keeps
the kernel accessible to S-mode, and faults any U-mode access (the isolation
win). Payloads relink to a low VA so user and kernel VAs never overlap.
Alternative — a trampoline that switches satp on trap entry (xv6-style) — is
stricter (kernel not mapped in user tables at all) but needs an identically
mapped trampoline page; deferred, not needed for the isolation guarantee.

**2026-07-14 · M5 · Frame allocator is a bitmap; page tables built in safe code over hal::phys_*.**
A plain AtomicU64 bitmap over the pool (former arena region), alloc/free with
zeroing. Page-table reads/writes go through bounds-checked hal::phys_read_u64/
write_u64, so mm.rs and elf.rs stay #![forbid(unsafe_code)] — the only paging
unsafe is the satp write + sfence in hal (budget 71/200). Frames are reaped
when a payload terminates (scheduler destroys its address space under the
kernel satp, never while the payload table is active), so repeated runs don't
leak — verified across 3 back-to-back isolation suites.

**2026-07-14 · M5 · Payloads still run sequentially (run-to-completion); yield stays a no-op.**
Address spaces are now isolated, but the scheduler keeps the M4 sequential
model (one payload at a time, spawn'd children queued). True cooperative/
preemptive multitasking needs full-frame suspend/resume, which the checkpoint
machinery (M6) builds anyway — fold the switch in there. yield remains
cap-gated + eventful but resumes the caller. The M5 acceptance (isolation) is
independent of concurrent scheduling.

**2026-07-14 · M5 · Payload PHDRS force three page-aligned segments.**
An empty output section's ALIGN is dropped by lld, which let a payload's data
segment share a page with .text (and, worse, land in an R-only segment — a
non-writable stack that only trivial payloads survived). Fixed with explicit
PHDRS (text R+X, rodata R, data R+W) plus bare `. = ALIGN(0x1000)` statements
between segments, so every segment starts on its own page with correct W^X
permissions. The loader relies on this (one page is never mapped twice).

**2026-07-14 · M6 · PROVISIONAL · Checkpoint = deep-copy of the address space + saved trap frame + CapSet; blobs stay in-kernel (not host-serialized) for now.**
snapshot(pid) deep-copies the payload's Sv39 address space into independent
frames, saves its trap frame (the register file, captured at the sys_snapshot
ecall), and its CapSet into an in-kernel Snapshot table. restore/fork deep-copy
the snapshot again so continuations never share frames. The RFC's blob-to-host
serialization (postcard/CBOR, P5) is deferred — M6 proves the mechanism
(snapshot/restore/fork as primitives) in-kernel; the wire format lands with the
MCP layer (M8) where snapshot/restore become control-plane verbs returning
handles. fork()-style semantics: snapshot returns a positive id in the original
and 0 in each continuation, so a payload can branch (the what-if verb).

**2026-07-14 · M6 · enter_user unified onto __resume_user (full-frame resume); fresh starts scrub registers.**
Previously enter_user set sepc + a few CSRs and sret'd, leaving the general
registers holding kernel scheduler values (M5-audit LOW: cross-boundary leak).
M6 needs full-frame resume for restore anyway (a global_asm __resume_user that
loads all 31 GPRs + sepc + sstatus from a frame), so fresh starts now build a
ZEROED frame (sepc=entry, U-mode sstatus) and resume through the same path —
every GPR is loaded as 0, scrubbing the register file for free. Bonus: moving
the register loads into global_asm dropped the inline-unsafe budget from 71 to
55/200.

**2026-07-14 · M6 · Confused-deputy fix (M5-audit HIGH): kernel-on-behalf-of-user memory access enforces the U bit.**
The write syscall copied user bytes via a permission-agnostic translate(), so a
payload could hand the kernel a pointer into the kernel gigapage (U=0) or the
frame pool and have the kernel read it and emit it as output — an isolation
break the direct-access `wild` fixture never caught (that faults in hardware;
this used the kernel as a deputy). Fixed: translate_checked requires the leaf
to be user-accessible (U) plus the needed R/W bits before the kernel
dereferences a user pointer. The `leaker` fixture (write() of 0x80200000)
regression-tests it: the write is refused (EFAULT), no bytes leak.

**2026-07-14 · M7 · Deterministic input replay uses QEMU's built-in record/replay, not a kernel `input` event or gdb-driven injection.**
The HANDOFF speculated M7 would need the kernel to echo an `input` event and
the harness to quantize input delivery to instruction boundaries (or drive
input via the gdb stub) so replays line up. QEMU's `-icount rr=record` already
solves exactly this: it logs every non-deterministic input — serial bytes and
timer reads — with the instruction count at which the guest consumed it, and
`rr=replay` re-injects each at the identical instruction. So M7 needs **zero**
kernel change: record a live operator session to an `rrfile`, then replay with
no live input and assert the raw event frames are byte-identical. Two gotchas,
both logged in `harness/qemu.py`: (1) `sleep=off` (run-as-fast-as-possible)
hangs the rr main loop — rr drives the loop itself, so it is used only outside
record/replay; (2) a byte sent before the guest is up is dropped in record
mode, so the harness waits for the boot `hello` frame before driving input.
The recorded session drives all four suites (p/m/i/f) then crashes (x),
exercising input at four widely-separated instruction counts; replay reproduces
every frame bit-for-bit. This keeps the serial layer dumb (no echo) and the
kernel unchanged — determinism is a property of the QEMU invocation (P9), where
it belongs.

**2026-07-14 · M8 · PROVISIONAL · MCP/JSON-RPC rides inside the existing length-prefixed frames (not newline-delimited); each response is wrapped in a stream-id envelope.**
RFC open question: "MCP framing over virtio-serial: newline-delimited JSON-RPC
vs. length-prefixed." Chosen: length-prefixed, reusing the exact outbound frame
(`AA 99 | u32 LE len | UTF-8`) for inbound too — one framing for both
directions keeps the serial layer dumb (P-serial) and lets the host reuse
`encode_frame`. JSON-RPC 2.0 request objects go in the frame body. Responses
are emitted as ordinary event frames wrapped `{"id":<stream>,"type":"rpc","rpc":{…}}`:
the kernel's invariant that every frame carries a strictly-monotonic stream id
(the P12 causal spine and the determinism anchor) must hold for control traffic
too, so the JSON-RPC object — with its own correlation `id` — nests inside
`rpc`. A strict-MCP client reads `.rpc`; the envelope is our transport detail.
Alternative (make every event a JSON-RPC notification, no envelope) was rejected
for M8: it would rewrite every emit site and every runner check and risk
regressing M0–M7 for no functional gain. Revisit at M9+ if a real external MCP
client needs the pure stream.

**2026-07-14 · M8 · Single command bytes stay as a compat/fallback control plane alongside JSON-RPC.**
`idle` still honors `r/x/p/m/i/f`; a leading `0xAA` switches that byte into the
framed-request reader. Keeping both means M0–M7 checks, `make demo`, and the
determinism/replay gates are untouched, and the byte plane remains a
zero-dependency operator escape hatch. The two never collide: command bytes are
printable ASCII, `0xAA` cannot begin one.

**2026-07-14 · M8 · Transport-level junk is silently dropped and resynced; only structurally-valid JSON requests get a response.**
A stray `0xAA` in the byte stream (the input-hardening test injects `AA 99` +
garbage on purpose) must not hang or answer. The reader drops silently on: a
second byte that isn't `0x99`, an implausible length (0 or > 512 — and it does
NOT drain a bogus multi-GiB length, which would hang), non-UTF-8 bodies, and
bodies that aren't a JSON object. Only a well-formed frame carrying `{…}` is
dispatched, and only then are JSON-RPC-level problems (unknown method/tool/
suite) answered with an error object — because that frame *was* a real client
request. This preserves P-serial ("garbage bytes are ignored") and the
hardening invariant that noise produces only hello/tick frames, mirroring the
host decoder's skip-and-rescan on an implausible length.

**2026-07-14 · M8 · PROVISIONAL · `resources/read` returns the resource JSON as the result directly, not MCP's `contents[].text` string-wrapping.**
Strict MCP wraps a resource read as `{"contents":[{"text":"<stringified json>"}]}`.
Stringifying a whole JSON document (escaping every quote) in no_std with a fixed
FrameBuf is wasteful and error-prone, so `resources/read` returns the decoded
resource object as the JSON-RPC `result` directly (e.g. the `spec` object). The
wire spec we would publish uses direct JSON; a strict-MCP shim can re-wrap it
host-side. `tools/call run_suite` is likewise async: it returns
`{"status":"accepted"}` immediately and the suite's own event frames stream out,
with the terminal `suite_done` as the completion signal (the kernel's scheduler
`run()` diverges, so a synchronous "return the result after running" is not
possible without threading state through the scheduler — deferred).

**2026-07-14 · M8 · Idempotency via client `opId`: a replayed mutating call runs once (RFC P4 retry-safety).**
`tools/call` may carry `params.opId`. The kernel keeps an 8-slot FIFO of FNV-1a
hashes of seen opIds; a call whose opId is already present is answered
`{"status":"duplicate"}` without re-executing. Agents replay, double-fire, and
lose connections (RFC), so a `run_suite`/`crash` that arrives twice must not run
twice. 8 slots is a deliberate small window (the fixture double-fires
back-to-back); a production window would be larger and possibly response-caching,
logged here rather than blocking M8.

**2026-07-14 · M8 · Adversarial audit of the JSON-RPC surface (3 reviewers) — findings + fixes.**
The untrusted-input parser got a dedicated audit before M9. The structural JSON
reader was proven robust: a reviewer compiled a verbatim port and ran ~17.9M
exhaustive + 500K random inputs with overflow-checks on → zero panics, zero
hangs (every slice uses `.get(..)?`, every index is bounds-guarded). Three real
findings were fixed here:
- **(MED→HIGH) Response overflow → silent hang.** A well-framed request could
  carry a huge/control-heavy `id`; `write_json_escaped` expands each control
  byte 6× (`\u00xx`), overflowing the 2 KiB FrameBuf → `emit()` sends nothing →
  the client waits forever. Fixed two ways: `write_id` clamps the echoed id to
  64 chars, and `emit_rpc` now checks `FrameBuf::overflowed()` after building
  and, on overflow, emits a small fixed `"response too large"` error under the
  same stream id instead of nothing (a general net that also covers future
  `processes`-table growth; a build-time `assert!(MAX_PROC*128+128 < CAPACITY)`
  guards that too).
- **(MED) Command-eating desync.** A crafted `AA 99 <len≤512>` with a withheld
  body made the blocking reader consume the operator's subsequent command bytes
  until the frame completed. Fixed with a per-byte STALL bound (`getc_stall`,
  MAX_STALL_WAITS=16): a real client's bytes arrive steadily so a legit frame of
  any length never trips it, while a withheld body abandons after ~16 ticks and
  frees the command plane. A per-byte stall bound was chosen over a total
  instruction-count deadline because `wait_for_interrupt` advances virtual time
  ~one tick per call, so a total-time budget wrongly abandons legit multi-wait
  reads (observed: it wedged even a 40-byte `initialize`).
- **(LOW) Invalid-but-non-injecting numeric id.** `write_id`'s charset check
  passed `1.2.3` / `--`, emitting a syntactically invalid JSON number in our own
  response. Tightened to a strict integer grammar (optional `-` then digits),
  else `null`. No structural injection was ever possible (the charset excludes
  `"{}[],:`).
- **(LOW) `state_name` fallback** changed `_ => "empty"` to `"unknown"` (a
  never-stored state should not read as an empty slot).
The idempotency window (8 opId slots) and the id-fidelity note (a genuinely
escaped string id is re-escaped, so echo isn't byte-identical) were judged
acceptable and left as documented behavior. Regression coverage for all three
fixes was added to `runner.py::m8` (big-id answered + clamped, malformed numeric
id → null, withheld-body recovery).

**2026-07-14 · M9 · Fault frame grows the full register file + a P12 causal parent (`caused_by`).**
The v0 fault frame showed only ra/sp; P6 wants "the frame alone is sufficient
to localize the bug". M9 adds `regs`: all 31 GPRs, ABI-named (ra/sp/gp/tp/t0…/
a0…a7/s0…s11/t3…t6), so an agent has the argument/temp/saved state a debugger
would otherwise be needed for. It also adds `caused_by` (P12 causal spine): each
payload records its `payload_start` event id, and its output/exit/fault/killed
events — and the fault frame — name it, so the log is a DAG rooted at the start,
not a flat line. A spawned child's start names the parent's start (a delegation
chain is a real path); a top-level payload's `caused_by` is null (the operator,
who has no in-band event). A kernel fault's `caused_by` is null. The frame's
existing `cause` field (the scause hex) is unchanged and distinct from
`caused_by` (the parent event) — different axes (what vs. why-here).

**2026-07-14 · M9 · PROVISIONAL · The surface-classic twin is a runtime toggle on the diagnostic path, not a second build.**
E1 needs the SAME fault rendered two ways: the rich agentic frame vs. a classic
printf line. Implemented as a runtime surface selector (`traps::CLASSIC_SURFACE`)
toggled over the control plane (`set_surface {classic|agentic}` tool, `surface`
resource), so one binary serves both arms and an operator flips between them
mid-session — exactly the A/B E1 runs. On the classic surface `emit_fault`
renders one dense, unframed `[FAULT] origin cause pc=… stval=… ra=… sp=…` line
via `RawConsole` (no frame, no id, no register file, no page-table walk, no ring
— deliberately the poorer surface), which lands in the host decoder's noise
channel like a legacy kernel's printk. Scope note: M9 twins the DIAGNOSTIC path
(the E1 stimulus) only; the rest of the event stream stays structured. A fully
classic surface (printf for every event, errno returns, no events at all — the
RFC's `surface-classic`) is E-suite machinery deferred to M12, where the A/B is
run end-to-end. Chosen over a cargo feature (two builds) because a runtime
toggle keeps `make test` single-binary and lets one recorded session compare
both surfaces.

**2026-07-14 · M10 · Multi-hop delegation attenuation proven with a greedy chain; the control plane is not a capability source.**
M4 proved single-hop attenuation (`spawner → child`). M10 adds a two-hop chain
(`delegator {write,spawn,yield} → redelegator {write,spawn} → worker {write}`)
where every hop *requests all capabilities* (`sys::spawn(sel, !0)`), and shows
the grant still shrinks monotonically at each hop: `granted = requested &
parent_caps & image_ceiling ⊆ parent_caps`, always. Defense in depth: `enqueue`
also re-clamps `caps & image_ceiling`, so even the seed path can't over-grant.
Operator model decision: `spawn` stays a payload-only, `CAP_SPAWN`-gated
syscall; the MCP control plane exposes no `spawn`/`set_caps` tool, so there is
no path for the operator to mint or widen a capability — the control plane runs
pre-defined suites, it is not a capability authority. (If a future milestone
adds a control-plane spawn, it must carry its own ceiling and respect the same
lattice; logged here so the invariant isn't quietly broken.) Fixtures:
`delegator`/`redelegator`/`worker`; suite seeded by `seed_suite_m10`, driven by
the `d` command byte or MCP `run_suite {suite:"d"}`; asserted by `runner.py::m10`.

**2026-07-14 · M10 · Attenuation audit came back clean; one defense-in-depth fix + a logged non-issue.**
A dedicated adversarial audit of the P10 lattice (bitmask tricks, chain
monotonicity, forged parent/selector, snapshot/fork inheritance, slot reuse,
control-plane injection) found **no capability-widening path**: `granted ⊆
requested & parent_caps & image_ceiling` holds on every path, caps freeze into a
child slot at `enqueue` time, high bits (≥3) are erased by the ceiling AND, and
`spawnable_image` is a closed allowlist whose widest reachable ceiling can only
restrict. One hardening applied: `enqueue_restore` was the sole caps writer that
*copied* rather than re-derived, relying on an unstated "snapshot was already
clamped" invariant — now it re-clamps `& IMAGES[image].caps` explicitly, so the
⊆-ceiling invariant is self-defending.
Non-issue logged (PROVISIONAL): `SYS_SNAPSHOT` is not capability-gated. It's not
a lattice risk (snapshot copies the caller's own attenuated caps and a payload
cannot self-restore — fork is kernel-triggered via `AUTO_FORK`), only a bounded
resource matter (returns `EAGAIN`/`ENOMEM` on slot/frame exhaustion). Left
ungated: snapshot is a self-service checkpoint primitive, and gating it would
need a `CAP_SNAPSHOT` bit threaded through the images + fixtures for no security
gain today. Revisit if snapshot authority ever needs delegating.

**2026-07-14 · M11 · Autonomy dial + P3 token-budgeted digest, both control-plane toggles; ticks suppressed at the frame layer only.**
P3 ("operator attention is the scarce resource"): observability endpoints
accept a budget and return a summary, not a firehose. Implemented as a `digest`
resource that takes a `budget` param and returns per-severity totals (every
timer tick collapsed into a count) plus at most `budget` of the most-recent
NOTABLE traps (faults, syscalls), newest first, with an `elided` count. A small
tick-proof NOTABLE ring (4 entries) guarantees a fault stays surfaced no matter
how many ticks follow it, and severity ranking (fault=error, ecall=info,
tick=trace) means a small budget preserves the high-severity events. The
autonomy dial (`set_autonomy {reactive|autonomous}`, mirror of `set_surface`):
at autonomous the kernel judges trace-severity tick FRAMES not worth the
operator's token budget and suppresses them from the wire — but only the frame
emission is gated; `TICKS` still increments, the timer is still re-armed, and
payload deadline checks still run, so determinism and the deadline kill are
untouched. Default is reactive, so every existing check (m2/hardening/
determinism, which depend on tick frames) is unaffected; only `m11` flips the
dial. Chosen a runtime dial over a build flag for the same reason as the M9
surface twin: one binary, one recorded session can exercise both modes.
