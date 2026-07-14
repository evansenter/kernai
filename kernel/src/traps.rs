#![forbid(unsafe_code)]
//! Trap dispatch, timer cadence, and the trap ring buffer.
//!
//! The ring is the first sliver of P11 (no kernel state observable only via
//! debugger): every trap is recorded, the last RING_SIZE are queryable over
//! serial, and the fault report carries them as history. The fault report
//! itself is the v0 P6 diagnostic frame — crude, but structured: cause,
//! sepc, decoded instruction fields, recent trap history.

use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::console::FrameBuf;
use crate::{events, hal};

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
        let mut f = FrameBuf::new();
        let _ = write!(
            f,
            r#"{{"id":{id},"type":"tick","seq":{seq},"time":{}}}"#,
            hal::read_time()
        );
        f.emit();
        // M4 seam: a running payload's instruction-count deadline is checked
        // here; an over-budget payload is redirected to the scheduler
        // (crate::payload::on_tick returns whether it killed the current one).
        if crate::payload::on_tick() {
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

/// The v0 diagnostic frame (P6): everything we know about the fault,
/// structured. M9 grows this into the full frame (page-table walk, richer
/// register file, causal parent), same event shape.
///
/// `pid` is `Some` for a payload fault (origin "payload", kernel survives)
/// and `None` for a kernel fault (origin "kernel", shutdown follows). The
/// frame shape is identical either way — an operator diagnoses both the same.
fn emit_fault(frame: &TrapFrame, id: u64, scause: u64, stval: u64, pid: Option<usize>) {
    let mut f = FrameBuf::new();
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
    let _ = write!(
        f,
        r#""cause":"0x{scause:x}","cause_name":"{}","sepc":"0x{:x}","stval":"0x{stval:x}","ra":"0x{:x}","sp":"0x{:x}","insn":"#,
        cause_name(scause),
        frame.sepc,
        frame.x(1),
        frame.x(2),
    );
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
    let _ = f.write_str(r#","ring":"#);
    let _ = write_ring(&mut f);
    let _ = f.write_str("}");
    f.emit();
}
