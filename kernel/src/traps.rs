#![forbid(unsafe_code)]
//! Trap dispatch, timer cadence, and the trap ring buffer.
//!
//! The ring is the first sliver of P11 (no kernel state observable only via
//! debugger): every trap is recorded, the last RING_SIZE are queryable over
//! serial, and the fault report carries them as history. The fault report
//! itself is the v0 P6 diagnostic frame — crude, but structured: cause,
//! sepc, decoded instruction fields, recent trap history.

use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::console::{FrameBuf, RawConsole};
use crate::{events, hal};

/// Diagnostic surface selector (P6/E1). Default (false) = the agentic surface:
/// the rich structured fault frame. When true, the *same* fault renders as a
/// single printf-style console line instead (`emit_fault_classic`) — the E1
/// A/B twin, so we can measure whether the surface, not the agent, decides how
/// fast a bug is localized. Toggled over the control plane (`set_surface`).
static CLASSIC_SURFACE: AtomicBool = AtomicBool::new(false);

/// Select the classic (true) or agentic (false) diagnostic surface.
pub fn set_classic_surface(on: bool) {
    CLASSIC_SURFACE.store(on, Ordering::Relaxed);
}

/// True if faults currently render on the classic printf surface.
pub fn classic_surface() -> bool {
    CLASSIC_SURFACE.load(Ordering::Relaxed)
}

/// Timer cadence in timebase units (10 MHz on qemu-virt). Under
/// `-icount shift=1` (2 ns per instruction) one unit is 50 instructions, so
/// ticks fire every 500_000 instructions — an instruction-count cadence,
/// never wall time (P9).
pub const TICK_INTERVAL: u64 = 10_000;

const INTERRUPT_BIT: u64 = 1 << 63;
const CAUSE_S_TIMER: u64 = INTERRUPT_BIT | 5;
const CAUSE_ILLEGAL_INSTRUCTION: u64 = 2;
const CAUSE_U_ECALL: u64 = 8; // environment call from U-mode

const SSTATUS_SPP: u64 = 1 << 8; // previous privilege: 0 = U, 1 = S
const SSTATUS_SPIE: u64 = 1 << 5; // previous interrupt-enable

// Register indices into TrapFrame::regs (regs[i] holds x(i+1)).
const X_SP: usize = 1; // x2
const X_A0: usize = 9; // x10
const X_A1: usize = 10; // x11
const X_A2: usize = 11; // x12
const X_A7: usize = 16; // x17

/// Saved by hal's trap vector; layout is matched by its asm (offsets
/// (n-1)*8 for x_n, 248 for sepc, 256 for sstatus). Plain data on purpose:
/// P11 pushes every kernel structure toward serializability.
#[repr(C)]
pub struct TrapFrame {
    /// x1..x31 — regs[i] holds x(i+1); x0 is hardwired zero.
    pub regs: [u64; 31],
    pub sepc: u64,
    pub sstatus: u64,
}

impl TrapFrame {
    fn x(&self, n: usize) -> u64 {
        if n == 0 { 0 } else { self.regs[n - 1] }
    }

    pub fn a0(&self) -> u64 {
        self.regs[X_A0]
    }
    pub fn a1(&self) -> u64 {
        self.regs[X_A1]
    }
    pub fn a2(&self) -> u64 {
        self.regs[X_A2]
    }
    pub fn a7(&self) -> u64 {
        self.regs[X_A7]
    }
    pub fn set_a0(&mut self, v: u64) {
        self.regs[X_A0] = v;
    }
    fn set_sp(&mut self, v: u64) {
        self.regs[X_SP] = v;
    }

    /// True if this trap came from U-mode (a payload), false if from S-mode
    /// (the kernel itself). Drives fault routing: payload faults kill the
    /// payload; kernel faults shut down.
    pub fn is_from_user(&self) -> bool {
        self.sstatus & SSTATUS_SPP == 0
    }

    /// Step past the 4-byte `ecall` so the payload resumes after it. Our sys
    /// runtime always emits an uncompressed `ecall`, so the width is fixed.
    fn advance_past_ecall(&mut self) {
        self.sepc = self.sepc.wrapping_add(4);
    }
}

pub const RING_SIZE: usize = 8;

/// One recorded trap. Atomics for interior mutability in a static without
/// unsafe; consistency comes from execution structure, not the atomics:
/// writers run in the trap handler (interrupts hardware-disabled), readers
/// snapshot under hal::without_interrupts.
struct RingSlot {
    id: AtomicU64,
    scause: AtomicU64,
    sepc: AtomicU64,
    stval: AtomicU64,
}

impl RingSlot {
    const fn new() -> Self {
        RingSlot {
            id: AtomicU64::new(0),
            scause: AtomicU64::new(0),
            sepc: AtomicU64::new(0),
            stval: AtomicU64::new(0),
        }
    }
}

static RING: [RingSlot; RING_SIZE] = [const { RingSlot::new() }; RING_SIZE];
/// Total traps ever recorded; slot for trap n is RING[n % RING_SIZE].
static RING_COUNT: AtomicU64 = AtomicU64::new(0);
/// Timer ticks serviced so far.
static TICKS: AtomicU64 = AtomicU64::new(0);
/// The timebase value the current tick was scheduled for. Re-arming from
/// this (not from "now") keeps ticks exactly TICK_INTERVAL apart — no
/// handler-latency drift in the cadence.
static NEXT_DEADLINE: AtomicU64 = AtomicU64::new(0);

// ---- M11: attention budgeting (P3) + autonomy dial ----------------------

/// Per-severity trap counters (P3): the digest coalesces the firehose into
/// these totals, so the operator spends tokens on a summary, not every tick.
/// (Timer ticks are counted by `TICKS` above.)
static ECALL_TRAPS: AtomicU64 = AtomicU64::new(0);
static FAULT_TRAPS: AtomicU64 = AtomicU64::new(0);

/// The last few NON-timer ("notable") traps, never evicted by ticks — so a
/// fault stays visible in the digest no matter how many ticks follow it.
const NOTABLE_SIZE: usize = 4;
static NOTABLE: [RingSlot; NOTABLE_SIZE] = [const { RingSlot::new() }; NOTABLE_SIZE];
static NOTABLE_COUNT: AtomicU64 = AtomicU64::new(0);

/// Autonomy dial (P1/P3). Reactive (default): every event reaches the operator.
/// Autonomous: the kernel decides trace-severity events (ticks) aren't worth
/// the operator's token budget and suppresses them from the wire — still
/// counting them and still checking payload deadlines — leaving a budgeted
/// `digest` to pull on demand.
static AUTONOMOUS: AtomicBool = AtomicBool::new(false);

/// Select autonomous (true) or reactive (false) event handling.
pub fn set_autonomous(on: bool) {
    AUTONOMOUS.store(on, Ordering::Relaxed);
}

/// True if the kernel is currently self-managing operator attention.
pub fn autonomous() -> bool {
    AUTONOMOUS.load(Ordering::Relaxed)
}

/// Severity of a trap by cause (P3): faults are errors, syscalls are info,
/// ticks are trace (coalesced away under a budget).
fn severity_of(scause: u64) -> &'static str {
    match scause {
        CAUSE_S_TIMER => "trace",
        CAUSE_U_ECALL => "info",
        s if s & INTERRUPT_BIT != 0 => "warn",
        _ => "error", // illegal instruction, page faults, etc.
    }
}

/// Schedule the first tick. Call once from kmain, before enabling
/// interrupts.
pub fn arm_first_tick() {
    let deadline = hal::read_time() + TICK_INTERVAL;
    NEXT_DEADLINE.store(deadline, Ordering::Relaxed);
    hal::set_timer(deadline);
}

fn ring_record(id: u64, scause: u64, sepc: u64, stval: u64) {
    let n = RING_COUNT.load(Ordering::Relaxed);
    let slot = &RING[(n % RING_SIZE as u64) as usize];
    slot.id.store(id, Ordering::Relaxed);
    slot.scause.store(scause, Ordering::Relaxed);
    slot.sepc.store(sepc, Ordering::Relaxed);
    slot.stval.store(stval, Ordering::Relaxed);
    RING_COUNT.store(n + 1, Ordering::Relaxed);
    // Severity tally + a tick-proof record of notable (non-timer) traps (P3).
    // Timer ticks are already counted by TICKS; here we log the events an
    // operator actually spends attention on so the digest can surface them.
    if scause != CAUSE_S_TIMER {
        if scause == CAUSE_U_ECALL {
            ECALL_TRAPS.fetch_add(1, Ordering::Relaxed);
        } else {
            FAULT_TRAPS.fetch_add(1, Ordering::Relaxed);
        }
        let m = NOTABLE_COUNT.load(Ordering::Relaxed);
        let nslot = &NOTABLE[(m % NOTABLE_SIZE as u64) as usize];
        nslot.id.store(id, Ordering::Relaxed);
        nslot.scause.store(scause, Ordering::Relaxed);
        nslot.sepc.store(sepc, Ordering::Relaxed);
        nslot.stval.store(stval, Ordering::Relaxed);
        NOTABLE_COUNT.store(m + 1, Ordering::Relaxed);
    }
}

/// All traps land here (from hal's vector). Timer ticks are serviced and
/// reported; everything else is a fault: emit the diagnostic frame, then
/// shut down — a fault must never be a hang (P6).
pub fn handle(frame: &mut TrapFrame) {
    let scause = hal::read_scause();
    let stval = hal::read_stval();
    let id = events::next_id();
    ring_record(id, scause, frame.sepc, stval);

    if scause == CAUSE_S_TIMER {
        // Re-arm first (set_timer clears the pending bit before we sret),
        // from the previous deadline so the cadence stays exact. Clamp
        // forward if we ever fall a whole interval behind — no catch-up
        // storm of back-to-back ticks.
        let now = hal::read_time();
        let mut next = NEXT_DEADLINE.load(Ordering::Relaxed) + TICK_INTERVAL;
        if next <= now {
            next = now + TICK_INTERVAL;
        }
        NEXT_DEADLINE.store(next, Ordering::Relaxed);
        hal::set_timer(next);
        let seq = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
        // Autonomy dial (P3): under autonomous attention the kernel suppresses
        // the tick FRAME (trace severity) from the wire — the tick still
        // happened (counted, deadline checked below), it just isn't spent on
        // the operator's token budget. Reactive (default) emits it.
        if !autonomous() {
            let mut f = FrameBuf::new();
            let _ = write!(
                f,
                r#"{{"id":{id},"type":"tick","seq":{seq},"time":{}}}"#,
                hal::read_time()
            );
            f.emit();
        }
        // Agentic input seam (doom builds): while a payload runs, the tick
        // drains serial bytes into the key ring the payload pops via
        // SYS_GETKEY — and byte 0x03 is the operator kill, a live remediation
        // (P1/P2, the E2 seed). Gated on a running payload inside drain_keys,
        // so the idle command loop / MCP reader never lose bytes to it.
        #[cfg(feature = "doom")]
        let op_killed = crate::payload::drain_keys();
        #[cfg(not(feature = "doom"))]
        let op_killed = false;
        // M4 seam: a running payload's instruction-count deadline is checked
        // here; an over-budget payload is redirected to the scheduler
        // (crate::payload::on_tick returns whether it killed the current one).
        if crate::payload::on_tick() || op_killed {
            redirect_to_scheduler(frame);
            return;
        }
        // M13 seam: pending control-plane input suspends a (non-keyboard)
        // payload so the scheduler can service it live, then resume — the
        // frame must be saved (maybe_preempt does) BEFORE the redirect
        // rewrites it.
        if crate::payload::maybe_preempt(frame) {
            redirect_to_scheduler(frame);
        }
        return;
    }

    // Environment call from a payload: dispatch the syscall.
    if scause == CAUSE_U_ECALL {
        frame.advance_past_ecall();
        match crate::syscall::dispatch(frame) {
            crate::syscall::Outcome::Resume(v) => frame.set_a0(v as u64),
            crate::syscall::Outcome::Leave => redirect_to_scheduler(frame),
        }
        return;
    }

    // Any other trap is a fault. From U-mode it kills only the payload and
    // the kernel keeps running (P1: a parked payload is an event, not a
    // kernel error); from S-mode it is a kernel bug — report and shut down.
    if frame.is_from_user() {
        emit_fault(frame, id, scause, stval, crate::payload::current_pid());
        crate::payload::mark_faulted();
        redirect_to_scheduler(frame);
    } else {
        emit_fault(frame, id, scause, stval, None);
        hal::shutdown(true);
    }
}

/// Rewrite `frame` so the trap-return `sret` lands in the scheduler in
/// S-mode on the boot stack, abandoning the departing payload. This is how
/// a payload hands the CPU back to the kernel without a separate context
/// switch: the normal trap-restore path does the work.
fn redirect_to_scheduler(frame: &mut TrapFrame) {
    // Clear CURRENT now, in trap context (interrupts off), so the scheduler —
    // which resumes with interrupts on — never runs with a stale CURRENT.
    crate::payload::leave_current();
    frame.sepc = (crate::payload::scheduler_resume as *const () as usize) as u64;
    frame.sstatus |= SSTATUS_SPP; // return to S-mode
    frame.sstatus |= SSTATUS_SPIE; // interrupts on after sret
    frame.set_sp(hal::boot_stack_top() as u64);
}

/// Serialize `frame` into a zeroed pool frame at `pa` in the trap vector's
/// layout (x_n at (n-1)*8, sepc @248, sstatus @256), overriding a0. Used by
/// M6 snapshot: the saved copy resumes past the `ecall`, and `a0` lets a
/// restored continuation see 0 while the original saw the snapshot id.
pub fn save_frame_to(frame: &TrapFrame, pa: usize, a0: u64) {
    for (i, &r) in frame.regs.iter().enumerate() {
        hal::phys_write_u64(pa + i * 8, r);
    }
    hal::phys_write_u64(pa + X_A0 * 8, a0);
    hal::phys_write_u64(pa + 248, frame.sepc);
    hal::phys_write_u64(pa + 256, frame.sstatus);
}

/// Initialize a zeroed pool frame `pa` as a fresh-start trap frame for a
/// payload: all GPRs zero (so no kernel register value leaks across the
/// privilege boundary), sepc = `entry`, sstatus = SPIE (interrupts on after
/// sret, SPP=0 = U-mode). `pa` must be a freshly zeroed frame.
pub fn init_frame_to(pa: usize, entry: usize) {
    hal::phys_write_u64(pa + 248, entry as u64);
    hal::phys_write_u64(pa + 256, SSTATUS_SPIE); // SPP=0, SPIE=1
}

fn cause_name(scause: u64) -> &'static str {
    match scause {
        0 => "instruction_address_misaligned",
        1 => "instruction_access_fault",
        2 => "illegal_instruction",
        3 => "breakpoint",
        4 => "load_address_misaligned",
        5 => "load_access_fault",
        6 => "store_address_misaligned",
        7 => "store_access_fault",
        8..=11 => "ecall",
        12 => "instruction_page_fault",
        13 => "load_page_fault",
        15 => "store_page_fault",
        CAUSE_S_TIMER => "timer",
        s if s & INTERRUPT_BIT != 0 => "unexpected_interrupt",
        _ => "unknown",
    }
}

/// Append the ring contents (oldest first) as a JSON array.
fn write_ring(f: &mut FrameBuf) -> core::fmt::Result {
    let count = RING_COUNT.load(Ordering::Relaxed);
    let first = count.saturating_sub(RING_SIZE as u64);
    f.write_str("[")?;
    for n in first..count {
        let slot = &RING[(n % RING_SIZE as u64) as usize];
        if n > first {
            f.write_str(",")?;
        }
        write!(
            f,
            r#"{{"id":{},"cause":"{}","sepc":"0x{:x}"}}"#,
            slot.id.load(Ordering::Relaxed),
            cause_name(slot.scause.load(Ordering::Relaxed)),
            slot.sepc.load(Ordering::Relaxed),
        )?;
    }
    f.write_str("]")
}

/// Serial-queryable ring dump (P11 seed; the P4 MCP resource
/// `/trace/traps` will be this same data behind a real protocol).
pub fn emit_ring_dump() {
    // The whole build+emit runs with interrupts off so a tick can neither
    // interleave bytes on the wire nor outrun the id we allocate here.
    hal::without_interrupts(|| {
        let mut f = FrameBuf::new();
        let _ = write!(
            f,
            r#"{{"id":{},"type":"trap_ring","count":{},"entries":"#,
            events::next_id(),
            RING_COUNT.load(Ordering::Relaxed),
        );
        let _ = write_ring(&mut f);
        let _ = f.write_str("}");
        f.emit();
    });
}

/// The ring as an MCP resource body: `{"count":N,"entries":[…]}`. Same data
/// as `emit_ring_dump` but as a *value* the caller embeds in a JSON-RPC
/// result (no envelope, no event id of its own) — the P4 `trap_ring` resource.
pub fn write_ring_resource(f: &mut FrameBuf) -> core::fmt::Result {
    write!(
        f,
        r#"{{"count":{},"entries":"#,
        RING_COUNT.load(Ordering::Relaxed)
    )?;
    write_ring(f)?;
    f.write_str("}")
}

/// A budgeted, coalesced digest of kernel activity (P3, the `digest` resource):
/// instead of the firehose, return per-severity totals — every timer tick
/// collapsed into a count — plus up to `budget` of the most-recent NOTABLE
/// traps (faults, syscalls), newest first. The operator spends a bounded number
/// of tokens and still sees what matters; `elided` is how much was summarized
/// away. This is P3 made concrete: "endpoints accept a budget, return a
/// summary, not a firehose."
pub fn write_digest(f: &mut FrameBuf, budget: usize) -> core::fmt::Result {
    let ticks = TICKS.load(Ordering::Relaxed);
    let ecalls = ECALL_TRAPS.load(Ordering::Relaxed);
    let faults = FAULT_TRAPS.load(Ordering::Relaxed);
    let total = RING_COUNT.load(Ordering::Relaxed);
    write!(
        f,
        r#"{{"budget":{budget},"autonomy":"{}","totals":{{"traps":{total},"timer":{ticks},"ecall":{ecalls},"fault":{faults}}},"by_severity":{{"error":{faults},"info":{ecalls},"trace":{ticks}}},"items":["#,
        if autonomous() {
            "autonomous"
        } else {
            "reactive"
        },
    )?;
    let recorded = NOTABLE_COUNT.load(Ordering::Relaxed);
    // Show at most `budget`, and at most what's actually in the notable ring.
    let show = (recorded as usize).min(budget).min(NOTABLE_SIZE);
    for k in 0..show {
        if k > 0 {
            f.write_str(",")?;
        }
        let idx = ((recorded - 1 - k as u64) % NOTABLE_SIZE as u64) as usize;
        let slot = &NOTABLE[idx];
        let sc = slot.scause.load(Ordering::Relaxed);
        write!(
            f,
            r#"{{"id":{},"severity":"{}","cause":"{}","sepc":"0x{:x}"}}"#,
            slot.id.load(Ordering::Relaxed),
            severity_of(sc),
            cause_name(sc),
            slot.sepc.load(Ordering::Relaxed),
        )?;
    }
    // Everything the digest coalesced away: all traps minus the few shown.
    let elided = total.saturating_sub(show as u64);
    write!(f, r#"],"elided":{elided}}}"#)
}

/// The v0 diagnostic frame (P6): everything we know about the fault,
/// structured. M9 grows this into the full frame (page-table walk, richer
/// register file, causal parent), same event shape.
///
/// `pid` is `Some` for a payload fault (origin "payload", kernel survives)
/// and `None` for a kernel fault (origin "kernel", shutdown follows). The
/// frame shape is identical either way — an operator diagnoses both the same.
fn emit_fault(frame: &TrapFrame, id: u64, scause: u64, stval: u64, pid: Option<usize>) {
    if classic_surface() {
        return emit_fault_classic(frame, scause, stval, pid);
    }
    let mut f = FrameBuf::new();
    // caused_by (P12): the payload_start this fault descends from, or null for
    // a kernel fault (no payload cause). Emitted first so the frame is a node
    // in the causal DAG, not an orphan.
    let caused_by = pid.and(crate::payload::current_cause());
    match pid {
        Some(pid) => {
            let _ = write!(
                f,
                r#"{{"id":{id},"type":"fault","origin":"payload","pid":{pid},"#
            );
        }
        None => {
            let _ = write!(f, r#"{{"id":{id},"type":"fault","origin":"kernel","#);
        }
    }
    let _ = f.write_str(r#""caused_by":"#);
    match caused_by {
        Some(ev) => {
            let _ = write!(f, "{ev}");
        }
        None => {
            let _ = f.write_str("null");
        }
    }
    let _ = write!(
        f,
        r#","cause":"0x{scause:x}","cause_name":"{}","sepc":"0x{:x}","stval":"0x{stval:x}","ra":"0x{:x}","sp":"0x{:x}","regs":"#,
        cause_name(scause),
        frame.sepc,
        frame.x(1),
        frame.x(2),
    );
    let _ = write_regs(&mut f, frame);
    let _ = f.write_str(r#","insn":"#);
    if scause == CAUSE_ILLEGAL_INSTRUCTION && stval != 0 {
        // On illegal instruction QEMU puts the offending encoding in stval.
        let _ = write!(
            f,
            r#"{{"bits":"0x{stval:x}","opcode":"0x{:x}","rd":{},"funct3":{},"rs1":{},"rs2":{},"funct7":"0x{:x}""#,
            stval & 0x7f,
            (stval >> 7) & 0x1f,
            (stval >> 12) & 0x7,
            (stval >> 15) & 0x1f,
            (stval >> 20) & 0x1f,
            (stval >> 25) & 0x7f,
        );
        if stval & 0x7f == 0x73 {
            // SYSTEM opcode: the I-immediate is the CSR number — for our
            // deliberate fault this decodes to 0xc00, the read-only cycle
            // counter, which is exactly "why" this instruction is illegal.
            let _ = write!(f, r#","csr":"0x{:x}""#, (stval >> 20) & 0xfff);
        }
        let _ = f.write_str("}");
    } else {
        let _ = f.write_str("null");
    }
    // For a payload page fault, walk its page table for the faulting address:
    // the operator sees exactly which level was invalid or lacked permission
    // (the seed of M9's full P6 diagnostic frame).
    let _ = f.write_str(r#","pagewalk":"#);
    if pid.is_some() && matches!(scause, 12 | 13 | 15) {
        write_pagewalk(&mut f, stval as usize);
    } else {
        let _ = f.write_str("null");
    }
    let _ = f.write_str(r#","ring":"#);
    let _ = write_ring(&mut f);
    let _ = f.write_str("}");
    if f.overflowed() {
        // A fault report that vanishes is the one failure P6 forbids. If the
        // enriched frame (regs + pagewalk + ring) ever exceeds the buffer,
        // emit a minimal frame under the SAME id instead of nothing — the
        // operator still gets origin + cause + PC + faulting address, tagged
        // `overflow` so a reader knows the rich fields were dropped.
        let mut g = FrameBuf::new();
        match pid {
            Some(pid) => {
                let _ = write!(
                    g,
                    r#"{{"id":{id},"type":"fault","origin":"payload","pid":{pid},"#
                );
            }
            None => {
                let _ = write!(g, r#"{{"id":{id},"type":"fault","origin":"kernel","#);
            }
        }
        let _ = write!(
            g,
            r#""overflow":true,"cause_name":"{}","sepc":"0x{:x}","stval":"0x{stval:x}"}}"#,
            cause_name(scause),
            frame.sepc,
        );
        g.emit();
    } else {
        f.emit();
    }
}

/// The 31 GPRs, ABI-named, as a JSON object — P6's full faulting register file.
/// An agent localizing a bug usually needs the argument/temp/saved registers,
/// not just ra/sp; withholding them is exactly the debugger dependency P6 aims
/// to remove.
fn write_regs(f: &mut FrameBuf, frame: &TrapFrame) -> core::fmt::Result {
    // NAMES[i] is the ABI name of x(i+1): NAMES[0]=ra=x1 … NAMES[30]=t6=x31.
    const NAMES: [&str; 31] = [
        "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1", "a2", "a3", "a4", "a5",
        "a6", "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "t3", "t4", "t5",
        "t6",
    ];
    f.write_str("{")?;
    for (i, name) in NAMES.iter().enumerate() {
        if i > 0 {
            f.write_str(",")?;
        }
        write!(f, r#""{name}":"0x{:x}""#, frame.x(i + 1))?;
    }
    f.write_str("}")
}

/// The classic diagnostic surface (P6/E1 twin): the SAME fault as one dense,
/// unstructured printf-style console line — no frame, no id, no register file,
/// no page-table walk, no ring history. Deliberately the poorer surface, so E1
/// can measure how much the structured frame actually buys an agent. Emitted
/// raw (unframed) under masked interrupts so a framed tick can't split it.
fn emit_fault_classic(frame: &TrapFrame, scause: u64, stval: u64, pid: Option<usize>) {
    hal::without_interrupts(|| {
        let origin = if pid.is_some() { "payload" } else { "kernel" };
        let _ = writeln!(
            RawConsole,
            "[FAULT] {origin} {} pc=0x{:x} stval=0x{stval:x} ra=0x{:x} sp=0x{:x}",
            cause_name(scause),
            frame.sepc,
            frame.x(1),
            frame.x(2),
        );
    });
}

/// Append the current payload's page-table walk for `va` as a JSON array of
/// per-level PTEs with decoded permission flags.
fn write_pagewalk(f: &mut FrameBuf, va: usize) {
    let walk = match crate::payload::current_pagewalk(va) {
        Some(w) => w,
        None => {
            let _ = f.write_str("null");
            return;
        }
    };
    let _ = f.write_str("[");
    for (i, (level, pte)) in walk.entries().enumerate() {
        if i > 0 {
            let _ = f.write_str(",");
        }
        let _ = write!(
            f,
            r#"{{"level":{level},"pte":"0x{pte:x}","v":{},"r":{},"w":{},"x":{},"u":{}}}"#,
            pte & 1,
            (pte >> 1) & 1,
            (pte >> 2) & 1,
            (pte >> 3) & 1,
            (pte >> 4) & 1,
        );
    }
    let _ = f.write_str("]");
}
