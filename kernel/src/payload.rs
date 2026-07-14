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
use core::sync::atomic::{AtomicIsize, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};

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
}

// ---- Payload images (statically embedded) -------------------------------

struct Image {
    name: &'static str,
    elf: &'static [u8],
    /// Ceiling of capabilities this image may ever hold.
    caps: u32,
    /// Instruction-count deadline in timebase units (0 = none). Under
    /// -icount, timebase is a deterministic instruction proxy (1 unit ≈ 50
    /// instructions), so this is an instruction budget, not wall time (P9).
    deadline: u64,
}

// build.rs sets these env vars to the payload ELF paths; the payload
// workspace builds before the kernel (`make build`).
static HELLO: &[u8] = include_bytes!(env!("PAYLOAD_HELLO"));
static CRASHER: &[u8] = include_bytes!(env!("PAYLOAD_CRASHER"));
static MUZZLED: &[u8] = include_bytes!(env!("PAYLOAD_MUZZLED"));
static SPAWNER: &[u8] = include_bytes!(env!("PAYLOAD_SPAWNER"));
static CHILD: &[u8] = include_bytes!(env!("PAYLOAD_CHILD"));
static RUNAWAY: &[u8] = include_bytes!(env!("PAYLOAD_RUNAWAY"));

static IMAGES: &[Image] = &[
    Image {
        name: "hello",
        elf: HELLO,
        caps: CAP_WRITE,
        deadline: 0,
    },
    Image {
        name: "crasher",
        elf: CRASHER,
        caps: CAP_WRITE,
        deadline: 0,
    },
    // muzzled: empty CapSet — its write must be refused.
    Image {
        name: "muzzled",
        elf: MUZZLED,
        caps: 0,
        deadline: 0,
    },
    // spawner: may write and spawn, but NOT yield — so a child it spawns can
    // never be granted yield (P10).
    Image {
        name: "spawner",
        elf: SPAWNER,
        caps: CAP_WRITE | CAP_SPAWN,
        deadline: 0,
    },
    // child: may hold write+yield, but is granted only what its parent has.
    Image {
        name: "child",
        elf: CHILD,
        caps: CAP_WRITE | CAP_YIELD,
        deadline: 0,
    },
    // runaway: spins forever; the deadline preempts it (~15k units ≈ 750k insns).
    Image {
        name: "runaway",
        elf: RUNAWAY,
        caps: CAP_WRITE,
        deadline: 15_000,
    },
];

const IMG_HELLO: usize = 0;
const IMG_CRASHER: usize = 1;
const IMG_MUZZLED: usize = 2;
const IMG_SPAWNER: usize = 3;
const IMG_CHILD: usize = 4;
const IMG_RUNAWAY: usize = 5;

/// Map a payload-supplied spawn selector (stable ABI, see payloads/sys) to an
/// image index. Only images a payload is allowed to spawn appear here.
fn spawnable_image(selector: usize) -> Option<usize> {
    match selector {
        0 => Some(IMG_CHILD), // sys::SPAWNABLE_CHILD
        _ => None,
    }
}

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
    /// Timebase value when this payload started running (for deadlines).
    started_at: AtomicU64,
    /// Instruction-count deadline in timebase units (0 = none).
    deadline: AtomicU64,
}

impl Slot {
    const fn new() -> Self {
        Slot {
            state: AtomicU8::new(EMPTY),
            image: AtomicUsize::new(0),
            caps: AtomicU32::new(0),
            parent: AtomicUsize::new(NO_PID),
            exit_code: AtomicIsize::new(0),
            started_at: AtomicU64::new(0),
            deadline: AtomicU64::new(0),
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

fn clear_table() {
    for slot in TABLE.iter() {
        slot.state.store(EMPTY, RE);
    }
}

/// M3 suite (operator-triggered by 'p'): a clean payload and a crashing one.
pub fn seed_suite_m3() {
    clear_table();
    enqueue(IMG_HELLO, IMAGES[IMG_HELLO].caps, NO_PID);
    enqueue(IMG_CRASHER, IMAGES[IMG_CRASHER].caps, NO_PID);
}

/// M4 suite (operator-triggered by 'm'): capability enforcement, spawn
/// attenuation, and deadline kill. `spawner` adds `child` to the queue at
/// run time.
pub fn seed_suite_m4() {
    clear_table();
    enqueue(IMG_MUZZLED, IMAGES[IMG_MUZZLED].caps, NO_PID);
    enqueue(IMG_SPAWNER, IMAGES[IMG_SPAWNER].caps, NO_PID);
    enqueue(IMG_RUNAWAY, IMAGES[IMG_RUNAWAY].caps, NO_PID);
}

fn enqueue(image: usize, caps: u32, parent: usize) -> Option<usize> {
    for (pid, slot) in TABLE.iter().enumerate() {
        if slot.state.load(RE) == EMPTY {
            slot.image.store(image, RE);
            slot.caps.store(caps & IMAGES[image].caps, RE);
            slot.parent.store(parent, RE);
            slot.exit_code.store(0, RE);
            slot.deadline.store(IMAGES[image].deadline, RE);
            slot.started_at.store(0, RE);
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
            TABLE[pid].started_at.store(hal::read_time(), RE);
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

/// Clear the running-payload marker as a payload leaves the CPU. Called from
/// the trap handler's redirect so the interrupts-enabled scheduler window
/// never observes a stale CURRENT (which on_tick could misread).
pub fn leave_current() {
    CURRENT.store(NO_PID, RE);
}

/// Called from the timer handler. Returns true if the current payload has
/// exhausted its instruction-count deadline and must be killed (P1/P2): a
/// runaway payload becomes a structured event, not a hung machine.
pub fn on_tick() -> bool {
    let pid = match current_pid() {
        Some(p) => p,
        None => return false,
    };
    // Only a RUNNING payload can be over budget. Guards against a stale
    // CURRENT (e.g. a tick landing in the scheduler right after a payload
    // left) causing a spurious kill of an already-terminal slot.
    if TABLE[pid].state.load(RE) != RUNNING {
        return false;
    }
    let deadline = TABLE[pid].deadline.load(RE);
    if deadline == 0 {
        return false;
    }
    let elapsed = hal::read_time().saturating_sub(TABLE[pid].started_at.load(RE));
    if elapsed > deadline {
        TABLE[pid].state.store(KILLED, RE);
        emit_killed(pid, elapsed, deadline);
        true
    } else {
        false
    }
}

/// Cooperative yield. Requires Cap::Yield. Pre-paging there is only one
/// resident payload, so a yield has nothing to switch to — it emits an event
/// and resumes the caller. Real rescheduling arrives with M5's per-payload
/// address spaces; the syscall + cap gate are wired now.
pub fn on_yield() -> isize {
    if !current_has(Cap::Yield) {
        emit_denied("yield", "yield");
        return crate::syscall::ENOCAP;
    }
    if let Some(pid) = current_pid() {
        emit_yield(pid);
    }
    0
}

/// Spawn a sub-payload (P10). Requires Cap::Spawn. The child's CapSet is
/// attenuated to `requested & parent_caps & image_ceiling` — a delegation
/// chain can never widen. Returns the child pid or a negative errno.
pub fn on_spawn(selector: usize, requested_caps: u32) -> isize {
    let parent = match current_pid() {
        Some(p) => p,
        None => return crate::syscall::EINVAL,
    };
    if !current_has(Cap::Spawn) {
        emit_denied("spawn", "spawn");
        return crate::syscall::ENOCAP;
    }
    let image = match spawnable_image(selector) {
        Some(i) => i,
        None => return crate::syscall::EINVAL,
    };
    let parent_caps = TABLE[parent].caps.load(RE);
    // The attenuation lattice: granted ⊆ parent, always.
    let granted = requested_caps & parent_caps & IMAGES[image].caps;
    match enqueue(image, granted, parent) {
        Some(child) => {
            emit_spawn(
                parent,
                child,
                IMAGES[image].name,
                parent_caps,
                requested_caps,
                granted,
            );
            child as isize
        }
        None => crate::syscall::EAGAIN,
    }
}

/// Emit a structured capability-denied event (P1/P6): a refused syscall is a
/// first-class event, not just an errno the payload may swallow.
pub fn emit_denied(syscall: &str, cap: &str) {
    let pid = current_pid().unwrap_or(NO_PID);
    hal::without_interrupts(|| {
        let mut f = FrameBuf::new();
        let _ = write!(
            f,
            r#"{{"id":{},"type":"syscall_denied","pid":{pid},"syscall":"{syscall}","reason":"missing_cap","cap":"{cap}"}}"#,
            events::next_id()
        );
        f.emit();
    });
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

fn emit_yield(pid: usize) {
    hal::without_interrupts(|| {
        let mut f = FrameBuf::new();
        let _ = write!(
            f,
            r#"{{"id":{},"type":"payload_yield","pid":{pid}}}"#,
            events::next_id()
        );
        f.emit();
    });
}

/// The delegation event (P10): the parent's CapSet, the requested set, and the
/// granted set, so an operator can verify attenuation directly — granted is
/// always a subset of the parent's caps.
fn emit_spawn(
    parent: usize,
    child: usize,
    name: &str,
    parent_caps: u32,
    requested: u32,
    granted: u32,
) {
    hal::without_interrupts(|| {
        let mut f = FrameBuf::new();
        let _ = write!(
            f,
            r#"{{"id":{},"type":"payload_spawn","parent":{parent},"child":{child},"name":"{name}","parent_caps":"#,
            events::next_id()
        );
        caps_json(&mut f, parent_caps);
        let _ = f.write_str(r#","requested":"#);
        caps_json(&mut f, requested);
        let _ = f.write_str(r#","granted":"#);
        caps_json(&mut f, granted);
        let _ = f.write_str(r#","attenuated":"#);
        let _ = f.write_str(if requested != granted {
            "true"
        } else {
            "false"
        });
        let _ = f.write_str("}");
        f.emit();
    });
}

fn emit_killed(pid: usize, elapsed: u64, deadline: u64) {
    hal::without_interrupts(|| {
        let mut f = FrameBuf::new();
        let _ = write!(
            f,
            r#"{{"id":{},"type":"payload_killed","pid":{pid},"reason":"deadline","elapsed":{elapsed},"deadline":{deadline}}}"#,
            events::next_id()
        );
        f.emit();
    });
}

fn emit_suite_done() {
    hal::without_interrupts(|| {
        let mut f = FrameBuf::new();
        let (mut exited, mut faulted, mut killed) = (0, 0, 0);
        for slot in TABLE.iter() {
            match slot.state.load(RE) {
                EXITED => exited += 1,
                FAULTED => faulted += 1,
                KILLED => killed += 1,
                _ => {}
            }
        }
        let _ = write!(
            f,
            r#"{{"id":{},"type":"suite_done","exited":{exited},"faulted":{faulted},"killed":{killed}}}"#,
            events::next_id()
        );
        f.emit();
    });
}
