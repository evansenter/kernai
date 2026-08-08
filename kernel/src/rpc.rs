#![forbid(unsafe_code)]
//! The MCP control plane (P4). Today the operator drives the kernel with
//! single command bytes; M8 adds a structured request path: JSON-RPC 2.0
//! (MCP method shapes) carried *inside* the existing length-prefixed frames,
//! so the serial layer stays dumb (CLAUDE.md hard rule) — an inbound frame is
//! the same `AA 99 | u32 LE len | UTF-8` envelope as every outbound event.
//!
//! Everything here is safe: a hand-rolled structural JSON reader (no alloc, no
//! serde) over a fixed request grammar, plus a dispatcher. Responses ride out
//! as ordinary event frames wrapped `{"id":<stream>,"type":"rpc","rpc":{…}}`
//! so the kernel's one invariant — every frame carries a monotonic stream id
//! (P12 spine, determinism) — holds for control traffic too; the JSON-RPC
//! object (with its own correlation `id`) nests inside `rpc`.
//!
//! Tools map to today's verbs (`run_suite`, `crash`, `ring_read`). Resources
//! expose kernel state (`trap_ring`, `processes`, and `spec` — the P5
//! self-describing surface). Mutating tool calls are idempotent: a client may
//! attach an `opId`, and a replayed call with a seen `opId` is acknowledged
//! without re-executing (agents replay and double-fire — RFC P4).

use core::fmt::Write;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};

use crate::console::FrameBuf;
use crate::{events, hal, payload, traps};

/// Inbound request cap. Requests are tiny (a method + small params); anything
/// larger is a malformed or hostile frame and is rejected, never buffered
/// unboundedly.
const MAX_REQ: usize = 512;

/// Abandon a partially-read frame after this many consecutive idle waits with
/// no new byte. This bounds a STALL, not total read time: a real client's
/// bytes arrive steadily (each resets the counter), so a well-formed frame of
/// any length up to MAX_REQ never trips it, while a crafted length whose body
/// is withheld frees the command plane after ~this many ticks instead of
/// eating the operator's commands until the frame happens to complete. A
/// per-byte STALL bound is used rather than a total instruction-count deadline
/// because `wait_for_interrupt` advances virtual time ~one tick per call, so a
/// total-time budget would abandon legit multi-wait reads.
const MAX_STALL_WAITS: u32 = 16;

// ---- Frame reader -------------------------------------------------------

/// Read one serial byte, or `None` if no byte arrives within `MAX_STALL_WAITS`
/// consecutive idle waits. The idle loop already parks on `wait_for_interrupt`;
/// reuse it so a multi-byte read does not busy-spin (timer ticks keep waking
/// us). The wait budget is per call, so it bounds how long a *stalled* frame
/// parks, never how long a steadily-arriving one takes.
fn getc_stall() -> Option<u8> {
    let mut idle = 0u32;
    loop {
        if let Some(b) = hal::console_getchar() {
            return Some(b);
        }
        idle += 1;
        if idle > MAX_STALL_WAITS {
            return None;
        }
        hal::wait_for_interrupt();
    }
}

/// Called from `idle` after it has consumed the first magic byte (0xAA). Reads
/// the rest of the inbound frame and dispatches it. Returns to `idle` for
/// read-only / control methods; diverges (into the scheduler, or a crash) for
/// `run_suite` / `crash` exactly as the single-byte `p`/`x` paths do.
///
/// Transport-level junk is *silently* dropped and resynced — never answered
/// with a frame (P-serial: "garbage bytes are ignored"; a reply would both
/// amplify noise and pollute the input-hardening invariant that only
/// hello/tick appear). A stray 0xAA (or a crafted length with a withheld body)
/// stalls at most `MAX_STALL_WAITS` waits before the reader gives up and
/// resyncs, so it can never wedge the command plane or eat an unbounded run of
/// operator command bytes. Only a well-formed frame carrying a JSON object is
/// dispatched; JSON-RPC-level problems (unknown method/tool) are answered
/// there, because that frame *was* a real client request.
pub fn read_request() {
    match getc_stall() {
        Some(0x99) => {}
        _ => return, // stray 0xAA (or a stall), not our magic — resync silently
    }
    let mut lenb = [0u8; 4];
    for b in &mut lenb {
        let Some(byte) = getc_stall() else {
            return; // frame stalled: abandon, resync
        };
        *b = byte;
    }
    let len = u32::from_le_bytes(lenb) as usize;
    if len == 0 || len > MAX_REQ {
        // Implausible length: a desync, not a frame. Drop, do NOT drain
        // (draining a bogus multi-GiB length would hang the kernel).
        return;
    }
    let mut buf = [0u8; MAX_REQ];
    for b in buf.iter_mut().take(len) {
        let Some(byte) = getc_stall() else {
            return; // withheld body: abandon before it eats the command plane
        };
        *b = byte;
    }
    let Ok(json) = core::str::from_utf8(&buf[..len]) else {
        return; // not UTF-8: transport junk, drop silently
    };
    // A real request is a JSON object; anything else is noise that happened to
    // survive framing (never emitted by a client), so ignore it.
    if json.trim_start().starts_with('{') {
        dispatch(json);
    }
}

// ---- Dispatch -----------------------------------------------------------

fn dispatch(json: &str) {
    let id = object_get(json, "id").unwrap_or("null");
    match object_get(json, "method").and_then(as_string) {
        Some("initialize") => respond_initialize(id),
        Some("tools/list") => respond_tools_list(id),
        Some("tools/call") => handle_tools_call(json, id),
        Some("resources/list") => respond_resources_list(id),
        Some("resources/read") => handle_resources_read(json, id),
        Some(_) => respond_error(id, -32601, "method not found"),
        None => respond_error(id, -32600, "missing method"),
    }
}

fn handle_tools_call(json: &str, id: &str) {
    let params = object_get(json, "params").unwrap_or("{}");
    let name = object_get(params, "name").and_then(as_string);
    let op = object_get(params, "opId").and_then(as_string);

    match name {
        Some("run_suite") => {
            let args = object_get(params, "arguments").unwrap_or("{}");
            let suite = object_get(args, "suite").and_then(as_string).unwrap_or("");
            // Validate BEFORE any side effect / divergence: an unknown suite
            // must leave the kernel in idle, answering.
            let seed: fn() = match suite {
                "p" | "m3" => payload::seed_suite_m3,
                "m" | "m4" => payload::seed_suite_m4,
                "i" | "m5" => payload::seed_suite_m5,
                "f" | "m6" => payload::seed_suite_m6,
                "d" | "m10" => payload::seed_suite_m10,
                "e" | "eval" => payload::seed_suite_eval,
                "l" | "e2" => payload::seed_suite_e2,
                // Feature-build workloads are first-class on the agent plane
                // too (P4): an agent starts DOOM the same way it starts any
                // suite — no fallback to the single-byte channel required.
                #[cfg(feature = "cpayloads")]
                "c" | "craycast" => payload::seed_suite_craycast,
                #[cfg(feature = "doom")]
                "D" | "doom" => payload::seed_suite_doom,
                _ => return respond_error(id, -32602, "unknown suite"),
            };
            // M13: this may now be reached from the mid-run service window (a
            // preempted payload is waiting). Reseeding would destroy live
            // payloads — refuse with a structured busy instead.
            if payload::any_alive() {
                return respond_error(id, -32002, "busy: payloads active");
            }
            if let Some(op) = op {
                let h = fnv1a(op);
                if op_seen(h) {
                    return respond_duplicate(id, op);
                }
                op_record(h);
            }
            // `run_suite` is async: acknowledge now, then run. The suite's own
            // events stream out and the terminal `suite_done` is the result
            // signal. `payload::run` diverges into the scheduler → idle, so
            // nothing after it runs (mirrors the single-byte 'p' path).
            respond_accepted(id, suite);
            seed();
            payload::run();
        }
        Some("crash") => {
            // Same mid-run guard as run_suite: the deliberate kernel fault is
            // an idle-plane stimulus, not a remediation verb.
            if payload::any_alive() {
                return respond_error(id, -32002, "busy: payloads active");
            }
            if let Some(op) = op {
                let h = fnv1a(op);
                if op_seen(h) {
                    return respond_duplicate(id, op);
                }
                op_record(h);
            }
            respond_result(id, |f| f.write_str(r#"{"status":"crashing"}"#));
            hal::trigger_illegal_instruction(); // diverges: fault → shutdown
        }
        Some("kill") => {
            // E2's remediation verb: kill a live payload by pid, servable
            // MID-RUN via an M13 preemption. Mutating → opId-idempotent.
            let args = object_get(params, "arguments").unwrap_or("{}");
            let pid = match object_get(args, "pid").and_then(parse_u32) {
                Some(p) => p as usize,
                None => return respond_error(id, -32602, "missing pid"),
            };
            if let Some(op) = op {
                let h = fnv1a(op);
                if op_seen(h) {
                    return respond_duplicate(id, op);
                }
                op_record(h);
            }
            match payload::kill_pid(pid) {
                Ok(elapsed) => respond_result(id, move |f| {
                    write!(
                        f,
                        r#"{{"status":"killed","pid":{pid},"elapsed":{elapsed}}}"#
                    )
                }),
                Err(reason) => respond_error(id, -32602, reason),
            }
        }
        Some("ring_read") => respond_result(id, traps::write_ring_resource),
        Some("set_surface") => {
            let args = object_get(params, "arguments").unwrap_or("{}");
            match object_get(args, "mode").and_then(as_string) {
                Some("classic") => {
                    traps::set_classic_surface(true);
                    respond_result(id, |f| f.write_str(r#"{"surface":"classic"}"#));
                }
                Some("agentic") => {
                    traps::set_classic_surface(false);
                    respond_result(id, |f| f.write_str(r#"{"surface":"agentic"}"#));
                }
                _ => respond_error(id, -32602, "mode must be classic|agentic"),
            }
        }
        Some("set_autonomy") => {
            let args = object_get(params, "arguments").unwrap_or("{}");
            match object_get(args, "mode").and_then(as_string) {
                Some("autonomous") => {
                    traps::set_autonomous(true);
                    respond_result(id, |f| f.write_str(r#"{"autonomy":"autonomous"}"#));
                }
                Some("reactive") => {
                    traps::set_autonomous(false);
                    respond_result(id, |f| f.write_str(r#"{"autonomy":"reactive"}"#));
                }
                _ => respond_error(id, -32602, "mode must be reactive|autonomous"),
            }
        }
        Some(_) => respond_error(id, -32602, "unknown tool"),
        None => respond_error(id, -32602, "missing tool name"),
    }
}

fn handle_resources_read(json: &str, id: &str) {
    let params = object_get(json, "params").unwrap_or("{}");
    match object_get(params, "uri").and_then(as_string) {
        Some("trap_ring") => respond_result(id, traps::write_ring_resource),
        Some("processes") => respond_result(id, payload::write_process_table),
        Some("spec") => respond_result(id, payload::write_spec),
        Some("surface") => respond_result(id, |f| {
            let mode = if traps::classic_surface() {
                "classic"
            } else {
                "agentic"
            };
            write!(f, r#"{{"surface":"{mode}"}}"#)
        }),
        Some("autonomy") => respond_result(id, |f| {
            let mode = if traps::autonomous() {
                "autonomous"
            } else {
                "reactive"
            };
            write!(f, r#"{{"autonomy":"{mode}"}}"#)
        }),
        Some("digest") => {
            // P3: the endpoint accepts a token/item budget parameter and returns
            // a coalesced summary. Default 4; clamped so a huge budget can't
            // grow the frame (the notable ring is small anyway).
            let budget = object_get(params, "budget")
                .and_then(parse_u32)
                .unwrap_or(4)
                .min(64) as usize;
            respond_result(id, move |f| traps::write_digest(f, budget));
        }
        Some(_) => respond_error(id, -32602, "unknown resource"),
        None => respond_error(id, -32602, "missing uri"),
    }
}

// ---- Responders ---------------------------------------------------------

/// Emit one control-plane frame: the stream-id + `type:"rpc"` envelope around
/// a JSON-RPC object built by `body`. Interrupts masked for the whole
/// build+emit, like every other emission path (no tick interleave, id fixed).
fn emit_rpc<F: FnOnce(&mut FrameBuf) -> core::fmt::Result>(body: F) {
    hal::without_interrupts(|| {
        let id = events::next_id();
        let mut f = FrameBuf::new();
        let _ = write!(f, r#"{{"id":{id},"type":"rpc","rpc":"#);
        let _ = body(&mut f);
        let _ = f.write_str("}");
        if f.overflowed() {
            // The response didn't fit a frame. Emitting nothing would leave the
            // client waiting forever (a silent hang); instead emit a small
            // fixed error under the SAME stream id. `id` is nulled because the
            // client's correlation id may itself be what overflowed.
            let mut e = FrameBuf::new();
            let _ = write!(
                e,
                r#"{{"id":{id},"type":"rpc","rpc":{{"jsonrpc":"2.0","id":null,"error":{{"code":-32001,"message":"response too large"}}}}}}"#
            );
            e.emit();
        } else {
            f.emit();
        }
    });
}

fn respond_result<F: FnOnce(&mut FrameBuf) -> core::fmt::Result>(id: &str, result: F) {
    emit_rpc(|f| {
        f.write_str(r#"{"jsonrpc":"2.0","id":"#)?;
        write_id(f, id)?;
        f.write_str(r#","result":"#)?;
        result(f)?;
        f.write_str("}")
    });
}

fn respond_error(id: &str, code: i32, message: &str) {
    emit_rpc(|f| {
        f.write_str(r#"{"jsonrpc":"2.0","id":"#)?;
        write_id(f, id)?;
        write!(f, r#","error":{{"code":{code},"message":""#)?;
        f.write_json_escaped(message)?;
        f.write_str(r#""}}"#)
    });
}

fn respond_accepted(id: &str, suite: &str) {
    respond_result(id, |f| {
        f.write_str(r#"{"status":"accepted","suite":""#)?;
        f.write_json_escaped(suite)?;
        f.write_str(r#""}"#)
    });
}

fn respond_duplicate(id: &str, op: &str) {
    respond_result(id, |f| {
        f.write_str(r#"{"status":"duplicate","opId":""#)?;
        f.write_json_escaped(op)?;
        f.write_str(r#""}"#)
    });
}

fn respond_initialize(id: &str) {
    respond_result(id, |f| {
        f.write_str(r#"{"protocolVersion":"2024-11-05","#)?;
        f.write_str(r#""serverInfo":{"name":"kernai","version":"0"},"#)?;
        f.write_str(r#""capabilities":{"tools":{},"resources":{}}}"#)
    });
}

fn respond_tools_list(id: &str) {
    respond_result(id, |f| {
        f.write_str(r#"{"tools":["#)?;
        tool(
            f,
            "run_suite",
            "Run a payload suite (p|m|i|f|d|e|e2; feature builds add craycast/doom).",
            Some("suite"),
        )?;
        f.write_str(",")?;
        tool(
            f,
            "crash",
            "Trigger a deliberate kernel fault → shutdown.",
            None,
        )?;
        f.write_str(",")?;
        tool(
            f,
            "kill",
            "Kill a live payload by pid — servable mid-run (M13/E2 remediation).",
            Some("pid"),
        )?;
        f.write_str(",")?;
        tool(f, "ring_read", "Read the trap ring buffer.", None)?;
        f.write_str(",")?;
        tool(
            f,
            "set_surface",
            "Select the diagnostic surface: classic|agentic (P6/E1).",
            Some("mode"),
        )?;
        f.write_str(",")?;
        tool(
            f,
            "set_autonomy",
            "Set the autonomy dial: reactive|autonomous (P1/P3).",
            Some("mode"),
        )?;
        f.write_str("]}")
    });
}

fn tool(f: &mut FrameBuf, name: &str, desc: &str, prop: Option<&str>) -> core::fmt::Result {
    write!(
        f,
        r#"{{"name":"{name}","description":"{desc}","inputSchema":{{"type":"object""#
    )?;
    if let Some(p) = prop {
        write!(
            f,
            r#","properties":{{"{p}":{{"type":"string"}}}},"required":["{p}"]"#
        )?;
    }
    f.write_str("}}")
}

fn respond_resources_list(id: &str) {
    respond_result(id, |f| {
        f.write_str(r#"{"resources":["#)?;
        resource(f, "trap_ring", "The trap ring buffer (last N traps).")?;
        f.write_str(",")?;
        resource(f, "processes", "The process table.")?;
        f.write_str(",")?;
        resource(
            f,
            "spec",
            "Self-describing surface: syscalls, caps, memory map (P5).",
        )?;
        f.write_str(",")?;
        resource(
            f,
            "surface",
            "The active diagnostic surface (classic|agentic).",
        )?;
        f.write_str(",")?;
        resource(
            f,
            "autonomy",
            "The active autonomy dial (reactive|autonomous).",
        )?;
        f.write_str(",")?;
        resource(
            f,
            "digest",
            "Budgeted, coalesced activity summary; takes a `budget` param (P3).",
        )?;
        f.write_str("]}")
    });
}

fn resource(f: &mut FrameBuf, uri: &str, desc: &str) -> core::fmt::Result {
    write!(
        f,
        r#"{{"uri":"{uri}","name":"{uri}","description":"{desc}","mimeType":"application/json"}}"#
    )
}

/// Longest client id we echo back. A correlation token needs no more; the
/// cap stops a client from overflowing the response frame with a huge id
/// (`write_json_escaped` expands each control byte 6x — see the FrameBuf
/// audit), which would silently drop the whole response.
const MAX_ID: usize = 64;

/// Echo the client's request id but *safely*: re-emit a JSON string through
/// the escaper (length-clamped), a strict JSON integer as-is, anything else as
/// `null`. A client can neither inject structure into our frame through the id
/// field nor produce a syntactically invalid id (e.g. `1.2.3`, `--`), nor blow
/// the frame with a giant id.
fn write_id(f: &mut FrameBuf, raw: &str) -> core::fmt::Result {
    let raw = raw.trim();
    if let Some(inner) = as_string(raw) {
        // Clamp to MAX_ID *chars* (never split a UTF-8 code point).
        let end = inner
            .char_indices()
            .nth(MAX_ID)
            .map_or(inner.len(), |(i, _)| i);
        f.write_str("\"")?;
        f.write_json_escaped(&inner[..end])?;
        return f.write_str("\"");
    }
    // A strict JSON integer: optional leading '-', then one or more digits and
    // nothing else. Rejects `1.2.3`, `--`, `1e9`, `+1` → `null`, so our own
    // response is always valid JSON.
    let digits = raw.strip_prefix('-').unwrap_or(raw);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        f.write_str(raw)
    } else {
        f.write_str("null")
    }
}

// ---- Idempotency (client opIds) ----------------------------------------

const OP_SLOTS: usize = 8;
static OP_IDS: [AtomicU64; OP_SLOTS] = [const { AtomicU64::new(0) }; OP_SLOTS];
static OP_HEAD: AtomicUsize = AtomicUsize::new(0);

/// FNV-1a, forced nonzero so it never collides with the empty-slot sentinel.
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in s.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h | 1
}

fn op_seen(h: u64) -> bool {
    OP_IDS.iter().any(|slot| slot.load(Relaxed) == h)
}

fn op_record(h: u64) {
    let idx = OP_HEAD.fetch_add(1, Relaxed) % OP_SLOTS;
    OP_IDS[idx].store(h, Relaxed);
}

// ---- Minimal structural JSON reader ------------------------------------
//
// Not a general parser: it walks a JSON object's *top-level* members and
// returns raw value slices, skipping nested structure so a key inside
// `params` never matches at the outer level. Enough for the fixed request
// grammar, and small enough to audit at a glance.

fn skip_ws(s: &[u8], mut i: usize) -> usize {
    while i < s.len() && matches!(s[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    i
}

/// `i` at the opening quote; return the index just past the closing quote.
fn skip_string(s: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    while j < s.len() {
        match s[j] {
            b'\\' => j += 2, // escaped char: skip both bytes
            b'"' => return j + 1,
            _ => j += 1,
        }
    }
    s.len()
}

/// `i` at `{` or `[`; return the index just past the matching close. Any
/// bracket type nests; strings are skipped whole so quoted brackets don't
/// miscount.
fn skip_container(s: &[u8], i: usize) -> usize {
    let mut depth = 0i32;
    let mut j = i;
    while j < s.len() {
        match s[j] {
            b'"' => j = skip_string(s, j),
            b'{' | b'[' => {
                depth += 1;
                j += 1;
            }
            b'}' | b']' => {
                depth -= 1;
                j += 1;
                if depth == 0 {
                    return j;
                }
            }
            _ => j += 1,
        }
    }
    s.len()
}

/// `i` at the first byte of a value; return the index just past it.
fn skip_value(s: &[u8], i: usize) -> usize {
    let i = skip_ws(s, i);
    if i >= s.len() {
        return s.len();
    }
    match s[i] {
        b'"' => skip_string(s, i),
        b'{' | b'[' => skip_container(s, i),
        _ => {
            // number / true / false / null: to the next structural byte
            let mut j = i;
            while j < s.len() && !matches!(s[j], b',' | b'}' | b']') {
                j += 1;
            }
            j
        }
    }
}

/// Find top-level member `key` in the JSON object `obj`; return its raw value
/// slice (trimmed), or None if absent or `obj` isn't a well-formed object.
fn object_get<'a>(obj: &'a str, key: &str) -> Option<&'a str> {
    let s = obj.as_bytes();
    let mut i = skip_ws(s, 0);
    if i >= s.len() || s[i] != b'{' {
        return None;
    }
    i += 1;
    loop {
        i = skip_ws(s, i);
        if i >= s.len() || s[i] != b'"' {
            return None; // '}' or malformed
        }
        let key_end = skip_string(s, i);
        let this_key = obj.get(i + 1..key_end - 1)?;
        i = skip_ws(s, key_end);
        if i >= s.len() || s[i] != b':' {
            return None;
        }
        i = skip_ws(s, i + 1);
        let val_start = i;
        let val_end = skip_value(s, i);
        if this_key == key {
            return Some(obj.get(val_start..val_end)?.trim());
        }
        i = skip_ws(s, val_end);
        if i < s.len() && s[i] == b',' {
            i += 1;
            continue;
        }
        return None;
    }
}

/// A raw value slice that is a JSON string → its inner content. Our request
/// strings (method names, tool names, uris, single-letter suites) carry no
/// escapes, so the inner slice compares directly.
fn as_string(raw: &str) -> Option<&str> {
    let b = raw.as_bytes();
    if b.len() >= 2 && b[0] == b'"' && b[b.len() - 1] == b'"' {
        raw.get(1..raw.len() - 1)
    } else {
        None
    }
}

/// Parse a raw JSON value slice as a small unsigned integer (for the `digest`
/// budget). Accepts a bare number or a quoted number; rejects anything else and
/// saturates rather than overflowing.
fn parse_u32(raw: &str) -> Option<u32> {
    let s = as_string(raw).unwrap_or(raw).trim();
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut n: u32 = 0;
    for b in s.bytes() {
        n = n.saturating_mul(10).saturating_add((b - b'0') as u32);
    }
    Some(n)
}
