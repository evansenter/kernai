#![forbid(unsafe_code)]
//! Payloads: the process table, capability sets, and the (sequential)
//! scheduler. Pre-paging (M5) only one payload is resident in the arena at a
//! time, so payloads run to completion/fault one after another; the run
//! queue is a list of pending images. Concurrent resident payloads arrive
//! with per-payload address spaces at M5.
//!
//! Everything here is scalar atomics in safe code — no `unsafe`, no heap.
//! Consistency is structural (single hart, cooperative): the table is
//! mutated either inside the trap handler (interrupts hardware-off) or in
//! the scheduler between payloads; the two never run concurrently.

use core::fmt::Write;
use core::sync::atomic::{AtomicIsize, AtomicU8, AtomicU32, AtomicUsize, Ordering};

use crate::console::FrameBuf;
use crate::{elf, events, hal};

const RE: Ordering = Ordering::Relaxed;

/// Per-call byte quota for `write` (P3/P7 seed): a hostile payload cannot
/// flood the operator's context window.
pub const WRITE_QUOTA: usize = 256;

// ---- Capabilities (P10 seed) --------------------------------------------

const CAP_WRITE: u32 = 1 << 0;
const CAP_YIELD: u32 = 1 << 1;
const CAP_SPAWN: u32 = 1 << 2;

/// A capability the operator/parent can grant a payload. A child's set is
/// always a subset of its parent's — the kernel enforces the lattice, never
/// widening a delegation chain.
#[derive(Clone, Copy)]
pub enum Cap {
    Write,
    Yield,
    Spawn,
}

impl Cap {
    fn bit(self) -> u32 {
        match self {
            Cap::Write => CAP_WRITE,
            Cap::Yield => CAP_YIELD,
            Cap::Spawn => CAP_SPAWN,
        }
    }
    // kept public-ish for M4 write hook symmetry
    pub const WRITE: Cap = Cap::Write;
}

// ---- Payload images (statically embedded) -------------------------------

struct Image {
    name: &'static str,
    elf: &'static [u8],
    caps: u32,
}

// build.rs sets these env vars to the payload ELF paths; the payload
// workspace builds before the kernel (`make build`).
static HELLO: &[u8] = include_bytes!(env!("PAYLOAD_HELLO"));
static CRASHER: &[u8] = include_bytes!(env!("PAYLOAD_CRASHER"));

static IMAGES: &[Image] = &[
    Image {
        name: "hello",
        elf: HELLO,
        caps: CAP_WRITE,
    },
    Image {
        name: "crasher",
        elf: CRASHER,
        caps: CAP_WRITE,
    },
];

const IMG_HELLO: usize = 0;
const IMG_CRASHER: usize = 1;

// ---- Process table ------------------------------------------------------

const MAX_PROC: usize = 8;
const NO_PID: usize = usize::MAX;

// Process states.
const EMPTY: u8 = 0;
const PENDING: u8 = 1; // queued, not yet started
const RUNNING: u8 = 2;
const EXITED: u8 = 3;
const FAULTED: u8 = 4;
const KILLED: u8 = 5;

struct Slot {
    state: AtomicU8,
    image: AtomicUsize,
    caps: AtomicU32,
    parent: AtomicUsize,
    exit_code: AtomicIsize,
}

impl Slot {
    const fn new() -> Self {
        Slot {
            state: AtomicU8::new(EMPTY),
            image: AtomicUsize::new(0),
            caps: AtomicU32::new(0),
            parent: AtomicUsize::new(NO_PID),
            exit_code: AtomicIsize::new(0),
        }
    }
}

static TABLE: [Slot; MAX_PROC] = [const { Slot::new() }; MAX_PROC];
/// pid of the payload currently on the CPU, or NO_PID in kernel/idle.
static CURRENT: AtomicUsize = AtomicUsize::new(NO_PID);

/// The kernel image must not overlap the payload arena (no paging yet).
/// Panics loudly at boot if the linker ever lets them collide.
pub fn assert_arena_clear() {
    let kend = hal::kernel_end();
    assert!(
        kend <= hal::ARENA_BASE,
        "kernel image overlaps payload arena"
    );
}

/// Seed the run queue with the acceptance suite (operator-triggered by 'p').
/// Clears any prior run's terminal slots first.
pub fn seed_suite() {
    for slot in TABLE.iter() {
        slot.state.store(EMPTY, RE);
    }
    enqueue(IMG_HELLO, IMAGES[IMG_HELLO].caps, NO_PID);
    enqueue(IMG_CRASHER, IMAGES[IMG_CRASHER].caps, NO_PID);
}

fn enqueue(image: usize, caps: u32, parent: usize) -> Option<usize> {
    for (pid, slot) in TABLE.iter().enumerate() {
        if slot.state.load(RE) == EMPTY {
            slot.image.store(image, RE);
            slot.caps.store(caps & IMAGES[image].caps, RE);
            slot.parent.store(parent, RE);
            slot.exit_code.store(0, RE);
            slot.state.store(PENDING, RE);
            return Some(pid);
        }
    }
    None
}

fn take_next_pending() -> Option<usize> {
    TABLE.iter().position(|s| s.state.load(RE) == PENDING)
}

// ---- Scheduler ----------------------------------------------------------

/// Redirect target: the trap handler sends a leaving payload here (S-mode,
/// boot stack) so the scheduler picks the next one. Extern "C" so its
/// address can be installed into a trap frame.
pub extern "C" fn scheduler_resume() -> ! {
    run()
}

/// Pick and start the next pending payload; when the queue drains, announce
/// it and drop to the idle command loop. Diverges either way.
pub fn run() -> ! {
    CURRENT.store(NO_PID, RE);
    match take_next_pending() {
        Some(pid) => start(pid),
        None => {
            emit_suite_done();
            crate::idle()
        }
    }
}

fn start(pid: usize) -> ! {
    let image = TABLE[pid].image.load(RE);
    let img = &IMAGES[image];
    match elf::load(img.elf) {
        Ok(entry) => {
            TABLE[pid].state.store(RUNNING, RE);
            CURRENT.store(pid, RE);
            emit_start(pid, img.name, TABLE[pid].caps.load(RE), entry);
            hal::enter_user(entry)
        }
        Err(e) => {
            TABLE[pid].state.store(FAULTED, RE);
            emit_load_fault(pid, img.name, e.as_str());
            run()
        }
    }
}

// ---- Syscall hooks (called from crate::syscall) -------------------------

pub fn current_pid() -> Option<usize> {
    match CURRENT.load(RE) {
        NO_PID => None,
        pid => Some(pid),
    }
}

pub fn current_has(cap: Cap) -> bool {
    match CURRENT.load(RE) {
        NO_PID => false,
        pid => TABLE[pid].caps.load(RE) & cap.bit() != 0,
    }
}

pub fn on_exit(code: usize) {
    if let NO_PID = CURRENT.load(RE) {
        return;
    }
    let pid = CURRENT.load(RE);
    TABLE[pid].exit_code.store(code as isize, RE);
    TABLE[pid].state.store(EXITED, RE);
    emit_exit(pid, code as isize);
}

pub fn mark_faulted() {
    if let Some(pid) = current_pid() {
        TABLE[pid].state.store(FAULTED, RE);
    }
}

/// Called from the timer handler. Returns true if the current payload has
/// exhausted its instruction-count deadline and should be killed. M3 has no
/// deadlines (returns false); M4 wires the budget check here.
pub fn on_tick() -> bool {
    false
}

/// M4 implements cooperative yield and capability-attenuated spawn; in M3
/// they are unimplemented so payloads that call them see ENOSYS.
pub fn on_yield() -> isize {
    crate::syscall::ENOSYS
}

pub fn on_spawn(_image: usize, _caps: u32) -> isize {
    crate::syscall::ENOSYS
}

/// Copy `len` bytes of payload memory at virtual `ptr` and emit them as an
/// untrusted output event (P7). `ptr` is an arena address (identity-mapped).
pub fn emit_output(ptr: usize, len: usize) -> Result<(), ()> {
    let mut buf = [0u8; WRITE_QUOTA];
    let n = len.min(WRITE_QUOTA);
    let offset = ptr.checked_sub(hal::ARENA_BASE).ok_or(())?;
    if !hal::arena_read(offset, &mut buf[..n]) {
        return Err(());
    }
    let pid = current_pid().unwrap_or(NO_PID);
    hal::without_interrupts(|| {
        let mut f = FrameBuf::new();
        // "untrusted":true and the bytes confined to a JSON string are the
        // P7 provenance seam: payload output can never be read by the
        // operator as kernel-issued instructions.
        let _ = write!(
            f,
            r#"{{"id":{},"type":"payload_output","pid":{pid},"untrusted":true,"len":{n},"data":""#,
            events::next_id()
        );
        let _ = f.write_json_escaped_bytes(&buf[..n]);
        let _ = f.write_str(r#""}"#);
        f.emit();
    });
    Ok(())
}

// ---- Events -------------------------------------------------------------

fn caps_json(f: &mut FrameBuf, caps: u32) {
    let names: [(&str, u32); 3] = [
        ("write", CAP_WRITE),
        ("yield", CAP_YIELD),
        ("spawn", CAP_SPAWN),
    ];
    let _ = f.write_str("[");
    let mut first = true;
    for (name, bit) in names {
        if caps & bit != 0 {
            if !first {
                let _ = f.write_str(",");
            }
            let _ = write!(f, "\"{name}\"");
            first = false;
        }
    }
    let _ = f.write_str("]");
}

fn emit_start(pid: usize, name: &str, caps: u32, entry: usize) {
    hal::without_interrupts(|| {
        let mut f = FrameBuf::new();
        let _ = write!(
            f,
            r#"{{"id":{},"type":"payload_start","pid":{pid},"name":"{name}","entry":"0x{entry:x}","caps":"#,
            events::next_id()
        );
        caps_json(&mut f, caps);
        let _ = f.write_str("}");
        f.emit();
    });
}

fn emit_exit(pid: usize, code: isize) {
    hal::without_interrupts(|| {
        let mut f = FrameBuf::new();
        let _ = write!(
            f,
            r#"{{"id":{},"type":"payload_exit","pid":{pid},"code":{code}}}"#,
            events::next_id()
        );
        f.emit();
    });
}

fn emit_load_fault(pid: usize, name: &str, reason: &str) {
    hal::without_interrupts(|| {
        let mut f = FrameBuf::new();
        let _ = write!(
            f,
            r#"{{"id":{},"type":"payload_load_fault","pid":{pid},"name":"{name}","reason":"{reason}"}}"#,
            events::next_id()
        );
        f.emit();
    });
}

fn emit_suite_done() {
    hal::without_interrupts(|| {
        let mut f = FrameBuf::new();
        let mut ran = 0;
        let mut faulted = 0;
        for slot in TABLE.iter() {
            match slot.state.load(RE) {
                EXITED => ran += 1,
                FAULTED | KILLED => faulted += 1,
                _ => {}
            }
        }
        let _ = write!(
            f,
            r#"{{"id":{},"type":"suite_done","exited":{ran},"faulted":{faulted}}}"#,
            events::next_id()
        );
        f.emit();
    });
}

// Keep the yet-unused cap variants/consts referenced so M4 wiring compiles
// cleanly and clippy stays quiet without allow-dead-code littering.
const _: (u32, u32) = (CAP_YIELD, CAP_SPAWN);
const _: [Cap; 2] = [Cap::Yield, Cap::Spawn];
