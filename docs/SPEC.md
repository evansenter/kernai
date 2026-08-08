# kernai surface spec v0.1

The agent-facing contract: the wire framing, the event stream, and the MCP
control plane. This is the durable artifact the RFC (`RFC-001`, §"durable
artifact") calls for — a versioned surface that another implementation (e.g. a
degraded Linux daemon) could target, with kernai as the reference. Principles
cited as `P#` are defined in RFC-001.

The kernel emits its own machine-readable version of this at runtime:
`resources/read {"uri":"spec"}` (P5 — the self-describing surface). This
document is the human-readable superset.

## 1. Transport framing (P-serial)

One length-prefixed frame, little-endian, in **both** directions:

```
+------+------+--------------+-------------------+
| 0xAA | 0x99 |  len: u32 LE |  payload (len B)  |
+------+------+--------------+-------------------+
```

- The two magic bytes let a reader lock onto the first frame in a stream that
  begins with the OpenSBI boot banner (they cannot occur in ASCII). No checksum
  — QEMU's virtual serial is lossless, and cleverness here contaminates every
  downstream measurement.
- Payloads are UTF-8 JSON. Outbound = events (§2) and RPC responses (§3).
  Inbound = JSON-RPC requests (§3) or single command bytes (§4).
- Bytes that aren't a valid frame are **noise**: silently skipped by the host
  decoder, silently dropped + resynced by the kernel reader (bounded by a
  per-byte stall). The classic diagnostic surface (§5) writes to this noise
  channel deliberately.

## 2. Event stream

Every outbound frame is a JSON object with a monotonic stream `id` (unique,
strictly increasing in emission order — the P12 causal spine and the
determinism anchor) and a `type`. Ids may have gaps (a suppressed tick still
consumes one) but never repeat or decrease.

| type | key fields | meaning |
|------|-----------|---------|
| `hello` | `name`, `proto` | boot handshake; always `id` 0 |
| `tick` | `seq`, `time` | timer heartbeat (P9: `time` is icount-derived). Suppressed on the wire under autonomous autonomy (§4) |
| `trap_ring` | `count`, `entries[]` | ring dump (P11) |
| `payload_start` | `pid`, `name`, `entry`, `caps[]`, `caused_by`, `restored` | a workload began; `caused_by` = the parent's start event or null (P12) |
| `payload_output` | `pid`, `untrusted:true`, `caused_by`, `len`, `data` | workload stdout — always tagged untrusted, bytes confined to a JSON string (P7) |
| `payload_exit` | `pid`, `code`, `caused_by` | clean exit |
| `payload_spawn` | `parent`, `child`, `name`, `parent_caps[]`, `requested[]`, `granted[]`, `attenuated` | delegation; `granted ⊆ requested & parent_caps & ceiling` (P10) |
| `syscall_denied` | `pid`, `syscall`, `cap`, `reason` | a capability-gated call was refused (P1: a refusal is an event) |
| `payload_killed` | `pid`, `reason`, `elapsed`, `deadline?`, `caused_by` | a payload was ended by policy: `reason:"deadline"` (instruction budget, P2, carries `deadline`) or `reason:"operator"` (the `kill` tool / doom kill key; `elapsed` = timebase units since start — the E2 MTTR fact; `caused_by` null if killed before its start event) |
| `sched` | `action:"preempt"`, `pid`, `reason:"pending_input"`, `caused_by` | M13: the scheduler suspended a payload to service pending control-plane input; it resumes silently afterward (P11: the scheduler explains itself) |
| `snapshot` | `pid`, `snapshot` | a checkpoint was taken (P8) |
| `payload_load_fault` | `pid`, `name`, `reason` | a payload's ELF failed to load / map (e.g. out of memory) |
| `payload_yield` | `pid` | a `Cap::Yield`-holding payload yielded |
| `fault` | see §2.1 | the P6 diagnostic frame |
| `frame` | `pid`, `untrusted:true`, `w`, `h`, `rows[]`, `checksum` | a payload framebuffer (via `blit`), rendered as an ASCII grid + an FNV checksum — the display-as-event seam (deterministic under -icount) |
| `suite_done` | `exited`, `faulted`, `killed` | a suite drained |
| `panic` | `location`, `msg` | a kernel panic (a bug); reported structured, then shutdown |
| `rpc` | `rpc:{…}` | a control-plane response envelope (§3) |

### 2.1 The fault frame (P6)

The design goal (P6): **the frame alone is sufficient for a competent agent to
localize the bug, no debugger.** Fields:

- `origin`: `"payload"` (kernel survives) or `"kernel"` (shutdown follows).
- `pid`, `caused_by`: the workload and the `payload_start` it descends from (P12).
- `cause` (raw scause hex), `cause_name` (decoded), `sepc`, `stval`.
- `ra`, `sp`: the return address and stack pointer, top-level for convenience
  (also present inside `regs`).
- `regs`: all 31 GPRs, ABI-named (`ra`,`sp`,…,`t6`).
- `overflow` (only if the enriched frame didn't fit the 2 KiB frame — then the
  rich fields are dropped and only `origin`/`cause_name`/`sepc`/`stval` remain,
  so a fault report is never silent).
- `insn`: decoded offending instruction (opcode/funct/rd/rs/`csr`) for illegal
  instructions, else null.
- `pagewalk`: per-level Sv39 PTEs with decoded `v/r/w/x/u` flags for page
  faults, else null — shows *which* permission was violated.
- `ring`: the last 8 traps (the flight recorder).

The `surface-classic` twin (§5) renders the same fault as one printf line
carrying only `origin`, `cause_name`, `sepc`, `stval`, `ra`, `sp` — the E1 A/B.

## 3. Control plane (MCP / JSON-RPC 2.0, P4)

A request is a framed JSON-RPC object: `{"jsonrpc":"2.0","id":<id>,"method":…,
"params":…}`. The response rides in an event envelope so it keeps a stream id:

```json
{"id":<stream>,"type":"rpc","rpc":{"jsonrpc":"2.0","id":<echoed>,"result":…}}
```

Errors use `"error":{"code":,"message":}` (JSON-RPC codes: -32600 invalid,
-32601 method not found, -32602 invalid params; -32001 = response too large).

### Methods

- `initialize` → `{protocolVersion, serverInfo:{name,version}, capabilities}`.
- `tools/list` → the tools below with `inputSchema`.
- `tools/call {name, arguments, opId?}` → per-tool result. A mutating call may
  carry an idempotency `opId`; a replayed opId is answered `{status:"duplicate"}`
  without re-executing (P4 retry-safety).
- `resources/list` → the resources below.
- `resources/read {uri, budget?}` → the resource's JSON directly as `result`.

### Tools

| tool | arguments | effect |
|------|-----------|--------|
| `run_suite` | `{suite: p\|m\|i\|f\|d\|e\|e2\|e7}` | run a workload suite; async — returns `{status:"accepted"}`, events stream, `suite_done` is completion. Answers `busy` (-32002) while payloads are alive |
| `crash` | — | deliberate kernel fault → shutdown. Answers `busy` while payloads are alive |
| `kill` | `{pid}` | kill a live payload — servable MID-RUN via an M13 preemption (E2 remediation). Returns `{status:"killed", pid, elapsed}`; emits `payload_killed reason:"operator"` |
| `set_budget` | `{pid, deadline}` | set a live payload's instruction budget (timebase units from its start; 0 = none) — P1's externalized deadline policy, servable mid-run. Tightening below elapsed lets the autonomous deadline mechanism end a runaway |
| `ring_read` | — | the trap ring as a result |
| `set_surface` | `{mode: agentic\|classic}` | select the diagnostic surface (§5, P6/E1) |
| `set_autonomy` | `{mode: reactive\|autonomous}` | the autonomy dial (§4, P1/P3) |

### Resources

| uri | content |
|-----|---------|
| `trap_ring` | `{count, entries[]}` |
| `processes` | `{processes:[{pid,name,state,exit_code,caps[]}]}` — what ran, how it ended (P11) |
| `spec` | `{proto,arch,syscalls[],caps[],memory}` — the self-describing surface (P5) |
| `surface` | `{surface: agentic\|classic}` |
| `autonomy` | `{autonomy: reactive\|autonomous}` |
| `memory` | `{allocated, capacity}` — frame-allocator state (P11); E7's leak probe |
| `digest` | budgeted, coalesced activity summary (§4, P3); accepts `{budget:N}` |

## 4. Attention & autonomy (P3)

`digest` is the P3 endpoint: it accepts a `budget` and returns a **summary, not
the firehose** — `{budget, autonomy, totals:{traps,timer,ecall,fault},
by_severity:{error,info,trace}, items:[…≤budget notable traps, newest first…],
elided}`. Timer ticks collapse into a count; a small tick-proof ring keeps
faults surfaced; severity ranking (fault=error, syscall=info, tick=trace) means
a small budget preserves the high-severity events.

The **autonomy dial** (`set_autonomy`) lets the kernel self-manage the operator's
token budget. `reactive` (default): every event reaches the operator.
`autonomous`: trace-severity tick *frames* are suppressed from the wire (still
counted, deadlines still enforced), leaving the `digest` to pull on demand.

## 5. Diagnostic surfaces (P6 / E1)

`set_surface` toggles how faults render, so one binary is the A/B for E1:

- `agentic` (default): the structured fault frame (§2.1).
- `classic`: one printf `[FAULT] origin cause pc=… stval=… ra=… sp=…` line
  written **unframed** to the noise channel — a legacy kernel's console log.

`make eval` scores the localization facts recoverable from each surface over a
seeded-fault set; the `e1` acceptance check asserts the structured surface
strictly dominates. This is the deterministic surface-content proxy; the full
agent-in-the-loop E1 (localization rate/tokens with a real model) is future work.

## 6. Command bytes (§4 fallback, compat)

A single non-`0xAA` byte is an operator command, the zero-dependency escape
hatch the structured plane grew out of: `r` ring · `x` crash · `p`/`m`/`i`/`f`/`d`/`e`
the suites. A leading `0xAA` instead begins a request frame (§3).

## 7. Payloads may be C (not just Rust)

A payload is any statically-linked rv64 ELF the loader can map (§ELF loader) —
the runtime is language-agnostic. `payloads/craycast` is a fixed-point raycaster
in freestanding C, clang-cross-compiled integer-only (`-march=rv64imac
-mabi=lp64`: the kernel leaves `sstatus.FS=0`, so no FP instruction may be
emitted — the same reason Doom is fixed-point) and linked with the shared
`link.ld`. It renders to a framebuffer in `.bss` and emits it with
`blit(ptr,w,h)` (syscall 5): the kernel reads the framebuffer through the
payload's page table (EFAULT on a bad pointer, like `write`), producing the
`frame` event (§2). Enabled by the `cpayloads` cargo feature; `make raycast`
builds and plays it.

## Versioning

`proto` is 0. This spec is v0.1: additive fields may appear (readers must ignore
unknown fields); removals or type changes bump `proto`.
