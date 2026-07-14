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
/// larger is a malformed or hostile frame and is drained + rejected, never
/// buffered unboundedly.
const MAX_REQ: usize = 512;

// ---- Frame reader -------------------------------------------------------

/// Block until one serial byte is available. The idle loop already parks on
/// `wait_for_interrupt`; reuse that so reading a multi-byte request frame does
/// not busy-spin (timer ticks keep waking us).
fn getc_blocking() -> u8 {
    loop {
        if let Some(b) = hal::console_getchar() {
            return b;
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
/// hello/tick appear). A stray 0xAA in the byte stream therefore costs at most
/// a handful of consumed bytes, like the host decoder's skip-and-rescan. Only
/// a well-formed frame carrying a JSON object is dispatched; JSON-RPC-level
/// problems (unknown method/tool) are answered there, because that frame *was*
/// a real client request.
pub fn read_request() {
    if getc_blocking() != 0x99 {
        return; // stray 0xAA, not our magic — resync silently
    }
    let mut lenb = [0u8; 4];
    for b in &mut lenb {
        *b = getc_blocking();
    }
    let len = u32::from_le_bytes(lenb) as usize;
    if len == 0 || len > MAX_REQ {
        // Implausible length: a desync, not a frame. Drop, do NOT drain
        // (draining a bogus multi-GiB length would hang the kernel).
        return;
    }
    let mut buf = [0u8; MAX_REQ];
    for b in buf.iter_mut().take(len) {
        *b = getc_blocking();
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
                _ => return respond_error(id, -32602, "unknown suite"),
            };
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
        Some("ring_read") => respond_result(id, traps::write_ring_resource),
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
        let mut f = FrameBuf::new();
        let _ = write!(f, r#"{{"id":{},"type":"rpc","rpc":"#, events::next_id());
        let _ = body(&mut f);
        let _ = f.write_str("}");
        f.emit();
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
            "Run a payload acceptance suite (p|m|i|f).",
            true,
        )?;
        f.write_str(",")?;
        tool(
            f,
            "crash",
            "Trigger a deliberate kernel fault → shutdown.",
            false,
        )?;
        f.write_str(",")?;
        tool(f, "ring_read", "Read the trap ring buffer.", false)?;
        f.write_str("]}")
    });
}

fn tool(f: &mut FrameBuf, name: &str, desc: &str, takes_suite: bool) -> core::fmt::Result {
    write!(
        f,
        r#"{{"name":"{name}","description":"{desc}","inputSchema":{{"type":"object""#
    )?;
    if takes_suite {
        f.write_str(r#","properties":{"suite":{"type":"string"}},"required":["suite"]"#)?;
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
        f.write_str("]}")
    });
}

fn resource(f: &mut FrameBuf, uri: &str, desc: &str) -> core::fmt::Result {
    write!(
        f,
        r#"{{"uri":"{uri}","name":"{uri}","description":"{desc}","mimeType":"application/json"}}"#
    )
}

/// Echo the client's request id verbatim but *safely*: re-emit a JSON string
/// through the escaper, a validated number as-is, anything else as `null`. A
/// client can never inject structure into our frame through the id field.
fn write_id(f: &mut FrameBuf, raw: &str) -> core::fmt::Result {
    let raw = raw.trim();
    if let Some(inner) = as_string(raw) {
        f.write_str("\"")?;
        f.write_json_escaped(inner)?;
        return f.write_str("\"");
    }
    let numeric = !raw.is_empty()
        && raw
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.' | b'e' | b'E'));
    if numeric {
        f.write_str(raw)
    } else {
        f.write_str("null") // includes the literal `null` and any junk
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
